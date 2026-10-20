import argparse
import itertools
import fnmatch
import json
import pathlib
import re
import subprocess
import sys
import time

from forge_transport import ForgeAPIError, ForgeTimeout, api
import forge_transport
import git_context
import batch_replacement
import review_publication

_context = None


def base_branch():
    return _context["base_branch"]


def forge_name():
    return _context.get("forge", "bitbucket")


def repository():
    return _context["repository"]
SHA = re.compile(r"[0-9a-f]{12,40}")
SHORT_SHA = re.compile(r"[0-9a-f]{7,40}")
BRANCH = re.compile(r"[A-Za-z0-9._/-]+")
AGENT = re.compile(r"claude|codex|openai|anthropic", re.IGNORECASE)
IGNORED_PATHS = ("pnpm-lock.yaml", "bun.lock", "bun.lockb", "package-lock.json", "yarn.lock", "Cargo.lock")
DECLINE_TEMPLATE = (
    "Included in batch #{lot} (merged into `{base_branch}`, `{merge}`): this PR head `{head}` is an ancestor "
    "of the merged batch candidate; its commits are preserved unchanged. {extra}Closed as replaced by #{lot}."
)
RUN_FIELDS = ("values.build_number,values.state.name,values.state.result.name,values.target.commit.hash,"
              "values.target.ref_name")
PR_FIELDS = ("size,next,values.id,values.title,values.author.display_name,values.source.branch.name,"
             "values.source.commit.hash,values.destination.branch.name,values.draft,values.created_on,"
             "values.updated_on,values.participants.user.display_name,values.participants.approved,"
             "values.participants.state")
EXIT_FAILED, EXIT_REFUSED, EXIT_TIMEOUT = 1, 2, 3
_sleep = time.sleep
_now = time.time


class Refused(Exception):
    """A precondition does not hold; nothing further is written."""


class Failed(Exception):
    """A tool or transport failed."""


def require_destination(data, pr):
    destination = ((data.get("destination") or {}).get("branch") or {}).get("name")
    if destination != base_branch():
        raise Refused(f"#{pr} targets {destination}, not {base_branch()}")


def say(line):
    print(line, flush=True)


def mask(text):
    return re.sub(r"//[^/\s@]+@", "//***@", str(text))


def read_text(path):
    try:
        return pathlib.Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise Refused(f"cannot read {path}: {exc}") from exc


def paged(path, timeout=120):
    """Every value of a paginated collection (path must ask for size,next); refuses a count that differs from size."""
    values, url, seen, size = [], path, set(), None
    while url:
        if url in seen:
            raise Refused(f"pagination loops on {url[:120]}")
        seen.add(url)
        page = api(url, timeout=timeout)
        if not isinstance(page, dict) or not isinstance(page.get("values"), list) or \
                any(not isinstance(value, dict) for value in page["values"]) or \
                (page.get("next") is not None and not isinstance(page["next"], str)):
            raise Refused("invalid paginated response")
        if size is None:
            size = page.get("size")
        if type(page.get("size")) is not int or page["size"] < 0 or page["size"] != size:
            raise Refused("missing or changed pagination size")
        values.extend(page["values"])
        url = page.get("next")
        if len(values) > size or (url and not page["values"]):
            raise Refused("pagination exceeds its size or has an empty intermediate page")
    name = path.split("?")[0]
    if not isinstance(size, int):
        raise Refused(f"{name}: no size reported, the count cannot be checked")
    if len(values) != size:
        raise Refused(f"{name}: counted {len(values)} values but size is {size}")
    return values


def coupling_paths(paths):
    return {p for p in paths if not any(fnmatch.fnmatchcase(p, pattern) for pattern in IGNORED_PATHS)}


def suggest(records):
    """Pairs sharing non-ignored paths and their connected components; drafts, changes requested and PRs not
    targeting the primary branch are kept apart and never grouped."""
    apart = {"draft": [], "changes requested": [], "not targeting primary branch": []}
    groupable = {}
    for pr in records:
        if pr["draft"]:
            apart["draft"].append(pr["id"])
        elif pr["changes_requested_by"]:
            apart["changes requested"].append(pr["id"])
        elif pr["destination"] != base_branch():
            apart["not targeting primary branch"].append(pr["id"])
        else:
            groupable[pr["id"]] = coupling_paths(pr["paths"])
    pairs, parent = [], {pid: pid for pid in groupable}

    def root(pid):
        while parent[pid] != pid:
            parent[pid] = parent[parent[pid]]
            pid = parent[pid]
        return pid

    for a, b in itertools.combinations(sorted(groupable), 2):
        shared = sorted(groupable[a] & groupable[b])
        if shared:
            pairs.append({"prs": [a, b], "count": len(shared), "paths": shared})
            parent[root(a)] = root(b)
    components = {}
    for pid in sorted(groupable):
        components.setdefault(root(pid), []).append(pid)
    lots = sorted((ids for ids in components.values() if len(ids) > 1), key=lambda ids: (-len(ids), ids[0]))
    alone = sorted(ids[0] for ids in components.values() if len(ids) == 1)
    return {"pairs": pairs, "lots": lots, "alone": alone, "apart": apart}


