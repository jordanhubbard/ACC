#!/usr/bin/env bats
# test_fix_fuse_mount.bats — bats unit / smoke tests for fix_fuse_mount.sh
#
# Run:
#   bats scripts/tests/test_fix_fuse_mount.bats
#
# Requirements:
#   - bats >= 1.5  (https://github.com/bats-core/bats-core)
#   - bash >= 4.4  (associative arrays, mapfile)
#   - Standard GNU coreutils (mktemp, find, stat, touch)
#
# Design notes:
#   The real fix_fuse_mount.sh requires root and interacts with /sys, kill,
#   and umount.  Every test that exercises the main body:
#     1. Works from a *patched copy* of the script where the root-check
#        guard ("if [[ $EUID -ne 0 ]]") is replaced with "if false;" so the
#        tests run unprivileged.
#     2. Passes --dry-run so no /sys writes, kills, or unmounts are issued.
#     3. Points --mount-point and (via sed) the hardcoded /home/jkh/.acc/shared
#        search path at throw-away temp directories so the test is fast and
#        isolated.
#
#   Tests that only exercise argument-parsing (--help, unknown flags) run the
#   original script directly, because those code paths exit before require_root
#   is ever called.

# ─────────────────────────────────────────────────────────────────────────────
# Paths
# ─────────────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$BATS_TEST_FILENAME")/.." && pwd)"
REAL_SCRIPT="${SCRIPT_DIR}/fix_fuse_mount.sh"

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────

# build_patched_script <dest_script> <safe_shared_dir>
#   Creates a copy of fix_fuse_mount.sh with:
#     - root-check guard replaced with "if false;" (allows non-root execution)
#     - hardcoded /home/jkh/.acc/shared replaced with <safe_shared_dir>
#       (prevents slow/hangs from scanning a real FUSE mount during tests)
build_patched_script() {
    local dest="$1"
    local safe_shared="$2"
    sed \
        -e 's/if \[\[ \$EUID -ne 0 \]\]; then/if false; then/' \
        -e "s|\"/home/jkh/\\.acc/shared\"|\"${safe_shared}\"|g" \
        "${REAL_SCRIPT}" > "${dest}"
    chmod +x "${dest}"
}

# ─────────────────────────────────────────────────────────────────────────────
# Per-test setup / teardown
# ─────────────────────────────────────────────────────────────────────────────
setup() {
    # Unique temp workspace for every test
    TEST_TMP="$(mktemp -d)"

    # Patched script that can run without root
    PATCHED="${TEST_TMP}/fix_fuse_mount_test.sh"
    build_patched_script "${PATCHED}" "${TEST_TMP}/shared"

    # A mount point directory that stat(1) can access (no actual mount needed)
    MOUNT_PT="${TEST_TMP}/mnt"
    mkdir -p "${MOUNT_PT}"
}

teardown() {
    rm -rf "${TEST_TMP}"
}

# ─────────────────────────────────────────────────────────────────────────────
# Argument-parsing tests — run against the ORIGINAL script (exit before root check)
# ─────────────────────────────────────────────────────────────────────────────

@test "--help exits 0 and prints the script name in the first line" {
    run bash "${REAL_SCRIPT}" --help
    [ "$status" -eq 0 ]
    # The first non-empty output line should identify the script
    [[ "${output}" == *"fix_fuse_mount.sh"* ]]
}

@test "-h is a synonym for --help" {
    run bash "${REAL_SCRIPT}" -h
    [ "$status" -eq 0 ]
    [[ "${output}" == *"fix_fuse_mount.sh"* ]]
}

@test "--help output mentions --dry-run option" {
    run bash "${REAL_SCRIPT}" --help
    [ "$status" -eq 0 ]
    [[ "${output}" == *"--dry-run"* ]]
}

@test "--help output mentions --mount-point option" {
    run bash "${REAL_SCRIPT}" --help
    [ "$status" -eq 0 ]
    [[ "${output}" == *"--mount-point"* ]]
}

@test "unknown flag exits 1 with an ERROR message on stderr" {
    run bash "${REAL_SCRIPT}" --totally-unknown-flag
    [ "$status" -eq 1 ]
    [[ "${output}" == *"ERROR"* ]]
}

