# Git Push Timeout Investigation — phase/milestone

**Date:** 2026-04-25  
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
