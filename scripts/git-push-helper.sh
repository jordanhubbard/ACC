#!/usr/bin/env bash
# git-push-helper.sh — robust git-push with DNS pre-flight and exponential back-off
#
# Provides five functions:
#   resolve_git_remote_host  — pure-bash URL/SCP parser; extracts the hostname from
#                              any remote format git supports (https/http/git/ssh/SCP).
#   dns_preflight            — retry loop that confirms the remote host is resolvable
#                              before attempting a push.
#   is_transient_network_error — classifies git-push stderr output; returns 0 (true)
#                              when the failure looks like a transient network hiccup
#                              that is worth retrying, or 1 (false) when the error is
#                              permanent (auth failure, non-fast-forward, etc.).
#   remove_stale_git_locks   — removes stale .git lock files (index.lock, HEAD.lock,
#                              packed-refs.lock) left by crashed git processes or CIFS
#                              D-state hangs (Mitigation I — Incident 9).
#   push_with_retry          — orchestrator that runs remove_stale_git_locks and
#                              dns_preflight then retries `git push` with exponential
#                              back-off, aborting early when is_transient_network_error
#                              classifies the failure as permanent.
#
# Environment variables (all optional, with defaults shown):
#   GIT_PUSH_RETRIES   — maximum push attempts            (default: 5)
#   GIT_PUSH_DNS_TRIES — maximum DNS lookup attempts      (default: 3)
#   GIT_PUSH_DNS_DELAY — seconds between DNS retries      (default: 2)
#
# Usage (source this file, then call push_with_retry):
#   source scripts/git-push-helper.sh
#   push_with_retry [remote] [refspec...]
#
# Or run directly:
#   bash scripts/git-push-helper.sh [remote] [refspec...]

set -euo pipefail

# ---------------------------------------------------------------------------
# resolve_git_remote_host
# ---------------------------------------------------------------------------
# Pure-bash parser that extracts the hostname from the URL associated with a
# git remote (or from a literal URL string passed directly).
#
# Supported formats:
#   https://user:pass@host.example.com/org/repo.git
#   http://host.example.com/org/repo.git
#   git://host.example.com/org/repo.git
#   ssh://user@host.example.com/org/repo.git
#   git@host.example.com:org/repo.git          (SCP-style)
#   user@host.example.com:path/to/repo.git     (generic SCP-style)
#
# Arguments:
#   $1 — git remote name (default: "origin") OR a literal remote URL
#
# Outputs:
#   Prints the bare hostname to stdout.
#   Returns 1 if the host cannot be determined.
#
# Example:
#   host=$(resolve_git_remote_host origin)
#   host=$(resolve_git_remote_host "git@github.com:org/repo.git")
resolve_git_remote_host() {
    local remote_or_url="${1:-origin}"
    local url

    # If the argument looks like a URL or SCP address, use it directly;
    # otherwise treat it as a remote name and look up its fetch URL.
    # Recognised as a literal URL/address when it contains "://",
    # contains "@…:" (SCP with user), or matches "word:path" (SCP without user,
    # i.e. the colon is not the first character and is followed by a non-slash).
    if [[ "$remote_or_url" == *"://"* \
       || "$remote_or_url" == *"@"*":"* \
       || "$remote_or_url" =~ ^[^/:]+:[^/] ]]; then
        url="$remote_or_url"
    else
        # Look up the remote; fail gracefully if git isn't available or the
        # remote doesn't exist.
        if ! url=$(git remote get-url "$remote_or_url" 2>/dev/null); then
            echo "[git-push-helper] ERROR: cannot get URL for remote '$remote_or_url'" >&2
            return 1
        fi
    fi

    local host=""

    case "$url" in
        # ----------------------------------------------------------------
        # Standard URL schemes:  scheme://[user[:pass]@]host[:port]/path
        # ----------------------------------------------------------------
        https://* | http://* | git://* | ssh://*)
            # Strip the scheme (everything up to and including "://")
            local after_scheme="${url#*://}"
            # Strip optional userinfo (user[:pass]@)
            local after_userinfo="${after_scheme#*@}"
            # Strip optional port and everything after the first "/"
            host="${after_userinfo%%/*}"   # drop path
            host="${host%%:*}"             # drop :port
            ;;

        # ----------------------------------------------------------------
        # SCP-style:  [user@]host:path
        # ----------------------------------------------------------------
        *@*:*)
            # Strip leading user@ part
            local after_at="${url#*@}"
            # The host is everything before the first ":"
            host="${after_at%%:*}"
            ;;

        *:*)
            # SCP-style without a user prefix:  host:path
            host="${url%%:*}"
            ;;

        *)
            echo "[git-push-helper] ERROR: unrecognised remote URL format: '$url'" >&2
            return 1
            ;;
    esac

    if [[ -z "$host" ]]; then
        echo "[git-push-helper] ERROR: could not extract host from URL: '$url'" >&2
        return 1
    fi

    printf '%s\n' "$host"
}

