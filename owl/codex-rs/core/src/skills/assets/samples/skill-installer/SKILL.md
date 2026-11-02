---
name: skill-installer
description: Install owl skills into $OWL_HOME/skills from a curated list or a GitHub repo path. Use when a user asks to list installable skills, install a curated skill, or install a skill from another repo (including private repos).
metadata:
  short-description: Install skills from the user's own skill repo or any other repo
---

# Skill Installer

Helps install skills. owl has no hosted catalog of its own: the collection to list comes from the user's `OWL_SKILLS_REPO` (optionally with `OWL_SKILLS_PATH`), or from a repo/path they give in the request.

Use the helper scripts based on the task:
- List the user's curated skills when the user asks what is available, or if the user uses this skill without specifying what to do. With no `OWL_SKILLS_REPO` set and no repo in the request, ask the user where their skills live instead of guessing.
- Install from the curated list when the user provides a skill name.
- Install from another repo when the user provides a GitHub repo/path (including private repos).

Install skills with the helper scripts.

## Communication

When listing curated skills, output approximately as follows, depending on the context of the user's request:
"""
Skills from {repo}:
1. skill-1
2. skill-2 (already installed)
3. ...
Which ones would you like installed?
"""

After installing a skill, tell the user: "Restart owl to pick up new skills."

## Scripts

All of these scripts use network, so when running in the sandbox, request escalation when running them.

- `scripts/list-curated-skills.py` (prints curated list with installed annotations)
- `scripts/list-curated-skills.py --format json`
- `scripts/install-skill-from-github.py --repo <owner>/<repo> --path <path/to/skill> [<path/to/skill> ...]`
- `scripts/install-skill-from-github.py --url https://github.com/<owner>/<repo>/tree/<ref>/<path>`

## Behavior and Options

- Defaults to direct download for public GitHub repos.
- If download fails with auth/permission errors, falls back to git sparse checkout.
- Aborts if the destination skill directory already exists.
- Installs into `$OWL_HOME/skills/<skill-name>` (defaults to `~/.owl/skills`).
- Multiple `--path` values install multiple skills in one run, each named from the path basename unless `--name` is supplied.
- Options: `--ref <ref>` (default `main`), `--dest <path>`, `--method auto|download|git`.

## Notes

- The curated listing is fetched from the user's own repo path (`--repo`/`--path`, or `OWL_SKILLS_REPO`/`OWL_SKILLS_PATH`) via the GitHub API. If it is unavailable, explain the error and exit.
- Private GitHub repos can be accessed via existing git credentials or optional `GITHUB_TOKEN`/`GH_TOKEN` for download.
- Git fallback tries HTTPS first, then SSH.
- owl's own bundled skills are already installed under `$OWL_HOME/skills/.system`, so no need to help users install those. If they ask, just explain this. If they insist, you can download and overwrite.
- Installed annotations come from `$OWL_HOME/skills`.
