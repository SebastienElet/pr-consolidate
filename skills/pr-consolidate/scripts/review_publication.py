import re


APPROVAL_CLAIM = re.compile(r"approve|merge[\s_-]*ready", re.IGNORECASE)
UUID = re.compile(r"\{[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\}")


def account(c):
    user = c.api("/user", timeout=60)
    if c.forge_name() == "github":
        identity = user.get("id")
        if type(identity) is not int or identity <= 0:
            raise c.Refused("connected GitHub account ID is missing or invalid")
        return identity
    identity = user.get("uuid")
    if not isinstance(identity, str) or not UUID.fullmatch(identity):
        raise c.Refused("connected account UUID is missing or invalid")
    return identity


def comments(c, pr):
    values = c.paged(f"pullrequests/{pr}/comments?pagelen=100&fields=size,next,values.id,values.deleted,"
                     "values.content.raw,values.user.uuid,values.parent.id,values.inline", timeout=60)
    ids = [(value.get("_surface", "comment"), value.get("id")) for value in values]
    if any(type(cid) is not int or cid <= 0 for _, cid in ids) or len(set(ids)) != len(ids):
        raise c.Refused(f"#{pr}: invalid or repeated comment IDs")
    for value in values:
        if "deleted" in value and type(value["deleted"]) is not bool:
            raise c.Refused(f"#{pr}: invalid comment deletion state")
        if value.get("deleted"):
            continue
        if not isinstance(value.get("content"), dict) or not isinstance(value["content"].get("raw"), str) or \
                not isinstance(value.get("user", {}), dict):
            raise c.Refused(f"#{pr}: invalid comment content or author")
    return values


def own_general(comment, uuid):
    identity = (comment.get("user") or {}).get("id" if type(uuid) is int else "uuid")
    return (type(identity) is type(uuid) and identity == uuid and comment.get("parent") is None
            and comment.get("inline") is None and not comment.get("deleted")
            and (comment.get("_surface") != "review" or
                 comment.get("review_state") in {"COMMENTED", "APPROVED", "CHANGES_REQUESTED"}))


def exact_comments(c, pr, body, uuid=None):
    uuid = uuid or account(c)
    matches = [v for v in comments(c, pr) if not v.get("deleted")
               and ((v.get("content") or {}).get("raw") or "") == body]
    if any(not own_general(v, uuid) for v in matches) or len(matches) > 1:
        raise c.Refused(f"#{pr}: replacement comment has ambiguous ownership or duplicates")
    return [v["id"] for v in matches]


def post_comment(c, pr, body):
    if APPROVAL_CLAIM.search(body):
        raise c.Refused(f"#{pr}: the comment would read as an approval claim")
    c.api(f"pullrequests/{pr}/comments", "POST", {"content": {"raw": body}}, timeout=120)
    return 0


def check_head(c, pr, head):
    data = c.api(f"pullrequests/{pr}?fields=state,source.commit.hash", timeout=60)
    observed = ((data.get("source") or {}).get("commit") or {}).get("hash")
    if data.get("state") != "OPEN" or observed != head:
        raise c.Refused(f"#{pr}: expected OPEN head {head}, observed {data.get('state')} {observed}")


def marked_comment(c, pr, marker, uuid):
    found = []
    for value in comments(c, pr):
        raw = (value.get("content") or {}).get("raw") or ""
        if value.get("deleted") or marker not in raw:
            continue
        if raw.splitlines()[0] != marker or raw.count("<!-- pr-consolidate:review ") != 1:
            raise c.Refused(f"#{pr}: review marker is misplaced or ambiguous")
        if value.get("_surface") == "review":
            head = re.search(r"head=([0-9a-f]{40})", marker)[1]
            if value.get("review_state") not in {"COMMENTED", "APPROVED", "CHANGES_REQUESTED"} or \
                    value.get("commit_id") != head:
                raise c.Refused(f"#{pr}: review summary is pending, dismissed or belongs to another head")
        found.append(value)
    if len(found) > 1 or any(not own_general(v, uuid) for v in found):
        raise c.Refused(f"#{pr}: review marker has ambiguous ownership or duplicates")
    return found[0] if found else None


def review_marker(pr, head):
    return f"<!-- pr-consolidate:review pr={pr} head={head} -->"


def require_review(c, pr, head, uuid):
    if not re.fullmatch(r"[0-9a-f]{40}", head) or not marked_comment(c, pr, review_marker(pr, head), uuid):
        raise c.Refused(f"#{pr}: missing owned general review comment for full head {head}")


def review_comment(c, pr, head, body_path, mode):
    if mode not in {"dry-run", "apply"} or not re.fullmatch(r"[0-9a-f]{40}", head):
        raise c.Refused("review-comment requires a full 40-character SHA and dry-run or apply")
    body = c.read_text(body_path).strip()
    if not body or APPROVAL_CLAIM.search(body) or "<!-- pr-consolidate:" in body:
        raise c.Refused("review body is empty, contains a reserved marker or an approval claim")
    uuid = account(c)
    marker = review_marker(pr, head)
    raw = f"{marker}\n\nReviewed head: `{head}`.\n\n{body}"
    check_head(c, pr, head)
    found = marked_comment(c, pr, marker, uuid)
    check_head(c, pr, head)
    action = "update" if found else "create"
    if found and found["content"]["raw"] == raw:
        action = "unchanged"
    c.say(f"#{pr} review-comment {head}: {action} ({mode})")
    if mode == "dry-run" or action == "unchanged":
        return 0
    found = marked_comment(c, pr, marker, uuid)
    check_head(c, pr, head)
    surface = "reviews" if found and found.get("_surface") == "review" else "comments"
    path = f"pullrequests/{pr}/{surface}" + (f"/{found['id']}" if found else "")
    c.api(path, "PUT" if found else "POST", {"content": {"raw": raw}}, timeout=120)
    observed = marked_comment(c, pr, marker, uuid)
    if not observed or observed["content"]["raw"] != raw:
        raise c.Failed(f"#{pr}: review comment write not observed; inspect before retry")
    check_head(c, pr, head)
    c.say(f"#{pr} review-comment observed as comment {observed['id']}")
    return 0