# ---------------------------------------------------------------------------
# dns_preflight
# ---------------------------------------------------------------------------
# Confirms that $host is resolvable via DNS before a push is attempted.
# Tries getent(1) first (fast, POSIX, available on Linux/macOS with libc);
# falls back to nslookup(1) if getent is absent or fails.
#
# Controlled by:
#   GIT_PUSH_DNS_TRIES — how many attempts before giving up (default: 3)
#   GIT_PUSH_DNS_DELAY — seconds to wait between attempts  (default: 2)
#
# Arguments:
#   $1 — hostname to resolve
#
# Returns:
#   0 if the host resolves within the allowed attempts.
#   1 if all attempts are exhausted without a successful lookup.
dns_preflight() {
    local host="${1:?dns_preflight: hostname argument is required}"
    local max_tries="${GIT_PUSH_DNS_TRIES:-3}"
    local delay="${GIT_PUSH_DNS_DELAY:-2}"
    local attempt

    echo "[git-push-helper] DNS pre-flight for host: $host" \
         "(tries=${max_tries}, delay=${delay}s)" >&2

    for (( attempt = 1; attempt <= max_tries; attempt++ )); do
        echo "[git-push-helper] DNS attempt ${attempt}/${max_tries} …" >&2

        # Prefer getent (fastest, no external process on glibc systems)
        if command -v getent >/dev/null 2>&1; then
            if getent hosts "$host" >/dev/null 2>&1; then
                echo "[git-push-helper] DNS resolved '$host' via getent." >&2
                return 0
            fi
        fi

        # Fall back to nslookup
        if command -v nslookup >/dev/null 2>&1; then
            if nslookup "$host" >/dev/null 2>&1; then
                echo "[git-push-helper] DNS resolved '$host' via nslookup." >&2
                return 0
            fi
        fi

        # Neither tool succeeded on this attempt
        if (( attempt < max_tries )); then
            echo "[git-push-helper] DNS lookup failed; retrying in ${delay}s …" >&2
            sleep "$delay"
        fi
    done

    echo "[git-push-helper] ERROR: DNS pre-flight failed for '$host'" \
         "after ${max_tries} attempt(s)." >&2
    return 1
}

# ---------------------------------------------------------------------------
# is_transient_network_error
# ---------------------------------------------------------------------------
# Classifies git-push stderr output and decides whether the failure is a
# transient network hiccup (safe to retry) or a permanent error (must not
# retry — retrying wastes time and may trigger rate-limiting).
#
# This is the bash equivalent of the Rust fn is_transient_network_error() in
# agent/acc-agent/src/tasks.rs.  Both must be kept in sync: any pattern added
# to one should be added to the other.
#
# Permanent / hard-fail patterns (return 1 — do NOT retry):
#   • authentication failed / could not read username / invalid username or password
#   • permission denied
#   • rejected  (covers "! [rejected]", "[remote rejected]")
#   • non-fast-forward
#
# Transient / retriable patterns (return 0 — retry is safe):
#   • could not resolve host
#   • unable to connect
#   • connection timed out / connection refused
#   • network is unreachable
#   • the remote end hung up
#   • curl error
#   • ssh: connect to host
#   • broken pipe
#   • temporary failure
#   • service unavailable / 503
#
# Arguments:
#   $1 — stderr output from `git push` (may be multi-line)
#
# Returns:
#   0 (true)  — transient error; the caller should retry
#   1 (false) — permanent error; the caller should abort
#
# Example:
#   stderr=$(git push origin main 2>&1 >/dev/null) || true
#   if is_transient_network_error "$stderr"; then
#       echo "transient — will retry"
#   else
#       echo "permanent — aborting"
#   fi
is_transient_network_error() {
    local stderr="${1:-}"
    # Work in lower-case for case-insensitive matching.
    local lower
    lower=$(printf '%s' "$stderr" | tr '[:upper:]' '[:lower:]')

    # ---- Permanent errors — hard-fail immediately (do NOT retry) -------------
    if printf '%s' "$lower" | grep -qE \
        'authentication failed|could not read username|invalid username or password|permission denied|rejected|non-fast-forward|\[remote rejected\]'; then
        return 1
    fi

    # ---- Transient network / infrastructure signals (safe to retry) ----------
    if printf '%s' "$lower" | grep -qE \
        'could not resolve host|unable to connect|connection timed out|connection refused|network is unreachable|the remote end hung up|curl error|ssh: connect to host|broken pipe|temporary failure|service unavailable|503'; then
        return 0
    fi

    # Unknown error — treat as permanent to avoid infinite loops.
    return 1
}

