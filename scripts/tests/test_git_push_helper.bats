#!/usr/bin/env bats
# test_git_push_helper.bats — unit tests for is_transient_network_error()
#                              defined in scripts/git-push-helper.sh
#
# Run:
#   bats scripts/tests/test_git_push_helper.bats
#
# Requirements:
#   - bats >= 1.5  (https://github.com/bats-core/bats-core)
#   - bash >= 4.4
#
# Design notes:
#   is_transient_network_error() is a pure function — it takes a string and
#   returns 0 (transient / retry) or 1 (permanent / abort).  No filesystem
#   access or network I/O is required, so every test is fast and isolated.
#
#   git-push-helper.sh uses `set -euo pipefail`, which would abort the test
#   process when we call is_transient_network_error and expect it to return 1
#   (false).  We source the helper inside a sub-shell wrapper that drops
#   set -e so that non-zero returns from the function don't kill bats.

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$BATS_TEST_FILENAME")/.." && pwd)"
HELPER="${SCRIPT_DIR}/git-push-helper.sh"

# Source the helper with set -e disabled so that functions that return 1
# (false) do not abort the test.
load_helper() {
    # shellcheck disable=SC1090
    set +e
    # shellcheck disable=SC1090
    source "$HELPER"
    set -e
}

setup() {
    load_helper
}

# Convenience wrappers that make test assertions read naturally.
assert_transient() {
    local stderr="$1"
    run bash -c "source '$HELPER'; is_transient_network_error \"\$1\"" _ "$stderr"
    [ "$status" -eq 0 ] || \
        { echo "FAIL: expected transient for: $stderr"; return 1; }
}

assert_permanent() {
    local stderr="$1"
    run bash -c "source '$HELPER'; is_transient_network_error \"\$1\"" _ "$stderr"
    [ "$status" -ne 0 ] || \
        { echo "FAIL: expected permanent for: $stderr"; return 1; }
}

# ─────────────────────────────────────────────────────────────────────────────
# Edge cases
# ─────────────────────────────────────────────────────────────────────────────

@test "empty string is NOT transient (unknown errors treated as permanent)" {
    assert_permanent ""
}

@test "whitespace-only string is NOT transient" {
    assert_permanent "   	
  "
}

@test "unrelated message is NOT transient" {
    assert_permanent "Everything is fine."
}

# ─────────────────────────────────────────────────────────────────────────────
# Transient / retriable patterns
# ─────────────────────────────────────────────────────────────────────────────

@test "could not resolve host is transient" {
    assert_transient "fatal: could not resolve host: github.com"
}

@test "unable to connect is transient" {
    assert_transient "fatal: unable to connect to github.com"
}

@test "connection timed out is transient" {
    assert_transient "ssh: connect to host github.com port 22: Connection timed out"
}

@test "connection refused is transient" {
    assert_transient "fatal: Connection refused"
}

@test "network is unreachable is transient" {
    assert_transient "fatal: Network is unreachable"
}

@test "the remote end hung up is transient" {
    assert_transient "error: The remote end hung up unexpectedly"
}

@test "curl error is transient" {
    assert_transient "error: curl_error(6)"
}

@test "ssh connect to host is transient" {
    assert_transient "ssh: connect to host github.com port 22: No route to host"
}

@test "broken pipe is transient" {
    assert_transient "fatal: the remote end hung up unexpectedly; broken pipe"
}

@test "temporary failure is transient" {
    assert_transient "Temporary failure in name resolution"
}

@test "service unavailable is transient" {
    assert_transient "Service Unavailable"
}

@test "503 status code is transient" {
    assert_transient "error: The requested URL returned error: 503"
}

@test "case-insensitive: BROKEN PIPE is transient" {
    assert_transient "BROKEN PIPE occurred during push"
}

@test "case-insensitive: SERVICE UNAVAILABLE is transient" {
    assert_transient "Service Unavailable (503)"
}

@test "503 embedded in longer text is transient" {
    assert_transient "HTTP/2 503 Service Unavailable — upstream timeout"
}

# ─────────────────────────────────────────────────────────────────────────────
# Permanent / hard-fail patterns
# ─────────────────────────────────────────────────────────────────────────────

@test "authentication failed is permanent" {
    assert_permanent "remote: Authentication failed for 'https://github.com/org/repo.git'"
}

@test "could not read username is permanent" {
    assert_permanent "fatal: could not read Username for 'https://github.com'"
}

@test "invalid username or password is permanent" {
    assert_permanent "remote: Invalid username or password."
}

@test "permission denied (publickey) is permanent" {
    assert_permanent "git@github.com: Permission denied (publickey)."
}

@test "[rejected] is permanent" {
    assert_permanent "! [rejected]        phase/1 -> phase/1 (fetch first)"
}

@test "non-fast-forward is permanent" {
    assert_permanent "! [remote rejected] main -> main (non-fast-forward)"
}

@test "[remote rejected] is permanent" {
    assert_permanent "! [remote rejected] HEAD -> main (pre-receive hook declined)"
}

@test "case-insensitive: REJECTED is permanent" {
    assert_permanent "! [REJECTED] main -> main (fetch first)"
}

@test "case-insensitive: Permission Denied is permanent" {
    assert_permanent "Permission Denied (publickey)."
}

# ─────────────────────────────────────────────────────────────────────────────
# Priority: permanent keywords beat transient keywords in the same string
# ─────────────────────────────────────────────────────────────────────────────

@test "rejected beats broken pipe (hard error wins)" {
    assert_permanent "! [rejected] main -> main (fetch first)
fatal: broken pipe"
}

@test "authentication failed beats connection timed out (hard error wins)" {
    assert_permanent "remote: Authentication failed
ssh: connect to host github.com port 22: Connection timed out"
}
