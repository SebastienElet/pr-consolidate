import json
import subprocess
import unittest
from unittest import mock

from test_forge import forge
import forge_transport


class TransportTest(unittest.TestCase):
    def setUp(self):
        forge_transport.configure({"repository": "workspace/project"})

    def test_repository_endpoint_and_nested_body_reach_bkt_as_argv(self):
        result = subprocess.CompletedProcess([], 0, stdout='{"id": 9}', stderr="")
        body = {"content": {"raw": "literal `$(never)` and quotes \""}}
        with mock.patch.object(subprocess, "run", return_value=result) as process:
            self.assertEqual(forge_transport.api("pullrequests/800/comments", "POST", body, timeout=7), {"id": 9})
        argv = process.call_args.args[0]
        self.assertEqual(argv[:5], ["bkt", "api", "/repositories/workspace/project/pullrequests/800/comments",
                                   "--method", "POST"])
        self.assertEqual(json.loads(argv[argv.index("--input") + 1]), body)
        self.assertEqual(process.call_args.kwargs["timeout"], 7)
        self.assertNotIn("shell", process.call_args.kwargs)

    def test_cloud_pagination_is_bound_to_repository(self):
        with mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "{}", "")) as run:
            forge_transport.api("https://api.bitbucket.org/2.0/repositories/workspace/project/pullrequests?page=2")
            self.assertEqual(run.call_args.args[0][2],
                             "/repositories/workspace/project/pullrequests?page=2")

    def test_foreign_or_traversing_pagination_never_starts_bkt(self):
        paths = ["https://evil.example/path", "https://api.bitbucket.org/2.0/repositories/other/repo/pullrequests",
                 "https://secret@api.bitbucket.org/2.0/repositories/workspace/project/pullrequests",
                 "https://api.bitbucket.org/2.0/repositories/workspace/project/../other/pullrequests",
                 "/repositories/other/repo/pullrequests", "../other", "//evil.example/path",
                 "https://api.bitbucket.org/2.0/repositories/workspace/project/%2e%2e/other",
                 "pullrequests/800/../../other", "pullrequests\\..\\other"]
        with mock.patch.object(subprocess, "run") as process:
            for path in paths:
                with self.assertRaises(forge_transport.ForgeAPIError):
                    forge_transport.api(path)
        process.assert_not_called()

    def test_missing_cli_timeout_invalid_json_and_error_do_not_expose_output(self):
        secret = "SECRET_SHOULD_NEVER_APPEAR"
        failures = [FileNotFoundError(secret), subprocess.TimeoutExpired(secret, 3),
                    subprocess.CompletedProcess([], 1, secret, secret),
                    subprocess.CompletedProcess([], 0, secret, secret),
                    subprocess.CompletedProcess([], 0, '[]', secret)]
        for failure in failures:
            parameters = {"side_effect": failure} if isinstance(failure, Exception) else {"return_value": failure}
            with mock.patch.object(subprocess, "run", **parameters):
                with self.assertRaises(forge_transport.ForgeAPIError) as raised:
                    forge_transport.api("pullrequests")
            self.assertNotIn(secret, str(raised.exception))


class GitHubTransportTest(unittest.TestCase):
    def setUp(self):
        forge_transport.configure({"repository": "workspace/project", "forge": "github"})

    def test_github_payload_reaches_explicit_host_as_stdin_without_shell_interpretation(self):
        payload = {"body": "literal `$(never)` and quotes \""}
        result = subprocess.CompletedProcess([], 0, '{"id": 1}', "")
        with mock.patch.object(subprocess, "run", return_value=result) as process:
            self.assertEqual(forge_transport.github_request("/repos/workspace/project/issues/800/comments",
                                                           "POST", payload, 7), {"id": 1})
        self.assertEqual(process.call_args.args[0], ["gh", "api", "/repos/workspace/project/issues/800/comments",
                                                   "--hostname", "github.com", "--method", "POST", "--input", "-"])
        self.assertEqual(json.loads(process.call_args.kwargs["input"]), payload)
        self.assertNotIn("shell", process.call_args.kwargs)
        self.assertEqual(process.call_args.kwargs["timeout"], 7)

    def test_foreign_traversal_credentials_and_absolute_endpoints_never_start_cli(self):
        for path in ("/repos/other/project/issues", "/repos/workspace/project/../other", "/user?SECRET",
                     "https://SECRET@api.github.com/repos/workspace/project/issues", "//api.github.com/user",
                     "/repos/workspace/project/%2e%2e/other", "/repos/workspace/project/\\..\\other"):
            with mock.patch.object(subprocess, "run") as process, self.assertRaises(forge_transport.ForgeAPIError):
                forge_transport.github_request(path)
            process.assert_not_called()

    def test_missing_cli_timeout_and_invalid_json_do_not_expose_credentials(self):
        secret = "SECRET_SHOULD_NOT_APPEAR"
        failures = (FileNotFoundError(secret), subprocess.TimeoutExpired(secret, 3),
                    subprocess.CompletedProcess([], 1, secret, secret),
                    subprocess.CompletedProcess([], 0, secret, secret))
        for failure in failures:
            params = {"side_effect": failure} if isinstance(failure, Exception) else {"return_value": failure}
            with mock.patch.object(subprocess, "run", **params), self.assertRaises(forge_transport.ForgeAPIError) as error:
                forge_transport.github_request("/user")
            self.assertNotIn(secret, str(error.exception))


if __name__ == "__main__":
    unittest.main()
