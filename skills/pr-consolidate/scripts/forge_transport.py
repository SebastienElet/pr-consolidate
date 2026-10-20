import json
import re
import subprocess
from urllib.parse import unquote, urlsplit


_repository = None
_forge = None


def configure(context):
    global _repository, _forge
    _repository = context["repository"]
    _forge = context.get("forge", "bitbucket")


class ForgeAPIError(RuntimeError):
    pass


class ForgeTimeout(ForgeAPIError):
    pass


def endpoint(path):
    if _repository is None:
        raise ForgeAPIError("repository context is not configured")
    root = f"/repositories/{_repository}/"
    parsed = urlsplit(path)
    if parsed.scheme or parsed.netloc:
        if parsed.scheme != "https" or parsed.netloc != "api.bitbucket.org" or parsed.fragment:
            raise ForgeAPIError("refusing pagination outside Bitbucket Cloud")
        if not parsed.path.startswith("/2.0" + root):
            raise ForgeAPIError("refusing pagination outside the configured repository")
        path = parsed.path[4:] + ("?" + parsed.query if parsed.query else "")
    elif path == "/user":
        return path
    elif not path.startswith(root):
        if path.startswith("/") or ".." in path.split("?")[0].split("/"):
            raise ForgeAPIError("refusing an endpoint outside the configured repository")
        path = root + path
    pathname = unquote(path.split("?")[0])
    if not path.startswith(root) or "\\" in pathname or "%" in pathname or \
            re.search(r"(?:^|/)\.\.(?:/|$)|[\x00-\x20\x7f]", pathname):
        raise ForgeAPIError("invalid repository endpoint")
    return path


def api(path, method="GET", payload=None, timeout=180):
    if _forge == "github":
        from github_adapter import GitHubAdapter
        return GitHubAdapter(_repository, github_request, ForgeAPIError).api(path, method, payload, timeout)
    if _forge != "bitbucket":
        raise ForgeAPIError("repository forge is not configured")
    return bitbucket_request(path, method, payload, timeout)


def github_request(path, method="GET", payload=None, timeout=180):
    root = f"/repos/{_repository}/"
    parsed = urlsplit(path)
    if parsed.scheme or parsed.netloc or (path != "/user" and not path.startswith(root)) or \
            parsed.fragment or ".." in parsed.path.split("/") or re.search(r"[\\%\x00-\x20\x7f]", parsed.path):
        raise ForgeAPIError("refusing an endpoint outside the configured GitHub repository")
    argv = ["gh", "api", path, "--hostname", "github.com", "--method", method]
    options = {}
    if payload is not None:
        argv.extend(["--input", "-"])
        options["input"] = json.dumps(payload, ensure_ascii=False)
    return execute(argv, timeout, options)


def bitbucket_request(path, method="GET", payload=None, timeout=180):
    target = endpoint(path)
    argv = ["bkt", "api", target, "--method", method, "--json"]
    if payload is not None:
        argv.extend(["--input", json.dumps(payload, ensure_ascii=False)])
    document = execute(argv, timeout)
    if not isinstance(document, dict):
        raise ForgeAPIError("bkt returned a non-object response")
    return document


def execute(argv, timeout, options=None):
    cli, method = argv[0], argv[argv.index("--method") + 1]
    try:
        result = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8", errors="replace",
                                timeout=timeout, check=False, **(options or {}))
    except subprocess.TimeoutExpired as exc:
        raise ForgeTimeout(f"{cli} {method} timed out after {timeout}s; inspect remote state before retry") from exc
    except OSError as exc:
        raise ForgeAPIError(f"{cli} could not start; check its installation") from exc
    if result.returncode:
        raise ForgeAPIError(f"{cli} {method} failed with exit {result.returncode}; remote state is unconfirmed")
    try:
        document = json.loads(result.stdout)
    except (ValueError, TypeError) as exc:
        raise ForgeAPIError(f"{cli} {method} returned invalid JSON; remote state is unconfirmed") from exc
    return document
