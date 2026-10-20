import os
import pathlib
import re
import subprocess
from urllib.parse import urlsplit


class ContextError(RuntimeError):
    pass


def git_environment():
    redirects = {"GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
                 "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE", "GIT_SHALLOW_FILE", "GIT_REPLACE_REF_BASE"}
    environment = {key: value for key, value in os.environ.items()
                   if key not in redirects and not key.startswith("GIT_CONFIG")}
    return {**environment, "GIT_TERMINAL_PROMPT": "0"}


def git(directory, *arguments, acceptable=(0,)):
    try:
        result = subprocess.run(
            ["git", "-C", str(directory), *arguments], capture_output=True, text=True,
            timeout=60, check=False, env=git_environment(),
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise ContextError(f"Git discovery could not run: {arguments[0]}") from exc
    if result.returncode not in acceptable:
        raise ContextError(f"Git discovery failed: {arguments[0]} (exit {result.returncode})")
    return result.stdout.strip()


def origin_identity(value):
    if not isinstance(value, str) or re.search(r"[\x00-\x20\x7f\\%]", value):
        raise ContextError("origin URL is invalid")
    match = re.fullmatch(r"git@(bitbucket\.org|github\.com):([^?#]+)", value)
    if match:
        host, path = match[1], match[2]
    else:
        try:
            parsed = urlsplit(value)
            port = parsed.port
        except ValueError as exc:
            raise ContextError("origin URL is invalid") from exc
        if parsed.query or parsed.fragment or parsed.password:
            raise ContextError("origin URL must not contain credentials, query or fragment")
        https = (parsed.scheme == "https" and parsed.hostname in {"bitbucket.org", "github.com"} and
                 parsed.username is None and port in (None, 443))
        ssh = (parsed.scheme == "ssh" and parsed.username == "git" and
               ((parsed.hostname in {"bitbucket.org", "github.com"} and port in (None, 22)) or
                (parsed.hostname == "ssh.bitbucket.org" and port == 443)))
        if not (https or ssh):
            raise ContextError("origin must identify a supported GitHub or Bitbucket Cloud repository")
        host = "bitbucket.org" if parsed.hostname == "ssh.bitbucket.org" else parsed.hostname
        path = parsed.path.removeprefix("/")
    path = path.removesuffix(".git")
    segments = path.split("/")
    if len(segments) != 2 or any(
            segment in {".", ".."} or not re.fullmatch(r"[A-Za-z0-9._-]+", segment)
            for segment in segments):
        raise ContextError("origin must identify exactly one workspace and repository")
    return {"repository": path.lower(), "forge": "github" if host == "github.com" else "bitbucket"}


def repository_from_url(value):
    return origin_identity(value)["repository"]


def remote_context(directory=None):
    directory = pathlib.Path(directory or os.getcwd()).resolve()
    root = git(directory, "rev-parse", "--show-toplevel")
    urls = git(root, "config", "--null", "--get-all", "remote.origin.url", acceptable=(0, 1)).split("\0")[:-1]
    if len(urls) != 1:
        raise ContextError("origin must have exactly one URL")
    identity = origin_identity(urls[0])
    push_urls = git(root, "config", "--null", "--get-all", "remote.origin.pushurl", acceptable=(0, 1)).split("\0")[:-1]
    if len(push_urls) > 1 or any(origin_identity(url) != identity for url in push_urls):
        raise ContextError("origin push URL does not identify the same repository")
    return {"root": str(pathlib.Path(root).resolve()), **identity, "remote_url": urls[0]}


def discover(directory=None):
    context = remote_context(directory)
    output = git(context["root"], "ls-remote", "--symref", "origin", "HEAD")
    branches = re.findall(r"^ref: refs/heads/([^\t\n]+)\tHEAD$", output, re.MULTILINE)
    heads = re.findall(r"^[0-9a-f]{40}\tHEAD$", output, re.MULTILINE)
    if len(branches) != 1 or len(heads) != 1:
        raise ContextError("origin HEAD does not declare one resolvable primary branch")
    git(context["root"], "check-ref-format", "--branch", branches[0])
    return {**context, "base_branch": branches[0]}
