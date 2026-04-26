#!/usr/bin/env bats
# test_fuse_watchdog.bats — bats integration tests for scripts/fuse-watchdog
#
# Run:
#   bats scripts/tests/test_fuse_watchdog.bats
#
# Requirements:
#   - bats >= 1.5   (https://github.com/bats-core/bats-core)
#   - bash >= 4.4
#   - Standard GNU coreutils (mktemp, mkdir, cat, echo, kill, sleep)
#   - Passwordless sudo for the root-required tests (tests skip gracefully
#     when sudo is unavailable).
#
# Design notes
# ────────────────────────────────────────────────────────────────────────────
# fuse-watchdog refuses to run unless EUID == 0.  Rather than patching the
# script (which would give false coverage), every test that exercises the main
# body is run under "sudo -n bash SCRIPT …".  Tests that only exercise
# argument-parsing paths that exit before the EUID check (--help, unknown
# flags) run the script directly without sudo.
#
# A mock SYSFS_ROOT is constructed in $TEST_TMP for every test:
#
#   $TEST_TMP/
#   └── fuse/
#       └── connections/
#           ├── <id>/
#           │   ├── waiting    (integer, written by setup helpers)
#           │   └── abort      (initially "0"; watchdog writes "1" on trigger)
#           └── …
#
# The --conn-dir flag points the watchdog at $TEST_TMP/fuse/connections so the
# real /sys/fs/fuse/connections is never touched.
#
# Environment variables are injected via "sudo -n env KEY=val bash …" because
# sudo's env_reset strips the caller's environment by default.

# ─────────────────────────────────────────────────────────────────────────────
# Paths
# ─────────────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$BATS_TEST_FILENAME")/.." && pwd)"
WATCHDOG="${SCRIPT_DIR}/fuse-watchdog"

# ─────────────────────────────────────────────────────────────────────────────
# sudo availability probe (evaluated once at load time)
# ─────────────────────────────────────────────────────────────────────────────
_sudo_ok() {
    sudo -n true 2>/dev/null
}

# ─────────────────────────────────────────────────────────────────────────────
# Per-test setup / teardown
# ─────────────────────────────────────────────────────────────────────────────
setup() {
    TEST_TMP="$(mktemp -d)"
    CONN_DIR="${TEST_TMP}/fuse/connections"
    mkdir -p "${CONN_DIR}"
}

teardown() {
    rm -rf "${TEST_TMP}"
}

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────

# make_conn <conn_id> <waiting_value>
#   Create a fake FUSE connection directory under $CONN_DIR.
#   Both "waiting" and "abort" files are written (abort initialised to "0").
make_conn() {
    local id="$1"
    local waiting="$2"
    local cdir="${CONN_DIR}/${id}"
    mkdir -p "${cdir}"
    printf '%s\n' "${waiting}" > "${cdir}/waiting"
    printf '0\n' > "${cdir}/abort"
}

# read_abort <conn_id>  →  prints the trimmed content of the abort file
read_abort() {
    local id="$1"
    tr -d '[:space:]' < "${CONN_DIR}/${id}/abort"
}

# run_watchdog [extra args …]
#   Runs fuse-watchdog under "sudo -n bash" with --conn-dir pointing at the
#   mock directory and --one-shot (so the test terminates immediately).
#   Populates $output, $status (via bats `run`).
run_watchdog() {
    run sudo -n bash "${WATCHDOG}" --conn-dir "${CONN_DIR}" --one-shot "$@"
}

# run_watchdog_env KEY=val … [-- extra args …]
#   Same as run_watchdog but injects environment variables via
#   "sudo -n env KEY=val … bash …" to bypass sudo's env_reset.
#   Pass script arguments after a bare "--".
run_watchdog_env() {
    local -a env_pairs=()
    local -a script_args=()
    local past_sep=0
    for arg in "$@"; do
        if [[ "$arg" == "--" ]]; then
            past_sep=1
        elif [[ "$past_sep" -eq 0 ]]; then
            env_pairs+=("$arg")
        else
            script_args+=("$arg")
        fi
    done
    run sudo -n env "${env_pairs[@]}" bash "${WATCHDOG}" \
        --conn-dir "${CONN_DIR}" --one-shot "${script_args[@]}"
}

# ─────────────────────────────────────────────────────────────────────────────
# §1  Startup validation — non-root rejection
#     (These run WITHOUT sudo so the EUID check fires.)
# ─────────────────────────────────────────────────────────────────────────────

