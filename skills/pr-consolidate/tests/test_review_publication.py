import copy
import re
import unittest
from unittest import mock

from test_forge import BbLotCase, FakeBitbucket, forge


USER = "{11111111-2222-3333-4444-555555555555}"
OTHER = "{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}"
HEAD = "a" * 40


class ReviewBitbucket(FakeBitbucket):
    def __init__(self):
        super().__init__()
        self.user = USER
        self.ignore_write = False
        self.error = None
        self.add_pr(800, head=HEAD)

    def __call__(self, path, method="GET", payload=None, timeout=180):
        if path == "/user":
            return {"uuid": self.user}
        match = re.fullmatch(r"pullrequests/(\d+)/comments(?:/(\d+))?(?:\?.*)?", path)
        if not match:
            return super().__call__(path, method, payload, timeout)
        self.calls.append((method, path, copy.deepcopy(payload)))
        pid, cid = int(match[1]), int(match[2]) if match[2] else None
        values = self.comments.setdefault(pid, [])
        if method == "GET":
            return {"size": len(values), "values": copy.deepcopy(values)}
        if self.error:
            raise self.error
        if not self.ignore_write:
            if cid:
                comment = next(v for v in values if v["id"] == cid)
                comment["content"] = payload["content"]
            else:
                values.append(self.comment(len(values) + 1, payload["content"]["raw"]))
        return {}

    @staticmethod
    def comment(cid, body, user=USER, **fields):
        return {"id": cid, "content": {"raw": body}, "user": {"uuid": user}, **fields}


