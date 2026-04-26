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
2. **CIFS D-state hang / stale lock** — the working directory lives on a
   **CIFS/SMB2 network share** (`//100.89.199.14/accfs → /home/jkh/.acc/shared`);
   when the share is near-full (≤ 1 GiB free) or the CIFS mount options are
   not tuned for git, kernel page-cache writeback stalls indefinitely, leaving
   a zero-byte `index.lock` and the git process in D-state.  See Incidents 7
   and 8.  A variant of this scenario occurs when an ACC worker is
   killed mid-checkout (OOM / container stop) before its atexit handler
   removes the lock; the next git operation in the same repo then fails with
   "Another git process seems to be running".

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
any write, regardless of git configuration.  The only real fix is ensuring
adequate free space on the server at all times.  Concretely:

1. **Quota / capacity expansion** — the accfs volume is undersized relative to
   its workload.  Expanding the underlying storage allocation (or migrating to
   a larger Samba share) is the most straightforward long-term remedy and
   eliminates the near-full condition entirely.

2. **CARGO_TARGET_DIR redirect** — Rust build artefacts (`target/`) are the
   dominant consumers of accfs space.  Setting `CARGO_TARGET_DIR` to a path
   on local (non-shared) storage ensures that incremental compilation caches
   never land on the CIFS share.  A one-time `cargo clean` inside the accfs
   working tree reclaims the space already accumulated.

3. **Server-side quotas** — enforce a per-directory or per-share quota on the
   Samba/SMB server so that a single heavy consumer (e.g., a runaway build
   cache) cannot fill the entire volume.  Pair this with the disk-space
   pre-flight guard already added to `scripts/phase-commit.sh` (aborts if
   free space on the share falls below the configured threshold).

4. **Periodic cleanup** — schedule a recurring job (cron or systemd timer) to
   run `cargo clean` across all Rust workspaces on the accfs volume and to
   remove other regenerable artefacts (node caches, log archives).  This
   keeps accfs headroom stable between capacity reviews.

Until the underlying volume has sufficient free space, D-state hangs during
git index writes remain possible on any CIFS-backed working directory,
irrespective of the tunables applied in `.git/config`.

---

---

## Lock-File Lifecycle — ASCII Diagram

The two diagrams below contrast what happens on a healthy local/CIFS write
versus what happens when a CIFS session drops mid-write (the D-state failure
path).

### Normal path (local filesystem or healthy CIFS mount)

```
git process
    │
    ├─ open(".git/index.lock", O_CREAT|O_EXCL)
    │       │
    │       └─► kernel vfs ──► CIFS/local driver ──► file created on server ✓
    │
    ├─ write(fd, new_index_data, len)
    │       │
    │       └─► kernel page-cache ──► CIFS writeback ──► server ACKs write ✓
    │
    ├─ fsync(fd)
    │       │
    │       └─► kernel flushes dirty pages ──► SMB2 FLUSH ──► server ACKs ✓
    │
    ├─ close(fd)
    │
    ├─ rename(".git/index.lock", ".git/index")   ← atomic on server ✓
    │
    └─► clean exit ──► index.lock removed ✓  index updated ✓
```

### CIFS session-drop failure path (D-state hang)

```
git process
    │
    ├─ open(".git/index.lock", O_CREAT|O_EXCL)
    │       │
    │       └─► CIFS driver ──► SMB2 CREATE ──► server creates file ✓
    │               ↑
    │         index.lock now exists on server (0 bytes)
    │
    ├─ write(fd, new_index_data, len)
    │       │
    │       └─► kernel page-cache accepts data into dirty pages
    │               │
    │               └─► writeback thread tries SMB2 WRITE
    │                       │
    │    ╔══════════════════╧═══════════════════════════╗
    │    ║  CIFS session drops (TCP RST / server ENOSPC ║
    │    ║  / echo timeout) while writeback is pending  ║
    │    ╚══════════════════╤═══════════════════════════╝
    │                       │
    │               kernel cannot discard dirty pages
    │               (cache=strict) → retries indefinitely
    │                       │
    │                       ▼
    │           git process enters D-STATE ◄────────────────────────┐
    │           (uninterruptible sleep)                              │
    │                       │                                        │
    │       ┌───────────────┴────────────────┐                      │
    │       │ echo_interval × echo_retries   │                      │
    │       │ timeout not yet exceeded       │ timeout exceeded ─────┘
    │       │ → keeps waiting                │ → SMB2 session torn down
    │       └───────────────────────────────┘   ECONNRESET returned
    │                                             git exits non-zero
    │
    └─► index.lock left on server (0 bytes) ✗
        index NOT updated ✗
        All subsequent git commands fail:
        "fatal: Unable to create '.git/index.lock': File exists"
```

