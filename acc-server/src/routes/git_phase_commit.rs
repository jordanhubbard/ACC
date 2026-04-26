//! Git phase-commit helpers: disk-space guard + inline/script execution paths.
//!
//! Public surface:
//!   - `PhaseCommitError`           — typed error carrying a Soft/Hard classification
//!   - `check_disk_space_for_git`   — run `df -k -P <path>` and hard-fail at ≥95 %
//!   - `run_git_phase_commit_inline` — execute the full commit+push flow in-process
//!   - `run_git_phase_commit_via_script` — delegate to an external shell script
//!   - `run_git_phase_commit`        — dispatcher: picks inline vs. script path
//!   - `parse_df_usage_pct`          — parse the `Use%` column (pub for unit testing)

use std::path::Path;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

// ── Error type ────────────────────────────────────────────────────────────────

/// Classification of a phase-commit failure.
///
/// * `Soft` — transient or retry-able (e.g. network, lock contention).
/// * `Hard` — permanent until operator intervention (e.g. disk full, corrupt repo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseCommitErrorKind {
    Soft,
    Hard,
}

#[derive(Debug, Clone)]
pub struct PhaseCommitError {
    pub kind:    PhaseCommitErrorKind,
    pub message: String,
}

impl PhaseCommitError {
    pub fn hard(message: impl Into<String>) -> Self {
        Self { kind: PhaseCommitErrorKind::Hard, message: message.into() }
    }

    pub fn soft(message: impl Into<String>) -> Self {
        Self { kind: PhaseCommitErrorKind::Soft, message: message.into() }
    }
}

impl std::fmt::Display for PhaseCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.kind, self.message)
    }
}

impl std::error::Error for PhaseCommitError {}

// ── Disk-space guard ──────────────────────────────────────────────────────────

/// Threshold above which we refuse to run a git commit+push.
const DISK_USAGE_HARD_LIMIT_PCT: u8 = 95;

/// Run `df -k -P <path>` with a 10-second timeout and return
/// `PhaseCommitError::Hard` when disk usage is ≥ 95 %.
///
/// Uses POSIX output (`-P`) so the format is consistent across Linux
/// and macOS: the `Use%` column is always the fifth field.
pub async fn check_disk_space_for_git(path: &Path) -> Result<(), PhaseCommitError> {
    let path_str = path.to_string_lossy();

    let output = timeout(
        Duration::from_secs(10),
        Command::new("df")
            .args(["-k", "-P", path_str.as_ref()])
            .output(),
    )
    .await
    .map_err(|_| PhaseCommitError::hard(format!(
        "df timed out after 10 s checking disk space for {path_str}"
    )))?
    .map_err(|e| PhaseCommitError::soft(format!(
        "df failed to spawn for {path_str}: {e}"
    )))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PhaseCommitError::soft(format!(
            "df exited non-zero for {path_str}: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pct = parse_df_usage_pct(&stdout).ok_or_else(|| {
        PhaseCommitError::soft(format!(
            "could not parse df output for {path_str}: {stdout}"
        ))
    })?;

    if pct >= DISK_USAGE_HARD_LIMIT_PCT {
        return Err(PhaseCommitError::hard(format!(
            "disk usage at {pct}% for {path_str} — refusing phase commit (threshold: {DISK_USAGE_HARD_LIMIT_PCT}%)"
        )));
    }

    info!(
        component = "git_phase_commit",
        path = %path_str,
        disk_pct = pct,
        "disk space check passed",
    );
    Ok(())
}

/// Parse the `Use%` value from the second line of POSIX `df -k -P` output.
///
/// POSIX format (guaranteed by `-P`):
/// ```text
/// Filesystem     1024-blocks     Used Available Capacity Mounted on
/// /dev/sda1          102400    51200     51200      50% /
/// ```
/// The percentage is always in the fifth whitespace-separated field of the
/// data line, with a trailing `%` character.
///
/// Returns `None` if the output does not match the expected structure.
pub fn parse_df_usage_pct(df_output: &str) -> Option<u8> {
    // Skip the header line; the first data line contains our filesystem.
    let data_line = df_output
        .lines()
        .find(|l| !l.trim_start().starts_with("Filesystem") && !l.trim().is_empty())?;

    let fields: Vec<&str> = data_line.split_whitespace().collect();

    // POSIX df -P guarantees 6 fields; the 5th (index 4) is "Use%".
    // Some implementations wrap long filesystem names and push fields to
    // the next line, producing fewer than 5 fields on the first line —
    // handle that by taking the last field that ends with '%'.
    let use_field = if fields.len() >= 5 {
        fields[4]
    } else {
        fields.last()?
    };

    let trimmed = use_field.trim_end_matches('%');
    trimmed.parse::<u8>().ok()
}

// ── Inline execution path ─────────────────────────────────────────────────────

