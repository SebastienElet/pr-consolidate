import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

from project_fixture import fixture, git, run, save, setup


class ProjectPreflightTests(unittest.TestCase):
    def test_project_check_receives_environment_without_persisting_registry_secret(self):
        token = 'fixture-private-registry-value'
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            plan['commands']['check'] = [{'argv': [sys.executable, '-c',
                'import os; print("credential-present=" + str(bool(os.environ.get("NPM_TOKEN")))); '
                'print(os.environ.get("NPM_TOKEN", ""))'], 'timeout_seconds': 3}]
            save(path, plan)
            setup(path, root)
            result = run(path, 'preflight', environment=dict(os.environ, NPM_TOKEN=token))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            logs = '\n'.join(p.read_text() for p in (root / 'logs').glob('*.log'))
            self.assertIn('credential-present=True', logs)
            self.assertNotIn(token, result.stdout + result.stderr + logs + path.read_text())
            self.assertFalse((root / 'repo/.npmrc').exists())

    def test_check_failure_and_dirty_check_result_are_not_success(self):
        for script in ('exit(19)', 'import pathlib; pathlib.Path("base.txt").write_text("changed")'):
            with self.subTest(script=script), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                plan['commands']['check'] = [{'argv': [sys.executable, '-c', script], 'timeout_seconds': 3}]
                save(path, plan)
                setup(path, root)
                result = run(path, 'preflight')
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_reinstall_is_explicit_and_missing_phase_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            result = run(path, 'resync')
            self.assertEqual(result.returncode, 2)
            self.assertIn('no reinstall command', result.stderr)
            plan['commands']['reinstall'] = [{'argv': [sys.executable, '-c', 'print("reinstalled")'],
                                               'timeout_seconds': 3}]
            save(path, plan)
            result = run(path, 'resync')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_merge_base_refuses_conflicts_outside_declared_lockfiles(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            repo, seed = root / 'repo', root / 'seed'
            (repo / 'base.txt').write_text('lot version\n')
            git(repo, 'add', 'base.txt')
            git(repo, 'commit', '-qm', 'lot changes')
            git(seed, 'checkout', 'trunk')
            (seed / 'base.txt').write_text('main version\n')
            git(seed, 'add', 'base.txt')
            git(seed, 'commit', '-qm', 'new base')
            new_base = git(seed, 'rev-parse', 'HEAD')
            git(repo, 'fetch', '--no-prune', str(seed), '+trunk:refs/remotes/origin/trunk')
            message = root / 'message.txt'
            message.write_text('merge updated base')
            result = run(path, 'merge-base', new_base, str(message))
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('outside declared lockfiles', result.stderr)
            self.assertEqual(git(repo, 'rev-parse', 'MERGE_HEAD'), new_base)

    def test_lockfile_conflict_uses_the_explicit_project_resolution_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            plan['lockfiles'] = ['base.txt']
            plan['commands']['lockfile'] = [{'argv': [sys.executable, '-c',
                'import pathlib; pathlib.Path("base.txt").write_text("resolved lock state\\n")'],
                'timeout_seconds': 3}]
            save(path, plan)
            setup(path, root)
            repo, seed = root / 'repo', root / 'seed'
            (repo / 'base.txt').write_text('lot dependencies\n')
            git(repo, 'add', 'base.txt')
            git(repo, 'commit', '-qm', 'lot dependencies')
            git(seed, 'checkout', 'trunk')
            (seed / 'base.txt').write_text('main dependencies\n')
            git(seed, 'add', 'base.txt')
            git(seed, 'commit', '-qm', 'main dependencies')
            new_base = git(seed, 'rev-parse', 'HEAD')
            git(repo, 'fetch', '--no-prune', str(seed), '+trunk:refs/remotes/origin/trunk')
            message = root / 'message.txt'
            message.write_text('Merge updated dependency baseline')
            result = run(path, 'merge-base', new_base, str(message))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((repo / 'base.txt').read_text(), 'resolved lock state\n')
            self.assertEqual(git(repo, 'show', '--format=%P', '--no-patch').split()[-1], new_base)


if __name__ == '__main__':
    unittest.main()
