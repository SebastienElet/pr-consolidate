use crate::context::{Context, ForgeKind};
use crate::error::{Error, Result};
use crate::forge::Forge;
use crate::transport::Api;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LOT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const UUID: &str = "{11111111-2222-3333-4444-555555555555}";

#[derive(Default)]
struct Memory {
    comments: BTreeMap<u64, Vec<Value>>,
    pulls: BTreeMap<u64, Value>,
    calls: Vec<(String, String, Option<Value>)>,
    account: Option<Value>,
    ancestry: Option<String>,
    fault: Option<Error>,
    invisible: bool,
    moved_after_post: bool,
    move_on_comments: bool,
    decline_fault: Option<Error>,
}
struct MemoryApi(RefCell<Memory>);
impl Api for MemoryApi {
    fn api(&self, path: &str, method: &str, payload: Option<&Value>, _: u64) -> Result<Value> {
        let mut state = self.0.borrow_mut();
        state
            .calls
            .push((path.into(), method.into(), payload.cloned()));
        if path == "/user" {
            return Ok(state.account.clone().unwrap_or(json!({"uuid":UUID})));
        }
        if path.starts_with("merge-base/") {
            return Ok(json!({"hash":state.ancestry.as_deref().unwrap_or(HEAD)}));
        }
        let endpoint = path.split('?').next().unwrap();
        let parts: Vec<_> = endpoint.split('/').collect();
        let id: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        if parts.len() == 2 && method == "GET" {
            return Ok(state
                .pulls
                .get(&id)
                .cloned()
                .unwrap_or(pull(id, HEAD, "OPEN")));
        }
        if parts.get(2) == Some(&"comments") || parts.get(2) == Some(&"reviews") {
            if method == "GET" {
                let values = state.comments.get(&id).cloned().unwrap_or_default();
                if state.move_on_comments {
                    state.pulls.insert(id, pull(id, LOT, "OPEN"));
                }
                return Ok(json!({"size":values.len(),"values":values}));
            }
            let fault = state.fault.take();
            if !state.invisible {
                let comments = state.comments.entry(id).or_default();
                let raw = payload.unwrap()["content"]["raw"].as_str().unwrap();
                if method == "PUT" {
                    let cid: u64 = parts[3].parse().unwrap();
                    comments.iter_mut().find(|v| v["id"] == cid).unwrap()["content"]["raw"] =
                        json!(raw);
                } else {
                    let cid = comments.len() as u64 + 1;
                    comments.push(comment(cid, raw));
                }
            }
            if state.moved_after_post {
                state.pulls.insert(id, pull(id, LOT, "OPEN"));
            }
            if let Some(error) = fault {
                return Err(error);
            }
            return Ok(json!({"id":1}));
        }
        if parts.get(2) == Some(&"decline") {
            let p = state
                .pulls
                .entry(id)
                .or_insert_with(|| pull(id, HEAD, "OPEN"));
            p["state"] = json!("DECLINED");
            let result = p.clone();
            if let Some(error) = state.decline_fault.take() {
                return Err(error);
            }
            return Ok(result);
        }
        panic!("unexpected request: {method} {path}");
    }
}
fn pull(id: u64, head: &str, state: &str) -> Value {
    json!({"id":id,"state":state,"source":{"commit":{"hash":head},"branch":{"name":"feature"}},"destination":{"branch":{"name":"trunk"}},"merge_commit":{"hash":"cccccccccccccccccccccccccccccccccccccccc"}})
}
fn comment(id: u64, raw: &str) -> Value {
    json!({"id":id,"content":{"raw":raw},"user":{"uuid":UUID}})
}
fn context() -> Context {
    Context {
        root: "/tmp".into(),
        repository: "owner/project".into(),
        forge: ForgeKind::Bitbucket,
        remote_url: "git@bitbucket.org:owner/project.git".into(),
        base_branch: "trunk".into(),
    }
}
fn forge() -> Forge<MemoryApi> {
    Forge {
        context: context(),
        transport: MemoryApi(RefCell::new(Memory::default())),
    }
}
fn writes(forge: &Forge<MemoryApi>) -> usize {
    forge
        .transport
        .0
        .borrow()
        .calls
        .iter()
        .filter(|(_, method, _)| method != "GET")
        .count()
}

#[test]
fn same_head_review_creates_once_and_updates_owned_comment() {
    let forge = forge();
    let dir = tempfile::tempdir().unwrap();
    let body = dir.path().join("review.md");
    std::fs::write(&body, "No demonstrated defect.").unwrap();
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    assert_eq!(writes(&forge), 1);
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    assert_eq!(writes(&forge), 1);
    std::fs::write(&body, "Updated evidence.").unwrap();
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    assert_eq!(writes(&forge), 2);
    assert_eq!(forge.transport.0.borrow().comments[&1].len(), 1);
}

