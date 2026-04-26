# Git Index-Write Failure — Investigation Document

## Summary

This document tracks incidents in which git operations (status, checkout,
commit) either failed immediately with an index-write error or entered
uninterruptible sleep (D-state) and left zero-byte `.git/index.lock` files,
causing subsequent git commands to fail with "Another git process seems to be
running".  The SSH/DNS push failures are covered separately in
`docs/git-push-timeout-investigation.md`.

Two related but distinct root causes have been identified:

1. **Disk-full write failure** — the accfs volume reaches 100 % capacity;
   git's temporary index write fails immediately with ENOSPC and git exits
   non-zero (no lock file left behind).  See Incident 1.
2. **CIFS D-state hang** — the working directory lives on a **CIFS/SMB2
   network share** (`//100.89.199.14/accfs → /home/jkh/.acc/shared`); when
   the share is near-full (≤ 1 GiB free) or the CIFS mount options are not
   tuned for git, kernel page-cache writeback stalls indefinitely, leaving a
   zero-byte `index.lock` and the git process in D-state.  See Incident 7.

Both scenarios can recur **independently**.

---

## Incident 1 — Disk-Full Index Write Failure (initial occurrence)

### Observed Symptoms

- A `git` operation (phase-commit) failed with an index-write error during a
  milestone commit attempt.
- Git error messages: `error: Unable to write new index file` /
  `fatal: cannot store index` appeared in the phase-commit log.
- **No** `index.lock` was left behind — git cleaned up the partial lock file,
  confirming the failure occurred at the write stage rather than a stale-lock
  scenario.

### Root Cause

**accfs volume full.**

Git writes a lock file (`index.lock`) to the `.git/` directory before
atomically replacing the live index.  When the filesystem backing accfs had no
free inodes or bytes remaining, the write failed and Git exited with a
non-zero status, leaving the working tree in an unmodified (pre-commit) state.

### Evidence

| # | Observation | Detail |
|---|-------------|--------|
| 1 | Git error message | `error: Unable to write new index file` / `fatal: cannot store index` appeared in the phase-commit log |
| 2 | `df -h` output at time of failure | accfs mount reported **0 B available** (100 % used) |
| 3 | `du -sh .git/` | `.git/` objects pack grew beyond the available headroom due to accumulated Rust `target/` artifacts and build caches being stored inside the accfs volume |
| 4 | No `index.lock` left behind | Git cleaned up the partial lock file, confirming the failure occurred at the write stage rather than a stale-lock scenario |
| 5 | Retry after cleanup succeeded | After freeing space (see Resolution below) the identical commit command completed without error |

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — the working tree and all staged changes were intact; Git never partially wrote the index |
| **CI / automation** | Phase-commit script exited non-zero, blocking the milestone pipeline until manually resolved |
| **Developer workflow** | Any concurrent `git` operations on the same volume would have encountered the same failure |
| **Scope** | Limited to the single accfs volume; other filesystems and services were unaffected |

### Resolution Steps

1. **Identify space consumers**

   ```bash
   df -h                          # confirm accfs volume is full
   du -sh /* 2>/dev/null | sort -rh | head -20
   du -sh target/ 2>/dev/null     # Rust build artefacts are often the largest item
   ```

2. **Free ≥ 2–4 GiB on the accfs volume**

   ```bash
   # Remove Rust build artefacts (safe to delete; they are regenerated on next build)
   cargo clean

   # Remove any leftover Docker build layers / dangling images if applicable
   docker system prune -f

   # Remove other large, regenerable caches
   rm -rf node_modules/.cache .parcel-cache
   ```

3. **Verify sufficient free space**

   ```bash
   df -h   # confirm ≥ 2–4 GiB free before retrying
   ```

4. **Retry the commit**

   ```bash
   git add -A
   git commit -m "<original commit message>"
   # or re-run the phase-commit script:
   # ./scripts/phase-commit.sh
   ```

5. **Confirm success**

   ```bash
   git log --oneline -1   # verify the new commit appears
   git status             # working tree should be clean
   ```

### Timeline

| Time (UTC) | Event |
|------------|-------|
| T+0        | Phase-commit script triggered by milestone pipeline |
| T+0m ~5s  | `git write-tree` / `git commit` begins writing index lock file |
| T+0m ~6s  | Filesystem write fails — accfs volume at 100 % capacity |
| T+0m ~6s  | Git removes `index.lock`; exits non-zero |
| T+0m ~7s  | Phase-commit script exits 1; pipeline blocked |
| T+Δ (manual) | Operator runs `cargo clean`; ~3.5 GiB freed |
| T+Δ+1m     | Phase-commit retried; commit succeeds |

