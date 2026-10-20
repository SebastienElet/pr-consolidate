import json
from pathlib import Path
import subprocess
import sys

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'


def git(where, *args, check=True):
    result = subprocess.run(['git', *args], cwd=where, text=True, capture_output=True)
    if check and result.returncode:
        raise AssertionError(result.stderr)
    return result.stdout.strip()


def fixture(directory):
    root = Path(directory).resolve()
    origin, seed = root / 'origin.git', root / 'seed'
    git(root, 'init', '--bare', '-q', '--initial-branch=trunk', str(origin))
    git(root, 'clone', '-q', str(origin), str(seed))
    git(seed, 'config', 'user.name', 'Fixture Reviewer')
    git(seed, 'config', 'user.email', 'fixture@example.invalid')
    (seed / 'base.txt').write_text('base\n')
    (seed / '.gitignore').write_text('build.marker\ncheck.marker\n')
    (seed / '.hooks').mkdir()
    hook = seed / '.hooks/pre-commit'
    hook.write_text('#!/bin/sh\nprintf hook > .git/hook-ran\n')
    hook.chmod(0o755)
    git(seed, 'add', '.')
    git(seed, 'commit', '-qm', 'initial')
    base = git(seed, 'rev-parse', 'HEAD')
    prs = []
    for number in (1, 2):
        git(seed, 'checkout', '-qb', f'feature/{number}', base)
        (seed / f'change-{number}.txt').write_text(f'change {number}\n')
        git(seed, 'add', '.')
        git(seed, 'commit', '-qm', f'change {number}')
        prs.append({'id': number, 'head': git(seed, 'rev-parse', 'HEAD'), 'branch': f'feature/{number}'})
    git(seed, 'push', '-q', 'origin', 'trunk', 'feature/1', 'feature/2')
    git(root, 'clone', '-q', str(origin), str(root / 'source'))
    git(root, 'clone', '-q', str(origin), str(root / 'push'))
    success = [{'argv': [sys.executable, '-c', 'print("native project check")'], 'timeout_seconds': 3}]
    plan = {'lot': 'fixture', 'root': str(root), 'branch': 'batch/example', 'base_branch': 'trunk',
            'base_sha': base, 'remote_url': str(origin), 'source': str(root / 'source'),
            'push_clone': str(root / 'push'), 'artifacts': str(root / 'artifacts'),
            'author_name': 'Human Reviewer', 'author_email': 'human@example.invalid',
            'hooks_path': '.hooks', 'commands': {'build': success, 'check': success}, 'prs': prs}
    path = root / 'lot.json'
    path.write_text(json.dumps(plan))
    return root, path, plan


def run(path, operation, *args, environment=None):
    return subprocess.run(['/bin/bash', str(SCRIPTS / f'lot-{operation}.sh'), str(path), *args],
                          text=True, capture_output=True, env=environment)


def setup(path, root):
    result = run(path, 'bundle')
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    result = run(path, 'setup', str(root / 'artifacts/fixture.bundle'))
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)


def save(path, plan):
    path.write_text(json.dumps(plan))
