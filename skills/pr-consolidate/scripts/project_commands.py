import json
import os
from pathlib import Path
import signal
import subprocess
import sys

PHASES = {'prepare', 'build', 'check', 'reinstall', 'lockfile'}


def validate(commands):
    if not isinstance(commands, dict) or set(commands) - PHASES:
        raise ValueError('commands must map supported project phases to command lists')
    secret = os.environ.get('NPM_TOKEN', '')
    for phase, steps in commands.items():
        if not isinstance(steps, list) or (not steps and phase != 'build'):
            raise ValueError(f'{phase} must contain at least one command')
        for step in steps:
            if not isinstance(step, dict) or set(step) != {'argv', 'timeout_seconds'}:
                raise ValueError(f'{phase}: expected argv and timeout_seconds')
            argv, timeout = step['argv'], step['timeout_seconds']
            if not isinstance(argv, list) or not argv or any(
                    not isinstance(value, str) or not value or '\x00' in value for value in argv):
                raise ValueError(f'{phase}: argv must be a non-empty list of strings')
            if type(timeout) is not int or not 1 <= timeout <= 86400:
                raise ValueError(f'{phase}: timeout_seconds must be an integer from 1 to 86400')
            if secret and any(secret in value for value in argv):
                raise ValueError(f'{phase}: registry credentials belong only in the environment')
    return commands


def redact(value):
    secret = os.environ.get('NPM_TOKEN', '')
    return value.replace(secret, '[REDACTED]') if secret else value


def run_phase(plan, phase, directory, log):
    if not isinstance(plan, dict):
        raise ValueError('project plan must be a JSON object')
    commands = validate(plan.get('commands'))
    if phase not in commands:
        raise ValueError(f'project has no {phase} command; discover its documented native command first')
    log = Path(log)
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open('w', encoding='utf-8') as output:
        if not commands[phase]:
            output.write(f'{phase}: no project build step (explicitly declared)\n')
        for index, step in enumerate(commands[phase], 1):
            try:
                process = subprocess.Popen(step['argv'], cwd=directory, stdout=subprocess.PIPE,
                                           stderr=subprocess.STDOUT, start_new_session=True)
            except OSError as exc:
                output.write(redact(str(exc)) + '\n')
                return 2
            try:
                stdout, _ = process.communicate(timeout=step['timeout_seconds'])
                status = process.returncode
            except subprocess.TimeoutExpired as expired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    stdout, _ = process.communicate(timeout=0.25)
                except subprocess.TimeoutExpired as lingering:
                    stdout = lingering.output or expired.output or b''
                    process.stdout.close()
                    process.wait(timeout=0.25)
                status = 124
            output.write(redact(stdout.decode('utf-8', errors='replace')))
            output.write(f'\n{phase} step={index} exit={status}\n')
            if status:
                return status if status > 0 else 128 - status
    return 0


def main(args):
    try:
        if len(args) != 4:
            raise ValueError('usage: project_commands.py <lot.json> <phase> <directory> <log>')
        plan = json.loads(Path(args[0]).read_text(encoding='utf-8'))
        return run_phase(plan, *args[1:])
    except (OSError, ValueError, TypeError, subprocess.SubprocessError) as exc:
        print(f'REFUSED: {redact(str(exc))}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
