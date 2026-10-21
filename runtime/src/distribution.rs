use crate::entrypoints;
use crate::error::{Error, Result};
use crate::process::{capture, mask};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct BuildHost {
    pub platform: &'static str,
    pub target: &'static str,
}

pub fn host(os: &str, arch: &str) -> Result<BuildHost> {
    let (platform, target) = match (os, arch) {
        ("macos", "aarch64") => ("darwin-arm64", "aarch64-apple-darwin"),
        ("macos", "x86_64") => ("darwin-x64", "x86_64-apple-darwin"),
        ("linux", "aarch64") => ("linux-arm64", "aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => ("linux-x64", "x86_64-unknown-linux-gnu"),
        _ => return Err(Error::Refused("unsupported platform".into())),
    };
    Ok(BuildHost { platform, target })
}

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

fn temporary(destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Refused("invalid executable destination".into()))?;
    fs::create_dir_all(parent)?;
    Ok(parent.join(format!(
        ".pr-consolidate-{}-{}",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )))
}

fn prepare_executable(source: &Path, destination: &Path) -> Result<PendingFile> {
    let path = temporary(destination)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let pending = PendingFile(path);
    fs::copy(source, &pending.0)?;
    fs::set_permissions(&pending.0, fs::Permissions::from_mode(0o755))?;
    Ok(pending)
}

pub fn install(root: &Path, runtime: &Path, launcher: &Path, platform: &str) -> Result<()> {
    let bin = root.join("skills/pr-consolidate/bin").join(platform);
    let runtime_destination = bin.join("pr-consolidate");
    let launcher_destination = bin.join("pr-consolidate-launcher");
    let runtime = prepare_executable(runtime, &runtime_destination)?;
    let launcher = prepare_executable(launcher, &launcher_destination)?;
    let mut aliases = Vec::new();
    for name in entrypoints::NAMES {
        let destination = root.join("skills/pr-consolidate/scripts").join(name);
        let path = temporary(&destination)?;
        fs::hard_link(&launcher.0, &path)?;
        let pending = PendingFile(path);
        aliases.push((pending, destination));
    }
    fs::rename(&runtime.0, runtime_destination)?;
    fs::rename(&launcher.0, launcher_destination)?;
    for (pending, destination) in aliases {
        fs::rename(&pending.0, destination)?;
    }
    Ok(())
}

pub fn build(root: &Path) -> Result<i32> {
    let host = host(std::env::consts::OS, std::env::consts::ARCH)?;
    let manifest = root.join("runtime/Cargo.toml");
    if !manifest.is_file() {
        return Err(Error::Refused("run the build from the package root".into()));
    }
    let target = root.join("runtime/target");
    let mut command = Command::new("cargo");
    command
        .args(["build", "--locked", "--release", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .args(["--target", host.target])
        .args([
            "--bin",
            "pr-consolidate",
            "--bin",
            "pr-consolidate-launcher",
        ])
        .env_remove("CARGO_BUILD_TARGET");
    let output = capture(&mut command, None, Duration::from_secs(900), true)?;
    print!("{}", mask(&output.stdout));
    if output.code != 0 {
        return Ok(output.code);
    }
    install(
        root,
        &target.join(host.target).join("release/pr-consolidate"),
        &target
            .join(host.target)
            .join("release/pr-consolidate-launcher"),
        host.platform,
    )?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_build_hosts_refuse_before_writing_artifacts() {
        assert!(host("windows", "x86_64").is_err());
        assert!(host("linux", "unknown").is_err());
    }
}
