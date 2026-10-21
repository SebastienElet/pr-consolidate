use crate::error::{Error, Result, require};
use crate::process::{capture, sanitize_git};
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeKind {
    GitHub,
    Bitbucket,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub root: PathBuf,
    pub repository: String,
    pub forge: ForgeKind,
    pub remote_url: String,
    pub base_branch: String,
}

pub fn origin_identity(value: &str) -> Result<(String, ForgeKind)> {
    let pattern = Regex::new(r"\A(?:git@(github\.com|bitbucket\.org):|https://((?i:github\.com|bitbucket\.org))(?::443)?/|ssh://git@((?i:github\.com|bitbucket\.org))(?::22)?/|ssh://git@(ssh\.bitbucket\.org):443/)([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)\z").unwrap();
    let found = pattern.captures(value).ok_or_else(|| Error::Refused("origin must identify a supported GitHub or Bitbucket Cloud repository without credentials, query or fragment".into()))?;
    let host = (1..=4)
        .find_map(|i| found.get(i))
        .unwrap()
        .as_str()
        .to_lowercase();
    let path = found[5].strip_suffix(".git").unwrap_or(&found[5]);
    require(
        path.split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".."),
        "origin must identify exactly one workspace and repository",
    )?;
    Ok((
        path.to_lowercase(),
        if host == "github.com" {
            ForgeKind::GitHub
        } else {
            ForgeKind::Bitbucket
        },
    ))
}

pub fn git(directory: &Path, args: &[&str], acceptable: &[i32]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory).args(args);
    sanitize_git(&mut command);
    let output = capture(&mut command, None, Duration::from_secs(60), false)
        .map_err(|_| Error::Refused(format!("Git discovery could not run: {}", args[0])))?;
    require(
        !output.timed_out && acceptable.contains(&output.code),
        format!("Git discovery failed: {} (exit {})", args[0], output.code),
    )?;
    Ok(output.stdout)
}

pub fn primary(output: &str) -> Result<String> {
    let branches: Vec<_> = output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("ref: refs/heads/")
                .and_then(|s| s.strip_suffix("\tHEAD"))
        })
        .collect();
    let hashes = output
        .lines()
        .filter(|line| {
            line.strip_suffix("\tHEAD")
                .is_some_and(|hash| is_sha(hash, 40, 40))
        })
        .count();
    require(
        branches.len() == 1 && hashes == 1,
        "origin HEAD does not declare one resolvable primary branch",
    )?;
    Ok(branches[0].to_owned())
}

pub fn is_sha(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn remote_context(directory: &Path) -> Result<Context> {
    let root = PathBuf::from(git(directory, &["rev-parse", "--show-toplevel"], &[0])?.trim())
        .canonicalize()?;
    let raw = git(
        &root,
        &["config", "--null", "--get-all", "remote.origin.url"],
        &[0, 1],
    )?;
    let urls: Vec<_> = raw.split_terminator('\0').collect();
    require(urls.len() == 1, "origin must have exactly one URL")?;
    let (repository, forge) = origin_identity(urls[0])?;
    let pushes = git(
        &root,
        &["config", "--null", "--get-all", "remote.origin.pushurl"],
        &[0, 1],
    )?;
    let pushes: Vec<_> = pushes.split_terminator('\0').collect();
    require(
        pushes.len() <= 1,
        "origin push URL does not identify the same repository",
    )?;
    for push in pushes {
        require(
            origin_identity(push)? == (repository.clone(), forge.clone()),
            "origin push URL does not identify the same repository",
        )?;
    }
    Ok(Context {
        root,
        repository,
        forge,
        remote_url: urls[0].to_owned(),
        base_branch: String::new(),
    })
}

pub fn discover(directory: &Path) -> Result<Context> {
    let mut context = remote_context(directory)?;
    context.base_branch = primary(&git(
        &context.root,
        &["ls-remote", "--symref", "origin", "HEAD"],
        &[0],
    )?)?;
    git(
        &context.root,
        &["check-ref-format", "--branch", &context.base_branch],
        &[0],
    )?;
    Ok(context)
}

impl Context {
    pub fn document(&self) -> Value {
        json!({"root": self.root, "repository": self.repository, "forge": if self.forge == ForgeKind::GitHub { "github" } else { "bitbucket" }, "remote_url": self.remote_url, "base_branch": self.base_branch})
    }
}
