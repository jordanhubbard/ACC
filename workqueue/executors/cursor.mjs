/**
 * cursor.mjs — Cursor CLI executor (opt-in, experimental)
 *
 * Runs `cursor --headless --task "<prompt>"` as a subprocess.
 * Only activate via `preferred_executor: cursor_cli` or `required_executors: ["cursor_cli"]`.
 * Not stable as of 2026-Q1 — use for opt-in tasks only.
 *
 * Auth: CURSOR_SESSION_TOKEN env var, or ~/.cursor/session (set by Cursor login).
 */

import { execFile } from 'child_process';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

/**
 * @param {object} item          Workqueue item
 * @param {object} agentConfig   { repoPath, timeoutMs }
 * @returns {Promise<object>}    { output, executor, exitCode }
 */
export async function runCursor(item, agentConfig = {}) {
  const repoPath  = item.repoPath || agentConfig.repoPath || process.cwd();
  const timeoutMs = agentConfig.timeoutMs || 300_000;

  const cliArgs = ['--headless', '--task', item.description];

  try {
    const { stdout, stderr } = await execFileAsync('cursor', cliArgs, {
      cwd:     repoPath,
      timeout: timeoutMs,
      env:     process.env,
    });
    return {
      output:   stdout || stderr,
      executor: 'cursor_cli',
      exitCode: 0,
    };
  } catch (err) {
    return {
      output:   (err.stdout || '') || err.message,
      executor: 'cursor_cli',
      exitCode: err.code ?? 1,
    };
  }
}
