#![allow(dead_code)]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const BATCH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";
pub const FIXTURE: &str = env!("CARGO_BIN_EXE_cli-fixture");

pub fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/pr-consolidate/scripts")
}

pub struct Observed {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
impl Observed {
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
    pub fn code(&self, expected: i32) {
        assert_eq!(self.code, expected, "{}", self.text());
    }
    pub fn success(&self) -> String {
        self.code(0);
        self.stdout.trim().to_owned()
    }
}

pub fn observe(command: &mut Command, input: Option<&str>) -> Observed {
    let out = tempfile::tempfile().unwrap();
    let err = tempfile::tempfile().unwrap();
    command
        .stdout(out.try_clone().unwrap())
        .stderr(err.try_clone().unwrap());
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.process_group(0);
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            child.kill().ok();
            child.wait().ok();
            panic!("fixture watchdog expired: {command:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    use std::io::{Read, Seek};
    let read = |mut file: fs::File| {
        file.rewind().unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        text
    };
    Observed {
        code: status.code().unwrap_or(-1),
        stdout: read(out),
        stderr: read(err),
    }
}

pub fn write(path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
    fs::write(path, text).unwrap();
}
pub fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}
pub fn save(path: impl AsRef<Path>, value: &Value) {
    write(path, serde_json::to_vec_pretty(value).unwrap());
}
pub fn load(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
pub fn executable(path: impl AsRef<Path>, source: &Path) {
    fs::copy(source, &path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
pub fn hook(path: impl AsRef<Path>, body: &str) {
    write(&path, body);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
pub fn clean(command: &mut Command, home: &Path) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env_remove("NPM_TOKEN")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1");
}
pub fn git(root: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    clean(&mut command, root);
    command
        .args([
            "-c",
            "maintenance.autoDetach=false",
            "-c",
            "gc.autoDetach=false",
            "-c",
            "user.name=Fixture Maintainer",
            "-c",
            "user.email=maintainer@example.invalid",
            "-C",
        ])
        .arg(root)
        .args(args);
    observe(&mut command, None).success()
}
pub fn step(args: &[&str], timeout: u64) -> Value {
    let mut argv = vec![FIXTURE];
    argv.extend_from_slice(args);
    json!({"argv":argv,"timeout_seconds":timeout})
}

pub struct Workspace {
    pub temp: TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
}
impl Workspace {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        write(
            home.join(".gitconfig"),
            "[user]\n name = Fixture Maintainer\n email = maintainer@example.invalid\n[maintenance]\n autoDetach = false\n[gc]\n autoDetach = false\n",
        );
        Self { temp, root, home }
    }
    pub fn command(&self, name: &str, args: &[&str]) -> Command {
        let mut command = Command::new(scripts().join(name));
        clean(&mut command, &self.home);
        command.args(args).current_dir(&self.root);
        command
    }
    pub fn run(&self, name: &str, args: &[&str]) -> Observed {
        observe(&mut self.command(name, args), None)
    }
    pub fn project(
        &self,
        commands: Value,
        input: Option<&str>,
        token: Option<&str>,
    ) -> (Observed, String) {
        let plan = self.root.join("plan.json");
        let log = self.root.join("run.log");
        save(&plan, &json!({"commands":commands}));
        let mut command = self.command(
            "project-commands",
            &[
                plan.to_str().unwrap(),
                "build",
                self.root.to_str().unwrap(),
                log.to_str().unwrap(),
            ],
        );
        if let Some(token) = token {
            command.env("NPM_TOKEN", token);
        }
        let result = observe(&mut command, input);
        (result, fs::read_to_string(log).unwrap_or_default())
    }
}

pub struct Campaign {
    pub workspace: Workspace,
    pub plan: Value,
    pub path: PathBuf,
}
impl Campaign {
    pub fn new() -> Self {
        let workspace = Workspace::new();
        let root = &workspace.root;
        let origin = root.join("origin.git");
        let seed = root.join("seed");
        git(
            root,
            &[
                "init",
                "--bare",
                "-q",
                "--initial-branch=trunk",
                origin.to_str().unwrap(),
            ],
        );
        git(&origin, &["config", "maintenance.autoDetach", "false"]);
        git(&origin, &["config", "gc.autoDetach", "false"]);
        git(
            root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                seed.to_str().unwrap(),
            ],
        );
        write(seed.join("base.txt"), "base\n");
        write(seed.join(".gitignore"), "build.marker\ncheck.marker\n");
        fs::create_dir(seed.join(".hooks")).unwrap();
        hook(
            seed.join(".hooks/pre-commit"),
            "#!/bin/sh\nprintf hook > .git/hook-ran\n",
        );
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-qm", "initial"]);
        let base = git(&seed, &["rev-parse", "HEAD"]);
        let mut prs = Vec::new();
        for number in [1, 2] {
            let branch = format!("feature/{number}");
            git(&seed, &["checkout", "-qb", &branch, &base]);
            write(
                seed.join(format!("change-{number}.txt")),
                format!("change {number}\n"),
            );
            git(&seed, &["add", "."]);
            git(&seed, &["commit", "-qm", &format!("change {number}")]);
            prs.push(json!({"id":number,"head":git(&seed, &["rev-parse","HEAD"]),"branch":branch}));
        }
        git(
            &seed,
            &["push", "-q", "origin", "trunk", "feature/1", "feature/2"],
        );
        for name in ["source", "push"] {
            git(
                root,
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    root.join(name).to_str().unwrap(),
                ],
            );
        }
        let success = json!([step(&["print", "native project check"], 3)]);
        let plan = json!({"lot":"fixture","root":root,"branch":"batch/example","base_branch":"trunk","base_sha":base,"remote_url":origin,"source":root.join("source"),"push_clone":root.join("push"),"artifacts":root.join("artifacts"),"author_name":"Human Reviewer","author_email":"human@example.invalid","hooks_path":".hooks","commands":{"build":success,"check":success},"prs":prs});
        let path = root.join("lot.json");
        save(&path, &plan);
        Self {
            workspace,
            plan,
            path,
        }
    }
    pub fn root(&self) -> &Path {
        &self.workspace.root
    }
    pub fn repo(&self) -> PathBuf {
        self.root().join("repo")
    }
    pub fn save(&self) {
        save(&self.path, &self.plan);
    }
    pub fn command(&self, operation: &str, args: &[&str]) -> Command {
        let mut command = self.workspace.command(&format!("lot-{operation}.sh"), &[]);
        command.arg(&self.path).args(args);
        command
    }
    pub fn run(&self, operation: &str, args: &[&str]) -> Observed {
        observe(&mut self.command(operation, args), None)
    }
    pub fn setup(&self) {
        self.run("bundle", &[]).code(0);
        self.run(
            "setup",
            &[self
                .root()
                .join("artifacts/fixture.bundle")
                .to_str()
                .unwrap()],
        )
        .code(0);
    }
    pub fn switch_primary(&self) {
        let origin = self.root().join("origin.git");
        git(
            &origin,
            &[
                "update-ref",
                "refs/heads/main",
                self.plan["base_sha"].as_str().unwrap(),
            ],
        );
        git(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    }
}

pub struct ForgeCli {
    pub workspace: Workspace,
    pub work: PathBuf,
    pub origin: PathBuf,
    pub url: String,
    pub bin: PathBuf,
    pub state_path: PathBuf,
}
impl ForgeCli {
    pub fn new(forge: &str, branch: &str) -> Self {
        let workspace = Workspace::new();
        let root = &workspace.root;
        let origin = root.join("origin.git");
        let work = root.join("work");
        git(
            root,
            &[
                "init",
                "--bare",
                "-q",
                "-b",
                branch,
                origin.to_str().unwrap(),
            ],
        );
        git(&origin, &["config", "maintenance.autoDetach", "false"]);
        git(&origin, &["config", "gc.autoDetach", "false"]);
        fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q", "-b", branch]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&work, &["push", "-q", "origin", branch]);
        let host = if forge == "github" {
            "github.com"
        } else {
            "bitbucket.org"
        };
        let url = format!("git@{host}:workspace/project.git");
        git(&work, &["remote", "set-url", "origin", &url]);
        git(
            &work,
            &[
                "config",
                &format!("url.file://{}.insteadOf", origin.display()),
                &url,
            ],
        );
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for name in ["gh", "bkt"] {
            executable(bin.join(name), Path::new(FIXTURE));
        }
        let state_path = root.join("forge-state.json");
        let mut resources = serde_json::Map::new();
        let prefix = if forge == "github" {
            "/repos/workspace/project"
        } else {
            "/repositories/workspace/project"
        };
        if forge == "github" {
            resources.insert("/user".into(), json!({"id":7,"login":"maintainer"}));
            resources.insert(
                format!("{prefix}/pulls"),
                json!([github_pull(800, HEAD, false)]),
            );
            for (id, head, merged) in [(800, HEAD, false), (900, BATCH, true)] {
                resources.insert(
                    format!("{prefix}/pulls/{id}"),
                    github_pull(id, head, merged),
                );
                resources.insert(format!("{prefix}/issues/{id}/comments"), json!([]));
                resources.insert(format!("{prefix}/pulls/{id}/reviews"), json!([]));
                resources.insert(format!("{prefix}/pulls/{id}/files"),json!([{"sha":"dddddddddddddddddddddddddddddddddddddddd","filename":"src/current.rs","previous_filename":"src/previous.rs","status":"renamed","additions":3,"deletions":1}]));
            }
            resources.insert(
                format!("{prefix}/commits/{HEAD}/check-runs"),
                json!({"total_count":0,"check_runs":[]}),
            );
            resources.insert(format!("{prefix}/commits/{HEAD}/statuses"), json!([]));
            for target in [MERGE, BATCH] {
                resources.insert(
                    format!("{prefix}/compare/{HEAD}...{target}"),
                    json!({"merge_base_commit":{"sha":HEAD}}),
                );
            }
        } else {
            resources.insert(
                "/user".into(),
                json!({"uuid":"{11111111-2222-3333-4444-555555555555}"}),
            );
            resources.insert(format!("{prefix}/pullrequests"), json!([]));
            resources.insert(format!("{prefix}/pullrequests/800"),json!({"id":800,"state":"OPEN","source":{"commit":{"hash":HEAD}},"destination":{"branch":{"name":branch}}}));
            resources.insert(format!("{prefix}/pullrequests/800/comments"), json!([]));
        }
        save(
            &state_path,
            &json!({"forge":forge,"resources":resources,"calls":[],"argv":[]}),
        );
        Self {
            workspace,
            work,
            origin,
            url,
            bin,
            state_path,
        }
    }
    pub fn state(&self) -> Value {
        load(&self.state_path)
    }
    pub fn save(&self, state: &Value) {
        save(&self.state_path, state);
    }
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = self.workspace.command("forge", args);
        let mut path = vec![self.bin.clone()];
        path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        command
            .current_dir(&self.work)
            .env("PATH", std::env::join_paths(path).unwrap())
            .env("FORGE_FIXTURE_STATE", &self.state_path);
        command
    }
    pub fn run(&self, args: &[&str]) -> Observed {
        observe(&mut self.command(args), None)
    }
    pub fn body(&self, text: &str) -> PathBuf {
        let path = self.workspace.root.join("body.md");
        write(&path, text);
        path
    }
    pub fn writes(&self) -> Vec<Value> {
        self.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v[0] != "GET")
            .cloned()
            .collect()
    }
}
pub fn github_pull(id: u64, head: &str, merged: bool) -> Value {
    json!({"id":id+1000,"number":id,"title":"Batch fixture","body":"Original description","user":{"id":9,"login":"author"},"head":{"sha":head,"ref":format!("feat/{id}")},"base":{"ref":"main"},"state":if merged {"closed"} else {"open"},"draft":false,"merged":merged,"merged_at":if merged {json!("2026-01-01T00:00:00Z")} else {Value::Null},"merge_commit_sha":if merged {json!(MERGE)} else {Value::Null},"changed_files":1})
}
