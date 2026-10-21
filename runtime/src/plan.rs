use crate::context::{git, is_sha};
use crate::error::{Error, Result, require};
use crate::project;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| Error::Refused(format!("missing or invalid {key}")))
}

pub fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_str(&std::fs::read_to_string(path)?)
        .map_err(|_| Error::Refused(format!("{}: invalid JSON", path.display())))
}

pub fn scalar(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

pub fn agent(value: &str) -> bool {
    Regex::new("(?i)claude|codex|openai|anthropic")
        .unwrap()
        .is_match(value)
}

pub fn branch_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
}

pub fn tag(value: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "tag must contain only letters, digits, dots, underscores or hyphens",
    )
}

pub fn relative(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\0')
        && value.split('/').all(|p| !matches!(p, "" | "." | ".."))
}

pub fn resolve(path: &Path) -> Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for component in full.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(component),
        }
        if result.exists() || result.is_symlink() {
            result = result.canonicalize()?;
        }
    }
    Ok(result)
}

pub fn load(path: &Path) -> Result<Value> {
    let plan = read_json(path)?;
    validate(&plan)?;
    Ok(plan)
}

pub fn validate(plan: &Value) -> Result<()> {
    let required = [
        "lot",
        "branch",
        "base_branch",
        "base_sha",
        "author_name",
        "author_email",
        "root",
        "source",
        "remote_url",
        "commands",
        "prs",
    ];
    let optional = [
        "push_clone",
        "artifacts",
        "reference",
        "lot_pr",
        "note",
        "hooks_path",
        "lockfiles",
    ];
    let object = plan
        .as_object()
        .ok_or_else(|| Error::Refused("lot.json has unknown or missing keys".into()))?;
    require(
        required.iter().all(|k| object.contains_key(*k))
            && object
                .keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str())),
        "lot.json has unknown or missing keys",
    )?;
    require(
        Regex::new(r"\A[a-z0-9][a-z0-9-]{0,30}\z")
            .unwrap()
            .is_match(text(plan, "lot")?),
        "invalid lot",
    )?;
    require(is_sha(text(plan, "base_sha")?, 40, 40), "invalid base_sha")?;
    require(
        Regex::new(r"\A[^@\s]+@[^@\s]+\z")
            .unwrap()
            .is_match(text(plan, "author_email")?),
        "invalid author_email",
    )?;
    require(
        !text(plan, "author_name")?.is_empty() && !text(plan, "author_name")?.contains('\n'),
        "invalid author_name",
    )?;
    for key in ["author_name", "author_email"] {
        require(!agent(text(plan, key)?), format!("{key} names an agent"))?;
    }
    let cwd = std::env::current_dir()?;
    for key in ["branch", "base_branch"] {
        let value = text(plan, key)?;
        require(branch_name(value), format!("invalid {key}"))?;
        git(&cwd, &["check-ref-format", "--branch", value], &[0])?;
    }
    require(
        plan["branch"] != plan["base_branch"],
        "lot branch must differ from the main branch",
    )?;
    for key in ["root", "source", "push_clone", "artifacts", "reference"] {
        if let Some(value) = object.get(key) {
            require(
                value
                    .as_str()
                    .is_some_and(|s| s.starts_with('/') && relative(&s[1..])),
                format!("{key} must be an absolute path without parent components"),
            )?;
        }
    }
    let root = resolve(Path::new(text(plan, "root")?))?;
    let source = resolve(Path::new(text(plan, "source")?))?;
    for key in ["source", "push_clone"] {
        if let Some(value) = object.get(key) {
            let value = resolve(Path::new(value.as_str().unwrap()))?;
            require(
                value != root && value.starts_with(&root) && value != root.join("repo"),
                format!("{key} must be a separate campaign-owned clone below root"),
            )?;
            if key == "push_clone" {
                require(value != source, "push_clone must differ from source")?;
            }
        }
    }
    let remote = text(plan, "remote_url")?;
    require(
        !remote.is_empty() && !remote.chars().any(|c| c.is_whitespace() || c == '\0'),
        "invalid remote_url",
    )?;
    if object.contains_key("hooks_path") {
        require(
            relative(text(plan, "hooks_path")?),
            "hooks_path must be a project-relative path",
        )?;
    }
    if let Some(files) = object.get("lockfiles") {
        require(
            files
                .as_array()
                .is_some_and(|a| a.iter().all(|s| s.as_str().is_some_and(relative))),
            "lockfiles must contain project-relative paths",
        )?;
    }
    if object.contains_key("lot_pr") {
        require(numeric(&scalar(&plan["lot_pr"])), "invalid lot_pr")?;
    }
    let prs = plan["prs"]
        .as_array()
        .ok_or_else(|| Error::Refused("prs must be a non-empty list".into()))?;
    require(!prs.is_empty(), "prs must be a non-empty list")?;
    let mut ids = HashSet::new();
    for pr in prs {
        require(
            pr.as_object().is_some_and(|o| {
                o.keys()
                    .all(|k| ["id", "head", "branch", "author", "note"].contains(&k.as_str()))
            }),
            "bad PR entry",
        )?;
        let id = scalar(&pr["id"]);
        require(
            numeric(&id) && is_sha(text(pr, "head")?, 40, 40) && branch_name(text(pr, "branch")?),
            "PR requires numeric id, complete head SHA and source branch",
        )?;
        git(
            &cwd,
            &["check-ref-format", "--branch", text(pr, "branch")?],
            &[0],
        )?;
        require(
            ids.insert(id) && pr["branch"] != plan["branch"] && pr["branch"] != plan["base_branch"],
            "PR ids must be unique and source branches distinct from lot/base branches",
        )?;
    }
    project::validate(
        &plan["commands"],
        &std::env::var("NPM_TOKEN").unwrap_or_default(),
    )?;
    require(
        plan["commands"].get("build").is_some() && plan["commands"].get("check").is_some(),
        "project build and check commands are required",
    )
}

pub fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

pub fn exports(plan: &Value) -> String {
    let mut result = Vec::new();
    for (key, value) in plan.as_object().unwrap() {
        if value.is_string() || value.is_i64() || value.is_u64() {
            result.push(format!(
                "export {}={}",
                key.to_uppercase(),
                shell_quote(&scalar(value))
            ));
        }
    }
    for (key, value) in [
        ("R", format!("{}/repo", text(plan, "root").unwrap())),
        ("LOGS", format!("{}/logs", text(plan, "root").unwrap())),
        (
            "PRS",
            plan["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| scalar(&p["id"]))
                .collect::<Vec<_>>()
                .join(" "),
        ),
    ] {
        result.push(format!("export {key}={}", shell_quote(&value)));
    }
    for pr in plan["prs"].as_array().unwrap() {
        for key in ["head", "branch"] {
            result.push(format!(
                "export PR_{}_{}={}",
                key.to_uppercase(),
                scalar(&pr["id"]),
                shell_quote(pr[key].as_str().unwrap())
            ));
        }
    }
    result.join("\n")
}
