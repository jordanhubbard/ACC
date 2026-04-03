#!/usr/bin/env node
/**
 * rcc/scripts/clawchat-ingest-watcher.mjs — ClawChat SSE → Milvus rcc_messages
 *
 * Connects to ClawChat SSE stream at CLAWCHAT_URL/api/stream.
 * On new message event: embeds via local ollama (nomic-embed-text, 768-dim)
 * and upserts into Milvus rcc_messages collection.
 *
 * Enables semantic recall of agent conversations fleet-wide via /api/memory/recall.
 * Complement to memory-ingest-watcher.mjs (handles MEMORY.md + daily notes).
 *
 * Env vars:
 *   CLAWCHAT_URL        default: http://146.190.134.110:8793
 *   CLAWCHAT_TOKEN      default: (empty — try unauthenticated first)
 *   AGENT_NAME          default: natasha
 *   MILVUS_ADDRESS      default: 146.190.134.110:19530
 *   MILVUS_COLLECTION   default: rcc_messages
 *   OLLAMA_BASE_URL     default: http://localhost:11434
 *   OLLAMA_EMBED_MODEL  default: nomic-embed-text
 *   RECONNECT_DELAY_MS  default: 5000
 *
 * wq-API-1775194767133 — implemented 2026-04-03 by natasha
 */

import { createHash } from 'crypto';

const CLAWCHAT_URL   = process.env.CLAWCHAT_URL       || 'http://146.190.134.110:8793';
const CLAWCHAT_TOKEN = process.env.CLAWCHAT_TOKEN      || '';
const AGENT_NAME     = process.env.AGENT_NAME          || 'natasha';
const MILVUS_ADDRESS = process.env.MILVUS_ADDRESS      || '146.190.134.110:19530';
const COLLECTION     = process.env.MILVUS_COLLECTION   || 'rcc_messages';
const OLLAMA_URL     = process.env.OLLAMA_BASE_URL     || 'http://localhost:11434';
const OLLAMA_MODEL   = process.env.OLLAMA_EMBED_MODEL  || 'nomic-embed-text';
const RECONNECT_MS   = parseInt(process.env.RECONNECT_DELAY_MS || '5000', 10);

function log(msg) {
  console.log(`[clawchat-watcher ${new Date().toISOString()}] ${msg}`);
}

// Milvus client (lazy)
let _milvus = null;
async function getMilvus() {
  if (!_milvus) {
    const { MilvusClient } = await import('@zilliz/milvus2-sdk-node');
    _milvus = new MilvusClient({ address: MILVUS_ADDRESS });
    log(`Milvus client connected: ${MILVUS_ADDRESS}`);
  }
  return _milvus;
}

// Embed via local ollama
async function embed(text) {
  const resp = await fetch(`${OLLAMA_URL}/api/embeddings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ model: OLLAMA_MODEL, prompt: text }),
  });
  if (!resp.ok) throw new Error(`ollama embed HTTP ${resp.status}`);
  const data = await resp.json();
  if (!data.embedding?.length) throw new Error('empty embedding from ollama');
  return data.embedding;
}

// Upsert a message into Milvus rcc_messages
async function upsertMessage(msg) {
  // Build content string: "channel/author: text"
  const content = [
    msg.channel_name ? `#${msg.channel_name}` : null,
    msg.username || msg.author || msg.user_id || 'unknown',
    msg.content || msg.message || msg.text || '',
  ].filter(Boolean).join(' | ');

  if (!content || content.length < 5) return;

  const id = createHash('sha256')
    .update(`clawchat:${msg.id || msg.message_id || content.slice(0, 128)}`)
    .digest('hex').slice(0, 32);

  const vector = await embed(content);

  const client = await getMilvus();
  await client.upsert({
    collection_name: COLLECTION,
    data: [{
      id,
      vector,
      content,
      agent: msg.username || msg.author || 'unknown',
      source: 'clawchat',
      scope: 'fleet',
      ts: msg.created_at || msg.ts || new Date().toISOString(),
    }],
  });

  log(`Upserted message id=${id} content="${content.slice(0, 80)}..."`);
}

// Connect to ClawChat SSE stream and process events
async function connectSSE() {
  const url = `${CLAWCHAT_URL}/api/stream`;
  const headers = {
    Accept: 'text/event-stream',
    'Cache-Control': 'no-cache',
  };
  if (CLAWCHAT_TOKEN) {
    headers['Authorization'] = `Bearer ${CLAWCHAT_TOKEN}`;
  }

  log(`Connecting to ClawChat SSE: ${url}`);

  let resp;
  try {
    resp = await fetch(url, { headers, signal: AbortSignal.timeout(30000) });
  } catch (err) {
    log(`Connection failed: ${err.message} — will retry in ${RECONNECT_MS}ms`);
    return false;
  }

  if (!resp.ok) {
    log(`SSE HTTP ${resp.status} — will retry in ${RECONNECT_MS}ms`);
    return false;
  }

  log('SSE stream connected. Listening for messages...');

  let buffer = '';
  const decoder = new TextDecoder();

  try {
    for await (const chunk of resp.body) {
      buffer += decoder.decode(chunk, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop(); // keep incomplete line

      let eventData = null;
      for (const line of lines) {
        if (line.startsWith('data: ')) {
          try {
            eventData = JSON.parse(line.slice(6));
          } catch {
            // not JSON — skip
          }
        } else if (line === '' && eventData) {
          // End of event — process it
          await handleEvent(eventData);
          eventData = null;
        }
      }
    }
  } catch (err) {
    log(`Stream error: ${err.message}`);
    return false;
  }

  log('SSE stream ended — will reconnect');
  return true;
}

async function handleEvent(event) {
  // ClawChat emits various event types
  const type = event.type || event.event;

  if (type === 'message' || type === 'new_message' || type === 'chat_message') {
    const msg = event.data || event.message || event;
    try {
      await upsertMessage(msg);
    } catch (err) {
      log(`Error ingesting message: ${err.message}`);
    }
    return;
  }

  // Also handle direct message objects (no wrapper)
  if (event.content || event.message || event.text) {
    try {
      await upsertMessage(event);
    } catch (err) {
      log(`Error ingesting direct msg: ${err.message}`);
    }
  }
}

// Main loop with reconnection
async function main() {
  log(`Starting ClawChat→Milvus ingest watcher (agent=${AGENT_NAME})`);
  log(`ClawChat: ${CLAWCHAT_URL} | Milvus: ${MILVUS_ADDRESS}/${COLLECTION}`);

  while (true) {
    await connectSSE();
    log(`Reconnecting in ${RECONNECT_MS}ms...`);
    await new Promise(r => setTimeout(r, RECONNECT_MS));
  }
}

main().catch(err => {
  log(`Fatal: ${err.message}`);
  process.exit(1);
});