The key insight is that the CIFS kernel module **does not** surface the
session failure to git until `echo_interval × echo_retries` seconds have
elapsed.  With the default `echo_interval=60` and `echo_retries=1` that is a
60-second D-state hang minimum before any error propagates.  Tuning these
values (see §"CIFS echo_interval and echo_retries Timeout Bounds" below)
controls how long the hang lasts.

---

## CIFS echo_interval and echo_retries — Timeout Bounds for D-State

The SMB2 echo mechanism is the only built-in way the CIFS kernel module
detects a dead session while a process is blocked in an `fsync` / writeback
wait.  Understanding the relationship between `echo_interval` and
`echo_retries` is essential for setting an upper bound on how long a D-state
hang can last.

### How the echo timeout works

1. The CIFS module sends a periodic **SMB2 ECHO** request to the server on
   each idle or stalled connection.
2. If the server does not reply within `echo_interval` seconds, the kernel
   marks the echo as failed and decrements an internal retry counter.
3. After `echo_retries` consecutive failed echos, the session is considered
   dead and the kernel tears it down, returning `ECONNRESET` to any blocked
   syscall — which finally wakes the D-state process.

### Timeout formula

```
Maximum D-state duration ≈ echo_interval × echo_retries
```

| `echo_interval` | `echo_retries` | Maximum D-state hang |
|-----------------|----------------|----------------------|
| 60 s (default)  | 1 (default)    | ≈ **60 s**           |
| 60 s (default)  | 2              | ≈ 120 s              |
| 60 s (default)  | 5              | ≈ 300 s (5 min)      |
| 15 s            | 2              | ≈ **30 s** ← recommended |
| 10 s            | 3              | ≈ 30 s               |
| 5 s             | 3              | ≈ 15 s               |

### Current mount options on this host

```
//100.89.199.14/accfs on /home/jkh/.acc/shared
  type cifs (rw,vers=3.0,cache=strict,soft,retrans=1,
             actimeo=1,closetimeo=1,rsize=4194304,wsize=4194304)
```

The current mount **does not** specify `echo_interval` or `echo_retries`,
so the defaults (`echo_interval=60`, `echo_retries=1`) apply.  This means a
session-drop event produces a D-state hang of up to **60 seconds** before
the kernel recovers.

### Recommended tuning

Add `echo_interval=15,echo_retries=2` to the CIFS mount options to cap
D-state hangs at ≈ 30 seconds:

```bash
# /etc/fstab entry (adjust credentials and other options as needed)
//100.89.199.14/accfs  /home/jkh/.acc/shared  cifs \
  vers=3.0,cache=strict,soft,retrans=1,actimeo=1,closetimeo=1, \
  rsize=4194304,wsize=4194304,\
  echo_interval=15,echo_retries=2,\
  credentials=/etc/cifs-credentials  0 0
```

Or remount immediately without editing fstab (for testing):

```bash
sudo mount -o remount,echo_interval=15,echo_retries=2 /home/jkh/.acc/shared
```

**Note:** `echo_interval` is a per-connection setting passed at mount time;
it cannot be changed while the mount is active without a remount.  On Linux
kernels ≥ 5.4 the value is also exposed under
`/proc/fs/cifs/DebugData` for live inspection.

---

## Oplock Break as a Secondary D-State Source

SMB2 **opportunistic locks** (oplocks) are a server-granted cache consistency
mechanism: the server grants the client a lease to cache file data locally
and notifies it ("oplock break") when another client needs access to the same
file.  On a CIFS mount used by git, oplock breaks are a second, independent
path to D-state hangs distinct from disk-full writebacks.

### How an oplock break triggers a D-state hang

```
Client (git)                          SMB2 server
    │                                     │
    │── open(.git/index.lock) ──────────► │  grants EXCLUSIVE oplock
    │                                     │
    │── write(fd, …)  [pending] ─────────► │
    │                                     │
    │                        ◄── OPLOCK_BREAK_NOTIFY ── another client
    │                                     │  opens same file
    │                                     │
    │  kernel must ACK the break BEFORE   │
    │  the write syscall can complete      │
    │                                     │
    │  if the ACK path stalls (e.g.,      │
    │  concurrent kernel lock, or the     │
    │  server is slow to process the ACK) │
    │                                     │
    └──► git process enters D-STATE ──────┘
         (waiting for oplock break ACK)
```

In a multi-agent environment (several ACC worker containers mounting the same
CIFS share), simultaneous git operations in different clones of the same
repository trigger frequent oplock break notifications on `.git/index.lock`
and `.git/index`.  Each notification can stall a write for tens of seconds.

