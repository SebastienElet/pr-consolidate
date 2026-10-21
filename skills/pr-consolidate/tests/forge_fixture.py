import copy
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import parse_qs, urlsplit

HEAD = "a" * 40
BATCH = "b" * 40
MERGE = "c" * 40


class GitHubFixture:
    def __init__(self):
        self.prs = {800: self.pull(800), 900: self.pull(900, head=BATCH, merged=True)}
        self.comments, self.reviews, self.calls, self.runs, self.statuses = {}, {}, [], [], []
        self.user, self.ancestor, self.after_comments = {"id": 7, "login": "maintainer"}, HEAD, None

    @staticmethod
    def pull(number, head=HEAD, merged=False):
        return {"id": number + 1000, "number": number, "title": "Batch fixture", "body": "Original description",
                "user": {"id": 9, "login": "author"}, "head": {"sha": head, "ref": f"feat/{number}"},
                "base": {"ref": "main"}, "state": "closed" if merged else "open", "draft": False,
                "merged": merged, "merged_at": "2026-01-01T00:00:00Z" if merged else None,
                "merge_commit_sha": MERGE if merged else None, "changed_files": 1}

    @staticmethod
    def comment(cid, body, user=7, **fields):
        return {"id": cid, "body": body, "user": {"id": user, "login": "maintainer"}, **fields}

    def __call__(self, path, method="GET", payload=None, timeout=180):
        self.calls.append((method, path, copy.deepcopy(payload)))
        parsed = urlsplit(path)
        route = parsed.path.removeprefix("/repos/workspace/project/")
        query = parse_qs(parsed.query)
        page = int(query.get("page", [1])[0])
        if path == "/user":
            return copy.deepcopy(self.user)
        if route == "pulls" and method == "GET":
            values = [p for p in self.prs.values() if p["state"] == "open"]
            return copy.deepcopy(values[(page - 1) * 100:page * 100])
        if route == "pulls" and method == "POST":
            pr = self.pull(901)
            pr.update(title=payload["title"], body=payload["body"])
            pr["head"]["ref"], pr["base"]["ref"] = payload["head"], payload["base"]
            self.prs[901] = pr
            return copy.deepcopy(pr)
        match = re.fullmatch(r"pulls/(\d+)", route)
        if match:
            pr = self.prs[int(match[1])]
            if method == "PATCH":
                pr.update(payload)
            return copy.deepcopy(pr)
        match = re.fullmatch(r"pulls/(\d+)/files", route)
        if match:
            return [{"sha": "d" * 40, "filename": "src/current.py", "previous_filename": "src/previous.py",
                     "status": "renamed", "additions": 3, "deletions": 1}]
        match = re.fullmatch(r"(issues|pulls)/(\d+)/(comments|reviews)(?:/(\d+))?", route)
        if match:
            number, surface = int(match[2]), match[3]
            values = (self.comments if surface == "comments" else self.reviews).setdefault(number, [])
            if method == "GET":
                result = copy.deepcopy(values[(page - 1) * 100:page * 100])
                if self.after_comments:
                    self.after_comments()
                return result
            if match[4]:
                next(v for v in values if v["id"] == int(match[4]))["body"] = payload["body"]
            else:
                values.append(self.comment(max([v["id"] for v in values], default=0) + 1, payload["body"]))
            return copy.deepcopy(values[-1])
        match = re.fullmatch(r"issues/comments/(\d+)", route)
        if match and method == "PATCH":
            comment = next(v for values in self.comments.values() for v in values if v["id"] == int(match[1]))
            comment["body"] = payload["body"]
            return copy.deepcopy(comment)
        if re.fullmatch(r"compare/[0-9a-f]+\.\.\.[0-9a-f]+", route):
            return {"merge_base_commit": {"sha": self.ancestor}}
        if re.fullmatch(r"commits/[0-9a-f]+/check-runs", route):
            return {"total_count": len(self.runs), "check_runs": copy.deepcopy(self.runs[(page - 1) * 100:page * 100])}
        if re.fullmatch(r"commits/[0-9a-f]+/statuses", route):
            return copy.deepcopy(self.statuses[(page - 1) * 100:page * 100])
        raise AssertionError(f"unexpected GitHub call {method} {path}")

    def writes(self):
        return [call for call in self.calls if call[0] != "GET"]