fn replacement_fixture() -> (Forge<MemoryApi>, tempfile::TempDir, std::path::PathBuf) {
    let forge = forge();
    {
        let mut state = forge.transport.0.borrow_mut();
        state.pulls.insert(99, pull(99, LOT, "MERGED"));
        for id in [1, 2] {
            state.comments.insert(
                id,
                vec![comment(
                    1,
                    &format!("{}\n\nReview", crate::review::review_marker(id, HEAD)),
                )],
            );
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replace.json");
    std::fs::write(
        &path,
        json!({"lot_pr":99,"items":[{"id":1,"head":HEAD},{"id":2,"head":HEAD}]}).to_string(),
    )
    .unwrap();
    (forge, dir, path)
}

#[test]
fn replacement_comments_before_closing_and_resumes_without_duplicate_writes() {
    let (forge, _dir, path) = replacement_fixture();
    assert_eq!(forge.replace(&path, "apply"), Ok(0));
    assert_eq!(writes(&forge), 4);
    let actions: Vec<_> = forge
        .transport
        .0
        .borrow()
        .calls
        .iter()
        .filter(|(_, m, _)| m != "GET")
        .map(|(p, _, _)| p.clone())
        .collect();
    assert_eq!(
        actions,
        [
            "pullrequests/1/comments",
            "pullrequests/1/decline",
            "pullrequests/2/comments",
            "pullrequests/2/decline"
        ]
    );
    assert_eq!(forge.replace(&path, "apply"), Ok(0));
    assert_eq!(writes(&forge), 4);
}

struct FixedApi(Value);
impl Api for FixedApi {
    fn api(&self, _: &str, _: &str, _: Option<&Value>, _: u64) -> Result<Value> {
        Ok(self.0.clone())
    }
}
#[derive(Default)]
struct TestClock(std::cell::Cell<u64>);
impl crate::ci::Clock for TestClock {
    fn now(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.0.get())
    }
    fn sleep(&self, duration: std::time::Duration) {
        self.0.set(self.0.get() + duration.as_secs().max(1));
    }
}

#[test]
fn all_skipped_github_checks_are_not_success() {
    let mut context = context();
    context.forge = ForgeKind::GitHub;
    let forge = Forge {
        context,
        transport: FixedApi(
            json!({"sha":HEAD,"checks":[{"status":"completed","conclusion":"skipped"}]}),
        ),
    };
    assert_eq!(forge.wait_ci(HEAD, &TestClock::default(), 2, 1), Ok(1));
}

struct FnApi<F>(RefCell<F>);
impl<F> Api for FnApi<F>
where
    F: FnMut(&str, &str, Option<&Value>, u64) -> Result<Value>,
{
    fn api(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        (self.0.borrow_mut())(path, method, payload, timeout)
    }
}
fn body_file() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("body.md");
    std::fs::write(&path, "No demonstrated defect.").unwrap();
    (dir, path)
}
fn marked(id: u64, head: &str) -> Value {
    comment(
        id,
        &format!("{}\n\nReview", crate::review::review_marker(1, head)),
    )
}