def inventory(out):
    prs = paged(f"pullrequests?state=OPEN&pagelen=50&fields={PR_FIELDS}")
    ids = [pr["id"] for pr in prs]
    if len(set(ids)) != len(ids):
        raise Refused("a PR is listed twice across pages")
    say(f"open PRs: {len(prs)}, count equals the reported size")
    records = []
    for pr in sorted(prs, key=lambda p: p["id"]):
        pid = pr["id"]
        diff = paged(f"pullrequests/{pid}/diffstat?pagelen=500&fields=size,next,values.new.path,values.old.path,"
                     "values.lines_added,values.lines_removed")
        paths = sorted({side["path"] for d in diff for side in (d.get("new"), d.get("old")) if side and side.get("path")})
        statuses = paged(f"pullrequests/{pid}/statuses?pagelen=50&fields=size,next,values.state", timeout=60)
        people = pr.get("participants") or []
        record = {
            "id": pid, "title": pr.get("title", ""), "author": (pr.get("author") or {}).get("display_name", ""),
            "branch": pr["source"]["branch"]["name"], "head": pr["source"]["commit"]["hash"],
            "destination": pr["destination"]["branch"]["name"], "draft": bool(pr.get("draft")),
            "approved_by": sorted(p["user"]["display_name"] for p in people
                                  if p.get("approved") or p.get("state") == "approved"),
            "changes_requested_by": sorted(p["user"]["display_name"] for p in people
                                           if p.get("state") == "changes_requested"),
            "created_on": pr.get("created_on", ""), "updated_on": pr.get("updated_on", ""), "paths": paths,
            "added": sum(d.get("lines_added") or 0 for d in diff),
            "removed": sum(d.get("lines_removed") or 0 for d in diff),
            "build": [s.get("state") for s in statuses],
        }
        records.append(record)
        say(f"#{pid} {'DRAFT ' if record['draft'] else ''}{record['author']} {record['branch']} {record['head'][:12]} "
            f"-> {record['destination']}: {len(paths)} paths +{record['added']} -{record['removed']}, "
            f"approved by {record['approved_by']}, changes requested by {record['changes_requested_by']}, "
            f"build {record['build']}, updated {record['updated_on'][:16]}")
    plan = suggest(records)
    for pair in plan["pairs"]:
        a, b = pair["prs"]
        more = " ..." if pair["count"] > 3 else ""
        say(f"pair #{a} #{b}: {pair['count']} shared paths: {', '.join(pair['paths'][:3])}{more}")
    for ids in plan["lots"]:
        say(f"suggested lot: {' '.join(f'#{pid}' for pid in ids)}")
    say(f"alone: {' '.join(f'#{pid}' for pid in plan['alone']) or '-'}")
    for reason, apart_ids in plan["apart"].items():
        if apart_ids:
            say(f"apart ({reason}), never grouped: {' '.join(f'#{pid}' for pid in apart_ids)}")
    say(f"ignored for coupling: {', '.join(IGNORED_PATHS)}")
    say("suggestion only: forming a lot stays a human decision")
    document = {"generated": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "repository": repository(),
                "base_branch": base_branch(), "size": len(records), "ignored_paths": list(IGNORED_PATHS), "prs": records, **plan}
    try:
        pathlib.Path(out).write_text(json.dumps(document, ensure_ascii=False, indent=1), encoding="utf-8")
    except OSError as exc:
        raise Failed(f"cannot write {out}: {exc}") from exc
    say(f"written {out}: {len(records)} PRs")
    return 0


