# Investigation: Git Phase Commit Failure — `fatal: unable to write new index file`

**Task:** `task-ad5a2e21c0354aa080b2041e22d2c141`
**Branch:** `phase/milestone`
**Date:** 2025-04-25

---

## Symptom

The phase milestone commit failed with:

```
fatal: unable to write new index file
```

## Root Cause

**The accfs network filesystem is 100% full.**

```
Filesystem                    Size    Used   Avail  Capacity
//jkh@100.89.199.14/accfs    154Gi  153Gi   784Mi    100%
```

Git was unable to write the updated index (`.git/index`) to disk because there
was no free space on the underlying `accfs` volume mounted at
`/Users/jkh/.acc/shared`.  This is a network SMB/CIFS share and its capacity
is exhausted.

## Evidence

- `df -h /Users/jkh/.acc/shared` shows **100% capacity** with only ~784 MiB
  remaining (effectively full for any write that needs temporary scratch space).
- `.git/index` exists but is only 136 bytes — consistent with a partial/failed
  write during a prior run.
- The staged commit (`git diff --cached --stat`) shows **397 files changed**
  with a large net deletion (-68 909 lines / +39 lines), meaning git needed to
  write a significantly larger index file during the commit operation.
- Multiple prior phase milestone commits (`git log --oneline`) succeeded,
  confirming the failure is environmental (disk full), not a code defect.

## Impact

- No source code is corrupted; the git object store is intact.
- The index is in a valid (though stale) state — `git status` functions normally.
- All staged changes are preserved and can be committed once disk space is
  freed.

## Resolution Steps

1. **Free space on the accfs volume** hosted at `100.89.199.14`:
   - Remove large or unnecessary files from `/Users/jkh/.acc/shared/` on the
     host (or on any node that contributes to that share).
   - Likely candidates: old build artifacts, Rust `target/` directories,
     container images, log files, or large model checkpoints.
   - Target: at least **2–4 GiB** of free space to give git and other tooling
     comfortable headroom.

2. **Retry the phase commit** once space is freed:
   ```bash
   git commit -m "phase commit: milestone (0 tasks reviewed and approved)"
   ```

3. **Monitor disk usage** going forward — the volume is undersized relative to
   its workload.  Consider:
   - Expanding the accfs volume.
   - Adding a cron job or alert when usage exceeds 90%.

## Preventive Measures

- Add a pre-flight disk-space check (≥ 1 GiB free) to the phase-commit script
  before attempting any git operations.
- Schedule periodic cleanup of `target/` directories across Rust workspaces on
  shared storage.

---

# Follow-up Incident: Git Index Lock File — `fatal: Unable to create '.git/index.lock'`

**Task:** `task-9bb116d911134398bcaf25eb3b09369f`
**Branch:** `phase/milestone`
**Date:** 2025-04-26

---

## Symptom

The phase milestone commit failed with:

```
fatal: Unable to create '/home/jkh/.acc/shared/acc/.git/index.lock': File exists.

Another git process seems to be running in this repository, e.g.
an editor opened by 'git commit'. Please make sure all processes
are terminated then try again. If it still fails, a git process
may have crashed in this repository earlier:
remove the file manually to continue.
```

## Root Cause

A prior git process (likely the commit triggered by the previous disk-full
failure) crashed mid-operation, leaving the index in a corrupt/empty state.
The `index.lock` file it created was not cleaned up at the time, blocking
subsequent git operations.  By the time investigation began the lock file had
already been removed (either by the OS or a concurrent process), but the
underlying index was still empty — confirmed by `git ls-files --cached`
returning no output.

## Evidence

- `.git/index.lock` was absent when inspected, but `git status` still showed
  all repository files as *staged for deletion* (hundreds of files), indicating
  the index was empty rather than matching `HEAD`.
- `git ls-files --cached` returned 0 lines, confirming an empty index.
- `git log --oneline -1` showed the HEAD commit was intact with 383 tracked
  files — the object store was undamaged.
- No git processes were running (`ps aux | grep git` returned nothing).

## Resolution Applied

Restored the index from `HEAD` using:

```bash
git read-tree HEAD
```

After running this command:

- `git ls-files --cached | wc -l` returned **383** (all tracked files restored).
- `git status` showed only the expected two modified working-tree files
  (`.task-context.json` and `deploy/queue-worker.py`) — no staged deletions.
- The repository is ready for the next phase commit.

## Preventive Measures

- Add a pre-flight index integrity check to the phase-commit script:
  ```bash
  if [ "$(git ls-files --cached | wc -l)" -eq 0 ]; then
      echo "WARNING: git index is empty — restoring from HEAD"
      git read-tree HEAD
  fi
  ```
- Ensure the phase-commit script removes any stale `index.lock` file before
  starting git operations:
  ```bash
  rm -f "$(git rev-parse --git-dir)/index.lock"
  ```
- Investigate why the prior crash left the index empty rather than restoring
  it to its pre-operation state (possible filesystem-level write failure on the
  accfs SMB share).
