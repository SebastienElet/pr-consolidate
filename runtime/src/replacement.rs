use crate::context::is_sha;
use crate::error::{Error, Result, require};
use crate::forge::{Forge, string};
use crate::plan::{numeric, read_json, scalar};
use crate::review::approval_claim;
use crate::transport::Api;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
struct MergedLot {
    id: u64,
    merge: String,
    head: String,
}
struct Item {
    id: u64,
    head: String,
    extra: String,
}
#[derive(Debug, PartialEq, Eq)]
struct Checked {
    body: String,
    state: String,
    head: String,
    present: Vec<u64>,
}

fn positive(value: &Value) -> Result<u64> {
    let value = scalar(value);
    require(
        numeric(&value) && !value.starts_with('0'),
        "plan requires a positive PR ID",
    )?;
    value
        .parse()
        .map_err(|_| Error::Refused("invalid PR ID".into()))
}
fn load(path: &Path) -> Result<(u64, Vec<Item>)> {
    let plan = read_json(path)?;
    require(
        plan.as_object().is_some_and(|o| {
            o.keys()
                .all(|k| ["lot_pr", "items", "note"].contains(&k.as_str()))
        }),
        "plan must be {lot_pr, items[, note]}",
    )?;
    let lot = positive(&plan["lot_pr"])?;
    let items = plan["items"]
        .as_array()
        .ok_or_else(|| Error::Refused("plan needs a non-empty items list".into()))?;
    require(!items.is_empty(), "plan needs a non-empty items list")?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for item in items {
        require(
            item.as_object().is_some_and(|o| {
                o.keys()
                    .all(|k| ["id", "head", "extra", "note"].contains(&k.as_str()))
            }) && item["head"].as_str().is_some_and(|h| is_sha(h, 12, 40))
                && item.get("extra").is_none_or(Value::is_string),
            "bad plan item: id, head (12-40 hex) and an optional extra string",
        )?;
        let id = positive(&item["id"])?;
        require(
            id != lot && seen.insert(id),
            format!("plan item #{id} is the lot or listed twice"),
        )?;
        result.push(Item {
            id,
            head: item["head"].as_str().unwrap().into(),
            extra: item["extra"].as_str().unwrap_or("").trim().into(),
        });
    }
    Ok((lot, result))
}

impl<A: Api> Forge<A> {
    fn merged_lot(&self, id: u64) -> Result<MergedLot> {
        let data = self.get(&format!("pullrequests/{id}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name"))?;
        let merge = string(&data, "/merge_commit/hash");
        let head = string(&data, "/source/commit/hash");
        require(
            data["state"] == "MERGED" && is_sha(merge, 7, 40),
            format!("lot #{id} is not MERGED"),
        )?;
        self.destination(&data, id)?;
        require(
            is_sha(head, 12, 40),
            format!("lot #{id} has no valid source head"),
        )?;
        Ok(MergedLot {
            id,
            merge: merge.into(),
            head: head.into(),
        })
    }
    fn checked_item(&self, item: &Item, lot: &MergedLot, account: &Value) -> Result<Checked> {
        let current = self.get(&format!(
            "pullrequests/{}?fields=state,source.commit.hash,destination.branch.name",
            item.id
        ))?;
        self.destination(&current, item.id)?;
        let head = string(&current, "/source/commit/hash");
        let state = current["state"].as_str().unwrap_or("");
        require(
            matches!(state, "OPEN" | "DECLINED")
                && is_sha(head, 40, 40)
                && head.starts_with(&item.head),
            format!(
                "#{}: state {state} head {head} (expected OPEN {})",
                item.id, item.head
            ),
        )?;
        let base = self.get(&format!("merge-base/{head}..{}", lot.head))?;
        require(
            base["hash"]
                .as_str()
                .is_some_and(|h| is_sha(h, 40, 40) && h == head),
            format!(
                "#{}: head {head} is not an ancestor of the lot head {} (merge-base is not the exact source head)",
                item.id, lot.head
            ),
        )?;
        let extra = if item.extra.is_empty() {
            String::new()
        } else {
            format!("{} ", item.extra)
        };
        let body = format!(
            "Included in batch #{} (merged into `{}`, `{}`): this PR head `{}` is an ancestor of the merged batch candidate; its commits are preserved unchanged. {extra}Closed as replaced by #{}.",
            lot.id,
            self.context.base_branch,
            &lot.merge[..8.min(lot.merge.len())],
            &head[..8],
            lot.id
        );
        require(
            !approval_claim(&body),
            format!(
                "#{}: the comment would read as an approval claim; reword its extra",
                item.id
            ),
        )?;
        self.require_review(item.id, head, account)?;
        let present = self.exact_comments(item.id, &body, account)?;
        require(
            state != "DECLINED" || !present.is_empty(),
            format!(
                "#{}: state DECLINED without the exact owned replacement comment",
                item.id
            ),
        )?;
        Ok(Checked {
            body,
            state: state.into(),
            head: head.into(),
            present,
        })
    }
    pub fn replace(&self, path: &Path, mode: &str) -> Result<i32> {
        require(
            matches!(mode, "dry-run" | "apply"),
            "mode must be dry-run or apply",
        )?;
        let (lot_id, items) = load(path)?;
        let lot = self.merged_lot(lot_id)?;
        let account = self.account()?;
        println!(
            "lot #{lot_id} MERGED merge_commit {} head {}",
            lot.merge, lot.head
        );
        for item in &items {
            let checked = self.checked_item(item, &lot, &account)?;
            println!(
                "#{} OK ({}, lot #{lot_id} merged {}) -> {}",
                item.id,
                checked.state,
                &lot.merge[..8.min(lot.merge.len())],
                checked.body
            );
        }
        if mode == "dry-run" {
            println!("dry-run: nothing written");
            return Ok(0);
        }
        for item in &items {
            require(
                self.merged_lot(lot_id)? == lot,
                format!("lot #{lot_id} changed during decline"),
            )?;
            let checked = self.checked_item(item, &lot, &account)?;
            if checked.state == "DECLINED" {
                println!(
                    "#{} already declined with exact owned replacement comment; resumed",
                    item.id
                );
                continue;
            }
            if checked.present.is_empty() {
                if self.post_comment(item.id, &checked.body)? != 0
                    || self
                        .exact_comments(item.id, &checked.body, &account)?
                        .is_empty()
                {
                    return Err(Error::Failed(format!(
                        "#{}: replacement comment write not observed; inspect before retry",
                        item.id
                    )));
                }
            } else {
                println!(
                    "#{} comment already present ({}), not posted again",
                    item.id, checked.present[0]
                );
            }
            let fresh = self.checked_item(item, &lot, &account)?;
            require(
                fresh.body == checked.body
                    && fresh.state == "OPEN"
                    && fresh.head == checked.head
                    && !fresh.present.is_empty(),
                format!("#{}: changed before decline", item.id),
            )?;
            self.api(
                &format!("pullrequests/{}/decline", item.id),
                "POST",
                None,
                120,
            )?;
            let after = self.get(&format!(
                "pullrequests/{}?fields=state,source.commit.hash,destination.branch.name",
                item.id
            ))?;
            println!("#{} declined -> state {}", item.id, after["state"]);
            if after["state"] != "DECLINED" || string(&after, "/source/commit/hash") != checked.head
            {
                return Err(Error::Failed(format!(
                    "#{}: decline write not observed at the expected head; inspect before retry",
                    item.id
                )));
            }
        }
        Ok(0)
    }
}
