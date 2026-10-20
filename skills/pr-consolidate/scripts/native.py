import hashlib
import os
from pathlib import Path
import sys


def digest(path):
    hasher = hashlib.sha256()
    handle = sys.stdin.buffer if path == '-' else Path(path).open('rb')
    try:
        for chunk in iter(lambda: handle.read(1024 * 1024), b''):
            hasher.update(chunk)
    finally:
        if path != '-':
            handle.close()
    return hasher.hexdigest()


def manifest(root, verify=False):
    root = Path(root)
    path = root / 'SHA256SUMS'
    if verify:
        for line in path.read_text(encoding='utf-8').splitlines():
            expected, relative = line.split('  ', 1)
            target = (root / relative).resolve()
            if not target.is_relative_to(root.resolve()) or digest(str(target)) != expected:
                raise ValueError(f'archive digest mismatch: {relative}')
    else:
        files = sorted(p for p in root.rglob('*') if p.is_file() and p != path)
        path.write_text(''.join(f'{digest(str(p))}  ./{p.relative_to(root)}\n' for p in files), encoding='utf-8')


def main(args):
    try:
        command, *values = args
        if command == 'path' and len(values) == 1:
            print(Path(values[0]).resolve())
        elif command == 'digest' and len(values) == 1:
            print(digest(values[0]))
        elif command in ('manifest', 'verify') and len(values) == 1:
            manifest(values[0], verify=command == 'verify')
        elif command == 'filter' and len(values) == 1:
            token = os.environ.get('NPM_TOKEN', '')
            for line in Path(values[0]).read_text(encoding='utf-8', errors='replace').splitlines():
                if 'authtoken' not in line.lower() and (not token or token not in line):
                    print(line)
        else:
            raise ValueError('usage: native.py path|digest|manifest|verify|filter <path>')
    except (OSError, ValueError) as exc:
        print(f'REFUSED: {exc}', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
