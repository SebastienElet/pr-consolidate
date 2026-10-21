use pr_consolidate::{distribution::host, entrypoints};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

fn launch() -> Result<(), String> {
    let mut args = std::env::args_os();
    let program = args.next().ok_or("missing executable name")?;
    let name = Path::new(&program)
        .file_name()
        .ok_or("invalid executable name")?;
    let prefix = entrypoints::arguments(name).ok_or("unknown native entrypoint")?;
    let location = std::env::current_exe().map_err(|_| "cannot locate the native entrypoint")?;
    let directory = location
        .parent()
        .ok_or("cannot locate the installed skill")?;
    let host =
        host(std::env::consts::OS, std::env::consts::ARCH).map_err(|_| "unsupported platform")?;
    let binary = directory
        .join("../bin")
        .join(host.platform)
        .join("pr-consolidate");
    let _error = Command::new(binary).args(prefix).args(args).exec();
    Err("native runtime is missing or cannot execute; run bun run build in the installed package root".into())
}

fn main() {
    if let Err(error) = launch() {
        eprintln!("REFUSED: {error}");
        std::process::exit(2);
    }
}