# ---------------------------------------------------------------------------
# remove_stale_git_locks
# ---------------------------------------------------------------------------
# Removes stale git lock files (index.lock, HEAD.lock, packed-refs.lock) that
# are left behind by two distinct failure scenarios:
#
#   a) Crashed git process (SIGKILL / OOM kill) — the process died before it
#      could write any content, leaving a zero-byte lock.  Safe to remove
#      unconditionally; no live process holds the file.
#      → Documented in: docs/git-push-timeout-investigation.md (Incident 9)
#
#   b) CIFS D-state hang — the kernel accepted the open() for the lock file
#      but the subsequent write stalled (typically on a near-full CIFS share).
#      The git process may still be alive in D-state.  Only removed after
#      confirming no live git process is detected for this repo.
#      → Documented in: docs/git-index-write-failure-investigation.md
#
# Both scenarios surface as:
#   "fatal: Unable to create '.git/index.lock': File exists.
#    Another git process seems to be running in this repository."
#
# This function is a reusable utility implementing Mitigation I from the
# Incident 9 runbook (docs/git-push-timeout-investigation.md).  The canonical
# inline version lives in scripts/phase-commit.sh (Pre-flight 2/3); this
# named function makes the same logic available to any caller that sources
# git-push-helper.sh — particularly push_with_retry, which calls it
# automatically before the first push attempt.
#
# Arguments:
#   $1 — path to the git-dir (default: auto-detected via `git rev-parse --git-dir`)
#
# Returns:
#   0 if all lock files were absent or successfully removed.
#   1 if a non-zero lock file is found and a live git process is still active
#     (the caller should wait or resolve the live process before retrying).
#
# Example:
#   remove_stale_git_locks             # auto-detect .git dir
#   remove_stale_git_locks .git        # explicit path
#   remove_stale_git_locks /repo/.git  # absolute path
remove_stale_git_locks() {
    local git_dir="${1:-}"

    # Auto-detect the git directory if not supplied
    if [[ -z "$git_dir" ]]; then
        if ! git_dir=$(git rev-parse --git-dir 2>/dev/null); then
            echo "[git-push-helper] remove_stale_git_locks: not inside a git repo; skipping." >&2
            return 0
        fi
    fi

    # Resolve to an absolute path so pgrep comparisons are reliable
    git_dir=$(cd "$git_dir" 2>/dev/null && pwd -P) || {
        echo "[git-push-helper] remove_stale_git_locks: cannot resolve git-dir '${1:-auto}'; skipping." >&2
        return 0
    }

    local git_root
    git_root=$(dirname "$git_dir")

    local lockfiles=(
        "${git_dir}/index.lock"
        "${git_dir}/HEAD.lock"
        "${git_dir}/packed-refs.lock"
    )

    local found_any=false

    for lockfile in "${lockfiles[@]}"; do
        [[ -f "$lockfile" ]] || continue
        found_any=true

        local lock_size
        lock_size=$(stat --format="%s" "$lockfile" 2>/dev/null || echo "?")

        if [[ "$lock_size" == "0" ]]; then
            # Zero-byte lock is unconditionally stale: the owning process was
            # killed before writing any content (crashed-process scenario,
            # Incident 9) or the write never completed (CIFS D-state scenario).
            echo "[git-push-helper] Removing zero-byte stale lock" \
                 "(crashed git process or CIFS stall): ${lockfile}" >&2
            rm -f "$lockfile"
        else
            # Non-zero lock: only remove if no live git process is active in
            # this repository — a live process owns the lock legitimately.
            local live_git
            live_git=$(pgrep -ax git 2>/dev/null \
                | grep -v "$$" \
                | grep "$git_root" || true)

            if [[ -z "$live_git" ]]; then
                echo "[git-push-helper] Removing stale lock" \
                     "(non-zero, no active git process): ${lockfile}" >&2
                rm -f "$lockfile"
            else
                echo "[git-push-helper] ERROR: lock file exists and another git" \
                     "process appears active: ${lockfile}" >&2
                echo "[git-push-helper]   Active git processes: ${live_git}" >&2
                echo "[git-push-helper]   → Wait for them to finish, or kill" \
                     "if stuck in D-state." >&2
                echo "[git-push-helper]   → See: docs/git-push-timeout-investigation.md" \
                     "(Incident 9) and docs/git-index-write-failure-investigation.md" >&2
                return 1
            fi
        fi
    done

    if [[ "$found_any" == "false" ]]; then
        echo "[git-push-helper] remove_stale_git_locks: no lock files present ✓" >&2
    else
        echo "[git-push-helper] remove_stale_git_locks: lock files cleared ✓" >&2
    fi

    return 0
}

