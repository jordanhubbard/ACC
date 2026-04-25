/**
 * codex.mjs — Codex CLI executor (OpenAI API + local vLLM routing)
 *
 * Runs `codex --approval-mode full-auto -q "<prompt>"` as a subprocess.
 *
 * Two modes:
 *   codex_cli  — routes to OpenAI (requires OPENAI_API_KEY)
 *   codex_vllm — routes to a local vLLM endpoint (no key needed)
 *
 * Auth: OPENAI_API_KEY env var, or 'none' for local vLLM.
 */

import { execFile } from 'child_process';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

/**
 * @param {object} item                Workqueue item
 * @param {object} opts
 * @param {string|null} opts.baseUrl   Override OPENAI_BASE_URL (null = use OpenAI default)
 * @param {string} [opts.model]        Model name (e.g. 'nemotron', 'gpt-4o')
 * @param {number} [opts.timeoutMs]    Subprocess timeout in ms (default: 300s)
 * @returns {Promise<object>}          { output, executor, exitCode }
 */
export async function runCodex(item, { baseUrl = null, model, timeoutMs = 300_000 } = {}) {
  const repoPath = item.repoPath || process.cwd();

  const env = { ...process.env };
  if (baseUrl) {
    env.OPENAI_BASE_URL = baseUrl;
    env.OPENAI_API_KEY  = env.OPENAI_API_KEY || 'none';
  }

  const cliArgs = ['--approval-mode', 'full-auto', '-q', item.description];
  if (model) cliArgs.splice(0, 0, '--model', model);

  try {
    const { stdout, stderr } = await execFileAsync('codex', cliArgs, {
      cwd:     repoPath,
      timeout: timeoutMs,
      env,
    });
    return {
      output:   stdout || stderr,
      executor: baseUrl ? 'codex_vllm' : 'codex_cli',
      exitCode: 0,
    };
  } catch (err) {
    return {
      output:   (err.stdout || '') || err.message,
      executor: baseUrl ? 'codex_vllm' : 'codex_cli',
      exitCode: err.code ?? 1,
    };
  }
}
