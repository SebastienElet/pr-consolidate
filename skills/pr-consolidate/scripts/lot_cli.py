from pathlib import Path
import re
import sys

from lot_git import Campaign, Refused
from lotenv import load
from project_commands import redact, run_phase


def phase(campaign, name, tag):
    if not re.fullmatch(r'[A-Za-z0-9._-]+', tag):
        raise Refused('tag must contain only letters, digits, dots, underscores or hyphens')
    campaign.branch()
    campaign.clean()
    before = campaign.value('rev-parse', 'HEAD')
    log = campaign.logs / f'{name}-{tag}.log'
    status = run_phase(campaign.plan, name, campaign.repo, log)
    print(f'{name} head={before} exit={status} log={log}')
    if status:
        return status
    campaign.clean()
    if campaign.value('rev-parse', 'HEAD') != before:
        raise Refused('project command changed HEAD')
    return 0


def lockfile(campaign, base, tag):
    files = campaign.plan.get('lockfiles', [])
    if not files:
        raise Refused('project has no declared lockfiles')
    if not re.fullmatch(r'[A-Za-z0-9._-]+', tag):
        raise Refused('invalid tag')
    campaign.branch()
    campaign.value('rev-parse', '--verify', f'{base}^{{commit}}')
    campaign.git('checkout', base, '--', *files)
    return run_phase(campaign.plan, 'lockfile', campaign.repo, campaign.logs / f'lockfile-{tag}.log')


def merge_base(campaign, base, message):
    if not re.fullmatch(r'[0-9a-f]{40}', base):
        raise Refused('base SHA must have 40 hexadecimal digits')
    campaign.branch()
    campaign.no_merge()
    campaign.clean()
    current = campaign.value('rev-parse', '--verify', f'refs/remotes/origin/{campaign.plan["base_branch"]}^{{commit}}')
    if base != current:
        raise Refused('base SHA differs from the imported main branch; refresh snapshot first')
    campaign.ancestor(campaign.plan['base_sha'], base)
    result = campaign.git('merge', '--no-ff', '--no-commit', base, check=False)
    print(redact(result.stdout + result.stderr), end='')
    if campaign.git('rev-parse', '-q', '--verify', 'MERGE_HEAD', check=False).returncode:
        raise Refused('no merge started')
    unmerged = campaign.git('diff', '--name-only', '-z', '--diff-filter=U').stdout.split('\x00')[:-1]
    files = campaign.plan.get('lockfiles', [])
    if set(unmerged) - set(files):
        raise Refused('conflicts outside declared lockfiles; merge left in progress')
    if unmerged:
        status = lockfile(campaign, base, base[:8])
        if status:
            return status
        campaign.git('add', '--', *files)
    return campaign.commit(message, f'merge-base-{base[:8]}', [])


def execute(command, campaign, args):
    if command == 'bundle' and not args:
        campaign.snapshot()
    elif command == 'setup' and len(args) == 1:
        campaign.setup(args[0])
    elif command == 'merge' and len(args) == 1:
        return campaign.merge(args[0])
    elif command == 'push' and len(args) == 1:
        campaign.push(args[0])
    elif command == 'commit' and len(args) >= 2:
        if not re.fullmatch(r'[A-Za-z0-9._-]+', args[1]):
            raise Refused('invalid tag')
        return campaign.commit(args[0], args[1], args[2:])
    elif command == 'build' and len(args) == 1:
        return phase(campaign, 'build', args[0])
    elif command in ('preflight', 'resync', 'prepare') and not args:
        name = {'preflight': 'check', 'resync': 'reinstall', 'prepare': 'prepare'}[command]
        return phase(campaign, name, campaign.value('rev-parse', '--short', 'HEAD'))
    elif command == 'lockfile' and len(args) == 2:
        return lockfile(campaign, *args)
    elif command == 'merge-base' and len(args) == 2:
        return merge_base(campaign, *args)
    else:
        raise Refused(f'invalid arguments for lot-{command}')
    return 0


def main(args):
    try:
        if len(args) < 2:
            raise Refused('usage: lot_cli.py <operation> <lot.json> [arguments]')
        command, path, *values = args
        return execute(command, Campaign(load(path)), values)
    except (OSError, ValueError, TypeError) as exc:
        print(f'REFUSED: {redact(str(exc))}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
