import re


def check_state(c, sha):
    data = c.api(f"checks/{sha}", timeout=60)
    checks = data.get("checks")
    if data.get("sha") != sha or not isinstance(checks, list) or any(not isinstance(check, dict) for check in checks):
        raise c.Refused("GitHub checks are malformed or belong to a different commit")
    if not checks:
        return "pending"
    statuses = []
    for check in checks:
        status, result = check.get("status"), check.get("conclusion")
        if status not in {"queued", "in_progress", "completed", "waiting", "pending", "requested"}:
            raise c.Refused("GitHub check has an unknown status")
        if status != "completed":
            statuses.append("pending")
        elif result == "success":
            statuses.append("passed")
        elif result in {"skipped", "neutral"}:
            statuses.append("skipped")
        elif result in {"failure", "error", "cancelled", "timed_out", "action_required", "startup_failure", "stale"}:
            statuses.append("failed")
        else:
            raise c.Refused("GitHub check has an unknown conclusion")
    c.say(f"checks on {sha[:12]}: {statuses.count('passed')} passed, {statuses.count('skipped')} skipped, "
          f"{statuses.count('failed')} failed, {statuses.count('pending')} pending")
    if "failed" in statuses:
        return "failed"
    if "pending" in statuses:
        return "pending"
    return "passed" if "passed" in statuses else "failed"


def wait_ci(c, sha, timeout=3600, poll=60):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise c.Refused("GitHub CI requires the full commit SHA")
    deadline = c._now() + timeout
    while c._now() < deadline:
        state = check_state(c, sha)
        if state != "pending":
            return 0 if state == "passed" else c.EXIT_FAILED
        c._sleep(poll)
    c.say(f"timeout: no successful completed checks for {sha} within {timeout}s")
    return c.EXIT_TIMEOUT


def watch(c, prs, timeout=5400, poll=60):
    pending, outcomes = set(prs), {}
    deadline = c._now() + timeout
    while pending and c._now() < deadline:
        for pr in sorted(pending):
            data = c.api(f"pullrequests/{pr}", timeout=60)
            c.require_destination(data, pr)
            if data.get("state") == "MERGED":
                c.say(f"#{pr} MERGED merge_commit={(data.get('merge_commit') or {}).get('hash')}")
                outcomes[pr] = "merged"
            elif data.get("state") != "OPEN":
                c.say(f"#{pr} closed without merge")
                outcomes[pr] = "left"
            else:
                head = ((data.get("source") or {}).get("commit") or {}).get("hash")
                if not isinstance(head, str) or not re.fullmatch(r"[0-9a-f]{40}", head):
                    raise c.Refused(f"#{pr} has no full head SHA")
                if check_state(c, head) == "failed":
                    outcomes[pr] = "left"
                    c.say(f"#{pr} has failing head checks")
        pending -= set(outcomes)
        if pending:
            c._sleep(poll)
    return c.EXIT_TIMEOUT if pending else 0 if all(value == "merged" for value in outcomes.values()) else c.EXIT_FAILED
