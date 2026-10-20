import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import unittest

from project_fixture import SCRIPTS, fixture, git, run, save, setup


class GenericGitTests(unittest.TestCase):
    def test_nonstandard_main_branch_batch_preserves_native_hooks_and_real_origin(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            repo = root / 'repo'
            self.assertEqual(git(repo, 'remote', 'get-url', 'origin'), plan['remote_url'])
            self.assertEqual(git(repo, 'config', 'core.hooksPath'), '.hooks')
            for number in ('1', '2'):
                self.assertEqual(run(path, 'merge', number).returncode, 0)
                message = root / 'message.txt'
                message.write_text(f'Merge source {number}')
                result = run(path, 'commit', str(message), f'pr-{number}')
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((repo / '.git/hook-ran').exists())
            candidate = git(repo, 'rev-parse', 'HEAD')
            result = run(path, 'push', candidate)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(git(root, '--git-dir=' + plan['remote_url'], 'rev-parse', plan['branch']), candidate)
            self.assertEqual(git(repo, 'log', '-1', '--format=%an <%ae>'), 'Human Reviewer <human@example.invalid>')

    def test_explicit_no_build_project_still_commits_with_its_native_hook(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            plan['commands']['build'] = []
            save(path, plan)
            setup(path, root)
            self.assertEqual(run(path, 'merge', '1').returncode, 0)
            message = root / 'message.txt'
            message.write_text('Merge source without a build phase')
            result = run(path, 'commit', str(message), 'no-build', environment=dict(
                os.environ, GIT_AUTHOR_NAME='Codex Agent', GIT_AUTHOR_EMAIL='agent@example.invalid',
                GIT_COMMITTER_NAME='Codex Agent', GIT_COMMITTER_EMAIL='agent@example.invalid'))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((root / 'repo/.git/hook-ran').exists())
            self.assertIn('no project build step', (root / 'logs/build-no-build.log').read_text())
            self.assertEqual(run(path, 'preflight').returncode, 0)

    def test_snapshot_refuses_moved_source_head_or_base_before_bundle(self):
        for branch in ('feature/1', 'trunk'):
            with self.subTest(branch=branch), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                seed = root / 'seed'
                git(seed, 'checkout', branch)
                git(seed, 'commit', '-q', '--allow-empty', '-m', 'moved')
                git(seed, 'push', '-q', 'origin', branch)
                result = run(path, 'bundle')
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn('moved', result.stderr)
                self.assertFalse((root / 'artifacts/fixture.bundle').exists())

    def test_setup_refuses_wrong_snapshot_before_creating_repo(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            self.assertEqual(run(path, 'bundle').returncode, 0)
            plan['prs'][0]['head'] = plan['base_sha']
            save(path, plan)
            result = run(path, 'setup', str(root / 'artifacts/fixture.bundle'))
            self.assertEqual(result.returncode, 2)
            self.assertFalse((root / 'repo').exists())

    def test_setup_refuses_a_source_worktree_with_shared_external_git_metadata(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as external:
            root, path, plan = fixture(directory)
            self.assertEqual(run(path, 'bundle').returncode, 0)
            shutil.rmtree(root / 'source')
            clone = Path(external) / 'shared'
            git(Path(external), 'clone', '-q', plan['remote_url'], str(clone))
            git(clone, 'worktree', 'add', '--detach', str(root / 'source'), 'trunk')
            result = run(path, 'setup', str(root / 'artifacts/fixture.bundle'))
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('outside root', result.stderr)
            self.assertFalse((root / 'repo').exists())

    def test_setup_does_not_force_a_hook_path_without_project_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            del plan['hooks_path']
            save(path, plan)
            setup(path, root)
            self.assertEqual(git(root / 'repo', 'config', '--get', 'core.hooksPath', check=False), '')

    def test_plan_refuses_base_source_lot_branch_collisions_and_escaping_clones(self):
        for key, value in [('branch', 'trunk'), ('branch', 'feature/1'), ('source', '/tmp/shared'),
                           ('root', '/tmp/../shared')]:
            with self.subTest(key=key), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                plan[key] = value
                save(path, plan)
                result = run(path, 'bundle')
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_commit_refuses_conflict_markers_dirty_untracked_and_failed_build(self):
        for fault in ('marker', 'newline-marker', 'unstaged', 'untracked', 'build', 'hook'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                setup(path, root)
                repo = root / 'repo'
                before = git(repo, 'rev-parse', 'HEAD')
                (repo / 'base.txt').write_text('<<<<<<< unresolved\n' if fault == 'marker' else 'change\n')
                git(repo, 'add', 'base.txt')
                if fault == 'newline-marker':
                    (repo / 'unsafe\nname.txt').write_text('<<<<<<< unresolved\n')
                    git(repo, 'add', 'unsafe\nname.txt')
                if fault == 'hook':
                    (repo / '.git/hooks').mkdir(exist_ok=True)
                    hook = repo / '.git/hooks/pre-commit'
                    hook.write_text('#!/bin/sh\nexit 23\n')
                    hook.chmod(0o755)
                    git(repo, 'config', 'core.hooksPath', '.git/hooks')
                if fault == 'unstaged':
                    (repo / 'base.txt').write_text('different\n')
                if fault == 'untracked':
                    (repo / 'unknown.txt').write_text('untracked')
                if fault == 'build':
                    plan['commands']['build'][0]['argv'] = ['missing-project-builder-xyz']
                    save(path, plan)
                message = root / 'message.txt'
                message.write_text('change')
                result = run(path, 'commit', str(message), 'failure')
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(git(repo, 'rev-parse', 'HEAD'), before)

    def test_push_refuses_unrelated_history_agent_identity_wrong_origin_and_nonfastforward(self):
        for fault in ('unrelated', 'agent', 'origin', 'divergent'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                setup(path, root)
                repo = root / 'repo'
                if fault == 'unrelated':
                    git(repo, 'checkout', '--orphan', 'orphan')
                    git(repo, 'rm', '-rf', '.')
                    git(repo, '-c', 'user.name=Human', '-c', 'user.email=human@example.invalid',
                        'commit', '-q', '--allow-empty', '-m', 'unrelated')
                    git(repo, 'branch', '-f', plan['branch'], 'HEAD')
                else:
                    author = 'Codex Agent' if fault == 'agent' else 'Human'
                    git(repo, '-c', f'user.name={author}', '-c', 'user.email=human@example.invalid',
                        'commit', '-q', '--allow-empty', '-m', 'candidate')
                candidate = git(repo, 'rev-parse', 'HEAD')
                if fault == 'origin':
                    git(root / 'push', 'remote', 'set-url', '--push', 'origin', str(root / 'wrong.git'))
                if fault == 'divergent':
                    git(root / 'seed', 'push', '-q', 'origin', f'feature/2:refs/heads/{plan["branch"]}')
                before = git(root, 'ls-remote', plan['remote_url'], f'refs/heads/{plan["branch"]}')
                result = run(path, 'push', candidate)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertEqual(git(root, 'ls-remote', plan['remote_url'], f'refs/heads/{plan["branch"]}'), before)

    def test_archive_verification_detects_modified_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'evidence.log').write_text('candidate evidence')
            subprocess.run(['python3', str(SCRIPTS / 'native.py'), 'manifest', directory], check=True)
            subprocess.run(['python3', str(SCRIPTS / 'native.py'), 'verify', directory], check=True)
            (root / 'evidence.log').write_text('modified evidence')
            result = subprocess.run(['python3', str(SCRIPTS / 'native.py'), 'verify', directory],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('archive digest mismatch: ./evidence.log', result.stderr)


if __name__ == '__main__':
    unittest.main()
