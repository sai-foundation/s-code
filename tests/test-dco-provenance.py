#!/usr/bin/env python3
"""Exercise the trust boundary for Dependabot's alternate DCO email."""

import contextlib
import copy
import importlib.util
import io
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import urllib.error


spec = importlib.util.spec_from_file_location(
    "check_dco", Path(__file__).resolve().parents[1] / "scripts/check-dco.py"
)
dco = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dco)

SHA = "a" * 40
OTHER_SHA = "b" * 40
REPOSITORY = "example/project"
PULL = {
    "number": 63,
    "user": {"id": 49699333, "login": "dependabot[bot]", "type": "Bot"},
    "base": {"repo": {"full_name": REPOSITORY}},
    "head": {"repo": {"full_name": REPOSITORY}, "sha": SHA, "ref": "dependabot/npm/test"},
}
COMMIT = {
    "sha": SHA,
    "author": PULL["user"],
    "committer": {"id": 19864447, "login": "web-flow"},
    "commit": {"verification": {"verified": True, "reason": "valid"}},
}


class ProvenanceTests(unittest.TestCase):
    def verify(self, pull=None, commit=None):
        with patch.object(dco, "github_api", side_effect=[
            copy.deepcopy(PULL if pull is None else pull),
            copy.deepcopy(COMMIT if commit is None else commit),
        ]) as api:
            result = dco.verified_dependabot_commits(REPOSITORY, 63, SHA, [SHA])
        return result, api

    def test_verified_dependabot(self):
        result, api = self.verify()
        self.assertTrue(result)
        self.assertEqual(api.call_count, 2)

    def test_forged_or_stale_pull_request(self):
        mutations = [
            ("user", {**PULL["user"], "id": 1}),
            ("user", {**PULL["user"], "login": "another[bot]"}),
            ("user", {**PULL["user"], "type": "User"}),
            ("number", 64),
            ("head", {**PULL["head"], "sha": OTHER_SHA}),
            ("head", {**PULL["head"], "ref": "user/dependabot-lookalike"}),
            ("head", {**PULL["head"], "repo": {"full_name": "attacker/fork"}}),
            ("base", {"repo": {"full_name": "another/project"}}),
            ("user", None),
            ("head", None),
        ]
        for key, value in mutations:
            with self.subTest(key=key, value=value):
                pull = copy.deepcopy(PULL)
                pull[key] = value
                result, api = self.verify(pull=pull)
                self.assertFalse(result)
                self.assertEqual(api.call_count, 1)

    def test_unverified_or_misattributed_commit(self):
        mutations = [
            ("sha", OTHER_SHA),
            ("author", {**COMMIT["author"], "id": 1}),
            ("author", {**COMMIT["author"], "login": "someone"}),
            ("author", {**COMMIT["author"], "type": "User"}),
            ("author", None),
            ("committer", {"id": 1, "login": "web-flow"}),
            ("committer", {"id": 19864447, "login": "someone"}),
            ("commit", {"verification": {"verified": False, "reason": "unsigned"}}),
            ("commit", {"verification": {"verified": True, "reason": "invalid"}}),
            ("commit", {"verification": {"verified": "true", "reason": "valid"}}),
            ("commit", None),
        ]
        for key, value in mutations:
            with self.subTest(key=key, value=value):
                commit = copy.deepcopy(COMMIT)
                commit[key] = value
                self.assertFalse(self.verify(commit=commit)[0])

    def test_every_exception_commit_is_verified(self):
        with patch.object(dco, "github_api", side_effect=[PULL, COMMIT, {
            **COMMIT, "sha": OTHER_SHA,
            "commit": {"verification": {"verified": False}},
        }]) as api:
            self.assertFalse(dco.verified_dependabot_commits(
                REPOSITORY, 63, SHA, [SHA, OTHER_SHA]
            ))
            self.assertEqual(api.call_count, 3)

    def test_invalid_repository_never_requests_token(self):
        with patch.object(dco, "github_api") as api:
            for repository in ("https://attacker.test", "../repos/x", "x/y?token=z"):
                with self.assertRaises(ValueError):
                    dco.verified_dependabot_commits(repository, 63, SHA, [SHA])
            with self.assertRaises(ValueError):
                dco.verified_dependabot_commits(REPOSITORY, -1, SHA, [SHA])
            api.assert_not_called()

    def test_api_failures_are_closed_and_do_not_expose_token(self):
        with patch.dict(os.environ, {"GITHUB_TOKEN": "synthetic-test-token"}):
            with patch.object(dco.urllib.request, "urlopen", side_effect=urllib.error.URLError("offline")):
                with self.assertRaisesRegex(ValueError, "verification is unavailable") as error:
                    dco.github_api(f"{REPOSITORY}/pulls/63")
                self.assertNotIn("synthetic-test-token", str(error.exception))
            for raw in ('[]', '{invalid json'):
                with self.subTest(raw=raw), patch.object(dco.urllib.request, "urlopen", return_value=io.StringIO(raw)):
                    with self.assertRaises(ValueError):
                        dco.github_api(f"{REPOSITORY}/pulls/63")
        with patch.dict(os.environ, {}, clear=True), patch.object(dco.urllib.request, "urlopen") as request:
            with self.assertRaisesRegex(ValueError, "GITHUB_TOKEN is required"):
                dco.github_api(f"{REPOSITORY}/pulls/63")
            request.assert_not_called()

    def run_check(self, author=dco.DEPENDABOT_AUTHOR, signer=dco.DEPENDABOT_SIGNER,
                  flags=True, verification=True, extra_unsigned=False):
        def git(*args):
            if args[0] == "rev-list":
                return SHA + ("\n" + OTHER_SHA if extra_unsigned else "")
            if args[0] == "rev-parse":
                return SHA
            if args[2] == "--format=%an%x00%ae":
                return "\x00".join(author)
            if args[-1] == OTHER_SHA or signer is None:
                return "Unsigned commit"
            return f"Update dependency\n\nSigned-off-by: {signer[0]} <{signer[1]}>\n"

        argv = ["check-dco.py", "--base", "main", "--head", "HEAD"]
        if flags:
            argv += ["--repository", REPOSITORY, "--pull-request", "63"]
        kwargs = {"side_effect": verification} if isinstance(verification, Exception) else {"return_value": verification}
        with patch("sys.argv", argv), patch.object(dco, "git", side_effect=git), \
                patch.object(dco, "verified_dependabot_commits", **kwargs) as verify, \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            status = dco.main()
        return status, verify

    def test_real_bot_format_requires_provenance(self):
        self.assertEqual(self.run_check()[0], 0)
        self.assertEqual(self.run_check(verification=False)[0], 1)
        self.assertEqual(self.run_check(verification=ValueError("offline"))[0], 1)
        status, verify = self.run_check(flags=False)
        self.assertEqual(status, 1)
        verify.assert_not_called()

    def test_normal_signoff_stays_offline(self):
        author = ("Alice Example", "alice@example.invalid")
        status, verify = self.run_check(author=author, signer=author)
        self.assertEqual(status, 0)
        verify.assert_not_called()

    def test_wrong_signer_unsigned_and_mixed_commits_fail(self):
        for options in ({"signer": None}, {"signer": ("other", "support@github.com")},
                        {"author": ("other", "other@example.invalid")}, {"extra_unsigned": True}):
            with self.subTest(options=options):
                status, verify = self.run_check(**options)
                self.assertEqual(status, 1)
                verify.assert_not_called()


if __name__ == "__main__":
    unittest.main()
