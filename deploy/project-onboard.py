#!/usr/bin/env python3
"""
project-onboard.py — Bootstrap a new project into the ACC fleet.

Creates a project record, syncs the git repo to AccFS shared storage,
generates an initial task set from PLAN.md and/or open beads,
creates a milestone "sync-to-git" task blocked on all others,
and broadcasts project.arrived on AgentBus so idle agents can join.

Usage:
    python3 project-onboard.py --repo owner/repo [--name "Name"] [--branch main]
    python3 project-onboard.py --repo owner/repo --local /path/to/existing/clone

Environment (from ~/.acc/.env):
    ACC_URL, ACC_AGENT_TOKEN
    ACC_SHARED_DIR (default: ~/.acc/shared)
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

_home = Path(os.environ.get("HOME", "/home"))
ACC_DIR  = _home / ".acc" if (_home / ".acc").exists() else _home / ".ccc"
ENV_FILE = ACC_DIR / ".env"

def _load_env() -> None:
    if ENV_FILE.exists():
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, _, v = line.partition("=")
                    os.environ.setdefault(k.strip(), v.strip().strip("'\""))

_load_env()

ACC_URL         = (os.environ.get("ACC_URL") or os.environ.get("CCC_URL", "")).rstrip("/")
ACC_AGENT_TOKEN = os.environ.get("ACC_AGENT_TOKEN") or os.environ.get("CCC_AGENT_TOKEN", "")
AGENT_NAME      = os.environ.get("AGENT_NAME", "unknown")
ACCFS_SHARED    = Path(os.environ.get("ACC_SHARED_DIR", str(ACC_DIR / "shared")))
HTTP_TIMEOUT    = 15


def _log(msg: str) -> None:
    ts = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    print(f"[{ts}] [project-onboard] {msg}", flush=True)


def _curl(method: str, path: str, body: dict | None = None) -> dict | None:
    if not ACC_URL or not ACC_AGENT_TOKEN:
        return None
    cmd = [
        "curl", "-sf", "--max-time", str(HTTP_TIMEOUT),
        "-X", method,
        "-H", f"Authorization: Bearer {ACC_AGENT_TOKEN}",
        "-H", "Content-Type: application/json",
    ]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    cmd.append(f"{ACC_URL}{path}")
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=HTTP_TIMEOUT + 5)
        if r.returncode == 0 and r.stdout.strip():
            return json.loads(r.stdout)
    except Exception:
        pass
    return None


def _rsync(src: Path, dst: Path) -> bool:
    """Rsync src → dst. Returns True on success."""
    dst.mkdir(parents=True, exist_ok=True)
    r = subprocess.run(
        ["rsync", "-a", "--delete", "--quiet", f"{src}/", f"{dst}/"],
        capture_output=True, text=True, timeout=300,
    )
    if r.returncode != 0:
        _log(f"rsync failed: {r.stderr.strip()[:200]}")
    return r.returncode == 0


def _slug(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def _proj_id(repo: str, branch: str) -> str:
    h = hashlib.sha1(f"{repo}@{branch}".encode()).hexdigest()[:8]
    return f"proj-{h}"


def _accfs_project_path(slug: str) -> Path:
    return ACCFS_SHARED / "projects" / slug


def _project_exists(slug: str) -> bool:
    return (_accfs_project_path(slug) / "project.json").exists()


def _clone_repo(repo: str, branch: str, dest: Path) -> str:
    """Clone repo + submodules. Returns HEAD sha."""
    url = f"https://github.com/{repo}.git" if "/" in repo and "://" not in repo else repo
    _log(f"Cloning {url} @ {branch} → {dest}")
    r = subprocess.run(
        ["git", "clone", "--recurse-submodules", "--depth=50",
         "--branch", branch, url, str(dest)],
        capture_output=True, text=True, timeout=300,
    )
    if r.returncode != 0:
        # Try without --branch (use default)
        r = subprocess.run(
            ["git", "clone", "--recurse-submodules", "--depth=50", url, str(dest)],
            capture_output=True, text=True, timeout=300,
        )
    if r.returncode != 0:
        raise RuntimeError(f"git clone failed: {r.stderr.strip()[:300]}")
    sha = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        capture_output=True, text=True, cwd=str(dest),
    ).stdout.strip()
    _log(f"Cloned at {sha}")
    return sha


def _parse_plan(plan_path: Path) -> list[str]:
    """Extract task descriptions from PLAN.md."""
    tasks: list[str] = []
    if not plan_path.exists():
        return tasks
    section = ""
    for line in plan_path.read_text(errors="replace").splitlines():
        if line.startswith("## "):
            section = line[3:].strip()
        m = re.match(r"^\s*[-*]\s+\[[ xX]?\]\s+(.+)", line)
        if m:
            text = m.group(1).strip()
            tasks.append(f"{section}: {text}" if section else text)
            continue
        m = re.match(r"^\s*\d+\.\s+(.+)", line)
        if m and section:
            tasks.append(f"{section}: {m.group(1).strip()}")
    _log(f"Parsed {len(tasks)} task(s) from PLAN.md")
    return tasks


def _parse_beads(repo_dir: Path) -> list[dict]:
    """Read open beads from .beads/issues.jsonl."""
    issues_file = repo_dir / ".beads" / "issues.jsonl"
    if not issues_file.exists():
        return []
    beads: list[dict] = []
    for line in issues_file.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            issue = json.loads(line)
            if issue.get("status") == "open":
                beads.append(issue)
        except Exception:
            pass
    _log(f"Found {len(beads)} open bead(s) in .beads/issues.jsonl")
    return beads


def _post_task(title: str, description: str, project_id: str,
               accfs_path: str, github_repo: str,
               tags: list[str] | None = None,
               depends_on: list[str] | None = None,
               bead_id: str = "") -> str | None:
    """POST a new queue item. Returns item id or None."""
    now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    item = {
        "title":                  title[:120],
        "description":            description,
        "status":                 "pending",
        "priority":               "normal",
        "assignee":               "all",
        "source":                 AGENT_NAME,
        "created":                now,
        "attempts":               0,
        "maxAttempts":            3,
        "tags":                   (tags or []) + ["project"],
        "project_id":             project_id,
        "project_accfs_path":     accfs_path,
        "project":                github_repo,
        "scout_key":              f"bead:{bead_id}" if bead_id else None,
    }
    if depends_on:
        item["dependsOn"] = depends_on
    resp = _curl("POST", "/api/queue", item)
    if resp:
        item_id = resp.get("id") or resp.get("item", {}).get("id")
        if item_id:
            return item_id
    _log(f"WARNING: failed to post task '{title[:60]}'")
    return None


def onboard(
    repo: str,
    name: str,
    branch: str = "main",
    local_path: Path | None = None,
    description: str = "",
) -> None:
    slug       = _slug(name or repo.split("/")[-1])
    project_id = _proj_id(repo, branch)
    accfs_dir  = _accfs_project_path(slug)
    accfs_ws   = str(accfs_dir / "workspace")

    _log(f"Onboarding project: {name!r} ({project_id})")
    _log(f"  repo:   {repo} @ {branch}")
    _log(f"  accfs:  {accfs_ws}")

    if _project_exists(slug):
        _log(f"Project '{slug}' already exists in AccFS — re-running task generation only")

    # ── Clone or use local copy ───────────────────────────────────────────────
    with tempfile.TemporaryDirectory(prefix="ccc-onboard-") as tmp:
        repo_dir = local_path or Path(tmp) / slug
        sha = "local"
        if local_path:
            repo_dir = local_path
        else:
            sha = _clone_repo(repo, branch, repo_dir)

        # ── Sync to AccFS ─────────────────────────────────────────────────────
        if ACCFS_SHARED.exists():
            _log(f"Syncing to AccFS: {accfs_ws}")
            _rsync(repo_dir, accfs_dir / "workspace")
        else:
            _log(f"WARNING: AccFS not mounted at {ACCFS_SHARED} — skipping shared sync")

        # ── Parse work from repo ──────────────────────────────────────────────
        plan_tasks = _parse_plan(repo_dir / "PLAN.md")
        bead_items = _parse_beads(repo_dir)

        # ── Create project record ─────────────────────────────────────────────
        now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        project = {
            "id":            project_id,
            "name":          name or repo.split("/")[-1],
            "slug":          slug,
            "description":   description,
            "status":        "active",
            "github_repo":   repo,
            "github_branch": branch,
            "github_sha":    sha,
            "accfs_path":    accfs_ws,
            "created_at":    now,
            "created_by":    AGENT_NAME,
            "task_ids":      [],
            "milestone_task_id": None,
            "tags":          [],
        }

        # ── Post tasks ────────────────────────────────────────────────────────
        task_ids: list[str] = []

        for title in plan_tasks:
            tid = _post_task(
                title=title,
                description=f"From PLAN.md in {repo}",
                project_id=project_id,
                accfs_path=accfs_ws,
                github_repo=repo,
                tags=["plan"],
            )
            if tid:
                task_ids.append(tid)
                _log(f"  task: {tid} — {title[:60]}")

        for bead in bead_items:
            bead_desc = (bead.get("description") or bead.get("body") or "").strip()
            if not bead_desc:
                bead_desc = f"Bead {bead.get('id')} from {repo}: {bead.get('title','')}"
            tid = _post_task(
                title=bead.get("title", "untitled bead"),
                description=bead_desc,
                project_id=project_id,
                accfs_path=accfs_ws,
                github_repo=repo,
                tags=["beads"],
                bead_id=str(bead.get("id", "")),
            )
            if tid:
                task_ids.append(tid)
                _log(f"  task: {tid} — (bead) {bead.get('title','')[:60]}")

        # ── Milestone sync task (blocked on all above) ────────────────────────
        milestone_id = None
        if task_ids:
            milestone_id = _post_task(
                title=f"[{name}] milestone: reconcile AccFS → GitHub",
                description=(
                    f"Sync completed project work from AccFS ({accfs_ws}) "
                    f"back to GitHub ({repo} @ {branch}).\n\n"
                    "1. Review all changes in the AccFS workspace\n"
                    "2. Run tests / build\n"
                    "3. Commit and push to a release branch\n"
                    "4. Open a PR or tag a release as appropriate"
                ),
                project_id=project_id,
                accfs_path=accfs_ws,
                github_repo=repo,
                tags=["milestone", "sync"],
                depends_on=task_ids,
            )
            _log(f"  milestone: {milestone_id} (blocked on {len(task_ids)} task(s))")

        project["task_ids"]          = task_ids
        project["milestone_task_id"] = milestone_id

        # ── Store project record in AccFS ─────────────────────────────────────
        if ACCFS_SHARED.exists():
            accfs_dir.mkdir(parents=True, exist_ok=True)
            (accfs_dir / "project.json").write_text(json.dumps(project, indent=2))
            _log(f"Project record saved: {accfs_dir}/project.json")

    # ── Broadcast project.arrived ─────────────────────────────────────────────
    _curl("POST", "/bus/send", {
        "from":    AGENT_NAME,
        "to":      "all",
        "type":    "project.arrived",
        "subject": "work",
        "body": json.dumps({
            "project_id":   project_id,
            "name":         project["name"],
            "slug":         slug,
            "accfs_path":   accfs_ws,
            "github_repo":  repo,
            "task_count":   len(task_ids),
            "milestone_id": milestone_id,
        }),
    })
    _log(f"Broadcast project.arrived → all agents ({len(task_ids)} task(s) available)")
    _log(f"Done. Project {project_id} is live.")


def main() -> None:
    p = argparse.ArgumentParser(description="Onboard a project into the CCC fleet")
    p.add_argument("--repo",   required=True, help="GitHub repo (owner/repo or full URL)")
    p.add_argument("--name",   default="",    help="Human-readable project name")
    p.add_argument("--branch", default="main",help="Branch to clone (default: main)")
    p.add_argument("--local",  default="",    help="Use existing local clone instead of cloning")
    p.add_argument("--description", default="", help="Project description")
    args = p.parse_args()

    local = Path(args.local) if args.local else None
    name  = args.name or args.repo.split("/")[-1]

    try:
        onboard(
            repo=args.repo,
            name=name,
            branch=args.branch,
            local_path=local,
            description=args.description,
        )
    except Exception as e:
        _log(f"ERROR: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