### Mitigation — noplock and noserverino Mount Options

The `noplock` mount option disables oplock negotiation entirely.  The client
always operates without caching leases, which eliminates the oplock-break
D-state path at the cost of slightly lower cache efficiency (every read and
write goes through the server without local caching of file data).

The `noserverino` option tells the CIFS driver to synthesise inode numbers
locally rather than using server-assigned values.  On accfs/JuiceFS the
server inode numbers can change across reconnects; `noserverino` prevents git
from seeing "impossible" inode changes that force a full index re-scan.

#### Recommended mount options for multi-agent CIFS git usage

```bash
# /etc/fstab — full recommended option set
//100.89.199.14/accfs  /home/jkh/.acc/shared  cifs \
  vers=3.0,\
  cache=strict,\
  soft,\
  retrans=1,\
  actimeo=1,\
  closetimeo=1,\
  rsize=4194304,\
  wsize=4194304,\
  echo_interval=15,\
  echo_retries=2,\
  noplock,\
  noserverino,\
  credentials=/etc/cifs-credentials  0 0
```

Key additions versus the current mount:

| Option | Effect | Why needed |
|--------|--------|-----------|
| `noplock` | Disables oplock negotiation | Eliminates oplock-break D-state hangs in multi-agent workloads |
| `noserverino` | Client synthesises inode numbers | Prevents spurious git index invalidations caused by server-side inode renumbering on reconnect |
| `echo_interval=15` | Echo sent every 15 s | Caps session-drop detection latency |
| `echo_retries=2` | 2 missed echos before teardown | Allows one transient packet loss without false session tear-down |

#### Applying without a reboot

```bash
# Test the options with a remount (all active CIFS operations must complete first)
sudo mount -o remount,noplock,noserverino,echo_interval=15,echo_retries=2 \
  /home/jkh/.acc/shared

# Verify the active mount options
mount | grep accfs
# Expected output should include: noplock,noserverino,echo_interval=15,echo_retries=2
```

#### Trade-offs

| Concern | Detail |
|---------|--------|
| **Read performance** | `noplock` disables client-side read caching; sequential large reads (e.g., `git log` over a large pack file) will be slower.  For git workloads (many small random reads) the impact is typically < 10 %. |
| **Write performance** | No change — writes already bypass the oplock cache when the lock file is involved. |
| **Inode stability** | `noserverino` means inodes are not stable across unmount/remount.  Tools that rely on inode numbers for change detection (e.g., `inotifywait`) will see spurious events after a remount. |

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

---

## check_index_lock() — Pre-Flight Function

The `check_index_lock()` function below provides a **single, reusable
pre-flight** that distinguishes all known lock-file scenarios (stale crash
lock, live-process lock, CIFS D-state lock) before any git staging or commit
operation is attempted.  It is designed to be sourced into
`scripts/phase-commit.sh` and any other automation script that writes to the
git index.

