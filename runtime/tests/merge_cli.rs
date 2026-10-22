#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::json;
use std::path::{Path, PathBuf};
use support::*;

struct MergeFixture {
    cli: ForgeCli,
    work: PathBuf,
    push: PathBuf,
    candidate: String,
}
impl MergeFixture {
    fn new() -> Self {
        let cli = ForgeCli::new("github", "main");
        let root = &cli.workspace.root;
        let origin = root.join("merge-origin.git");
        let work = root.join("merge-work");
        let push = root.join("push-clone");
        git(
            root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        Self::commit(&work, "app.txt", "base\n");
        git(&work, &["push", "-q", "origin", "main"]);
        git(&work, &["checkout", "-q", "-b", "feat/lot"]);
        Self::commit(&work, "feature.txt", "feature\n");
        Self::commit(&work, "app.txt", "base\nfeature\n");
        git(&work, &["push", "-q", "origin", "feat/lot"]);
        git(&work, &["checkout", "-q", "main"]);
        Self::commit(&work, "other.txt", "main moved\n");
        git(&work, &["push", "-q", "origin", "main"]);
        git(
            root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                push.to_str().unwrap(),
            ],
        );
        let candidate = git(&work, &["rev-parse", "feat/lot"]);
        git(&push, &["remote", "set-url", "origin", &cli.url]);
        git(
            &push,
            &[
                "config",
                &format!("url.file://{}.insteadOf", origin.display()),
                &cli.url,
            ],
        );
        Self {
            cli,
            work,
            push,
            candidate,
        }
    }
    fn commit(root: &Path, name: &str, text: &str) {
        write(root.join(name), text);
        git(root, &["add", name]);
        git(root, &["commit", "-q", "-m", &format!("change {name}")]);
    }
    fn squash(&self, wrong_tree: bool, agent: bool) {
        let tree = if wrong_tree {
            git(&self.work, &["rev-parse", "feat/lot^{tree}"])
        } else {
            git(
                &self.work,
                &["merge-tree", "--write-tree", "main", "feat/lot"],
            )
            .lines()
            .next()
            .unwrap()
            .to_owned()
        };
        let merged = git(
            &self.work,
            &[
                "-c",
                if agent {
                    "user.name=Claude"
                } else {
                    "user.name=Fixture Maintainer"
                },
                "commit-tree",
                &tree,
                "-p",
                "main",
                "-m",
                "Lot (pull request #958)",
            ],
        );
        git(
            &self.work,
            &["push", "-q", "origin", &format!("{merged}:refs/heads/main")],
        );
        let mut state = self.cli.state();
        let mut pull = github_pull(958, &self.candidate, true);
        pull["merge_commit_sha"] = json!(merged);
        state["resources"]["/repos/workspace/project/pulls/958"] = pull;
        self.cli.save(&state);
    }
}

#[test]
fn squash_verification_uses_merged_tree_and_refuses_foreign_clone_agent_head_or_state() {
    for fault in ["none", "forge", "tree", "agent", "candidate", "open"] {
        let fixture = MergeFixture::new();
        fixture.squash(fault == "tree", fault == "agent");
        if fault == "forge" {
            git(
                &fixture.push,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://bitbucket.org/workspace/project.git",
                ],
            );
        }
        if fault == "open" {
            let mut state = fixture.cli.state();
            let pr = &mut state["resources"]["/repos/workspace/project/pulls/958"];
            pr["state"] = json!("open");
            pr["merged"] = json!(false);
            pr["merged_at"] = json!(null);
            fixture.cli.save(&state);
        }
        let candidate = if fault == "candidate" {
            git(&fixture.work, &["rev-parse", "main"])
        } else {
            fixture.candidate.clone()
        };
        let result = fixture.cli.run(&[
            "verify-merge",
            "958",
            &candidate,
            fixture.push.to_str().unwrap(),
        ]);
        result.code(if fault == "none" { 0 } else { 2 });
        let diagnostic = match fault {
            "none" => "verified: #958",
            "forge" => "does not match",
            "tree" => "REFUSED: tree of",
            "agent" => "names an agent identity",
            "candidate" => "is not the PR head",
            _ => "REFUSED: #958 is not MERGED",
        };
        assert!(result.text().contains(diagnostic), "{}", result.text());
    }
}
