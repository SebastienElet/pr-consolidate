import json
import re

import review_publication


def load_plan(c, path):
    try:
        plan = json.loads(c.read_text(path))
    except json.JSONDecodeError as exc:
        raise c.Refused(f"{path}: invalid JSON") from exc
    if not isinstance(plan, dict) or set(plan) - {"lot_pr", "items", "note"}:
        raise c.Refused("plan must be {lot_pr, items[, note]}")
    lot, items = str(plan.get("lot_pr", "")), plan.get("items")
    if not re.fullmatch(r"[1-9][0-9]*", lot) or not isinstance(items, list) or not items:
        raise c.Refused("plan needs a lot_pr and a non-empty items list")
    seen = set()
    result = []
    for item in items:
        if not isinstance(item, dict) or set(item) - {"id", "head", "extra", "note"} or \
                not re.fullmatch(r"[1-9][0-9]*", str(item.get("id", ""))) or \
                not c.SHA.fullmatch(str(item.get("head", ""))) or not isinstance(item.get("extra", ""), str):
            raise c.Refused("bad plan item: id, head (12-40 hex) and an optional extra string")
        if str(item["id"]) in seen or str(item["id"]) == lot:
            raise c.Refused(f"plan item #{item['id']} is the lot or listed twice")
        seen.add(str(item["id"]))
        result.append({"id": int(item["id"]), "head": item["head"], "extra": item.get("extra", "").strip()})
    return int(lot), result


def merged_lot(c, lot_id):
    data = c.api(f"pullrequests/{lot_id}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name",
                 timeout=60)
    merge = (data.get("merge_commit") or {}).get("hash") or ""
    if data.get("state") != "MERGED" or not c.SHORT_SHA.fullmatch(merge):
        raise c.Refused(f"lot #{lot_id} is {data.get('state')}, not MERGED")
    if ((data.get("destination") or {}).get("branch") or {}).get("name") != c.base_branch():
        raise c.Refused(f"lot #{lot_id} does not target {c.base_branch()}")
    head = ((data.get("source") or {}).get("commit") or {}).get("hash") or ""
    if not c.SHA.fullmatch(head):
        raise c.Refused(f"lot #{lot_id} has no valid source head")
    return {"id": lot_id, "merge": merge, "head": head}


def decline_body(c, item, lot):
    current = c.api(f"pullrequests/{item['id']}?fields=state,source.commit.hash,destination.branch.name", timeout=60)
    c.require_destination(current, item["id"])
    head = ((current.get("source") or {}).get("commit") or {}).get("hash") or ""
    state = current.get("state")
    if state not in {"OPEN", "DECLINED"} or not isinstance(head, str) or \
            not re.fullmatch(r"[0-9a-f]{40}", head) or not head.startswith(item["head"]):
        raise c.Refused(f"#{item['id']}: state {state} head {head} (expected OPEN {item['head']})")
    base = c.api(f"merge-base/{head}..{lot['head']}", timeout=60).get("hash") or ""
    if not isinstance(base, str) or not re.fullmatch(r"[0-9a-f]{40}", base) or base != head:
        raise c.Refused(f"#{item['id']}: head {head} is not an ancestor of the lot head {lot['head']} "
                        "(merge-base is not the exact source head)")
    extra = f"{item['extra']} " if item["extra"] else ""
    body = c.DECLINE_TEMPLATE.format(lot=lot["id"], merge=lot["merge"][:8], head=head[:8], extra=extra,
                                    base_branch=c.base_branch())
    if review_publication.APPROVAL_CLAIM.search(body):
        raise c.Refused(f"#{item['id']}: the comment would read as an approval claim; reword its extra")
    return body, state, head


def checked_item(c, item, lot, uuid):
    body, state, head = decline_body(c, item, lot)
    review_publication.require_review(c, item["id"], head, uuid)
    present = review_publication.exact_comments(c, item["id"], body, uuid)
    if state == "DECLINED" and not present:
        raise c.Refused(f"#{item['id']}: state DECLINED without the exact owned replacement comment")
    return body, state, head, present


def decline(c, plan_path, mode):
    if mode not in {"dry-run", "apply"}:
        raise c.Refused(f"mode must be dry-run or apply, not {mode!r}")
    lot_id, items = load_plan(c, plan_path)
    lot = merged_lot(c, lot_id)
    uuid = review_publication.account(c)
    c.say(f"lot #{lot_id} MERGED merge_commit {lot['merge']} head {lot['head']}")
    for item in items:
        body, state, head, present = checked_item(c, item, lot, uuid)
        c.say(f"#{item['id']} OK ({state}, lot #{lot_id} merged {lot['merge'][:8]}) -> {body}")
    if mode == "dry-run":
        c.say("dry-run: nothing written")
        return 0
    for item in items:
        if merged_lot(c, lot_id) != lot:
            raise c.Refused(f"lot #{lot_id} changed during decline")
        body, state, head, present = checked_item(c, item, lot, uuid)
        if state == "DECLINED":
            c.say(f"#{item['id']} already declined with exact owned replacement comment; resumed")
            continue
        if present:
            c.say(f"#{item['id']} comment already present ({present[0]}), not posted again")
        else:
            code = c.post_comment(item["id"], body)
            if code != 0 or not review_publication.exact_comments(c, item["id"], body, uuid):
                raise c.Failed(f"#{item['id']}: replacement comment write not observed; inspect before retry")
        fresh_body, fresh_state, fresh_head, fresh_present = checked_item(c, item, lot, uuid)
        if (fresh_body, fresh_state, fresh_head) != (body, "OPEN", head) or not fresh_present:
            raise c.Refused(f"#{item['id']}: changed before decline")
        c.api(f"pullrequests/{item['id']}/decline", "POST", timeout=120)
        after = c.api(f"pullrequests/{item['id']}?fields=state,source.commit.hash,destination.branch.name", timeout=60)
        c.say(f"#{item['id']} declined -> state {after.get('state')}")
        if after.get("state") != "DECLINED" or after.get("source", {}).get("commit", {}).get("hash") != head:
            raise c.Failed(f"#{item['id']}: decline write not observed at the expected head; inspect before retry")
    return 0
