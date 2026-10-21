use crate::error::Result;
use serde_json::Value;

pub trait Api {
    fn api(&self, path: &str, method: &str, payload: Option<&Value>, timeout: u64)
    -> Result<Value>;
}

use crate::context::{Context, ForgeKind};
use crate::error::Error;
use crate::github::GitHub;
use crate::process::capture;
use std::process::Command;
use std::time::Duration;

pub struct NativeTransport {
    pub context: Context,
}
pub struct NativeRequest {
    pub repository: String,
    pub forge: ForgeKind,
}

pub fn bitbucket_endpoint(repository: &str, path: &str) -> Result<String> {
    let root = format!("/repositories/{repository}/");
    let path = if let Some(rest) = path.strip_prefix("https://api.bitbucket.org/2.0") {
        if rest.contains('#') || !rest.starts_with(&root) {
            return Err(Error::Failed(
                "refusing pagination outside the configured repository".into(),
            ));
        }
        rest.to_owned()
    } else if path == "/user" {
        return Ok(path.into());
    } else if path.starts_with(&root) {
        path.into()
    } else {
        if path.starts_with('/') || path.contains("://") {
            return Err(Error::Failed(
                "refusing an endpoint outside the configured repository".into(),
            ));
        }
        format!("{root}{path}")
    };
    let pathname = path.split('?').next().unwrap();
    let decoded = decode_percent(pathname)?;
    if !path.starts_with(&root) || invalid_path(&decoded) || path.contains('#') {
        return Err(Error::Failed("invalid repository endpoint".into()));
    }
    Ok(path)
}

fn decode_percent(value: &str) -> Result<String> {
    let mut result = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let digit = |b: Option<u8>| b.and_then(|b| (b as char).to_digit(16));
            let a = digit(bytes.next())
                .ok_or_else(|| Error::Failed("invalid repository endpoint".into()))?;
            let b = digit(bytes.next())
                .ok_or_else(|| Error::Failed("invalid repository endpoint".into()))?;
            result.push((a * 16 + b) as u8);
        } else {
            result.push(byte);
        }
    }
    String::from_utf8(result).map_err(|_| Error::Failed("invalid repository endpoint".into()))
}

pub fn invalid_path(path: &str) -> bool {
    path.bytes()
        .any(|b| b <= 32 || b == 127 || b == b'\\' || b == b'%')
        || path.split('/').any(|s| s == "..")
}

impl Api for NativeTransport {
    fn api(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        let request = NativeRequest {
            repository: self.context.repository.clone(),
            forge: self.context.forge.clone(),
        };
        match self.context.forge {
            ForgeKind::GitHub => GitHub {
                repository: self.context.repository.clone(),
                request,
            }
            .api(path, method, payload, timeout),
            ForgeKind::Bitbucket => request.api(path, method, payload, timeout),
        }
    }
}

impl Api for NativeRequest {
    fn api(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        let (cli, target) = match self.forge {
            ForgeKind::GitHub => {
                let root = format!("/repos/{}/", self.repository);
                if (path != "/user" && !path.starts_with(&root))
                    || path.contains('#')
                    || invalid_path(path.split('?').next().unwrap())
                {
                    return Err(Error::Failed(
                        "refusing an endpoint outside the configured GitHub repository".into(),
                    ));
                }
                ("gh", path.to_owned())
            }
            ForgeKind::Bitbucket => ("bkt", bitbucket_endpoint(&self.repository, path)?),
        };
        let body = payload.map(Value::to_string);
        let mut command = Command::new(cli);
        command.args(["api", &target]);
        if cli == "gh" {
            command.args(["--hostname", "github.com"]);
        }
        command.args(["--method", method]);
        if cli == "bkt" {
            command.arg("--json");
        }
        if let Some(body) = &body {
            command.args(["--input", if cli == "gh" { "-" } else { body }]);
        }
        execute(
            &mut command,
            cli,
            method,
            if cli == "gh" { body.as_deref() } else { None },
            timeout,
        )
    }
}

fn execute(
    command: &mut Command,
    cli: &str,
    method: &str,
    body: Option<&str>,
    timeout: u64,
) -> Result<Value> {
    let result = capture(command, body, Duration::from_secs(timeout), false)
        .map_err(|_| Error::Failed(format!("{cli} could not start; check its installation")))?;
    if result.timed_out {
        return Err(Error::Timeout(format!(
            "{cli} {method} timed out after {timeout}s; inspect remote state before retry"
        )));
    }
    if result.code != 0 {
        return Err(Error::Failed(format!(
            "{cli} {method} failed with exit {}; remote state is unconfirmed",
            result.code
        )));
    }
    let value: Value = serde_json::from_str(&result.stdout).map_err(|_| {
        Error::Failed(format!(
            "{cli} {method} returned invalid JSON; remote state is unconfirmed"
        ))
    })?;
    if cli == "bkt" && !value.is_object() {
        return Err(Error::Failed("bkt returned a non-object response".into()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitbucket_endpoints_stay_within_repository_after_decoding() {
        assert_eq!(
            bitbucket_endpoint("owner/project", "pullrequests/1").unwrap(),
            "/repositories/owner/project/pullrequests/1"
        );
        assert_eq!(
            bitbucket_endpoint(
                "owner/project",
                "https://api.bitbucket.org/2.0/repositories/owner/project/pullrequests?page=2"
            )
            .unwrap(),
            "/repositories/owner/project/pullrequests?page=2"
        );
        assert_eq!(
            bitbucket_endpoint("owner/project", "/user").unwrap(),
            "/user"
        );
        for path in [
            "https://api.bitbucket.org/2.0/repositories/other/project/pullrequests",
            "https://evil.example/repositories/owner/project/pullrequests",
            "http://api.bitbucket.org/2.0/repositories/owner/project/pullrequests",
            "https://token@api.bitbucket.org/2.0/repositories/owner/project/pullrequests",
            "../other",
            "/repositories/other/project/pullrequests",
            "pullrequests/%2e%2e/%2e%2e",
            "pullrequests/%252e%252e",
            "pullrequests/%00",
            "pullrequests/\\bad",
            "pullrequests/%zz",
            "pullrequests#fragment",
        ] {
            assert!(bitbucket_endpoint("owner/project", path).is_err(), "{path}");
        }
    }
    #[test]
    fn cli_errors_never_expose_captured_output_and_timeout_is_distinct() {
        for (script, code) in [
            ("printf private-output; exit 23", 1),
            ("printf private-output", 1),
            ("printf private-output; sleep 8", 3),
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            let result = execute(&mut command, "gh", "POST", None, 1).unwrap_err();
            assert_eq!(result.code(), code);
            assert!(!result.to_string().contains("private-output"));
        }
        let mut missing = Command::new("missing-forge-cli-fixture");
        let result = execute(&mut missing, "bkt", "GET", None, 1).unwrap_err();
        assert_eq!(result.code(), 1);
    }
    #[test]
    fn github_native_request_refuses_foreign_and_traversing_endpoints_before_start() {
        let request = NativeRequest {
            repository: "owner/project".into(),
            forge: ForgeKind::GitHub,
        };
        for path in [
            "https://api.github.com/repos/owner/project/pulls",
            "//github.com/owner/project",
            "/repos/other/project/pulls",
            "/repos/owner/project/../other",
            "/repos/owner/project/%2e%2e",
            "/repos/owner/project/\\bad",
            "/repos/owner/project/pulls#fragment",
        ] {
            assert!(
                request
                    .api(path, "GET", None, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("outside"),
                "{path}"
            );
        }
    }
}
