use crate::error::{Error, Result, require};
use crate::plan::resolve;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn digest(mut input: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn files(root: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(&entry.path(), result)?;
        } else if entry.path().is_file() {
            result.push(entry.path());
        }
    }
    Ok(())
}

pub fn execute(command: &str, path: &str) -> Result<i32> {
    let root = Path::new(path);
    match command {
        "path" => println!("{}", resolve(root)?.display()),
        "digest" => println!(
            "{}",
            if path == "-" {
                digest(std::io::stdin().lock())?
            } else {
                digest(std::fs::File::open(root)?)?
            }
        ),
        "manifest" => {
            let manifest = root.join("SHA256SUMS");
            let mut paths = Vec::new();
            files(root, &mut paths)?;
            paths.sort();
            let mut document = String::new();
            for file in paths {
                if file == manifest {
                    continue;
                }
                let relative = file
                    .strip_prefix(root)
                    .map_err(|_| Error::Refused("invalid archive path".into()))?;
                document.push_str(&format!(
                    "{}  ./{}\n",
                    digest(std::fs::File::open(&file)?)?,
                    relative.display()
                ));
            }
            std::fs::write(manifest, document)?;
        }
        "verify" => {
            let resolved = root.canonicalize()?;
            for line in std::fs::read_to_string(root.join("SHA256SUMS"))?.lines() {
                let (expected, relative) = line
                    .split_once("  ")
                    .ok_or_else(|| Error::Refused("invalid archive manifest".into()))?;
                let target = root.join(relative).canonicalize()?;
                require(
                    target.starts_with(&resolved)
                        && digest(std::fs::File::open(target)?)? == expected,
                    format!("archive digest mismatch: {relative}"),
                )?;
            }
        }
        "filter" => {
            let token = std::env::var("NPM_TOKEN").unwrap_or_default();
            for line in String::from_utf8_lossy(&std::fs::read(root)?).lines() {
                if !line.to_lowercase().contains("authtoken")
                    && (token.is_empty() || !line.contains(&token))
                {
                    println!("{line}");
                }
            }
        }
        _ => {
            return Err(Error::Refused(
                "usage: pr-consolidate native path|digest|manifest|verify|filter <path>".into(),
            ));
        }
    }
    Ok(0)
}