#[test]
fn new_head_preserves_historical_review() {
    let forge = forge();
    let (_dir, body) = body_file();
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    forge
        .transport
        .0
        .borrow_mut()
        .pulls
        .insert(1, pull(1, LOT, "OPEN"));
    assert_eq!(forge.review_comment(1, LOT, &body, "apply"), Ok(0));
    let state = forge.transport.0.borrow();
    assert_eq!(state.comments[&1].len(), 2);
    assert!(
        state.comments[&1][0]["content"]["raw"]
            .as_str()
            .unwrap()
            .contains(HEAD)
    );
}
#[test]
fn review_dry_run_does_not_write() {
    let forge = forge();
    let (_dir, body) = body_file();
    assert_eq!(forge.review_comment(1, HEAD, &body, "dry-run"), Ok(0));
    assert_eq!(writes(&forge), 0);
}
#[test]
fn moved_and_abbreviated_review_heads_refuse() {
    for head in [&HEAD[..12], LOT] {
        let forge = forge();
        let (_dir, body) = body_file();
        assert!(matches!(
            forge.review_comment(1, head, &body, "apply"),
            Err(Error::Refused(_))
        ));
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn review_checks_head_after_comment_read_even_for_dry_run_and_unchanged() {
    for mode in ["apply", "dry-run"] {
        let forge = forge();
        let (_dir, body) = body_file();
        assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
        forge.transport.0.borrow_mut().move_on_comments = true;
        assert!(matches!(
            forge.review_comment(1, HEAD, &body, mode),
            Err(Error::Refused(_))
        ));
        assert_eq!(writes(&forge), 1);
    }
}
#[test]
fn quoted_misplaced_and_repeated_review_markers_refuse() {
    let marker = crate::review::review_marker(1, HEAD);
    for raw in [
        format!("Quoted\n{marker}"),
        format!(" {marker}"),
        format!("{marker}\n{marker}"),
    ] {
        let forge = forge();
        let (_dir, body) = body_file();
        forge
            .transport
            .0
            .borrow_mut()
            .comments
            .insert(1, vec![comment(1, &raw)]);
        assert!(matches!(
            forge.review_comment(1, HEAD, &body, "apply"),
            Err(Error::Refused(_))
        ));
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn empty_approval_reserved_and_unreadable_bodies_refuse() {
    for raw in [
        "",
        " \n",
        "Approved",
        "Merge_ready",
        "<!-- pr-consolidate:other -->",
    ] {
        let forge = forge();
        let (_dir, body) = body_file();
        std::fs::write(&body, raw).unwrap();
        assert!(matches!(
            forge.review_comment(1, HEAD, &body, "apply"),
            Err(Error::Refused(_))
        ));
        assert_eq!(writes(&forge), 0);
    }
    let forge = forge();
    assert!(matches!(
        forge.review_comment(
            1,
            HEAD,
            std::path::Path::new("/missing-review-file"),
            "apply"
        ),
        Err(Error::Refused(_))
    ));
}
#[test]
fn foreign_missing_author_reply_inline_deleted_and_duplicate_reviews_refuse() {
    for fault in [
        "foreign",
        "missing",
        "reply",
        "inline",
        "duplicate",
        "malformed",
        "deletion",
    ] {
        let forge = forge();
        let (_dir, body) = body_file();
        let mut value = marked(1, HEAD);
        match fault {
            "foreign" => value["user"]["uuid"] = json!("foreign"),
            "missing" => {
                value.as_object_mut().unwrap().remove("user");
            }
            "reply" => value["parent"] = json!({"id":9}),
            "inline" => value["inline"] = json!({"path":"src.rs"}),
            "malformed" => value["content"] = json!({"raw":true}),
            "deletion" => value["deleted"] = json!("false"),
            _ => {}
        }
        let mut values = vec![value];
        if fault == "duplicate" {
            values.push(marked(2, HEAD));
        }
        forge.transport.0.borrow_mut().comments.insert(1, values);
        assert!(
            matches!(
                forge.review_comment(1, HEAD, &body, "apply"),
                Err(Error::Refused(_))
            ),
            "{fault}"
        );
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn deleted_review_is_ignored_and_does_not_replace_current_review() {
    let forge = forge();
    let (_dir, body) = body_file();
    let mut deleted = marked(1, HEAD);
    deleted["deleted"] = json!(true);
    forge
        .transport
        .0
        .borrow_mut()
        .comments
        .insert(1, vec![deleted]);
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    assert_eq!(forge.transport.0.borrow().comments[&1].len(), 2);
}
#[test]
fn missing_connected_account_refuses_both_forges() {
    for kind in [ForgeKind::GitHub, ForgeKind::Bitbucket] {
        for user in [
            json!({}),
            json!({"id":true,"uuid":true}),
            json!({"id":"7","uuid":"bad"}),
            json!({"id":0}),
        ] {
            let mut forge = forge();
            forge.context.forge = kind.clone();
            let (_dir, body) = body_file();
            forge.transport.0.borrow_mut().account = Some(user);
            assert!(matches!(
                forge.review_comment(1, HEAD, &body, "apply"),
                Err(Error::Refused(_))
            ));
            assert_eq!(writes(&forge), 0);
        }
    }
}
#[test]
fn unobserved_review_write_fails_and_moved_postpublication_head_refuses() {
    let forge = forge();
    let (_dir, body) = body_file();
    forge.transport.0.borrow_mut().invisible = true;
    assert!(matches!(
        forge.review_comment(1, HEAD, &body, "apply"),
        Err(Error::Failed(_))
    ));
    forge.transport.0.borrow_mut().invisible = false;
    forge.transport.0.borrow_mut().moved_after_post = true;
    assert!(matches!(
        forge.review_comment(1, HEAD, &body, "apply"),
        Err(Error::Refused(_))
    ));
}
#[test]
fn accepted_review_write_with_timeout_or_transport_failure_reconciles_before_retry() {
    for error in [
        Error::Timeout("ambiguous write".into()),
        Error::Failed("ambiguous write".into()),
    ] {
        let forge = forge();
        let (_dir, body) = body_file();
        forge.transport.0.borrow_mut().fault = Some(error.clone());
        assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Err(error));
        assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
        assert_eq!(writes(&forge), 1);
        assert_eq!(forge.transport.0.borrow().comments[&1].len(), 1);
    }
}
#[test]
fn github_review_summary_requires_owned_public_state_and_matching_head() {
    for fault in [
        "pending",
        "dismissed",
        "unknown",
        "head",
        "boolean",
        "foreign",
        "duplicate",
    ] {
        let mut forge = forge();
        forge.context.forge = ForgeKind::GitHub;
        let (_dir, body) = body_file();
        let mut value = marked(1, HEAD);
        value["user"] = json!({"id":7});
        value["_surface"] = json!("review");
        value["review_state"] = json!("COMMENTED");
        value["commit_id"] = json!(HEAD);
        match fault {
            "pending" => value["review_state"] = json!("PENDING"),
            "dismissed" => value["review_state"] = json!("DISMISSED"),
            "unknown" => value["review_state"] = json!("UNKNOWN"),
            "head" => value["commit_id"] = json!(LOT),
            "boolean" => value["user"]["id"] = json!(true),
            "foreign" => value["user"]["id"] = json!(8),
            _ => {}
        }
        let mut values = vec![value];
        if fault == "duplicate" {
            let mut other = values[0].clone();
            other["_surface"] = json!("issue");
            values.push(other);
        }
        {
            let mut state = forge.transport.0.borrow_mut();
            state.account = Some(json!({"id":7}));
            state.comments.insert(1, values);
        }
        assert!(
            matches!(
                forge.review_comment(1, HEAD, &body, "apply"),
                Err(Error::Refused(_))
            ),
            "{fault}"
        );
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn owned_github_review_summary_updates_its_existing_surface() {
    let mut forge = forge();
    forge.context.forge = ForgeKind::GitHub;
    let (_dir, body) = body_file();
    let mut value = marked(1, HEAD);
    value["user"] = json!({"id":7});
    value["_surface"] = json!("review");
    value["review_state"] = json!("COMMENTED");
    value["commit_id"] = json!(HEAD);
    {
        let mut state = forge.transport.0.borrow_mut();
        state.account = Some(json!({"id":7}));
        state.comments.insert(1, vec![value]);
    }
    assert_eq!(forge.review_comment(1, HEAD, &body, "apply"), Ok(0));
    assert!(
        forge
            .transport
            .0
            .borrow()
            .calls
            .iter()
            .any(|(p, m, _)| p == "pullrequests/1/reviews/1" && m == "PUT")
    );
}
#[test]
fn later_page_review_is_found_and_invalid_pagination_refuses() {
    let value = marked(1, HEAD);
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path == "page2" {
                    json!({"size":1,"values":[value.clone()]})
                } else {
                    json!({"size":1,"values":[],"next":"page2"})
                })
            },
        )),
    };
    assert!(forge.marked_comment(1, HEAD, &json!(UUID)).is_err());
    for page in [
        json!({"size":2,"values":[value.clone()]}),
        json!({"size":1,"values":[value.clone()],"next":"again"}),
        json!({"size":1,"values":"bad"}),
        json!({"size":true,"values":[]}),
        json!({"values":[]}),
        json!({"size":0,"values":[],"next":42}),
    ] {
        let forge = Forge {
            context: context(),
            transport: FixedApi(page),
        };
        assert!(matches!(
            forge.marked_comment(1, HEAD, &json!(UUID)),
            Err(Error::Refused(_))
        ));
    }
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path == "page2" {
                    json!({"size":2,"values":[value.clone()]})
                } else {
                    json!({"size":2,"values":[comment(9,"unrelated")],"next":"page2"})
                })
            },
        )),
    };
    assert_eq!(
        forge
            .marked_comment(1, HEAD, &json!(UUID))
            .unwrap()
            .unwrap()["id"],
        1
    );
}
#[test]
fn changed_page_count_and_duplicate_comment_ids_refuse() {
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path == "page2" {
                    json!({"size":3,"values":[comment(2,"second")]})
                } else {
                    json!({"size":2,"values":[comment(1,"first")],"next":"page2"})
                })
            },
        )),
    };
    assert!(forge.comments(1).is_err());
    let forge = Forge {
        context: context(),
        transport: FixedApi(json!({"size":2,"values":[comment(1,"one"),comment(1,"two")]})),
    };
    assert!(forge.comments(1).is_err());
}
#[test]
fn replacement_dry_run_is_read_only() {
    let (forge, _dir, path) = replacement_fixture();
    assert_eq!(forge.replace(&path, "dry-run"), Ok(0));
    assert_eq!(writes(&forge), 0);
}
#[test]
fn replacement_validates_all_sources_before_first_write() {
    for fault in [
        "target",
        "lot-target",
        "unmerged",
        "merged-source",
        "head",
        "prefix",
        "ancestry",
        "short-ancestry",
        "malformed-ancestry",
        "review",
        "closed",
    ] {
        let (forge, _dir, path) = replacement_fixture();
        {
            let mut state = forge.transport.0.borrow_mut();
            state.pulls.insert(2, pull(2, HEAD, "OPEN"));
            match fault {
                "target" => {
                    state.pulls.get_mut(&2).unwrap()["destination"]["branch"]["name"] =
                        json!("other")
                }
                "lot-target" => {
                    state.pulls.get_mut(&99).unwrap()["destination"]["branch"]["name"] =
                        json!("other")
                }
                "unmerged" => state.pulls.get_mut(&99).unwrap()["state"] = json!("OPEN"),
                "merged-source" => state.pulls.get_mut(&2).unwrap()["state"] = json!("MERGED"),
                "head" => state.pulls.get_mut(&2).unwrap()["source"]["commit"]["hash"] = json!(LOT),
                "prefix" => {
                    state.pulls.get_mut(&2).unwrap()["source"]["commit"]["hash"] =
                        json!(format!("{}{}", &HEAD[..12], "b".repeat(28)))
                }
                "ancestry" => state.ancestry = Some(LOT.into()),
                "short-ancestry" => state.ancestry = Some(HEAD[..12].into()),
                "malformed-ancestry" => state.ancestry = Some("bad".into()),
                "review" => {
                    state.comments.remove(&2);
                }
                "closed" => state.pulls.get_mut(&2).unwrap()["state"] = json!("DECLINED"),
                _ => unreachable!(),
            }
        }
        assert!(
            matches!(forge.replace(&path, "apply"), Err(Error::Refused(_))),
            "{fault}"
        );
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn replacement_rejects_wrong_head_foreign_deleted_inline_and_replacement_only_review() {
    for fault in ["head", "foreign", "deleted", "inline", "replacement-only"] {
        let (forge, _dir, path) = replacement_fixture();
        let mut value = comment(
            1,
            &format!("{}\n\nReview", crate::review::review_marker(2, HEAD)),
        );
        match fault {
            "head" => value["content"]["raw"] = json!(crate::review::review_marker(2, LOT)),
            "foreign" => value["user"]["uuid"] = json!("foreign"),
            "deleted" => value["deleted"] = json!(true),
            "inline" => value["inline"] = json!({"path":"x"}),
            _ => value["content"]["raw"] = json!("Closed as replaced by #99."),
        }
        forge
            .transport
            .0
            .borrow_mut()
            .comments
            .insert(2, vec![value]);
        assert!(
            matches!(forge.replace(&path, "apply"), Err(Error::Refused(_))),
            "{fault}"
        );
        assert_eq!(writes(&forge), 0);
    }
}
#[test]
fn replacement_missing_observation_or_moved_head_never_closes_source() {
    for moved in [false, true] {
        let (forge, _dir, path) = replacement_fixture();
        {
            let mut state = forge.transport.0.borrow_mut();
            state.invisible = !moved;
            state.moved_after_post = moved;
        }
        assert!(forge.replace(&path, "apply").is_err());
        assert!(
            !forge
                .transport
                .0
                .borrow()
                .calls
                .iter()
                .any(|(p, _, _)| p.ends_with("/decline"))
        );
    }
}
#[test]
fn replacement_ambiguous_comment_and_close_writes_resume_without_duplicates() {
    for stage in ["comment", "close"] {
        let (forge, _dir, path) = replacement_fixture();
        {
            let mut state = forge.transport.0.borrow_mut();
            let error = Some(Error::Timeout("accepted but timed out".into()));
            if stage == "comment" {
                state.fault = error;
            } else {
                state.decline_fault = error;
            }
        }
        assert!(matches!(
            forge.replace(&path, "apply"),
            Err(Error::Timeout(_))
        ));
        assert_eq!(forge.replace(&path, "apply"), Ok(0));
        assert_eq!(writes(&forge), 4);
    }
}
#[test]
fn replacement_resumption_rejects_foreign_or_ambiguous_owned_notice() {
    for fault in [
        "foreign",
        "missing-author",
        "duplicate",
        "reply",
        "pending",
        "dismissed",
        "unknown",
    ] {
        let (forge, _dir, path) = replacement_fixture();
        assert_eq!(forge.replace(&path, "apply"), Ok(0));
        {
            let mut state = forge.transport.0.borrow_mut();
            let comments = state.comments.get_mut(&1).unwrap();
            match fault {
                "foreign" => comments[1]["user"]["uuid"] = json!("foreign"),
                "missing-author" => {
                    comments[1].as_object_mut().unwrap().remove("user");
                }
                "duplicate" => {
                    let mut duplicate = comments[1].clone();
                    duplicate["id"] = json!(3);
                    comments.push(duplicate);
                }
                "reply" => comments[1]["parent"] = json!({"id":8}),
                _ => {
                    comments[1]["_surface"] = json!("review");
                    comments[1]["review_state"] = json!(fault.to_uppercase());
                }
            }
        }
        assert!(
            matches!(forge.replace(&path, "apply"), Err(Error::Refused(_))),
            "{fault}"
        );
        assert_eq!(writes(&forge), 4);
    }
}
#[test]
fn invalid_replacement_plans_modes_and_approval_extras_never_write() {
    for plan in [
        json!({"lot_pr":99,"items":[]}),
        json!({"lot_pr":99,"items":[{"id":99,"head":HEAD}]}),
        json!({"lot_pr":99,"items":[{"id":1,"head":"short"}]}),
        json!({"lot_pr":99,"items":[{"id":1,"head":HEAD,"body":"custom"}]}),
        json!({"lot_pr":99,"items":[{"id":1,"head":HEAD},{"id":1,"head":HEAD}]}),
        json!({"lot_pr":99,"items":[{"id":1,"head":HEAD,"extra":"Approved"}]}),
        json!({"lot_pr":99,"items":[{"id":true,"head":HEAD}]}),
        json!({"lot_pr":99,"items":[{"id":1,"head":HEAD,"extra":4}]}),
    ] {
        let (forge, _dir, path) = replacement_fixture();
        std::fs::write(&path, plan.to_string()).unwrap();
        assert!(matches!(
            forge.replace(&path, "apply"),
            Err(Error::Refused(_))
        ));
        assert_eq!(writes(&forge), 0);
    }
    let (forge, _dir, path) = replacement_fixture();
    assert!(forge.replace(&path, "invalid").is_err());
    assert_eq!(writes(&forge), 0);
}

#[test]
fn github_ci_distinguishes_success_skips_pending_failure_and_unknown_states() {
    for (checks, expected) in [
        (
            json!([{"status":"completed","conclusion":"success"},{"status":"completed","conclusion":"skipped"}]),
            Ok(0),
        ),
        (json!([]), Ok(3)),
        (json!([{"status":"queued"}]), Ok(3)),
        (
            json!([{"status":"completed","conclusion":"neutral"}]),
            Ok(1),
        ),
        (
            json!([{"status":"completed","conclusion":"failure"}]),
            Ok(1),
        ),
        (
            json!([{"status":"completed","conclusion":"new-state"}]),
            Err(Error::Refused(
                "GitHub check has an unknown conclusion".into(),
            )),
        ),
        (
            json!([{"status":"unknown"}]),
            Err(Error::Refused("GitHub check has an unknown status".into())),
        ),
    ] {
        let mut context = context();
        context.forge = ForgeKind::GitHub;
        let forge = Forge {
            context,
            transport: FixedApi(json!({"sha":HEAD,"checks":checks})),
        };
        assert_eq!(forge.wait_ci(HEAD, &TestClock::default(), 2, 1), expected);
    }
}
#[test]
fn github_ci_rejects_short_sha_wrong_commit_malformed_checks_and_api_failure() {
    for value in [
        json!({"sha":LOT,"checks":[]}),
        json!({"sha":HEAD,"checks":null}),
        json!({"sha":HEAD,"checks":[false]}),
    ] {
        let mut context = context();
        context.forge = ForgeKind::GitHub;
        let forge = Forge {
            context,
            transport: FixedApi(value),
        };
        assert!(forge.wait_ci(HEAD, &TestClock::default(), 2, 1).is_err());
        assert!(
            forge
                .wait_ci(&HEAD[..12], &TestClock::default(), 2, 1)
                .is_err()
        );
    }
    let mut context = context();
    context.forge = ForgeKind::GitHub;
    let forge = Forge {
        context,
        transport: FnApi(RefCell::new(
            |_: &str, _: &str, _: Option<&Value>, _: u64| Err(Error::Failed("unavailable".into())),
        )),
    };
    assert!(matches!(
        forge.wait_ci(HEAD, &TestClock::default(), 2, 1),
        Err(Error::Failed(_))
    ));
}
fn pipeline(sha: &str, branch: &str, result: &str) -> Value {
    json!({"build_number":9,"target":{"commit":{"hash":sha},"ref_name":branch},"state":{"name":"COMPLETED","result":{"name":result}}})
}
fn step(result: &str) -> Value {
    json!({"name":"project-test","state":{"name":"COMPLETED","result":{"name":result}}})
}
#[test]
fn bitbucket_ci_requires_a_successful_pipeline_and_executed_successful_steps() {
    for (result, steps, expected) in [
        (
            "SUCCESSFUL",
            vec![
                step("SUCCESSFUL"),
                step("SKIPPED"),
                json!({"name":"not-run","state":{"name":"NOT_RUN"}}),
            ],
            0,
        ),
        ("FAILED", vec![step("SUCCESSFUL"), step("FAILED")], 1),
        ("SUCCESSFUL", vec![step("SUCCESSFUL"), step("ERROR")], 1),
        ("SUCCESSFUL", vec![], 1),
        ("SUCCESSFUL", vec![step("SKIPPED")], 1),
    ] {
        let forge = Forge {
            context: context(),
            transport: FnApi(RefCell::new(
                |path: &str, _: &str, _: Option<&Value>, _: u64| {
                    Ok(if path.starts_with("pipelines?") {
                        json!({"values":[pipeline(HEAD,"feature",result)]})
                    } else {
                        json!({"size":steps.len(),"values":steps})
                    })
                },
            )),
        };
        assert_eq!(
            forge.wait_ci(HEAD, &TestClock::default(), 2, 1),
            Ok(expected)
        );
    }
}
#[test]
fn bitbucket_ci_ignores_merge_queue_and_nonexact_hashes_and_refuses_short_sha() {
    for run in [
        pipeline(HEAD, "bitbucket-merge-queue-1", "SUCCESSFUL"),
        pipeline(&format!("{HEAD}a"), "feature", "SUCCESSFUL"),
        pipeline(LOT, "feature", "SUCCESSFUL"),
    ] {
        let forge = Forge {
            context: context(),
            transport: FixedApi(json!({"values":[run]})),
        };
        assert_eq!(forge.wait_ci(HEAD, &TestClock::default(), 2, 1), Ok(3));
        assert!(
            forge
                .wait_ci(&HEAD[..12], &TestClock::default(), 2, 1)
                .is_err()
        );
    }
}
#[test]
fn bitbucket_ci_transient_error_is_bounded_and_bad_pagination_is_refused() {
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |_: &str, _: &str, _: Option<&Value>, _: u64| Err(Error::Failed("unavailable".into())),
        )),
    };
    assert_eq!(forge.wait_ci(HEAD, &TestClock::default(), 2, 1), Ok(3));
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path.starts_with("pipelines?") {
                    json!({"values":[pipeline(HEAD,"feature","SUCCESSFUL")]})
                } else {
                    json!({"size":2,"values":[step("SUCCESSFUL")]})
                })
            },
        )),
    };
    assert!(matches!(
        forge.wait_ci(HEAD, &TestClock::default(), 2, 1),
        Err(Error::Refused(_))
    ));
}
#[test]
fn watch_distinguishes_merged_closed_pending_wrong_destination_and_failed_queue() {
    for kind in [ForgeKind::GitHub, ForgeKind::Bitbucket] {
        for (state, target, expected) in [
            ("MERGED", "trunk", Some(0)),
            ("DECLINED", "trunk", Some(1)),
            ("OPEN", "trunk", Some(3)),
            ("MERGED", "other", None),
        ] {
            let mut context = context();
            context.forge = kind.clone();
            let forge = Forge {
                context,
                transport: FnApi(RefCell::new(
                    |path: &str, _: &str, _: Option<&Value>, _: u64| {
                        Ok(if path.starts_with("pipelines?") {
                            json!({"values":[]})
                        } else if path.starts_with("checks/") {
                            json!({"sha":HEAD,"checks":[]})
                        } else {
                            let mut p = pull(1, HEAD, state);
                            p["destination"]["branch"]["name"] = json!(target);
                            p
                        })
                    },
                )),
            };
            let result = forge.watch(&[1], &TestClock::default(), 2, 1);
            if let Some(code) = expected {
                assert_eq!(result, Ok(code));
            } else {
                assert!(matches!(result, Err(Error::Refused(_))));
            }
        }
    }
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path.starts_with("pipelines?") {
                    json!({"values":[pipeline(HEAD,"bitbucket-merge-queue-1","FAILED")]})
                } else {
                    pull(1, HEAD, "OPEN")
                })
            },
        )),
    };
    assert_eq!(forge.watch(&[1], &TestClock::default(), 2, 1), Ok(1));
}
fn record(id: u64, paths: Value) -> Value {
    json!({"id":id,"paths":paths,"draft":false,"changes_requested_by":[],"destination":"trunk"})
}
#[test]
fn inventory_grouping_retains_connected_components_and_excludes_lockfile_only_coupling() {
    let records = vec![
        record(1, json!(["src/a", "bun.lock"])),
        record(2, json!(["src/a", "src/b"])),
        record(3, json!(["src/b"])),
        record(4, json!(["bun.lock"])),
        record(5, json!(["bun.lock"])),
    ];
    let plan = crate::forge::suggest(&records, "trunk");
    assert_eq!(plan["lots"], json!([[1, 2, 3]]));
    assert_eq!(plan["alone"], json!([4, 5]));
    assert_eq!(plan["pairs"].as_array().unwrap().len(), 2);
}
#[test]
fn drafts_changes_requested_and_foreign_targets_never_group() {
    let mut records = vec![
        record(1, json!(["src/a"])),
        record(2, json!(["src/a"])),
        record(3, json!(["src/a"])),
        record(4, json!(["src/a"])),
    ];
    records[1]["draft"] = json!(true);
    records[2]["changes_requested_by"] = json!(["reviewer"]);
    records[3]["destination"] = json!("other");
    let plan = crate::forge::suggest(&records, "trunk");
    assert_eq!(plan["lots"], json!([]));
    assert_eq!(
        plan["apart"],
        json!({"draft":[2],"changes requested":[3],"not targeting primary branch":[4]})
    );
}
#[test]
fn inventory_incomplete_listing_does_not_create_output() {
    let forge = Forge {
        context: context(),
        transport: FixedApi(json!({"size":2,"values":[{"id":1}]})),
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("inventory.json");
    assert!(forge.inventory(&out).is_err());
    assert!(!out.exists());
}
#[test]
fn create_refuses_unsafe_branch_existing_pr_and_incomplete_listing() {
    let (_dir, body) = body_file();
    for branch in ["trunk", "batch;touch bad", "", "$(bad)"] {
        let forge = forge();
        assert!(forge.create(branch, "Batch", &body).is_err());
        assert_eq!(writes(&forge), 0);
    }
    for value in [
        json!({"size":1,"values":[{"id":1}]}),
        json!({"size":2,"values":[{"id":1}]}),
    ] {
        let called = std::cell::Cell::new(false);
        let forge = Forge {
            context: context(),
            transport: FnApi(RefCell::new(
                |_: &str, m: &str, _: Option<&Value>, _: u64| {
                    called.set(m != "GET");
                    Ok(value.clone())
                },
            )),
        };
        assert!(forge.create("batch", "Batch", &body).is_err());
        assert!(!called.get());
    }
}
#[test]
fn create_targets_discovered_primary_and_keeps_source_branch() {
    let (_dir, body) = body_file();
    let forge = Forge {
        context: context(),
        transport: FnApi(RefCell::new(
            |_: &str, m: &str, p: Option<&Value>, _: u64| {
                if m == "GET" {
                    return Ok(json!({"size":0,"values":[]}));
                }
                let p = p.unwrap();
                assert_eq!(p["destination"]["branch"]["name"], "trunk");
                assert_eq!(p["close_source_branch"], false);
                let mut value = p.clone();
                value["id"] = json!(1);
                value["source"]["commit"] = json!({"hash":HEAD});
                Ok(value)
            },
        )),
    };
    assert_eq!(forge.create("batch", "Batch", &body), Ok(0));
}
#[test]
fn description_comparison_check_and_observation_preserve_cli_semantics() {
    let (_dir, body) = body_file();
    for (old, check, expected, expected_writes) in [
        ("No demonstrated defect.", true, true, 0),
        ("changed", false, false, 0),
        ("No demonstrated defect.", false, true, 1),
    ] {
        let count = std::cell::Cell::new(0);
        let forge = Forge {
            context: context(),
            transport: FnApi(RefCell::new(
                |_: &str, m: &str, p: Option<&Value>, _: u64| {
                    if m == "GET" {
                        Ok(
                            json!({"description":old,"title":"Batch","state":"OPEN","source":{"commit":{"hash":HEAD}}}),
                        )
                    } else {
                        count.set(count.get() + 1);
                        Ok(p.unwrap().clone())
                    }
                },
            )),
        };
        let result = forge.desc(1, &body, &body, check);
        assert_eq!(result.is_ok(), expected);
        assert_eq!(count.get(), expected_writes);
    }
}

fn github_pull() -> Value {
    json!({"id":1001,"number":1,"title":"Batch","body":"Description","user":{"id":9,"login":"author"},"head":{"sha":HEAD,"ref":"feature"},"base":{"ref":"trunk"},"state":"open","changed_files":1})
}
#[test]
fn github_malformed_pull_payloads_refuse_instead_of_empty_success() {
    for (key, value) in [
        ("number", json!(true)),
        ("head", json!({"sha":"short","ref":"feature"})),
        ("base", json!({"ref":null})),
        ("user", json!({"login":false})),
        ("body", json!(4)),
        ("state", json!("unknown")),
    ] {
        let mut value_ = github_pull();
        value_[key] = value;
        let adapter = crate::github::GitHub {
            repository: "owner/project".into(),
            request: FixedApi(value_),
        };
        assert!(
            matches!(
                adapter.api("pullrequests/1", "GET", None, 1),
                Err(Error::Failed(_))
            ),
            "{key}"
        );
    }
}
#[test]
fn github_collection_rejects_repeated_missing_and_boolean_ids() {
    for values in [
        json!([{"id":1},{"id":1}]),
        json!([{"id":true}]),
        json!([{"id":null}]),
        json!({}),
        json!([null]),
    ] {
        let adapter = crate::github::GitHub {
            repository: "owner/project".into(),
            request: FixedApi(values),
        };
        assert!(matches!(
            adapter.api("pullrequests", "GET", None, 1),
            Err(Error::Failed(_))
        ));
    }
}
#[test]
fn github_checks_validate_total_count_exact_sha_and_status_payload() {
    for fault in [
        "count",
        "head",
        "status",
        "identity",
        "missing-count",
        "commit-status",
    ] {
        let adapter = crate::github::GitHub {
            repository: "owner/project".into(),
            request: FnApi(RefCell::new(
                |path: &str, _: &str, _: Option<&Value>, _: u64| {
                    if path.contains("check-runs") {
                        let mut value = json!({"total_count":1,"check_runs":[{"id":1,"name":"CI","head_sha":HEAD,"status":"completed","conclusion":"success"}]});
                        match fault {
                            "count" => value["total_count"] = json!(2),
                            "head" => value["check_runs"][0]["head_sha"] = json!(LOT),
                            "status" => value["check_runs"][0]["status"] = json!("unknown"),
                            "identity" => value["check_runs"][0]["id"] = json!(true),
                            "missing-count" => {
                                value.as_object_mut().unwrap().remove("total_count");
                            }
                            _ => {}
                        };
                        Ok(value)
                    } else {
                        Ok(json!([{"id":2,"context":"old-ci","state":"unknown"}]))
                    }
                },
            )),
        };
        assert!(
            adapter
                .api(&format!("checks/{HEAD}"), "GET", None, 1)
                .is_err(),
            "{fault}"
        );
    }
}
#[test]
fn github_checks_preserve_latest_commit_status_and_real_check_success() {
    let adapter = crate::github::GitHub {
        repository: "owner/project".into(),
        request: FnApi(RefCell::new(
            |path: &str, _: &str, _: Option<&Value>, _: u64| {
                Ok(if path.contains("check-runs") {
                    json!({"total_count":1,"check_runs":[{"id":1,"name":"CI","head_sha":HEAD,"status":"completed","conclusion":"success"}]})
                } else {
                    json!([{"id":2,"context":"legacy","state":"success"},{"id":3,"context":"legacy","state":"failure"}])
                })
            },
        )),
    };
    let result = adapter
        .api(&format!("checks/{HEAD}"), "GET", None, 1)
        .unwrap();
    assert_eq!(result["checks"].as_array().unwrap().len(), 2);
    assert_eq!(result["checks"][1]["conclusion"], "success");
}
#[test]
fn github_changed_files_require_complete_count_and_numeric_line_counts() {
    for fault in ["count", "cap", "negative", "boolean", "duplicate"] {
        let adapter = crate::github::GitHub {
            repository: "owner/project".into(),
            request: FnApi(RefCell::new(
                |path: &str, _: &str, _: Option<&Value>, _: u64| {
                    if path.contains("/files") {
                        let mut file = json!({"filename":"src/new.rs","previous_filename":"src/old.rs","additions":3,"deletions":1});
                        if fault == "negative" {
                            file["additions"] = json!(-1);
                        }
                        if fault == "boolean" {
                            file["deletions"] = json!(true);
                        }
                        Ok(if fault == "duplicate" {
                            json!([file.clone(), file])
                        } else {
                            json!([file])
                        })
                    } else {
                        let mut pull = github_pull();
                        if fault == "count" {
                            pull["changed_files"] = json!(2);
                        }
                        if fault == "cap" {
                            pull["changed_files"] = json!(3001);
                        }
                        Ok(pull)
                    }
                },
            )),
        };
        assert!(
            adapter
                .api("pullrequests/1/diffstat", "GET", None, 1)
                .is_err(),
            "{fault}"
        );
    }
}
#[test]
fn github_review_state_selects_latest_decision_and_rejects_unknown_identity() {
    for invalid in [false, true] {
        let adapter = crate::github::GitHub {
            repository: "owner/project".into(),
            request: FnApi(RefCell::new(
                |path: &str, _: &str, _: Option<&Value>, _: u64| {
                    if path.contains("/reviews") {
                        let mut values = json!([{"id":1,"state":"APPROVED","user":{"id":9,"login":"reviewer"}},{"id":2,"state":"COMMENTED","user":{"id":9,"login":"reviewer"}},{"id":3,"state":"CHANGES_REQUESTED","user":{"id":9,"login":"reviewer"}}]);
                        if invalid {
                            values[2]["user"]["id"] = json!(true);
                        }
                        Ok(values)
                    } else {
                        Ok(json!([github_pull()]))
                    }
                },
            )),
        };
        let result = adapter.api("pullrequests", "GET", None, 1);
        if invalid {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert_eq!(
                result["values"][0]["participants"][0]["state"],
                "changes_requested"
            );
        }
    }
}
#[test]
fn github_unsupported_and_foreign_operations_refuse_before_transport() {
    let adapter = crate::github::GitHub {
        repository: "owner/project".into(),
        request: FnApi(RefCell::new(
            |_: &str, _: &str, _: Option<&Value>, _: u64| -> Result<Value> {
                panic!("transport must not start")
            },
        )),
    };
    for path in [
        "https://github.com/owner/project",
        "/repos/owner/project/pulls",
        "../pullrequests",
        "pullrequests/%2e%2e",
        "pullrequests/\\bad",
        "unknown",
    ] {
        assert!(adapter.api(path, "GET", None, 1).is_err(), "{path}");
    }
}
