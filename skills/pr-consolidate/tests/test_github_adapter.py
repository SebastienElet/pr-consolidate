import copy
import json
import re
import unittest
from urllib.parse import parse_qs, urlsplit
from unittest import mock

from test_forge import BbLotCase, forge
import forge_transport


HEAD = "a" * 40
BATCH = "b" * 40
MERGE = "c" * 40


class GitHubFixture:
    def __init__(self):
        self.prs = {800: self.pull(800), 900: self.pull(900, head=BATCH, merged=True)}
        self.comments, self.reviews, self.calls, self.runs, self.statuses = {}, {}, [], [], []
        self.user, self.ancestor, self.after_comments = {"id": 7, "login": "maintainer"}, HEAD, None

    @staticmethod
    def pull(number, head=HEAD, merged=False):
        return {"id": number + 1000, "number": number, "title": "Batch fixture", "body": "Original description",
                "user": {"id": 9, "login": "author"}, "head": {"sha": head, "ref": f"feat/{number}"},
                "base": {"ref": "main"}, "state": "closed" if merged else "open", "draft": False,
                "merged": merged, "merged_at": "2026-01-01T00:00:00Z" if merged else None,
                "merge_commit_sha": MERGE if merged else None, "changed_files": 1}

    @staticmethod
    def comment(cid, body, user=7, **fields):
        return {"id": cid, "body": body, "user": {"id": user, "login": "maintainer"}, **fields}

    def __call__(self, path, method="GET", payload=None, timeout=180):
        self.calls.append((method, path, copy.deepcopy(payload)))
        parsed = urlsplit(path)
        route = parsed.path.removeprefix("/repos/workspace/project/")
        query = parse_qs(parsed.query)
        page = int(query.get("page", [1])[0])
        if path == "/user":
            return copy.deepcopy(self.user)
        if route == "pulls" and method == "GET":
            values = [p for p in self.prs.values() if p["state"] == "open"]
            return copy.deepcopy(values[(page - 1) * 100:page * 100])
        if route == "pulls" and method == "POST":
            pr = self.pull(901)
            pr.update(title=payload["title"], body=payload["body"])
            pr["head"]["ref"], pr["base"]["ref"] = payload["head"], payload["base"]
            self.prs[901] = pr
            return copy.deepcopy(pr)
        match = re.fullmatch(r"pulls/(\d+)", route)
        if match:
            pr = self.prs[int(match[1])]
            if method == "PATCH":
                pr.update(payload)
            return copy.deepcopy(pr)
        match = re.fullmatch(r"pulls/(\d+)/files", route)
        if match:
            return [{"sha": "d" * 40, "filename": "src/current.py", "previous_filename": "src/previous.py",
                     "status": "renamed", "additions": 3, "deletions": 1}]
        match = re.fullmatch(r"(issues|pulls)/(\d+)/(comments|reviews)(?:/(\d+))?", route)
        if match:
            number, surface = int(match[2]), match[3]
            values = (self.comments if surface == "comments" else self.reviews).setdefault(number, [])
            if method == "GET":
                result = copy.deepcopy(values[(page - 1) * 100:page * 100])
                if self.after_comments:
                    self.after_comments()
                return result
            if match[4]:
                next(v for v in values if v["id"] == int(match[4]))["body"] = payload["body"]
            else:
                values.append(self.comment(max([v["id"] for v in values], default=0) + 1, payload["body"]))
            return copy.deepcopy(values[-1])
        match = re.fullmatch(r"issues/comments/(\d+)", route)
        if match and method == "PATCH":
            comment = next(v for values in self.comments.values() for v in values if v["id"] == int(match[1]))
            comment["body"] = payload["body"]
            return copy.deepcopy(comment)
        if re.fullmatch(r"compare/[0-9a-f]+\.\.\.[0-9a-f]+", route):
            return {"merge_base_commit": {"sha": self.ancestor}}
        if re.fullmatch(r"commits/[0-9a-f]+/check-runs", route):
            return {"total_count": len(self.runs), "check_runs": copy.deepcopy(self.runs[(page - 1) * 100:page * 100])}
        if re.fullmatch(r"commits/[0-9a-f]+/statuses", route):
            return copy.deepcopy(self.statuses[(page - 1) * 100:page * 100])
        raise AssertionError(f"unexpected GitHub call {method} {path}")

    def writes(self):
        return [call for call in self.calls if call[0] != "GET"]


