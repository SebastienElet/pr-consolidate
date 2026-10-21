use crate::{error::Result, forge::Forge, transport::Api};
use std::time::{Duration, Instant};

pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}
pub struct SystemClock(Instant);
impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

use crate::context::{ForgeKind, is_sha};
use crate::error::{Error, require};
use crate::forge::{RUN_FIELDS, string};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq)]
enum CheckState {
    Passed,
    Failed,
    Pending,
}

pub fn classify_step(step: &Value) -> &'static str {
    let name = string(step, "/state/name");
    let result = string(step, "/state/result/name");
    if name == "COMPLETED" && result == "SUCCESSFUL" {
        "passed"
    } else if (name == "COMPLETED" && result == "SKIPPED")
        || name == "NOT_RUN"
        || result == "NOT_RUN"
    {
        "skipped"
    } else {
        "failed"
    }
}

impl<A: Api> Forge<A> {
    fn check_state(&self, sha: &str) -> Result<CheckState> {
        let data = self.get(&format!("checks/{sha}"))?;
        require(
            data["sha"] == sha
                && data["checks"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_object)),
            "GitHub checks are malformed or belong to a different commit",
        )?;
        let checks = data["checks"].as_array().unwrap();
        if checks.is_empty() {
            return Ok(CheckState::Pending);
        }
        let mut counts = [0; 4];
        for check in checks {
            let status = check["status"].as_str().unwrap_or("");
            require(
                matches!(
                    status,
                    "queued" | "in_progress" | "completed" | "waiting" | "pending" | "requested"
                ),
                "GitHub check has an unknown status",
            )?;
            let kind = if status != "completed" {
                3
            } else {
                match check["conclusion"].as_str() {
                    Some("success") => 0,
                    Some("skipped" | "neutral") => 1,
                    Some(
                        "failure" | "error" | "cancelled" | "timed_out" | "action_required"
                        | "startup_failure" | "stale",
                    ) => 2,
                    _ => {
                        return Err(Error::Refused(
                            "GitHub check has an unknown conclusion".into(),
                        ));
                    }
                }
            };
            counts[kind] += 1;
        }
        println!(
            "checks on {}: {} passed, {} skipped, {} failed, {} pending",
            &sha[..12],
            counts[0],
            counts[1],
            counts[2],
            counts[3]
        );
        Ok(if counts[2] > 0 {
            CheckState::Failed
        } else if counts[3] > 0 {
            CheckState::Pending
        } else if counts[0] > 0 {
            CheckState::Passed
        } else {
            CheckState::Failed
        })
    }
    fn bitbucket_ci(&self, sha: &str) -> Result<Option<i32>> {
        let data = self.get(&format!(
            "pipelines?sort=-created_on&pagelen=50&fields={RUN_FIELDS}"
        ))?;
        let empty = Vec::new();
        let runs = data["values"].as_array().unwrap_or(&empty);
        let run = runs.iter().find(|r| {
            string(r, "/target/commit/hash") == sha
                && !string(r, "/target/ref_name").starts_with("bitbucket-merge-queue")
        });
        let Some(run) = run else {
            return Ok(None);
        };
        println!(
            "pipeline {} {} on {}",
            run["build_number"],
            string(run, "/state/name"),
            &sha[..12]
        );
        if string(run, "/state/name") != "COMPLETED" {
            return Ok(None);
        }
        let steps = self.paged(&format!("pipelines/{}/steps?pagelen=100&fields=size,next,values.name,values.state.name,values.state.result.name", run["build_number"]), 60)?;
        let mut passed = 0;
        let mut failed = 0;
        for step in &steps {
            let kind = classify_step(step);
            println!(
                "step {}: {}/{} -> {kind}",
                step["name"],
                string(step, "/state/name"),
                string(step, "/state/result/name")
            );
            if kind == "passed" {
                passed += 1;
            } else if kind == "failed" {
                failed += 1;
            }
        }
        println!(
            "pipeline {} {} on {}: {passed} passed, {failed} failed",
            run["build_number"],
            string(run, "/state/result/name"),
            &sha[..12]
        );
        if string(run, "/state/result/name") != "SUCCESSFUL" || failed > 0 || passed == 0 {
            return Ok(Some(1));
        }
        Ok(Some(0))
    }
    pub fn wait_ci(&self, sha: &str, clock: &impl Clock, timeout: u64, poll: u64) -> Result<i32> {
        require(is_sha(sha, 40, 40), "CI requires the full commit SHA")?;
        let deadline = clock.now() + Duration::from_secs(timeout);
        while clock.now() < deadline {
            if self.context.forge == ForgeKind::GitHub {
                match self.check_state(sha)? {
                    CheckState::Passed => return Ok(0),
                    CheckState::Failed => return Ok(1),
                    CheckState::Pending => {}
                }
            } else {
                match self.bitbucket_ci(sha) {
                    Ok(Some(code)) => return Ok(code),
                    Ok(None) => {}
                    Err(Error::Failed(message) | Error::Timeout(message)) => eprintln!(
                        "transient: {}",
                        crate::process::mask(&message)
                            .chars()
                            .take(100)
                            .collect::<String>()
                    ),
                    Err(error) => return Err(error),
                }
            }
            clock.sleep(Duration::from_secs(poll.max(1)).min(deadline.saturating_sub(clock.now())));
        }
        println!("timeout: no completed successful branch checks for {sha} within {timeout}s");
        Ok(3)
    }
    pub fn watch(&self, prs: &[u64], clock: &impl Clock, timeout: u64, poll: u64) -> Result<i32> {
        let mut pending: BTreeSet<_> = prs.iter().copied().collect();
        let mut outcomes = BTreeMap::new();
        let deadline = clock.now() + Duration::from_secs(timeout);
        while !pending.is_empty() && clock.now() < deadline {
            let queue = if self.context.forge == ForgeKind::Bitbucket {
                match self.get("pipelines?sort=-created_on&pagelen=50&fields=values.build_number,values.state.name,values.state.result.name,values.target.ref_name") {
                    Ok(data) => data["values"].as_array().cloned().unwrap_or_default(),
                    Err(Error::Failed(message) | Error::Timeout(message)) => {
                        eprintln!("transient: {}", crate::process::mask(&message));
                        clock.sleep(Duration::from_secs(poll.max(1)).min(deadline.saturating_sub(clock.now())));
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            } else {
                Vec::new()
            };
            for &pr in &pending {
                let data = match self.get(&format!("pullrequests/{pr}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name")) {
                    Ok(data) => data,
                    Err(Error::Failed(message) | Error::Timeout(message)) if self.context.forge == ForgeKind::Bitbucket => { eprintln!("transient #{pr}: {}", crate::process::mask(&message)); continue; }
                    Err(error) => return Err(error),
                };
                self.destination(&data, pr)?;
                if data["state"] == "MERGED" {
                    println!(
                        "#{pr} MERGED merge_commit={}",
                        string(&data, "/merge_commit/hash")
                    );
                    outcomes.insert(pr, true);
                } else if data["state"] != "OPEN" {
                    println!("#{pr} closed without merge");
                    outcomes.insert(pr, false);
                } else if self.context.forge == ForgeKind::GitHub {
                    let head = string(&data, "/source/commit/hash");
                    require(is_sha(head, 40, 40), format!("#{pr} has no full head SHA"))?;
                    if self.check_state(head)? == CheckState::Failed {
                        println!("#{pr} has failing head checks");
                        outcomes.insert(pr, false);
                    }
                } else if queue
                    .iter()
                    .find(|r| {
                        string(r, "/target/ref_name") == format!("bitbucket-merge-queue-{pr}")
                    })
                    .is_some_and(|run| {
                        string(run, "/state/name") == "COMPLETED"
                            && string(run, "/state/result/name") != "SUCCESSFUL"
                    })
                {
                    println!("#{pr} LEFT THE QUEUE");
                    outcomes.insert(pr, false);
                }
            }
            pending.retain(|p| !outcomes.contains_key(p));
            if !pending.is_empty() {
                clock.sleep(
                    Duration::from_secs(poll.max(1)).min(deadline.saturating_sub(clock.now())),
                );
            }
        }
        for pr in &pending {
            println!("#{pr} still pending after {} min", timeout / 60);
        }
        Ok(if !pending.is_empty() {
            3
        } else if outcomes.values().all(|merged| *merged) {
            0
        } else {
            1
        })
    }
}