class ReviewCommentTest(BbLotCase):
    def setUp(self):
        super().setUp()
        self.bb = ReviewBitbucket()
        patcher = mock.patch.object(forge, "api", self.bb)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.body = self.write("review.md", "Findings: no defect established. Evidence: local tests. Limit: remote CI.")

    def review(self, head=HEAD, mode="apply"):
        try:
            return self.run_main("review-comment", 800, head, self.body, mode)
        except SystemExit as exc:
            return exc.code, "review-comment unavailable"

    def test_same_sha_creates_once_and_updates_own_general_comment(self):
        code, out = self.review()
        self.assertEqual(code, 0, out)
        self.assertEqual(len(self.bb.comments[800]), 1)
        raw = self.bb.comments[800][0]["content"]["raw"]
        self.assertIn(f"pr=800 head={HEAD}", raw)
        self.assertIn(HEAD, raw)
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(len(self.bb.writes()), 1)
        self.body.write_text("Corrected finding, checked locally.")
        self.assertEqual(self.review()[0], 0)
        self.assertEqual(len(self.bb.comments[800]), 1)
        self.assertIn("Corrected finding", self.bb.comments[800][0]["content"]["raw"])

    def test_new_sha_preserves_historical_comment(self):
        self.assertEqual(self.review()[0], 0)
        original = copy.deepcopy(self.bb.comments[800][0])
        self.bb.prs[800]["head"] = "b" * 40
        self.assertEqual(self.review(head="b" * 40)[0], 0)
        self.assertEqual(self.bb.comments[800][0], original)
        self.assertEqual(len(self.bb.comments[800]), 2)

    def test_quoted_or_ambiguous_marker_never_rewrites_historical_review(self):
        marker = f"<!-- pr-consolidate:review pr=800 head={HEAD} -->"
        old = f"<!-- pr-consolidate:review pr=800 head={'b' * 40} -->"
        for raw in (f"Citation :\n> {marker}", f"{old}\n\nCitation : {marker}",
                    f"{marker}\n\n{old}", f" {marker}", f"{marker}\n{marker}"):
            self.bb.comments[800] = [self.bb.comment(1, raw)]
            original = copy.deepcopy(self.bb.comments[800])
            code, out = self.review()
            self.assertEqual(code, 2, out)
            self.assertEqual(self.bb.comments[800], original)
            self.assertEqual(self.bb.writes(), [])

    def test_unchanged_or_dry_run_rechecks_head_after_reading_comments(self):
        self.assertEqual(self.review()[0], 0)
        original = self.bb.__call__
        self.bb.calls.clear()

        def moving(path, method="GET", payload=None, timeout=180):
            answer = original(path, method, payload, timeout)
            if method == "GET" and "/comments?" in path:
                self.bb.prs[800]["head"] = "b" * 40
            return answer

        for mode in ("apply", "dry-run"):
            self.bb.prs[800]["head"] = HEAD
            with mock.patch.object(forge, "api", moving):
                code, out = self.review(mode=mode)
            self.assertEqual(code, 2, out)
            self.assertEqual(self.bb.writes(), [])

    def test_dry_run_does_not_write(self):
        self.assertEqual(self.review(mode="dry-run")[0], 0)
        self.assertEqual(self.bb.writes(), [])

    def test_moved_or_abbreviated_head_refuses(self):
        for head in ("b" * 40, "a" * 12):
            self.assertEqual(self.review(head=head)[0], 2)
        self.assertEqual(self.bb.writes(), [])

    def test_empty_approval_reserved_or_unreadable_body_refuses(self):
        for body in ("", "APPROVED", "MERGE_READY", "<!-- pr-consolidate:review forged -->"):
            self.body.write_text(body)
            self.assertEqual(self.review()[0], 2)
        self.body.unlink()
        self.assertEqual(self.review()[0], 2)
        self.assertEqual(self.bb.writes(), [])

    def test_foreign_missing_author_reply_inline_and_duplicate_markers_refuse(self):
        marker = f"<!-- pr-consolidate:review pr=800 head={HEAD} -->"
        cases = [[self.bb.comment(1, marker, user=OTHER)], [self.bb.comment(1, marker, user=None)],
                 [self.bb.comment(1, marker, parent={"id": 99})],
                 [self.bb.comment(1, marker, inline={"to": 10})],
                 [self.bb.comment(1, marker, parent={})], [self.bb.comment(1, marker, inline={})],
                 [self.bb.comment(1, marker), self.bb.comment(2, marker)]]
        for comments in cases:
            self.bb.comments[800] = comments
            self.assertEqual(self.review()[0], 2)
        self.assertEqual(self.bb.writes(), [])

    def test_missing_connected_account_refuses(self):
        self.bb.user = None
        self.assertEqual(self.review()[0], 2)
        self.assertEqual(self.bb.writes(), [])

    def test_unobserved_write_and_transport_errors_fail(self):
        self.bb.ignore_write = True
        self.assertEqual(self.review()[0], 1)
        self.bb.error = forge.ForgeAPIError("unavailable")
        self.assertEqual(self.review()[0], 1)
        self.bb.error = forge.ForgeTimeout("timeout")
        self.assertEqual(self.review()[0], 3)

    def test_head_changes_after_publish_reports_refusal(self):
        original = self.bb.__call__

        def moving(path, method="GET", payload=None, timeout=180):
            answer = original(path, method, payload, timeout)
            if method == "POST":
                self.bb.prs[800]["head"] = "b" * 40
            return answer

        with mock.patch.object(forge, "api", moving):
            self.assertEqual(self.review()[0], 2)
        self.assertEqual(len(self.bb.comments[800]), 1)

    def test_later_page_marker_is_updated_without_duplicate(self):
        marker = f"<!-- pr-consolidate:review pr=800 head={HEAD} -->"
        self.bb.comments[800] = [self.bb.comment(1, "Unrelated"), self.bb.comment(2, marker)]
        original = self.bb.__call__

        def pages(path, method="GET", payload=None, timeout=180):
            if method == "GET" and "/comments?" in path:
                values = copy.deepcopy(self.bb.comments[800])
                if "page=2" in path:
                    return {"size": 2, "values": values[1:]}
                return {"size": 2, "values": values[:1], "next": "pullrequests/800/comments?page=2"}
            return original(path, method, payload, timeout)

        with mock.patch.object(forge, "api", pages):
            code, out = self.review()
        self.assertEqual(code, 0, out)
        self.assertEqual(len(self.bb.comments[800]), 2)
        self.assertIn("PUT", [write[0] for write in self.bb.writes()])

    def test_incomplete_looping_or_malformed_pagination_refuses(self):
        original = self.bb.__call__
        for page in ({"size": 1, "values": []}, {"size": 0, "values": [], "next": []},
                     {"size": 0, "values": [], "next": "pullrequests/800/comments?page=2"},
                     {"size": 1, "values": [None]}, {"size": 0, "values": "bad"}):
            def pages(path, method="GET", payload=None, timeout=180):
                return page if "/comments?" in path else original(path, method, payload, timeout)

            with mock.patch.object(forge, "api", pages):
                code, out = self.review()
            self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.writes(), [])

    def test_malformed_comment_content_refuses_before_write(self):
        for comment in ({"id": 1, "content": {"raw": 4}, "user": {"uuid": USER}},
                        {"id": 1, "content": {"raw": "text"}, "user": {"uuid": USER}, "deleted": "false"}):
            self.bb.comments[800] = [comment]
            code, out = self.review()
            self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.writes(), [])


if __name__ == "__main__":
    unittest.main()