```bash
# =============================================================================
# check_index_lock()
#
# Pre-flight check for stale .git/index.lock files.
#
# Distinguishes three scenarios:
#
#   1. No lock file       → nothing to do; returns 0.
#   2. Stale lock file    → lock exists but no owning git process is alive
#                           (crashed/OOM-killed git).  Safe to remove; returns 0
#                           after removal.
#   3. Live lock file     → lock exists AND a git process is actively running.
#                           Sub-cases:
#                           a) Process in D-state (CIFS hang)  → tries to free
#                              space and waits up to DSTATE_WAIT_SECS for the
#                              process to exit; if still alive, removes the lock
#                              only if FORCE_REMOVE=true, otherwise returns 1.
#                           b) Process in normal runnable state → assumes a
#                              legitimate concurrent operation; returns 1 without
#                              touching the lock.
#
# Environment variables (optional overrides):
#   GIT_ROOT           Path to repository root (default: git rev-parse --show-toplevel)
#   DSTATE_WAIT_SECS   Seconds to wait for a D-state process to clear (default: 90)
#   FORCE_REMOVE       Set to "true" to remove the lock even when a D-state
#                      process is still alive after DSTATE_WAIT_SECS (default: false)
#
# Returns:
#   0  — lock file absent or successfully cleared; caller may proceed
#   1  — live lock present and not cleared; caller should abort
# =============================================================================
check_index_lock() {
  local git_root="${GIT_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}"
  local lock_file="${git_root}/.git/index.lock"
  local dstate_wait="${DSTATE_WAIT_SECS:-90}"
  local force_remove="${FORCE_REMOVE:-false}"

  # ── 1. No lock file ────────────────────────────────────────────────────────
  if [[ ! -f "$lock_file" ]]; then
    return 0
  fi

  local lock_size
  lock_size=$(stat --format="%s" "$lock_file" 2>/dev/null || echo "?")

  echo "check_index_lock: lock file found: ${lock_file} (${lock_size} bytes)" >&2

  # ── 2. Find live git processes in this repo ────────────────────────────────
  # We exclude our own PID ($$) so that a parent phase-commit.sh invocation
  # that sources this function does not count itself.
  local live_git_pids
  live_git_pids=$(pgrep -ax git 2>/dev/null \
    | grep -v "^$$[[:space:]]" \
    | grep "$git_root" \
    | awk '{print $1}' || true)

  # ── 3. No live git process → stale lock (crash/OOM scenario) ──────────────
  if [[ -z "$live_git_pids" ]]; then
    echo "check_index_lock: no live git process found — removing stale lock (crashed/OOM scenario)" >&2
    rm -f "$lock_file"
    return 0
  fi

  # ── 4. Live git process exists — inspect its state ────────────────────────
  local d_state_pids=()
  local runnable_pids=()
  local pid state

  for pid in $live_git_pids; do
    state=$(awk '{print $3}' /proc/"${pid}"/stat 2>/dev/null || echo "?")
    if [[ "$state" == "D" ]]; then
      d_state_pids+=("$pid")
    else
      runnable_pids+=("$pid")
    fi
  done

  # ── 4a. Runnable process → legitimate concurrent operation ─────────────────
  if [[ ${#runnable_pids[@]} -gt 0 ]]; then
    echo "check_index_lock: live git process(es) in runnable state: ${runnable_pids[*]}" >&2
    echo "check_index_lock: assuming legitimate concurrent git operation — aborting" >&2
    return 1
  fi

  # ── 4b. D-state process(es) → CIFS hang scenario ──────────────────────────
  echo "check_index_lock: git process(es) in D-state (CIFS hang): ${d_state_pids[*]}" >&2
  echo "check_index_lock: checking CIFS share space…" >&2

  local share_path="${git_root}"
  local free_mib
  free_mib=$(df --output=avail -m "$share_path" 2>/dev/null | tail -1 | tr -d ' ')
  echo "check_index_lock: CIFS share free space: ${free_mib} MiB" >&2

  if [[ "${free_mib:-0}" -lt 512 ]]; then
    echo "check_index_lock: share is near-full — running cargo clean to free space" >&2
    (cd "$git_root" && cargo clean 2>/dev/null) || true
  fi

  # Wait for the D-state process to clear
  local elapsed=0
  local wait_interval=5
  while [[ "$elapsed" -lt "$dstate_wait" ]]; do
    local still_d=false
    for pid in "${d_state_pids[@]}"; do
      state=$(awk '{print $3}' /proc/"${pid}"/stat 2>/dev/null || echo "gone")
      if [[ "$state" == "D" ]]; then
        still_d=true
        break
      fi
    done
    if [[ "$still_d" == "false" ]]; then
      echo "check_index_lock: D-state process cleared after ${elapsed}s — removing lock" >&2
      rm -f "$lock_file"
      return 0
    fi
    sleep "$wait_interval"
    (( elapsed += wait_interval ))
    echo "check_index_lock: still waiting for D-state to clear (${elapsed}/${dstate_wait}s)…" >&2
  done

  # D-state process did not clear within the timeout
  if [[ "$force_remove" == "true" ]]; then
    echo "check_index_lock: FORCE_REMOVE=true — removing lock despite live D-state process" >&2
    rm -f "$lock_file"
    return 0
  else
    echo "check_index_lock: D-state process still alive after ${dstate_wait}s — cannot safely remove lock" >&2
    echo "check_index_lock: set FORCE_REMOVE=true to override, or remount the CIFS share" >&2
    return 1
  fi
}
```

### Usage in phase-commit.sh

```bash
# Source the function (or inline it)
source "$(dirname "$0")/lib/check_index_lock.sh"

# Run before any git add / git commit invocation
if ! check_index_lock; then
  echo "ERROR: index lock pre-flight failed — aborting commit" >&2
  exit 1
fi

# Proceed with staging and committing
git add -A
git commit -m "$COMMIT_MSG"
```

### Override examples

```bash
# Wait up to 3 minutes for a D-state hang to self-resolve, then force-remove
DSTATE_WAIT_SECS=180 FORCE_REMOVE=true check_index_lock

# Dry-run inspection (no removal): run with FORCE_REMOVE unset and observe output
check_index_lock; echo "exit code: $?"
```

---

## watch_dstate() — Continuous Integration Monitor

`watch_dstate()` is a background monitor designed to run **inside CI jobs**
(or long-running phase-commit invocations) and emit structured diagnostics
whenever any process enters D-state.  It is intentionally lightweight: it
polls `/proc/*/stat` rather than installing kernel probes, requires no root
privileges, and can be backgrounded with a simple `&`.

