import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

from project_fixture import SCRIPTS, fixture, git, run, setup


class ProcessEnvironmentTests(unittest.TestCase):
    def test_environment_git_config_cannot_disable_the_real_commit_hook(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            repo = root / 'repo'
            hook = repo / '.git/hooks/pre-commit'
            hook.write_text('#!/bin/sh\nexit 23\n')
            hook.chmod(0o755)
            git(repo, 'config', 'core.hooksPath', '.git/hooks')
            before = git(repo, 'rev-parse', 'HEAD')
            (repo / 'base.txt').write_text('candidate\n')
            git(repo, 'add', 'base.txt')
            message = root / 'message.txt'
            message.write_text('must observe real hook')
            environment = dict(os.environ, GIT_CONFIG_COUNT='1',
                               GIT_CONFIG_KEY_0='core.hooksPath', GIT_CONFIG_VALUE_0='/dev/null')
            result = run(path, 'commit', str(message), 'hook-bypass', environment=environment)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(git(repo, 'rev-parse', 'HEAD'), before)

    def test_git_redirect_environment_cannot_commit_in_another_campaign_clone(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            repo, other = root / 'repo', root / 'seed'
            hook = repo / '.git/hooks/pre-commit'
            hook.write_text('#!/bin/sh\nexit 23\n')
            hook.chmod(0o755)
            git(repo, 'config', 'core.hooksPath', '.git/hooks')
            git(other, 'checkout', '-qb', plan['branch'], plan['base_sha'])
            for clone in (repo, other):
                (clone / 'base.txt').write_text('staged candidate\n')
                git(clone, 'add', 'base.txt')
            before = git(other, 'rev-parse', 'HEAD')
            message = root / 'message.txt'
            message.write_text('must stay in the owned candidate repository')
            environment = dict(os.environ, GIT_DIR=str(other / '.git'),
                               GIT_WORK_TREE=str(other), GIT_INDEX_FILE=str(other / '.git/index'))
            result = run(path, 'commit', str(message), 'redirect-bypass', environment=environment)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('build exit=0', result.stdout)
            self.assertEqual(git(other, 'rev-parse', 'HEAD'), before)
            self.assertEqual(git(repo, 'rev-parse', 'HEAD'), plan['base_sha'])

    def test_timeout_does_not_wait_for_an_escaped_child_holding_stdout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child_code = 'import os,time; os.setsid(); time.sleep(8)'
            parent_code = ('import pathlib,subprocess,sys,time; '
                           'child=subprocess.Popen([sys.executable,"-c",sys.argv[1]]); '
                           'pathlib.Path(sys.argv[2]).write_text(str(child.pid)); time.sleep(20)')
            plan = {'commands': {'build': [{'argv': [sys.executable, '-c', parent_code,
                            child_code, str(root / 'child.pid')], 'timeout_seconds': 1}]}}
            path = root / 'lot.json'
            path.write_text(json.dumps(plan))
            start = time.monotonic()
            try:
                result = subprocess.run([sys.executable, str(SCRIPTS / 'project_commands.py'),
                                         str(path), 'build', directory, str(root / 'run.log')],
                                        text=True, capture_output=True, timeout=12)
                elapsed = time.monotonic() - start
                self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
                self.assertLess(elapsed, 2.5)
            finally:
                if (root / 'child.pid').exists():
                    try:
                        os.kill(int((root / 'child.pid').read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass


if __name__ == '__main__':
    unittest.main()
