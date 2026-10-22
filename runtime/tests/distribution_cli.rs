#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use support::*;

#[test]
fn missing_runtime_and_unknown_native_entrypoint_refuse_without_fallback() {
    let workspace = Workspace::new();
    let scripts_dir = workspace.root.join("scripts");
    fs::create_dir(&scripts_dir).unwrap();
    for (name, diagnostic) in [
        ("forge", "native runtime is missing"),
        ("unexpected-entrypoint", "unknown native entrypoint"),
    ] {
        let path = scripts_dir.join(name);
        executable(&path, &scripts().join("forge"));
        let result = observe(Command::new(path).arg("context"), None);
        result.code(2);
        assert!(result.stderr.contains(diagnostic));
    }
}

#[test]
fn native_utilities_and_project_commands_need_no_interpreter_in_path() {
    let workspace = Workspace::new();
    let evidence = workspace.root.join("evidence");
    write(&evidence, "Native runtime only");
    let mut command = workspace.command("native", &["digest", evidence.to_str().unwrap()]);
    command.env("PATH", &workspace.root);
    let result = observe(&mut command, None);
    result.code(0);
    use sha2::{Digest, Sha256};
    assert_eq!(
        result.stdout.trim(),
        format!("{:x}", Sha256::digest(b"Native runtime only"))
    );
    let plan = workspace.root.join("plan.json");
    let log = workspace.root.join("build.log");
    save(
        &plan,
        &json!({"commands":{"build":[step(&["print","native"],2)]}}),
    );
    let mut command = workspace.command(
        "project-commands",
        &[
            plan.to_str().unwrap(),
            "build",
            workspace.root.to_str().unwrap(),
            log.to_str().unwrap(),
        ],
    );
    command.env("PATH", &workspace.root);
    observe(&mut command, None).code(0);
    assert!(read(log).contains("native"));
}

#[test]
fn evidence_manifest_detects_modified_copy() {
    let workspace = Workspace::new();
    write(workspace.root.join("evidence.log"), "candidate evidence");
    workspace
        .run("native", &["manifest", workspace.root.to_str().unwrap()])
        .code(0);
    workspace
        .run("native", &["verify", workspace.root.to_str().unwrap()])
        .code(0);
    write(workspace.root.join("evidence.log"), "modified evidence");
    let result = workspace.run("native", &["verify", workspace.root.to_str().unwrap()]);
    result.code(2);
    assert!(
        result
            .stderr
            .contains("archive digest mismatch: ./evidence.log")
    );
}

fn builder(workspace: &Workspace, status: i32) -> Command {
    fs::create_dir_all(workspace.root.join("runtime")).unwrap();
    write(
        workspace.root.join("runtime/Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.0.1\"\n",
    );
    let bin = workspace.root.join("toolchain");
    fs::create_dir_all(&bin).unwrap();
    executable(bin.join("cargo"), Path::new(FIXTURE));
    let mut command = Command::new(env!("CARGO_BIN_EXE_build-runtime"));
    clean(&mut command, &workspace.home);
    command
        .current_dir(&workspace.root)
        .env("PATH", &bin)
        .env("FIXTURE_CARGO_STATUS", status.to_string());
    command
}

#[test]
fn failed_or_incomplete_build_preserves_previous_runtime_and_entrypoints() {
    let host =
        pr_consolidate::distribution::host(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    for (status, runtime_ready, expected) in [(17, false, 17), (0, false, 2), (0, true, 2)] {
        let workspace = Workspace::new();
        let mut command = builder(&workspace, status);
        let entrypoint = workspace.root.join("skills/pr-consolidate/scripts/native");
        fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
        executable(&entrypoint, &scripts().join("native"));
        let previous = fs::read(&entrypoint).unwrap();
        let installed = workspace
            .root
            .join("skills/pr-consolidate/bin")
            .join(host.platform)
            .join("pr-consolidate");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        write(&installed, "previous runtime");
        if runtime_ready {
            let release = workspace
                .root
                .join("runtime/target")
                .join(host.target)
                .join("release/pr-consolidate");
            fs::create_dir_all(release.parent().unwrap()).unwrap();
            write(release, "incomplete new runtime");
        }
        observe(&mut command, None).code(expected);
        assert_eq!(fs::read(&entrypoint).unwrap(), previous);
        assert_eq!(read(installed), "previous runtime");
    }
}

#[test]
fn rebuild_replaces_native_image_while_previous_invocation_finishes() {
    let workspace = Workspace::new();
    let mut command = builder(&workspace, 0);
    let host =
        pr_consolidate::distribution::host(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let release = workspace
        .root
        .join("runtime/target")
        .join(host.target)
        .join("release");
    fs::create_dir_all(&release).unwrap();
    executable(
        release.join("pr-consolidate-launcher"),
        &scripts().join("forge"),
    );
    executable(release.join("pr-consolidate"), Path::new(FIXTURE));
    observe(&mut command, None).code(0);
    let installed = workspace
        .root
        .join("skills/pr-consolidate/bin")
        .join(host.platform)
        .join("pr-consolidate");
    let ready = workspace.root.join("ready");
    let mut running = Command::new(&installed)
        .arg("wait")
        .arg(&ready)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !ready.exists()
        && running.try_wait().unwrap().is_none()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !ready.exists() {
        running.kill().ok();
        running.wait().ok();
        panic!("old invocation did not become ready");
    }
    executable(
        release.join("pr-consolidate"),
        Path::new(env!("CARGO_BIN_EXE_pr-consolidate")),
    );
    let result = observe(&mut command, None);
    drop(running.stdin.take());
    let old_status = running.wait().unwrap();
    result.code(0);
    assert!(old_status.success());
    observe(&mut Command::new(installed), None).code(2);
}
