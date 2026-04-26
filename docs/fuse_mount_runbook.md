# FUSE Mount Operations Runbook

**Scope:** JuiceFS FUSE mount on `rocky` (`/mnt/accfs`) and any NFS export
used as a git working directory in the ACC fleet.

**Related docs:**
- `docs/accfs.md` — AccFS architecture and mc-based remote access
- `docs/git-index-write-failure-investigation.md` — CIFS D-state hang root-cause analysis
- `docs/git-push-timeout-investigation.md` — git push timeout incident log

---

## 1. Background — Why FUSE Mounts Cause D-State Hangs

FUSE (Filesystem in Userspace) interposes every VFS call between the kernel
and the user-space daemon (here: the `juicefs` process).  When that daemon
stalls — because the backing store (MinIO/Redis) is slow, near-full, or
unreachable — the kernel cannot complete the I/O and keeps the calling
process in **uninterruptible sleep (D-state)**.  The same pathology occurs on
NFS mounts with `hard` semantics.

### 1.1 The root cause in this fleet

AccFS (`/mnt/accfs`) is a JuiceFS volume backed by:

| Layer | Service | Host |
|-------|---------|------|
| Metadata | Redis DB 1 | `127.0.0.1:6379` on rocky |
| Object data | MinIO bucket `accfs` | `127.0.0.1:9000` on rocky |
| FUSE mount | `juicefs mount` | `/mnt/accfs` on rocky only |

Git is run **inside** `/mnt/accfs/repos/CCC` (symlinked as
`~/.ccc/workspace`).  Every `git` index write (`open`, `write`, `fsync`,
`rename`) crosses the FUSE layer; when the MinIO bucket is near-full or the
JuiceFS daemon is unhealthy, those calls stall indefinitely.

Three concrete triggers have been observed:

| Trigger | Mechanism | Observable signal |
|---------|-----------|-------------------|
| **MinIO / accfs volume full** | `juicefs` returns `ENOSPC` from object PUT; FUSE blocks the `write` call | `df /mnt/accfs` shows ≥ 99 % used; `juicefs status` reports 0 B free |
| **JuiceFS daemon crash / restart** | FUSE file descriptor goes stale; kernel waits for daemon to re-register | `/proc/<pid>/wchan` shows `fuse_wait_answer`; `ps aux` shows `juicefs` absent |
| **Redis metadata latency spike** | JuiceFS metadata operations block in Redis round-trips under high load | `/proc/<pid>/wchan` shows `fuse_wait_answer`; Redis `SLOWLOG` shows commands > 100 ms |

### 1.2 NFS hard-mount analogue

An NFS mount with `hard` semantics (the kernel default) behaves identically:
if the NFS server becomes unreachable or saturated, all I/O to the mount
blocks until the server recovers.  The process state is D; `kill -9` has no
effect.  Switching to `soft,timeo=N,retrans=M` bounds the hang duration at
the cost of potentially returning `EIO` to the application.

---

## 2. Diagnostic Signals

Run these checks in order whenever git hangs or leaves a stale
`.git/index.lock`.

### 2.1 Quick triage

```bash
# Is the FUSE mount alive?
timeout 5 stat /mnt/accfs > /dev/null 2>&1 \
  && echo "FUSE: OK" || echo "FUSE: STALLED or ABSENT"

# Is the JuiceFS daemon running?
pgrep -ax juicefs || echo "juicefs daemon not running"

# Is Redis alive?
redis-cli ping 2>/dev/null || echo "Redis not responding"

# How full is the volume?
df -h /mnt/accfs 2>/dev/null || echo "(df timed out — mount stalled)"

# Any process stuck in D-state?
ps aux | awk '$8 == "D" {print}'
```

### 2.2 D-state process inspection

```bash
# Identify which kernel function the D-state process is blocked in
# (replace <PID> with the stuck process's PID)
cat /proc/<PID>/wchan

# Common wchan values and their meaning:
#   fuse_wait_answer     — waiting for FUSE daemon to reply to a kernel request
#   fuse_dev_do_read     — FUSE daemon blocked reading from /dev/fuse
#   nfs_wait_bit_killable — NFS hard-mount I/O wait
#   call_rwsem_down_write_failed — page-cache writeback waiting for a semaphore
```

```bash
# Full stack trace (requires root or same UID, kernel ≥ 4.9)
cat /proc/<PID>/stack 2>/dev/null
```

### 2.3 JuiceFS volume health