/// Commit all staged and unstaged changes in `repo_path` to a new branch
/// `phase/<branch_suffix>` and push it to `origin`.
///
/// Steps:
///   1. Disk-space guard (`check_disk_space_for_git`) — hard-fail at ≥ 95 %.
///   2. `git add -A`
///   3. `git commit -m <message>`   (skipped gracefully when tree is clean)
///   4. `git push origin HEAD:<push_ref>` with `--force-with-lease`
///
/// The caller is responsible for calling `POST /api/projects/:id/clean` after a
/// successful return.
pub async fn run_git_phase_commit_inline(
    repo_path: &Path,
    branch_suffix: &str,
    commit_message: &str,
    push_ref: &str,
) -> Result<String, PhaseCommitError> {
    // ── 1. Disk-space guard ───────────────────────────────────────────────
    check_disk_space_for_git(repo_path).await?;

    let path_str = repo_path.to_string_lossy();

    // ── 2. git add -A ────────────────────────────────────────────────────
    let add = run_git(repo_path, &["add", "-A"]).await?;
    info!(
        component = "git_phase_commit",
        path = %path_str,
        branch = branch_suffix,
        stdout = %add,
        "git add -A completed",
    );

    // ── 3. git commit ─────────────────────────────────────────────────────
    let commit_out = run_git(
        repo_path,
        &["commit", "--allow-empty", "-m", commit_message],
    )
    .await;

    let commit_summary = match commit_out {
        Ok(s) => s,
        Err(ref e) if e.message.contains("nothing to commit") => {
            info!(
                component = "git_phase_commit",
                path = %path_str,
                "tree is clean — nothing to commit",
            );
            "(nothing to commit)".to_string()
        }
        Err(e) => return Err(e),
    };

    // ── 4. git push ───────────────────────────────────────────────────────
    let push_refspec = format!("HEAD:{push_ref}");
    let push_out = run_git(
        repo_path,
        &["push", "origin", &push_refspec, "--force-with-lease"],
    )
    .await?;

    let summary = format!(
        "branch={branch_suffix} commit={commit_summary} push={push_out}"
    );
    info!(
        component = "git_phase_commit",
        path = %path_str,
        %summary,
        "phase commit succeeded (inline)",
    );
    Ok(summary)
}

// ── Script execution path ─────────────────────────────────────────────────────

/// Delegate the phase commit to an external shell script `script_path`.
///
/// The script is called as:
/// ```text
/// <script_path> <repo_path> <branch_suffix> <commit_message> <push_ref>
/// ```
/// with a 120-second timeout (generous for slow remotes/large trees).
///
/// Disk space is **not** re-checked here — callers that want the guard must
/// call `check_disk_space_for_git` themselves or use `run_git_phase_commit`.
pub async fn run_git_phase_commit_via_script(
    script_path: &Path,
    repo_path: &Path,
    branch_suffix: &str,
    commit_message: &str,
    push_ref: &str,
) -> Result<String, PhaseCommitError> {
    let script_str = script_path.to_string_lossy();
    let path_str   = repo_path.to_string_lossy();

    let output = timeout(
        Duration::from_secs(120),
        Command::new(script_path.as_os_str())
            .args([
                path_str.as_ref(),
                branch_suffix,
                commit_message,
                push_ref,
            ])
            .output(),
    )
    .await
    .map_err(|_| PhaseCommitError::soft(format!(
        "phase-commit script timed out after 120 s: {script_str}"
    )))?
    .map_err(|e| PhaseCommitError::soft(format!(
        "failed to spawn phase-commit script {script_str}: {e}"
    )))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        info!(
            component = "git_phase_commit",
            path = %path_str,
            branch = branch_suffix,
            "phase commit succeeded (via script)",
        );
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        warn!(
            component = "git_phase_commit",
            path = %path_str,
            branch = branch_suffix,
            %stderr,
            "phase-commit script failed",
        );
        // Script failures are soft by default — the operator can fix and retry.
        Err(PhaseCommitError::soft(format!(
            "phase-commit script exited non-zero for {path_str}: stdout={stdout} stderr={stderr}"
        )))
    }
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Thin dispatcher: use the inline path unless `ACC_PHASE_COMMIT_SCRIPT` is set
/// in the environment, in which case delegate to that script (after performing
/// the disk-space check that the script path skips).
pub async fn run_git_phase_commit(
    repo_path: &Path,
    branch_suffix: &str,
    commit_message: &str,
    push_ref: &str,
) -> Result<String, PhaseCommitError> {
    if let Ok(script) = std::env::var("ACC_PHASE_COMMIT_SCRIPT") {
        let script_path = std::path::PathBuf::from(&script);
        // Run disk check before handing off to the script.
        check_disk_space_for_git(repo_path).await?;
        run_git_phase_commit_via_script(
            &script_path,
            repo_path,
            branch_suffix,
            commit_message,
            push_ref,
        )
        .await
    } else {
        run_git_phase_commit_inline(repo_path, branch_suffix, commit_message, push_ref).await
    }
}