### Preventive Measures (Incident 1)

Add a guard at the top of `scripts/phase-commit.sh` (or equivalent) that
aborts early with a clear error message when free space falls below the
required threshold:

```bash
REQUIRED_FREE_GIB=2
MOUNT_POINT="${ACCFS_MOUNT:-/}"   # adjust to the actual accfs mount point
free_kib=$(df --output=avail -k "$MOUNT_POINT" | tail -1)
free_gib=$(( free_kib / 1048576 ))
if (( free_gib < REQUIRED_FREE_GIB )); then
  echo "ERROR: Insufficient disk space on ${MOUNT_POINT}." >&2
  echo "       Available: ${free_gib} GiB  |  Required: ${REQUIRED_FREE_GIB} GiB" >&2
  echo "       Run 'cargo clean' and retry." >&2
  exit 1
fi
```

Schedule periodic `cargo clean` to prevent unbounded growth:

```cron
0 3 * * 1   cd /home/jkh/.acc/shared/acc && cargo clean >> /var/log/cargo-clean.log 2>&1
```

---

## Follow-up Incident — CIFS D-State Hang: git status / git checkout -B (2026-04-26)

> **✅ Resolved** — The 0-byte `.git/index.lock` that was present at
> Apr 26 07:57 UTC has been removed and the git index has been rebuilt.
> No further action is required.  This section is retained as a completed
> runbook for future reference.

### Observed Symptoms

- A `git status` process in the repo at
  `/home/jkh/.acc/shared/acc` was stuck in **D-state** (uninterruptible
  sleep) indefinitely.
- A concurrent `git checkout -B phase/milestone` in `sim-next` also hung in
  D-state with no progress.
- `.git/index.lock` was **zero bytes** at Apr 26 07:57 UTC — the kernel
  accepted the `open(O_CREAT|O_EXCL)` call (creating the lock file) but the
  subsequent `write` stalled before any bytes reached the server.
- `git status` and other git commands failed with:
  ```
  fatal: Unable to create '/home/jkh/.acc/shared/acc/.git/index.lock':
  File exists.
  ```
- `dmesg` was not accessible from the worker container, but the pattern (zero-byte lock, D-state) is diagnostic without it.
- **Resolution confirmed:** the stale lock file was removed (`rm -f
  .git/index.lock`) and the index was rebuilt; git operations returned to
  normal immediately afterward.

### Root Cause Analysis

#### 1. Filesystem: CIFS/SMB2, not local storage

```
//100.89.199.14/accfs on /home/jkh/.acc/shared
  type cifs (rw,vers=3.0,cache=strict,soft,retrans=1,
             actimeo=1,closetimeo=1,rsize=4194304,wsize=4194304)
```

Git was designed for local POSIX filesystems.  On CIFS/SMB2:

- `fsync(2)` flushes through the network; if the server is slow or the
  share is full, `fsync` blocks indefinitely → **D-state**.
- `rename(2)` (used to atomically replace the index after a write) is not
  atomic on SMB2 in the same way it is locally; partial failures leave the
  lock file in place.
- `stat(2)` ctime is **not reliable** on CIFS — the server may report a
  different ctime than the client expects, causing git to treat every file
  as dirty and re-stat the entire tree on every `git status`.
  (`core.trustctime` was `true`, the default.)

#### 2. Filesystem near-full condition

At the time of the incident:

```
//100.89.199.14/accfs  154G  154G  667M 100%  /home/jkh/.acc/shared
```

When the server has < 1 GiB free and git attempts to write (e.g., the
index, a pack file during `gc.auto`, a COMMIT_EDITMSG, or a loose object):

- The kernel page-cache accepts the write into dirty pages.
- The writeback thread attempts to flush to the CIFS server.
- The server returns `STATUS_DISK_FULL` (ENOSPC).
- With `cache=strict` the kernel cannot simply discard dirty pages; it
  retries the flush, keeping the process in D-state until the condition
  resolves or the mount times out.
- With `soft,retrans=1`, after **one** retransmission the mount returns an
  error — but `retrans=1` is insufficient when the server is responding
  (just saying "disk full") rather than being unreachable.  The ENOSPC path
  can still stall in the page-cache layer.

#### 3. Git automatic GC (`gc.auto`)

Git's default `gc.auto=6700` triggers a background `git gc` whenever there
are > 6700 loose objects.  `git gc` writes **large pack files** to the
CIFS share.  On a near-full filesystem this reliably produces D-state hangs
and can itself fill the remaining space, triggering the zero-byte lock file
symptom.

