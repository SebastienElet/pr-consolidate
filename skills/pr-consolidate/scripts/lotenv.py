import json
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import sys

from project_commands import validate

PATTERNS = {
    'lot': r'[a-z0-9][a-z0-9-]{0,30}',
    'branch': r'[A-Za-z0-9._/-]+',
    'base_branch': r'[A-Za-z0-9._/-]+',
    'base_sha': r'[0-9a-f]{40}',
    'author_name': r'[^\n]+',
    'author_email': r'[^@\s]+@[^@\s]+',
}
REQUIRED = set(PATTERNS) | {'root', 'source', 'remote_url', 'commands', 'prs'}
OPTIONAL = {'push_clone', 'artifacts', 'reference', 'lot_pr', 'note', 'hooks_path', 'lockfiles'}
AGENT_RE = re.compile(r'claude|codex|openai|anthropic', re.I)


def relative_path(value):
    return isinstance(value, str) and value and not PurePosixPath(value).is_absolute() and all(
        part not in ('', '.', '..') for part in value.split('/')) and '\x00' not in value


def absolute_path(value):
    return isinstance(value, str) and value.startswith('/') and all(
        part not in ('', '.', '..') for part in value[1:].split('/')) and '\x00' not in value


def validate_branch(branch):
    result = subprocess.run(['git', 'check-ref-format', '--branch', branch], capture_output=True)
    if result.returncode:
        raise ValueError('invalid Git branch name')


def validate_prs(prs, branch, base_branch):
    if not isinstance(prs, list) or not prs:
        raise ValueError('prs must be a non-empty list')
    ids = set()
    for pr in prs:
        if not isinstance(pr, dict) or set(pr) - {'id', 'head', 'branch', 'author', 'note'}:
            raise ValueError('bad PR entry')
        pid = str(pr.get('id', ''))
        head, source = pr.get('head', ''), pr.get('branch', '')
        if not re.fullmatch(r'[0-9]+', pid) or not isinstance(head, str) or not re.fullmatch(
                r'[0-9a-f]{40}', head) or not isinstance(source, str) or not re.fullmatch(
                PATTERNS['branch'], source):
            raise ValueError('PR requires numeric id, complete head SHA and source branch')
        validate_branch(source)
        if pid in ids or source in (branch, base_branch):
            raise ValueError('PR ids must be unique and source branches distinct from lot/base branches')
        ids.add(pid)


def load(path):
    lot = json.loads(Path(path).read_text(encoding='utf-8'))
    if not isinstance(lot, dict) or set(lot) - REQUIRED - OPTIONAL or REQUIRED - set(lot):
        raise ValueError('lot.json has unknown or missing keys')
    for key, pattern in PATTERNS.items():
        if not isinstance(lot[key], str) or not re.fullmatch(pattern, lot[key]):
            raise ValueError(f'invalid {key}')
    for key in ('branch', 'base_branch'):
        validate_branch(lot[key])
    if lot['branch'] == lot['base_branch']:
        raise ValueError('lot branch must differ from the main branch')
    for key in ('root', 'source', 'push_clone', 'artifacts', 'reference'):
        if key in lot and not absolute_path(lot[key]):
            raise ValueError(f'{key} must be an absolute path without parent components')
    root = Path(lot['root']).resolve()
    for key in ('source', 'push_clone'):
        if key in lot:
            value = Path(lot[key]).resolve()
            if value == root or not value.is_relative_to(root) or value == root / 'repo':
                raise ValueError(f'{key} must be a separate campaign-owned clone below root')
    if 'push_clone' in lot and Path(lot['push_clone']).resolve() == Path(lot['source']).resolve():
        raise ValueError('push_clone must differ from source')
    for key in ('author_name', 'author_email'):
        if AGENT_RE.search(lot[key]):
            raise ValueError(f'{key} names an agent')
    remote = lot['remote_url']
    if not isinstance(remote, str) or not remote or any(c.isspace() for c in remote) or '\x00' in remote:
        raise ValueError('invalid remote_url')
    if 'hooks_path' in lot and not relative_path(lot['hooks_path']):
        raise ValueError('hooks_path must be a project-relative path')
    files = lot.get('lockfiles', [])
    if not isinstance(files, list) or any(not relative_path(value) for value in files):
        raise ValueError('lockfiles must contain project-relative paths')
    if 'lot_pr' in lot and not re.fullmatch(r'[0-9]+', str(lot['lot_pr'])):
        raise ValueError('invalid lot_pr')
    validate_prs(lot['prs'], lot['branch'], lot['base_branch'])
    validate(lot['commands'])
    if not {'build', 'check'} <= set(lot['commands']):
        raise ValueError('project build and check commands are required')
    return lot


def main(args):
    try:
        if len(args) != 1:
            raise ValueError('usage: lotenv.py <lot.json>')
        lot = load(args[0])
        exports = {key.upper(): str(value) for key, value in lot.items()
                   if key in REQUIRED | OPTIONAL and isinstance(value, (str, int))}
        exports.update(R=f'{lot["root"]}/repo', LOGS=f'{lot["root"]}/logs',
                       PRS=' '.join(str(pr['id']) for pr in lot['prs']))
        for pr in lot['prs']:
            exports[f'PR_HEAD_{pr["id"]}'] = pr['head']
            exports[f'PR_BRANCH_{pr["id"]}'] = pr['branch']
        print('\n'.join(f'export {key}={shlex.quote(value)}' for key, value in exports.items()))
        return 0
    except (OSError, ValueError, TypeError) as exc:
        print(f'REFUSED lot.json: {exc}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
