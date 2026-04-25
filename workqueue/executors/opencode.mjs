/**
 * opencode.mjs — opencode CLI executor
 *
 * Runs `opencode run --print "<prompt>"` via an OpenAI-compatible endpoint.
 * Supports local ollama and remote vLLM (Boris pattern).
 *
 * Provider priority (first reachable wins):
 *   1. agentConfig.opencodeBaseUrl / OPENCODE_BASE_URL (explicit override)
 *   2. Local ollama at OLLAMA_BASE_URL (default: http://localhost:11434)
 *   3. Remote vLLM at agentConfig.vllmUrl / VLLM_BASE_URL
 */

import { execFile } from 'child_process';
import { promisify } from 'util';
import { request }   from 'http';

const execFileAsync = promisify(execFile);

/** Quick reachability probe — resolves true if the /v1/models endpoint responds. */
function isReachable(baseUrl, timeoutMs = 3000) {
  return new Promise(resolve => {
    try {
      const url = new URL('/v1/models', baseUrl);
      const req = request({ hostname: url.hostname, port: url.port || 80, path: url.pathname, method: 'GET' }, res => {
        resolve(res.statusCode < 500);
        res.resume();
      });
      req.setTimeout(timeoutMs, () => { req.destroy(); resolve(false); });
      req.on('error', () => resolve(false));
      req.end();
    } catch {
      resolve(false);
    }
  });
}

/**
 * @param {object} item          Workqueue item
 * @param {object} agentConfig   { opencodeBaseUrl, vllmUrl, model, timeoutMs }
 * @returns {Promise<object>}    { output, executor, exitCode, provider }
 */
export async function runOpencode(item, agentConfig = {}) {
  const repoPath  = item.repoPath || agentConfig.repoPath || process.cwd();
  const timeoutMs = agentConfig.timeoutMs || 300_000;
  const model     = agentConfig.opencodeModel
    || item.model
    || process.env.OPENCODE_MODEL
    || 'qwen2.5-coder:32b';

  // Determine provider base URL
  const explicitBase = agentConfig.opencodeBaseUrl || process.env.OPENCODE_BASE_URL;
  const ollamaBase   = process.env.OLLAMA_BASE_URL || 'http://localhost:11434';
  const vllmBase     = agentConfig.vllmUrl || process.env.VLLM_BASE_URL;

  let providerBase = null;
  let providerName = '';

  if (explicitBase) {
    providerBase = explicitBase;
    providerName = 'explicit';
  } else if (await isReachable(ollamaBase)) {
    providerBase = `${ollamaBase}/v1`;
    providerName = 'ollama';
  } else if (vllmBase && await isReachable(vllmBase)) {
    // Probe for served model name
    providerBase = `${vllmBase}/v1`;
    providerName = 'vllm';
  }

  if (!providerBase) {
    return {
      output:   'opencode: no reachable provider (ollama / vLLM)',
      executor: 'opencode',
      exitCode: 1,
      provider: 'none',
    };
  }

  const env = {
    ...process.env,
    OPENAI_BASE_URL: providerBase,
    OPENAI_API_KEY:  process.env.OPENAI_API_KEY || 'local',
  };

  const cliArgs = ['run', '--model', `openai/${model}`, '--print', item.description];

  try {
    const { stdout, stderr } = await execFileAsync('opencode', cliArgs, {
      cwd:     repoPath,
      timeout: timeoutMs,
      env,
    });
    return {
      output:   stdout || stderr,
      executor: 'opencode',
      exitCode: 0,
      provider: providerName,
    };
  } catch (err) {
    return {
      output:   (err.stdout || '') || err.message,
      executor: 'opencode',
      exitCode: err.code ?? 1,
      provider: providerName,
    };
  }
}
