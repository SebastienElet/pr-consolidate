use crate::context::{ForgeKind, is_sha};
use crate::error::{Error, Result, require};
use crate::forge::{Forge, read_text, string};
use crate::transport::Api;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

pub fn approval_claim(body: &str) -> bool {
    Regex::new(r"(?i)approve|merge[\s_-]*ready")
        .unwrap()
        .is_match(body)
}
pub fn review_marker(pr: u64, head: &str) -> String {
    format!("<!-- pr-consolidate:review pr={pr} head={head} -->")
}

pub fn own_general(comment: &Value, account: &Value) -> bool {
    let key = if account.is_u64() { "id" } else { "uuid" };
    comment["user"][key] == *account
        && comment["parent"].is_null()
        && comment["inline"].is_null()
        && comment["deleted"] != true
        && (comment["_surface"] != "review"
            || matches!(
                comment["review_state"].as_str(),
                Some("COMMENTED" | "APPROVED" | "CHANGES_REQUESTED")
            ))
}

impl<A: Api> Forge<A> {
    pub fn account(&self) -> Result<Value> {
        let user = self.get("/user")?;
        if self.context.forge == ForgeKind::GitHub {
            require(
                user["id"].as_u64().is_some_and(|n| n > 0),
                "connected GitHub account ID is missing or invalid",
            )?;
            Ok(user["id"].clone())
        } else {
            require(
                user["uuid"].as_str().is_some_and(|s| {
                    Regex::new(r"\A\{[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\}\z")
                        .unwrap()
                        .is_match(s)
                }),
                "connected account UUID is missing or invalid",
            )?;
            Ok(user["uuid"].clone())
        }
    }
    pub fn comments(&self, pr: u64) -> Result<Vec<Value>> {
        let values = self.paged(&format!("pullrequests/{pr}/comments?pagelen=100&fields=size,next,values.id,values.deleted,values.content.raw,values.user.uuid,values.parent.id,values.inline"), 60)?;
        let mut ids = HashSet::new();
        for value in &values {
            require(
                value["id"].as_u64().is_some_and(|n| n > 0)
                    && ids.insert((
                        value["_surface"].as_str().unwrap_or("comment"),
                        value["id"].as_u64(),
                    )),
                format!("#{pr}: invalid or repeated comment IDs"),
            )?;
            require(
                value.get("deleted").is_none_or(Value::is_boolean),
                format!("#{pr}: invalid comment deletion state"),
            )?;
            if value["deleted"] == true {
                continue;
            }
            require(
                value["content"]["raw"].is_string()
                    && value.get("user").is_none_or(Value::is_object),
                format!("#{pr}: invalid comment content or author"),
            )?;
        }
        Ok(values)
    }
    pub fn marked_comment(&self, pr: u64, head: &str, account: &Value) -> Result<Option<Value>> {
        let marker = review_marker(pr, head);
        let mut found = Vec::new();
        for value in self.comments(pr)? {
            let raw = string(&value, "/content/raw");
            if value["deleted"] == true || !raw.contains(&marker) {
                continue;
            }
            require(
                raw.lines().next() == Some(&marker)
                    && raw.matches("<!-- pr-consolidate:review ").count() == 1,
                format!("#{pr}: review marker is misplaced or ambiguous"),
            )?;
            if value["_surface"] == "review" {
                require(
                    matches!(
                        value["review_state"].as_str(),
                        Some("COMMENTED" | "APPROVED" | "CHANGES_REQUESTED")
                    ) && value["commit_id"] == head,
                    format!(
                        "#{pr}: review summary is pending, dismissed or belongs to another head"
                    ),
                )?;
            }
            found.push(value);
        }
        require(
            found.len() <= 1 && found.iter().all(|v| own_general(v, account)),
            format!("#{pr}: review marker has ambiguous ownership or duplicates"),
        )?;
        Ok(found.pop())
    }
    pub fn check_head(&self, pr: u64, head: &str) -> Result<()> {
        let data = self.get(&format!(
            "pullrequests/{pr}?fields=state,source.commit.hash"
        ))?;
        require(
            data["state"] == "OPEN" && string(&data, "/source/commit/hash") == head,
            format!(
                "#{pr}: expected OPEN head {head}, observed {} {}",
                data["state"],
                string(&data, "/source/commit/hash")
            ),
        )
    }
    pub fn exact_comments(&self, pr: u64, body: &str, account: &Value) -> Result<Vec<u64>> {
        let matches: Vec<_> = self
            .comments(pr)?
            .into_iter()
            .filter(|v| v["deleted"] != true && string(v, "/content/raw") == body)
            .collect();
        require(
            matches.len() <= 1 && matches.iter().all(|v| own_general(v, account)),
            format!("#{pr}: replacement comment has ambiguous ownership or duplicates"),
        )?;
        Ok(matches.iter().map(|v| v["id"].as_u64().unwrap()).collect())
    }
    pub fn require_review(&self, pr: u64, head: &str, account: &Value) -> Result<()> {
        require(
            is_sha(head, 40, 40) && self.marked_comment(pr, head, account)?.is_some(),
            format!("#{pr}: missing owned general review comment for full head {head}"),
        )
    }
    pub fn post_comment(&self, pr: u64, body: &str) -> Result<i32> {
        require(
            !approval_claim(body),
            format!("#{pr}: the comment would read as an approval claim"),
        )?;
        self.api(
            &format!("pullrequests/{pr}/comments"),
            "POST",
            Some(&json!({"content":{"raw":body}})),
            120,
        )?;
        Ok(0)
    }
    pub fn review_comment(&self, pr: u64, head: &str, body: &Path, mode: &str) -> Result<i32> {
        require(
            matches!(mode, "dry-run" | "apply") && is_sha(head, 40, 40),
            "review-comment requires a full 40-character SHA and dry-run or apply",
        )?;
        let body = read_text(body)?;
        let body = body.trim();
        require(
            !body.is_empty() && !approval_claim(body) && !body.contains("<!-- pr-consolidate:"),
            "review body is empty, contains a reserved marker or an approval claim",
        )?;
        let account = self.account()?;
        let raw = format!(
            "{}\n\nReviewed head: `{head}`.\n\n{body}",
            review_marker(pr, head)
        );
        self.check_head(pr, head)?;
        let found = self.marked_comment(pr, head, &account)?;
        self.check_head(pr, head)?;
        let action = if found
            .as_ref()
            .is_some_and(|v| string(v, "/content/raw") == raw)
        {
            "unchanged"
        } else if found.is_some() {
            "update"
        } else {
            "create"
        };
        println!("#{pr} review-comment {head}: {action} ({mode})");
        if mode == "dry-run" || action == "unchanged" {
            return Ok(0);
        }
        let found = self.marked_comment(pr, head, &account)?;
        self.check_head(pr, head)?;
        let surface = if found.as_ref().is_some_and(|v| v["_surface"] == "review") {
            "reviews"
        } else {
            "comments"
        };
        let suffix = found
            .as_ref()
            .map(|v| format!("/{}", v["id"]))
            .unwrap_or_default();
        self.api(
            &format!("pullrequests/{pr}/{surface}{suffix}"),
            if found.is_some() { "PUT" } else { "POST" },
            Some(&json!({"content":{"raw":raw}})),
            120,
        )?;
        let observed = self.marked_comment(pr, head, &account)?;
        if observed
            .as_ref()
            .is_none_or(|v| string(v, "/content/raw") != raw)
        {
            return Err(Error::Failed(format!(
                "#{pr}: review comment write not observed; inspect before retry"
            )));
        }
        self.check_head(pr, head)?;
        println!(
            "#{pr} review-comment observed as comment {}",
            observed.unwrap()["id"]
        );
        Ok(0)
    }
}
