#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::json;
use std::fs;
use support::*;

#[test]
fn nonstandard_primary_preserves_native_hooks_origin_human_identity_and_push() {
    let campaign = Campaign::new();
    campaign.setup();
    let repo = campaign.repo();
    assert_eq!(
        git(&repo, &["remote", "get-url", "origin"]),
        campaign.plan["remote_url"].as_str().unwrap()
    );
    assert_eq!(git(&repo, &["config", "core.hooksPath"]), ".hooks");
    for number in ["1", "2"] {
        campaign.run("merge", &[number]).code(0);
        let message = campaign.root().join("message.txt");
        write(&message, format!("Merge source {number}"));
        campaign
            .run(
                "commit",
                &[message.to_str().unwrap(), &format!("pr-{number}")],
            )
            .code(0);
    }
    assert!(repo.join(".git/hook-ran").exists());
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%an <%ae>"]),
        "Human Reviewer <human@example.invalid>"
    );
    let candidate = git(&repo, &["rev-parse", "HEAD"]);
    campaign.run("push", &[&candidate]).code(0);
    assert_eq!(
        git(
            &campaign.root().join("origin.git"),
            &["rev-parse", "batch/example"]
        ),
        candidate
    );
}

#[test]
fn explicit_empty_build_retains_native_hook_and_preflight() {
    let mut campaign = Campaign::new();
    campaign.plan["commands"]["build"] = json!([]);
    campaign.save();
    campaign.setup();
    campaign.run("merge", &["1"]).code(0);
    let message = campaign.root().join("message.txt");
    write(&message, "Merge source without build");
    let mut command = campaign.command("commit", &[message.to_str().unwrap(), "no-build"]);
    for key in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
        command.env(key, "Codex Agent");
    }
    for key in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
        command.env(key, "agent@example.invalid");
    }
    observe(&mut command, None).code(0);
    assert!(campaign.repo().join(".git/hook-ran").exists());
    assert!(
        read(campaign.root().join("logs/build-no-build.log")).contains("no project build step")
    );
    campaign.run("preflight", &[]).code(0);
}

#[test]
fn moved_source_or_base_refuses_before_bundle() {
    for branch in ["feature/1", "trunk"] {
        let campaign = Campaign::new();
        let seed = campaign.root().join("seed");
        git(&seed, &["checkout", branch]);
        git(&seed, &["commit", "-q", "--allow-empty", "-m", "moved"]);
        git(&seed, &["push", "-q", "origin", branch]);
        let result = campaign.run("bundle", &[]);
        result.code(2);
        assert!(result.stderr.contains("moved"));
        assert!(!campaign.root().join("artifacts/fixture.bundle").exists());
    }
}

