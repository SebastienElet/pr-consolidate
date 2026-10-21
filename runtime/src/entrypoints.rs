use std::ffi::OsStr;

pub const NAMES: &[&str] = &[
    "pr-consolidate",
    "forge",
    "native",
    "lotenv",
    "project-commands",
    "lot-build.sh",
    "lot-bundle.sh",
    "lot-commit.sh",
    "lot-lockfile.sh",
    "lot-merge-base.sh",
    "lot-merge.sh",
    "lot-preflight.sh",
    "lot-prepare.sh",
    "lot-push.sh",
    "lot-resync.sh",
    "lot-setup.sh",
];

pub fn arguments(name: &OsStr) -> Option<Vec<&'static str>> {
    let name = name.to_str()?;
    if name == "pr-consolidate" {
        return Some(Vec::new());
    }
    let known = NAMES.iter().copied().find(|candidate| *candidate == name)?;
    match known
        .strip_prefix("lot-")
        .and_then(|name| name.strip_suffix(".sh"))
    {
        Some(operation) => Some(vec!["lot", operation]),
        None => Some(vec![known]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_and_lot_aliases_dispatch_to_their_existing_operations() {
        assert_eq!(arguments(OsStr::new("forge")), Some(vec!["forge"]));
        assert_eq!(
            arguments(OsStr::new("lot-merge-base.sh")),
            Some(vec!["lot", "merge-base"])
        );
        assert_eq!(arguments(OsStr::new("pr-consolidate")), Some(vec![]));
        assert_eq!(arguments(OsStr::new("unknown-command")), None);
    }
}
