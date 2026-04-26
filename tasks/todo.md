# Task List: Auto-Dispatch, Idle Discovery, Idea Voting

See tasks/plan.md for full acceptance criteria and dependency graph.

---

## Phase 0 — Infrastructure Remediation

> These tasks address active reliability blockers and must be completed before
> or alongside Phase 1.  They are independent of the dispatch/voting work.

### INFRA-1 — Reclaim disk space on rocky / AccFS / MinIO (BLOCKER)

**Context:**
The CIFS share (`//100.89.199.14/accfs → /home/jkh/.acc/shared`) was observed
at **100 % capacity** (667 MiB free of 154 GiB total) during the Incident 7
D-state hang investigation (`docs/git-index-write-failure-investigation.md`).
A near-full filesystem is the **primary driver** of D-state git hangs; git
index writes stall in kernel page-cache writeback when the CIFS server returns
`STATUS_DISK_FULL`, causing processes to enter uninterruptible sleep.  No
remediation ticket was filed after the investigation concluded, meaning the
underlying condition will recur.

**Acceptance criteria:**
- [ ] AccFS/MinIO volume on rocky has **≥ 10 GiB free** after cleanup.
- [ ] JuiceFS garbage collection has been run to reclaim unreferenced chunks.
- [ ] Large stale artifacts (build outputs, temp files, old logs) have been
      identified and removed from the AccFS bucket.
- [ ] `df -h /mnt/accfs` on rocky confirms ≥ 10 GiB free.
- [ ] A cron job (or systemd timer) on rocky is in place to **alert when free
      space drops below 5 GiB** (see monitoring task INFRA-2 below).

**Remediation steps (run on rocky as the designated AccFS node):**

```bash
# 1. Assess current usage
df -h /mnt/accfs
mc du --recursive local/accfs/ | sort -rh | head -20

# 2. Run JuiceFS GC to reclaim unreferenced / stale chunks
juicefs gc redis://127.0.0.1:6379/1 --delete

# 3. Remove stale temporary files left by aborted git / JuiceFS operations
mc rm --recursive --force local/accfs/accfs/tmp/ 2>/dev/null || true
mc rm --recursive --force local/accfs/accfs/.trash/ 2>/dev/null || true

# 4. Identify and remove large regenerable artifacts (Rust target/, node caches)
mc find local/accfs/accfs/ --name "*.rlib"  | xargs -r mc rm
mc find local/accfs/accfs/ --name "*.rmeta" | xargs -r mc rm
mc find local/accfs/accfs/ --name "CACHEDIR.TAG" | xargs -r mc rm

# 5. Re-check free space; repeat gc if still < 10 GiB
df -h /mnt/accfs
juicefs status redis://127.0.0.1:6379/1   # shows used / available bytes
```

**References:**
- Root-cause analysis: `docs/git-index-write-failure-investigation.md` § Incident 7
- JuiceFS GC docs: <https://juicefs.com/docs/community/administration/status_check_and_maintenance>

---

### INFRA-2 — Ongoing AccFS disk-space monitoring (follow-up to INFRA-1)

**Context:**
Without continuous monitoring, the disk-full condition will silently recur.
A lightweight cron job on rocky should alert (log + optional notification) when
free space on the AccFS FUSE mount drops below the safe threshold.

**Acceptance criteria:**
- [ ] A cron entry (or systemd timer + service) on rocky runs the space check
      at least every 15 minutes.
- [ ] When free space < 5 GiB the check emits a `WARN` line to syslog /
      a log file and (optionally) publishes a `rocky:alert` message on the
      agentbus.
- [ ] When free space < 2 GiB the check emits a `CRIT` line and exits non-zero
      so that any monitoring harness can page on it.
- [ ] The check script is committed to `scripts/` and referenced in
      `deploy/crontab-acc.txt` (or equivalent systemd unit).

**Suggested cron entry (add to rocky's crontab):**

```cron
*/15 * * * *  root  bash /home/jkh/.acc/shared/acc/scripts/cifs-mount-health.sh \
                         --mount-path /home/jkh/.acc/shared \
                         >> /var/log/accfs-health.log 2>&1
```

**References:**
- Health-check script: `scripts/cifs-mount-health.sh`
- Investigation: `docs/git-index-write-failure-investigation.md` § Preventive Measures

---

## Phase 1 — Dispatch Foundation
- [ ] T1: dispatch.rs skeleton + tokio::spawn in main.rs
- [ ] T2: capability matcher pure functions + unit tests
- [ ] T3: directed nudge on task create (routes/tasks.rs)
- [ ] T4: agent wakes on bus nudge (agent/acc-agent/src/tasks.rs)
- [ ] T5: tick loop — broadcast nudge, explicit assign, backfill

**Checkpoint 1:** basic dispatch end-to-end

## Phase 2 — Idea Lifecycle
- [ ] T6: PUT /api/tasks/:id/vote endpoint
- [ ] T7: vote nudges in dispatch loop
- [ ] T8: idea tally, promotion, rejection
- [ ] T9: Rocky pre-expiry warning
- [ ] T10: Rocky response handler + expiry timeout

**Checkpoint 2:** idea lifecycle end-to-end

## Phase 3 — Idle Discovery
- [ ] T11: idle agent detection
- [ ] T12: discovery task auto-creation

**Checkpoint 3:** full system end-to-end