```bash
# Volume statistics (requires Redis access)
juicefs status redis://127.0.0.1:6379/1

# Expected output includes:
#   UsedSpace: <N> GiB  /  <total> GiB   ← watch for near-full
#   UsedInodes: <N>                       ← watch for inode exhaustion

# Check for unreferenced ("leaked") chunks that are wasting space
juicefs gc redis://127.0.0.1:6379/1

# Run garbage collection with actual deletion of leaked chunks
juicefs gc redis://127.0.0.1:6379/1 --delete
```

### 2.4 Redis health

```bash
redis-cli -n 1 ping              # should return PONG
redis-cli -n 1 GET setting       # should return JSON (JuiceFS volume metadata)
redis-cli slowlog get 10         # inspect recent slow commands
redis-cli info memory | grep used_memory_human
```

### 2.5 MinIO health

```bash
# Gateway (S3 API used by remote agents) — should return HTTP 200
curl -sf -o /dev/null -w "%{http_code}" http://127.0.0.1:9100/minio/health/live

# Direct MinIO backend
curl -sf -o /dev/null -w "%{http_code}" http://127.0.0.1:9000/minio/health/live

# Bucket usage
mc du --recursive local/accfs/ 2>/dev/null | sort -rh | head -10
```

### 2.6 Stale lock-file check

```bash
REPO=/home/jkh/.acc/shared/acc   # adjust if needed
ls -lh "${REPO}/.git/index.lock" 2>/dev/null \
  && echo "LOCK FILE PRESENT" \
  || echo "No lock file"

# Is any git process in this repo still alive?
pgrep -af "git.*${REPO}"

# Is that process in D-state?
GPID=$(pgrep -af "git.*${REPO}" | awk '{print $1}' | head -1)
[[ -n "$GPID" ]] && awk '{print $3}' /proc/"${GPID}"/stat
```

---

## 3. Recovery Procedures

### 3.1 FUSE daemon absent or crashed

```bash
# Remount AccFS
sudo systemctl restart accfs.service        # restart S3 gateway
juicefs mount redis://127.0.0.1:6379/1 /mnt/accfs -d
timeout 5 stat /mnt/accfs && echo "Mount OK"
```

### 3.2 Volume near-full — free space on rocky

```bash
# Step 1 — Run JuiceFS GC to reclaim leaked chunks
juicefs gc redis://127.0.0.1:6379/1 --delete

# Step 2 — Remove Rust build artefacts (safe; regenerated on next build)
cd /home/jkh/.acc/shared/acc && cargo clean

# Step 3 — Remove stale tmp/trash objects
mc rm --recursive --force local/accfs/accfs/tmp/   2>/dev/null || true
mc rm --recursive --force local/accfs/accfs/.trash/ 2>/dev/null || true

# Step 4 — Confirm ≥ 2 GiB free before retrying git
df -h /mnt/accfs
```

### 3.3 Remove a stale index.lock after a D-state hang

```bash
REPO=/home/jkh/.acc/shared/acc

# Only safe to remove if no live git process owns the lock
if pgrep -af "git.*${REPO}" | grep -qv grep; then
  echo "WARNING: live git process found — wait for it to exit first"
  ps aux | grep git
else
  rm -f "${REPO}/.git/index.lock"
  echo "Lock removed"
  git -C "${REPO}" status
fi
```

### 3.4 Force-unmount a stalled FUSE mount (last resort)

```bash
# Graceful attempt first
sudo umount /mnt/accfs

# If umount hangs (device busy), force-unmount
sudo umount -l /mnt/accfs    # lazy unmount — detaches mount point immediately

# Verify the mount is gone
mount | grep accfs || echo "No accfs mount active"

# Remount
juicefs mount redis://127.0.0.1:6379/1 /mnt/accfs -d
```

---

## 4. Hardened JuiceFS Mount Options

The default `juicefs mount` invocation does not set several options that are
important for reliability in a production shared-filesystem / git workload.
The recommended mount command for rocky is:

```bash
juicefs mount \
  --cache-size    0 \
  --attr-cache    1 \
  --entry-cache   1 \
  --dir-entry-cache 1 \
  --open-cache    0 \
  --writeback-cache=false \
  --no-bgjob \
  redis://127.0.0.1:6379/1 \
  /mnt/accfs \
  -d
```

### Option explanations

