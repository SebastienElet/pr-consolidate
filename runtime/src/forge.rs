use crate::context::{Context, is_sha, remote_context};
use crate::error::{Error, Result, require};
use crate::plan::{agent, branch_name};
use crate::process::{capture, mask, sanitize_git};
use crate::transport::Api;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub const IGNORED_PATHS: &[&str] = &[
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "package-lock.json",
    "yarn.lock",
    "Cargo.lock",
];
pub const RUN_FIELDS: &str = "values.build_number,values.state.name,values.state.result.name,values.target.commit.hash,values.target.ref_name";
const PR_FIELDS: &str = "size,next,values.id,values.title,values.author.display_name,values.source.branch.name,values.source.commit.hash,values.destination.branch.name,values.draft,values.created_on,values.updated_on,values.participants.user.display_name,values.participants.approved,values.participants.state";

pub struct Forge<A> {
    pub context: Context,
    pub transport: A,
}

pub fn string<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}
pub fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| Error::Refused(format!("cannot read {}: {error}", path.display())))
}

impl<A: Api> Forge<A> {
    pub fn api(
        &self,
        path: &str,
        method: &str,
        payload: Option<&Value>,
        timeout: u64,
    ) -> Result<Value> {
        self.transport.api(path, method, payload, timeout)
    }
    pub fn get(&self, path: &str) -> Result<Value> {
        self.api(path, "GET", None, 60)
    }
    pub fn destination(&self, data: &Value, pr: u64) -> Result<()> {
        let destination = string(data, "/destination/branch/name");
        require(
            destination == self.context.base_branch,
            format!(
                "#{pr} targets {destination}, not {}",
                self.context.base_branch
            ),
        )
    }
    pub fn paged(&self, path: &str, timeout: u64) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        let mut url = path.to_owned();
        let mut seen = HashSet::new();
        let mut size = None;
        while !url.is_empty() {
            require(
                seen.len() < 500 && seen.insert(url.clone()),
                "pagination loops or exceeds its bound",
            )?;
            let page = self.api(&url, "GET", None, timeout)?;
            let batch = page["values"]
                .as_array()
                .ok_or_else(|| Error::Refused("invalid paginated response".into()))?;
            require(
                page.is_object()
                    && batch.iter().all(Value::is_object)
                    && (page["next"].is_null() || page["next"].is_string()),
                "invalid paginated response",
            )?;
            let current = page["size"]
                .as_u64()
                .ok_or_else(|| Error::Refused("missing or changed pagination size".into()))?;
            require(
                size.is_none_or(|size| size == current),
                "missing or changed pagination size",
            )?;
            size = Some(current);
            values.extend(batch.iter().cloned());
            url = page["next"].as_str().unwrap_or("").to_owned();
            require(
                values.len() as u64 <= current && (url.is_empty() || !batch.is_empty()),
                "pagination exceeds its size or has an empty intermediate page",
            )?;
        }
        require(
            size == Some(values.len() as u64),
            format!(
                "{}: count differs from reported size",
                path.split('?').next().unwrap()
            ),
        )?;
        Ok(values)
    }
    pub fn create(&self, branch: &str, title: &str, description: &Path) -> Result<i32> {
        require(
            branch_name(branch) && branch != self.context.base_branch,
            "bad source branch",
        )?;
        require(
            !title.trim().is_empty() && !title.contains('\n'),
            "the title must be one non-empty line",
        )?;
        let description = read_text(description)?;
        require(!description.trim().is_empty(), "description is empty")?;
        let existing = self.paged(&format!("pullrequests?q=source.branch.name=\"{branch}\" AND state=\"OPEN\"&fields=size,next,values.id"), 60)?;
        require(
            existing.is_empty(),
            format!("an open PR already exists for {branch}"),
        )?;
        let pr = self.api("pullrequests", "POST", Some(&json!({"title":title,"description":description,"source":{"branch":{"name":branch}},"destination":{"branch":{"name":self.context.base_branch}},"close_source_branch":false})), 120)?;
        println!(
            "created #{} {} {} -> {} close_source_branch={}",
            pr["id"],
            string(&pr, "/source/commit/hash"),
            string(&pr, "/source/branch/name"),
            string(&pr, "/destination/branch/name"),
            pr["close_source_branch"]
        );
        Ok(
            if string(&pr, "/source/branch/name") == branch
                && string(&pr, "/destination/branch/name") == self.context.base_branch
                && (self.context.forge == crate::context::ForgeKind::GitHub
                    || pr["close_source_branch"] == false)
            {
                0
            } else {
                1
            },
        )
    }
    pub fn desc(&self, pr: u64, expected: &Path, new: &Path, check_only: bool) -> Result<i32> {
        let expected = read_text(expected)?;
        let new = read_text(new)?;
        require(!new.trim().is_empty(), "new description is empty")?;
        let current = self.get(&format!(
            "pullrequests/{pr}?fields=title,state,description,source.commit.hash"
        ))?;
        let same = current["description"].as_str().unwrap_or("").trim() == expected.trim();
        println!(
            "#{pr} {} head {} description unchanged: {same}",
            current["state"],
            string(&current, "/source/commit/hash")
        );
        require(
            same,
            format!(
                "#{pr} description is no longer the expected content; re-read it before writing"
            ),
        )?;
        if check_only {
            return Ok(0);
        }
        let updated = self.api(
            &format!("pullrequests/{pr}"),
            "PUT",
            Some(&json!({"title":current["title"],"description":new})),
            120,
        )?;
        let ok = updated["description"].as_str().unwrap_or("").trim() == new.trim();
        println!("updated #{pr}: description now matches: {ok}");
        Ok(if ok { 0 } else { 1 })
    }
    pub fn inventory(&self, out: &Path) -> Result<i32> {
        let mut prs = self.paged(
            &format!("pullrequests?state=OPEN&pagelen=50&fields={PR_FIELDS}"),
            120,
        )?;
        let mut ids = HashSet::new();
        for pr in &prs {
            require(
                pr["id"].as_u64().is_some_and(|id| id > 0 && ids.insert(id)),
                "a PR has a missing or repeated ID",
            )?;
        }
        prs.sort_by_key(|p| p["id"].as_u64().unwrap());
        println!("open PRs: {}, count equals the reported size", prs.len());
        let mut records = Vec::new();
        for pr in prs {
            let id = pr["id"].as_u64().unwrap();
            let diff = self.paged(&format!("pullrequests/{id}/diffstat?pagelen=500&fields=size,next,values.new.path,values.old.path,values.lines_added,values.lines_removed"), 120)?;
            let mut paths = BTreeSet::new();
            for entry in &diff {
                for side in ["new", "old"] {
                    if let Some(path) = entry[side]["path"].as_str().filter(|s| !s.is_empty()) {
                        paths.insert(path);
                    }
                }
            }
            let statuses = self.paged(
                &format!("pullrequests/{id}/statuses?pagelen=50&fields=size,next,values.state"),
                60,
            )?;
            let mut approved = Vec::new();
            let mut rejected = Vec::new();
            if let Some(people) = pr["participants"].as_array() {
                for person in people {
                    let name = string(person, "/user/display_name");
                    if person["approved"] == true || person["state"] == "approved" {
                        approved.push(name);
                    }
                    if person["state"] == "changes_requested" {
                        rejected.push(name);
                    }
                }
            }
            approved.sort();
            rejected.sort();
            let added: u64 = diff
                .iter()
                .map(|d| d["lines_added"].as_u64().unwrap_or(0))
                .sum();
            let removed: u64 = diff
                .iter()
                .map(|d| d["lines_removed"].as_u64().unwrap_or(0))
                .sum();
            let build: Vec<_> = statuses.iter().map(|s| s["state"].clone()).collect();
            let record = json!({"id":id,"title":pr["title"].as_str().unwrap_or(""),"author":string(&pr,"/author/display_name"),"branch":string(&pr,"/source/branch/name"),"head":string(&pr,"/source/commit/hash"),"destination":string(&pr,"/destination/branch/name"),"draft":pr["draft"] == true,"approved_by":approved,"changes_requested_by":rejected,"created_on":pr["created_on"].as_str().unwrap_or(""),"updated_on":pr["updated_on"].as_str().unwrap_or(""),"paths":paths,"added":added,"removed":removed,"build":build});
            println!(
                "#{id} {}{} {} {} -> {}: {} paths +{added} -{removed}, approved by {}, changes requested by {}, build {}",
                if record["draft"] == true {
                    "DRAFT "
                } else {
                    ""
                },
                record["author"].as_str().unwrap(),
                record["branch"].as_str().unwrap(),
                record["head"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .take(12)
                    .collect::<String>(),
                record["destination"].as_str().unwrap(),
                paths.len(),
                record["approved_by"],
                record["changes_requested_by"],
                record["build"]
            );
            records.push(record);
        }
        let mut document = suggest(&records, &self.context.base_branch);
        for pair in document["pairs"].as_array().unwrap() {
            println!(
                "pair #{} #{}: {} shared paths: {}",
                pair["prs"][0], pair["prs"][1], pair["count"], pair["paths"]
            );
        }
        for lot in document["lots"].as_array().unwrap() {
            println!(
                "suggested lot: {}",
                lot.as_array()
                    .unwrap()
                    .iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        println!("alone: {}", document["alone"]);
        for (reason, apart) in document["apart"].as_object().unwrap() {
            if !apart.as_array().unwrap().is_empty() {
                println!("apart ({reason}), never grouped: {apart}");
            }
        }
        println!(
            "ignored for coupling: {}\nsuggestion only: forming a lot stays a human decision",
            IGNORED_PATHS.join(", ")
        );
        document["generated"] = json!(timestamp());
        document["repository"] = json!(self.context.repository);
        document["base_branch"] = json!(self.context.base_branch);
        document["size"] = json!(records.len());
        document["ignored_paths"] = json!(IGNORED_PATHS);
        document["prs"] = json!(records);
        std::fs::write(out, serde_json::to_string_pretty(&document).unwrap())
            .map_err(|e| Error::Failed(format!("cannot write {}: {e}", out.display())))?;
        println!("written {}: {} PRs", out.display(), records.len());
        Ok(0)
    }
    fn git(
        &self,
        clone: &Path,
        args: &[&str],
        acceptable: &[i32],
    ) -> Result<crate::process::Output> {
        let mut command = Command::new("git");
        command.arg("-C").arg(clone).args(args);
        sanitize_git(&mut command);
        let output = capture(&mut command, None, Duration::from_secs(900), false)
            .map_err(|_| Error::Failed(format!("git {} could not run", args[0])))?;
        if output.timed_out || !acceptable.contains(&output.code) {
            return Err(Error::Failed(format!(
                "git {} exited {}: {}",
                args[0],
                output.code,
                mask(&output.stderr).chars().take(300).collect::<String>()
            )));
        }
        Ok(output)
    }
    fn rev_parse(&self, clone: &Path, rev: &str) -> Result<Option<String>> {
        let result = self.git(
            clone,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
            &[0, 1],
        )?;
        Ok(if result.code == 0 {
            Some(result.stdout.trim().into())
        } else {
            None
        })
    }
    pub fn verify_merge(&self, pr: u64, candidate: &str, clone: &Path) -> Result<i32> {
        require(is_sha(candidate, 12, 40), "bad candidate")?;
        require(clone.is_dir(), "no push clone")?;
        let context = remote_context(clone)?;
        require(
            context.repository == self.context.repository && context.forge == self.context.forge,
            "push clone origin does not match the current repository",
        )?;
        let data = self.get(&format!("pullrequests/{pr}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name"))?;
        let merge = string(&data, "/merge_commit/hash");
        let source = string(&data, "/source/commit/hash");
        println!(
            "#{pr} {} -> {}, source {source}, merge_commit {merge}",
            data["state"],
            string(&data, "/destination/branch/name")
        );
        require(
            data["state"] == "MERGED" && is_sha(merge, 7, 40),
            format!("#{pr} is not MERGED with a merge commit"),
        )?;
        self.destination(&data, pr)?;
        let candidate = self
            .rev_parse(clone, candidate)?
            .ok_or_else(|| Error::Refused("candidate is not a commit of the push clone".into()))?;
        require(
            !source.is_empty() && candidate.starts_with(source),
            "candidate is not the PR head",
        )?;
        let base = &self.context.base_branch;
        self.git(
            clone,
            &[
                "fetch",
                "--no-tags",
                "origin",
                &format!("refs/heads/{base}:refs/remotes/origin/{base}"),
            ],
            &[0],
        )?;
        let tip = self
            .rev_parse(clone, &format!("refs/remotes/origin/{base}"))?
            .ok_or_else(|| Error::Refused("primary tip is missing".into()))?;
        let merged = self
            .rev_parse(clone, merge)?
            .ok_or_else(|| Error::Refused("merge commit not found after fetch".into()))?;
        let identities = self
            .git(
                clone,
                &["show", "-s", "--format=%an <%ae>%n%cn <%ce>", &merged],
                &[0],
            )?
            .stdout;
        require(!agent(&identities), "merge commit names an agent identity")?;
        require(
            self.git(
                clone,
                &["merge-base", "--is-ancestor", &merged, &tip],
                &[0, 1],
            )?
            .code
                == 0,
            "merge commit is not in the primary branch",
        )?;
        let parent = self
            .rev_parse(clone, &format!("{merged}^1"))?
            .ok_or_else(|| Error::Refused("merge commit has no parent".into()))?;
        let tree = self
            .git(clone, &["rev-parse", &format!("{merged}^{{tree}}")], &[0])?
            .stdout;
        let replay = self.git(
            clone,
            &["merge-tree", "--write-tree", &parent, &candidate],
            &[0, 1],
        )?;
        require(
            replay.code == 0,
            "candidate conflicts with the first parent",
        )?;
        let expected = replay.stdout.lines().next().unwrap_or("").trim();
        require(
            tree.trim() == expected,
            "tree of merge differs from candidate merged onto its first parent",
        )?;
        println!(
            "verified: #{pr} merged as {merged} in origin/{base}, tree equals the candidate merged onto its parent"
        );
        Ok(0)
    }
}

pub fn suggest(records: &[Value], base: &str) -> Value {
    let mut apart = json!({"draft":[],"changes requested":[],"not targeting primary branch":[]});
    let mut paths: BTreeMap<u64, BTreeSet<&str>> = BTreeMap::new();
    for pr in records {
        let reason = if pr["draft"] == true {
            Some("draft")
        } else if pr["changes_requested_by"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            Some("changes requested")
        } else if pr["destination"] != base {
            Some("not targeting primary branch")
        } else {
            None
        };
        if let Some(reason) = reason {
            apart[reason].as_array_mut().unwrap().push(pr["id"].clone());
        } else {
            paths.insert(
                pr["id"].as_u64().unwrap(),
                pr["paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|p| !IGNORED_PATHS.contains(p))
                    .collect(),
            );
        }
    }
    let ids: Vec<_> = paths.keys().copied().collect();
    let mut parents: BTreeMap<_, _> = ids.iter().map(|&id| (id, id)).collect();
    fn root(parents: &BTreeMap<u64, u64>, mut id: u64) -> u64 {
        while parents[&id] != id {
            id = parents[&id];
        }
        id
    }
    let mut pairs = Vec::new();
    for (i, &a) in ids.iter().enumerate() {
        for &b in &ids[i + 1..] {
            let shared: Vec<_> = paths[&a].intersection(&paths[&b]).copied().collect();
            if !shared.is_empty() {
                pairs.push(json!({"prs":[a,b],"count":shared.len(),"paths":shared}));
                parents.insert(root(&parents, a), root(&parents, b));
            }
        }
    }
    let mut components: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for id in ids {
        components.entry(root(&parents, id)).or_default().push(id);
    }
    let mut lots: Vec<_> = components
        .values()
        .filter(|ids| ids.len() > 1)
        .cloned()
        .collect();
    lots.sort_by_key(|ids| (std::cmp::Reverse(ids.len()), ids[0]));
    let mut alone: Vec<_> = components
        .values()
        .filter(|ids| ids.len() == 1)
        .map(|ids| ids[0])
        .collect();
    alone.sort();
    json!({"pairs":pairs,"lots":lots,"alone":alone,"apart":apart})
}

fn timestamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut result = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::gmtime_r(&now, result.as_mut_ptr()) }.is_null() {
        return String::new();
    }
    let result = unsafe { result.assume_init() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        result.tm_year + 1900,
        result.tm_mon + 1,
        result.tm_mday,
        result.tm_hour,
        result.tm_min,
        result.tm_sec
    )
}
