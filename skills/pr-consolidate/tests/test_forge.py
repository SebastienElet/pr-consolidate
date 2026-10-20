"""Unit tests of scripts/forge.py: a fake Bitbucket replaces bb.api, verify-merge runs on temporary git repositories.

Usage: python3 -m unittest discover -s tests -p "test_forge.py" -v   (from the skill directory)
Writes only temporary directories; never reaches Bitbucket, the lot clones or the push clone.
"""
import contextlib
import copy
import io
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "scripts"))
import forge  # noqa: E402


class FakeBitbucket:
    """Answers the bb.api calls forge makes from an in-memory repository state; records every call."""

    def __init__(self):
        self.prs, self.comments, self.ancestors, self.routes, self.calls = {}, {}, set(), [], []

    def add_pr(self, pid, state="OPEN", head="a" * 12, merge=None, branch=None, destination="main",
               description="", title="t"):
        self.prs[pid] = {"state": state, "head": head, "merge": merge, "branch": branch or f"feat/{pid}",
                         "destination": destination, "description": description, "title": title}

    def view(self, pid):
        pr = self.prs[pid]
        return {"id": pid, "state": pr["state"], "title": pr["title"], "description": pr["description"],
                "source": {"commit": {"hash": pr["head"]}, "branch": {"name": pr["branch"]}},
                "destination": {"branch": {"name": pr["destination"]}},
                "merge_commit": {"hash": pr["merge"]} if pr["merge"] else None}

    def __call__(self, path, method="GET", payload=None, timeout=180):
        self.calls.append((method, path, copy.deepcopy(payload)))
        if path == "/user" and method == "GET":
            return {"uuid": "{11111111-2222-3333-4444-555555555555}"}
        for verb, pattern, answer in self.routes:
            match = re.fullmatch(pattern, path)
            if verb == method and match:
                return copy.deepcopy(answer(match, payload) if callable(answer) else answer)
        match = re.fullmatch(r"pullrequests/(\d+)(\?fields=.*)?", path)
        if match and method == "GET":
            return self.view(int(match[1]))
        if match and method == "PUT":
            self.prs[int(match[1])]["description"] = payload["description"]
            return self.view(int(match[1]))
        match = re.fullmatch(r"pullrequests/(\d+)/comments\?.*", path)
        if match and method == "GET":
            raws = self.comments.get(int(match[1]), [])
            return {"size": len(raws), "values": [{"id": n, "deleted": False,
                                                   "user": {"uuid": "{11111111-2222-3333-4444-555555555555}"},
                                                   "content": {"raw": raw}}
                                                  for n, raw in enumerate(raws, 1)]}
        match = re.fullmatch(r"pullrequests/(\d+)/decline", path)
        if match and method == "POST":
            self.prs[int(match[1])]["state"] = "DECLINED"
            return {}
        match = re.fullmatch(r"merge-base/([0-9a-f]+)\.\.([0-9a-f]+)", path)
        if match and method == "GET":
            return {"hash": match[1] if (match[1], match[2]) in self.ancestors else "f" * 40}
        raise AssertionError(f"unexpected call {method} {path}")

    def writes(self):
        return [call for call in self.calls if call[0] != "GET"]


