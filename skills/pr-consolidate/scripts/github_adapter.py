import re
from urllib.parse import urlsplit


class GitHubAdapter:
    def __init__(self, repository, request, error):
        self.repository, self.request, self.error = repository, request, error
        self.root = f"/repos/{repository}/"

    def object(self, path, method="GET", payload=None, timeout=180):
        value = self.request(path, method, payload, timeout)
        if not isinstance(value, dict):
            raise self.error("GitHub returned a non-object response")
        return value

    def collection(self, path, key=None, timeout=180, identity_key="id"):
        values, seen, total = [], set(), None
        separator = "&" if "?" in path else "?"
        for page in range(1, 501):
            data = self.request(f"{path}{separator}per_page=100&page={page}", "GET", None, timeout)
            batch = data.get(key) if key and isinstance(data, dict) else data
            if not isinstance(batch, list) or any(not isinstance(item, dict) for item in batch):
                raise self.error("GitHub returned an invalid collection")
            if key:
                current = data.get("total_count")
                if type(current) is not int or current < 0 or (total is not None and current != total):
                    raise self.error("GitHub collection count is invalid or changed")
                total = current
            for item in batch:
                identity = item.get(identity_key)
                if (identity_key == "id" and (type(identity) is not int or identity <= 0)) or \
                        (identity_key != "id" and (not isinstance(identity, str) or not identity)) or identity in seen:
                    raise self.error("GitHub collection has missing or repeated IDs")
                seen.add(identity)
            values.extend(batch)
            if len(batch) < 100:
                if key and (type(data.get("total_count")) is not int or len(values) != data["total_count"]):
                    raise self.error("GitHub collection count is incomplete")
                return values
        raise self.error("GitHub pagination exceeded its bound")

    def pull(self, value):
        try:
            number, head, base, user = value["number"], value["head"], value["base"], value["user"]
            if type(number) is not int or number <= 0 or value["state"] not in {"open", "closed"} or \
                    not re.fullmatch(r"[0-9a-f]{40}", head["sha"]) or \
                    not all(isinstance(branch, str) and branch for branch in (head["ref"], base["ref"])) or \
                    not isinstance(value["title"], str) or not isinstance(user["login"], str) or \
                    (value.get("body") is not None and not isinstance(value["body"], str)):
                raise ValueError()
            merged = value.get("merged") is True or bool(value.get("merged_at"))
            return {"id": number, "state": "MERGED" if merged else "OPEN" if value["state"] == "open" else "DECLINED",
                    "title": value["title"], "description": value.get("body") or "",
                    "author": {"display_name": user["login"]}, "draft": value.get("draft", False),
                    "source": {"branch": {"name": head["ref"]}, "commit": {"hash": head["sha"]}},
                    "destination": {"branch": {"name": base["ref"]}},
                    "merge_commit": {"hash": value.get("merge_commit_sha")} if merged else None,
                    "created_on": value.get("created_at", ""), "updated_on": value.get("updated_at", "")}
        except (KeyError, TypeError, ValueError) as exc:
            raise self.error("GitHub returned an invalid pull request") from exc

    def reviews(self, number, timeout):
        return self.collection(self.root + f"pulls/{number}/reviews", timeout=timeout)

    def participants(self, number, timeout):
        latest = {}
        for review in self.reviews(number, timeout):
            user, state = review.get("user"), review.get("state")
            if not isinstance(user, dict) or type(user.get("id")) is not int or \
                    not isinstance(user.get("login"), str) or \
                    state not in {"APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING"}:
                raise self.error("GitHub returned an invalid review identity or state")
            if state != "COMMENTED" and state != "PENDING":
                latest[user["id"]] = {"user": {"display_name": user["login"]},
                                      "state": state.lower(), "approved": state == "APPROVED"}
        return list(latest.values())

    def checks(self, sha, timeout):
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise self.error("GitHub checks require the complete commit SHA")
        runs = self.collection(self.root + f"commits/{sha}/check-runs?filter=latest", "check_runs", timeout)
        statuses = self.collection(self.root + f"commits/{sha}/statuses", timeout=timeout)
        result, latest = [], set()
        for run in runs:
            if run.get("head_sha") != sha or not isinstance(run.get("name"), str) or \
                    run.get("status") not in {"queued", "in_progress", "completed", "waiting", "pending", "requested"}:
                raise self.error("GitHub returned checks for an unknown state or different commit")
            result.append({"name": run["name"], "status": run["status"], "conclusion": run.get("conclusion")})
        for status in statuses:
            name, state = status.get("context"), status.get("state")
            if not isinstance(name, str) or state not in {"pending", "success", "error", "failure"}:
                raise self.error("GitHub returned an invalid commit status")
            if name not in latest:
                latest.add(name)
                result.append({"name": name, "status": "pending" if state == "pending" else "completed",
                               "conclusion": "success" if state == "success" else state if state != "pending" else None})
        return {"sha": sha, "checks": result}

    def comments(self, number, timeout):
        result = []
        for surface, values in (("issue", self.collection(self.root + f"issues/{number}/comments", timeout=timeout)),
                                ("review", self.reviews(number, timeout))):
            for value in values:
                if not isinstance(value.get("body"), str) or not isinstance(value.get("user"), dict):
                    raise self.error("GitHub returned invalid comment content or identity")
                result.append({"id": value["id"], "content": {"raw": value["body"]}, "user": value["user"],
                               "_surface": surface, "review_state": value.get("state"),
                               "commit_id": value.get("commit_id")})
        return {"size": len(result), "values": result}

    def files(self, number, timeout):
        pull = self.object(self.root + f"pulls/{number}", timeout=timeout)
        values = self.collection(self.root + f"pulls/{number}/files", timeout=timeout, identity_key="filename")
        count = pull.get("changed_files")
        if type(count) is not int or len(values) != count or count > 3000:
            raise self.error("GitHub file listing is incomplete or exceeds the supported API limit")
        result = []
        for value in values:
            if not isinstance(value.get("filename"), str) or \
                    any(type(value.get(key)) is not int or value[key] < 0 for key in ("additions", "deletions")):
                raise self.error("GitHub returned an invalid changed file")
            result.append({"new": {"path": value["filename"]},
                           "old": {"path": value.get("previous_filename", value["filename"])},
                           "lines_added": value["additions"], "lines_removed": value["deletions"]})
        return {"size": len(result), "values": result}

    def api(self, path, method="GET", payload=None, timeout=180):
        parsed = urlsplit(path)
        if path == "/user" and method == "GET":
            return self.object("/user", timeout=timeout)
        if parsed.scheme or parsed.netloc or path.startswith("/") or \
                re.search(r"[\\%\x00-\x20\x7f]", parsed.path) or ".." in parsed.path.split("/"):
            raise self.error("invalid GitHub consolidation endpoint")
        endpoint = parsed.path
        if endpoint.startswith("checks/") and method == "GET":
            return self.checks(endpoint.removeprefix("checks/"), timeout)
        if endpoint == "pullrequests":
            if method == "POST":
                return self.pull(self.object(self.root + "pulls", "POST", {
                    "title": payload["title"], "body": payload["description"],
                    "head": payload["source"]["branch"]["name"], "base": payload["destination"]["branch"]["name"],
                }, timeout))
            values = self.collection(self.root + "pulls?state=open", timeout=timeout)
            branch = re.search(r'source\.branch\.name="([A-Za-z0-9._/-]+)"', parsed.query)
            if branch:
                values = [value for value in values if value.get("head", {}).get("ref") == branch[1]]
            records = [self.pull(value) for value in values]
            if not branch:
                for record in records:
                    record["participants"] = self.participants(record["id"], timeout)
            return {"size": len(records), "values": records}
        match = re.fullmatch(r"pullrequests/(\d+)(?:/(.*))?", endpoint)
        if match:
            number, suffix = int(match[1]), match[2]
            if suffix is None:
                data = None if payload is None else {"title": payload["title"], "body": payload["description"]}
                return self.pull(self.object(self.root + f"pulls/{number}", "PATCH" if method == "PUT" else method,
                                             data, timeout))
            if suffix == "diffstat" and method == "GET":
                return self.files(number, timeout)
            if suffix == "statuses" and method == "GET":
                pr = self.pull(self.object(self.root + f"pulls/{number}", timeout=timeout))
                checks = self.checks(pr["source"]["commit"]["hash"], timeout)["checks"]
                return {"size": len(checks), "values": [{"state": check["conclusion"] or check["status"]}
                                                        for check in checks]}
            if suffix == "comments" and method == "GET":
                return self.comments(number, timeout)
            if suffix == "decline" and method == "POST":
                return self.pull(self.object(self.root + f"pulls/{number}", "PATCH", {"state": "closed"}, timeout))
            comment = re.fullmatch(r"(comments|reviews)(?:/(\d+))?", suffix)
            if comment and method in {"POST", "PUT"}:
                if comment[2]:
                    target = (self.root + f"issues/comments/{comment[2]}" if comment[1] == "comments"
                              else self.root + f"pulls/{number}/reviews/{comment[2]}")
                    verb = "PATCH" if comment[1] == "comments" else "PUT"
                else:
                    target, verb = self.root + f"issues/{number}/comments", "POST"
                return self.object(target, verb, {"body": payload["content"]["raw"]}, timeout)
        match = re.fullmatch(r"merge-base/([0-9a-f]{40})\.\.([0-9a-f]{12,40})", endpoint)
        if match and method == "GET":
            value = self.object(self.root + f"compare/{match[1]}...{match[2]}", timeout=timeout)
            return {"hash": (value.get("merge_base_commit") or {}).get("sha")}
        raise self.error("unsupported GitHub consolidation operation")
