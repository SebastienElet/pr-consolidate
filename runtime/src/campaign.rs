use crate::context::{is_sha, primary};
use crate::error::{Error, Result, require};
use crate::plan::{agent, resolve, scalar, tag, text};
use crate::process::{Output, capture, redact, sanitize_git};
use crate::project::run_phase;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub struct Campaign {
    plan: Value,
    root: PathBuf,
    repo: PathBuf,
    logs: PathBuf,
}

impl Campaign {
    pub fn new(plan: Value) -> Result<Self> {
        let root = PathBuf::from(text(&plan, "root")?);
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs)?;
        Ok(Self {
            repo: root.join("repo"),
            plan,
            root,
            logs,
        })
    }

    fn s(&self, key: &str) -> &str {
        self.plan[key].as_str().unwrap()
    }
    fn prs(&self) -> &[Value] {
        self.plan["prs"].as_array().unwrap()
    }
    fn git(&self, where_: &Path, args: &[&str], check: bool) -> Result<Output> {
        let mut command = Command::new("git");
        command.args(args).current_dir(where_);
        sanitize_git(&mut command);
        for key in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
            command.env(key, self.s("author_name"));
        }
        for key in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
            command.env(key, self.s("author_email"));
        }
        let output = capture(&mut command, None, Duration::from_secs(60), false)?;
        require(!output.timed_out, "Git operation timed out")?;
        if check && output.code != 0 {
            let message = if output.stderr.trim().is_empty() {
                output.stdout.trim()
            } else {
                output.stderr.trim()
            };
            return Err(Error::Refused(redact(if message.is_empty() {
                "git command failed"
            } else {
                message
            })));
        }
        Ok(output)
    }
    fn value(&self, where_: &Path, args: &[&str]) -> Result<String> {
        Ok(self.git(where_, args, true)?.stdout.trim().to_owned())
    }
    fn clean(&self) -> Result<()> {
        let status = self.value(
            &self.repo,
            &["status", "--porcelain", "--untracked-files=normal"],
        )?;
        require(status.is_empty(), format!("dirty tree:\n{status}"))
    }
    fn no_merge(&self) -> Result<()> {
        require(
            self.git(
                &self.repo,
                &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
                false,
            )?
            .code
                != 0,
            "a merge is in progress",
        )
    }
    fn owned(&self, where_: &Path) -> Result<()> {
        require(
            !where_.is_symlink(),
            "campaign clone may not be a symbolic link",
        )?;
        let common = self.value(where_, &["rev-parse", "--git-common-dir"])?;
        require(
            resolve(&where_.join(common))?.starts_with(resolve(&self.root)?),
            "campaign clone refers to a Git directory outside root",
        )
    }
    fn origin(&self, where_: &Path) -> Result<()> {
        self.owned(where_)?;
        let urls = self.value(where_, &["remote", "get-url", "--all", "origin"])?;
        let pushes = self.value(where_, &["remote", "get-url", "--push", "--all", "origin"])?;
        require(
            urls.lines().collect::<Vec<_>>() == [self.s("remote_url")]
                && pushes.lines().collect::<Vec<_>>() == [self.s("remote_url")],
            "origin fetch/push URL differs from the campaign remote_url",
        )?;
        let actual = primary(&self.value(where_, &["ls-remote", "--symref", "origin", "HEAD"])?)?;
        self.git(where_, &["check-ref-format", "--branch", &actual], true)?;
        require(
            self.s("branch") != actual,
            "lot branch is the actual remote primary branch",
        )?;
        require(
            self.s("base_branch") == actual,
            "plan base_branch differs from the actual remote primary branch",
        )
    }
    fn branch(&self) -> Result<()> {
        self.origin(&self.repo)?;
        require(
            self.value(&self.repo, &["symbolic-ref", "-q", "HEAD"])?
                == format!("refs/heads/{}", self.s("branch")),
            "HEAD is not on the lot branch",
        )
    }
    fn heads(&self, where_: &Path, base_ref: &str, prefix: &str, by_branch: bool) -> Result<()> {
        require(
            self.value(
                where_,
                &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")],
            )? == self.s("base_sha"),
            "main branch moved from the reviewed base SHA",
        )?;
        for pr in self.prs() {
            let suffix = if by_branch {
                pr["branch"].as_str().unwrap().to_owned()
            } else {
                scalar(&pr["id"])
            };
            require(
                self.value(
                    where_,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("{prefix}{suffix}^{{commit}}"),
                    ],
                )? == pr["head"].as_str().unwrap(),
                format!("PR {} moved from its reviewed head", scalar(&pr["id"])),
            )?;
        }
        Ok(())
    }
    fn ancestor(&self, where_: &Path, base: &str, head: &str) -> Result<()> {
        require(
            self.git(where_, &["merge-base", "--is-ancestor", base, head], false)?
                .code
                == 0,
            format!("{head} does not contain {base}"),
        )
    }
    fn snapshot(&self) -> Result<()> {
        let where_ = Path::new(text(&self.plan, "push_clone")?);
        self.origin(where_)?;
        let mut specs = vec![format!(
            "+refs/heads/{0}:refs/remotes/origin/{0}",
            self.s("base_branch")
        )];
        specs.extend(self.prs().iter().map(|pr| {
            format!(
                "+refs/heads/{0}:refs/remotes/origin/{0}",
                pr["branch"].as_str().unwrap()
            )
        }));
        let mut args = vec!["fetch", "--no-prune", "--no-tags", "origin"];
        args.extend(specs.iter().map(String::as_str));
        self.git(where_, &args, true)?;
        self.heads(
            where_,
            &format!("refs/remotes/origin/{}", self.s("base_branch")),
            "refs/remotes/origin/",
            true,
        )?;
        self.git(
            where_,
            &["update-ref", "refs/lots/base", self.s("base_sha")],
            true,
        )?;
        let mut refs = vec!["refs/lots/base".to_owned()];
        for pr in self.prs() {
            let reference = format!("refs/lots/pr/{}", scalar(&pr["id"]));
            self.git(
                where_,
                &["update-ref", &reference, pr["head"].as_str().unwrap()],
                true,
            )?;
            refs.push(reference);
        }
        let artifacts = self.plan["artifacts"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.logs.clone());
        std::fs::create_dir_all(&artifacts)?;
        let bundle = artifacts.join(format!("{}.bundle", self.s("lot")));
        let bundle_str = bundle
            .to_str()
            .ok_or_else(|| Error::Refused("invalid bundle path".into()))?;
        let mut args = vec!["bundle", "create", bundle_str];
        args.extend(refs.iter().map(String::as_str));
        self.git(where_, &args, true)?;
        self.git(where_, &["bundle", "verify", bundle_str], true)?;
        println!("{}", bundle.display());
        Ok(())
    }
    fn setup(&self, bundle: &str) -> Result<()> {
        let source = Path::new(self.s("source"));
        self.origin(source)?;
        require(
            !self.repo.is_symlink(),
            "campaign repo may not be a symbolic link",
        )?;
        let bundle = resolve(Path::new(bundle))?;
        let bundle = bundle
            .to_str()
            .ok_or_else(|| Error::Refused("invalid bundle path".into()))?;
        self.git(source, &["bundle", "verify", bundle], true)?;
        self.git(
            source,
            &[
                "fetch",
                "--no-prune",
                "--no-tags",
                bundle,
                &format!("+refs/lots/*:refs/lots/{}/*", self.s("lot")),
            ],
            true,
        )?;
        let prefix = format!("refs/lots/{}", self.s("lot"));
        self.heads(
            source,
            &format!("{prefix}/base"),
            &format!("{prefix}/pr/"),
            false,
        )?;
        if self.repo.exists() {
            require(
                resolve(Path::new(
                    &self.value(&self.repo, &["rev-parse", "--show-toplevel"])?,
                ))? == resolve(&self.repo)?,
                "campaign repo is not a repository root",
            )?;
            self.clean()?;
            self.no_merge()?;
            self.origin(&self.repo)?;
        } else {
            self.git(
                &self.root,
                &[
                    "clone",
                    "--no-checkout",
                    "--no-hardlinks",
                    self.s("source"),
                    self.repo.to_str().unwrap(),
                ],
                true,
            )?;
            self.git(
                &self.repo,
                &["remote", "set-url", "origin", self.s("remote_url")],
                true,
            )?;
        }
        let mut specs = vec![format!(
            "+{prefix}/base:refs/remotes/origin/{}",
            self.s("base_branch")
        )];
        specs.extend(
            self.prs()
                .iter()
                .map(|p| format!("+{prefix}/pr/{0}:refs/remotes/pr/{0}", scalar(&p["id"]))),
        );
        let mut args = vec!["fetch", "--no-prune", "--no-tags", self.s("source")];
        args.extend(specs.iter().map(String::as_str));
        self.git(&self.repo, &args, true)?;
        self.heads(
            &self.repo,
            &format!("refs/remotes/origin/{}", self.s("base_branch")),
            "refs/remotes/pr/",
            false,
        )?;
        self.git(
            &self.repo,
            &["config", "user.name", self.s("author_name")],
            true,
        )?;
        self.git(
            &self.repo,
            &["config", "user.email", self.s("author_email")],
            true,
        )?;
        if let Some(path) = self.plan["hooks_path"].as_str() {
            self.git(&self.repo, &["config", "core.hooksPath", path], true)?;
        }
        let branch = self.s("branch");
        if self
            .git(
                &self.repo,
                &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                false,
            )?
            .code
            == 0
        {
            self.ancestor(
                &self.repo,
                self.s("base_sha"),
                &format!("refs/heads/{branch}"),
            )?;
            self.git(&self.repo, &["checkout", branch], true)?;
        } else {
            self.git(
                &self.repo,
                &["checkout", "-b", branch, self.s("base_sha")],
                true,
            )?;
        }
        println!("{}", self.value(&self.repo, &["rev-parse", "HEAD"])?);
        Ok(())
    }
    fn merge(&self, id: &str) -> Result<i32> {
        let pr = self
            .prs()
            .iter()
            .find(|p| scalar(&p["id"]) == id)
            .ok_or_else(|| Error::Refused("PR is not in this lot".into()))?;
        self.branch()?;
        self.no_merge()?;
        self.clean()?;
        let head = self.value(
            &self.repo,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/pr/{id}^{{commit}}"),
            ],
        )?;
        require(
            head == pr["head"].as_str().unwrap(),
            "PR ref moved from reviewed head",
        )?;
        let result = self.git(
            &self.repo,
            &["merge", "--no-ff", "--no-commit", &head],
            false,
        )?;
        print!("{}", redact(&(result.stdout + &result.stderr)));
        Ok(result.code)
    }
    fn staged(&self) -> Result<()> {
        for (args, message) in [
            (
                vec!["diff", "--name-only", "--diff-filter=U"],
                "unmerged paths",
            ),
            (vec!["diff", "--name-only"], "unstaged changes"),
            (
                vec!["ls-files", "--others", "--exclude-standard"],
                "untracked files",
            ),
        ] {
            require(self.value(&self.repo, &args)?.is_empty(), message)?;
        }
        let raw = self
            .git(
                &self.repo,
                &[
                    "diff",
                    "--cached",
                    "--name-only",
                    "-z",
                    "--diff-filter=ACMR",
                ],
                true,
            )?
            .stdout;
        let paths: Vec<_> = raw.split_terminator('\0').collect();
        if !paths.is_empty() {
            let mut args = vec![
                "--literal-pathspecs",
                "grep",
                "--cached",
                "-I",
                "-l",
                "-E",
                "^(<<<<<<<|>>>>>>>)( |$)",
                "--",
            ];
            args.extend(paths);
            let result = self.git(&self.repo, &args, false)?;
            require(result.code != 0, "conflict markers staged")?;
            require(result.code == 1, "conflict marker inspection failed")?;
        }
        Ok(())
    }
    fn commit(&self, message: &str, tag_: &str, paths: &[String]) -> Result<i32> {
        tag(tag_)?;
        self.branch()?;
        if !paths.is_empty() {
            let mut args = vec!["add", "--"];
            args.extend(paths.iter().map(String::as_str));
            self.git(&self.repo, &args, true)?;
        }
        require(
            self.git(&self.repo, &["diff", "--cached", "--quiet"], false)?
                .code
                != 0
                || self
                    .git(
                        &self.repo,
                        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
                        false,
                    )?
                    .code
                    == 0,
            "nothing staged",
        )?;
        self.staged()?;
        for ident in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            require(
                !agent(&self.value(&self.repo, &["var", ident])?),
                "commit identity names an agent",
            )?;
        }
        let log = self.logs.join(format!("build-{tag_}.log"));
        let status = run_phase(&self.plan, "build", &self.repo, &log)?;
        println!("build exit={status} log={}", log.display());
        if status != 0 {
            return Ok(status);
        }
        self.staged()?;
        let message = resolve(Path::new(message))?;
        let result = self.git(
            &self.repo,
            &["commit", "-F", message.to_str().unwrap()],
            false,
        )?;
        print!("{}", redact(&(result.stdout + &result.stderr)));
        if result.code == 0 {
            require(
                !agent(&self.value(&self.repo, &["log", "-1", "--format=%an <%ae>%n%cn <%ce>"])?),
                "created commit names an agent; do not push",
            )?;
        }
        Ok(result.code)
    }
    fn push(&self, candidate: &str) -> Result<()> {
        require(
            is_sha(candidate, 40, 40),
            "push requires push_clone and a complete candidate SHA",
        )?;
        let where_ = Path::new(text(&self.plan, "push_clone")?);
        self.origin(where_)?;
        let reference = format!("refs/lots/candidate/{}", self.s("lot"));
        self.git(
            where_,
            &[
                "fetch",
                "--no-prune",
                "--no-tags",
                self.repo.to_str().unwrap(),
                &format!("+refs/heads/{}:{reference}", self.s("branch")),
            ],
            true,
        )?;
        require(
            self.value(where_, &["rev-parse", &reference])? == candidate,
            "candidate SHA differs from the lot branch",
        )?;
        self.ancestor(where_, self.s("base_sha"), candidate)?;
        let identities = self.value(
            where_,
            &[
                "log",
                "--format=%an <%ae>%n%cn <%ce>",
                &format!("{}..{candidate}", self.s("base_sha")),
            ],
        )?;
        require(
            !identities.is_empty() && !agent(&identities),
            "candidate has no new commits or names an agent",
        )?;
        let branch = self.s("branch");
        let remote = self.value(
            where_,
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
        )?;
        if let Some(previous) = remote.split_whitespace().next() {
            self.git(
                where_,
                &[
                    "fetch",
                    "--no-prune",
                    "--no-tags",
                    "origin",
                    &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
                ],
                true,
            )?;
            self.ancestor(where_, previous, candidate)?;
        }
        self.origin(where_)?;
        self.git(
            where_,
            &[
                "push",
                "origin",
                &format!("{candidate}:refs/heads/{branch}"),
            ],
            true,
        )?;
        let after = self.value(
            where_,
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
        )?;
        require(
            after.split_whitespace().next() == Some(candidate),
            "remote branch does not match pushed candidate",
        )?;
        println!("{candidate}");
        Ok(())
    }
    fn phase(&self, name: &str, tag_: &str) -> Result<i32> {
        tag(tag_)?;
        self.branch()?;
        self.clean()?;
        let before = self.value(&self.repo, &["rev-parse", "HEAD"])?;
        let log = self.logs.join(format!("{name}-{tag_}.log"));
        let status = run_phase(&self.plan, name, &self.repo, &log)?;
        println!("{name} head={before} exit={status} log={}", log.display());
        if status != 0 {
            return Ok(status);
        }
        self.clean()?;
        require(
            self.value(&self.repo, &["rev-parse", "HEAD"])? == before,
            "project command changed HEAD",
        )?;
        Ok(0)
    }
    fn lockfile(&self, base: &str, tag_: &str) -> Result<i32> {
        let files = self.lockfiles();
        require(!files.is_empty(), "project has no declared lockfiles")?;
        tag(tag_)?;
        self.branch()?;
        self.value(
            &self.repo,
            &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
        )?;
        let mut args = vec!["checkout", base, "--"];
        args.extend(files);
        self.git(&self.repo, &args, true)?;
        run_phase(
            &self.plan,
            "lockfile",
            &self.repo,
            &self.logs.join(format!("lockfile-{tag_}.log")),
        )
    }
    fn lockfiles(&self) -> Vec<&str> {
        self.plan["lockfiles"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }
    fn merge_base(&self, base: &str, message: &str) -> Result<i32> {
        require(
            is_sha(base, 40, 40),
            "base SHA must have 40 hexadecimal digits",
        )?;
        self.branch()?;
        self.no_merge()?;
        self.clean()?;
        require(
            base == self.value(
                &self.repo,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/remotes/origin/{}^{{commit}}", self.s("base_branch")),
                ],
            )?,
            "base SHA differs from the imported main branch; refresh snapshot first",
        )?;
        self.ancestor(&self.repo, self.s("base_sha"), base)?;
        let result = self.git(
            &self.repo,
            &["merge", "--no-ff", "--no-commit", base],
            false,
        )?;
        print!("{}", redact(&(result.stdout + &result.stderr)));
        require(
            self.git(
                &self.repo,
                &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
                false,
            )?
            .code
                == 0,
            "no merge started",
        )?;
        let raw = self
            .git(
                &self.repo,
                &["diff", "--name-only", "-z", "--diff-filter=U"],
                true,
            )?
            .stdout;
        let unmerged: Vec<_> = raw.split_terminator('\0').collect();
        let files = self.lockfiles();
        require(
            unmerged.iter().all(|p| files.contains(p)),
            "conflicts outside declared lockfiles; merge left in progress",
        )?;
        if !unmerged.is_empty() {
            let status = self.lockfile(base, &base[..8])?;
            if status != 0 {
                return Ok(status);
            }
            let mut args = vec!["add", "--"];
            args.extend(files);
            self.git(&self.repo, &args, true)?;
        }
        self.commit(message, &format!("merge-base-{}", &base[..8]), &[])
    }
    pub fn execute(&self, command: &str, args: &[String]) -> Result<i32> {
        match (command, args) {
            ("bundle", []) => self.snapshot()?,
            ("setup", [bundle]) => self.setup(bundle)?,
            ("merge", [id]) => return self.merge(id),
            ("push", [candidate]) => self.push(candidate)?,
            ("commit", [message, tag_, paths @ ..]) => return self.commit(message, tag_, paths),
            ("build", [tag_]) => return self.phase("build", tag_),
            ("preflight" | "resync" | "prepare", []) => {
                let phase = match command {
                    "preflight" => "check",
                    "resync" => "reinstall",
                    _ => "prepare",
                };
                return self.phase(
                    phase,
                    &self.value(&self.repo, &["rev-parse", "--short", "HEAD"])?,
                );
            }
            ("lockfile", [base, tag_]) => return self.lockfile(base, tag_),
            ("merge-base", [base, message]) => return self.merge_base(base, message),
            _ => {
                return Err(Error::Refused(format!(
                    "invalid arguments for lot-{command}"
                )));
            }
        }
        Ok(0)
    }
}
