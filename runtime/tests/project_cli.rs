#![cfg(feature = "test-fixtures")]
mod support;
use serde_json::json;
use support::*;

#[test]
fn project_argv_is_literal_and_stdin_is_inherited() {
    let workspace = Workspace::new();
    let literal = "literal $(touch injected) `not-a-command` café";
    let (result, _) = workspace.project(
        json!({"build":[step(&["write","result",literal],3)]}),
        None,
        None,
    );
    result.code(0);
    assert_eq!(read(workspace.root.join("result")), literal);
    assert!(!workspace.root.join("injected").exists());
    let (result, log) = workspace.project(
        json!({"build":[step(&["stdin","supplied input\n"],2)]}),
        Some("supplied input\n"),
        None,
    );
    result.code(0);
    assert!(log.contains("supplied input"));
}

#[test]
fn nonzero_exit_stops_following_commands() {
    let workspace = Workspace::new();
    let (result, log) = workspace.project(
        json!({"build":[step(&["exit","17","first"],3),step(&["print","unexpected"],3)]}),
        None,
        None,
    );
    result.code(17);
    assert!(log.contains("first"));
    assert!(!log.contains("unexpected"));
}

#[test]
fn missing_phase_executable_and_malformed_specs_refuse() {
    for commands in [
        json!({}),
        json!({"check":[]}),
        json!({"build":[{"argv":"echo yes","timeout_seconds":3}]}),
        json!({"build":[{"argv":["missing-project-runtime-xyz"],"timeout_seconds":3}]}),
        json!({"build":[{"argv":["echo","yes"],"timeout_seconds":true}]}),
    ] {
        let workspace = Workspace::new();
        workspace.project(commands, None, None).0.code(2);
    }
}

#[test]
fn timeout_redacts_registry_secret_and_secret_argv_refuses() {
    let workspace = Workspace::new();
    let token = "private-fixture-secret";
    let (result, log) = workspace.project(
        json!({"build":[step(&["secret-sleep"],1)]}),
        None,
        Some(token),
    );
    result.code(124);
    assert!(!format!("{}{log}", result.text()).contains(token));
    assert!(log.contains("[REDACTED]"));
    let (result, log) = workspace.project(
        json!({"build":[step(&["print",token],3)]}),
        None,
        Some(token),
    );
    result.code(2);
    assert!(!format!("{}{log}", result.text()).contains(token));
}

#[test]
fn escaped_output_child_cannot_extend_timeout() {
    let workspace = Workspace::new();
    let pid = workspace.root.join("child.pid");
    let start = std::time::Instant::now();
    let (result, _) = workspace.project(
        json!({"build":[step(&["spawn","escaped",pid.to_str().unwrap()],1)]}),
        None,
        None,
    );
    let elapsed = start.elapsed();
    if pid.exists() {
        let pid: i32 = read(pid).parse().unwrap();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    result.code(124);
    assert!(elapsed.as_secs_f64() < 2.5, "{elapsed:?}");
}

#[test]
fn exited_parent_cannot_leave_a_late_candidate_mutation() {
    let workspace = Workspace::new();
    let marker = workspace.root.join("late-change");
    let (result, _) = workspace.project(
        json!({"build":[step(&["spawn","late",marker.to_str().unwrap()],1)]}),
        None,
        None,
    );
    result.code(124);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(!marker.exists());
}

#[test]
fn exited_parent_waits_for_output_child_and_complete_log() {
    let workspace = Workspace::new();
    let start = std::time::Instant::now();
    let (result, log) = workspace.project(
        json!({"build":[step(&["spawn","short","unused"],2)]}),
        None,
        None,
    );
    result.code(0);
    assert!(start.elapsed().as_millis() >= 300);
    assert!(log.contains("completed child"));
}