@test "non-root: exits non-zero" {
    run bash "${WATCHDOG}" --conn-dir "${CONN_DIR}" --one-shot
    [ "$status" -ne 0 ]
}

@test "non-root: emits FATAL on stderr" {
    run bash "${WATCHDOG}" --conn-dir "${CONN_DIR}" --one-shot
    [[ "${output}" == *"FATAL"* ]]
}

@test "non-root: FATAL message mentions 'root'" {
    run bash "${WATCHDOG}" --conn-dir "${CONN_DIR}" --one-shot
    [[ "${output}" == *"root"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# §2  Startup validation — bad arguments
#     (--help exits before the EUID check; the others need root.)
# ─────────────────────────────────────────────────────────────────────────────

@test "--help: exits 0" {
    run bash "${WATCHDOG}" --help
    [ "$status" -eq 0 ]
}

@test "--help: output mentions 'fuse-watchdog'" {
    run bash "${WATCHDOG}" --help
    [[ "${output}" == *"fuse-watchdog"* ]]
}

@test "--help: output documents --dry-run flag" {
    run bash "${WATCHDOG}" --help
    [[ "${output}" == *"--dry-run"* ]]
}

@test "--help: output documents --threshold flag" {
    run bash "${WATCHDOG}" --help
    [[ "${output}" == *"--threshold"* ]]
}

@test "--help: output documents --one-shot flag" {
    run bash "${WATCHDOG}" --help
    [[ "${output}" == *"--one-shot"* ]]
}

@test "-h is a synonym for --help and exits 0" {
    run bash "${WATCHDOG}" -h
    [ "$status" -eq 0 ]
    [[ "${output}" == *"fuse-watchdog"* ]]
}

@test "unknown flag: exits non-zero" {
    # The arg-parser calls log_fatal before the function is defined, so bash
    # exits 127 (command not found) via set -e.  We only assert non-zero.
    run bash "${WATCHDOG}" --totally-unknown-flag
    [ "$status" -ne 0 ]
}

@test "missing conn-dir: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run sudo -n bash "${WATCHDOG}" --conn-dir "/nonexistent/path/$$" --one-shot
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

@test "threshold=0: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --threshold 0
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

@test "threshold=abc: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --threshold abc
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

@test "threshold negative: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --threshold -5
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

@test "interval=0: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --interval 0
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

@test "interval=abc: exits 1 with FATAL (root required)" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --interval abc
    [ "$status" -ne 0 ]
    [[ "${output}" == *"FATAL"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# §3  One-shot mode
# ─────────────────────────────────────────────────────────────────────────────

@test "one-shot: exits 0 with empty conn-dir" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog
    [ "$status" -eq 0 ]
}

@test "one-shot: output contains 'stopped after 1 poll(s)'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog
    [[ "${output}" == *"stopped after 1 poll(s)"* ]]
}

@test "one-shot: output contains 'One-shot mode'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog
    [[ "${output}" == *"One-shot mode"* ]]
}

@test "one-shot: exits 0 when all connections are below threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 3
    make_conn 2 7
    run_watchdog --threshold 16
    [ "$status" -eq 0 ]
}

# ─────────────────────────────────────────────────────────────────────────────
# §4  Poll logic — threshold boundary conditions
# ─────────────────────────────────────────────────────────────────────────────

@test "poll: waiting < threshold — abort file stays '0'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 10 9
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort 10)" = "0" ]
}

@test "poll: waiting < threshold — no ABORT line in output" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 10 9
    run_watchdog --threshold 10
    [[ "${output}" != *"ABORT"* ]]
}

@test "poll: waiting == threshold — abort file written with '1'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 10 10
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort 10)" = "1" ]
}

@test "poll: waiting == threshold — ABORT line present in output" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 10 10
    run_watchdog --threshold 10
    [[ "${output}" == *"ABORT"* ]]
}

@test "poll: waiting > threshold — abort file written with '1'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 20 99
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort 20)" = "1" ]
}

@test "poll: waiting > threshold — ABORT line present in output" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 20 99
    run_watchdog --threshold 10
    [[ "${output}" == *"ABORT"* ]]
}

@test "poll: multiple connections — only stalled one is aborted" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn healthy 2
    make_conn stalled 50
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort healthy)" = "0" ]
    [ "$(read_abort stalled)" = "1" ]
}

@test "poll: multiple connections — both aborted when both exceed threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn conn_a 20
    make_conn conn_b 30
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort conn_a)" = "1" ]
    [ "$(read_abort conn_b)" = "1" ]
}

