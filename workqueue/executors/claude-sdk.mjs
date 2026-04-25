/**
 * claude-sdk.mjs — Claude Code SDK executor
 *
 * Uses @anthropic-ai/claude-code `query()` to run tasks programmatically.
 * Captures structured output, token cost, and session ID.
 *
 * Auth: reads ~/.claude/credentials (same session as `claude login`).
 * Nodes without a valid claude login session will fail here — use claude-cli.mjs instead.
 */

import { query } from '@anthropic-ai/claude-code';

/**
 * @param {object} item          Workqueue item
 * @param {object} agentConfig   { repoPath, model, maxTurns }
 * @returns {Promise<object>}    { output, executor, exitCode, sessionId, cost }
 */
export async function runClaudeSDK(item, agentConfig = {}) {
  const repoPath = item.repoPath || agentConfig.repoPath || process.cwd();
  const model    = agentConfig.model || process.env.CLAUDE_MODEL || undefined;
  const maxTurns = agentConfig.maxTurns || 20;

  const messages = [];
  const cost = { input: 0, output: 0, cache_read: 0 };

  for await (const msg of query({
    prompt: item.description,
    options: {
      cwd:            repoPath,
      permissionMode: 'bypassPermissions',
      maxTurns,
      ...(model ? { model } : {}),
    },
  })) {
    messages.push(msg);

    if (msg.type === 'result' && msg.usage) {
      cost.input      += msg.usage.input_tokens      ?? 0;
      cost.output     += msg.usage.output_tokens     ?? 0;
      cost.cache_read += msg.usage.cache_read_tokens ?? 0;
    }
  }

  const result = messages.find(m => m.type === 'result');

  return {
    output:    result?.result ?? '',
    executor:  'claude_sdk',
    exitCode:  result?.is_error ? 1 : 0,
    sessionId: result?.session_id ?? null,
    cost,
  };
}