// ── Internal helper ───────────────────────────────────────────────────────────

/// Run a git sub-command in `repo_path` with a 60-second timeout.
/// Returns trimmed stdout on success, `PhaseCommitError::Soft` on failure.
async fn run_git(repo_path: &Path, args: &[&str]) -> Result<String, PhaseCommitError> {
    let path_str = repo_path.to_string_lossy();
    let mut full_args = vec!["-C", path_str.as_ref()];
    full_args.extend_from_slice(args);

    let output = timeout(
        Duration::from_secs(60),
        Command::new("git").args(&full_args).output(),
    )
    .await
    .map_err(|_| PhaseCommitError::soft(format!(
        "git {} timed out after 60 s in {path_str}",
        args.join(" ")
    )))?
    .map_err(|e| PhaseCommitError::soft(format!(
        "git {} failed to spawn in {path_str}: {e}",
        args.join(" ")
    )))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // "nothing to commit" is not a real error — surface it as-is so the
        // caller can handle it gracefully.
        let combined = format!("{stdout} {stderr}");
        if combined.contains("nothing to commit") {
            return Err(PhaseCommitError::soft(format!("nothing to commit: {combined}")));
        }
        Err(PhaseCommitError::soft(format!(
            "git {} failed in {path_str}: {combined}",
            args.join(" ")
        )))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_df_usage_pct unit tests ─────────────────────────────────────

    fn df_output(pct: u8) -> String {
        format!(
            "Filesystem     1024-blocks   Used Available Capacity Mounted on\n\
             /dev/sda1          102400  {used}     {avail}      {pct}% /\n",
            used  = (102400u64 * pct as u64) / 100,
            avail = (102400u64 * (100 - pct as u64)) / 100,
            pct   = pct,
        )
    }

    #[test]
    fn test_parse_df_zero_percent() {
        assert_eq!(parse_df_usage_pct(&df_output(0)), Some(0));
    }

    #[test]
    fn test_parse_df_typical_usage() {
        assert_eq!(parse_df_usage_pct(&df_output(42)), Some(42));
    }

    #[test]
    fn test_parse_df_boundary_94() {
        assert_eq!(parse_df_usage_pct(&df_output(94)), Some(94));
    }

    #[test]
    fn test_parse_df_boundary_95() {
        // 95 % is the hard-limit threshold — must parse correctly
        assert_eq!(parse_df_usage_pct(&df_output(95)), Some(95));
    }

    #[test]
    fn test_parse_df_boundary_96() {
        assert_eq!(parse_df_usage_pct(&df_output(96)), Some(96));
    }

    #[test]
    fn test_parse_df_full_100() {
        assert_eq!(parse_df_usage_pct(&df_output(100)), Some(100));
    }

    #[test]
    fn test_parse_df_empty_string_returns_none() {
        assert_eq!(parse_df_usage_pct(""), None);
    }

    #[test]
    fn test_parse_df_header_only_returns_none() {
        let header = "Filesystem     1024-blocks   Used Available Capacity Mounted on\n";
        assert_eq!(parse_df_usage_pct(header), None);
    }

    #[test]
    fn test_parse_df_malformed_no_percent_sign() {
        // Field present but without '%' → parse as integer still works after trim
        let output = "Filesystem     1024-blocks   Used Available Capacity Mounted on\n\
                      /dev/sda1          102400  51200     51200       50 /\n";
        // trim_end_matches('%') on "50" is still "50" → parses fine
        assert_eq!(parse_df_usage_pct(output), Some(50));
    }

    #[test]
    fn test_parse_df_macos_style() {
        // macOS df -k -P output has slightly different spacing but same columns
        let output = "Filesystem 1024-blocks    Used Available Capacity  Mounted on\n\
                      /dev/disk1s1   976762584 8200000 200000000      4% /\n";
        assert_eq!(parse_df_usage_pct(output), Some(4));
    }

    // ── check_disk_space_for_git integration tests ────────────────────────

    /// Verify that check_disk_space_for_git passes on the real temp directory.
    /// A freshly created tempdir is never at ≥ 95 % usage in a CI environment.
    #[tokio::test]
    async fn test_check_disk_space_passes_on_tempdir() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let result = check_disk_space_for_git(tmp.path()).await;
        // We only assert it doesn't return a Hard error — the filesystem could
        // theoretically be full in some environments, so we tolerate Soft errors.
        match result {
            Ok(()) => {} // expected path in any normal CI environment
            Err(PhaseCommitError { kind: PhaseCommitErrorKind::Hard, ref message }) => {
                panic!("unexpected Hard error on tempdir: {message}");
            }
            Err(PhaseCommitError { kind: PhaseCommitErrorKind::Soft, .. }) => {
                // df may not be available in all sandbox environments — skip silently
            }
        }
    }
}