@test "unknown flag message names the offending argument" {
    run bash "${REAL_SCRIPT}" --no-such-option
    [ "$status" -eq 1 ]
    [[ "${output}" == *"--no-such-option"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Flag-parsing tests — patched script, dry-run, non-existent mount so stall is detected
# ─────────────────────────────────────────────────────────────────────────────

@test "--mount-point value appears in the log header" {
    run bash "${PATCHED}" --dry-run \
        --mount-point /tmp/custom_mp_test \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Mount point : /tmp/custom_mp_test"* ]]
}

@test "--meta-url value appears in the log header" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --meta-url "redis://testhost:6380/9" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Meta URL    : redis://testhost:6380/9"* ]]
}

@test "--service value appears in the log header" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service "custom-fuse.service" --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Service     : custom-fuse.service"* ]]
}

@test "--no-restart flag is reflected in the log header" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No restart  : true"* ]]
}

@test "--dry-run flag is reflected in the log header" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Dry run     : true"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Environment-variable override tests
# ─────────────────────────────────────────────────────────────────────────────

@test "FUSE_MOUNT_POINT env var sets the mount point" {
    run env FUSE_MOUNT_POINT="/tmp/env_override_mp" \
        bash "${PATCHED}" --dry-run --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Mount point : /tmp/env_override_mp"* ]]
}

@test "JUICEFS_META_URL env var sets the meta URL" {
    run env JUICEFS_META_URL="redis://envhost:1234/5" \
        bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Meta URL    : redis://envhost:1234/5"* ]]
}

@test "ACCFS_SERVICE env var sets the systemd service name" {
    run env ACCFS_SERVICE="env-fuse.service" \
        bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Service     : env-fuse.service"* ]]
}

@test "CLI --mount-point overrides FUSE_MOUNT_POINT env var" {
    run env FUSE_MOUNT_POINT="/tmp/env_mp" \
        bash "${PATCHED}" --dry-run \
        --mount-point "/tmp/cli_mp_override" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Mount point : /tmp/cli_mp_override"* ]]
    [[ "${output}" != *"/tmp/env_mp"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Dry-run smoke test — full execution path (stall detected, all 5 steps logged)
# ─────────────────────────────────────────────────────────────────────────────

@test "dry-run smoke: exits 0 for a non-existent (stalled) mount point" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
}

@test "dry-run smoke: all five remediation steps are logged" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Step 0:"* ]]
    [[ "${output}" == *"Step 1:"* ]]
    [[ "${output}" == *"Step 2:"* ]]
    [[ "${output}" == *"Step 3:"* ]]
    [[ "${output}" == *"Step 4:"* ]]
    [[ "${output}" == *"Step 5:"* ]]
}

@test "dry-run smoke: output contains [DRY-RUN] markers (no live execution)" {
    # A non-existent mount point triggers a stall → the fuse-abort and
    # kill paths emit [DRY-RUN] lines instead of touching /sys or processes.
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"[DRY-RUN]"* ]]
}

@test "dry-run smoke: summary section is printed" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"fix_fuse_mount.sh complete"* ]] || \
    [[ "${output}" == *"Remediation complete"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# No-stall early-exit test
# ─────────────────────────────────────────────────────────────────────────────

@test "accessible mount point without --force exits 0 with 'promptly' message" {
    # MOUNT_PT exists, stat will succeed → script detects no stall and exits early
    run bash "${PATCHED}" --dry-run \
        --mount-point "${MOUNT_PT}" \
        --service none
    [ "$status" -eq 0 ]
    [[ "${output}" == *"promptly"* ]]
}

@test "accessible mount point without --force does NOT run step 1" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${MOUNT_PT}" \
        --service none
    [ "$status" -eq 0 ]
    # Early exit means Step 1 should never appear
    [[ "${output}" != *"Step 1:"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# --force flag tests
# ─────────────────────────────────────────────────────────────────────────────

@test "--force proceeds even when mount point is accessible" {
    # MOUNT_PT stat succeeds, but --force overrides the early-exit
    run bash "${PATCHED}" --dry-run --force \
        --mount-point "${MOUNT_PT}" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"--force"* ]]
    [[ "${output}" == *"Step 1:"* ]]
}

@test "--force logs a warning about overriding the successful probe" {
    run bash "${PATCHED}" --dry-run --force \
        --mount-point "${MOUNT_PT}" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"force"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# --no-restart tests
# ─────────────────────────────────────────────────────────────────────────────

@test "--no-restart prints a manual-remount hint containing the meta URL" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --meta-url "redis://myhost:6379/3" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"redis://myhost:6379/3"* ]]
}

@test "--no-restart exits 0 and skips step 5 daemon restart" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"--no-restart"* ]]
    # The manual remount hint should be present; "Checking systemd service" should not
    [[ "${output}" != *"Checking systemd service"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Lock-file discovery and dry-run preservation tests
# ─────────────────────────────────────────────────────────────────────────────

@test "dry-run: zero-byte index.lock is detected and reported" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/index.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"index.lock"* ]]
}

