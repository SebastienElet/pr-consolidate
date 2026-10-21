use crate::error::{Result, require};
use crate::process::{capture, redact};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn validate(commands: &Value, secret: &str) -> Result<()> {
    let commands = commands.as_object().ok_or_else(|| {
        crate::error::Error::Refused(
            "commands must map supported project phases to command lists".into(),
        )
    })?;
    for (phase, steps) in commands {
        require(
            matches!(
                phase.as_str(),
                "prepare" | "build" | "check" | "reinstall" | "lockfile"
            ),
            "commands must map supported project phases to command lists",
        )?;
        let steps = steps.as_array().ok_or_else(|| {
            crate::error::Error::Refused(format!("{phase} must contain a command list"))
        })?;
        require(
            !steps.is_empty() || phase == "build",
            format!("{phase} must contain at least one command"),
        )?;
        for step in steps {
            require(
                step.as_object().is_some_and(|s| {
                    s.len() == 2 && s.contains_key("argv") && s.contains_key("timeout_seconds")
                }),
                format!("{phase}: expected argv and timeout_seconds"),
            )?;
            let argv = step["argv"].as_array().ok_or_else(|| {
                crate::error::Error::Refused(format!(
                    "{phase}: argv must be a non-empty list of strings"
                ))
            })?;
            require(
                !argv.is_empty()
                    && argv.iter().all(|arg| {
                        arg.as_str()
                            .is_some_and(|s| !s.is_empty() && !s.contains('\0'))
                    }),
                format!("{phase}: argv must be a non-empty list of strings"),
            )?;
            require(
                step["timeout_seconds"]
                    .as_u64()
                    .is_some_and(|t| (1..=86400).contains(&t)),
                format!("{phase}: timeout_seconds must be an integer from 1 to 86400"),
            )?;
            require(
                secret.is_empty()
                    || argv
                        .iter()
                        .all(|arg| !arg.as_str().unwrap().contains(secret)),
                format!("{phase}: registry credentials belong only in the environment"),
            )?;
        }
    }
    Ok(())
}

pub fn run_phase(plan: &Value, phase: &str, directory: &Path, log: &Path) -> Result<i32> {
    require(plan.is_object(), "project plan must be a JSON object")?;
    validate(
        &plan["commands"],
        &std::env::var("NPM_TOKEN").unwrap_or_default(),
    )?;
    let steps = plan["commands"][phase].as_array().ok_or_else(|| {
        crate::error::Error::Refused(format!(
            "project has no {phase} command; discover its documented native command first"
        ))
    })?;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut output = std::fs::File::create(log)?;
    if steps.is_empty() {
        writeln!(
            output,
            "{phase}: no project build step (explicitly declared)"
        )?;
    }
    for (index, step) in steps.iter().enumerate() {
        let argv: Vec<_> = step["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let mut command = Command::new(argv[0]);
        command.args(&argv[1..]).current_dir(directory);
        let result = match capture(
            &mut command,
            None,
            Duration::from_secs(step["timeout_seconds"].as_u64().unwrap()),
            true,
        ) {
            Ok(result) => result,
            Err(error) => {
                writeln!(output, "{}", redact(&error.to_string()))?;
                return Ok(2);
            }
        };
        write!(
            output,
            "{}\n{phase} step={} exit={}\n",
            redact(&result.stdout),
            index + 1,
            result.code
        )?;
        if result.code != 0 {
            return Ok(result.code);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_credentials_are_rejected_in_command_arguments() {
        let commands =
            json!({"build": [{"argv": ["echo", "private-fixture-secret"], "timeout_seconds": 3}]});
        assert!(validate(&commands, "private-fixture-secret").is_err());
    }
}