def create(branch, title, description_path):
    if not BRANCH.fullmatch(branch) or branch == base_branch():
        raise Refused(f"bad source branch {branch!r}")
    if not title.strip() or "\n" in title:
        raise Refused("the title must be one non-empty line")
    description = read_text(description_path)
    if not description.strip():
        raise Refused(f"{description_path} is empty")
    existing = paged(f'pullrequests?q=source.branch.name="{branch}" AND state="OPEN"&fields=size,next,values.id',
                     timeout=60)
    if existing:
        raise Refused(f"an open PR already exists for {branch}: {[pr.get('id') for pr in existing]}")
    pr = api("pullrequests", "POST", {"title": title, "description": description,
                                      "source": {"branch": {"name": branch}},
                                      "destination": {"branch": {"name": base_branch()}}, "close_source_branch": False},
             timeout=120)
    source, destination = pr["source"]["branch"]["name"], pr["destination"]["branch"]["name"]
    say(f"created #{pr['id']} {pr['source']['commit']['hash']} {source} -> {destination} "
        f"close_source_branch={pr.get('close_source_branch')}")
    retention_ok = forge_name() == "github" or pr.get("close_source_branch") is False
    return 0 if source == branch and destination == base_branch() and retention_ok else EXIT_FAILED


def desc(pr, expected_path, new_path, check_only):
    expected, new = read_text(expected_path), read_text(new_path)
    if not new.strip():
        raise Refused(f"{new_path} is empty")
    current = api(f"pullrequests/{pr}?fields=title,state,description,source.commit.hash", timeout=60)
    same = (current.get("description") or "").strip() == expected.strip()
    say(f"#{pr} {current.get('state')} head {current['source']['commit']['hash']} description unchanged since "
        f"{pathlib.Path(expected_path).name}: {same}")
    if not same:
        raise Refused(f"#{pr} description is no longer {pathlib.Path(expected_path).name}; re-read it before writing")
    if check_only:
        return 0
    updated = api(f"pullrequests/{pr}", "PUT", {"title": current["title"], "description": new}, timeout=120)
    ok = (updated.get("description") or "").strip() == new.strip()
    say(f"updated #{pr}: description now matches {pathlib.Path(new_path).name}: {ok}")
    return 0 if ok else EXIT_FAILED


def wait_ci(sha, timeout=3600, poll=60):
    import github_ci
    import bitbucket_ci
    implementation = github_ci if forge_name() == "github" else bitbucket_ci
    return implementation.wait_ci(sys.modules[__name__], sha, timeout, poll)


def watch(prs, timeout=5400, poll=60):
    import github_ci
    import bitbucket_ci
    implementation = github_ci if forge_name() == "github" else bitbucket_ci
    return implementation.watch(sys.modules[__name__], prs, timeout, poll)


def git(clone, *args, ok=(0,)):
    try:
        done = subprocess.run(["git", "-C", str(clone), *args], capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=900, check=False,
                              env=git_context.git_environment())
    except (OSError, subprocess.SubprocessError) as exc:
        raise Failed(f"git {args[0]}: {mask(exc)}") from exc
    if done.returncode not in ok:
        raise Failed(f"git {' '.join(args[:2])} exited {done.returncode}: {mask(done.stderr.strip())[:300]}")
    return done


def rev_parse(clone, rev):
    done = git(clone, "rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}", ok=(0, 1))
    return done.stdout.strip() if done.returncode == 0 else None


def verify_merge(pr, candidate, clone):
    if not SHA.fullmatch(candidate):
        raise Refused(f"bad candidate {candidate!r}")
    if not pathlib.Path(clone).is_dir():
        raise Refused(f"no push clone at {clone}")
    clone_context = git_context.remote_context(clone)
    if (clone_context["repository"], clone_context["forge"]) != (repository(), forge_name()):
        raise Refused("push clone origin does not match the current repository")
    data = api(f"pullrequests/{pr}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name",
               timeout=60)
    merge = (data.get("merge_commit") or {}).get("hash") or ""
    source = ((data.get("source") or {}).get("commit") or {}).get("hash") or ""
    destination = ((data.get("destination") or {}).get("branch") or {}).get("name")
    say(f"#{pr} {data.get('state')} -> {destination}, source {source or '-'}, merge_commit {merge or '-'}")
    if data.get("state") != "MERGED" or not SHORT_SHA.fullmatch(merge):
        raise Refused(f"#{pr} is {data.get('state')} without a merge commit")
    if destination != base_branch():
        raise Refused(f"#{pr} targets {destination}, not {base_branch()}")
    full_candidate = rev_parse(clone, candidate)
    if not full_candidate:
        raise Refused(f"candidate {candidate} is not a commit of {clone}")
    if not source or not full_candidate.startswith(source):
        raise Refused(f"candidate {full_candidate[:12]} is not the head {source or '-'} of #{pr}")
    git(clone, "fetch", "--no-tags", "origin", f"refs/heads/{base_branch()}:refs/remotes/origin/{base_branch()}")
    tip = rev_parse(clone, f"refs/remotes/origin/{base_branch()}")
    say(f"fetched origin/{base_branch()} {tip}")
    merged = rev_parse(clone, merge)
    if not merged:
        raise Refused(f"merge commit {merge} not found in {clone} after the fetch")
    author, committer = git(clone, "show", "-s", "--format=%an <%ae>%n%cn <%ce>", merged).stdout.strip().split("\n")
    say(f"merge commit {merged} author {author} committer {committer}")
    if AGENT.search(author) or AGENT.search(committer):
        raise Refused(f"merge commit {merged[:12]} names an agent identity")
    if git(clone, "merge-base", "--is-ancestor", merged, tip, ok=(0, 1)).returncode:
        raise Refused(f"merge commit {merged[:12]} is not in origin/{base_branch()}")
    parent = rev_parse(clone, f"{merged}^1")
    if not parent:
        raise Refused(f"merge commit {merged[:12]} has no parent")
    tree = git(clone, "rev-parse", f"{merged}^{{tree}}").stdout.strip()
    replay = git(clone, "merge-tree", "--write-tree", parent, full_candidate, ok=(0, 1))
    if replay.returncode:
        raise Refused(f"candidate {full_candidate[:12]} conflicts with the first parent {parent[:12]}")
    expected = replay.stdout.split("\n", 1)[0].strip()
    say(f"first parent {parent}; merge tree {tree}; merge-tree --write-tree parent candidate {expected}")
    if tree != expected:
        raise Refused(f"tree of {merged[:12]} differs from candidate {full_candidate[:12]} merged onto {parent[:12]}")
    say(f"verified: #{pr} merged as {merged[:12]} in origin/{base_branch()}, tree equals the candidate merged onto its parent")
    return 0