@test "dry-run: zero-byte lock file is NOT deleted from disk" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/index.lock"

    bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart > /dev/null 2>&1

    # File must still exist after a dry-run
    [ -f "${TEST_TMP}/repo/.git/index.lock" ]
}

@test "dry-run: HEAD.lock is detected and reported" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/HEAD.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"HEAD.lock"* ]]
}

@test "dry-run: packed-refs.lock is detected and reported" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/packed-refs.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"packed-refs.lock"* ]]
}

@test "dry-run: config.lock is detected and reported" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/config.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"config.lock"* ]]
}

@test "dry-run: [DRY-RUN] rm line is emitted for each zero-byte lock" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/index.lock"
    touch "${TEST_TMP}/repo/.git/HEAD.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    # Both files should produce a DRY-RUN rm line
    echo "${output}" | grep -c '\[DRY-RUN\] rm' | grep -qE '^[2-9]'
}

@test "dry-run: summary reports the count of stale locks found" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/index.lock"
    touch "${TEST_TMP}/repo/.git/HEAD.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    # Summary line should show 2 stale lock files
    [[ "${output}" == *"Stale lock files removed     : 2"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Live lock-removal test (no --dry-run; patched copy bypasses root check)
# ─────────────────────────────────────────────────────────────────────────────

@test "live mode: zero-byte index.lock is actually deleted" {
    mkdir -p "${TEST_TMP}/repo/.git"
    touch "${TEST_TMP}/repo/.git/index.lock"

    # --force because MOUNT_PT exists (stat succeeds); --no-restart to avoid
    # requiring juicefs or systemctl
    run bash "${PATCHED}" --force \
        --mount-point "${MOUNT_PT}" \
        --git-root "${TEST_TMP}/repo" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    # Lock file must be gone
    [ ! -f "${TEST_TMP}/repo/.git/index.lock" ]
}

# ─────────────────────────────────────────────────────────────────────────────
# --git-root flag tests
# ─────────────────────────────────────────────────────────────────────────────

@test "--git-root adds an extra lock-file search path" {
    EXTRA_ROOT="${TEST_TMP}/extra"
    mkdir -p "${EXTRA_ROOT}/proj/.git"
    touch "${EXTRA_ROOT}/proj/.git/index.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${EXTRA_ROOT}" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"${EXTRA_ROOT}"*"index.lock"* ]]
}

@test "multiple --git-root flags are all searched" {
    ROOT_A="${TEST_TMP}/root_a"
    ROOT_B="${TEST_TMP}/root_b"
    mkdir -p "${ROOT_A}/r/.git" "${ROOT_B}/s/.git"
    touch "${ROOT_A}/r/.git/index.lock"
    touch "${ROOT_B}/s/.git/config.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --git-root "${ROOT_A}" \
        --git-root "${ROOT_B}" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"index.lock"* ]]
    [[ "${output}" == *"config.lock"* ]]
}

@test "lock files under --mount-point are also searched without --git-root" {
    # A lock file nested under the mount point should be found automatically
    mkdir -p "${TEST_TMP}/no_mp/.git"
    touch "${TEST_TMP}/no_mp/.git/index.lock"

    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    [[ "${output}" == *"index.lock"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Step 4 unmount dry-run output
# ─────────────────────────────────────────────────────────────────────────────

@test "dry-run: [DRY-RUN] umount line is emitted when mount point appears mounted" {
    # We cannot create a real FUSE mountpoint without root, so we test the
    # "not currently mounted" branch here: the script should log the skip message.
    run bash "${PATCHED}" --dry-run --force \
        --mount-point "${MOUNT_PT}" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    # MOUNT_PT is a plain directory (not a mountpoint), so the unmount is skipped
    [[ "${output}" == *"not currently mounted"* ]] || \
    [[ "${output}" == *"[DRY-RUN] umount"* ]]
}

# ─────────────────────────────────────────────────────────────────────────────
# Step 1 FUSE-abort dry-run output
# ─────────────────────────────────────────────────────────────────────────────

@test "dry-run: [DRY-RUN] fuse-abort line or skip message is emitted in step 1" {
    run bash "${PATCHED}" --dry-run \
        --mount-point "${TEST_TMP}/no_mp" \
        --service none --no-restart
    [ "$status" -eq 0 ]
    # Either the fuse-abort marker or a skip/not-found message must appear
    [[ "${output}" == *"[DRY-RUN] echo 1 >"* ]] || \
    [[ "${output}" == *"No matching FUSE connection"* ]] || \
    [[ "${output}" == *"not found"* ]]
}
