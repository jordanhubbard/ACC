#!/usr/bin/env python3
"""
queue-worker.py — Persistent CCC queue worker daemon.

Polls /api/queue every 60s, claims and executes pending items.
Enforces the AgentFS workspace lifecycle for every task:
  1. Init: git clone repo → local workspace → mirror to AgentFS
  2. Execute: claude or hermes runs inside the local workspace (cwd)
  3. Finalize: local → AgentFS → ONE git push to task/  branch

Usage (direct):   python3 queue-worker.py
Supervisord:      command=python3 /home/.../queue-worker.py
Systemd:          ExecStart=/usr/bin/python3 AGENT_HOME/.ccc/workspace/deploy/queue-worker.py
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

# ── Configuration ────────────────────────────────────────────────────────────

CCC_DIR = Path(os.environ.get("HOME", "/home")) / ".ccc"

_home_local_bin = str(Path(os.environ.get("HOME", "/home")) / ".local" / "bin")
if _home_local_bin not in os.environ.get("PATH", ""):
    os.environ["PATH"] = _home_local_bin + ":" + os.environ.get("PATH", "/usr/bin:/bin")

ENV_FILE          = CCC_DIR / ".env"
LOG_FILE          = CCC_DIR / "logs" / "queue-worker.log"
QUENCH_FILE       = CCC_DIR / "quench"
WORKSPACE         = CCC_DIR / "workspace"
TASK_WORKSPACE_BASE = CCC_DIR / "task-workspaces"

POLL_INTERVAL_IDLE  = 60        # seconds between queue checks when idle
POLL_INTERVAL_BUSY  = 5         # seconds between checks right after completing a task
KEEPALIVE_INTERVAL  = 25 * 60   # 25 min (TTL is 2h for claude_cli)
CLAUDE_TIMEOUT      = 7200      # 2 hours max per task
HTTP_TIMEOUT        = 15        # seconds for API calls


# ── Logging ──────────────────────────────────────────────────────────────────

LOG_FILE.parent.mkdir(parents=True, exist_ok=True)
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%dT%H:%M:%SZ",
    stream=sys.stdout,
)
log = logging.getLogger("queue-worker")
logging.Formatter.converter = time.gmtime


# ── Load .env ────────────────────────────────────────────────────────────────

def load_env() -> None:
    if ENV_FILE.exists():
        with open(ENV_FILE) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, val = line.partition("=")
                    os.environ.setdefault(key.strip(), val.strip().strip("'\""))

load_env()

AGENT_NAME      = os.environ.get("AGENT_NAME", "")
CCC_URL         = os.environ.get("CCC_URL", "").rstrip("/")
CCC_AGENT_TOKEN = os.environ.get("CCC_AGENT_TOKEN", "")

if not AGENT_NAME or not CCC_URL or not CCC_AGENT_TOKEN:
    log.error("AGENT_NAME, CCC_URL, and CCC_AGENT_TOKEN must be set in ~/.ccc/.env")
    sys.exit(1)


# ── Capability detection ──────────────────────────────────────────────────────

def _detect_capabilities() -> frozenset[str]:
    """
    Return the set of executor types this agent can handle.
    Checked against task required_executors (hard filter) at claim time.

    Override by setting AGENT_CAPABILITIES=claude_cli,gpu,... in ~/.ccc/.env.
    """
    from_env = os.environ.get("AGENT_CAPABILITIES", "")
    if from_env:
        return frozenset(c.strip() for c in from_env.split(",") if c.strip())

    caps: set[str] = set()
    if shutil.which("claude"):
        caps.add("claude_cli")
        caps.add("claude_sdk")
    if shutil.which("hermes"):
        caps.add("hermes")
    if os.environ.get("NVIDIA_API_KEY") or os.environ.get("ANTHROPIC_API_KEY"):
        caps.add("inference_key")
    # GPU: nvidia driver or AMD ROCm present
    if (
        Path("/proc/driver/nvidia").exists()
        or shutil.which("nvidia-smi")
        or Path("/dev/kfd").exists()   # AMD ROCm
    ):
        caps.add("gpu")
    return frozenset(caps)

AGENT_CAPABILITIES = _detect_capabilities()


# ── HTTP helpers ─────────────────────────────────────────────────────────────

def _curl(method: str, path: str, body: dict | None = None) -> dict | None:
    url = f"{CCC_URL}{path}"
    cmd = [
        "curl", "-sf", "--max-time", str(HTTP_TIMEOUT),
        "-X", method,
        "-H", f"Authorization: Bearer {CCC_AGENT_TOKEN}",
        "-H", "Content-Type: application/json",
    ]
    if body is not None:
        cmd += ["-d", json.dumps(body)]
    cmd.append(url)
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=HTTP_TIMEOUT + 5)
        if result.returncode != 0 or not result.stdout.strip():
            return None
        return json.loads(result.stdout)
    except (subprocess.TimeoutExpired, json.JSONDecodeError, Exception) as e:
        log.debug(f"curl {method} {path} failed: {e}")
        return None


def post_heartbeat(note: str = "idle") -> None:
    """Update agent lastSeen on the hub. Called every poll cycle so online status stays green."""
    _curl("POST", f"/api/heartbeat/{AGENT_NAME}", {
        "ts": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "status": "ok",
        "note": note,
    })


def get_queue() -> list[dict]:
    data = _curl("GET", "/api/queue")
    return (data or {}).get("items", [])


def claim_item(item_id: str) -> bool:
    resp = _curl("POST", f"/api/item/{item_id}/claim", {
        "agent": AGENT_NAME, "note": "queue-worker claiming",
    })
    return resp is not None


def post_keepalive(item_id: str, note: str = "still working") -> None:
    _curl("POST", f"/api/item/{item_id}/keepalive", {"agent": AGENT_NAME, "note": note})


def post_complete(item_id: str, result: str) -> None:
    _curl("POST", f"/api/item/{item_id}/complete", {
        "agent": AGENT_NAME,
        "result": result[:4000],
        "resolution": result[:4000],
    })


def post_fail(item_id: str, reason: str) -> None:
    _curl("POST", f"/api/item/{item_id}/fail", {
        "agent": AGENT_NAME, "reason": reason[:2000],
    })


def post_comment(item_id: str, text: str) -> None:
    _curl("POST", f"/api/item/{item_id}/comment", {
        "text": text[:2000], "author": AGENT_NAME,
    })


# ── Quench check ─────────────────────────────────────────────────────────────

def is_quenched() -> bool:
    if not QUENCH_FILE.exists():
        return False
    try:
        until_str = QUENCH_FILE.read_text().strip()
        until = datetime.fromisoformat(until_str.replace("Z", "+00:00"))
        if datetime.now(timezone.utc) < until:
            return True
        QUENCH_FILE.unlink(missing_ok=True)
    except Exception:
        pass
    return False


# ── Agent instructions ────────────────────────────────────────────────────────

def load_agent_instructions() -> str:
    candidates = [
        WORKSPACE / "workqueue" / f"WORKQUEUE_AGENT_{AGENT_NAME.upper()}.md",
        WORKSPACE / "workqueue" / "WORKQUEUE_AGENT.md",
    ]
    for path in candidates:
        if path.exists():
            return path.read_text()
    return (
        f"You are the workqueue agent for {AGENT_NAME}. "
        "Process the assigned task completely. "
        "Report your result clearly and concisely."
    )


# ── Task workspace lifecycle ──────────────────────────────────────────────────

def _repo_url_from_item(item: dict) -> str:
    """Derive git repo URL from task item or fall back to CCC workspace origin."""
    project = item.get("project", "").strip()
    if project:
        if "://" not in project and "@" not in project and project.count("/") == 1:
            return f"https://github.com/{project}"
        return project

    # Fall back to the CCC workspace origin
    try:
        r = subprocess.run(
            ["git", "remote", "get-url", "origin"],
            capture_output=True, text=True, cwd=str(WORKSPACE), timeout=5,
        )
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()
    except Exception:
        pass
    return ""


def task_workspace_init(item: dict) -> tuple[Path, str]:
    """
    Bootstrap an isolated workspace for this task.

    1. Clones the task's git repo (or reuses existing clone) to a local dir.
    2. Mirrors the local dir to AgentFS (if MinIO is configured).

    Returns (local_path, agentfs_path).
    agentfs_path is empty string if MinIO is not available.
    """
    item_id = item["id"]
    workspace_local = TASK_WORKSPACE_BASE / item_id

    # Reuse existing workspace (task resume case)
    if workspace_local.exists() and any(workspace_local.iterdir()):
        log.info(f"[{item_id}] Reusing workspace: {workspace_local}")
        return workspace_local, _agentfs_workspace_path(item_id)

    workspace_local.mkdir(parents=True, exist_ok=True)

    repo_url = _repo_url_from_item(item)
    branch   = item.get("branch", "main")

    if repo_url:
        log.info(f"[{item_id}] Cloning {repo_url} ({branch}) → {workspace_local}")
        _git_clone(item_id, repo_url, branch, workspace_local)
    else:
        log.info(f"[{item_id}] No repo URL — using empty workspace")

    agentfs_path = _agentfs_workspace_path(item_id)
    if agentfs_path:
        _mc_mirror_push(item_id, workspace_local, agentfs_path)
        _write_agentfs_meta(item, agentfs_path, repo_url, branch, workspace_local)

    return workspace_local, agentfs_path


def _git_clone(item_id: str, repo_url: str, branch: str, dest: Path) -> None:
    """Clone repo into dest, gracefully handling branch-not-found."""
    try:
        r = subprocess.run(
            ["git", "clone", "--depth=1", "--branch", branch, repo_url, str(dest)],
            capture_output=True, text=True, timeout=120,
        )
        if r.returncode != 0:
            subprocess.run(
                ["git", "clone", "--depth=1", repo_url, str(dest)],
                capture_output=True, text=True, timeout=120, check=True,
            )
        sha = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, cwd=str(dest), timeout=5,
        ).stdout.strip()
        log.info(f"[{item_id}] Cloned at {sha}")
    except Exception as e:
        log.warning(f"[{item_id}] git clone failed: {e} — task will run with empty workspace")


def _agentfs_workspace_path(item_id: str) -> str:
    """Return the AgentFS path for this task's workspace, or '' if unavailable."""
    if not shutil.which("mc"):
        return ""
    if not os.environ.get("MINIO_ENDPOINT"):
        return ""
    mc_alias = os.environ.get("MINIO_ALIAS", "ccc-hub")
    bucket   = os.environ.get("MINIO_BUCKET", "agents")
    return f"{mc_alias}/{bucket}/tasks/{item_id}/workspace"