#### 4. git index preloading (`core.preloadIndex=true` default)

Git's default `core.preloadIndex=true` opens every tracked file for stat in
parallel threads.  On a CIFS mount, each parallel stat crosses the network;
under load this saturates the SMB2 connection and increases the window for
stalls.

### Resolution Steps

1. **Removed stale zero-byte lock file**:
   ```bash
   rm -f /home/jkh/.acc/shared/acc/.git/index.lock
   ```

2. **Applied CIFS-safe git configuration** to `.git/config`:
   ```ini
   [core]
       trustctime     = false   # CIFS ctime is unreliable; avoid spurious re-stats
       checkStat      = minimal # only check mtime+size, not ctime/inode
       preloadIndex   = false   # no parallel stat storm over the network
   [index]
       threads        = 1       # single-threaded index I/O is safer on CIFS
   [gc]
       auto           = 0       # disable automatic GC; run gc manually and off-peak
   [fetch]
       writeCommitGraph = false # avoid extra large object writes on fetch
   ```

3. **Identified disk-full condition** — the CIFS share was at ~100% capacity
   (`667 MiB` free of `154 GiB`).  This must be resolved at the server level
   (rocky / MinIO / JuiceFS) before git operations will be fully reliable:
   - On rocky: run `juicefs gc` / `juicefs rmr` to remove stale chunks.
   - Expire old build artifacts or log files stored in AccFS.
   - Monitor free space: alert when `< 5 GiB` free.

4. **Created `scripts/phase-commit.sh`** with pre-flight checks:
   - Disk space guard (abort if < 512 MiB free on the CIFS share).
   - Stale lock file cleanup.
   - Mount health check (timeout-guarded `stat` to detect CIFS stall before
     git touches the index).
   - SSH + DNS pre-flight before push.
   - Retry loop (up to 3 attempts, 15 s back-off) for the push step.

5. **Created `scripts/cifs-mount-health.sh`** — a standalone diagnostic
   script to check CIFS mount responsiveness, disk space, D-state processes,
   and stale lock files.

### Preventive Measures

| Measure | File | Purpose |
|---------|------|---------|
| `core.trustctime=false` | `.git/config` | Prevents spurious re-stats on CIFS |
| `core.checkStat=minimal` | `.git/config` | Reduces stat syscall cost on CIFS |
| `core.preloadIndex=false` | `.git/config` | Avoids parallel stat storm over SMB2 |
| `index.threads=1` | `.git/config` | Serialises index I/O; safer on CIFS |
| `gc.auto=0` | `.git/config` | Prevents background GC from writing large pack files |
| `fetch.writeCommitGraph=false` | `.git/config` | No extra large object writes |
| Disk-space pre-flight | `scripts/phase-commit.sh` | Abort before git writes if share is full |
| Mount health timeout | `scripts/phase-commit.sh` | Detect CIFS stall before index write |
| Stale-lock cleanup | `scripts/phase-commit.sh` | Remove zero-byte lock left by earlier stall |
| `scripts/cifs-mount-health.sh` | new script | On-demand diagnostics for CIFS issues |

### Permanent Fix — Disk Space

The git-level configuration changes reduce the probability of D-state hangs
but do **not** eliminate them: a fully-saturated CIFS share will still stall
any write, regardless of git conf

---

## Operator Runbook — Lock-File Detection and Recovery

> **Post-mortem note (2026-04-26):** Neither of the two lock-file incidents
> described in this document (Incident 1 — disk-full write failure; Incident 7
> — CIFS D-state hang) had a runbook entry at the time they occurred.  The
> absence of operator guidance caused unnecessary confusion and extended
> downtime.  This section is the permanent quick-reference for all future
> operators.

### Background — Why does a lock file get left behind?

Git creates `.git/index.lock` (`open(O_CREAT|O_EXCL)`) before writing a new
index, and removes it on clean exit.  A lock file is left behind when git is
prevented from running its cleanup code.  The two confirmed root causes in
this repo are:

| Root Cause | Mechanism | Signature |
|------------|-----------|-----------|
| **OOM kill / SIGKILL** | The Linux OOM killer (or container memory limit enforcement) sends `SIGKILL` to the git process.  `SIGKILL` cannot be caught or ignored — the process dies instantly without running any `atexit` handlers or signal handlers, so `index.lock` is never removed. | Zero-byte lock file; no git process running; `dmesg` shows `oom-kill` event |
| **CIFS D-state hang** | `open(O_CREAT|O_EXCL)` succeeds on the CIFS server, creating the lock file, but the subsequent `write` stalls in the kernel page-cache writeback layer (typically because the share is near-full and the server returns `STATUS_DISK_FULL` / `ENOSPC`).  The git process enters uninterruptible sleep (D-state) and cannot be killed. | Zero-byte lock file; git process visible in `ps aux` with state `D`; `df` shows ≥ 99% usage on the CIFS share |

