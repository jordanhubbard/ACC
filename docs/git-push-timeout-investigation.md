# Git Push Failure Investigation — phase/milestone

---

## Incident 2 — DNS Resolution Failure (2026-04-25)

**Affected task:** `task-f7fa50cf1765488a8cc32bdf86772e9b`  
**Symptom:** Phase milestone commit failed with:

```
ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known
fatal: Could not read from remote repository.
```

### Root Cause

The agent host lost DNS resolution for `github.com` at the moment the milestone
commit tried to push.  This is a **transient network partition** (DNS resolver
unavailable, upstream resolver timeout, or `/etc/resolv.conf` misconfiguration)
rather than a persistent connectivity loss — the host can reach the internet but
cannot resolve hostnames momentarily.

Key evidence:
- Error message `nodename nor servname provided, or not known` is the POSIX
  `getaddrinfo()` return for `EAI_NONAME` / `EAI_AGAIN`, which indicates the DNS
  resolver returned an error or was unreachable, not that GitHub itself was down.
- The `phase/milestone` branch has accumulated many successful commits
  (`d54824c`, `bccacc7`, `bf957d4`, …) — confirming this is not a credential or
  permission issue.
- Local branch `phase/milestone` is at `d54824c0` which equals
  `refs/remotes/origin/phase/milestone` — meaning the most recent milestone
  commit was eventually pushed (or the remote tracking ref was updated), so the
  content is not lost.

### Fix Applied

1. **Pre-flight DNS check in `acc-repo-sync.sh`:** Added a `dns_preflight` step
   that resolves `github.com` (via `getent hosts` or `dig`) before attempting a
   fetch/push.  If DNS fails, the script logs a warning and exits with code `0`
   (non-fatal) so the systemd timer retries on the next cycle instead of
   accumulating error noise.

2. **Retry loop around the push step:** `acc-repo-sync.sh` now retries the push
   up to 3 times with a 10-second back-off, so a single-cycle DNS blip does not
   cause a permanent miss.

3. **`.git/config` tracking entry for `phase/milestone`:** Added an explicit
   `[branch "phase/milestone"]` section so `git push` can resolve the upstream
   without a fetch first (reduces the number of DNS calls needed per sync cycle).

---

## Incident 1 — SSH Timeout (2026-04-25)

**Affected task:** `task-770121149e5f414ea8940fec81d6c7bb`  
**Symptom:** `git push` timed out during phase milestone commit on branch `phase/milestone`

---

## Root Cause

The repository remote uses SSH (`git@github.com:/jordanhubbard/ACC.git`) with **no
timeout configuration** in `.git/config` or the global git config. When the SSH
handshake or data transfer stalls (transient GitHub connectivity blip, NAT
keepalive expiry, or momentary DNS hiccup), git waits indefinitely and the
milestone-commit process is eventually killed by the outer process watchdog.

Key evidence:
- `git config --list` shows no `http.lowSpeedLimit`, `http.lowSpeedTime`, or
  `core.sshCommand` entries — meaning git uses system defaults with no explicit
  timeout guard.
- The remote URL uses the `git@github.com:/…` SSH form (note the leading `/` after
  the colon, which is redundant but harmless on most SSH clients). The slash can
  cause a DNS-level extra lookup on some OpenSSH versions before it resolves to
  the correct GitHub endpoint; under degraded connectivity this adds latency.
- Multiple prior phase milestone commits (`55b701f`, `ae2f2bd`, `0c34006`, …) all
  show `0 tasks reviewed and approved`, meaning the branch accumulates commits
  normally — this is not a persistent failure, confirming the cause is transient.

---

## Fix Applied

Added SSH timeout configuration to `.git/config` (local repo scope) so future
pushes will fail fast rather than hang:

```
[core]
    sshCommand = ssh -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
```

This gives:
- **30 s** to complete the initial TCP/SSH handshake  
- **15 s** keepalive probes while the connection is open, with **3** retries before
  the client considers the server dead — meaning a stalled push will be aborted
  within ~45 s instead of hanging forever.

The remote URL also corrected to the canonical form (`github.com:jordanhubbard/ACC.git`
without the leading `/`) to eliminate the redundant path component.

---

## Recommended Follow-Up

1. **Retry push:** The milestone-commit cron will pick up the `phase/milestone`
   branch on its next cycle. With the timeout guard in place the retry should
   succeed or fail quickly.

2. **Global git config (optional):** Apply the same `sshCommand` setting to
   `~/.gitconfig` on each agent host so all repos benefit, not just this one.

3. **Monitor:** If timeouts recur more than once per week, investigate whether
   the agent host is behind an aggressive NAT or firewall that drops idle TCP
   connections before the SSH handshake completes; increasing `ServerAliveInterval`
   to `10` or switching to HTTPS with a token would be the next step.

4. **DNS health:** If DNS resolution failures recur, check `/etc/resolv.conf` and
   consider adding `8.8.8.8` as a fallback resolver, or switching the agent host
   to use systemd-resolved with a local cache so transient upstream failures do
   not prevent hostname lookups.