def _mc_mirror_push(item_id: str, local: Path, agentfs: str) -> None:
    """Mirror local workspace → AgentFS."""
    try:
        r = subprocess.run(
            ["mc", "mirror", "--overwrite", "--quiet", f"{local}/", agentfs],
            capture_output=True, text=True, timeout=120,
        )
        if r.returncode == 0:
            log.info(f"[{item_id}] Workspace → AgentFS: {agentfs}")
        else:
            log.warning(f"[{item_id}] AgentFS push failed: {r.stderr.strip()[:200]}")
    except Exception as e:
        log.warning(f"[{item_id}] AgentFS push error: {e}")


def _mc_mirror_pull(item_id: str, agentfs: str, local: Path) -> None:
    """Pull AgentFS → local workspace (for resumed tasks)."""
    if not agentfs or not shutil.which("mc"):
        return
    try:
        subprocess.run(
            ["mc", "mirror", "--overwrite", "--quiet", agentfs, f"{local}/"],
            capture_output=True, text=True, timeout=120,
        )
        log.info(f"[{item_id}] AgentFS → workspace pulled")
    except Exception as e:
        log.warning(f"[{item_id}] AgentFS pull error: {e}")


def _write_agentfs_meta(
    item: dict, agentfs: str, repo_url: str, branch: str, workspace: Path,
) -> None:
    mc_alias = agentfs.split("/")[0]
    bucket   = agentfs.split("/")[1] if len(agentfs.split("/")) > 1 else "agents"
    try:
        sha = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True, text=True, cwd=str(workspace), timeout=5,
        ).stdout.strip() if (workspace / ".git").exists() else ""
    except Exception:
        sha = ""
    meta = json.dumps({
        "task_id":      item["id"],
        "title":        item.get("title", ""),
        "repo":         repo_url,
        "branch":       branch,
        "sha":          sha,
        "initiated_at": datetime.now(timezone.utc).isoformat(),
        "agent":        AGENT_NAME,
    })
    try:
        subprocess.run(
            ["mc", "pipe", f"{mc_alias}/{bucket}/tasks/{item['id']}/meta.json"],
            input=meta, capture_output=True, text=True, timeout=15,
        )
    except Exception:
        pass