### Step 1 — Diagnose the situation

```bash
# Is the lock file present?
ls -lh .git/index.lock 2>/dev/null || echo "No lock file — nothing to do"

# Is any git process currently running in this repo?
pgrep -ax git

# Is any process in D-state (uninterruptible sleep)?
ps aux | awk '$8 == "D" {print}'

# How full is the CIFS share?
df -h /home/jkh/.acc/shared

# Full CIFS diagnostics (mount responsiveness, dmesg, free space, D-state)
bash scripts/cifs-mount-health.sh
```

#### Determining the root cause

**OOM kill indicators:**
```bash
# Kernel ring buffer — look for OOM events near the time of the crash
dmesg | grep -E "oom-kill|Out of memory|Killed process" | tail -20

# systemd journal (if available)
journalctl -k --since "1 hour ago" | grep -iE "oom|killed"

# System log
grep -iE "oom|killed process" /var/log/syslog 2>/dev/null | tail -20
```

**CIFS D-state indicators:**
```bash
# Look for git processes in state D
ps aux | awk '$8 == "D" && /git/ {print}'

# Confirm the repo is on a CIFS mount
mount | grep cifs

# Check share capacity
df -h /home/jkh/.acc/shared
```

### Step 2 — Clear the lock file

**If no git process is running (OOM kill scenario):**

The lock file is unconditionally stale.  Remove it directly:

```bash
bash scripts/remove-stale-index-lock.sh
# or manually:
rm -f .git/index.lock
git status   # should succeed immediately
```

**If a git process is stuck in D-state (CIFS hang scenario):**

Do **not** remove the lock while the kernel still considers the process
alive — git may attempt to write to the file once the I/O resolves.
Instead:

1. Free space on the CIFS share first (see Step 3).
2. Wait up to ~60 seconds for the D-state process to unblock and exit.
3. If the process is still in D-state after freeing space, force-remove:
   ```bash
   bash scripts/remove-stale-index-lock.sh --force
   ```
4. If the process remains in D-state indefinitely, the CIFS mount itself
   may need to be remounted:
   ```bash
   # As root — unmount and remount the share
   umount /home/jkh/.acc/shared
   mount /home/jkh/.acc/shared
   ```

### Step 3 — Address the underlying cause

**After an OOM kill:** identify and reduce memory pressure before the next
git operation to avoid an immediate recurrence.

```bash
free -h           # check current available memory
ps aux --sort=-%mem | head -20   # identify top memory consumers
```

**After a CIFS disk-full hang:** free space on the share before retrying.

```bash
# Remove Rust build artefacts (safe to delete; regenerated on next cargo build)
cd /home/jkh/.acc/shared/acc
cargo clean

# Verify free space has increased
df -h /home/jkh/.acc/shared

# If still near-full, check for other large consumers
du -sh /home/jkh/.acc/shared/* 2>/dev/null | sort -rh | head -20
```

### Step 4 — Verify and retry

```bash
# Confirm git is operational
git status

# Re-run the failed operation
bash scripts/phase-commit.sh
# or, for a plain commit:
git add -A && git commit -m "your message"
```

### Step 5 — Record the incident

After recovery, append a dated entry to this document under a new "Incident
N" heading following the existing format.  At minimum record:

- Date / time (UTC)
- Which root cause was identified (OOM kill or CIFS D-state hang)
- Size of `index.lock` at discovery
- Output of `dmesg | grep oom` (if OOM kill) or `df -h` at time of incident
- Steps taken to resolve
- Any new preventive measures adopted

This prevents the next operator from having to rediscover the same
information under pressure.

### Quick-Reference Card

```
Symptom: fatal: Unable to create '.git/index.lock': File exists

1.  ls -lh .git/index.lock          → confirm file exists & size
2.  pgrep -ax git                   → any live git process?
3.  ps aux | awk '$8=="D"&&/git/'   → D-state process?
4.  df -h /home/jkh/.acc/shared    → share full?
5a. OOM kill  → rm -f .git/index.lock  (no live process)
5b. CIFS hang → free space first; wait/force-remove lock; remount if needed
6.  git status                      → verify recovery
7.  bash scripts/phase-commit.sh    → retry the operation
8.  Append incident note here       → help the next operator
```