| Option | Value | Rationale |
|--------|-------|-----------|
| `--cache-size 0` | 0 MiB | Disables the local block cache.  With a local MinIO backend the latency benefit is negligible and a stale cache can cause spurious git index invalidations. |
| `--attr-cache 1` | 1 s | Short attribute cache reduces stat round-trips without masking changes between agents. |
| `--entry-cache 1` | 1 s | Short directory-entry cache for the same reason. |
| `--dir-entry-cache 1` | 1 s | Same as `--entry-cache` for directory lookups. |
| `--open-cache 0` | disabled | With `--open-cache` enabled, JuiceFS keeps file handles open in the daemon.  When the daemon restarts, any cached handles become invalid and the calling process enters D-state.  Disabling it makes each `open(2)` independently recoverable. |
| `--writeback-cache=false` | off | When writeback cache is enabled, `write()` returns before data reaches the object store, which can produce D-state on `close(2)` / `fsync(2)` if MinIO is slow.  Disabling it makes writes synchronous and errors surface immediately. |
| `--no-bgjob` | set | Suppresses background JuiceFS maintenance jobs (compaction, GC) that can compete with foreground I/O and consume the last available disk space unexpectedly. |

### Systemd unit override

If `/etc/systemd/system/accfs-fuse.service` (or equivalent) manages the
FUSE mount, add the options above to the `ExecStart` line:

```ini
[Unit]
Description=AccFS JuiceFS FUSE mount
After=redis-server.service minio.service
Requires=redis-server.service minio.service

[Service]
Type=forking
ExecStart=/usr/local/bin/juicefs mount \
  --cache-size 0 \
  --attr-cache 1 \
  --entry-cache 1 \
  --dir-entry-cache 1 \
  --open-cache 0 \
  --writeback-cache=false \
  --no-bgjob \
  redis://127.0.0.1:6379/1 \
  /mnt/accfs \
  -d
ExecStop=/bin/umount -l /mnt/accfs
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

---

## 5. Hardened NFS Mount Options

When a git working directory resides on an NFS export (e.g., a shared
`/mnt/nfs-repos` on a build server), use the following `/etc/fstab` entry:

```
<server>:/export/repos  /mnt/nfs-repos  nfs4 \
  rw,\
  soft,\
  timeo=30,\
  retrans=3,\
  rsize=1048576,\
  wsize=1048576,\
  noatime,\
  nodiratime,\
  lookupcache=none,\
  nofail  0 0
```

### Option explanations

| Option | Rationale |
|--------|-----------|
| `soft` | Returns `EIO` to the application after `timeo × retrans` tenths-of-a-second, rather than blocking forever.  Allows git to fail fast and report an error rather than entering D-state. |
| `timeo=30` | 3.0-second timeout per RPC attempt.  Combined with `retrans=3` this caps a dead-server hang at ≈ 9 seconds. |
| `retrans=3` | Retry each RPC up to 3 times before returning an error. |
| `rsize=1048576,wsize=1048576` | 1 MiB read/write buffer — improves throughput for git pack-file transfers. |
| `noatime,nodiratime` | Suppresses access-time updates, which would otherwise trigger NFS `SETATTR` calls on every `git status` read. |
| `lookupcache=none` | Forces the NFS client to revalidate every directory lookup from the server.  Prevents git from operating against a stale directory cache after concurrent agent writes. |
| `nofail` | Allows the system to boot (and other mounts to proceed) even if the NFS server is unreachable at boot time. |

### Testing NFS mount health

```bash
# Is the mount responding within 2 seconds?
timeout 2 stat /mnt/nfs-repos > /dev/null 2>&1 \
  && echo "NFS: OK" || echo "NFS: STALLED or ABSENT"

# Round-trip latency
time ls /mnt/nfs-repos > /dev/null

# Check for D-state processes blocked on NFS
ps aux | awk '$8 == "D" {print}' | grep -v grep
```

---

## 6. Recommended Git Configuration for FUSE / NFS Working Trees

When a git repository lives on a FUSE-backed or NFS filesystem, the following
settings in `.git/config` reduce the number and size of kernel I/O operations
that can trigger D-state hangs.

Apply them once after cloning (or add them to `~/.gitconfig` globally):

```bash
git -C /path/to/repo config core.trustctime      false
git -C /path/to/repo config core.checkStat       minimal
git -C /path/to/repo config core.preloadIndex    false
git -C /path/to/repo config index.threads        1
git -C /path/to/repo config gc.auto              0
git -C /path/to/repo config fetch.writeCommitGraph false
```

Resulting `.git/config` section:

```ini
[core]
    trustctime        = false
    checkStat         = minimal
    preloadIndex      = false
[index]
    threads           = 1
[gc]
    auto              = 0
[fetch]
    writeCommitGraph  = false