def task_workspace_finalize(
    item_id: str, workspace_local: Path, workspace_agentfs: str, task_output: str,
) -> str:
    """
    Finalize a successfully completed task workspace:
    1. Sync local → AgentFS (durable copy).
    2. Git commit all changes + ONE push to task/<item-id> branch.
    3. Clean up local workspace.

    Returns a short git result string that gets appended to the task result.
    """
    if not workspace_local.exists():
        return ""

    # Final sync to AgentFS
    if workspace_agentfs:
        _mc_mirror_push(item_id, workspace_local, workspace_agentfs)

    # Git push — only if workspace is a git repo
    if not (workspace_local / ".git").exists():
        log.info(f"[{item_id}] No .git in workspace — skipping git push")
        _cleanup_workspace(item_id, workspace_local)
        return ""

    git_result = _git_push_once(item_id, workspace_local, task_output)

    _cleanup_workspace(item_id, workspace_local)
    return git_result


def _git_push_once(item_id: str, workspace: Path, task_output: str) -> str:
    """Commit all changes and push to task branch exactly once."""
    task_branch = f"task/{item_id}"
    try:
        # Nothing to commit?
        status = subprocess.run(
            ["git", "status", "--porcelain"],
            capture_output=True, text=True, cwd=str(workspace), timeout=10,
        )
        if not status.stdout.strip():
            log.info(f"[{item_id}] Workspace clean — no git push")
            return "workspace clean — no changes to push"

        # Switch to task branch
        subprocess.run(
            ["git", "checkout", "-b", task_branch],
            capture_output=True, cwd=str(workspace), timeout=10,
        )

        # Stage all changes
        subprocess.run(
            ["git", "add", "-A"],
            capture_output=True, cwd=str(workspace), timeout=30, check=True,
        )

        # Commit
        commit_msg = (
            f"task({item_id}): complete\n\n"
            f"Agent: {AGENT_NAME}\n\n"
            f"{task_output[:500]}"
        )
        subprocess.run(
            ["git",
             "-c", f"user.email={AGENT_NAME}@ccc",
             "-c", f"user.name={AGENT_NAME}",
             "commit", "-m", commit_msg],
            capture_output=True, cwd=str(workspace), timeout=30, check=True,
        )

        sha = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, cwd=str(workspace), timeout=5,
        ).stdout.strip()

        # ONE push
        remote_url = subprocess.run(
            ["git", "remote", "get-url", "origin"],
            capture_output=True, text=True, cwd=str(workspace), timeout=5,
        ).stdout.strip()

        if not remote_url:
            result = f"committed locally @ {sha} (no remote)"
            log.info(f"[{item_id}] {result}")
            return result

        # Rewrite HTTPS GitHub URL → SSH when deploy key is available.
        # Bootstrap installs the key at ~/.ssh/ccc-deploy-key; HTTPS clones can't
        # use it, but SSH pushes can.
        _deploy_key = Path.home() / ".ssh" / "ccc-deploy-key"
        if (
            _deploy_key.exists()
            and "github.com" in remote_url
            and remote_url.startswith("https://")
        ):
            ssh_url = remote_url.replace("https://github.com/", "git@github.com:", 1)
            subprocess.run(
                ["git", "remote", "set-url", "origin", ssh_url],
                cwd=str(workspace), capture_output=True, timeout=5,
            )
            log.info(f"[{item_id}] Rewrote remote URL to SSH for push")

        push = subprocess.run(
            ["git", "push", "--force-with-lease", "origin", task_branch],
            capture_output=True, text=True, cwd=str(workspace), timeout=120,
        )
        if push.returncode != 0:
            push = subprocess.run(
                ["git", "push", "--set-upstream", "origin", task_branch],
                capture_output=True, text=True, cwd=str(workspace), timeout=120,
            )

        if push.returncode == 0:
            result = f"pushed to {task_branch} @ {sha}"
        else:
            result = f"commit @ {sha} — push failed: {push.stderr.strip()[:200]}"

        log.info(f"[{item_id}] git: {result}")
        return result

    except subprocess.CalledProcessError as e:
        msg = f"git error: {e}"
        log.warning(f"[{item_id}] {msg}")
        return msg
    except Exception as e:
        msg = f"git finalization error: {e}"
        log.warning(f"[{item_id}] {msg}")
        return msg


