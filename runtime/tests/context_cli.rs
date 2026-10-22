#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::Value;
use std::fs;
use support::*;

#[test]
fn nested_and_explicit_repository_discover_remote_primary_for_both_forges() {
    for forge in ["github", "bitbucket"] {
        for branch in ["main", "release/trunk"] {
            let cli = ForgeCli::new(forge, branch);
            git(&cli.work, &["checkout", "-qb", "local-feature"]);
            git(
                &cli.work,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/stale",
                ],
            );
            let nested = cli.work.join("nested");
            fs::create_dir(&nested).unwrap();
            for explicit in [false, true] {
                let args = if explicit {
                    vec!["--repository", cli.work.to_str().unwrap(), "context"]
                } else {
                    vec!["context"]
                };
                let mut command = cli.command(&args);
                command.current_dir(if explicit {
                    &cli.workspace.root
                } else {
                    &nested
                });
                let result = observe(&mut command, None);
                result.code(0);
                let context: Value = serde_json::from_str(&result.stdout).unwrap();
                assert_eq!(context["root"], cli.work.to_str().unwrap());
                assert_eq!(context["repository"], "workspace/project");
                assert_eq!(context["forge"], forge);
                assert_eq!(context["base_branch"], branch);
            }
            assert!(cli.state()["calls"].as_array().unwrap().is_empty());
        }
    }
}

#[test]
fn supported_ssh_https_and_hidden_repository_names() {
    let cli = ForgeCli::new("github", "main");
    for url in [
        "https://bitbucket.org/Workspace/Project.git",
        "git@bitbucket.org:Workspace/Project.git",
        "ssh://git@bitbucket.org/Workspace/Project.git",
        "ssh://git@ssh.bitbucket.org:443/Workspace/Project.git",
        "git@github.com:owner/.dotfiles.git",
        "https://github.com/owner/.dotfiles.git",
    ] {
        git(&cli.work, &["remote", "set-url", "origin", url]);
        git(
            &cli.work,
            &[
                "config",
                &format!("url.file://{}.insteadOf", cli.origin.display()),
                url,
            ],
        );
        let result = cli.run(&["context"]);
        result.code(0);
        let context: Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(
            context["repository"],
            if url.contains("dotfiles") {
                "owner/.dotfiles"
            } else {
                "workspace/project"
            }
        );
    }
}

#[test]
fn invalid_origins_refuse_without_api_output_or_secret_disclosure() {
    let cli = ForgeCli::new("github", "main");
    for url in [
        "https://gitlab.com/workspace/project.git",
        "file:///tmp/project.git",
        "https://tokenSECRET@bitbucket.org/workspace/project.git",
        "https://user:SECRET@bitbucket.org/workspace/project.git",
        "ssh://git:SECRET@bitbucket.org/workspace/project.git",
        "git@bitbucket.org:workspace/../project.git",
        "git@bitbucket.org:workspace/.git",
        "https://bitbucket.org/workspace/project/extra.git",
        "https://bitbucket.org/../project.git",
        "https://bitbucket.org/workspace/%2e%2e.git",
        "https://bitbucket.org/workspace/project.git?SECRET",
        "https://bitbucket.org/workspace/project.git#SECRET",
        "https://bitbucket.org:invalid/x/y",
        "git@bitbucket.org:workspace/project.git/",
        "ssh://git@ssh.bitbucket.org:22/workspace/project.git",
        "https://bitbucket.org//workspace/project.git",
        "https://bitbucket.org/workspace/project.git ",
        "https://bitbucket.org/workspace/project.git\n",
        "https://bitbucket.org/workspace/project.git\t",
    ] {
        git(&cli.work, &["remote", "set-url", "origin", url]);
        let out = cli.workspace.root.join("inventory.json");
        let result = cli.run(&["inventory", "--out", out.to_str().unwrap()]);
        result.code(2);
        assert!(!result.text().contains("SECRET"));
        assert!(!out.exists());
    }
    assert!(cli.state()["calls"].as_array().unwrap().is_empty());
}

#[test]
fn absent_multiple_and_foreign_push_origins_refuse_before_api() {
    for fault in ["missing", "multiple", "push"] {
        let cli = ForgeCli::new("github", "main");
        match fault {
            "missing" => {
                git(&cli.work, &["remote", "remove", "origin"]);
            }
            "multiple" => {
                git(
                    &cli.work,
                    &[
                        "config",
                        "--add",
                        "remote.origin.url",
                        "https://github.com/other/repo.git",
                    ],
                );
            }
            _ => {
                git(
                    &cli.work,
                    &[
                        "config",
                        "remote.origin.pushurl",
                        "https://github.com/other/repo.git",
                    ],
                );
            }
        }
        cli.run(&["context"]).code(2);
        assert!(cli.state()["calls"].as_array().unwrap().is_empty());
    }
}

#[test]
fn hostile_git_environment_cannot_redirect_or_inject_discovery() {
    let cli = ForgeCli::new("github", "main");
    let foreign = ForgeCli::new("github", "trunk");
    let mut command = cli.command(&["context"]);
    command
        .env("GIT_DIR", foreign.work.join(".git"))
        .env("GIT_WORK_TREE", &foreign.work)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.origin.url")
        .env(
            "GIT_CONFIG_VALUE_0",
            "https://github.com/foreign/repository.git",
        );
    let result = observe(&mut command, None);
    result.code(0);
    let context: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(context["root"], cli.work.to_str().unwrap());
    assert_eq!(context["base_branch"], "main");
}

#[test]
fn unresolved_detached_and_failed_remote_head_queries_refuse() {
    for fault in ["unborn", "detached", "missing"] {
        let cli = ForgeCli::new("github", "main");
        match fault {
            "unborn" => {
                git(&cli.origin, &["symbolic-ref", "HEAD", "refs/heads/missing"]);
            }
            "detached" => {
                git(
                    &cli.origin,
                    &[
                        "update-ref",
                        "--no-deref",
                        "HEAD",
                        &git(&cli.work, &["rev-parse", "HEAD"]),
                    ],
                );
            }
            _ => fs::rename(&cli.origin, cli.workspace.root.join("gone.git")).unwrap(),
        }
        cli.run(&["context"]).code(2);
    }
}
