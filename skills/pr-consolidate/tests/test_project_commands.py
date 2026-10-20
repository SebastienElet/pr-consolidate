import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'


class ProjectCommandsTests(unittest.TestCase):
    def test_build_runs_project_argv_without_a_runtime_assumption(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'repo').mkdir()
            lot = {'commands': {'build': [{'argv': [sys.executable, '-c',
                    'import pathlib,sys; pathlib.Path("result").write_text(sys.argv[1])',
                    'literal $(touch injected)'], 'timeout_seconds': 3}]}}
            plan = root / 'lot.json'
            plan.write_text(json.dumps(lot))
            result = subprocess.run([sys.executable, str(SCRIPTS / 'project_commands.py'),
                                     str(plan), 'build', str(root / 'repo'), str(root / 'build.log')],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'repo/result').read_text(), 'literal $(touch injected)')
            self.assertFalse((root / 'repo/injected').exists())

    def run_command(self, commands, phase='build', token=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan = root / 'lot.json'
            plan.write_text(json.dumps({'commands': commands}))
            log = root / 'execution.log'
            environment = dict(os.environ)
            if token:
                environment['NPM_TOKEN'] = token
            result = subprocess.run([sys.executable, str(SCRIPTS / 'project_commands.py'),
                                     str(plan), phase, directory, str(log)],
                                    env=environment, capture_output=True, text=True)
            return result, log.read_text() if log.exists() else ''

    def test_nonzero_exit_is_preserved_and_stops_following_commands(self):
        commands = {'build': [{'argv': [sys.executable, '-c', 'print("first"); exit(17)'],
                               'timeout_seconds': 3},
                              {'argv': [sys.executable, '-c', 'print("unexpected")'],
                               'timeout_seconds': 3}]}
        result, log = self.run_command(commands)
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertIn('first', log)
        self.assertNotIn('unexpected', log)

    def test_missing_command_missing_phase_and_malformed_specs_refuse(self):
        cases = [{}, {'check': []}, {'build': [{'argv': 'echo yes', 'timeout_seconds': 3}]},
                 {'build': [{'argv': ['missing-project-runtime-xyz'], 'timeout_seconds': 3}]},
                 {'build': [{'argv': ['echo', 'yes'], 'timeout_seconds': True}]}]
        for commands in cases:
            with self.subTest(commands=commands):
                result, log = self.run_command(commands)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr + log)

    def test_timeout_fails_and_logs_do_not_persist_inherited_registry_secret(self):
        token = 'private-fixture-secret'
        commands = {'build': [{'argv': [sys.executable, '-c',
                    'import os,time; print(os.environ["NPM_TOKEN"],flush=True); time.sleep(10)'],
                    'timeout_seconds': 1}]}
        result, log = self.run_command(commands, token=token)
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertNotIn(token, result.stdout + result.stderr + log)
        self.assertIn('[REDACTED]', log)

    def test_secret_cannot_be_persisted_in_plan_command_arguments(self):
        commands = {'build': [{'argv': ['echo', 'private-fixture-secret'], 'timeout_seconds': 3}]}
        result, log = self.run_command(commands, token='private-fixture-secret')
        self.assertEqual(result.returncode, 2)
        self.assertNotIn('private-fixture-secret', result.stderr + log)


if __name__ == '__main__':
    unittest.main()