def _cleanup_workspace(item_id: str, workspace: Path) -> None:
    try:
        shutil.rmtree(workspace)
        log.info(f"[{item_id}] Workspace cleaned up")
    except Exception as e:
        log.debug(f"[{item_id}] Workspace cleanup error: {e}")


def task_workspace_abandon(item_id: str, workspace_local: Path) -> None:
    """Abandon workspace on failure — mirror to AgentFS for debugging, then clean up."""
    if not workspace_local.exists():
        return
    agentfs = _agentfs_workspace_path(item_id)
    if agentfs:
        log.info(f"[{item_id}] Preserving failed workspace in AgentFS: {agentfs}")
        _mc_mirror_push(item_id, workspace_local, agentfs)
    _cleanup_workspace(item_id, workspace_local)


def build_task_env(item_id: str, workspace_local: Path, workspace_agentfs: str) -> dict:
    """Build subprocess environment with task workspace variables."""
    return {
        **os.environ,
        "TASK_ID":                item_id,
        "TASK_WORKSPACE_LOCAL":   str(workspace_local),
        "TASK_WORKSPACE_AGENTFS": workspace_agentfs,
        "TASK_BRANCH":            f"task/{item_id}",
    }


# ── Task prompt ───────────────────────────────────────────────────────────────

def build_task_prompt(item: dict, instructions: str, workspace_local: Path) -> str:
    parts = [
        f"# Queue Item: {item['id']}",
        f"**Title:** {item.get('title', '(no title)')}",
        f"**Priority:** {item.get('priority', 'normal')}",
        f"**Assignee:** {item.get('assignee', AGENT_NAME)}",
        "",
        "## Task Workspace",
        f"Your working directory is: `{workspace_local}`",
        "All file edits must happen inside this directory.",
        "Do NOT run `git commit` or `git push` — the queue-worker handles the single",
        "git push on your behalf after you complete the task.",
        "",
        "## Description",
        item.get("description", "(no description)"),
    ]
    if item.get("notes"):
        parts += ["", "## Notes / Context", item["notes"]]
    if item.get("tags"):
        parts += ["", f"**Tags:** {', '.join(item['tags'])}"]
    parts += [
        "",
        "---",
        "## Your Task",
        "Complete the above work item. Work only within the Task Workspace directory above.",
        "When done, summarize what you did as your final output.",
        "The queue-worker will commit and push your changes automatically.",
    ]
    return "\n".join(parts)


