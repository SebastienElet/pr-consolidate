import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from cli_fixture import ForgeCLI

GIT_ENV = {"GIT_AUTHOR_NAME": "Fixture Maintainer", "GIT_AUTHOR_EMAIL": "maintainer@example.org",
           "GIT_COMMITTER_NAME": "Fixture Maintainer", "GIT_COMMITTER_EMAIL": "maintainer@example.org",
           "GIT_CONFIG_NOSYSTEM": "1"}


class VerifyMergeTest(unittest.TestCase):
    def git(self, cwd, *args, env=None):
        done = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, check=False,
                              env={**getattr(self, "environment", os.environ), **(env or {})})
        self.assertEqual(done.returncode, 0, f"git {args}: {done.stderr}")
        return done.stdout.strip()

    def commit(self, cwd, name, text):
        (cwd / name).write_text(text, encoding="utf-8")
        self.git(cwd, "add", name)
        self.git(cwd, "commit", "-q", "-m", f"change {name}")

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="verify-merge-")
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.cli = ForgeCLI(self.dir)
        self.context = {"remote_url": self.cli.url}
        self.root = self.dir
        self.write = lambda name, text: (self.dir / name).write_text(text)
        config = self.dir / "gitconfig"
        config.write_text("", encoding="utf-8")
        self.environment = {**self.cli.environment, **GIT_ENV, "GIT_CONFIG_GLOBAL": str(config)}
        self.origin = self.dir / "workspace" / "project.git"
        self.origin.parent.mkdir()
        self.git(self.dir, "init", "-q", "--bare", "-b", "main", str(self.origin))
        self.work = self.dir / "merge-work"
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
        fixture = self.cli.fixture
        fixture.prs[958] = fixture.pull(958, head=self.candidate, merged=True)
        fixture.prs[958]["merge_commit_sha"] = merged
        self.cli.save_fixture()
        return merged

    def run_main(self, *args):
        result = self.cli.run(*args, environment=self.environment)
        return result.returncode, result.stdout + result.stderr

    def test_other_forge_clone_refuses(self):
        self.squash()
        self.git(self.push_clone, "remote", "set-url", "origin", "https://bitbucket.org/workspace/project.git")
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("does not match", out)

    def test_squash_with_the_merged_tree_passes(self):
        merged = self.squash()
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 0, out)
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
        self.assertIn("is not the PR head", out)

    def test_pr_not_merged_refuses(self):
        self.squash()
        self.cli.fixture.prs[958]["state"] = "open"
        self.cli.fixture.prs[958]["merged"] = False
        self.cli.fixture.prs[958]["merged_at"] = None
        self.cli.save_fixture()
        code, out = self.run_main("verify-merge", 958, self.candidate, self.push_clone)
        self.assertEqual(code, 2, out)
        self.assertIn("REFUSED: #958 is not MERGED", out)
