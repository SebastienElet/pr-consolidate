import os
from pathlib import Path
import re
import subprocess

from lotenv import AGENT_RE
from project_commands import redact


class Refused(ValueError):
    pass


def git_environment(plan):
    redirects = {'GIT_DIR', 'GIT_COMMON_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE',
                 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_NAMESPACE',
                 'GIT_SHALLOW_FILE', 'GIT_REPLACE_REF_BASE'}
    environment = {key: value for key, value in os.environ.items()
                   if key not in redirects and not key.startswith('GIT_CONFIG')}
    environment.update(GIT_AUTHOR_NAME=plan['author_name'], GIT_COMMITTER_NAME=plan['author_name'],
                       GIT_AUTHOR_EMAIL=plan['author_email'], GIT_COMMITTER_EMAIL=plan['author_email'])
    return environment


class Campaign:
    def __init__(self, plan):
        self.plan = plan
        self.root = Path(plan['root'])
        self.repo = self.root / 'repo'
        self.logs = self.root / 'logs'
        self.logs.mkdir(parents=True, exist_ok=True)
        self.env = git_environment(plan)

    def owned(self, where):
        directory = Path(where)
        if directory.is_symlink():
            raise Refused('campaign clone may not be a symbolic link')
        common = self.value('rev-parse', '--git-common-dir', where=where)
        gitdir = (directory / common).resolve()
        if not gitdir.is_relative_to(self.root.resolve()):
            raise Refused('campaign clone refers to a Git directory outside root')

    def git(self, *args, where=None, check=True):
        try:
            result = subprocess.run(['git', *args], cwd=where or self.repo, env=self.env,
                                    capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired as exc:
            raise Refused('Git operation timed out') from exc
        if check and result.returncode:
            raise Refused(redact(result.stderr.strip() or result.stdout.strip() or 'git command failed'))
        return result

    def value(self, *args, where=None):
        return self.git(*args, where=where).stdout.strip()

    def clean(self, untracked=True):
        status = self.value('status', '--porcelain', '--untracked-files=normal' if untracked else '--untracked-files=no')
        if status:
            raise Refused(f'dirty tree:\n{status}')

    def branch(self):
        self.origin(self.repo)
        if self.value('symbolic-ref', '-q', 'HEAD') != f'refs/heads/{self.plan["branch"]}':
            raise Refused('HEAD is not on the lot branch')

    def no_merge(self):
        if self.git('rev-parse', '-q', '--verify', 'MERGE_HEAD', check=False).returncode == 0:
            raise Refused('a merge is in progress')

    def origin(self, where):
        self.owned(where)
        urls = self.value('remote', 'get-url', '--all', 'origin', where=where).splitlines()
        pushes = self.value('remote', 'get-url', '--push', '--all', 'origin', where=where).splitlines()
        if urls != [self.plan['remote_url']] or pushes != [self.plan['remote_url']]:
            raise Refused('origin fetch/push URL differs from the campaign remote_url')
        self.primary(where)

    def primary(self, where):
        output = self.value('ls-remote', '--symref', 'origin', 'HEAD', where=where)
        branches = re.findall(r'^ref: refs/heads/([^\t\n]+)\tHEAD$', output, re.MULTILINE)
        heads = re.findall(r'^[0-9a-f]{40}\tHEAD$', output, re.MULTILINE)
        if len(branches) != 1 or len(heads) != 1:
            raise Refused('origin HEAD does not declare one resolvable primary branch')
        actual = branches[0]
        self.git('check-ref-format', '--branch', actual, where=where)
        if self.plan['branch'] == actual:
            raise Refused('lot branch is the actual remote primary branch')
        if self.plan['base_branch'] != actual:
            raise Refused('plan base_branch differs from the actual remote primary branch')

    def heads(self, where, base_ref, pr_ref):
        if self.value('rev-parse', '--verify', f'{base_ref}^{{commit}}', where=where) != self.plan['base_sha']:
            raise Refused('main branch moved from the reviewed base SHA')
        for pr in self.plan['prs']:
            ref = pr_ref.format(id=pr['id'], branch=pr['branch'])
            if self.value('rev-parse', '--verify', f'{ref}^{{commit}}', where=where) != pr['head']:
                raise Refused(f'PR {pr["id"]} moved from its reviewed head')

    def ancestor(self, base, head, where=None):
        if self.git('merge-base', '--is-ancestor', base, head, where=where, check=False).returncode:
            raise Refused(f'{head} does not contain {base}')

    def snapshot(self):
        where = self.plan.get('push_clone')
        if not where:
            raise Refused('push_clone is required for a remote snapshot')
        self.origin(where)
        specs = [f'+refs/heads/{self.plan["base_branch"]}:refs/remotes/origin/{self.plan["base_branch"]}']
        specs += [f'+refs/heads/{pr["branch"]}:refs/remotes/origin/{pr["branch"]}' for pr in self.plan['prs']]
        self.git('fetch', '--no-prune', '--no-tags', 'origin', *specs, where=where)
        self.heads(where, f'refs/remotes/origin/{self.plan["base_branch"]}', 'refs/remotes/origin/{branch}')
        self.git('update-ref', 'refs/lots/base', self.plan['base_sha'], where=where)
        refs = ['refs/lots/base']
        for pr in self.plan['prs']:
            ref = f'refs/lots/pr/{pr["id"]}'
            self.git('update-ref', ref, pr['head'], where=where)
            refs.append(ref)
        artifacts = Path(self.plan.get('artifacts', self.logs))
        artifacts.mkdir(parents=True, exist_ok=True)
        bundle = artifacts / f'{self.plan["lot"]}.bundle'
        self.git('bundle', 'create', str(bundle), *refs, where=where)
        self.git('bundle', 'verify', str(bundle), where=where)
        print(bundle)

    def setup(self, bundle):
        source = self.plan['source']
        self.origin(source)
        if self.repo.is_symlink():
            raise Refused('campaign repo may not be a symbolic link')
        self.git('bundle', 'verify', str(Path(bundle).resolve()), where=source)
        self.git('fetch', '--no-prune', '--no-tags', str(Path(bundle).resolve()),
                 f'+refs/lots/*:refs/lots/{self.plan["lot"]}/*', where=source)
        prefix = f'refs/lots/{self.plan["lot"]}'
        self.heads(source, f'{prefix}/base', prefix + '/pr/{id}')
        if self.repo.exists():
            if Path(self.value('rev-parse', '--show-toplevel')).resolve() != self.repo.resolve():
                raise Refused('campaign repo is not a repository root')
            self.clean()
            self.no_merge()
            self.origin(self.repo)
        else:
            self.git('clone', '--no-checkout', '--no-hardlinks', source, str(self.repo), where=self.root)
            self.git('remote', 'set-url', 'origin', self.plan['remote_url'])
        self.git('fetch', '--no-prune', '--no-tags', source,
                 f'+{prefix}/base:refs/remotes/origin/{self.plan["base_branch"]}',
                 *[f'+{prefix}/pr/{pr["id"]}:refs/remotes/pr/{pr["id"]}' for pr in self.plan['prs']])
        self.heads(self.repo, f'refs/remotes/origin/{self.plan["base_branch"]}', 'refs/remotes/pr/{id}')
        self.git('config', 'user.name', self.plan['author_name'])
        self.git('config', 'user.email', self.plan['author_email'])
        if 'hooks_path' in self.plan:
            self.git('config', 'core.hooksPath', self.plan['hooks_path'])
        branch = self.plan['branch']
        exists = self.git('rev-parse', '--verify', f'refs/heads/{branch}', check=False).returncode == 0
        if exists:
            self.ancestor(self.plan['base_sha'], f'refs/heads/{branch}')
            self.git('checkout', branch)
        else:
            self.git('checkout', '-b', branch, self.plan['base_sha'])
        print(self.value('rev-parse', 'HEAD'))

    def merge(self, pid):
        pr = next((pr for pr in self.plan['prs'] if str(pr['id']) == pid), None)
        if pr is None:
            raise Refused('PR is not in this lot')
        self.branch()
        self.no_merge()
        self.clean()
        head = self.value('rev-parse', '--verify', f'refs/remotes/pr/{pid}^{{commit}}')
        if head != pr['head']:
            raise Refused('PR ref moved from reviewed head')
        result = self.git('merge', '--no-ff', '--no-commit', head, check=False)
        print(redact(result.stdout + result.stderr), end='')
        return result.returncode

    def staged(self):
        if self.value('diff', '--name-only', '--diff-filter=U'):
            raise Refused('unmerged paths')
        if self.value('diff', '--name-only'):
            raise Refused('unstaged changes')
        if self.value('ls-files', '--others', '--exclude-standard'):
            raise Refused('untracked files')
        paths = self.git('diff', '--cached', '--name-only', '-z', '--diff-filter=ACMR').stdout.split('\x00')[:-1]
        if paths:
            result = self.git('--literal-pathspecs', 'grep', '--cached', '-I', '-l', '-E',
                              '^(<<<<<<<|>>>>>>>)( |$)', '--', *paths, check=False)
            if result.returncode == 0:
                raise Refused('conflict markers staged')
            if result.returncode != 1:
                raise Refused('conflict marker inspection failed')

    def commit(self, message, tag, paths):
        from project_commands import run_phase
        self.branch()
        if paths:
            self.git('add', '--', *paths)
        if self.git('diff', '--cached', '--quiet', check=False).returncode == 0 and self.git(
                'rev-parse', '-q', '--verify', 'MERGE_HEAD', check=False).returncode:
            raise Refused('nothing staged')
        self.staged()
        for ident in ('GIT_AUTHOR_IDENT', 'GIT_COMMITTER_IDENT'):
            if AGENT_RE.search(self.value('var', ident)):
                raise Refused('commit identity names an agent')
        log = self.logs / f'build-{tag}.log'
        status = run_phase(self.plan, 'build', self.repo, log)
        print(f'build exit={status} log={log}')
        if status:
            return status
        self.staged()
        result = self.git('commit', '-F', str(Path(message).resolve()), check=False)
        print(redact(result.stdout + result.stderr), end='')
        if not result.returncode and AGENT_RE.search(self.value('log', '-1', '--format=%an <%ae>%n%cn <%ce>')):
            raise Refused('created commit names an agent; do not push')
        return result.returncode

    def push(self, candidate):
        where = self.plan.get('push_clone')
        if not where or not re.fullmatch(r'[0-9a-f]{40}', candidate):
            raise Refused('push requires push_clone and a complete candidate SHA')
        self.origin(where)
        ref = f'refs/lots/candidate/{self.plan["lot"]}'
        self.git('fetch', '--no-prune', '--no-tags', str(self.repo), f'+refs/heads/{self.plan["branch"]}:{ref}', where=where)
        if self.value('rev-parse', ref, where=where) != candidate:
            raise Refused('candidate SHA differs from the lot branch')
        self.ancestor(self.plan['base_sha'], candidate, where)
        identities = self.value('log', '--format=%an <%ae>%n%cn <%ce>',
                                f'{self.plan["base_sha"]}..{candidate}', where=where)
        if not identities or AGENT_RE.search(identities):
            raise Refused('candidate has no new commits or names an agent')
        branch = self.plan['branch']
        remote = self.value('ls-remote', 'origin', f'refs/heads/{branch}', where=where)
        if remote:
            previous = remote.split()[0]
            self.git('fetch', '--no-prune', '--no-tags', 'origin', f'+refs/heads/{branch}:refs/remotes/origin/{branch}', where=where)
            self.ancestor(previous, candidate, where)
        self.origin(where)
        self.git('push', 'origin', f'{candidate}:refs/heads/{branch}', where=where)
        after = self.value('ls-remote', 'origin', f'refs/heads/{branch}', where=where)
        if not after or after.split()[0] != candidate:
            raise Refused('remote branch does not match pushed candidate')
        print(candidate)