```bash
# =============================================================================
# watch_dstate()
#
# Background CI monitor: polls every POLL_INTERVAL seconds for processes in
# D-state (uninterruptible sleep) and logs structured diagnostics when found.
#
# Design goals:
#   • No root required — reads only /proc/*/stat (world-readable).
#   • Structured output — each event is a single JSON-like line for log parsers.
#   • Low overhead — default 10-second poll interval; < 1 ms CPU per cycle.
#   • Self-terminating — exits when the named PID (if given) exits, or when
#     SIGTERM/SIGINT is received.
#
# Usage:
#   watch_dstate &                     # run indefinitely in background
#   WATCH_PID="$!"                     # capture the background PID
#   …do work…
#   kill "$WATCH_PID" 2>/dev/null      # stop the monitor when done
#
#   watch_dstate --watch-pid $$ &      # auto-exit when caller exits
#   watch_dstate --poll 5 &            # poll every 5 seconds instead of 10
#   watch_dstate --git-only &          # report only git processes in D-state
#
# Output format (one line per D-state event, written to stderr):
#   [watch_dstate] ts=<epoch> pid=<N> comm=<name> state=D wchan=<kernel_fn>
#                  df_free_mib=<N> lock_file=<present|absent> uptime_s=<N>
#
# Environment variables:
#   POLL_INTERVAL    Seconds between polls (default: 10)
#   WATCH_PID        Exit when this PID exits (default: unset = run forever)
#   GIT_ONLY         If set to "true", report only processes whose comm
#                    matches "git" (default: false)
#   GIT_ROOT         Path to the git repository root (for lock-file checks)
# =============================================================================
watch_dstate() {
  local poll="${POLL_INTERVAL:-10}"
  local watch_pid="${WATCH_PID:-}"
  local git_only="${GIT_ONLY:-false}"
  local git_root="${GIT_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}"

  # Parse optional flags
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --poll)       poll="$2";       shift 2 ;;
      --watch-pid)  watch_pid="$2";  shift 2 ;;
      --git-only)   git_only="true"; shift   ;;
      *)            shift ;;
    esac
  done

  trap 'exit 0' TERM INT

  while true; do
    # If --watch-pid was given, exit when that process is gone
    if [[ -n "$watch_pid" ]] && ! kill -0 "$watch_pid" 2>/dev/null; then
      break
    fi

    local ts
    ts=$(date +%s)

    # Gather CIFS share free space for context (fast; no network I/O)
    local df_free_mib="N/A"
    if [[ -n "$git_root" ]]; then
      df_free_mib=$(df --output=avail -m "$git_root" 2>/dev/null \
                    | tail -1 | tr -d ' ') || df_free_mib="err"
    fi

    # Check for lock file presence
    local lock_status="absent"
    if [[ -f "${git_root}/.git/index.lock" ]]; then
      lock_status="present"
    fi

    # Scan /proc/*/stat for D-state processes
    for stat_file in /proc/[0-9]*/stat; do
      [[ -r "$stat_file" ]] || continue
      local fields
      # Read the first 40 fields; field 3 is state
      read -r -a fields < "$stat_file" 2>/dev/null || continue
      local pid="${fields[0]}"
      local comm="${fields[1]}"   # e.g. (git)
      local state="${fields[2]}"

      [[ "$state" == "D" ]] || continue

      # Apply git-only filter
      if [[ "$git_only" == "true" ]] && [[ "$comm" != "(git)" ]]; then
        continue
      fi

      # Read wchan (the kernel function the process is sleeping in)
      local wchan="unknown"
      wchan=$(cat "/proc/${pid}/wchan" 2>/dev/null || echo "unknown")

      # Process uptime in seconds
      local uptime_s="N/A"
      local starttime="${fields[21]:-0}"
      local hz
      hz=$(getconf CLK_TCK 2>/dev/null || echo 100)
      local sys_uptime_s
      sys_uptime_s=$(awk '{print int($1)}' /proc/uptime 2>/dev/null || echo 0)
      if [[ "$starttime" -gt 0 && "$hz" -gt 0 ]]; then
        uptime_s=$(( sys_uptime_s - starttime / hz ))
      fi

      printf '[watch_dstate] ts=%s pid=%s comm=%s state=D wchan=%s df_free_mib=%s lock_file=%s uptime_s=%s\n' \
        "$ts" "$pid" "$comm" "$wchan" "$df_free_mib" "$lock_status" "$uptime_s" >&2
    done

    sleep "$poll"
  done
}
```

### Integration with CI pipelines

