/**
 * claude-cli.mjs — Claude Code CLI executor
 *
 * Runs `claude --print --permission-mode bypassPermissions` as a subprocess.
 * Detects throttle/credit-exhaustion signals and re-throws a ThrottleError so
 * the caller can fall back to another executor.
 *
 * Auth: reads ~/.claude/credentials (same as claude login) or ANTHROPIC_API_KEY env var.
 */

import { execFile } from 'child_process';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

const THROTTLE_RE = /429|rate[\s-]?limit|too many requests|credit[\s-]?balance|token[\s-]?exhaust|quota[\s-]?exceed|billing|overload/i;

export class ThrottleError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ThrottleError';
  }
}

/**
 * @param {object} item          Workqueue item
 * @param {object} agentConfig   { repoPath, model, timeoutMs }
 * @returns {Promise<object>}    { output, executor, exitCode }
 * @throws {ThrottleError}       If Claude signals rate-limiting or credit exhaustion
 */
export async function runClaudeCLI(item, agentConfig = {}) {
  const repoPath  = item.repoPath || agentConfig.repoPath || process.cwd();
  const timeoutMs = agentConfig.timeoutMs || 300_000;

  const claudeBin = agentConfig.claudeBin || 'claude';
  const cliArgs = ['--print', '--permission-mode', 'bypassPermissions', item.description];

  let stdout = '';
  let stderr = '';

  try {
    const result = await execFileAsync(claudeBin, cliArgs, {
      cwd:     repoPath,
      timeout: timeoutMs,
      env:     process.env,
    });
    stdout = result.stdout;
    stderr = result.stderr;
  } catch (err) {
    const combined = (err.stdout || '') + (err.stderr || '') + (err.message || '');
    if (THROTTLE_RE.test(combined)) {
      throw new ThrottleError(`Claude throttled/exhausted: ${combined.slice(0, 200)}`);
    }
    return {
      output:   (err.stdout || '') || err.message,
      executor: 'claude_cli',
      exitCode: err.code ?? 1,
    };
  }

  const output = stdout || stderr;
  if (THROTTLE_RE.test(output)) {
    throw new ThrottleError(`Claude throttled/exhausted: ${output.slice(0, 200)}`);
  }

  return {
    output,
    executor: 'claude_cli',
    exitCode: 0,
  };
}
