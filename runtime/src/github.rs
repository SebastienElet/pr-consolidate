use crate::context::is_sha;
use crate::error::{Error, Result};
use crate::transport::{Api, invalid_path};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub struct GitHub<R> {
    pub repository: String,
    pub request: R,
}

fn failed(message: &str) -> Error {
    Error::Failed(message.into())
}
fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(failed(message))
    }
}

impl<R: Api> GitHub<R> {
    fn root(&self) -> String {
        format!("/repos/{}/", self.repository)
    }
    fn object(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        let value = self.request.api(path, method, payload, timeout)?;
        check(value.is_object(), "GitHub returned a non-object response")?;
        Ok(value)
    }
    fn collection(
        &self,
        path: &str,
        key: Option<&str>,
        timeout: u64,
        identity_key: &str,
    ) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        let mut seen = HashSet::new();
        let mut total = None;
        let separator = if path.contains('?') { '&' } else { '?' };
        for page in 1..=500 {
            let data = self.request.api(
                &format!("{path}{separator}per_page=100&page={page}"),
                "GET",
                None,
                timeout,
            )?;
            let batch = if let Some(key) = key {
                &data[key]
            } else {
                &data
            };
            let batch = batch
                .as_array()
                .ok_or_else(|| failed("GitHub returned an invalid collection"))?;
            check(
                batch.iter().all(Value::is_object),
                "GitHub returned an invalid collection",
            )?;
            if key.is_some() {
                let current = data["total_count"]
                    .as_u64()
                    .ok_or_else(|| failed("GitHub collection count is invalid or changed"))?;
                check(
                    total.is_none_or(|total| total == current),
                    "GitHub collection count is invalid or changed",
                )?;
                total = Some(current);
            }
            for item in batch {
                let identity = &item[identity_key];
                check(
                    if identity_key == "id" {
                        identity.as_u64().is_some_and(|n| n > 0)
                    } else {
                        identity.as_str().is_some_and(|s| !s.is_empty())
                    },
                    "GitHub collection has missing or repeated IDs",
                )?;
                check(
                    seen.insert(identity.to_string()),
                    "GitHub collection has missing or repeated IDs",
                )?;
            }
            values.extend(batch.iter().cloned());
            if batch.len() < 100 {
                check(
                    total.is_none_or(|n| values.len() as u64 == n),
                    "GitHub collection count is incomplete",
                )?;
                return Ok(values);
            }
        }
        Err(failed("GitHub pagination exceeded its bound"))
    }
    fn pull(&self, value: &Value) -> Result<Value> {
        check(
            value["number"].as_u64().is_some_and(|n| n > 0)
                && matches!(value["state"].as_str(), Some("open" | "closed"))
                && value["head"]["sha"]
                    .as_str()
                    .is_some_and(|s| is_sha(s, 40, 40))
                && [&value["head"]["ref"], &value["base"]["ref"]]
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                && value["title"].is_string()
                && value["user"]["login"].is_string()
                && (value["body"].is_null() || value["body"].is_string()),
            "GitHub returned an invalid pull request",
        )?;
        let merged =
            value["merged"] == true || value["merged_at"].as_str().is_some_and(|s| !s.is_empty());
        Ok(
            json!({"id":value["number"], "state":if merged {"MERGED"} else if value["state"] == "open" {"OPEN"} else {"DECLINED"},
            "title":value["title"],"description":value["body"].as_str().unwrap_or(""),"author":{"display_name":value["user"]["login"]},
            "draft":value.get("draft").cloned().unwrap_or(json!(false)),"source":{"branch":{"name":value["head"]["ref"]},"commit":{"hash":value["head"]["sha"]}},
            "destination":{"branch":{"name":value["base"]["ref"]}},"merge_commit":if merged {json!({"hash":value["merge_commit_sha"]})} else {Value::Null},
            "created_on":value["created_at"].as_str().unwrap_or(""),"updated_on":value["updated_at"].as_str().unwrap_or("")}),
        )
    }
    fn reviews(&self, number: u64, timeout: u64) -> Result<Vec<Value>> {
        self.collection(
            &format!("{}pulls/{number}/reviews", self.root()),
            None,
            timeout,
            "id",
        )
    }
    fn participants(&self, number: u64, timeout: u64) -> Result<Vec<Value>> {
        let mut latest = BTreeMap::new();
        for review in self.reviews(number, timeout)? {
            let user = &review["user"];
            let state = review["state"]
                .as_str()
                .ok_or_else(|| failed("GitHub returned an invalid review identity or state"))?;
            check(
                user["id"].is_i64()
                    && user["login"].is_string()
                    && matches!(
                        state,
                        "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "PENDING"
                    ),
                "GitHub returned an invalid review identity or state",
            )?;
            if !matches!(state, "COMMENTED" | "PENDING") {
                latest.insert(user["id"].to_string(), json!({"user":{"display_name":user["login"]},"state":state.to_lowercase(),"approved":state == "APPROVED"}));
            }
        }
        Ok(latest.into_values().collect())
    }
    fn checks(&self, sha: &str, timeout: u64) -> Result<Value> {
        check(
            is_sha(sha, 40, 40),
            "GitHub checks require the complete commit SHA",
        )?;
        let runs = self.collection(
            &format!("{}commits/{sha}/check-runs?filter=latest", self.root()),
            Some("check_runs"),
            timeout,
            "id",
        )?;
        let statuses = self.collection(
            &format!("{}commits/{sha}/statuses", self.root()),
            None,
            timeout,
            "id",
        )?;
        let mut result = Vec::new();
        let mut latest = HashSet::new();
        for run in runs {
            check(
                run["head_sha"] == sha
                    && run["name"].is_string()
                    && matches!(
                        run["status"].as_str(),
                        Some(
                            "queued"
                                | "in_progress"
                                | "completed"
                                | "waiting"
                                | "pending"
                                | "requested"
                        )
                    ),
                "GitHub returned checks for an unknown state or different commit",
            )?;
            result.push(
                json!({"name":run["name"],"status":run["status"],"conclusion":run["conclusion"]}),
            );
        }
        for status in statuses {
            let state = status["state"]
                .as_str()
                .ok_or_else(|| failed("GitHub returned an invalid commit status"))?;
            check(
                status["context"].is_string()
                    && matches!(state, "pending" | "success" | "error" | "failure"),
                "GitHub returned an invalid commit status",
            )?;
            if latest.insert(status["context"].to_string()) {
                result.push(json!({"name":status["context"],"status":if state == "pending" {"pending"} else {"completed"},"conclusion":if state == "pending" {None} else {Some(state)}}));
            }
        }
        Ok(json!({"sha":sha,"checks":result}))
    }
    fn comments(&self, number: u64, timeout: u64) -> Result<Value> {
        let issue = self.collection(
            &format!("{}issues/{number}/comments", self.root()),
            None,
            timeout,
            "id",
        )?;
        let review = self.reviews(number, timeout)?;
        let mut result = Vec::new();
        for (surface, values) in [("issue", issue), ("review", review)] {
            for value in values {
                check(
                    value["body"].is_string() && value["user"].is_object(),
                    "GitHub returned invalid comment content or identity",
                )?;
                result.push(json!({"id":value["id"],"content":{"raw":value["body"]},"user":value["user"],"_surface":surface,"review_state":value["state"],"commit_id":value["commit_id"]}));
            }
        }
        Ok(json!({"size":result.len(),"values":result}))
    }
    fn files(&self, number: u64, timeout: u64) -> Result<Value> {
        let pull = self.object(
            &format!("{}pulls/{number}", self.root()),
            "GET",
            None,
            timeout,
        )?;
        let values = self.collection(
            &format!("{}pulls/{number}/files", self.root()),
            None,
            timeout,
            "filename",
        )?;
        check(
            pull["changed_files"]
                .as_u64()
                .is_some_and(|n| n <= 3000 && n == values.len() as u64),
            "GitHub file listing is incomplete or exceeds the supported API limit",
        )?;
        let mut result = Vec::new();
        for value in values {
            check(
                value["filename"].is_string()
                    && value["additions"].is_u64()
                    && value["deletions"].is_u64(),
                "GitHub returned an invalid changed file",
            )?;
            result.push(json!({"new":{"path":value["filename"]},"old":{"path":value.get("previous_filename").unwrap_or(&value["filename"])},"lines_added":value["additions"],"lines_removed":value["deletions"]}));
        }
        Ok(json!({"size":result.len(),"values":result}))
    }
}