class BbLotCase(unittest.TestCase):
    def setUp(self):
        self.bb = FakeBitbucket()
        self.posted = []
        self.clock = [0.0]

        def post_comment(pr, body):
            self.posted.append((pr, body))
            self.bb.comments.setdefault(pr, []).append(body)
            return 0

        def now():
            self.clock[0] += 60
            return self.clock[0]

        for name, value in (("api", self.bb), ("post_comment", post_comment), ("_sleep", lambda seconds: None),
                            ("_now", now)):
            patcher = mock.patch.object(forge, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        self.tmp = tempfile.TemporaryDirectory(prefix="bb-lot-test-", ignore_cleanup_errors=True)
        self.addCleanup(self.tmp.cleanup)
        self.dir = pathlib.Path(self.tmp.name)
        self.context = {"root": str(self.dir), "repository": "workspace/project", "forge": "bitbucket", "base_branch": "main",
                        "remote_url": "https://bitbucket.org/workspace/project.git"}
        for target, name, value in ((forge.git_context, "discover", lambda directory: self.context),
                                     (forge, "_context", self.context)):
            patcher = mock.patch.object(target, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_main(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = forge.main([str(arg) for arg in argv])
        return code, out.getvalue()

    def write(self, name, text):
        path = self.dir / name
        path.write_text(text, encoding="utf-8")
        return path


def run(number, state, result=None, ref=None, commit="c" * 40):
    target = {"commit": {"hash": commit}}
    if ref:
        target["ref_name"] = ref
    return {"build_number": number, "target": target,
            "state": {"name": state, **({"result": {"name": result}} if result else {})}}


def step(name, state, result=None):
    return {"name": name, "state": {"name": state, **({"result": {"name": result}} if result else {})}}


class WaitCiTest(BbLotCase):
    def pipelines(self, *rounds):
        answers = iter(rounds)
        last = [None]

        def answer(match, payload):
            last[0] = next(answers, last[0])
            return {"values": last[0]}
        self.bb.routes.append(("GET", r"pipelines\?sort=-created_on.*", answer))

    def test_skipped_step_with_successful_pipeline_exits_0(self):
        self.pipelines([run(9, "IN_PROGRESS")],
                       [run(10, "COMPLETED", "FAILED", ref="bitbucket-merge-queue-958"),
                        run(9, "COMPLETED", "SUCCESSFUL", ref="feat/lot")])
        self.bb.routes.append(("GET", r"pipelines/9/steps\?.*", {"size": 3, "values": [
            step("turbo-build", "COMPLETED", "SUCCESSFUL"), step("api-image", "COMPLETED", "SKIPPED"),
            step("helm", "NOT_RUN")]}))
        code, out = self.run_main("wait-ci", "c" * 40)
        self.assertEqual(code, 0, out)
        self.assertIn("step api-image: COMPLETED/SKIPPED -> skipped", out)
        self.assertIn("step helm: NOT_RUN/- -> skipped", out)
        self.assertIn("pipeline 9 SUCCESSFUL on cccccccccccc: 1 passed, skipped ['api-image', 'helm'], failed []", out)
        self.assertNotIn("pipeline 10", out)

    def test_failed_step_exits_1(self):
        self.pipelines([run(9, "COMPLETED", "FAILED")])
        self.bb.routes.append(("GET", r"pipelines/9/steps\?.*", {"size": 2, "values": [
            step("turbo-build", "COMPLETED", "SUCCESSFUL"), step("turbo-test", "COMPLETED", "FAILED")]}))
        code, out = self.run_main("wait-ci", "c" * 40)
        self.assertEqual(code, 1, out)
        self.assertIn("step turbo-test: COMPLETED/FAILED -> failed", out)
        self.assertIn("failed ['turbo-test']", out)

    def test_successful_pipeline_with_failed_step_is_not_green(self):
        self.pipelines([run(9, "COMPLETED", "SUCCESSFUL")])
        self.bb.routes.append(("GET", r"pipelines/9/steps\?.*", {"size": 1, "values": [
            step("e2e", "COMPLETED", "ERROR")]}))
        code, out = self.run_main("wait-ci", "c" * 40)
        self.assertEqual(code, 1, out)
        self.assertIn("INCONSISTENT", out)

    def test_only_a_merge_queue_pipeline_times_out(self):
        self.pipelines([run(10, "COMPLETED", "SUCCESSFUL", ref="bitbucket-merge-queue-958")])
        code, out = self.run_main("wait-ci", "c" * 40)
        self.assertEqual(code, 3, out)
        self.assertIn("timeout", out)

    def test_successful_pipeline_without_an_executed_success_never_passes(self):
        for values in ([], [step("optional", "COMPLETED", "SKIPPED")], [step("optional", "NOT_RUN")]):
            self.bb.routes.clear()
            self.pipelines([run(9, "COMPLETED", "SUCCESSFUL")])
            self.bb.routes.append(("GET", r"pipelines/9/steps\?.*", {"size": len(values), "values": values}))
            code, out = self.run_main("wait-ci", "c" * 40)
            self.assertEqual(code, 1, out)

    def test_ci_rejects_short_or_invalid_sha_and_does_not_match_a_longer_hash(self):
        for sha in ("invalid", "c" * 12):
            code, out = self.run_main("wait-ci", sha)
            self.assertEqual(code, 2, out)
        self.pipelines([run(9, "COMPLETED", "SUCCESSFUL", commit="c" * 40 + "a")])
        code, out = self.run_main("wait-ci", "c" * 40)
        self.assertEqual(code, 3, out)


class WatchTest(BbLotCase):
    def test_watching_foreign_destination_never_reports_merge_success(self):
        self.bb.add_pr(958, state="MERGED", merge="1" * 12, destination="other")
        self.bb.routes.append(("GET", r"pipelines\?sort=-created_on.*", {"values": []}))
        code, out = self.run_main("watch", 958)
        self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.writes(), [])

    def test_merged_and_left_queue(self):
        self.bb.add_pr(958, state="MERGED", merge="1" * 12)
        self.bb.add_pr(959)
        self.bb.routes.append(("GET", r"pipelines\?sort=-created_on.*", {"values": [
            run(11, "COMPLETED", "FAILED", ref="bitbucket-merge-queue-959")]}))
        code, out = self.run_main("watch", 958, 959)
        self.assertEqual(code, 1, out)
        self.assertIn("#958 MERGED merge_commit=111111111111", out)
        self.assertIn("#959 LEFT THE QUEUE: pipeline 11 FAILED", out)


class DeclineTest(BbLotCase):
    LOT_HEAD, MERGE = "3ab97aa2ba83", "15509fd75c43"

    def setUp(self):
        super().setUp()
        self.bb.add_pr(958, state="MERGED", head=self.LOT_HEAD, merge=self.MERGE)
        for pid, head in ((800, "98e9bde6cdcc"), (952, "5abe8c73526d")):
            head += "0" * 28
            self.bb.add_pr(pid, head=head)
            self.bb.ancestors.add((head, self.LOT_HEAD))
            self.bb.comments[pid] = [f"<!-- pr-consolidate:review pr={pid} head={head} -->\n\nReview source."]

    def plan(self, items=None, lot=958):
        items = items if items is not None else [{"id": 800, "head": "98e9bde6cdcc"},
                                                 {"id": 952, "head": "5abe8c73526d", "extra": "Follow-up documented."}]
        return self.write("plan.json", json.dumps({"lot_pr": lot, "items": items}))

    def assert_nothing_written(self):
        self.assertEqual(self.bb.writes(), [])
        self.assertEqual(self.posted, [])

    def test_dry_run_writes_nothing(self):
        code, out = self.run_main("decline", self.plan(), "dry-run")
        self.assertEqual(code, 0, out)
        self.assert_nothing_written()
        self.assertIn("Included in batch #958 (merged into `main`, `15509fd7`): this PR head `98e9bde6`", out)
        self.assertIn("Follow-up documented. Closed as replaced by #958.", out)
        self.assertIn("dry-run: nothing written", out)

    def test_apply_comments_then_declines_each(self):
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 0, out)
        self.assertEqual([pr for pr, _ in self.posted], [800, 952])
        self.assertEqual([c[1] for c in self.bb.writes()], ["pullrequests/800/decline", "pullrequests/952/decline"])
        self.assertEqual(self.bb.prs[800]["state"], "DECLINED")
        self.assertIn("#952 declined -> state DECLINED", out)

    def test_apply_does_not_repost_a_present_comment(self):
        body = forge.decline_body({"id": 800, "head": "98e9bde6cdcc", "extra": ""},
                                   {"id": 958, "merge": self.MERGE, "head": self.LOT_HEAD})
        self.bb.comments[800].append(body)
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 0, out)
        self.assertEqual([pr for pr, _ in self.posted], [952])
        self.assertIn("#800 comment already present", out)

    def test_resume_after_first_pr_was_declined(self):
        body = forge.decline_body({"id": 800, "head": "98e9bde6cdcc", "extra": ""},
                                   {"id": 958, "merge": self.MERGE, "head": self.LOT_HEAD})
        self.bb.comments[800].append(body)
        self.bb.prs[800]["state"] = "DECLINED"
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 0, out)
        self.assertEqual([pr for pr, _ in self.posted], [952])
        self.assertEqual(self.bb.prs[952]["state"], "DECLINED")

    def test_full_planned_head_is_not_truncated_to_twelve_characters(self):
        old = "98e9bde6cdcc" + "a" * 28
        changed = "98e9bde6cdcc" + "b" * 28
        self.bb.prs[800]["head"] = changed
        self.bb.ancestors.add((changed, self.LOT_HEAD))
        code, out = self.run_main("decline", self.plan([{"id": 800, "head": old}]), "apply")
        self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_resume_refuses_foreign_or_ambiguous_replacement_comment(self):
        body = forge.decline_body({"id": 800, "head": "98e9bde6cdcc", "extra": ""},
                                   {"id": 958, "merge": self.MERGE, "head": self.LOT_HEAD})
        self.bb.prs[800]["state"] = "DECLINED"
        for values in ([{"id": 1, "content": {"raw": body}, "user": {"uuid": "foreign"}}],
                       [{"id": 1, "content": {"raw": body}}]):
            self.bb.routes[:] = [("GET", r"pullrequests/800/comments\?.*", {"size": len(values), "values": values})]
            code, out = self.run_main("decline", self.plan(), "apply")
            self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_destination_other_than_main_refuses(self):
        self.bb.prs[958]["destination"] = "other"
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_source_target_other_than_primary_refuses_before_any_write(self):
        self.bb.prs[800]["destination"] = "other"
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_post_without_observed_comment_does_not_decline(self):
        with mock.patch.object(forge, "post_comment", return_value=0):
            code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 1, out)
        self.assert_nothing_written()

    def test_head_moves_after_comment_does_not_decline(self):
        def moving(pr, body):
            self.bb.comments.setdefault(pr, []).append(body)
            self.bb.prs[pr]["head"] = "c" * 40
            return 0

        with mock.patch.object(forge, "post_comment", moving):
            code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.writes(), [])

    def test_lot_not_merged_refuses(self):
        for state, merge in (("OPEN", None), ("SUPERSEDED", self.MERGE)):
            self.bb.prs[958].update(state=state, merge=merge)
            code, out = self.run_main("decline", self.plan(), "apply")
            self.assertEqual(code, 2, out)
            self.assertIn(f"REFUSED: lot #958 is {state}, not MERGED", out)
        self.assert_nothing_written()

    def test_item_not_open_refuses_before_any_write(self):
        self.bb.prs[952]["state"] = "DECLINED"
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assertIn("REFUSED: #952: state DECLINED", out)
        self.assert_nothing_written()

    def test_head_moved_refuses(self):
        self.bb.prs[800]["head"] = "0123456789ab"
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assertIn("REFUSED: #800: state OPEN head 0123456789ab (expected OPEN 98e9bde6cdcc)", out)
        self.assert_nothing_written()

    def test_head_not_ancestor_of_lot_refuses(self):
        self.bb.ancestors.discard(("5abe8c73526d" + "0" * 28, self.LOT_HEAD))
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assertIn("is not an ancestor of the lot head", out)
        self.assert_nothing_written()

    def test_malformed_or_abbreviated_merge_base_refuses_before_write(self):
        head = self.bb.prs[800]["head"]
        for value in (head + "-NOT-A-COMMIT", head + "0", head[:12], "f" * 40, None, 42):
            self.bb.routes[:] = [("GET", r"merge-base/.*", {"hash": value})]
            code, out = self.run_main("decline", self.plan([{"id": 800, "head": head}]), "apply")
            self.assertEqual(code, 2, out)
            self.assert_nothing_written()

    def test_missing_source_review_refuses_before_any_write(self):
        self.bb.comments[952] = []
        code, out = self.run_main("decline", self.plan(), "apply")
        self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_wrong_sha_foreign_deleted_inline_or_replacement_only_review_refuses(self):
        head = self.bb.prs[800]["head"]
        marker = f"<!-- pr-consolidate:review pr=800 head={head} -->"
        owned = {"uuid": "{11111111-2222-3333-4444-555555555555}"}
        cases = [{"content": {"raw": marker.replace(head, "f" * 40)}, "user": owned},
                 {"content": {"raw": marker.replace("pr=800", "pr=801")}, "user": owned},
                 {"content": {"raw": marker}, "user": {"uuid": "foreign"}},
                 {"content": {"raw": marker}, "user": owned, "deleted": True},
                 {"content": {"raw": marker}, "user": owned, "inline": {"to": 4}},
                 {"content": {"raw": "Closed as replaced by #958."}, "user": owned}]
        for comment in cases:
            comment["id"] = 1
            self.bb.routes[:] = [("GET", r"pullrequests/800/comments\?.*", {"size": 1, "values": [comment]})]
            for mode in ("dry-run", "apply"):
                code, out = self.run_main("decline", self.plan(), mode)
                self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_approval_word_in_extra_refuses(self):
        for extra in ("Approved by the lead.", "The batch is MERGE_READY."):
            code, out = self.run_main("decline", self.plan([{"id": 800, "head": "98e9bde6cdcc", "extra": extra}]),
                                      "apply")
            self.assertEqual(code, 2, out)
            self.assertIn("approval claim", out)
        self.assert_nothing_written()

    def test_bad_plans_refuse(self):
        for items, lot in (([{"id": 800, "head": "98e9bde6"}], 958), ([{"id": 958, "head": "3ab97aa2ba83"}], 958),
                           ([], 958), ([{"id": 800, "head": "98e9bde6cdcc", "body": "x"}], 958)):
            code, out = self.run_main("decline", self.plan(items, lot), "apply")
            self.assertEqual(code, 2, out)
        self.assert_nothing_written()

    def test_mode_must_be_explicit(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            forge.main(["decline", str(self.plan()), "go"])
        self.assert_nothing_written()


class DescTest(BbLotCase):
    def setUp(self):
        super().setUp()
        self.bb.add_pr(958, description="Description v1\n")
        self.expected = self.write("v1.md", "Description v1")
        self.new = self.write("v2.md", "Description v2")

    def test_changed_description_refuses(self):
        self.bb.prs[958]["description"] = "Edited by a reviewer"
        code, out = self.run_main("desc", 958, self.expected, self.new)
        self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.writes(), [])

    def test_check_writes_nothing(self):
        code, out = self.run_main("desc", 958, self.expected, self.new, "--check")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.bb.writes(), [])

    def test_unchanged_description_is_replaced(self):
        code, out = self.run_main("desc", 958, self.expected, self.new)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.bb.writes(), [("PUT", "pullrequests/958", {"title": "t", "description": "Description v2"})])
        self.assertIn("description now matches v2.md: True", out)


