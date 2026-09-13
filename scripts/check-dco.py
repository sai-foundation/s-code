#!/usr/bin/env python3
"""Require a Developer Certificate of Origin sign-off on pull-request commits."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request


SIGN_OFF = re.compile(
    r"(?im)^Signed-off-by:\s*(?P<name>[^<>\r\n]+?)\s*"
    r"<(?P<email>[^<>\s]+@[^<>\s]+)>\s*$"
)
DEPENDABOT_AUTHOR = (
    "dependabot[bot]", "49699333+dependabot[bot]@users.noreply.github.com"
)
DEPENDABOT_SIGNER = ("dependabot[bot]", "support@github.com")


def git(*arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments], text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"git {' '.join(arguments)} failed: {detail}")
    return result.stdout


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="reviewed base commit")
    parser.add_argument("--head", required=True, help="pull-request head commit")
    parser.add_argument("--repository", help="GitHub owner/repository for bot verification")
    parser.add_argument("--pull-request", type=int, help="GitHub PR number for bot verification")
    return parser.parse_args()


def identity(name: str, email: str) -> tuple[str, str]:
    return (" ".join(name.split()).casefold(), email.strip().casefold())


def github_api(resource: str) -> dict:
    token = os.environ.get("GITHUB_TOKEN")
    if not token:
        raise ValueError("GITHUB_TOKEN is required to verify Dependabot provenance")
    request = urllib.request.Request(
        f"https://api.github.com/repos/{resource}",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "User-Agent": "s-code-dco-check",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            data = json.load(response)
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        # Do not include request headers or tokens in CI output.
        raise ValueError("GitHub provenance verification is unavailable") from error
    if not isinstance(data, dict):
        raise ValueError("GitHub provenance response must be an object")
    return data


def verified_dependabot_commits(repository: str, number: int, head: str, commits: list[str]) -> bool:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) or number <= 0:
        raise ValueError("bot verification requires a valid GitHub repository and PR number")
    pull = github_api(f"{repository}/pulls/{number}")
    user = pull.get("user") or {}
    base = pull.get("base") or {}
    remote_head = pull.get("head") or {}
    if not (
        pull.get("number") == number
        and user.get("id") == 49699333
        and user.get("login") == "dependabot[bot]"
        and user.get("type") == "Bot"
        and (base.get("repo") or {}).get("full_name") == repository
        and (remote_head.get("repo") or {}).get("full_name") == repository
        and remote_head.get("sha") == head
        and str(remote_head.get("ref", "")).startswith("dependabot/")
    ):
        return False
    for sha in commits:
        data = github_api(f"{repository}/commits/{sha}")
        author = data.get("author") or {}
        committer = data.get("committer") or {}
        verification = (data.get("commit") or {}).get("verification") or {}
        if not (
            data.get("sha") == sha
            and author.get("id") == 49699333
            and author.get("login") == "dependabot[bot]"
            and author.get("type") == "Bot"
            and committer.get("id") == 19864447
            and committer.get("login") == "web-flow"
            and verification.get("verified") is True
            and verification.get("reason") == "valid"
        ):
            return False
    return True


def main() -> int:
    arguments = parse_args()
    try:
        commits = git("rev-list", "--reverse", "--no-merges", f"{arguments.base}..{arguments.head}").splitlines()
        if not commits:
            raise ValueError("pull request contains no non-merge commits")
        invalid: list[str] = []
        bot_candidates: list[str] = []
        for commit in commits:
            author_name, author_email = git(
                "show", "-s", "--format=%an%x00%ae", commit
            ).rstrip("\n").split("\x00", 1)
            author = identity(author_name, author_email)
            message = git("show", "-s", "--format=%B", commit)
            signers = {
                identity(match.group("name"), match.group("email"))
                for match in SIGN_OFF.finditer(message)
            }
            if author not in signers:
                if author == DEPENDABOT_AUTHOR and DEPENDABOT_SIGNER in signers:
                    bot_candidates.append(commit)
                else:
                    invalid.append(commit)
        if bot_candidates:
            # Git author fields are forgeable. The alternate signer is accepted
            # only with GitHub's PR identity and verified commit provenance.
            if invalid or not arguments.repository or not arguments.pull_request:
                invalid.extend(bot_candidates)
            elif not verified_dependabot_commits(
                arguments.repository, arguments.pull_request,
                git("rev-parse", "--verify", f"{arguments.head}^{{commit}}").strip(),
                bot_candidates,
            ):
                invalid.extend(bot_candidates)
        if invalid:
            raise ValueError(
                "commits missing an author-matching Signed-off-by trailer: "
                + ", ".join(commit[:12] for commit in invalid)
            )
    except ValueError as error:
        print(f"DCO check failed: {error}", file=sys.stderr)
        return 1
    print(f"DCO check passed ({len(commits)} commits)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