# ── Task execution ────────────────────────────────────────────────────────────

def run_claude(
    prompt: str, item_id: str, task_env: dict, workspace_local: Path,
) -> tuple[str, int]:
    """Run claude -p in the task workspace. Sends keepalives during execution."""
    stop_keepalive = threading.Event()

    def keepalive_loop():
        while not stop_keepalive.wait(KEEPALIVE_INTERVAL):
            post_keepalive(item_id, "claude still working")
            log.info(f"[{item_id}] keepalive sent")

    threading.Thread(target=keepalive_loop, daemon=True).start()

    claude_bin = shutil.which("claude") or "claude"
    try:
        result = subprocess.run(
            [claude_bin, "-p", prompt],
            capture_output=True,
            text=True,
            timeout=CLAUDE_TIMEOUT,
            env=task_env,
            cwd=str(workspace_local),  # run inside task workspace
        )
        output = result.stdout.strip() or result.stderr.strip() or "(no output)"
        if result.returncode != 0 and result.stderr:
            output = f"{output}\n\nSTDERR:\n{result.stderr.strip()}"
        return output, result.returncode
    except subprocess.TimeoutExpired:
        return f"[timed out after {CLAUDE_TIMEOUT}s]", 124
    except FileNotFoundError:
        return "ERROR: 'claude' CLI not found in PATH", 127
    except Exception as e:
        return f"ERROR: {e}", 1
    finally:
        stop_keepalive.set()