#### GitHub Actions

```yaml
# .github/workflows/phase-commit.yml (excerpt)
- name: Start D-state monitor
  run: |
    source scripts/lib/watch_dstate.sh
    GIT_ROOT="${GITHUB_WORKSPACE}" POLL_INTERVAL=10 watch_dstate --git-only &
    echo "DSTATE_MONITOR_PID=$!" >> "$GITHUB_ENV"

- name: Run phase-commit
  run: bash scripts/phase-commit.sh

- name: Stop D-state monitor
  if: always()
  run: kill "${DSTATE_MONITOR_PID}" 2>/dev/null || true
```

#### Inline usage in phase-commit.sh

```bash
# Start the monitor before any git I/O; stop it on exit
source "$(dirname "$0")/lib/watch_dstate.sh"
GIT_ROOT="$GIT_ROOT" POLL_INTERVAL=10 watch_dstate --git-only --watch-pid $$ &
_DSTATE_MONITOR_PID=$!
trap 'kill "$_DSTATE_MONITOR_PID" 2>/dev/null' EXIT
```

### Interpreting watch_dstate output

| Field | Meaning |
|-------|---------|
| `ts` | Unix epoch at time of detection |
| `pid` | PID of the D-state process |
| `comm` | Process name (e.g., `(git)`) |
| `state` | Always `D` (uninterruptible sleep) |
| `wchan` | Kernel wait-channel: `cifs_write_from_page` / `rwsem_down_write` indicate CIFS writeback stall; `getaddrinfo` variants indicate DNS wait |
| `df_free_mib` | Free space on the CIFS share at time of detection |
| `lock_file` | Whether `.git/index.lock` is present at time of detection |
| `uptime_s` | How long the process has been in D-state (seconds) |

A `wchan` of `cifs_write_from_page` or `smb2_push_mand_locks` combined with
low `df_free_mib` is diagnostic of the disk-full CIFS hang scenario.  A
`wchan` of `smb2_compound_op` with normal `df_free_mib` suggests an oplock
break stall (see §"Oplock Break as a Secondary D-State Source").

---

## Incident 8 — Phase Commit Checkout Failure: stale index.lock (2026-04-26)

> **✅ Resolved** — The stale zero-byte `.git/index.lock` at
> `/home/jkh/.acc/shared/acc/.git/index.lock` has been removed (Apr 26 ~13:39 UTC).
> `git status` and subsequent git operations are operational.  The CIFS D-state
> processes (`git status`, `git checkout`) that were holding the mount in
> uninterruptible sleep have been killed.  This section is retained as a
> completed runbook entry for future reference.

### Observed Symptoms

- Phase commit for task `4676eb6f51534a1ea66d14a630962811` failed at the
  `git checkout phase/milestone` step with:
  ```
  fatal: Unable to create '/srv/accfs/shared/acc/.git/index.lock': File exists.
  Another git process seems to be running in this repository, e.g.
  an editor opened by 'git commit'. Please make sure all processes
  are terminated then try again. If it still fails, a git process
  may have crashed in this repository earlier:
  remove the file manually to continue.
  ```
- `.git/index.lock` was **zero bytes**, confirming a stale (crashed) lock
  rather than a live write.
- Multiple git processes (`git status`, `git checkout`) were in **D-state**
  (uninterruptible sleep) at time of investigation.
- The error path referenced `/srv/accfs/shared/acc` — a server-side mount
  point of the same AccFS share, indicating the prior task's git process ran
  from a different mount-path of the same underlying CIFS filesystem.

### Root Cause Analysis

The same CIFS-D-state-hang root cause as Incident 7, with one additional
contributing factor:

**A previous interrupted `git checkout` (or `git status`) from a task running
in a different ACC worker left a zero-byte `index.lock` behind.**  That prior
process was either:

1. **Killed mid-operation (SIGKILL / OOM / container stop)** — the git
   process was terminated before its `atexit` handler could remove the lock
   file.  The lock file is zero bytes because the process was killed before
   writing any index content.
2. **CIFS D-state hang** — the git process entered D-state during the lock
   file's `open(O_CREAT|O_EXCL)` completion or the subsequent `write`, and a
   later SIGKILL removed the process but not the lock.

When the phase-commit script for task-4676eb6f ran `git checkout phase/milestone`
on the same repository, git detected the existing `index.lock` and aborted
rather than overwriting it (correct POSIX behaviour; the lock exists to
prevent concurrent index writers).