class BitbucketFixture:
    def __init__(self):
        self.prs, self.comments, self.calls = {}, {}, []
        self.user = {'uuid': '{11111111-2222-3333-4444-555555555555}'}

    def __call__(self, path, method='GET', payload=None, timeout=180):
        self.calls.append((method, path, copy.deepcopy(payload)))
        route = path.split('?')[0]
        if route == '/user':
            return self.user
        if route == 'pullrequests':
            if method == 'POST':
                value = copy.deepcopy(payload)
                value.update(id=901, state='OPEN')
                value['source']['commit'] = {'hash': 'a' * 40}
                self.prs['901'] = value
                return value
            values = [v for v in self.prs.values() if v['state'] == 'OPEN']
            branch = re.search(r'source.branch.name="([A-Za-z0-9._/-]+)"', path)
            if branch:
                values = [v for v in values if v['source']['branch']['name'] == branch[1]]
            return {'size': len(values), 'values': values}
        match = re.fullmatch(r'pullrequests/(\d+)(?:/(.*))?', route)
        if match:
            number, suffix = match[1], match[2]
            if suffix is None:
                if method == 'PUT':
                    self.prs[number].update(payload)
                return self.prs[number]
            if suffix == 'diffstat':
                return {'size': 1, 'values': [{'new': {'path': 'src/current.rs'}, 'old': {'path': 'src/previous.rs'}, 'lines_added': 3, 'lines_removed': 1}]}
            if suffix == 'statuses':
                return {'size': 0, 'values': []}
            if suffix == 'decline':
                self.prs[number]['state'] = 'DECLINED'
                return self.prs[number]
            if suffix.startswith('comments'):
                values = self.comments.setdefault(number, [])
                if method == 'GET':
                    return {'size': len(values), 'values': values}
                if '/' in suffix:
                    value = next(v for v in values if v['id'] == int(suffix.split('/')[1]))
                    value['content'] = payload['content']
                else:
                    value = {'id': len(values) + 1, 'content': payload['content'], 'user': self.user}
                    values.append(value)
                return value
        if route.startswith('merge-base/'):
            return {'hash': self.__dict__.get('ancestor', 'a' * 40)}
        raise AssertionError(f'unexpected Bitbucket call {method} {path}')


def main():
    state_path = Path(os.environ['FORGE_FIXTURE_STATE'])
    state = json.loads(state_path.read_text())
    fixture = GitHubFixture() if state['forge'] == 'github' else BitbucketFixture()
    fixture.__dict__.update(state['data'])
    if state['forge'] == 'github':
        fixture.prs = {int(k): v for k, v in fixture.prs.items()}
        fixture.comments = {int(k): v for k, v in fixture.comments.items()}
        fixture.reviews = {int(k): v for k, v in fixture.reviews.items()}
    argv = sys.argv[1:]
    path = argv[1]
    if state['forge'] == 'bitbucket':
        path = path.removeprefix('/repositories/workspace/project/')
    method = argv[argv.index('--method') + 1]
    payload = None
    if '--input' in argv:
        value = argv[argv.index('--input') + 1]
        payload = json.loads(sys.stdin.read() if value == '-' else value)
    state.setdefault('argv', []).append(argv)
    if state.get('invalid_output'):
        print('private-cli-output')
        return 0
    if state.get('exit_code'):
        print('private-cli-output', file=sys.stderr)
        return state['exit_code']
    result = fixture(path, method, payload)
    state['data'] = fixture.__dict__
    state_path.write_text(json.dumps(state))
    if method != 'GET' and state.get('accepted_write_failure'):
        state.pop('accepted_write_failure')
        state_path.write_text(json.dumps(state))
        print('private-cli-output', file=sys.stderr)
        return 23
    print(json.dumps(result))
    return 0


if __name__ == '__main__':
    sys.exit(main())
