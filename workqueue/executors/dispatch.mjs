#!/usr/bin/env node
/**
 * dispatch.mjs — CCC task executor router
 *
 * Selects the right coding backend for a workqueue item based on:
 *   1. item.required_executors  — hard filter (item MUST run on one of these)
 *   2. item.preferred_executor  — soft hint within the allowed set
 *   3. agentConfig.defaultExecutor — agent-level default
 *   4. 'claude_cli'             — global fallback
 *
 * Usage (CLI):
 *   node dispatch.mjs --item '<json>' [--config '<json>']
 *
 * Usage (ESM import):
 *   import { dispatch } from './dispatch.mjs';
 *   const result = await dispatch(item, agentConfig);
 *
 * Result shape:
 *   { output, executor, exitCode, sessionId?, cost? }
 */

import { runClaudeSDK } from './claude-sdk.mjs';
import { runClaudeCLI } from './claude-cli.mjs';
import { runCodex }     from './codex.mjs';
import { runOpencode }  from './opencode.mjs';
import { runCursor }    from './cursor.mjs';

/** All executor type strings — must match CAPABILITIES.md */
export const EXECUTOR_TYPES = [
  'claude_cli',
  'claude_sdk',
  'codex_cli',
  'codex_vllm',
  'cursor_cli',
  'opencode',
  'inference_key',
];

/**
 * @param {object} item          Workqueue item (from /api/workqueue)
 * @param {object} agentConfig   Agent config (vllmUrl, repoPath, model, defaultExecutor, …)
 * @returns {Promise<object>}    Result: { output, executor, exitCode, sessionId?, cost? }
 */
export async function dispatch(item, agentConfig = {}) {
  // Determine executor to use
  let exec = agentConfig.defaultExecutor || 'claude_cli';

  // Soft hint from item
  if (item.preferred_executor) {
    exec = item.preferred_executor;
  }

  // Hard filter: if required_executors is set, the chosen exec must be in the list.
  // If it isn't, pick the first required executor instead (agent shouldn't have
  // claimed this item unless it supports at least one required executor).
  if (Array.isArray(item.required_executors) && item.required_executors.length > 0) {
    if (!item.required_executors.includes(exec)) {
      exec = item.required_executors[0];
    }
  }

  switch (exec) {
    case 'claude_sdk':
      return runClaudeSDK(item, agentConfig);

    case 'claude_cli':
      return runClaudeCLI(item, agentConfig);

    case 'codex_cli':
      return runCodex(item, { baseUrl: null });

    case 'codex_vllm':
      return runCodex(item, {
        baseUrl: agentConfig.vllmUrl || process.env.VLLM_BASE_URL || 'http://localhost:18081/v1',
      });

    case 'cursor_cli':
      return runCursor(item, agentConfig);

    case 'opencode':
      return runOpencode(item, agentConfig);

    case 'inference_key':
      // inference_key tasks are handled by the brain/API layer, not a coding agent.
      // If dispatch is called with inference_key, treat it as a no-op coding task.
      return {
        output:   `inference_key task "${item.title}" does not require a coding executor.`,
        executor: 'inference_key',
        exitCode: 0,
      };

    default:
      throw new Error(`Unknown executor: ${exec}. Valid types: ${EXECUTOR_TYPES.join(', ')}`);
  }
}

// ── CLI entry point ────────────────────────────────────────────────────────────
if (process.argv[1] && new URL(import.meta.url).pathname === process.argv[1]) {
  const args = process.argv.slice(2);
  let itemJson = null;
  let configJson = '{}';

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--item'   && args[i + 1]) { itemJson   = args[++i]; }
    if (args[i] === '--config' && args[i + 1]) { configJson = args[++i]; }
  }

  if (!itemJson) {
    console.error('Usage: node dispatch.mjs --item \'{"id":"...","description":"...",...}\' [--config \'{...}\']');
    process.exit(2);
  }

  try {
    const item   = JSON.parse(itemJson);
    const config = JSON.parse(configJson);
    const result = await dispatch(item, config);
    process.stdout.write(result.output + '\n');
    process.exit(result.exitCode ?? 0);
  } catch (err) {
    console.error(`dispatch error: ${err.message}`);
    process.exit(1);
  }
}
