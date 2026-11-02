#!/usr/bin/env python3
"""List curated skills from a GitHub repo path."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error

from github_utils import github_api_contents_url, github_request

# owl ships no hosted skill catalog: the user says where their own collection lives,
# either with --repo/--path or by setting OWL_SKILLS_REPO (and optionally
# OWL_SKILLS_PATH) once.
DEFAULT_PATH = "skills/.curated"
DEFAULT_REF = "main"


class ListError(Exception):
    pass


class Args(argparse.Namespace):
    repo: str | None
    path: str | None
    ref: str
    format: str


def _request(url: str) -> bytes:
    return github_request(url, "owl-skill-list")


def _owl_home() -> str:
    for var in ("OWL_HOME", "CODEX_HOME"):
        value = os.environ.get(var)
        if value:
            return value
    return os.path.expanduser("~/.owl")


def _installed_skills() -> set[str]:
    root = os.path.join(_owl_home(), "skills")
    if not os.path.isdir(root):
        return set()
    entries = set()
    for name in os.listdir(root):
        path = os.path.join(root, name)
        if os.path.isdir(path):
            entries.add(name)
    return entries


def _list_curated(repo: str, path: str, ref: str) -> list[str]:
    api_url = github_api_contents_url(repo, path, ref)
    try:
        payload = _request(api_url)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            raise ListError(
                "Curated skills path not found: "
                f"https://github.com/{repo}/tree/{ref}/{path}"
            ) from exc
        raise ListError(f"Failed to fetch curated skills: HTTP {exc.code}") from exc
    data = json.loads(payload.decode("utf-8"))
    if not isinstance(data, list):
        raise ListError("Unexpected curated listing response.")
    skills = [item["name"] for item in data if item.get("type") == "dir"]
    return sorted(skills)


def _parse_args(argv: list[str]) -> Args:
    parser = argparse.ArgumentParser(description="List curated skills.")
    parser.add_argument("--repo", help="owner/repo holding the skill collection")
    parser.add_argument("--path", help="path inside that repo")
    parser.add_argument("--ref", default=DEFAULT_REF)
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="Output format",
    )
    return parser.parse_args(argv, namespace=Args())


def _curated_source(args: Args) -> tuple[str, str]:
    """Where this user's skills live: explicit flags first, then the environment."""
    repo = args.repo or os.environ.get("OWL_SKILLS_REPO")
    if not repo:
        raise ListError(
            "owl has no hosted skill catalog of its own. Tell me where your skills "
            "live: --repo <owner>/<repo> --path <path/to/skills>, or set "
            "OWL_SKILLS_REPO once and I will use it every time."
        )
    path = args.path or os.environ.get("OWL_SKILLS_PATH") or DEFAULT_PATH
    return repo, path


def main(argv: list[str]) -> int:
    args = _parse_args(argv)
    try:
        repo, path = _curated_source(args)
        skills = _list_curated(repo, path, args.ref)
        installed = _installed_skills()
        if args.format == "json":
            payload = [
                {"name": name, "installed": name in installed} for name in skills
            ]
            print(json.dumps(payload))
        else:
            for idx, name in enumerate(skills, start=1):
                suffix = " (already installed)" if name in installed else ""
                print(f"{idx}. {name}{suffix}")
        return 0
    except ListError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
