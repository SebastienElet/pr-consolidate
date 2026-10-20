import sys
import re


def classify_step(step):
    state = step.get("state") or {}
    result = (state.get("result") or {}).get("name")
    if state.get("name") == "COMPLETED" and result == "SUCCESSFUL":
        return "passed"
    if (state.get("name") == "COMPLETED" and result == "SKIPPED") or "NOT_RUN" in (state.get("name"), result):
        return "skipped"
    return "failed"


def wait_ci(c, sha, timeout=3600, poll=60):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise c.Refused("Bitbucket CI requires the full commit SHA")
    deadline, seen = c._now() + timeout, None
    while c._now() < deadline:
        try:
            runs = c.api(f"pipelines?sort=-created_on&pagelen=50&fields={c.RUN_FIELDS}", timeout=60).get("values") or []
            mine = [r for r in runs if ((r.get("target") or {}).get("commit") or {}).get("hash") == sha
                    and not ((r.get("target") or {}).get("ref_name") or "").startswith("bitbucket-merge-queue")]
            run = mine[0] if mine else None
            steps = None
            if run and run["state"]["name"] == "COMPLETED":
                steps = c.paged(f"pipelines/{run['build_number']}/steps?pagelen=100&fields=size,next,values.name,"
                              "values.state.name,values.state.result.name", timeout=60)
        except c.ForgeAPIError as exc:
            print(f"transient: {c.mask(exc)[:100]}", file=sys.stderr, flush=True)
            c._sleep(poll)
            continue
        if run and (run["build_number"], run["state"]["name"]) != seen:
            seen = (run["build_number"], run["state"]["name"])
            c.say(f"pipeline {run['build_number']} {run['state']['name']} on {sha[:12]}")
        if steps is not None:
            result = (run["state"].get("result") or {}).get("name")
            kinds = {}
            for step in steps:
                kind = classify_step(step)
                kinds.setdefault(kind, []).append(step.get("name"))
                state = step.get("state") or {}
                c.say(f"step {step.get('name')}: {state.get('name')}/{(state.get('result') or {}).get('name') or '-'}"
                    f" -> {kind}")
            failed = kinds.get("failed", [])
            c.say(f"pipeline {run['build_number']} {result} on {sha[:12]}: {len(kinds.get('passed', []))} passed, "
                f"skipped {kinds.get('skipped', [])}, failed {failed}")
            if result != "SUCCESSFUL":
                return c.EXIT_FAILED
            if failed:
                c.say(f"INCONSISTENT: pipeline {run['build_number']} is SUCCESSFUL but steps {failed} did not pass")
                return c.EXIT_FAILED
            if not kinds.get("passed"):
                c.say("no executed step succeeded; an empty or entirely skipped pipeline is not validation")
                return c.EXIT_FAILED
            return 0
        c._sleep(poll)
    c.say(f"timeout: no completed branch pipeline for {sha[:12]} within {timeout // 60} min")
    return c.EXIT_TIMEOUT


def watch(c, prs, timeout=5400, poll=60):
    pending, outcome = set(prs), {}
    deadline = c._now() + timeout
    while pending and c._now() < deadline:
        try:
            runs = c.api("pipelines?sort=-created_on&pagelen=50&fields=values.build_number,values.state.name,"
                       "values.state.result.name,values.target.ref_name", timeout=60).get("values") or []
        except c.ForgeAPIError as exc:
            print(f"transient: {c.mask(exc)[:100]}", file=sys.stderr, flush=True)
            c._sleep(poll)
            continue
        for pr in sorted(pending):
            try:
                data = c.api(f"pullrequests/{pr}?fields=state,merge_commit.hash,source.commit.hash,destination.branch.name", timeout=60)
            except c.ForgeAPIError as exc:
                print(f"transient #{pr}: {c.mask(exc)[:100]}", file=sys.stderr, flush=True)
                continue
            c.require_destination(data, pr)
            queue = [r for r in runs if (r.get("target") or {}).get("ref_name") == f"bitbucket-merge-queue-{pr}"]
            last = queue[0] if queue else None
            if data["state"] == "MERGED":
                c.say(f"#{pr} MERGED merge_commit={(data.get('merge_commit') or {}).get('hash')} "
                    f"queue_pipeline={last and last['build_number']}")
                outcome[pr] = "merged"
            elif data["state"] != "OPEN":
                c.say(f"#{pr} {data['state']}: left without merge")
                outcome[pr] = "left"
            elif last and last["state"]["name"] == "COMPLETED" and \
                    (last["state"].get("result") or {}).get("name") != "SUCCESSFUL":
                c.say(f"#{pr} LEFT THE QUEUE: pipeline {last['build_number']} "
                    f"{(last['state'].get('result') or {}).get('name')}")
                outcome[pr] = "left"
        pending -= set(outcome)
        if pending:
            c._sleep(poll)
    for pr in sorted(pending):
        c.say(f"#{pr} still pending after {timeout // 60} min")
    if pending:
        return c.EXIT_TIMEOUT
    return 0 if all(kind == "merged" for kind in outcome.values()) else c.EXIT_FAILED