```

### Setting explanations

| Setting | Value | Rationale |
|---------|-------|-----------|
| `core.trustctime` | `false` | JuiceFS and NFS ctimes are not reliable — the server may report a different ctime than the client cached, causing git to treat every file as modified and re-stat the entire working tree on every `git status`.  Disabling ctime trust makes git compare only mtime and file size. |
| `core.checkStat` | `minimal` | Further reduces stat comparisons to mtime and size only (equivalent to `core.trustctime=false` but also skips inode number and device checks, which change across FUSE daemon restarts). |
| `core.preloadIndex` | `false` | By default git opens every tracked file for `stat(2)` in parallel background threads during index refresh.  On FUSE/NFS each parallel stat is a separate round-trip; under load this saturates the connection and widens the window during which a daemon stall can freeze many threads simultaneously. |
| `index.threads` | `1` | Forces single-threaded index I/O, eliminating the parallel stat storm entirely at a small performance cost on small working trees. |
| `gc.auto` | `0` | Disables automatic background garbage collection.  `git gc` writes large pack files; on a near-full FUSE/NFS volume this is the most reliable way to trigger a D-state hang.  Run `git gc` manually during a maintenance window when free space is confirmed adequate. |
| `fetch.writeCommitGraph` | `false` | Prevents `git fetch` from writing a commit-graph file (a large write to `.git/objects/info/commit-graph`).  On a near-full or slow mount this write can stall or fill the remaining space. |

---

## 7. Preventive Monitoring

### 7.1 Space-usage alert (cron, rocky)

Add to rocky's crontab (`crontab -e` or `/etc/cron.d/accfs-monitor`):

```cron
# Alert when AccFS free space drops below 5 GiB (runs every 15 minutes)
*/15 * * * *  root  bash /home/jkh/.acc/shared/acc/scripts/cifs-mount-health.sh \
                         --mount-path /mnt/accfs \
                         >> /var/log/accfs-health.log 2>&1
```

### 7.2 JuiceFS GC — weekly maintenance (cron, rocky)

```cron
# Reclaim leaked JuiceFS chunks every Sunday at 03:00 UTC
0 3 * * 0  root  juicefs gc redis://127.0.0.1:6379/1 --delete \
                   >> /var/log/juicefs-gc.log 2>&1
```

### 7.3 Free-space thresholds

| Threshold | Action |
|-----------|--------|
| < 10 GiB | **Warning** — plan cleanup; run `juicefs gc` |
| < 5 GiB  | **Alert** — run `cargo clean`; investigate large objects |
| < 2 GiB  | **Critical** — block all git operations; free space before retrying |
| < 512 MiB | `scripts/phase-commit.sh` pre-flight guard fires; commit aborted automatically |

### 7.4 D-state watchdog (CI / long-running automation)

Source the `watch_dstate()` function from
`docs/git-index-write-failure-investigation.md` and run it in the background
during any CI job that performs git operations on the FUSE/NFS mount:

```bash
# Start monitor before git I/O
GIT_ROOT=/home/jkh/.acc/shared/acc \
POLL_INTERVAL=10 \
watch_dstate --git-only &
_DSTATE_MONITOR_PID=$!
trap 'kill "$_DSTATE_MONITOR_PID" 2>/dev/null' EXIT

# … git add / git commit / git push …
```

Output lines with `wchan=fuse_wait_answer` identify FUSE-layer stalls;
`wchan=nfs_wait_bit_killable` identifies NFS stalls.

---

## 8. Quick-Reference Card

```
Symptom: git hangs / D-state / index.lock left behind

DIAGNOSE
1.  timeout 5 stat /mnt/accfs         → FUSE mount alive?
2.  pgrep -ax juicefs                 → daemon running?
3.  redis-cli ping                    → Redis alive?
4.  df -h /mnt/accfs                  → volume full?
5.  ps aux | awk '$8=="D"{print}'     → D-state processes?
6.  cat /proc/<PID>/wchan             → fuse_wait_answer = FUSE stall

RECOVER
A.  FUSE daemon gone   → juicefs mount redis://127.0.0.1:6379/1 /mnt/accfs -d
B.  Volume full        → juicefs gc --delete; cargo clean; verify df ≥ 2 GiB
C.  Stale lock + no git process → rm -f .git/index.lock
D.  Mount hung         → sudo umount -l /mnt/accfs; then remount (A)

HARDEN (apply once per repo)
  git config core.trustctime      false
  git config core.checkStat       minimal
  git config core.preloadIndex    false
  git config index.threads        1
  git config gc.auto              0
  git config fetch.writeCommitGraph false

MOUNT (juicefs)
  juicefs mount --cache-size 0 --writeback-cache=false --open-cache 0 \
    --no-bgjob redis://127.0.0.1:6379/1 /mnt/accfs -d

MOUNT (NFS)
  soft,timeo=30,retrans=3,rsize=1048576,wsize=1048576,lookupcache=none,nofail
```