# ---------------------------------------------------------------------------
# push_with_retry
# ---------------------------------------------------------------------------
# Orchestrates a reliable `git push` by:
#   1. Running remove_stale_git_locks to clear any stale index/HEAD/packed-refs
#      lock files left by prior crashes or CIFS D-state hangs (Mitigation I).
#   2. Resolving the remote's hostname via resolve_git_remote_host.
#   3. Running dns_preflight to confirm connectivity before each push attempt.
#   4. Executing `git push` and capturing stderr so is_transient_network_error
#      can classify the failure: permanent errors (auth, rejection, non-fast-
#      forward) abort immediately; transient errors (connection timeout, DNS
#      failure, broken pipe, …) are retried with exponential back-off.
#
# Controlled by:
#   GIT_PUSH_RETRIES   — maximum push attempts (default: 5)
#   GIT_PUSH_DNS_TRIES — forwarded to dns_preflight (default: 3)
#   GIT_PUSH_DNS_DELAY — forwarded to dns_preflight (default: 2)
#
# Arguments:
#   $1        — remote name (default: "origin")
#   $2 …      — additional arguments forwarded verbatim to `git push`
#               (refspecs, flags, etc.)
#
# Returns:
#   0 on a successful push.
#   1 if all retry attempts fail (or if DNS pre-flight cannot be satisfied).
#
# Example:
#   push_with_retry origin main
#   push_with_retry origin --tags
#   GIT_PUSH_RETRIES=3 push_with_retry origin HEAD:main
push_with_retry() {
    local remote="${1:-origin}"
    shift || true   # remaining args (if any) are extra git-push arguments

    local max_retries="${GIT_PUSH_RETRIES:-5}"
    local attempt
    local backoff=1   # initial back-off in seconds (doubles after each failure)

    # ---- Mitigation I: clear stale lock files before any git operation -------
    # Removes zero-byte or ownerless index.lock / HEAD.lock / packed-refs.lock
    # left by crashed processes or CIFS D-state hangs (Incident 9).
    # Fails fast if a live git process still holds the lock.
    if ! remove_stale_git_locks; then
        echo "[git-push-helper] ERROR: stale lock check failed; aborting push." >&2
        return 1
    fi

    # ---- Resolve hostname once (the remote URL doesn't change per attempt) ----
    local host
    if ! host=$(resolve_git_remote_host "$remote"); then
        echo "[git-push-helper] ERROR: cannot determine remote host; aborting." >&2
        return 1
    fi
    echo "[git-push-helper] Remote '$remote' → host '$host'" >&2

    # ---- Retry loop -----------------------------------------------------------
    for (( attempt = 1; attempt <= max_retries; attempt++ )); do
        echo "[git-push-helper] Push attempt ${attempt}/${max_retries} …" >&2

        # DNS pre-flight before every attempt
        if ! dns_preflight "$host"; then
            echo "[git-push-helper] DNS pre-flight failed on attempt ${attempt};" \
                 "aborting push." >&2
            return 1
        fi

        # Attempt the push — capture stderr so is_transient_network_error can
        # inspect it while still letting stdout flow through normally.
        local push_stderr
        if push_stderr=$(git push "$remote" "$@" 2>&1); then
            echo "[git-push-helper] Push succeeded on attempt ${attempt}." >&2
            return 0
        fi

        local exit_code=$?
        echo "[git-push-helper] git push exited with code ${exit_code}" \
             "on attempt ${attempt}/${max_retries}." >&2
        # Print captured stderr so the caller/log can see it.
        [[ -n "$push_stderr" ]] && printf '%s\n' "$push_stderr" >&2

        # Check whether the failure is transient (worth retrying) or permanent.
        if ! is_transient_network_error "$push_stderr"; then
            echo "[git-push-helper] ERROR: git push failed with a permanent error" \
                 "(auth, non-fast-forward, or rejection) — aborting without retry." >&2
            return 1
        fi

        if (( attempt < max_retries )); then
            echo "[git-push-helper] Transient network error — waiting ${backoff}s before next attempt …" >&2
            sleep "$backoff"
            # Exponential back-off: 1s, 2s, 4s, 8s, …
            backoff=$(( backoff * 2 ))
        fi
    done

    echo "[git-push-helper] ERROR: git push to '$remote' failed after" \
         "${max_retries} attempt(s)." >&2
    return 1
}

# ---------------------------------------------------------------------------
# Main — allow the script to be executed directly as well as sourced.
# ---------------------------------------------------------------------------
# When run directly (not sourced), treat the first argument as the remote
# name and forward any remaining arguments to push_with_retry.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    push_with_retry "${@:-origin}"
fi
