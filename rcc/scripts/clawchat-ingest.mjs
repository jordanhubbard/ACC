#!/usr/bin/env node
/**
 * rcc/scripts/clawchat-ingest.mjs — ClawChat SSE → Milvus rcc_messages ingest
 *
 * Connects to ClawChat SSE stream, embeds each new message via local ollama
 * (nomic-embed-text, 768-dim), and upserts to Rocky's Milvus rcc_messages
 * collection. Enables agents to recall past conversation via /api/memory/recall.
 *
 * Reconnects automatically on disconnect (exponential backoff, max 60s).
 *
 * Env vars:
 *   CLAWCHAT_URL        default: https://chat.yourmom.photos
 *   CLAWCHAT_TOKEN      default: (from env)
 *   MILVUS_ADDRESS      default: 146.190.134.110:19530
 *   MILVUS_COLLECTION   default: rcc_messages
 *   OLLAMA_BASE_URL     default: http://localhost:11434
 *   OLLAMA_EMBED_MODEL  default: nomic-embed-text
 *   AGENT_NAME          default: natasha
 */

import { createHash } from 'crypto';
import { homedir }    from 'os';
import { readFileSync, existsSync } from 'fs';
import { join }       from 'path';

const CLAWCHAT_URL   = process.env.CLAWCHAT_URL        || 'https://chat.yourmom.photos';
const CLAWCHAT_TOKEN = process.env.CLAWCHAT_TOKEN       || loadEnvToken();
const MILVUS_ADDRESS = process.env.MILVUS_ADDRESS       || '146.190.134.110:19530';
const COLLECTION     = process.env.MILVUS_COLLECTION    || 'rcc_messages';
const OLLAMA_URL     = process.env.OLLAMA_BASE_URL      || 'http://localhost:11434';
const OLLAMA_MODEL   = process.env.OLLAMA_EMBED_MODEL   || 'nomic-embed-text';
const AGENT_NAME     = process.env.AGENT_NAME           || 'natasha';

function loadEnvToken() {
  const envFile = join(homedir(), '.rcc', '.env');
  if (!existsSync(envFile)) return '';
  try {
    const lines = readFileSync(envFile, 'utf8').split('\n');
    const line  = lines.find(l => l.startsWith('CLAWCHAT_TOKEN='));
    return line ? line.split('=')[1].trim() : '';
  } catch { return ''; }
}

function log(msg) {
  console.log(`[clawchat-ingest ${new Date().toISOString()}] ${msg}`);
}

// Milvus client (lazy)
let _milvus = null;
async function getMilvus() {
  if (!_milvus) {
    const { MilvusClient } = await import('@zilliz/milvus2-sdk-node');
    _milvus = new MilvusClient({ address: MILVUS_ADDRESS });
    log(`Milvus connected: ${MILVUS_ADDRESS}`);
  }
  return _milvus;
}

async function embed(text) {
  const resp = await fetch(`${OLLAMA_URL}/api/embeddings`, {
    method:  'POST',
    headers: { 'Content-Type': 'application/json' },
    body:    JSON.stringify({ model: OLLAMA_MODEL, prompt: text }),
  });
  if (!resp.ok) throw new Error(`ollama embed HTTP ${resp.status}`);
  const data = await resp.json();
  if (!data.embedding?.length) throw new Error('empty embedding');
  return data.embedding;
}

async function ingestMessage(msg) {
  // msg = { id, ts, from_agent, text, channel, ... }
  if (!msg.text || msg.text.trim().length < 5) return;
  const text   = `[${msg.channel}] ${msg.from_agent}: ${msg.text}`.slice(0, 1500);
  const id     = createHash('sha256').update(`${AGENT_NAME}:clawchat:${msg.id}`).digest('hex').slice(0, 32);
  try {
    const vector = await embed(text);
    const milvus = await getMilvus();
    await milvus.upsert({
      collection_name: COLLECTION,
      data: [{
        id,
        vector,
        text,
        agent:   msg.from_agent || AGENT_NAME,
        source:  `clawchat:${msg.channel}`,
        scope:   'fleet',
        ts:      msg.ts || Date.now(),
      }],
    });
    log(`✓ ingested msg ${msg.id} from ${msg.from_agent} in #${msg.channel}`);
  } catch (e) {
    log(`✗ msg ${msg.id}: ${e.message}`);
  }
}

// SSE stream consumer with reconnect
let backoffMs = 2000;

async function connect() {
  log(`Connecting to SSE stream: ${CLAWCHAT_URL}/api/stream`);
  let buffer = '';
  try {
    const headers = { Accept: 'text/event-stream' };
    if (CLAWCHAT_TOKEN) headers['Authorization'] = `Bearer ${CLAWCHAT_TOKEN}`;

    const resp = await fetch(`${CLAWCHAT_URL}/api/stream`, { headers });
    if (!resp.ok) {
      log(`SSE connect failed: HTTP ${resp.status}. Will retry.`);
      scheduleReconnect();
      return;
    }

    log('SSE stream connected.');
    backoffMs = 2000;  // reset on successful connect

    for await (const chunk of resp.body) {
      buffer += new TextDecoder().decode(chunk);
      const lines = buffer.split('\n');
      buffer = lines.pop();  // keep incomplete last line

      for (const line of lines) {
        if (!line.startsWith('data:')) continue;
        const raw = line.slice(5).trim();
        if (!raw || raw === 'ping') continue;
        try {
          const msg = JSON.parse(raw);
          if (msg.type === 'message' || msg.id) {
            await ingestMessage(msg.data || msg);
          }
        } catch { /* non-JSON SSE event, skip */ }
      }
    }

    log('SSE stream ended. Reconnecting...');
  } catch (e) {
    log(`SSE error: ${e.message}. Reconnecting in ${backoffMs}ms...`);
  }
  scheduleReconnect();
}

function scheduleReconnect() {
  setTimeout(() => connect(), backoffMs);
  backoffMs = Math.min(backoffMs * 2, 60000);
}

// Warm up Milvus on start
getMilvus().catch(e => log(`WARN milvus warmup: ${e.message}`));
connect();