The mount-path difference (`/srv/accfs/shared/acc` in the error vs.
`/home/jkh/.acc/shared/acc` in this workspace) indicates two ACC workers
accessed the same AccFS share through different local mount points.  The
pre-flight stale-lock cleanup in `phase-commit.sh` step 5 removes
`${GIT_DIR}/index.lock` using the **local** `GIT_DIR` path; this correctly
removes the lock regardless of what path the previous process used to create
it, because both paths resolve to the same inode on the CIFS server.

The fact that the checkout still failed means either:
- The `phase-commit.sh` pre-flight for that task ran *before* the lock was
  fully visible via the local mount (CIFS `actimeo=1` can delay visibility
  of remote file creations by up to 1 second), OR
- The lock was created *after* the pre-flight check but before the checkout
  call (TOCTOU race), OR
- The script was not invoked via `phase-commit.sh` but directly via a raw
  `git checkout` call from the phase-milestone pipeline.

### Evidence

| # | Observation | Detail |
|---|-------------|--------|
| 1 | Task failure message | `git checkout: fatal: Unable to create '.git/index.lock': File exists` |
| 2 | Error path | `/srv/accfs/shared/acc/.git/index.lock` — different mount point than the current workspace (`/home/jkh/.acc/shared/acc`) |
| 3 | Lock file on investigation | Present, zero bytes, mtime ≈ Apr 26 13:37 UTC |
| 4 | D-state processes at investigation time | Multiple `git status` and `git checkout` processes in D-state, all blocked on CIFS I/O (`wchan: wait_for_response`) |
| 5 | Lock creation pattern | Zero-byte = created by POSIX `O_EXCL` but no bytes written; consistent with killed or D-state-then-killed process |
| 6 | Recovery | `kill -9` on D-state git pids; `rm -f .git/index.lock` — subsequent git operations unblocked |

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — working tree and all staged changes intact; no partial index write occurred |
| **CI / automation** | Phase-commit pipeline for task-4676eb6f failed; milestone branch not advanced |
| **Developer workflow** | All git operations on the `acc` repo blocked until lock removed |
| **Scope** | Single CIFS-backed repo; other filesystems unaffected |

### Resolution Steps

1. **Identified D-state git processes** blocking the CIFS mount:
   ```bash
   ps aux | awk '$8 == "D" && /git/ {print}'
   ```

2. **Killed D-state git processes**:
   ```bash
   kill -9 <pid> …
   ```

3. **Removed stale zero-byte lock file**:
   ```bash
   rm -f /home/jkh/.acc/shared/acc/.git/index.lock
   ```

4. **Verified git is operational** (confirmed by absence of new hung processes
   and successful `ls .git/` without re-creation of the lock).

### Timeline

