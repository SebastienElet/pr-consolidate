import json
from pathlib import Path
import tempfile
import unittest

from cli_fixture import ForgeCLI

HEAD = 'a' * 40


class ForgeCLITests(unittest.TestCase):
    def body(self, root, content='Findings: no demonstrated defect. Evidence: fixture checks.'):
        body = Path(root) / 'review.md'
        body.write_text(content)
        return body

    def test_github_inventory_create_description_and_review_cross_native_cli(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            body = self.body(root)
            for args in [('inventory', '--out', Path(root) / 'inventory.json'),
                         ('create', 'batch/new', 'Batch', body),
                         ('review-comment', 800, HEAD, body, 'dry-run'),
                         ('review-comment', 800, HEAD, body, 'apply'),
                         ('review-comment', 800, HEAD, body, 'apply')]:
                result = cli.run(*args)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            old = self.body(root, 'Original description')
            new = Path(root) / 'new.md'
            new.write_text('Updated description')
            result = cli.run('desc', 800, old, new, '--check')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = cli.run('desc', 800, old, new)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = cli.state()
            writes = [c for c in state['data']['calls'] if c[0] != 'GET']
            self.assertEqual([c[0] for c in writes], ['POST', 'POST', 'PATCH'])
            self.assertEqual(len(state['data']['comments']['800']), 1)
            inventory = json.loads((Path(root) / 'inventory.json').read_text())
            self.assertEqual(inventory['prs'][0]['paths'], ['src/current.py', 'src/previous.py'])
            self.assertTrue(all('--hostname' in argv and 'github.com' in argv for argv in state['argv']))
            self.assertTrue(all(argv[argv.index('--input') + 1] == '-' for argv in state['argv'] if '--input' in argv))

    def test_bitbucket_create_preserves_literal_body_and_targets_remote_primary(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root, forge='bitbucket', branch='release/trunk')
            body = self.body(root, 'Literal $(touch injected) `not-a-command` and "quotes". Unicode: café.')
            result = cli.run('create', 'batch/new', 'Batch', body)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = cli.state()
            created = state['data']['prs']['901']
            self.assertEqual(created['destination']['branch']['name'], 'release/trunk')
            self.assertFalse(created['close_source_branch'])
            self.assertEqual(created['description'], body.read_text())
            self.assertFalse((cli.work / 'injected').exists())
            self.assertEqual(state['argv'][-1][1], '/repositories/workspace/project/pullrequests')

    def test_github_review_summary_updates_existing_surface_without_approval(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            marker = f'<!-- pr-consolidate:review pr=800 head={HEAD} -->'
            cli.fixture.reviews[800] = [cli.fixture.comment(71, marker + '\nOriginal review', state='COMMENTED', commit_id=HEAD)]
            cli.save_fixture()
            result = cli.run('review-comment', 800, HEAD, self.body(root), 'apply')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = cli.state()
            writes = [c for c in state['data']['calls'] if c[0] != 'GET']
            self.assertEqual(writes[0][0:2], ['PUT', '/repos/workspace/project/pulls/800/reviews/71'])
            self.assertEqual(set(writes[0][2]), {'body'})

    def test_github_later_comment_page_updates_existing_comment_and_file_count_is_checked(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            cli.fixture.comments[800] = [cli.fixture.comment(n, 'Unrelated') for n in range(1, 101)]
            cli.fixture.comments[800].append(cli.fixture.comment(101, f'<!-- pr-consolidate:review pr=800 head={HEAD} -->\nReview'))
            cli.save_fixture()
            result = cli.run('review-comment', 800, HEAD, self.body(root), 'apply')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(len(cli.state()['data']['comments']['800']), 101)
            state = cli.state()
            state['data']['prs']['800']['changed_files'] = 2
            cli.write_state(state)
            out = Path(root) / 'inventory.json'
            result = cli.run('inventory', '--out', out)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertFalse(out.exists())

    def test_github_native_replacement_observes_reviews_ancestry_notice_and_close(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            body = self.body(root)
            self.assertEqual(cli.run('review-comment', 800, HEAD, body, 'apply').returncode, 0)
            plan = Path(root) / 'replacement.json'
            plan.write_text(json.dumps({'lot_pr': 900, 'items': [{'id': 800, 'head': HEAD}]}))
            for command, mode in [('replace', 'dry-run'), ('decline', 'apply'), ('replace', 'apply')]:
                result = cli.run(command, plan, mode)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            writes = [c for c in cli.state()['data']['calls'] if c[0] != 'GET']
            self.assertEqual([c[0] for c in writes], ['POST', 'POST', 'PATCH'])
            self.assertEqual(cli.state()['data']['prs']['800']['state'], 'closed')

    def test_accepted_cli_write_failure_is_reconciled_without_duplicate_comment(self):
        for forge in ('github', 'bitbucket'):
            with self.subTest(forge=forge), tempfile.TemporaryDirectory() as root:
                cli = ForgeCLI(root, forge=forge)
                if forge == 'bitbucket':
                    cli.fixture.prs['800'] = {'id': 800, 'state': 'OPEN', 'source': {'commit': {'hash': HEAD}}, 'destination': {'branch': {'name': 'main'}}}
                    cli.save_fixture()
                state = cli.state()
                state['accepted_write_failure'] = True
                cli.write_state(state)
                body = self.body(root)
                result = cli.run('review-comment', 800, HEAD, body, 'apply')
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertNotIn('private-cli-output', result.stdout + result.stderr)
                result = cli.run('review-comment', 800, HEAD, body, 'apply')
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                writes = [c for c in cli.state()['data']['calls'] if c[0] != 'GET']
                self.assertEqual(len(writes), 1)

    def test_transport_failure_and_invalid_json_do_not_expose_cli_output(self):
        for forge in ('github', 'bitbucket'):
            for mode in ('invalid_output', 'exit_code'):
                with self.subTest(forge=forge, mode=mode), tempfile.TemporaryDirectory() as root:
                    cli = ForgeCLI(root, forge=forge)
                    state = cli.state()
                    state[mode] = 23 if mode == 'exit_code' else True
                    cli.write_state(state)
                    result = cli.run('inventory', '--out', Path(root) / 'out.json')
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertNotIn('private-cli-output', result.stdout + result.stderr)
                    self.assertFalse((Path(root) / 'out.json').exists())

    def test_argument_errors_refuse_before_discovery_or_forge_access(self):
        with tempfile.TemporaryDirectory() as root:
            cli = ForgeCLI(root)
            for args in [('review-comment', '0', HEAD, 'body', 'apply'), ('review-comment', '1', HEAD, 'body', 'unknown'),
                         ('replace', 'plan'), ('watch', 'true'), ('inventory',)]:
                result = cli.run(*args, directory=root)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertEqual(cli.state()['data']['calls'], [])
