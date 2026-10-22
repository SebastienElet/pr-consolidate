#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::json;
use support::*;

#[test]
fn github_inventory_create_description_and_idempotent_review_use_native_transport() {
    let cli = ForgeCli::new("github", "main");
    let body = cli.body("Findings: no demonstrated defect. Evidence: fixture checks.");
    let out = cli.workspace.root.join("inventory.json");
    cli.run(&["inventory", "--out", out.to_str().unwrap()])
        .code(0);
    assert_eq!(
        load(out)["prs"][0]["paths"],
        json!(["src/current.rs", "src/previous.rs"])
    );
    cli.run(&["create", "batch/new", "Batch", body.to_str().unwrap()])
        .code(0);
    for mode in ["dry-run", "apply", "apply"] {
        cli.run(&["review-comment", "800", HEAD, body.to_str().unwrap(), mode])
            .code(0);
    }
    let old = cli.body("Original description");
    let new = cli.workspace.root.join("new.md");
    write(&new, "Updated description");
    for args in [
        vec![
            "desc",
            "800",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
            "--check",
        ],
        vec!["desc", "800", old.to_str().unwrap(), new.to_str().unwrap()],
    ] {
        cli.run(&args).code(0);
    }
    let writes = cli.writes();
    assert_eq!(
        writes
            .iter()
            .map(|v| v[0].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["POST", "POST", "PATCH"]
    );
    let state = cli.state();
    assert_eq!(
        state["resources"]["/repos/workspace/project/issues/800/comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for args in state["argv"].as_array().unwrap() {
        let args = args.as_array().unwrap();
        assert!(
            args.windows(2)
                .any(|a| a == [json!("--hostname"), json!("github.com")])
        );
        if let Some(index) = args.iter().position(|a| a == "--input") {
            assert_eq!(args[index + 1], "-");
        }
    }
}

#[test]
fn bitbucket_creation_preserves_literal_body_and_discovered_primary() {
    let cli = ForgeCli::new("bitbucket", "release/trunk");
    let text = "Literal $(touch injected) `not-a-command` and \"quotes\". Unicode: café.";
    let body = cli.body(text);
    cli.run(&["create", "batch/new", "Batch", body.to_str().unwrap()])
        .code(0);
    let state = cli.state();
    let created = &state["resources"]["/repositories/workspace/project/pullrequests/901"];
    assert_eq!(created["destination"]["branch"]["name"], "release/trunk");
    assert_eq!(created["close_source_branch"], false);
    assert_eq!(created["description"], text);
    assert!(!cli.work.join("injected").exists());
    assert_eq!(
        state["argv"].as_array().unwrap().last().unwrap()[1],
        "/repositories/workspace/project/pullrequests"
    );
}

#[test]
fn github_summary_updates_owned_review_surface_without_approval() {
    let cli = ForgeCli::new("github", "main");
    let mut state = cli.state();
    state["resources"]["/repos/workspace/project/pulls/800/reviews"] = json!([{"id":71,"body":format!("<!-- pr-consolidate:review pr=800 head={HEAD} -->\nOriginal review"),"user":{"id":7,"login":"maintainer"},"state":"COMMENTED","commit_id":HEAD}]);
    cli.save(&state);
    let body = cli.body("Updated findings.");
    cli.run(&[
        "review-comment",
        "800",
        HEAD,
        body.to_str().unwrap(),
        "apply",
    ])
    .code(0);
    let writes = cli.writes();
    assert_eq!(writes[0][0], "PUT");
    assert_eq!(
        writes[0][1],
        "/repos/workspace/project/pulls/800/reviews/71"
    );
    assert_eq!(
        writes[0][2].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["body"]
    );
}

#[test]
fn later_comment_page_updates_existing_comment_and_incomplete_files_refuse() {
    let cli = ForgeCli::new("github", "main");
    let mut state = cli.state();
    let mut comments: Vec<_> = (1..=100)
        .map(|id| json!({"id":id,"body":"Unrelated","user":{"id":7,"login":"maintainer"}}))
        .collect();
    comments.push(json!({"id":101,"body":format!("<!-- pr-consolidate:review pr=800 head={HEAD} -->\nReview"),"user":{"id":7,"login":"maintainer"}}));
    state["resources"]["/repos/workspace/project/issues/800/comments"] = json!(comments);
    cli.save(&state);
    let body = cli.body("Updated findings.");
    cli.run(&[
        "review-comment",
        "800",
        HEAD,
        body.to_str().unwrap(),
        "apply",
    ])
    .code(0);
    let mut state = cli.state();
    assert_eq!(
        state["resources"]["/repos/workspace/project/issues/800/comments"]
            .as_array()
            .unwrap()
            .len(),
        101
    );
    state["resources"]["/repos/workspace/project/pulls"][0]["changed_files"] = json!(2);
    state["resources"]["/repos/workspace/project/pulls/800"]["changed_files"] = json!(2);
    cli.save(&state);
    let out = cli.workspace.root.join("out.json");
    cli.run(&["inventory", "--out", out.to_str().unwrap()])
        .code(1);
    assert!(!out.exists());
}

#[test]
fn native_replacement_observes_source_review_ancestry_notice_then_close() {
    let cli = ForgeCli::new("github", "main");
    let body = cli.body("Findings and evidence.");
    cli.run(&[
        "review-comment",
        "800",
        HEAD,
        body.to_str().unwrap(),
        "apply",
    ])
    .code(0);
    let plan = cli.workspace.root.join("replacement.json");
    save(
        &plan,
        &json!({"lot_pr":900,"items":[{"id":800,"head":HEAD}]}),
    );
    for (command, mode) in [
        ("replace", "dry-run"),
        ("decline", "apply"),
        ("replace", "apply"),
    ] {
        cli.run(&[command, plan.to_str().unwrap(), mode]).code(0);
    }
    let writes = cli.writes();
    assert_eq!(
        writes
            .iter()
            .map(|v| v[0].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["POST", "POST", "PATCH"]
    );
    assert_eq!(
        cli.state()["resources"]["/repos/workspace/project/pulls/800"]["state"],
        "closed"
    );
}

#[test]
fn accepted_transport_failure_reconciles_without_duplicate_review_for_both_forges() {
    for forge in ["github", "bitbucket"] {
        let cli = ForgeCli::new(forge, "main");
        let mut state = cli.state();
        state["accepted_write_failure"] = json!(true);
        cli.save(&state);
        let body = cli.body("Findings and evidence.");
        let result = cli.run(&[
            "review-comment",
            "800",
            HEAD,
            body.to_str().unwrap(),
            "apply",
        ]);
        result.code(1);
        assert!(!result.text().contains("private-cli-output"));
        cli.run(&[
            "review-comment",
            "800",
            HEAD,
            body.to_str().unwrap(),
            "apply",
        ])
        .code(0);
        assert_eq!(cli.writes().len(), 1);
    }
}

#[test]
fn invalid_json_and_transport_failures_never_expose_cli_output_or_write_inventory() {
    for forge in ["github", "bitbucket"] {
        for mode in ["invalid_output", "exit_code"] {
            let cli = ForgeCli::new(forge, "main");
            let mut state = cli.state();
            state[mode] = if mode == "exit_code" {
                json!(23)
            } else {
                json!(true)
            };
            cli.save(&state);
            let out = cli.workspace.root.join("out.json");
            let result = cli.run(&["inventory", "--out", out.to_str().unwrap()]);
            result.code(1);
            assert!(!result.text().contains("private-cli-output"));
            assert!(!out.exists());
        }
    }
}

#[test]
fn argument_errors_refuse_before_discovery_or_forge_access() {
    let cli = ForgeCli::new("github", "main");
    for args in [
        vec!["review-comment", "0", HEAD, "body", "apply"],
        vec!["review-comment", "1", HEAD, "body", "unknown"],
        vec!["replace", "plan"],
        vec!["watch", "true"],
        vec!["inventory"],
    ] {
        let mut command = cli.command(&args);
        command.current_dir(&cli.workspace.root);
        observe(&mut command, None).code(2);
    }
    assert!(cli.state()["calls"].as_array().unwrap().is_empty());
}
