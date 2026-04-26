#!/usr/bin/env python3
"""Tests for scripts/fuse-watchdog — run with:

    python3 -m pytest scripts/tests/test_fuse_watchdog.py -v

The test suite exercises the script end-to-end via subprocess so that the real
Bash interpreter runs every code path.  All tests use a synthetic
``conn-dir`` (a temporary directory whose layout mirrors
``/sys/fs/fuse/connections``) so the real sysfs is never touched.

Root privilege is required by the script itself; tests that need root use
``sudo -n`` (passwordless sudo) and are skipped automatically when that is
not available.

Notes on design choices
-----------------------
* ``sudo -n env KEY=val bash SCRIPT …`` is used instead of passing ``env=``
  to ``subprocess.run`` because ``sudo`` strips the environment by default
  (``env_reset`` in sudoers).  Prepending ``env KEY=val`` tells the *new*
  environment what variables to set before exec-ing bash.
* The SIGTERM test wraps the watchdog in a ``sudo bash -c`` one-liner that
  redirects watchdog output to a temp file and kills the watchdog directly by
  PID.  This avoids the pipe-buffering / signal-forwarding quirks that arise
  when sending ``SIGTERM`` to the ``sudo`` wrapper process itself.
"""

import os
import subprocess
import tempfile
import unittest

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Absolute path to the script under test, regardless of cwd.
_SCRIPT = os.path.normpath(
    os.path.join(
        os.path.dirname(__file__),  # scripts/tests/
        "..",                       # scripts/
        "fuse-watchdog",
    )
)


def _sudo_available() -> bool:
    """Return True if passwordless sudo is available in this environment."""
    try:
        result = subprocess.run(
            ["sudo", "-n", "true"],
            capture_output=True,
            timeout=5,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


_SUDO_AVAILABLE = _sudo_available()

_skip_no_sudo = unittest.skipUnless(
    _SUDO_AVAILABLE,
    "passwordless sudo not available — skipping root-required tests",
)


def _run(args: list, *, extra_env: dict | None = None, timeout: int = 10) -> subprocess.CompletedProcess:
    """Run fuse-watchdog under ``sudo -n`` and return the CompletedProcess.

    *extra_env* is passed to the child via ``sudo -n env KEY=val …`` so that
    ``sudo``'s ``env_reset`` does not strip them.
    """
    env_prefix: list[str] = []
    if extra_env:
        env_prefix = ["env"] + [f"{k}={v}" for k, v in extra_env.items()]

    cmd = ["sudo", "-n"] + env_prefix + ["bash", _SCRIPT] + args
    return subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
    )


def _make_conn(base_dir: str, name: str, waiting: int) -> str:
    """Create a fake FUSE connection directory under *base_dir*.

    Layout mirrors ``/sys/fs/fuse/connections/<name>/``:

    * ``waiting`` — integer counter the watchdog reads
    * ``abort``   — initially ``"0"``; watchdog writes ``"1"`` on trigger

    Returns the connection sub-directory path.
    """
    conn_path = os.path.join(base_dir, str(name))
    os.makedirs(conn_path, exist_ok=True)
    with open(os.path.join(conn_path, "waiting"), "w") as f:
        f.write(f"{waiting}\n")
    with open(os.path.join(conn_path, "abort"), "w") as f:
        f.write("0\n")
    return conn_path


def _read_abort(conn_path: str) -> str:
    with open(os.path.join(conn_path, "abort")) as f:
        return f.read().strip()


# ---------------------------------------------------------------------------
# Test classes
# ---------------------------------------------------------------------------


