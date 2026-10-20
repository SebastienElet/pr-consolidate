import contextlib
import io
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest import mock

from test_forge import forge
import forge_transport
import git_context


class RepositoryContextTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="pr-context-")
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.config = self.root / "gitconfig"
        self.config.write_text("")
        patcher = mock.patch.dict(os.environ, {
            "GIT_CONFIG_GLOBAL": str(self.config), "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.org",
            "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.org",
        })
        patcher.start()
        self.addCleanup(patcher.stop)

    def git(self, directory, *args):
        result = subprocess.run(["git", "-C", str(directory), *args], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def checkout(self, name, branch):
        bare = self.root / f"{name}.git"
        self.git(self.root, "init", "--bare", "-q", "-b", branch, str(bare))
        work = self.root / name
        work.mkdir()
        self.git(work, "init", "-q", "-b", branch)
        self.git(work, "commit", "--allow-empty", "-q", "-m", "initial")
        self.git(work, "remote", "add", "origin", str(bare))
        self.git(work, "push", "-q", "origin", branch)
        url = f"https://bitbucket.org/workspace/{name}.git"
        self.git(work, "remote", "set-url", "origin", url)
        self.git(work, "config", f"url.{bare.as_uri()}.insteadOf", url)
        return work

    def test_create_targets_each_current_repository_remote_primary_branch(self):
        for name, branch in (("alpha", "main"), ("beta", "release/trunk")):
            with self.subTest(repository=name):
                work = self.checkout(name, branch)
                description = work / "description.md"
                description.write_text("Consolidation fixture")
                posts = []

                def api(path, method="GET", payload=None, timeout=180):
                    if method == "GET":
                        return {"size": 0, "values": []}
                    posts.append(payload)
                    return {"id": 1, "source": {**payload["source"], "commit": {"hash": "a" * 40}},
                            "destination": payload["destination"], "close_source_branch": False}

                with contextlib.chdir(work), mock.patch.object(forge, "api", api), \
                        contextlib.redirect_stdout(io.StringIO()):
                    code = forge.main(["create", "feat/batch", "Batch", str(description)])
                self.assertEqual(code, 0)
                self.assertEqual(posts[0]["destination"]["branch"]["name"], branch)

    def discover(self, work):
        with contextlib.chdir(work):
            return git_context.discover()

    def test_discovery_in_subdirectory_uses_current_origin_and_remote_head(self):
        for name, branch in (("alpha", "main"), ("beta", "trunk")):
            work = self.checkout(name, branch)
            nested = work / "nested"
            nested.mkdir()
            context = self.discover(nested)
            self.assertEqual(context, {"root": str(work.resolve()), "repository": f"workspace/{name}",
                                      "remote_url": f"https://bitbucket.org/workspace/{name}.git",
                                      "base_branch": branch, "forge": "bitbucket"})
            self.git(work, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/stale")
            self.assertEqual(self.discover(work)["base_branch"], branch)

    def test_ssh_and_https_urls_identify_exact_bitbucket_repository(self):
        for url in ("https://bitbucket.org/Workspace/Project.git", "git@bitbucket.org:Workspace/Project.git",
                    "ssh://git@bitbucket.org/Workspace/Project.git",
                    "ssh://git@ssh.bitbucket.org:443/Workspace/Project.git"):
            self.assertEqual(git_context.repository_from_url(url), "workspace/project")

    def test_valid_hidden_repository_names_are_not_rejected(self):
        self.assertEqual(git_context.origin_identity("git@github.com:owner/.dotfiles.git"),
                         {"repository": "owner/.dotfiles", "forge": "github"})

    def test_unsupported_credentials_malformed_and_traversing_origins_are_refused(self):
        urls = ("https://gitlab.com/workspace/project.git", "file:///tmp/project.git",
                "https://tokenSECRET@bitbucket.org/workspace/project.git",
                "https://user:SECRET@bitbucket.org/workspace/project.git",
                "ssh://git:SECRET@bitbucket.org/workspace/project.git",
                "git@bitbucket.org:workspace/../project.git", "git@bitbucket.org:workspace/.git",
                "https://bitbucket.org/workspace/project/extra.git", "https://bitbucket.org/../project.git",
                "https://bitbucket.org/workspace/%2e%2e.git", "https://bitbucket.org/workspace/project.git?SECRET",
                "https://bitbucket.org/workspace/project.git#SECRET", "https://bitbucket.org:invalid/x/y",
                "https://bitbucket.org/workspace/project.git\n", "git@bitbucket.org:workspace/project.git/",
                "ssh://git@ssh.bitbucket.org:22/workspace/project.git", "https://bitbucket.org//workspace/project.git")
        for url in urls:
            with self.subTest(url=url), self.assertRaises(git_context.ContextError) as error:
                git_context.repository_from_url(url)
            self.assertNotIn("SECRET", str(error.exception))

    def test_missing_ambiguous_origin_and_foreign_pushurl_stop_before_api(self):
        for state in ("missing", "multiple", "foreign_push", "credential_url"):
            work = self.checkout(state, "main")
            if state == "missing":
                self.git(work, "remote", "remove", "origin")
            elif state == "multiple":
                self.git(work, "config", "--add", "remote.origin.url", "https://bitbucket.org/other/project.git")
            elif state == "foreign_push":
                self.git(work, "config", "remote.origin.pushurl", "https://bitbucket.org/other/project.git")
            else:
                self.git(work, "remote", "set-url", "origin", "https://SECRET@bitbucket.org/workspace/project.git")
            output = io.StringIO()
            with contextlib.chdir(work), mock.patch.object(forge, "api") as api, \
                    contextlib.redirect_stdout(output):
                code = forge.main(["inventory", "--out", str(work / "inventory.json")])
            self.assertEqual(code, 2, output.getvalue())
            self.assertNotIn("SECRET", output.getvalue())
            self.assertFalse((work / "inventory.json").exists())
            api.assert_not_called()

    def test_github_origin_detects_forge_and_primary_branch(self):
        work = self.checkout("github_project", "trunk")
        url = "git@github.com:workspace/project.git"
        self.git(work, "remote", "set-url", "origin", url)
        self.git(work, "config", f"url.{(self.root / 'github_project.git').as_uri()}.insteadOf", url)
        context = self.discover(work)
        self.assertEqual(context["repository"], "workspace/project")
        self.assertEqual(context["forge"], "github")
        self.assertEqual(context["base_branch"], "trunk")

    def test_git_environment_cannot_redirect_discovery_to_another_repository(self):
        work = self.checkout("selected", "main")
        other = self.checkout("other", "trunk")
        with mock.patch.dict(os.environ, {"GIT_DIR": str(other / ".git"), "GIT_WORK_TREE": str(other),
                                          "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "remote.origin.url",
                                          "GIT_CONFIG_VALUE_0": "https://github.com/foreign/repository.git"}):
            context = git_context.discover(work)
        self.assertEqual(context["root"], str(work.resolve()))
        self.assertEqual(context["repository"], "workspace/selected")
        self.assertEqual(context["base_branch"], "main")

    def test_origin_values_are_not_trimmed_into_valid_urls(self):
        for suffix in (" ", "\n", "\t"):
            work = self.checkout(f"padding{len(suffix)}{ord(suffix)}", "main")
            self.git(work, "remote", "set-url", "origin", f"https://bitbucket.org/workspace/project.git{suffix}")
            with self.assertRaises(git_context.ContextError):
                git_context.remote_context(work)

    def test_remote_without_primary_branch_and_failed_remote_query_are_refused(self):
        work = self.checkout("unborn", "main")
        self.git(self.root / "unborn.git", "symbolic-ref", "HEAD", "refs/heads/missing")
        with self.assertRaises(git_context.ContextError):
            self.discover(work)
        self.git(self.root / "unborn.git", "symbolic-ref", "HEAD", "refs/heads/main")
        (self.root / "unborn.git").rename(self.root / "gone.git")
        with self.assertRaises(git_context.ContextError):
            self.discover(work)

    def test_transport_changes_repository_without_retaining_previous_identity(self):
        result = subprocess.CompletedProcess([], 0, '{"id": 1}', "")
        for name in ("alpha", "beta"):
            work = self.checkout(name, "main")
            forge_transport.configure(self.discover(work))
            with mock.patch.object(subprocess, "run", return_value=result) as process:
                forge_transport.api("pullrequests/9/comments", "POST", {"content": {"raw": "review"}})
            self.assertEqual(process.call_args.args[0][2], f"/repositories/workspace/{name}/pullrequests/9/comments")
            other = "beta" if name == "alpha" else "alpha"
            with mock.patch.object(subprocess, "run") as process, self.assertRaises(forge_transport.ForgeAPIError):
                forge_transport.api(f"https://api.bitbucket.org/2.0/repositories/workspace/{other}/pullrequests?page=2")
            process.assert_not_called()


if __name__ == "__main__":
    unittest.main()