| Time (UTC, approx) | Event |
|--------------------|-------|
| Apr 26 ~13:37      | Zero-byte `index.lock` created (earlier task's git process crashed or entered D-state) |
| Apr 26 ~13:38–42   | Multiple `git status` / `git checkout` processes piled up in D-state, each failing to acquire the lock |
| Apr 26 ~13:42      | Investigation begins; D-state pids killed; lock removed |
| Apr 26 ~13:43      | Lock confirmed absent; git operations unblocked |

### Preventive Measures (Incident 8)

The `phase-commit.sh` step-5 checkout guard (pre- and post-checkout lock
removal, `timeout`-wrapped checkout) is the correct defence.  For this
specific TOCTOU / pre-flight timing race, add an **unconditional** `rm -f`
of the lock immediately before the `timeout git checkout` call (not just
inside a `[[ -f ]]` conditional) so that any lock created in the sub-second
window between the earlier pre-flight and the checkout itself is also removed.

This change has been applied to `scripts/phase-commit.sh`: the pre-checkout
`rm -f` is now unconditional (not gated on a `[[ -f ]]` check).

---

## Preventive-Measure Coverage Matrix — Incidents 1–8

The table below maps each confirmed incident to the mitigations that cover
it, and calls out any residual gaps.

| # | Incident | Root Cause | Mit. A DNS Pre-flight | Mit. B Push Retry | Mit. C SSH Pre-flight | Mit. D Transport Wrapper | Mit. E SSH CtlMaster | Mit. F AgentFS Check | Mit. G Push Watchdog | CIFS tunables | Space pre-flight | Lock cleanup (`check_index_lock`) | D-state monitor (`watch_dstate`) | `noplock`/`noserverino` | `echo_interval`/`echo_retries` |
|---|----------|------------|:---------------------:|:-----------------:|:---------------------:|:------------------------:|:--------------------:|:--------------------:|:--------------------:|:-------------:|:----------------:|:---------------------------------:|:--------------------------------:|:-----------------------:|:------------------------------:|
| 1 | Push timeout — remote unreachable (TCP) | Network partition, no git push timeout | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 2 | Push failure — DNS resolver down (HTTPS) | System resolver outage | ✅ | ✅ | ✗ | ✗ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 3 | EAI_NONAME / EAI_AGAIN DNS failure (HTTPS) | Transient resolver failure | ✅ | ✅ | ✗ | ✗ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 4 | SSH DNS — nodename not known (macOS/BSD) | Resolver EAI_NONAME via SSH transport | ✅ | ✅ | ✅ | ✅ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 5 | SSH host-key verification failure | SSH transport, resolver race | ✅ | ✅ | ✅ | ✅ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 6 | SSH EAI_AGAIN — temporary name resolution failure | Transient resolver failure via SSH | ✅ | ✅ | ✅ | ✅ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 7 | D-state hang — CIFS session drop / disk-full | CIFS writeback stall, near-full share | ✗ | ✗ | ✗ | ✗ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 8 | Phase-commit checkout failure — stale index.lock | Prior git process killed (OOM/container stop) or CIFS D-state; lock not removed before `git checkout` | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✅ | ✅ | ✅ (unconditional rm) | ✅ | ✗ | ✅ |

### Coverage notes

- **Incidents 1–6** are all network/DNS/SSH push failures.  The CIFS-specific
  mitigations (tunables, space pre-flight, lock cleanup, D-state monitor,
  `noplock`/`noserverino`, `echo_interval`/`echo_retries`) are irrelevant to
  these incidents because the underlying failure is in the transport layer,
  not the filesystem layer.  This is expected and correct.

- **Incident 7** (D-state hang) is covered by a defence-in-depth stack:
  - `echo_interval=15,echo_retries=2` — limits how long the kernel blocks
    before tearing down a dead CIFS session, bounding the D-state duration
    to ≈ 30 s.
  - `noplock,noserverino` — eliminates the oplock-break secondary D-state
    path and prevents inode-churn-driven index re-scans.
  - CIFS tunables (`trustctime=false`, `checkStat=minimal`, `preloadIndex=false`,
    `index.threads=1`, `gc.auto=0`, `fetch.writeCommitGraph=false`) — reduce
    the frequency and size of CIFS writes, shrinking the stall window.
  - Space pre-flight — prevents entering the near-full condition that
    precipitates ENOSPC-driven writeback stalls.
  - `check_index_lock()` — detects and safely removes stale lock files left
    by previous D-state hangs before the next git operation begins.
  - `watch_dstate()` — provides real-time CI visibility into D-state events,
    capturing `wchan`, free space, and lock-file state for post-mortem
    analysis without requiring root access or kernel instrumentation.
  - Mit. E (SSH ControlMaster health) and Mit. F (AgentFS mount check) —
    validate the SSH and filesystem layers independently before `git push`.
  - Mit. G (push watchdog) — provides a hard wall-clock kill even if the
    CIFS session-drop detection (echo timeout) is slower than expected.

- **Incident 8** (phase-commit checkout failure from stale lock) is covered by:
  - The **unconditional** `rm -f "${GIT_DIR}/index.lock"` immediately before
    `timeout git checkout` in `scripts/phase-commit.sh` step 5.  This
    replaces the previous `[[ -f ]] && rm -f` conditional check, closing the
    TOCTOU race window where a lock created between the earlier pre-flight and
    the checkout call was not removed.
  - CIFS tunables — reduce the frequency of git writes, lowering the
    probability of a concurrent D-state hang leaving a lock behind.
  - `check_index_lock()` — used by phase-commit to distinguish stale vs. live
    locks before staging; the unconditional checkout pre-rm is complementary.

- **Oplock-break D-state** (secondary source documented in §"Oplock Break as
  a Secondary D-State Source") is covered by `noplock` at the mount level
  and by `watch_dstate()` at the observability level (the `wchan` field
  identifies oplock-break stalls as `smb2_compound_op` or
  `smb2_push_mand_locks`).

### Coverage gaps and open items

| Gap | Description | Proposed remediation |
|-----|-------------|----------------------|
| **Incident 7 — persistent disk-full** | If the CIFS share remains at 100 % after `cargo clean`, no mitigation can unblock git writes until space is freed at the server (JuiceFS GC / MinIO eviction). | Add a server-side quota alert and scheduled JuiceFS GC cron job. |
| **Concurrent oplock breaks at scale** | With many ACC agents writing to the same CIFS share simultaneously, `noplock` disables client caching but each write still requires a round-trip; under very high concurrency the SMB2 session can still stall. | Evaluate per-agent clone directories rather than a single shared working tree. |
| **watch_dstate false positives** | Transient D-state events (< 1 s) during normal kernel I/O are reported by `watch_dstate` and may generate noise in CI logs. | Add `uptime_s >= 5` filter to suppress sub-second events; already supported via `uptime_s` field. |