@test "poll: conn dir with no 'waiting' file — warns and skips" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    # Create a connection directory with only an abort file (no waiting file)
    mkdir -p "${CONN_DIR}/bad"
    printf '0\n' > "${CONN_DIR}/bad/abort"
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [[ "${output}" == *"WARN"* ]]
}

@test "poll: conn dir with no 'abort' file — warns and skips" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    # Create a connection directory with only a waiting file (no abort file)
    mkdir -p "${CONN_DIR}/bad"
    printf '5\n' > "${CONN_DIR}/bad/waiting"
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [[ "${output}" == *"WARN"* ]]
}

@test "poll: non-numeric waiting value — warns and skips, exits 0" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    mkdir -p "${CONN_DIR}/weird"
    printf 'garbage\n' > "${CONN_DIR}/weird/waiting"
    printf '0\n'        > "${CONN_DIR}/weird/abort"
    run_watchdog --threshold 10
    [ "$status" -eq 0 ]
    [[ "${output}" == *"WARN"* ]]
    # abort must not have been touched
    [ "$(tr -d '[:space:]' < "${CONN_DIR}/weird/abort")" = "0" ]
}

@test "poll: waiting=0 is treated as below threshold — no abort" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn zero 0
    run_watchdog --threshold 1
    [ "$status" -eq 0 ]
    [ "$(read_abort zero)" = "0" ]
}

# ─────────────────────────────────────────────────────────────────────────────
# §5  DRY_RUN mode
# ─────────────────────────────────────────────────────────────────────────────

@test "dry-run flag: exits 0 with empty conn-dir" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [ "$status" -eq 0 ]
}

@test "dry-run flag: exits 0 below threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 3
    run_watchdog --dry-run --threshold 10
    [ "$status" -eq 0 ]
    [[ "${output}" != *"ABORT"* ]]
}

@test "dry-run flag: exits 0 when threshold exceeded" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 50
    run_watchdog --dry-run --threshold 10
    [ "$status" -eq 0 ]
}

@test "dry-run flag: logs [DRY-RUN] marker when threshold exceeded" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 50
    run_watchdog --dry-run --threshold 10
    [[ "${output}" == *"[DRY-RUN]"* ]]
}

@test "dry-run flag: abort file NOT written when threshold exceeded" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 50
    run_watchdog --dry-run --threshold 10
    [ "$(read_abort 1)" = "0" ]
}

@test "dry-run flag: logs ABORT line alongside [DRY-RUN] when threshold exceeded" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 50
    run_watchdog --dry-run --threshold 10
    [[ "${output}" == *"ABORT"*    ]]
    [[ "${output}" == *"[DRY-RUN]"* ]]
}

@test "DRY_RUN env var: behaves identically to --dry-run flag" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 50
    run_watchdog_env "DRY_RUN=true" -- --threshold 10
    [ "$status" -eq 0 ]
    [[ "${output}" == *"[DRY-RUN]"* ]]
    [ "$(read_abort 1)" = "0" ]
}

@test "DRY_RUN env var: multiple connections — none aborted" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn a 20
    make_conn b 30
    run_watchdog_env "DRY_RUN=true" -- --threshold 10
    [ "$(read_abort a)" = "0" ]
    [ "$(read_abort b)" = "0" ]
}

# ─────────────────────────────────────────────────────────────────────────────
# §6  Environment-variable overrides
# ─────────────────────────────────────────────────────────────────────────────

@test "FUSE_WAITING_THRESHOLD env var: triggers abort at custom threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 5
    # 5 >= 4 should trigger
    run_watchdog_env "FUSE_WAITING_THRESHOLD=4" --
    [ "$status" -eq 0 ]
    [ "$(read_abort 1)" = "1" ]
}

@test "FUSE_WAITING_THRESHOLD env var: does NOT trigger when waiting < custom threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 3
    # 3 < 4 — no trigger
    run_watchdog_env "FUSE_WAITING_THRESHOLD=4" --
    [ "$status" -eq 0 ]
    [ "$(read_abort 1)" = "0" ]
}

@test "FUSE_CONN_DIR env var: overrides the connection directory" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    # Create an alternate conn-dir with a stalled connection
    local alt="${TEST_TMP}/alt_connections"
    mkdir -p "${alt}/99"
    printf '100\n' > "${alt}/99/waiting"
    printf '0\n'   > "${alt}/99/abort"
    # Run without --conn-dir; rely solely on the env var
    run sudo -n env "FUSE_CONN_DIR=${alt}" bash "${WATCHDOG}" \
        --threshold 10 --one-shot
    [ "$status" -eq 0 ]
    [ "$(tr -d '[:space:]' < "${alt}/99/abort")" = "1" ]
}