class CreateTest(BbLotCase):
    def setUp(self):
        super().setUp()
        self.description = self.write("lot.md", "Batch of changes")
        self.open = []
        self.bb.routes.append(("GET", r'pullrequests\?q=source\.branch\.name="feat/lot" AND state="OPEN".*',
                               lambda m, p: {"size": len(self.open), "values": [{"id": i} for i in self.open]}))
        self.bb.routes.append(("POST", r"pullrequests", lambda m, p: {
            "id": 960, "close_source_branch": p["close_source_branch"],
            "source": {"branch": p["source"]["branch"], "commit": {"hash": "abcdef123456"}},
            "destination": p["destination"]}))

    def test_open_pr_on_branch_refuses(self):
        self.open = [958]
        code, out = self.run_main("create", "feat/lot", "Feat: lot", self.description)
        self.assertEqual(code, 2, out)
        self.assertIn("an open PR already exists for feat/lot: [958]", out)
        self.assertEqual(self.bb.writes(), [])

    def test_incomplete_existing_pr_listing_refuses_before_creating_a_duplicate(self):
        for response in ({"size": 1}, {"size": 1, "values": []}, {"size": False, "values": []},
                         {"size": 0, "values": [], "next": "pullrequests?page=2"}):
            self.bb.routes[0] = ("GET", r"pullrequests\?.*", response)
            code, out = self.run_main("create", "feat/lot", "Batch", self.description)
            self.assertEqual(code, 2, out)
            self.assertEqual(self.bb.writes(), [])

    def test_creates_to_main_keeping_the_branch(self):
        code, out = self.run_main("create", "feat/lot", "Feat: lot", self.description)
        self.assertEqual(code, 0, out)
        (method, path, payload), = self.bb.writes()
        self.assertEqual((method, path), ("POST", "pullrequests"))
        self.assertEqual(payload["destination"], {"branch": {"name": "main"}})
        self.assertIs(payload["close_source_branch"], False)
        self.assertIn("created #960 abcdef123456 feat/lot -> main", out)

    def test_bad_branch_refuses(self):
        code, out = self.run_main("create", 'feat/x" OR state="MERGED', "Feat: lot", self.description)
        self.assertEqual(code, 2, out)
        self.assertEqual(self.bb.calls, [])


