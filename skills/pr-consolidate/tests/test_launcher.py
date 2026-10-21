import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

from project_fixture import SCRIPTS


class NativeLauncherTests(unittest.TestCase):
    def test_absent_native_runtime_refuses_without_falling_back_to_a_development_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(SCRIPTS, root / 'scripts')
            result = subprocess.run([str(root / 'scripts/forge'), 'context'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('native runtime is missing', result.stderr)

    def test_unknown_native_entrypoint_refuses_before_running_an_operation(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / 'unexpected-entrypoint'
            shutil.copyfile(SCRIPTS / 'forge', executable)
            executable.chmod(0o755)
            result = subprocess.run([str(executable), 'context'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('unknown native entrypoint', result.stderr)

    def test_utilities_and_project_commands_run_without_shell_python_bun_or_cargo_in_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = dict(os.environ, PATH=str(root))
            evidence = root / 'evidence'
            evidence.write_text('Native runtime only')
            result = subprocess.run([str(SCRIPTS / 'native'), 'digest', str(evidence)],
                                    env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.strip(), hashlib.sha256(evidence.read_bytes()).hexdigest())
            plan = root / 'plan.json'
            plan.write_text(json.dumps({'commands': {'build': [{'argv': [shutil.which('printf'), 'native'], 'timeout_seconds': 2}]}}))
            result = subprocess.run([str(SCRIPTS / 'project-commands'), str(plan), 'build', directory, str(root / 'build.log')],
                                    env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('native', (root / 'build.log').read_text())


    def test_rebuild_activates_new_executable_while_previous_invocation_can_finish(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'toolchain').mkdir()
            compiler = subprocess.run(['rustc', '-vV'], capture_output=True, text=True, check=True)
            host = next(line.removeprefix('host: ') for line in compiler.stdout.splitlines() if line.startswith('host: '))
            target = root / 'runtime/target' / host / 'release'
            target.mkdir(parents=True)
            (root / 'runtime/Cargo.toml').write_text('[package]\nname = \"fixture\"\nversion = \"0.0.1\"\n')
            builder = SCRIPTS.parents[2] / 'runtime/target/debug/build-runtime'
            launcher = SCRIPTS.parents[2] / 'runtime/target' / host / 'release/pr-consolidate-launcher'
            shutil.copyfile(launcher, target / 'pr-consolidate-launcher')
            cargo = root / 'toolchain/cargo'
            cargo.write_text('#!/bin/sh\nexit 0\n')
            cargo.chmod(0o755)
            environment = dict(os.environ, PATH=str(root / 'toolchain') + os.pathsep + os.environ['PATH'])
            release = target / 'pr-consolidate'
            source = root / 'fixture.rs'
            source.write_text('use std::io::Read; fn main() { '
                'std::fs::write(std::env::args().nth(1).unwrap(), "ready").unwrap(); '
                'std::io::stdin().read_to_string(&mut String::new()).unwrap(); }')
            compiled = subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(release)],
                                      capture_output=True, text=True)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            built = subprocess.run([str(builder)], cwd=root,
                                   env=environment, capture_output=True, text=True)
            self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
            installed = next((root / 'skills/pr-consolidate/bin').glob('*/pr-consolidate'))
            ready = root / 'ready'
            running = subprocess.Popen([str(installed), str(ready)], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 3
                while not ready.exists() and running.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists())
                source.write_text('fn main() { std::process::exit(17); }')
                compiled = subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(release)],
                                          capture_output=True, text=True)
                self.assertEqual(compiled.returncode, 0, compiled.stderr)
                built = subprocess.run([str(builder)], cwd=root,
                                       env=environment, capture_output=True, text=True)
                self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
                running.communicate(input='', timeout=3)
                self.assertEqual(running.returncode, 0)
                result = subprocess.run([str(installed)], capture_output=True)
                self.assertEqual(result.returncode, 17, result.stderr)
            finally:
                if running.poll() is None:
                    running.kill()
                running.communicate(timeout=3)

    def test_failed_or_incomplete_native_build_does_not_replace_existing_entrypoints(self):
        builder = SCRIPTS.parents[2] / 'runtime/target/debug/build-runtime'
        for cargo_status, runtime_ready, expected in ((17, False, 17), (0, False, 2), (0, True, 2)):
            with self.subTest(cargo_status=cargo_status, runtime_ready=runtime_ready), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'runtime').mkdir()
                (root / 'runtime/Cargo.toml').write_text('[package]\nname = "fixture"\nversion = "0.0.1"\n')
                (root / 'toolchain').mkdir()
                cargo = root / 'toolchain/cargo'
                cargo.write_text(f'#!/bin/sh\nexit {cargo_status}\n')
                cargo.chmod(0o755)
                entrypoint = root / 'skills/pr-consolidate/scripts/native'
                entrypoint.parent.mkdir(parents=True)
                shutil.copyfile(SCRIPTS / 'native', entrypoint)
                previous = entrypoint.read_bytes()
                compiler = subprocess.run(['rustc', '-vV'], capture_output=True, text=True, check=True)
                host = next(line.removeprefix('host: ') for line in compiler.stdout.splitlines() if line.startswith('host: '))
                platform = {'aarch64-apple-darwin': 'darwin-arm64', 'x86_64-apple-darwin': 'darwin-x64',
                            'aarch64-unknown-linux-gnu': 'linux-arm64', 'x86_64-unknown-linux-gnu': 'linux-x64'}[host]
                installed = root / 'skills/pr-consolidate/bin' / platform / 'pr-consolidate'
                installed.parent.mkdir(parents=True)
                installed.write_bytes(b'previous runtime')
                if runtime_ready:
                    release = root / 'runtime/target' / host / 'release/pr-consolidate'
                    release.parent.mkdir(parents=True)
                    release.write_bytes(b'incomplete new runtime')
                environment = dict(os.environ, PATH=str(root / 'toolchain') + os.pathsep + os.environ['PATH'])
                result = subprocess.run([str(builder)], cwd=root, env=environment, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                self.assertEqual(entrypoint.read_bytes(), previous)
                self.assertEqual(installed.read_bytes(), b'previous runtime')