@test "CLI --threshold overrides FUSE_WAITING_THRESHOLD env var" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 15
    # env says threshold=20 (15 < 20 → no abort) but CLI says 10 (15 >= 10 → abort)
    run_watchdog_env "FUSE_WAITING_THRESHOLD=20" -- --threshold 10
    [ "$status" -eq 0 ]
    [ "$(read_abort 1)" = "1" ]
}

# ─────────────────────────────────────────────────────────────────────────────
# §7  Output / log formatting
# ─────────────────────────────────────────────────────────────────────────────

@test "output: startup banner contains 'fuse-watchdog starting'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [[ "${output}" == *"fuse-watchdog starting"* ]]
}

@test "output: startup banner shows conn dir" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [[ "${output}" == *"conn dir"* ]]
}

@test "output: startup banner shows threshold" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [[ "${output}" == *"threshold"* ]]
}

@test "output: startup banner shows dry-run" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [[ "${output}" == *"dry-run"* ]]
}

@test "output: shutdown line contains 'stopped after'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog
    [[ "${output}" == *"stopped after"* ]]
}

@test "output: every non-separator line begins with '[fuse-watchdog]'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog --dry-run
    [ "$status" -eq 0 ]
    while IFS= read -r line; do
        # Skip blank lines and the Unicode separator lines (────…)
        [[ -z "${line}"              ]] && continue
        [[ "${line}" == ─*           ]] && continue
        [[ "${line}" == \[fuse-watchdog\]* ]] && continue
        echo "Unexpected line format: ${line}" >&2
        return 1
    done <<< "${output}"
}

@test "output: Poll #1 line appears exactly once in one-shot mode" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    run_watchdog
    local count
    count=$(grep -c "Poll #1" <<< "${output}" || true)
    [ "${count}" -eq 1 ]
}

# ─────────────────────────────────────────────────────────────────────────────
# §8  SIGTERM — graceful shutdown
#
# sudo does not forward SIGTERM to its child when run non-interactively.
# We therefore use a "sudo bash -c" wrapper that:
#   1. Starts the watchdog in the background (long interval so it sleeps).
#   2. Waits for it to print its startup banner (confirms it is running).
#   3. Sends SIGTERM directly to the watchdog's bash PID.
#   4. Waits for it to exit and captures the exit code.
#   5. Emits a WATCHDOG_EXIT=<N> sentinel the test can grep for.
# ─────────────────────────────────────────────────────────────────────────────

@test "SIGTERM: watchdog exits 0 after receiving SIGTERM" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 0

    local shell_script
    shell_script="$(cat <<SHELL
OUTFILE=\$(mktemp)
bash '${WATCHDOG}' --conn-dir '${CONN_DIR}' --interval 30 >"\$OUTFILE" 2>&1 &
WPID=\$!
# Poll until the startup banner appears (max 5 s)
for i in \$(seq 1 50); do
    grep -q 'fuse-watchdog starting' "\$OUTFILE" 2>/dev/null && break
    sleep 0.1
done
kill -TERM \$WPID
wait \$WPID 2>/dev/null || true
WXIT=\$?
cat "\$OUTFILE"
rm -f "\$OUTFILE"
echo "WATCHDOG_EXIT=\$WXIT"
SHELL
)"

    run sudo -n bash -c "${shell_script}"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"WATCHDOG_EXIT=0"* ]]
}

@test "SIGTERM: output contains 'shutting down gracefully'" {
    if ! _sudo_ok; then skip "passwordless sudo unavailable"; fi
    make_conn 1 0

    local shell_script
    shell_script="$(cat <<SHELL
OUTFILE=\$(mktemp)
bash '${WATCHDOG}' --conn-dir '${CONN_DIR}' --interval 30 >"\$OUTFILE" 2>&1 &
WPID=\$!
for i in \$(seq 1 50); do
    grep -q 'fuse-watchdog starting' "\$OUTFILE" 2>/dev/null && break
    sleep 0.1
done
kill -TERM \$WPID
wait \$WPID 2>/dev/null || true
cat "\$OUTFILE"
rm -f "\$OUTFILE"
SHELL
)"

    run sudo -n bash -c "${shell_script}"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"shutting down gracefully"* ]]
}
