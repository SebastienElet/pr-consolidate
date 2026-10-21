import json
import os
from pathlib import Path
import subprocess
import sys

from forge_fixture import BitbucketFixture, GitHubFixture

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'


def git(where, *args, check=True, environment=None):
    result = subprocess.run(['git', '-C', str(where), *args], capture_output=True, text=True, env=environment)
    if check and result.returncode:
        raise AssertionError(result.stderr)
    return result.stdout.strip()


class ForgeCLI:
    def __init__(self, root, forge='github', branch='main'):
        self.root = Path(root).resolve()
        self.work = self.root / 'work'
        self.origin = self.root / 'origin.git'
        git(self.root, 'init', '--bare', '-q', '-b', branch, str(self.origin))
        self.work.mkdir()
        git(self.work, 'init', '-q', '-b', branch)
        git(self.work, 'config', 'user.name', 'Fixture Maintainer')
        git(self.work, 'config', 'user.email', 'maintainer@example.invalid')
        git(self.work, 'commit', '-q', '--allow-empty', '-m', 'initial')
        git(self.work, 'remote', 'add', 'origin', str(self.origin))
        git(self.work, 'push', '-q', 'origin', branch)
        self.url = f'git@{"github.com" if forge == "github" else "bitbucket.org"}:workspace/project.git'
        git(self.work, 'remote', 'set-url', 'origin', self.url)
        git(self.work, 'config', f'url.{self.origin.as_uri()}.insteadOf', self.url)
        self.fixture = GitHubFixture() if forge == 'github' else BitbucketFixture()
        self.state_path = self.root / 'forge-state.json'
        self.write_state({'forge': forge, 'data': self.fixture.__dict__})
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        for name in ('gh', 'bkt'):
            executable = bin_dir / name
            executable.write_text(f'#!{sys.executable}\nimport sys\nsys.path.insert(0, {str(Path(__file__).parent)!r})\nfrom forge_fixture import main\nsys.exit(main())\n')
            executable.chmod(0o755)
        self.environment = dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ['PATH'], FORGE_FIXTURE_STATE=str(self.state_path))

    def state(self):
        return json.loads(self.state_path.read_text())

    def write_state(self, state):
        self.state_path.write_text(json.dumps(state))

    def save_fixture(self):
        state = self.state()
        state['data'] = self.fixture.__dict__
        self.write_state(state)

    def run(self, *args, directory=None, environment=None):
        return subprocess.run([str(SCRIPTS / 'forge'), *map(str, args)], cwd=directory or self.work,
                              env=environment or self.environment, capture_output=True, text=True, timeout=30)