def decline_body(item, lot):
    return batch_replacement.decline_body(sys.modules[__name__], item, lot)[0]


def comment_ids(pr, body):
    return review_publication.exact_comments(sys.modules[__name__], pr, body)


def post_comment(pr, body):
    return review_publication.post_comment(sys.modules[__name__], pr, body)


def decline(plan_path, mode):
    return batch_replacement.decline(sys.modules[__name__], plan_path, mode)


def pr_id(value):
    if not re.fullmatch(r"[1-9][0-9]*", value):
        raise argparse.ArgumentTypeError(f"bad PR id {value!r}")
    return int(value)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="forge.py", description="Consolidate pull requests from the current repository")
    parser.add_argument("--repository", help="repository checkout directory (default: current directory)")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("context")
    sub.add_parser("inventory").add_argument("--out", required=True)
    p = sub.add_parser("create")
    for name in ("branch", "title", "description"):
        p.add_argument(name)
    p = sub.add_parser("desc")
    p.add_argument("pr", type=pr_id)
    p.add_argument("expected")
    p.add_argument("new")
    p.add_argument("--check", action="store_true", help="compare only, write nothing")
    sub.add_parser("wait-ci").add_argument("sha")
    sub.add_parser("watch").add_argument("prs", type=pr_id, nargs="+")
    p = sub.add_parser("verify-merge")
    p.add_argument("pr", type=pr_id)
    p.add_argument("candidate")
    p.add_argument("push_clone")
    p = sub.add_parser("replace", aliases=["decline"])
    p.add_argument("plan")
    p.add_argument("mode", choices=["dry-run", "apply"])
    p = sub.add_parser("review-comment")
    p.add_argument("pr", type=pr_id)
    p.add_argument("head")
    p.add_argument("body")
    p.add_argument("mode", choices=["dry-run", "apply"])
    args = parser.parse_args(argv)
    try:
        global _context
        _context = git_context.discover(args.repository)
        forge_transport.configure(_context)
        if args.command == "context":
            say(json.dumps(_context, ensure_ascii=False))
            return 0
        if args.command == "inventory":
            return inventory(args.out)
        if args.command == "create":
            return create(args.branch, args.title, args.description)
        if args.command == "desc":
            return desc(args.pr, args.expected, args.new, args.check)
        if args.command == "wait-ci":
            return wait_ci(args.sha)
        if args.command == "watch":
            return watch(args.prs)
        if args.command == "verify-merge":
            return verify_merge(args.pr, args.candidate, args.push_clone)
        if args.command == "review-comment":
            return review_publication.review_comment(sys.modules[__name__], args.pr, args.head, args.body, args.mode)
        return decline(args.plan, args.mode)
    except (Refused, git_context.ContextError) as exc:
        say(f"REFUSED: {mask(exc)}")
        return EXIT_REFUSED
    except ForgeTimeout as exc:
        say(f"TIMEOUT: {exc}")
        return EXIT_TIMEOUT
    except (Failed, ForgeAPIError) as exc:
        say(f"FAILED: {mask(exc)[:300]}")
        return EXIT_FAILED


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="backslashreplace")
    sys.exit(main())