# ── Hermes executor ───────────────────────────────────────────────────────────

_HERMES_TAGS = {"hermes", "gpu", "render", "simulation", "omniverse", "isaaclab"}


def _hermes_applicable(item: dict) -> bool:
    tags = set(item.get("tags", []))
    preferred = item.get("preferred_executor", "")
    return bool(tags & _HERMES_TAGS) or preferred == "hermes"


def run_hermes(
    item: dict, item_id: str, task_env: dict, workspace_local: Path,
) -> tuple[str, int]:
    """Route a task to hermes-driver.py, running inside the task workspace."""
    driver = shutil.which("hermes-driver") or str(Path(__file__).parent / "hermes-driver.py")
    query = f"{item.get('title', '')}\n\n{item.get('description', '')}"
    if item.get("notes"):
        query += f"\n\nContext:\n{item['notes']}"

    stop_keepalive = threading.Event()

    def keepalive_loop():
        while not stop_keepalive.wait(KEEPALIVE_INTERVAL):
            post_keepalive(item_id, "hermes still working")
            log.info(f"[{item_id}] keepalive sent")

    threading.Thread(target=keepalive_loop, daemon=True).start()

    try:
        python = shutil.which("python3") or "python3"
        result = subprocess.run(
            [python, driver, "--item", item_id, "--query", query],
            capture_output=True, text=True,
            timeout=86400,
            env=task_env,
            cwd=str(workspace_local),  # run inside task workspace
        )
        output = result.stdout.strip() or result.stderr.strip() or "(no output)"
        return output, result.returncode
    except subprocess.TimeoutExpired:
        return "[hermes-driver timed out]", 124
    except Exception as e:
        return f"ERROR: {e}", 1
    finally:
        stop_keepalive.set()


# ── Item selector ─────────────────────────────────────────────────────────────

def select_item(items: list[dict]) -> dict | None:
    """
    Fan-out model: every worker races to claim every eligible task.
    Eligibility rules (in order):
      1. Status must be pending.
      2. Skip tasks reserved for a human (assignee == "jkh").
      3. Skip tasks explicitly assigned to a *different* named agent.
         Tasks with assignee in ("", "all") are open to everyone.
      4. If required_executors is set, this agent must support at least one.
         (Hard hardware filter — e.g. gpu, hermes. Unusual in practice.)
      5. preferred_executor is advisory only — no filtering.
    Workers race; /api/item/:id/claim is atomic — only one succeeds.
    """
    priority_order = {"urgent": 0, "high": 1, "normal": 2, "medium": 2, "low": 3, "idea": 99}
    candidates = []
    for item in items:
        if item.get("status") != "pending":
            continue

        assignee = item.get("assignee", "")
        # Skip human-reserved tasks
        if assignee == "jkh":
            continue
        # Skip tasks pinned to a different specific agent
        if assignee and assignee not in (AGENT_NAME, "all"):
            continue

        # Hard capability gate: required_executors must intersect our capabilities
        required = set(item.get("required_executors") or [])
        if required and not (required & AGENT_CAPABILITIES):
            log.debug(f"Skipping {item['id']} — requires {required}, have {AGENT_CAPABILITIES}")
            continue

        candidates.append(item)

    if not candidates:
        return None

    candidates.sort(key=lambda x: (
        priority_order.get(x.get("priority", "normal"), 2),
        x.get("created", ""),
    ))
    return candidates[0]