class GitHubCommandsTest(BbLotCase):
    def setUp(self):
        super().setUp()
        self.context["forge"] = "github"
        self.context["remote_url"] = "git@github.com:workspace/project.git"
        self.github = GitHubFixture()
        for target, name, value in ((forge, "api", forge_transport.api),
                                     (forge_transport, "github_request", self.github),
                                     (forge, "post_comment", lambda pr, body: forge.review_publication.post_comment(forge, pr, body))):
            patcher = mock.patch.object(target, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        self.body = self.write("review.md", "Findings: no defect established. Evidence: local checks. Limits: remote CI.")

    def review(self, mode="apply"):
        return self.run_main("review-comment", 800, HEAD, self.body, mode)

    def test_create_description_and_inventory_use_github_shapes(self):
        code, out = self.run_main("create", "feat/batch", "Batch", self.body)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.github.prs[901]["base"]["ref"], "main")
        expected = self.write("old.md", "Original description")
        code, out = self.run_main("desc", 800, expected, self.body)
        self.assertEqual(code, 0, out)
        self.assertIn("Findings:", self.github.prs[800]["body"])
        inventory = self.dir / "inventory.json"
        code, out = self.run_main("inventory", "--out", inventory)
        self.assertEqual(code, 0, out)
        data = json.loads(inventory.read_text())
        self.assertEqual(data["repository"], "workspace/project")
        self.assertEqual(data["prs"][0]["paths"], ["src/current.py", "src/previous.py"])
        self.assertEqual(data["prs"][0]["added"], 3)

    def test_review_creates_once_updates_owned_issue_comment_and_keeps_foreign_comments(self):
        self.github.comments[800] = [self.github.comment(10, "Other reviewer", user=20)]
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(len(self.github.comments[800]), 2)
        self.body.write_text("Updated assessment")
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(self.github.comments[800][0]["body"], "Other reviewer")
        self.assertEqual([c[0] for c in self.github.writes()], ["POST", "PATCH"])

    def test_owned_review_summary_updates_its_own_surface_without_native_approval(self):
        marker = forge.review_publication.review_marker(800, HEAD)
        self.github.reviews[800] = [self.github.comment(12, marker + "\n\nPrevious assessment", state="COMMENTED",
                                                     commit_id=HEAD)]
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(self.github.writes()[0][:2], ("PUT", "/repos/workspace/project/pulls/800/reviews/12"))
        self.assertEqual(self.github.reviews[800][0]["state"], "COMMENTED")
        self.assertEqual(self.github.comments[800], [])

    def test_stale_dismissed_or_pending_review_summary_and_boolean_author_refuse(self):
        marker = forge.review_publication.review_marker(800, HEAD)
        for state, commit in (("COMMENTED", "f" * 40), ("DISMISSED", HEAD), ("PENDING", HEAD)):
            self.github.reviews[800] = [self.github.comment(12, marker, state=state, commit_id=commit)]
            code, out = self.review()
            self.assertEqual(code, 2, out)
        self.github.reviews[800] = []
        self.github.user["id"] = 1
        self.github.comments[800] = [self.github.comment(1, marker, user=True)]
        self.assertEqual(self.review()[0], 2)
        self.assertEqual(self.github.writes(), [])

    def test_foreign_or_duplicate_marker_across_comment_surfaces_refuses_before_write(self):
        marker = forge.review_publication.review_marker(800, HEAD)
        for foreign in (True, False):
            self.github.comments[800] = [self.github.comment(1, marker)]
            self.github.reviews[800] = [self.github.comment(1, marker, user=20 if foreign else 7,
                                                         state="COMMENTED", commit_id=HEAD)]
            code, out = self.review()
            self.assertEqual(code, 2, out)
        self.assertEqual(self.github.writes(), [])

    def test_later_comment_page_is_updated_without_duplicate_and_incomplete_files_refuse(self):
        marker = forge.review_publication.review_marker(800, HEAD)
        self.github.comments[800] = [self.github.comment(i, f"Historical comment {i}") for i in range(1, 101)]
        self.github.comments[800].append(self.github.comment(101, marker + "\n\nPrevious assessment"))
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(len(self.github.comments[800]), 101)
        self.assertEqual(self.github.writes()[0][:2], ("PATCH", "/repos/workspace/project/issues/comments/101"))
        self.github.prs[800]["changed_files"] = 2
        path = self.dir / "incomplete.json"
        code, out = self.run_main("inventory", "--out", path)
        self.assertEqual(code, 1, out)
        self.assertFalse(path.exists())

    def test_unknown_publishing_account_or_moved_head_refuses_before_write(self):
        for identity in (None, "7", True):
            self.github.user["id"] = identity
            self.assertEqual(self.review()[0], 2)
        self.github.user["id"] = 7
        self.github.after_comments = lambda: self.github.prs[800]["head"].update(sha="d" * 40)
        self.assertEqual(self.review()[0], 2)
        self.assertEqual(self.github.writes(), [])

    def test_replacement_closes_only_after_owned_head_review_and_ancestry_and_resumes(self):
        marker = forge.review_publication.review_marker(800, HEAD)
        self.github.comments[800] = [self.github.comment(1, marker + "\n\nAssessment")]
        plan = self.write("replacement.json", json.dumps({"lot_pr": 900, "items": [{"id": 800, "head": HEAD}]}))
        code, out = self.run_main("replace", plan, "apply")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.github.prs[800]["state"], "closed")
        writes = copy.deepcopy(self.github.writes())
        self.assertEqual(self.run_main("replace", plan, "apply")[0], 0)
        self.assertEqual(self.github.writes(), writes)
        self.assertEqual([call[0] for call in writes], ["POST", "PATCH"])

    def test_unpublished_dismissed_or_unknown_review_summary_cannot_replace_public_comment(self):
        forge_transport.configure(self.context)
        self.github.comments[800] = [self.github.comment(1, forge.review_publication.review_marker(800, HEAD))]
        body = forge.decline_body({"id": 800, "head": HEAD, "extra": ""},
                                  {"id": 900, "head": BATCH, "merge": MERGE})
        plan = self.write("replacement.json", json.dumps({"lot_pr": 900, "items": [{"id": 800, "head": HEAD}]}))
        for state in ("PENDING", "DISMISSED", "UNKNOWN"):
            self.github.reviews[800] = [self.github.comment(12, body, state=state, commit_id=HEAD)]
            code, out = self.run_main("replace", plan, "apply")
            self.assertEqual(code, 2, out)
            self.assertEqual(self.github.prs[800]["state"], "open")
            self.assertEqual(self.github.writes(), [])

    def test_missing_review_wrong_ancestry_and_wrong_target_never_close(self):
        plan = self.write("replacement.json", json.dumps({"lot_pr": 900, "items": [{"id": 800, "head": HEAD}]}))
        self.assertEqual(self.run_main("replace", plan, "apply")[0], 2)
        self.github.comments[800] = [self.github.comment(1, forge.review_publication.review_marker(800, HEAD))]
        self.github.ancestor = "f" * 40
        self.assertEqual(self.run_main("replace", plan, "apply")[0], 2)
        self.github.ancestor = HEAD
        self.github.prs[900]["base"]["ref"] = "other"
        self.assertEqual(self.run_main("replace", plan, "apply")[0], 2)
        self.assertEqual(self.github.writes(), [])

    def test_checks_success_requires_one_real_success_and_full_exact_sha(self):
        self.github.runs = [{"id": 1, "name": "tests", "head_sha": HEAD, "status": "completed", "conclusion": "success"},
                            {"id": 2, "name": "optional", "head_sha": HEAD, "status": "completed", "conclusion": "skipped"}]
        code, out = self.run_main("wait-ci", HEAD)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.run_main("wait-ci", HEAD[:12])[0], 2)
        self.github.runs[0]["head_sha"] = "f" * 40
        self.assertEqual(self.run_main("wait-ci", HEAD)[0], 1)

    def test_absent_all_skipped_failing_and_unknown_checks_never_pass(self):
        self.assertEqual(self.run_main("wait-ci", HEAD)[0], 3)
        self.github.runs = [{"id": 1, "name": "tests", "head_sha": HEAD, "status": "completed", "conclusion": "skipped"}]
        self.assertEqual(self.run_main("wait-ci", HEAD)[0], 1)
        self.github.runs[0]["conclusion"] = "failure"
        self.assertEqual(self.run_main("wait-ci", HEAD)[0], 1)
        self.github.runs[0]["conclusion"] = "unknown_result"
        self.assertEqual(self.run_main("wait-ci", HEAD)[0], 2)

    def test_watch_distinguishes_merged_closed_pending_and_wrong_destination(self):
        self.assertEqual(self.run_main("watch", 900)[0], 0)
        self.github.prs[800]["state"] = "closed"
        self.assertEqual(self.run_main("watch", 800)[0], 1)
        self.github.prs[800]["state"] = "open"
        self.assertEqual(self.run_main("watch", 800)[0], 3)
        self.github.prs[900]["base"]["ref"] = "other"
        self.assertEqual(self.run_main("watch", 900)[0], 2)

    def test_unrecognized_github_payload_refuses_and_api_failure_never_becomes_empty_success(self):
        self.github.prs[900] = {"source": {}, "state": "OPEN"}
        code, out = self.run_main("watch", 900)
        self.assertEqual(code, 1, out)
        with mock.patch.object(forge_transport, "github_request", side_effect=forge_transport.ForgeAPIError("unavailable")):
            self.assertEqual(self.run_main("wait-ci", HEAD)[0], 1)
        self.assertEqual(self.github.writes(), [])


if __name__ == "__main__":
    unittest.main()