class InventoryTest(BbLotCase):
    PATHS = {
        1: ["src/core/a.ts", "pnpm-lock.yaml", "yarn.lock"],
        2: ["src/core/a.ts", "src/core/b.ts"],
        3: ["pnpm-lock.yaml", "package-lock.json", "yarn.lock", "Cargo.lock", "tooling/c.ts"],
        4: ["src/core/a.ts"],
        5: ["src/core/b.ts"],
        6: ["src/core/b.ts", "apps/f.ts"],
        7: ["apps/g.ts"],
        8: ["apps/g.ts"],
    }

    def setUp(self):
        super().setUp()
        self.size = 8

        def pr(pid):
            people = [{"user": {"display_name": "Lead"}, "approved": pid == 2, "state": "approved" if pid == 2 else None}]
            if pid == 5:
                people.append({"user": {"display_name": "Reviewer"}, "approved": False, "state": "changes_requested"})
            return {"id": pid, "title": f"PR {pid}", "author": {"display_name": "Dev"},
                    "source": {"branch": {"name": f"feat/{pid}"}, "commit": {"hash": f"{pid:012d}"}},
                    "destination": {"branch": {"name": "main"}}, "draft": pid == 4, "participants": people,
                    "created_on": "2026-10-12T08:00:00+00:00", "updated_on": "2026-10-13T08:00:00+00:00"}

        self.bb.routes += [
            ("GET", r"pullrequests\?state=OPEN&page=2", lambda m, p: {
                "size": self.size, "values": [pr(i) for i in (5, 6, 7, 8)]}),
            ("GET", r"pullrequests\?state=OPEN&pagelen=50.*", lambda m, p: {
                "size": self.size, "next": "pullrequests?state=OPEN&page=2", "values": [pr(i) for i in (1, 2, 3, 4)]}),
            ("GET", r"pullrequests/2/diffstat\?page=2", {"size": 2, "values": [
                {"new": {"path": "src/core/b.ts"}, "old": None, "lines_added": 3, "lines_removed": 0}]}),
            ("GET", r"pullrequests/2/diffstat\?.*", {"size": 2, "next": "pullrequests/2/diffstat?page=2", "values": [
                {"new": {"path": "src/core/a.ts"}, "old": {"path": "src/core/a.ts"}, "lines_added": 1,
                 "lines_removed": 1}]}),
            ("GET", r"pullrequests/(\d+)/diffstat\?.*", lambda m, p: {"size": len(self.PATHS[int(m[1])]), "values": [
                {"new": {"path": path}, "old": {"path": path}, "lines_added": 1, "lines_removed": 0}
                for path in self.PATHS[int(m[1])]]}),
            ("GET", r"pullrequests/(\d+)/statuses\?.*", {"size": 1, "values": [{"state": "SUCCESSFUL"}]}),
        ]
        self.out = self.dir / "inventory.json"

    def test_grouping_suggestion(self):
        code, out = self.run_main("inventory", "--out", self.out)
        self.assertEqual(code, 0, out)
        self.assertIn("suggested lot: #1 #2 #6", out)
        self.assertIn("suggested lot: #7 #8", out)
        self.assertIn("alone: #3", out)

        self.assertIn("apart (draft), never grouped: #4", out)
        self.assertIn("apart (changes requested), never grouped: #5", out)
        self.assertIn("pair #1 #2: 1 shared paths: src/core/a.ts", out)
        self.assertNotIn("pair #1 #3", out)
        pairs = [line for line in out.splitlines() if line.startswith("pair ")]
        self.assertEqual(len(pairs), 3, pairs)
        self.assertFalse([line for line in pairs if "#4" in line or "#5" in line], pairs)
        document = json.loads(self.out.read_text(encoding="utf-8"))
        self.assertEqual(document["size"], 8)
        self.assertEqual(document["lots"], [[1, 2, 6], [7, 8]])
        self.assertEqual(document["apart"]["draft"], [4])
        by_id = {pr["id"]: pr for pr in document["prs"]}
        self.assertEqual(by_id[2]["paths"], ["src/core/a.ts", "src/core/b.ts"])
        self.assertEqual(by_id[2]["approved_by"], ["Lead"])
        self.assertEqual(by_id[5]["changes_requested_by"], ["Reviewer"])
        self.assertTrue(by_id[4]["draft"])
        self.assertEqual(self.bb.writes(), [])

    def test_shared_bun_lockfile_does_not_form_a_lot(self):
        self.PATHS = {**self.PATHS, 7: ["bun.lock"], 8: ["bun.lock"]}
        code, out = self.run_main("inventory", "--out", self.out)
        self.assertEqual(code, 0, out)
        document = json.loads(self.out.read_text(encoding="utf-8"))
        self.assertIn(7, document["alone"])
        self.assertIn(8, document["alone"])
        self.assertNotIn([7, 8], document["lots"])

    def test_draft_and_changes_requested_never_couple(self):
        plan = forge.suggest([
            {"id": 1, "draft": False, "changes_requested_by": [], "destination": "main", "paths": ["x"]},
            {"id": 2, "draft": True, "changes_requested_by": [], "destination": "main", "paths": ["x"]},
            {"id": 3, "draft": False, "changes_requested_by": ["R"], "destination": "main", "paths": ["x"]},
            {"id": 4, "draft": False, "changes_requested_by": [], "destination": "release", "paths": ["x"]},
        ])
        self.assertEqual(plan["pairs"], [])
        self.assertEqual(plan["lots"], [])
        self.assertEqual(plan["alone"], [1])
        self.assertEqual(plan["apart"], {"draft": [2], "changes requested": [3], "not targeting primary branch": [4]})

    def test_count_mismatch_refuses_and_writes_nothing(self):
        self.size = 9
        code, out = self.run_main("inventory", "--out", self.out)
        self.assertEqual(code, 2, out)
        self.assertIn("counted 8 values but size is 9", out)
        self.assertFalse(self.out.exists())