# ── Main loop ─────────────────────────────────────────────────────────────────

def main() -> None:
    log.info(f"Starting queue-worker (agent={AGENT_NAME}, hub={CCC_URL})")
    log.info(f"Capabilities: {sorted(AGENT_CAPABILITIES) or ['(none detected — set AGENT_CAPABILITIES in .env)']}")
    post_heartbeat("queue-worker starting")
    instructions  = load_agent_instructions()
    poll_interval = POLL_INTERVAL_IDLE

    while True:
        if is_quenched():
            log.info("Quenched — skipping this cycle")
            time.sleep(POLL_INTERVAL_IDLE)
            continue

        post_heartbeat("idle")

        try:
            items = get_queue()
        except Exception as e:
            log.warning(f"Failed to fetch queue: {e}")
            time.sleep(POLL_INTERVAL_IDLE)
            continue

        item = select_item(items)
        if item is None:
            log.debug("No claimable items — sleeping")
            time.sleep(poll_interval)
            poll_interval = POLL_INTERVAL_IDLE
            continue

        item_id  = item["id"]
        title    = item.get("title", "?")
        priority = item.get("priority", "normal")

        # ── Claim ─────────────────────────────────────────────────────────────
        if not claim_item(item_id):
            log.info(f"[{item_id}] Claim rejected — backing off")
            time.sleep(POLL_INTERVAL_BUSY)
            continue

        log.info(f"[{item_id}] Claimed [{priority}] {title[:60]}")

        if priority == "urgent":
            _curl("POST", "/bus/send", {
                "from": AGENT_NAME, "to": "all", "type": "text", "subject": "ops",
                "body": f"[URGENT] {title} assigned to {AGENT_NAME} — working now",
            })

        # ── Init workspace ────────────────────────────────────────────────────
        workspace_local, workspace_agentfs = task_workspace_init(item)
        task_env = build_task_env(item_id, workspace_local, workspace_agentfs)
        log.info(f"[{item_id}] Workspace: {workspace_local}"
                 + (f" → AgentFS: {workspace_agentfs}" if workspace_agentfs else ""))

        post_comment(item_id, f"{AGENT_NAME} starting — workspace {workspace_local}")

        # ── Execute ───────────────────────────────────────────────────────────
        if _hermes_applicable(item):
            log.info(f"[{item_id}] Routing to hermes-driver...")
            output, exit_code = run_hermes(item, item_id, task_env, workspace_local)
            log.info(f"[{item_id}] hermes-driver exited {exit_code} ({len(output)} chars)")
        else:
            prompt = build_task_prompt(item, instructions, workspace_local)
            log.info(f"[{item_id}] Invoking claude...")
            output, exit_code = run_claude(prompt, item_id, task_env, workspace_local)
            log.info(f"[{item_id}] claude exited {exit_code} ({len(output)} chars)")

        # ── Finalize ──────────────────────────────────────────────────────────
        if exit_code == 0:
            git_result = task_workspace_finalize(
                item_id, workspace_local, workspace_agentfs, output,
            )
            full_result = output
            if git_result:
                full_result = f"{output}\n\n---\ngit: {git_result}"
            post_complete(item_id, full_result)
            log.info(f"[{item_id}] Completed OK")
        else:
            task_workspace_abandon(item_id, workspace_local)
            post_fail(item_id, f"exit_code={exit_code}\n{output[:1000]}")
            log.info(f"[{item_id}] Failed (exit={exit_code})")

        poll_interval = POLL_INTERVAL_BUSY


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        log.info("Interrupted — exiting")
        sys.exit(0)
