import json
import os
from pathlib import Path
import tempfile
import unittest

from cli_fixture import ForgeCLI, git


class RepositoryContextTests(unittest.TestCase):
    def test_current_nested_checkout_and_explicit_repository_use_remote_primary_not_local_head(self):
        for forge in ('github', 'bitbucket'):
            for branch in ('main', 'release/trunk'):
                with self.subTest(forge=forge, branch=branch), tempfile.TemporaryDirectory() as root:
                    cli = ForgeCLI(root, forge=forge, branch=branch)
                    nested = cli.work / 'nested'
                    nested.mkdir()
                    git(cli.work, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/stale')
                    for args, directory in [(('context',), nested), (('--repository', cli.work, 'context'), Path(root))]:
                        result = cli.run(*args, directory=directory)
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        context = json.loads(result.stdout)
                        self.assertEqual(context['root'], str(cli.work.resolve()))
                        self.assertEqual((context['repository'], context['forge'], context['base_branch']), ('workspace/project', forge, branch))
                    self.assertEqual(cli.state()['data']['calls'], [])

    def test_supported_ssh_https_and_hidden_repository_names(self):
        urls = ['https://bitbucket.org/Workspace/Project.git', 'git@bitbucket.org:Workspace/Project.git',
                'ssh://git@bitbucket.org/Workspace/Project.git', 'ssh://git@ssh.bitbucket.org:443/Workspace/Project.git',
                'git@github.com:owner/.dotfiles.git', 'https://github.com/owner/.dotfiles.git']
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            for url in urls:
                with self.subTest(url=url):
                    git(cli.work, 'remote', 'set-url', 'origin', url)
                    git(cli.work, 'config', f'url.{cli.origin.as_uri()}.insteadOf', url)
                    result = cli.run('context')
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(json.loads(result.stdout)['repository'], 'owner/.dotfiles' if 'dotfiles' in url else 'workspace/project')

    def test_unsupported_credential_malformed_traversing_and_padded_origins_refuse_without_api(self):
        urls = ['https://gitlab.com/workspace/project.git', 'file:///tmp/project.git',
                'https://tokenSECRET@bitbucket.org/workspace/project.git', 'https://user:SECRET@bitbucket.org/workspace/project.git',
                'ssh://git:SECRET@bitbucket.org/workspace/project.git', 'git@bitbucket.org:workspace/../project.git',
                'git@bitbucket.org:workspace/.git', 'https://bitbucket.org/workspace/project/extra.git',
                'https://bitbucket.org/../project.git', 'https://bitbucket.org/workspace/%2e%2e.git',
                'https://bitbucket.org/workspace/project.git?SECRET', 'https://bitbucket.org/workspace/project.git#SECRET',
                'https://bitbucket.org:invalid/x/y', 'git@bitbucket.org:workspace/project.git/',
                'ssh://git@ssh.bitbucket.org:22/workspace/project.git', 'https://bitbucket.org//workspace/project.git']
        urls.extend('https://bitbucket.org/workspace/project.git' + suffix for suffix in (' ', '\n', '\t'))
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            for url in urls:
                with self.subTest(url=url):
                    git(cli.work, 'remote', 'set-url', 'origin', url)
                    out = Path(root) / 'inventory.json'
                    result = cli.run('inventory', '--out', out)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                    self.assertNotIn('SECRET', result.stdout + result.stderr)
                    self.assertFalse(out.exists())
            self.assertEqual(cli.state()['data']['calls'], [])

    def test_missing_multiple_and_foreign_push_origins_refuse_before_api(self):
        for fault in ('missing', 'multiple', 'push'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as root:
                cli = ForgeCLI(root)
                if fault == 'missing':
                    git(cli.work, 'remote', 'remove', 'origin')
                elif fault == 'multiple':
                    git(cli.work, 'config', '--add', 'remote.origin.url', 'https://github.com/other/repo.git')
                else:
                    git(cli.work, 'config', 'remote.origin.pushurl', 'https://github.com/other/repo.git')
                result = cli.run('context')
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertEqual(cli.state()['data']['calls'], [])

    def test_git_environment_cannot_redirect_discovery_or_inject_configuration(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as other:
            cli = ForgeCLI(root, branch='main')
            foreign = ForgeCLI(other, branch='trunk')
            environment = dict(cli.environment, GIT_DIR=str(foreign.work / '.git'), GIT_WORK_TREE=str(foreign.work),
                               GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='remote.origin.url',
                               GIT_CONFIG_VALUE_0='https://github.com/foreign/repository.git')
            result = cli.run('context', environment=environment)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads(result.stdout)['root'], str(cli.work.resolve()))
            self.assertEqual(json.loads(result.stdout)['base_branch'], 'main')

    def test_unresolvable_detached_and_failed_remote_head_queries_refuse(self):
        for fault in ('unborn', 'detached', 'missing'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as root:
                cli = ForgeCLI(root)
                if fault == 'unborn':
                    git(cli.origin, 'symbolic-ref', 'HEAD', 'refs/heads/missing')
                elif fault == 'detached':
                    git(cli.origin, 'update-ref', '--no-deref', 'HEAD', git(cli.work, 'rev-parse', 'HEAD'))
                else:
                    cli.origin.rename(Path(root) / 'gone.git')
                result = cli.run('context')
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_successive_repository_invocations_do_not_retain_previous_identity(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as other:
            for directory, branch in ((root, 'main'), (other, 'trunk')):
                cli = ForgeCLI(directory, branch=branch)
                self.assertEqual(json.loads(cli.run('context').stdout)['base_branch'], branch)