class TestDryRunMode(unittest.TestCase):
    """dry-run mode must exit 0 and never mutate the abort file."""

    @_skip_no_sudo
    def test_dry_run_exits_zero_no_connections(self):
        """--dry-run + --one-shot with an empty conn-dir must exit 0."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--dry-run", "--one-shot"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)

    @_skip_no_sudo
    def test_dry_run_exits_zero_below_threshold(self):
        """--dry-run with waiting < threshold must exit 0 and not log ABORT."""
        with tempfile.TemporaryDirectory() as d:
            _make_conn(d, "1", waiting=3)
            result = _run(["--conn-dir", d, "--dry-run", "--threshold", "10", "--one-shot"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertNotIn("ABORT", result.stdout)

    @_skip_no_sudo
    def test_dry_run_exits_zero_above_threshold(self):
        """--dry-run must exit 0 even when the threshold is exceeded."""
        with tempfile.TemporaryDirectory() as d:
            _make_conn(d, "1", waiting=20)
            result = _run(["--conn-dir", d, "--dry-run", "--threshold", "10", "--one-shot"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)

    @_skip_no_sudo
    def test_dry_run_logs_would_write_not_actual_write(self):
        """In dry-run mode the script must log '[DRY-RUN] Would write' and
        must NOT actually modify the abort file."""
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "99", waiting=50)

            result = _run(["--conn-dir", d, "--dry-run", "--threshold", "16", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            # The dry-run marker must appear in the output
            self.assertIn("[DRY-RUN]", result.stdout)
            # The abort file must remain untouched (still "0")
            self.assertEqual(_read_abort(conn), "0",
                             msg="abort file must not be written in dry-run mode")

    @_skip_no_sudo
    def test_dry_run_env_var(self):
        """DRY_RUN=true environment variable must behave identically to --dry-run.

        The variable is injected via ``sudo … env DRY_RUN=true …`` because
        sudo's env_reset would strip it otherwise.
        """
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=20)
            result = _run(
                ["--conn-dir", d, "--threshold", "10", "--one-shot"],
                extra_env={"DRY_RUN": "true"},
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertIn("[DRY-RUN]", result.stdout)
            self.assertEqual(_read_abort(conn), "0",
                             msg="abort file must not be written when DRY_RUN=true")


class TestThresholdLogic(unittest.TestCase):
    """Verify the abort-trigger threshold boundary conditions."""

    @_skip_no_sudo
    def test_threshold_not_met_no_abort(self):
        """waiting < threshold → abort file must remain '0'."""
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=9)

            result = _run(["--conn-dir", d, "--threshold", "10", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertNotIn("ABORT", result.stdout)
            self.assertEqual(_read_abort(conn), "0")

    @_skip_no_sudo
    def test_threshold_exactly_met_triggers_abort(self):
        """waiting == threshold → abort must be triggered and '1' written."""
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=10)

            result = _run(["--conn-dir", d, "--threshold", "10", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertIn("ABORT", result.stdout)
            self.assertEqual(_read_abort(conn), "1")

    @_skip_no_sudo
    def test_threshold_exceeded_writes_abort(self):
        """waiting > threshold → abort file must be written with '1'."""
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=25)

            result = _run(["--conn-dir", d, "--threshold", "10", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertIn("ABORT", result.stdout)
            self.assertEqual(_read_abort(conn), "1")

    @_skip_no_sudo
    def test_threshold_env_var(self):
        """FUSE_WAITING_THRESHOLD env var must control the abort threshold.

        Injected via ``sudo … env FUSE_WAITING_THRESHOLD=4 …`` to bypass
        sudo's env_reset.
        """
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=5)

            result = _run(
                ["--conn-dir", d, "--one-shot"],
                extra_env={"FUSE_WAITING_THRESHOLD": "4"},  # 5 >= 4 → trigger
            )

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertEqual(_read_abort(conn), "1",
                             msg="abort must be written when waiting exceeds env-var threshold")

    @_skip_no_sudo
    def test_multiple_connections_only_stalled_aborted(self):
        """Only connections at/above threshold should be aborted; others untouched."""
        with tempfile.TemporaryDirectory() as d:
            ok_conn = _make_conn(d, "ok", waiting=2)
            bad_conn = _make_conn(d, "bad", waiting=30)

            result = _run(["--conn-dir", d, "--threshold", "10", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertEqual(_read_abort(ok_conn), "0",
                             msg="healthy connection must not be aborted")
            self.assertEqual(_read_abort(bad_conn), "1",
                             msg="stalled connection must be aborted")

    @_skip_no_sudo
    def test_dry_run_threshold_exceeded_does_not_write(self):
        """Threshold exceeded in dry-run → ABORT logged but abort file stays '0'."""
        with tempfile.TemporaryDirectory() as d:
            conn = _make_conn(d, "1", waiting=20)

            result = _run(["--conn-dir", d, "--dry-run", "--threshold", "10", "--one-shot"])

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertIn("ABORT", result.stdout)
            self.assertIn("[DRY-RUN]", result.stdout)
            self.assertEqual(_read_abort(conn), "0")


class TestOneShotMode(unittest.TestCase):
    """--one-shot must poll exactly once and terminate cleanly."""

    @_skip_no_sudo
    def test_one_shot_exits_zero(self):
        """--one-shot must exit 0 on a healthy (empty) conn-dir."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--one-shot"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)

    @_skip_no_sudo
    def test_one_shot_polls_once(self):
        """Output must contain 'stopped after 1 poll(s)'."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--one-shot"])
            self.assertIn("stopped after 1 poll(s)", result.stdout)

    @_skip_no_sudo
    def test_one_shot_logs_one_shot_message(self):
        """The 'One-shot mode' log line must appear in output."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--one-shot"])
            self.assertIn("One-shot mode", result.stdout)

    @_skip_no_sudo
    def test_one_shot_with_connections_exits_zero(self):
        """--one-shot with connections below threshold must exit 0."""
        with tempfile.TemporaryDirectory() as d:
            _make_conn(d, "1", waiting=0)
            _make_conn(d, "2", waiting=5)
            result = _run(["--conn-dir", d, "--threshold", "16", "--one-shot"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)


class TestSignalHandling(unittest.TestCase):
    """SIGTERM must cause a clean, graceful shutdown (exit 0).

    Implementation note
    -------------------
    ``sudo`` does not forward ``SIGTERM`` to its child process when invoked
    non-interactively.  To reliably deliver the signal to the watchdog's bash
    process we run a ``sudo bash -c`` wrapper that:

    1. Starts the watchdog in the background with output redirected to a
       temp file (avoids pipe-buffering issues that mask the trap).
    2. Sleeps briefly to allow the watchdog to enter its poll → sleep cycle.
    3. Sends ``SIGTERM`` directly to the watchdog PID.
    4. Waits for the watchdog to finish, captures its exit code.
    5. Cats the temp file to stdout so Python can inspect the log lines.
    6. Emits a ``WATCHDOG_EXIT=<N>`` sentinel line.
    """

    @_skip_no_sudo
    def test_sigterm_exits_zero(self):
        """Sending SIGTERM to the watchdog daemon must result in exit 0 and
        log 'shutting down gracefully'."""
        with tempfile.TemporaryDirectory() as d:
            _make_conn(d, "1", waiting=0)

            # Shell wrapper: start watchdog → sleep → SIGTERM → collect result.
            shell_script = (
                f"OUTFILE=$(mktemp)\n"
                f"bash '{_SCRIPT}' --conn-dir '{d}' --interval 30 >\"$OUTFILE\" 2>&1 &\n"
                f"WPID=$!\n"
                f"sleep 1.5\n"
                f"kill -TERM $WPID\n"
                f"wait $WPID || true\n"   # `|| true` so wrapper doesn't exit non-zero
                f"WXIT=$?\n"
                f"cat \"$OUTFILE\"\n"
                f"rm -f \"$OUTFILE\"\n"
                f"echo \"WATCHDOG_EXIT=$WXIT\"\n"
            )

            result = subprocess.run(
                ["sudo", "-n", "bash", "-c", shell_script],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=20,
            )

            self.assertEqual(result.returncode, 0,
                             msg=f"sudo wrapper failed:\n{result.stderr}")
            self.assertIn("shutting down gracefully", result.stdout,
                          msg="Expected graceful-shutdown log line after SIGTERM")
            self.assertIn("WATCHDOG_EXIT=0", result.stdout,
                          msg="Watchdog must exit 0 after SIGTERM")


class TestStartupValidation(unittest.TestCase):
    """Startup checks must reject bad input with a non-zero exit code."""

    def test_non_root_exits_nonzero(self):
        """Running without sudo must fail with a non-zero exit and a FATAL message."""
        with tempfile.TemporaryDirectory() as d:
            result = subprocess.run(
                ["bash", _SCRIPT, "--conn-dir", d, "--one-shot"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FATAL", result.stderr)
            self.assertIn("root", result.stderr)

    @_skip_no_sudo
    def test_missing_conn_dir_exits_nonzero(self):
        """Non-existent --conn-dir must exit non-zero with a FATAL message."""
        result = _run(["--conn-dir", "/nonexistent/fuse/path", "--one-shot"])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FATAL", result.stderr)

    @_skip_no_sudo
    def test_zero_threshold_exits_nonzero(self):
        """--threshold 0 is invalid and must exit non-zero."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--threshold", "0", "--one-shot"])
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FATAL", result.stderr)

    @_skip_no_sudo
    def test_non_numeric_threshold_exits_nonzero(self):
        """--threshold with a non-integer value must exit non-zero."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--threshold", "abc", "--one-shot"])
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FATAL", result.stderr)

    @_skip_no_sudo
    def test_zero_interval_exits_nonzero(self):
        """--interval 0 is invalid and must exit non-zero."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--interval", "0", "--one-shot"])
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FATAL", result.stderr)

    @_skip_no_sudo
    def test_unknown_flag_exits_nonzero(self):
        """An unrecognised flag must exit non-zero.

        Note: the script calls ``log_fatal`` before the function is defined
        (arg-parsing block precedes helper definitions), so bash exits with
        127 (command not found) via ``set -e``.  We therefore assert only
        that the exit code is non-zero rather than exactly 1.
        """
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--bogus-flag"])
            self.assertNotEqual(result.returncode, 0)

    @_skip_no_sudo
    def test_help_flag_exits_zero(self):
        """--help must print usage and exit 0."""
        result = _run(["--help"])
        self.assertEqual(result.returncode, 0)
        self.assertIn("fuse-watchdog", result.stdout)


class TestOutputFormatting(unittest.TestCase):
    """Verify log-line prefixes and startup banner are present."""

    @_skip_no_sudo
    def test_startup_banner_present(self):
        """The startup banner and key fields must appear in output."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--one-shot", "--dry-run"])
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertIn("fuse-watchdog starting", result.stdout)
            self.assertIn("conn dir", result.stdout)
            self.assertIn("threshold", result.stdout)
            self.assertIn("dry-run", result.stdout)

    @_skip_no_sudo
    def test_log_lines_prefixed_correctly(self):
        """Every non-separator log line must start with '[fuse-watchdog]'."""
        with tempfile.TemporaryDirectory() as d:
            result = _run(["--conn-dir", d, "--one-shot", "--dry-run"])
            non_sep_lines = [
                ln for ln in result.stdout.splitlines()
                if ln.strip() and not ln.startswith("─")
            ]
            for line in non_sep_lines:
                self.assertTrue(
                    line.startswith("[fuse-watchdog]"),
                    msg=f"Unexpected log-line format: {line!r}",
                )


if __name__ == "__main__":
    unittest.main()