#[test]
fn wrong_snapshot_and_shared_external_metadata_refuse_before_clone() {
    for shared in [false, true] {
        let mut campaign = Campaign::new();
        campaign.run("bundle", &[]).code(0);
        let external = Workspace::new();
        if shared {
            fs::remove_dir_all(campaign.root().join("source")).unwrap();
            let clone = external.root.join("shared");
            git(
                &external.root,
                &[
                    "clone",
                    "-q",
                    campaign.plan["remote_url"].as_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
            git(
                &clone,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    campaign.root().join("source").to_str().unwrap(),
                    "trunk",
                ],
            );
        } else {
            campaign.plan["prs"][0]["head"] = campaign.plan["base_sha"].clone();
            campaign.save();
        }
        let result = campaign.run(
            "setup",
            &[campaign
                .root()
                .join("artifacts/fixture.bundle")
                .to_str()
                .unwrap()],
        );
        result.code(2);
        if shared {
            assert!(result.stderr.contains("outside root"));
        }
        assert!(!campaign.repo().exists());
    }
}

#[test]
fn absent_project_hook_configuration_does_not_force_hooks_path() {
    let mut campaign = Campaign::new();
    campaign.plan.as_object_mut().unwrap().remove("hooks_path");
    campaign.save();
    campaign.setup();
    let mut command = std::process::Command::new("git");
    clean(&mut command, &campaign.workspace.home);
    command
        .arg("-C")
        .arg(campaign.repo())
        .args(["config", "--get", "core.hooksPath"]);
    let result = observe(&mut command, None);
    result.code(1);
    assert!(result.stdout.is_empty());
}

#[test]
fn branch_collisions_and_escaping_clones_refuse() {
    for (key, value) in [
        ("branch", "trunk"),
        ("branch", "feature/1"),
        ("source", "/tmp/shared"),
        ("root", "/tmp/../shared"),
    ] {
        let mut campaign = Campaign::new();
        campaign.plan[key] = json!(value);
        campaign.save();
        campaign.run("bundle", &[]).code(2);
    }
}

#[test]
fn commit_refuses_conflicts_dirty_tree_failed_build_and_failed_hook_without_moving_head() {
    for fault in [
        "marker",
        "newline-marker",
        "unstaged",
        "untracked",
        "build",
        "hook",
    ] {
        let mut campaign = Campaign::new();
        campaign.setup();
        let repo = campaign.repo();
        let before = git(&repo, &["rev-parse", "HEAD"]);
        write(
            repo.join("base.txt"),
            if fault == "marker" {
                "<<<<<<< unresolved\n"
            } else {
                "change\n"
            },
        );
        git(&repo, &["add", "base.txt"]);
        match fault {
            "newline-marker" => {
                write(repo.join("unsafe\nname.txt"), "<<<<<<< unresolved\n");
                git(&repo, &["add", "unsafe\nname.txt"]);
            }
            "unstaged" => write(repo.join("base.txt"), "different\n"),
            "untracked" => write(repo.join("unknown.txt"), "untracked"),
            "build" => {
                campaign.plan["commands"]["build"][0]["argv"] =
                    json!(["missing-project-builder-xyz"]);
                campaign.save();
            }
            "hook" => {
                hook(repo.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 23\n");
                git(&repo, &["config", "core.hooksPath", ".git/hooks"]);
            }
            _ => {}
        }
        let message = campaign.root().join("message.txt");
        write(&message, "change");
        let result = campaign.run("commit", &[message.to_str().unwrap(), "failure"]);
        assert_ne!(result.code, 0, "{}", result.text());
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before);
    }
}

#[test]
fn push_refuses_unrelated_history_agent_origin_and_nonfastforward_without_remote_mutation() {
    for fault in ["unrelated", "agent", "origin", "divergent"] {
        let campaign = Campaign::new();
        campaign.setup();
        let repo = campaign.repo();
        if fault == "unrelated" {
            git(&repo, &["checkout", "--orphan", "orphan"]);
            git(&repo, &["rm", "-rf", "."]);
            git(&repo, &["commit", "-q", "--allow-empty", "-m", "unrelated"]);
            git(&repo, &["branch", "-f", "batch/example", "HEAD"]);
        } else {
            git(
                &repo,
                &[
                    "-c",
                    if fault == "agent" {
                        "user.name=Codex Agent"
                    } else {
                        "user.name=Human"
                    },
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "candidate",
                ],
            );
        }
        let candidate = git(&repo, &["rev-parse", "HEAD"]);
        if fault == "origin" {
            git(
                &campaign.root().join("push"),
                &[
                    "remote",
                    "set-url",
                    "--push",
                    "origin",
                    campaign.root().join("wrong.git").to_str().unwrap(),
                ],
            );
        }
        if fault == "divergent" {
            git(
                &campaign.root().join("seed"),
                &["push", "-q", "origin", "feature/2:refs/heads/batch/example"],
            );
        }
        let before = git(
            campaign.root(),
            &[
                "ls-remote",
                campaign.plan["remote_url"].as_str().unwrap(),
                "refs/heads/batch/example",
            ],
        );
        campaign.run("push", &[&candidate]).code(2);
        assert_eq!(
            git(
                campaign.root(),
                &[
                    "ls-remote",
                    campaign.plan["remote_url"].as_str().unwrap(),
                    "refs/heads/batch/example"
                ]
            ),
            before
        );
    }
}

#[test]
fn changed_primary_blocks_bundle_setup_merge_commit_and_push_before_mutation() {
    for operation in ["bundle", "setup", "merge", "commit", "push"] {
        let mut campaign = Campaign::new();
        if operation == "setup" {
            campaign.run("bundle", &[]).code(0);
        }
        if ["merge", "commit", "push"].contains(&operation) {
            campaign.setup();
        }
        let message = campaign.root().join("message.txt");
        write(&message, "must not commit");
        let mut before = String::new();
        let mut status_before = String::new();
        let mut candidate = String::new();
        if operation == "commit" {
            write(campaign.repo().join("base.txt"), "staged candidate\n");
            git(&campaign.repo(), &["add", "base.txt"]);
        }
        if operation == "push" {
            git(
                &campaign.repo(),
                &["commit", "-q", "--allow-empty", "-m", "candidate"],
            );
            candidate = git(&campaign.repo(), &["rev-parse", "HEAD"]);
            git(&campaign.repo(), &["branch", "-m", "main"]);
        }
        if campaign.repo().exists() {
            before = git(&campaign.repo(), &["rev-parse", "HEAD"]);
            status_before = git(&campaign.repo(), &["status", "--porcelain"]);
        }
        campaign.switch_primary();
        if ["bundle", "setup", "push"].contains(&operation) {
            campaign.plan["branch"] = json!("main");
            campaign.save();
        }
        let bundle = campaign.root().join("artifacts/fixture.bundle");
        let args = match operation {
            "setup" => vec![bundle.to_str().unwrap()],
            "merge" => vec!["1"],
            "commit" => vec![message.to_str().unwrap(), "unsafe"],
            "push" => vec![candidate.as_str()],
            _ => vec![],
        };
        let result = campaign.run(operation, &args);
        result.code(2);
        assert!(result.stderr.contains("primary branch"));
        if campaign.repo().exists() {
            assert_eq!(git(&campaign.repo(), &["rev-parse", "HEAD"]), before);
            assert_eq!(
                git(&campaign.repo(), &["status", "--porcelain"]),
                status_before
            );
        } else {
            assert!(!campaign.repo().exists());
        }
        if operation == "setup" {
            assert!(
                git(
                    &campaign.root().join("source"),
                    &["for-each-ref", "--format=%(refname)", "refs/lots/fixture"]
                )
                .is_empty()
            );
        }
        if operation == "bundle" {
            assert!(!bundle.exists());
        }
        assert_eq!(
            git(
                &campaign.root().join("origin.git"),
                &["rev-parse", "refs/heads/main"]
            ),
            campaign.plan["base_sha"].as_str().unwrap()
        );
    }
}

#[test]
fn missing_or_detached_primary_refuses_snapshot() {
    for detached in [false, true] {
        let campaign = Campaign::new();
        let origin = campaign.root().join("origin.git");
        if detached {
            git(
                &origin,
                &[
                    "update-ref",
                    "--no-deref",
                    "HEAD",
                    campaign.plan["base_sha"].as_str().unwrap(),
                ],
            );
        } else {
            git(&origin, &["symbolic-ref", "HEAD", "refs/heads/missing"]);
        }
        let result = campaign.run("bundle", &[]);
        result.code(2);
        assert!(result.stderr.contains("primary branch"));
    }
}

#[test]
fn hostile_git_environment_cannot_bypass_real_hook_or_redirect_commit() {
    for redirect in [false, true] {
        let campaign = Campaign::new();
        campaign.setup();
        let repo = campaign.repo();
        let other = campaign.root().join("seed");
        hook(repo.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 23\n");
        git(&repo, &["config", "core.hooksPath", ".git/hooks"]);
        git(
            &other,
            &[
                "checkout",
                "-qb",
                "batch/example",
                campaign.plan["base_sha"].as_str().unwrap(),
            ],
        );
        for clone in [&repo, &other] {
            write(clone.join("base.txt"), "staged candidate\n");
            git(clone, &["add", "base.txt"]);
        }
        let before = git(&other, &["rev-parse", "HEAD"]);
        let message = campaign.root().join("message.txt");
        write(&message, "must observe real hook in owned repo");
        let mut command = campaign.command("commit", &[message.to_str().unwrap(), "bypass"]);
        if redirect {
            command
                .env("GIT_DIR", other.join(".git"))
                .env("GIT_WORK_TREE", &other)
                .env("GIT_INDEX_FILE", other.join(".git/index"));
        } else {
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "core.hooksPath")
                .env("GIT_CONFIG_VALUE_0", "/dev/null");
        }
        let result = observe(&mut command, None);
        assert_ne!(result.code, 0, "{}", result.text());
        assert!(result.stdout.contains("build exit=0"));
        assert_eq!(git(&other, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            git(&repo, &["rev-parse", "HEAD"]),
            campaign.plan["base_sha"].as_str().unwrap()
        );
    }
}

#[test]
fn preflight_receives_and_redacts_secret_without_persisting_it() {
    let mut campaign = Campaign::new();
    campaign.plan["commands"]["check"] = json!([step(&["secret"], 3)]);
    campaign.save();
    campaign.setup();
    let token = "fixture-private-registry-value";
    let mut command = campaign.command("preflight", &[]);
    command.env("NPM_TOKEN", token);
    let result = observe(&mut command, None);
    result.code(0);
    let logs = fs::read_dir(campaign.root().join("logs"))
        .unwrap()
        .map(|p| read(p.unwrap().path()))
        .collect::<String>();
    assert!(logs.contains("credential-present=true"));
    assert!(!format!("{}{logs}{}", result.text(), read(&campaign.path)).contains(token));
    assert!(!campaign.repo().join(".npmrc").exists());
}

#[test]
fn failed_or_dirty_check_refuses_and_reinstall_requires_explicit_command() {
    for command in [
        step(&["exit", "19", "failure"], 3),
        step(&["write", "base.txt", "changed"], 3),
    ] {
        let mut campaign = Campaign::new();
        campaign.plan["commands"]["check"] = json!([command]);
        campaign.save();
        campaign.setup();
        assert_ne!(campaign.run("preflight", &[]).code, 0);
    }
    let mut campaign = Campaign::new();
    campaign.setup();
    let result = campaign.run("resync", &[]);
    result.code(2);
    assert!(result.stderr.contains("no reinstall command"));
    campaign.plan["commands"]["reinstall"] = json!([step(&["print", "reinstalled"], 3)]);
    campaign.save();
    campaign.run("resync", &[]).code(0);
}

#[test]
fn base_conflicts_require_declared_lockfile_and_project_resolution() {
    for declared in [false, true] {
        let mut campaign = Campaign::new();
        if declared {
            campaign.plan["lockfiles"] = json!(["base.txt"]);
            campaign.plan["commands"]["lockfile"] =
                json!([step(&["write", "base.txt", "resolved lock state\n"], 3)]);
            campaign.save();
        }
        campaign.setup();
        let repo = campaign.repo();
        let seed = campaign.root().join("seed");
        write(repo.join("base.txt"), "lot version\n");
        git(&repo, &["add", "base.txt"]);
        git(&repo, &["commit", "-qm", "lot changes"]);
        git(&seed, &["checkout", "trunk"]);
        write(seed.join("base.txt"), "main version\n");
        git(&seed, &["add", "base.txt"]);
        git(&seed, &["commit", "-qm", "new base"]);
        let new_base = git(&seed, &["rev-parse", "HEAD"]);
        git(
            &repo,
            &[
                "fetch",
                "--no-prune",
                seed.to_str().unwrap(),
                "+trunk:refs/remotes/origin/trunk",
            ],
        );
        let message = campaign.root().join("message.txt");
        write(&message, "Merge updated baseline");
        let result = campaign.run("merge-base", &[&new_base, message.to_str().unwrap()]);
        if declared {
            result.code(0);
            assert_eq!(read(repo.join("base.txt")), "resolved lock state\n");
            assert_eq!(
                git(&repo, &["show", "--format=%P", "--no-patch"])
                    .split_whitespace()
                    .last()
                    .unwrap(),
                new_base
            );
        } else {
            result.code(2);
            assert!(result.stderr.contains("outside declared lockfiles"));
            assert_eq!(git(&repo, &["rev-parse", "MERGE_HEAD"]), new_base);
        }
    }
}
