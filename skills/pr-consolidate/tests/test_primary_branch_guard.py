import tempfile
import unittest

from project_fixture import fixture, git, run, save, setup


def switch_primary(root, plan):
    git(root, '--git-dir=' + plan['remote_url'], 'update-ref', 'refs/heads/main', plan['base_sha'])
    git(root, '--git-dir=' + plan['remote_url'], 'symbolic-ref', 'HEAD', 'refs/heads/main')


class PrimaryBranchGuardTests(unittest.TestCase):
    def test_snapshot_refuses_a_plan_that_names_another_base_and_the_real_primary_as_lot(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            switch_primary(root, plan)
            plan['branch'] = 'main'
            save(path, plan)
            result = run(path, 'bundle')
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('primary branch', result.stderr)
            self.assertFalse((root / 'artifacts/fixture.bundle').exists())

    def test_setup_refuses_remote_primary_change_before_import_or_clone(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            self.assertEqual(run(path, 'bundle').returncode, 0)
            switch_primary(root, plan)
            plan['branch'] = 'main'
            save(path, plan)
            result = run(path, 'setup', str(root / 'artifacts/fixture.bundle'))
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertFalse((root / 'repo').exists())
            self.assertEqual(git(root / 'source', 'for-each-ref', '--format=%(refname)',
                                 'refs/lots/fixture'), '')

    def test_local_mutation_refuses_primary_change_after_setup(self):
        for operation in ('merge', 'commit'):
            with self.subTest(operation=operation), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                setup(path, root)
                switch_primary(root, plan)
                before = git(root / 'repo', 'rev-parse', 'HEAD')
                if operation == 'commit':
                    (root / 'repo/base.txt').write_text('staged candidate\n')
                    git(root / 'repo', 'add', 'base.txt')
                status_before = git(root / 'repo', 'status', '--porcelain')
                message = root / 'message.txt'
                message.write_text('must not commit')
                args = ['1'] if operation == 'merge' else [str(message), 'unsafe']
                result = run(path, operation, *args)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn('primary branch', result.stderr)
                self.assertEqual(git(root / 'repo', 'rev-parse', 'HEAD'), before)
                self.assertEqual(git(root / 'repo', 'status', '--porcelain'), status_before)

    def test_push_does_not_move_the_real_primary_when_plan_base_is_false(self):
        with tempfile.TemporaryDirectory() as directory:
            root, path, plan = fixture(directory)
            setup(path, root)
            repo = root / 'repo'
            git(repo, 'commit', '-q', '--allow-empty', '-m', 'candidate')
            candidate = git(repo, 'rev-parse', 'HEAD')
            git(repo, 'branch', '-m', 'main')
            switch_primary(root, plan)
            plan['branch'] = 'main'
            save(path, plan)
            result = run(path, 'push', candidate)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('primary branch', result.stderr)
            self.assertEqual(git(root, '--git-dir=' + plan['remote_url'], 'rev-parse', 'refs/heads/main'),
                             plan['base_sha'])

    def test_snapshot_refuses_unresolvable_or_nonsymbolic_remote_head(self):
        for head in ('missing', 'detached'):
            with self.subTest(head=head), tempfile.TemporaryDirectory() as directory:
                root, path, plan = fixture(directory)
                if head == 'missing':
                    git(root, '--git-dir=' + plan['remote_url'], 'symbolic-ref', 'HEAD', 'refs/heads/missing')
                else:
                    git(root, '--git-dir=' + plan['remote_url'], 'update-ref', '--no-deref', 'HEAD', plan['base_sha'])
                result = run(path, 'bundle')
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn('primary branch', result.stderr)


if __name__ == '__main__':
    unittest.main()
