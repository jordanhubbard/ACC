#!/usr/bin/env node
/**
 * ollama-watchdog.mjs — Ollama model health watchdog for sparky
 *
 * Periodically health-checks ollama models (qwen2.5-coder:32b by default).
 * If response time > RESPONSE_TIMEOUT_MS or output is malformed, restarts the model.
 * Reports ollama_status (ok|degraded|restarting) to the RCC heartbeat payload.
 *
 * Usage:
 *   node ollama-watchdog.mjs
 *
 * Env vars:
 *   OLLAMA_BASE_URL      default: http://localhost:11434
 *   OLLAMA_MODELS        comma-separated, default: qwen2.5-coder:32b
 *   CHECK_INTERVAL_MS    default: 900000 (15 min)
 *   RESPONSE_TIMEOUT_MS  default: 30000 (30s)
 *   RCC_API              default: https://api.yourmom.photos
 *   RCC_AUTH_TOKEN       default: rcc-agent-rocky-20maaghccmbmnby63so
 *   AGENT_NAME           default: natasha
 */

import { execSync, spawn } from 'child_process';
import { writeFileSync, readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

const OLLAMA_BASE_URL     = process.env.OLLAMA_BASE_URL     || 'http://localhost:11434';
const OLLAMA_MODELS       = (process.env.OLLAMA_MODELS      || 'qwen2.5-coder:32b').split(',').map(s => s.trim());
const CHECK_INTERVAL_MS   = parseInt(process.env.CHECK_INTERVAL_MS   || '900000', 10);  // 15 min
const RESPONSE_TIMEOUT_MS = parseInt(process.env.RESPONSE_TIMEOUT_MS || '30000', 10);   // 30s
const RCC_API             = process.env.RCC_API             || 'https://api.yourmom.photos';
const RCC_AUTH_TOKEN      = process.env.RCC_AUTH_TOKEN      || 'rcc-agent-rocky-20maaghccmbmnby63so';
const AGENT_NAME          = process.env.AGENT_NAME          || 'natasha';
const STATUS_FILE         = resolve(process.env.STATUS_FILE || '/home/jkh/.rcc/ollama-status.json');
const TEST_PROMPT         = 'Say "ok" in one word.';

let ollamaStatus = {};

function log(msg) {
  console.log(`[ollama-watchdog ${new Date().toISOString()}] ${msg}`);
}

/** Save status to disk (picked up by heartbeat) */
function saveStatus() {
  try {
    writeFileSync(STATUS_FILE, JSON.stringify({ ...ollamaStatus, updatedAt: new Date().toISOString() }, null, 2));
  } catch (e) {
    log(`WARN: could not write status file: ${e.message}`);
  }
}

/** POST ollama_status to RCC heartbeat */
async function reportToRCC(model, status) {
  try {
    const res = await fetch(`${RCC_API}/api/heartbeat/${AGENT_NAME}`, {
      method: 'POST',
      headers: { 'Authorization': `Bearer ${RCC_AUTH_TOKEN}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'online',
        host: 'sparky',
        ts: new Date().toISOString(),
        ollama_status: ollamaStatus,
      }),
    });
    if (!res.ok) log(`WARN: RCC heartbeat responded ${res.status}`);
  } catch (e) {
    log(`WARN: RCC heartbeat failed: ${e.message}`);
  }
}

/** Attempt to restart an ollama model */
async function restartModel(model) {
  log(`Restarting model: ${model}`);
  ollamaStatus[model] = 'restarting';
  saveStatus();
  try {
    // Stop the model
    execSync(`ollama stop ${model}`, { timeout: 15000, stdio: 'pipe' });
    log(`Stopped ${model}`);
  } catch (e) {
    log(`WARN: ollama stop failed (may not have been running): ${e.message}`);
  }
  // Brief pause
  await new Promise(r => setTimeout(r, 3000));
  // Pull/warm up the model (non-blocking — just load it)
  try {
    execSync(`ollama run ${model} "" 2>/dev/null || true`, { timeout: 60000, stdio: 'pipe' });
    log(`Warmed up ${model}`);
  } catch (e) {
    log(`WARN: ollama run warmup failed: ${e.message}`);
  }
  ollamaStatus[model] = 'ok';
  saveStatus();
  await reportToRCC(model, 'ok');
  log(`Model ${model} restarted successfully`);
}

/** Health-check a single model */
async function checkModel(model) {
  log(`Checking model: ${model}`);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), RESPONSE_TIMEOUT_MS);
  const t0 = Date.now();

  try {
    const res = await fetch(`${OLLAMA_BASE_URL}/api/generate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model, prompt: TEST_PROMPT, stream: false }),
      signal: controller.signal,
    });
    clearTimeout(timeout);

    if (!res.ok) {
      log(`ERROR: model ${model} returned HTTP ${res.status}`);
      ollamaStatus[model] = 'degraded';
      saveStatus();
      await restartModel(model);
      return;
    }

    const data = await res.json();
    const elapsed = Date.now() - t0;
    const output = (data.response || '').trim();

    if (!output || output.length === 0) {
      log(`DEGRADED: model ${model} returned empty response in ${elapsed}ms`);
      ollamaStatus[model] = 'degraded';
      saveStatus();
      await restartModel(model);
      return;
    }

    log(`OK: model ${model} responded in ${elapsed}ms — "${output.slice(0, 60)}"`);
    ollamaStatus[model] = 'ok';
    saveStatus();
    await reportToRCC(model, 'ok');

  } catch (e) {
    clearTimeout(timeout);
    const elapsed = Date.now() - t0;
    if (e.name === 'AbortError') {
      log(`TIMEOUT: model ${model} did not respond within ${RESPONSE_TIMEOUT_MS}ms`);
    } else {
      log(`ERROR: model ${model} check failed after ${elapsed}ms: ${e.message}`);
    }
    ollamaStatus[model] = 'degraded';
    saveStatus();
    await restartModel(model);
  }
}

/** Run one full check cycle */
async function checkAll() {
  log(`Starting check cycle for models: ${OLLAMA_MODELS.join(', ')}`);
  for (const model of OLLAMA_MODELS) {
    await checkModel(model);
  }
  log('Check cycle complete');
}

// ── Main loop ──────────────────────────────────────────────────────────────
log(`Ollama watchdog starting. Models: ${OLLAMA_MODELS.join(', ')}, interval: ${CHECK_INTERVAL_MS}ms`);

// Load existing status if present
if (existsSync(STATUS_FILE)) {
  try {
    const prev = JSON.parse(readFileSync(STATUS_FILE, 'utf8'));
    ollamaStatus = prev;
    log(`Loaded previous status: ${JSON.stringify(ollamaStatus)}`);
  } catch (_) {}
}

// Run immediately, then on interval
checkAll().catch(e => log(`ERROR in check cycle: ${e.message}`));
setInterval(() => checkAll().catch(e => log(`ERROR in check cycle: ${e.message}`)), CHECK_INTERVAL_MS);