impl<R: Api> Api for GitHub<R> {
    fn api(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        if path == "/user" && method == "GET" {
            return self.object(path, method, None, timeout);
        }
        let (endpoint, query) = path.split_once('?').unwrap_or((path, ""));
        check(
            !path.starts_with('/')
                && !path.contains("://")
                && !path.contains('#')
                && !invalid_path(endpoint),
            "invalid GitHub consolidation endpoint",
        )?;
        if let Some(sha) = endpoint.strip_prefix("checks/") {
            if method == "GET" {
                return self.checks(sha, timeout);
            }
        }
        if endpoint == "pullrequests" {
            if method == "POST" {
                let payload = payload.ok_or_else(|| failed("missing GitHub request payload"))?;
                return self.pull(&self.object(&format!("{}pulls", self.root()), "POST", Some(&json!({"title":payload["title"],"body":payload["description"],"head":payload["source"]["branch"]["name"],"base":payload["destination"]["branch"]["name"]})), timeout)?);
            }
            let mut values = self.collection(
                &format!("{}pulls?state=open", self.root()),
                None,
                timeout,
                "id",
            )?;
            let branch_pattern =
                Regex::new(r#"source\.branch\.name="([A-Za-z0-9._/-]+)""#).unwrap();
            let branch = branch_pattern.captures(query);
            if let Some(branch) = &branch {
                values.retain(|v| v["head"]["ref"] == branch[1]);
            }
            let mut records = Vec::new();
            for value in values {
                let mut record = self.pull(&value)?;
                if branch.is_none() {
                    record["participants"] =
                        json!(self.participants(record["id"].as_u64().unwrap(), timeout)?);
                }
                records.push(record);
            }
            return Ok(json!({"size":records.len(),"values":records}));
        }
        if let Some(found) = Regex::new(r"\Apullrequests/([0-9]+)(?:/(.*))?\z")
            .unwrap()
            .captures(endpoint)
        {
            let number: u64 = found[1]
                .parse()
                .map_err(|_| failed("invalid pull request number"))?;
            let suffix = found.get(2).map(|m| m.as_str());
            let root = self.root();
            match (suffix, method) {
                (None, _) => {
                    let body = payload.map(|p| json!({"title":p["title"],"body":p["description"]}));
                    return self.pull(&self.object(
                        &format!("{root}pulls/{number}"),
                        if method == "PUT" { "PATCH" } else { method },
                        body.as_ref(),
                        timeout,
                    )?);
                }
                (Some("diffstat"), "GET") => return self.files(number, timeout),
                (Some("statuses"), "GET") => {
                    let pr = self.pull(&self.object(
                        &format!("{root}pulls/{number}"),
                        "GET",
                        None,
                        timeout,
                    )?)?;
                    let checks =
                        self.checks(pr["source"]["commit"]["hash"].as_str().unwrap(), timeout)?;
                    let values: Vec<_> = checks["checks"].as_array().unwrap().iter().map(|c| json!({"state": if c["conclusion"].is_null() { &c["status"] } else { &c["conclusion"] }})).collect();
                    return Ok(json!({"size":values.len(),"values":values}));
                }
                (Some("comments"), "GET") => return self.comments(number, timeout),
                (Some("decline"), "POST") => {
                    return self.pull(&self.object(
                        &format!("{root}pulls/{number}"),
                        "PATCH",
                        Some(&json!({"state":"closed"})),
                        timeout,
                    )?);
                }
                (Some(suffix), "POST" | "PUT") => {
                    if let Some(comment) = Regex::new(r"\A(comments|reviews)(?:/([0-9]+))?\z")
                        .unwrap()
                        .captures(suffix)
                    {
                        let (target, verb) = if let Some(id) = comment.get(2) {
                            if &comment[1] == "comments" {
                                (format!("{root}issues/comments/{}", id.as_str()), "PATCH")
                            } else {
                                (
                                    format!("{root}pulls/{number}/reviews/{}", id.as_str()),
                                    "PUT",
                                )
                            }
                        } else {
                            (format!("{root}issues/{number}/comments"), "POST")
                        };
                        let payload =
                            payload.ok_or_else(|| failed("missing GitHub request payload"))?;
                        return self.object(
                            &target,
                            verb,
                            Some(&json!({"body":payload["content"]["raw"]})),
                            timeout,
                        );
                    }
                }
                _ => {}
            }
        }
        if let Some(found) = Regex::new(r"\Amerge-base/([0-9a-f]{40})\.\.([0-9a-f]{12,40})\z")
            .unwrap()
            .captures(endpoint)
        {
            if method == "GET" {
                let value = self.object(
                    &format!("{}compare/{}...{}", self.root(), &found[1], &found[2]),
                    method,
                    None,
                    timeout,
                )?;
                return Ok(json!({"hash":value["merge_base_commit"]["sha"]}));
            }
        }
        Err(failed("unsupported GitHub consolidation operation"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Raw;
    impl Api for Raw {
        fn api(&self, path: &str, method: &str, payload: Option<&Value>, _: u64) -> Result<Value> {
            assert_eq!((path, method), ("/repos/owner/project/pulls", "POST"));
            assert_eq!(payload.unwrap()["base"], "trunk");
            Ok(
                json!({"number":1,"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","ref":"batch"},"base":{"ref":"trunk"},"user":{"login":"human"},"title":"Batch","state":"open","body":"Description"}),
            )
        }
    }
    #[test]
    fn create_maps_github_payload_and_preserves_normalized_head() {
        let adapter = GitHub {
            repository: "owner/project".into(),
            request: Raw,
        };
        let result = adapter.api("pullrequests", "POST", Some(&json!({"title":"Batch","description":"Description","source":{"branch":{"name":"batch"}},"destination":{"branch":{"name":"trunk"}}})), 120);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(result.unwrap()["source"]["commit"]["hash"], "a".repeat(40));
    }
}