GIT_ENV = {"GIT_AUTHOR_NAME": "Fixture Maintainer", "GIT_AUTHOR_EMAIL": "maintainer@example.org",
           "GIT_COMMITTER_NAME": "Fixture Maintainer", "GIT_COMMITTER_EMAIL": "maintainer@example.org",
           "GIT_CONFIG_NOSYSTEM": "1"}


class VerifyMergeTest(BbLotCase):
    def git(self, cwd, *args, env=None):
        done = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, check=False,
                              env={**os.environ, **(env or {})})
        self.assertEqual(done.returncode, 0, f"git {args}: {done.stderr}")
        return done.stdout.strip()

    def commit(self, cwd, name, text):
        (cwd / name).write_text(text, encoding="utf-8")
        self.git(cwd, "add", name)
        self.git(cwd, "commit", "-q", "-m", f"change {name}")

    def setUp(self):
        super().setUp()
        config = self.dir / "gitconfig"
        config.write_text("", encoding="utf-8")
        patcher = mock.patch.dict(os.environ, {**GIT_ENV, "GIT_CONFIG_GLOBAL": str(config)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.origin = self.dir / "workspace" / "project.git"
        self.origin.parent.mkdir()
        self.git(self.dir, "init", "-q", "--bare", "-b", "main", str(self.origin))
        self.work = self.dir / "work"
        self.work.mkdir()
        self.git(self.work, "init", "-q", "-b", "main")
        self.git(self.work, "remote", "add", "origin", self.origin.as_posix())
        self.commit(self.work, "app.txt", "base\n")
        self.git(self.work, "push", "-q", "origin", "main")
        self.git(self.work, "checkout", "-q", "-b", "feat/lot")
        self.commit(self.work, "feature.txt", "feature\n")
        self.commit(self.work, "app.txt", "base\nfeature\n")
        self.git(self.work, "push", "-q", "origin", "feat/lot")
        self.git(self.work, "checkout", "-q", "main")
        self.commit(self.work, "other.txt", "main moved\n")
        self.git(self.work, "push", "-q", "origin", "main")
        self.push_clone = self.dir / "push-clone"
        self.git(self.dir, "clone", "-q", self.origin.as_posix(), str(self.push_clone))
        self.candidate = self.git(self.work, "rev-parse", "feat/lot")
        self.git(self.push_clone, "remote", "set-url", "origin", self.context["remote_url"])
        self.git(self.push_clone, "config", f"url.{self.origin.as_uri()}.insteadOf", self.context["remote_url"])

    def squash(self, tree=None, author="Fixture Maintainer"):
        tree = tree or self.git(self.work, "merge-tree", "--write-tree", "main", "feat/lot").splitlines()[0]
        merged = self.git(self.work, "commit-tree", tree, "-p", "main", "-m", "Lot (pull request #958)",
                          env={"GIT_AUTHOR_NAME": author})
        self.git(self.work, "push", "-q", "origin", f"{merged}:refs/heads/main")
        self.bb.add_pr(958, state="MERGED", head=self.candidate[:12], merge=merged[:12])
        return merged

    def test_github_merge_verifies_real_fetched_tree_and_rejects_other_forge_clone(self):
        from test_github_adapter import GitHubFixture
        import forge_transport
        merged = self.squash()
        github = GitHubFixture()
        github.prs[958] = github.pull(958, head=self.candidate, merged=True)
        github.prs[958]["merge_commit_sha"] = merged
        self.context.update(forge="github", remote_url="git@github.com:workspace/project.git")
        self.git(self.push_clone, "remote", "set-url", "origin", self.context["remote_url"])
        self.git(self.push_clone, "config", f"url.{self.origin.as_uri()}.insteadOf", self.context["remote_url"])
        with mock.patch.object(forge, "api", forge_transport.api), \
                mock.patch.object(forge_transport, "github_request", github):
            code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
            self.assertEqual(code, 0, out)
            self.assertIn("verified: #958", out)
            self.git(self.push_clone, "remote", "set-url", "origin", "https://bitbucket.org/workspace/project.git")
            code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
            self.assertEqual(code, 2, out)
            self.assertIn("does not match", out)

    def test_squash_with_the_merged_tree_passes(self):
        merged = self.squash()
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 0, out)
        self.assertIn(f"merge commit {merged} author Fixture Maintainer <maintainer@example.org>", out)
        self.assertIn(f"fetched origin/main {merged}", out)
        self.assertIn("verified: #958", out)

    def test_squash_with_another_tree_refuses(self):
        self.squash(tree=self.git(self.work, "rev-parse", "feat/lot^{tree}"))
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("REFUSED: tree of", out)

    def test_agent_identity_refuses(self):
        self.squash(author="Claude")
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("names an agent identity", out)

    def test_candidate_other_than_pr_head_refuses(self):
        self.squash()
        main = self.git(self.work, "rev-parse", "main")
        code, out = self.run_main("verify-merge", 958, main, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("is not the head", out)

    def test_pr_not_merged_refuses(self):
        self.squash()
        self.bb.prs[958]["state"] = "OPEN"
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("REFUSED: #958 is OPEN", out)


if __name__ == "__main__":
    unittest.main()
