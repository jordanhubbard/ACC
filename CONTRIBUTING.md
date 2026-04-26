# Contributing to ACC

Thank you for contributing to the Agent Command Center (ACC) project.  This
document covers development conventions, the milestone-commit workflow, and —
critically — how to detect and recover from stale git lock files left by
crashes.

---

## Table of Contents

1. [Development Workflow](#development-workflow)
2. [Milestone Commits](#milestone-commits)
3. [Incident Post-Mortem: Stale `.git/index.lock` Files](#incident-post-mortem-stale-gitindexlock-files)
   - [Root Causes](#root-causes)
   - [How to Detect a Stale Lock File](#how-to-detect-a-stale-lock-file)
   - [How to Clear a Stale Lock File](#how-to-clear-a-stale-lock-file)
   - [Preventing Recurrence](#preventing-recurrence)
4. [CIFS / SMB2 Filesystem Notes](#cifs--smb2-filesystem-notes)
5. [Further Reading](#further-reading)

---

## Development Workflow

```bash
# Clone and enter the repo
git clone git@github.com:jordanhubbard/ACC.git
cd ACC

# Build everything
cargo build

# Run the test suite
cargo test

# Start the hub locally
make docker-up
```

For a full environment walkthrough see [GETTING_STARTED.md](GETTING_STARTED.md).

---

## Milestone Commits

Automated agents and human contributors both push milestone commits via the
`scripts/phase-commit.sh` helper.  That script includes pre-flight guards for
disk space, CIFS mount health, stale lock files, DNS resolution, and SSH
connectivity.  Run it instead of bare `git commit && git push` whenever you
are working on a CIFS-backed accfs volume:

```bash
bash scripts/phase-commit.sh --message "your message here"
```

See the inline comments in `scripts/phase-commit.sh` for all available
options and environment-variable overrides.

---

## Incident Post-Mortem: Stale `.git/index.lock` Files

> **Why this section exists:** Two production incidents left a zero-byte
> `.git/index.lock` file on the CIFS-backed `accfs` share, blocking all
> subsequent git operations with the message
> `fatal: Unable to create '.git/index.lock': File exists.`
> Neither incident had any operator runbook entry at the time, causing
> confusion and unnecessary downtime.  This section is the permanent record
> so that the next operator knows exactly what happened and what to do.

### Root Causes

Two distinct failure modes have been observed; both leave a stale
`.git/index.lock`:

#### 1. OOM Kill / SIGKILL during a git write (crash-induced lock)

When the agent process (or any git subprocess) is killed with an
unblockable signal — `SIGKILL` from the Linux OOM killer, a container
runtime enforcing a memory limit, or a manual `kill -9` — the process
terminates instantly without running any cleanup handlers.  Git's normal
exit path removes `index.lock` before exiting; a `SIGKILL` bypasses this
path entirely.

**Signature:** `index.lock` is typically **zero bytes** (the file was
`open(O_CREAT|O_EXCL)`-created but no bytes were written before the kill),
and no git process is running when you check.

**How to confirm OOM kill:**
```bash
# Check the kernel ring buffer for OOM events (run as root or with sudo)
dmesg | grep -E "oom|Out of memory|Killed process" | tail -20

# Check system journal if systemd is available
journalctl -k --since "1 hour ago" | grep -i "oom\|killed"

# Look at recent process accounting / audit log
grep -i "oom\|killed" /var/log/syslog | tail -20
```

#### 2. CIFS/SMB2 D-state hang during a git write (filesystem-induced lock)

The `accfs` share is a **CIFS/SMB2 network mount**
(`//100.89.199.14/accfs → /home/jkh/.acc/shared`).  Git was designed for
local POSIX filesystems.  When the share is near-full (≤ 1 GiB free) or
the SMB2 server is slow, the kernel page-cache writeback thread stalls
trying to flush dirty pages to the server.  The git process enters
**D-state** (uninterruptible sleep) and cannot be killed — even with
`SIGKILL` — until the I/O resolves or the mount is remounted.  The
`open(O_CREAT|O_EXCL)` call that creates `index.lock` succeeded, but the
subsequent `write` never returned.

**Signature:** `index.lock` is **zero bytes**, and a git process is stuck
in D-state (`ps aux | grep " D "` lists it).  The share is typically at
or near 100 % capacity at the time of the incident.

**Confirmed incident (2026-04-26 ~07:57 UTC):**
- `git status` and `git checkout -B phase/milestone` both entered D-state.
- `//100.89.199.14/accfs` reported `667 MiB` free out of `154 GiB` (≈ 100% full).
- `.git/index.lock` was zero bytes.
- Removing the lock file and freeing disk space restored normal operation.

Full technical analysis:
[`docs/git-index-write-failure-investigation.md`](docs/git-index-write-failure-investigation.md)

---

### How to Detect a Stale Lock File

```bash
# 1. Check whether the lock file exists
ls -lh .git/index.lock 2>/dev/null && echo "LOCK FILE PRESENT" || echo "No lock file"

# 2. Check whether any live git process holds the lock
pgrep -ax git

# 3. Check for D-state git processes (uninterruptible sleep = probable CIFS stall)
ps aux | awk '$8 == "D" && /git/ {print}'

# 4. Check CIFS mount free space
df -h /home/jkh/.acc/shared

# 5. Run the bundled diagnostic script for a full report
bash scripts/cifs-mount-health.sh
```

A lock file is **stale** (safe to remove) when:
- The file exists **and**
- No live git process is running in the same repository **or** the only
  process is stuck in D-state with no prospect of completing.

---

### How to Clear a Stale Lock File

**Option A — Use the provided helper script (recommended):**

```bash
bash scripts/remove-stale-index-lock.sh
```

The script detects live git processes and refuses to remove the lock unless
`--force` is passed or no live process is found.  Use `--force` only when
you have confirmed the git process is stuck in D-state and cannot complete.

```bash
# Force-remove even if a (D-state) process appears to be running
bash scripts/remove-stale-index-lock.sh --force

# Dry run — print what would be done without removing anything
bash scripts/remove-stale-index-lock.sh --dry-run
```

**Option B — Manual removal:**

```bash
# Confirm no healthy git process is running first!
pgrep -ax git

# Remove the lock
rm -f .git/index.lock

# Verify git is operational again
git status
```

**If the CIFS share was full**, free space before retrying:

```bash
# Remove Rust build artefacts (safe; regenerated on next build)
cargo clean

# Check free space after cleanup
df -h /home/jkh/.acc/shared

# Retry the failed operation
bash scripts/phase-commit.sh
```

---

### Preventing Recurrence

The following safeguards are already in place in `scripts/phase-commit.sh`
and `scripts/remove-stale-index-lock.sh`.  They address both root causes:

| Safeguard | Addresses |
|-----------|-----------|
| Disk-space pre-flight (abort if < 512 MiB free on CIFS share) | CIFS D-state from full filesystem |
| Timed mount-responsiveness probe (`timeout stat .git/HEAD`) | CIFS D-state — detect stall before writing |
| Stale lock-file cleanup before any git write | Both root causes |
| `core.trustctime=false` / `core.checkStat=minimal` in `.git/config` | Reduces unnecessary CIFS stat pressure |
| `gc.auto=0` in `.git/config` | Prevents background GC from writing large pack files on a full share |
| OOM headroom monitoring | OOM kill — alert before memory is exhausted |

**Operators:** if a crash leaves a lock file and the root cause is unclear,
run the following immediately after clearing the lock:

```bash
# Capture a snapshot for the post-mortem
dmesg | grep -E "oom|Killed" | tail -30          > /tmp/dmesg-oom.txt
df -h                                             > /tmp/df-at-incident.txt
ps auxf                                           > /tmp/ps-at-incident.txt
bash scripts/cifs-mount-health.sh                 > /tmp/cifs-health.txt 2>&1
cat /tmp/dmesg-oom.txt /tmp/df-at-incident.txt /tmp/ps-at-incident.txt /tmp/cifs-health.txt
```

Add a dated entry to `docs/git-index-write-failure-investigation.md` with
the output so the next operator has the full history.

---

## CIFS / SMB2 Filesystem Notes

The working directory (`/home/jkh/.acc/shared/acc`) lives on a CIFS/SMB2
network mount.  Several git defaults behave poorly on CIFS:

| git setting | Problem on CIFS | Recommended value |
|-------------|-----------------|-------------------|
| `core.trustctime` | CIFS ctime is unreliable; causes spurious re-stats | `false` |
| `core.checkStat` | Full stat check crosses the network per file | `minimal` |
| `core.preloadIndex` | Parallel stat storm saturates the SMB2 connection | `false` |
| `index.threads` | Parallel index I/O is unsafe on CIFS | `1` |
| `gc.auto` | Background GC writes large pack files; stalls on full share | `0` |

These are already applied to `.git/config`.  If you clone a fresh copy,
re-apply them:

```bash
git config core.trustctime     false
git config core.checkStat      minimal
git config core.preloadIndex   false
git config index.threads       1
git config gc.auto             0
git config fetch.writeCommitGraph false
```

---

## Further Reading

| Document | Contents |
|----------|----------|
| [`docs/git-index-write-failure-investigation.md`](docs/git-index-write-failure-investigation.md) | Full technical post-mortem for all index-lock and disk-full incidents |
| [`docs/git-push-timeout-investigation.md`](docs/git-push-timeout-investigation.md) | Post-mortem for SSH/DNS push-timeout incidents |
| [`scripts/phase-commit.sh`](scripts/phase-commit.sh) | Milestone commit automation with all pre-flight safeguards |
| [`scripts/remove-stale-index-lock.sh`](scripts/remove-stale-index-lock.sh) | Safe stale-lock removal helper |
| [`scripts/cifs-mount-health.sh`](scripts/cifs-mount-health.sh) | On-demand CIFS mount diagnostics |
