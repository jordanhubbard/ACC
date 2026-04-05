/**
 * rcc/vector/index.mjs — Qdrant Vector Search Integration
 *
 * Replaces Milvus. Uses Qdrant HTTP API (no SDK) via fetch.
 *
 * Provides semantic search over:
 *   - Lessons learned (rcc_lessons → agent_memories filtered by type=lesson)
 *   - Work queue items (rcc_queue → rcc_queue_dedup)
 *   - Agent memory / context snippets (rcc_memory → agent_memories)
 *   - Channel memory (rcc_channel_memory → agent_memories filtered by type=channel)
 *
 * Embedding model: NVIDIA NIM text-embedding-3-large (3072-dim)
 * Vector DB: Qdrant at QDRANT_FLEET_URL (default: http://localhost:6333)
 *
 * Collections:
 *   - agent_memories    — agent memory + lessons + channel memory
 *   - rcc_queue_dedup   — work queue items for dedup
 *   - slack_history     — Slack message history
 *
 * Usage:
 *   import { vectorSearch, vectorUpsert, vectorDelete, ensureCollections } from '../vector/index.mjs';
 *   await ensureCollections();
 *   await vectorUpsert('rcc_lessons', lesson.id, lesson.symptom + ' ' + lesson.fix, { ...meta });
 *   const hits = await vectorSearch('rcc_lessons', 'how to handle rate limits', 5);
 */

import { createHash } from 'crypto';

// ── Config ──────────────────────────────────────────────────────────────────
const QDRANT_URL      = process.env.QDRANT_FLEET_URL || 'http://localhost:6333';
const QDRANT_KEY      = process.env.QDRANT_FLEET_KEY || process.env.QDRANT_API_KEY || '';
const EMBED_API_URL   = process.env.NVIDIA_EMBED_URL || 'http://localhost:8090/v1/embeddings';
const EMBED_API_KEY   = process.env.TOKENHUB_AGENT_KEY || process.env.NVIDIA_API_KEY || '';
const EMBED_MODEL     = process.env.EMBED_MODEL || 'azure/openai/text-embedding-3-large';
const EMBED_DIM       = parseInt(process.env.EMBED_DIM || '3072', 10);
const AGENT_NAME      = process.env.AGENT_NAME || 'rocky';

// ── Collection name mapping ──────────────────────────────────────────────────
// Legacy collection names → Qdrant collection + optional payload filter
const COLLECTION_MAP = {
  rcc_lessons:        { qdrant: 'agent_memories', typeFilter: 'lesson' },
  rcc_queue:          { qdrant: 'rcc_queue_dedup', typeFilter: null },
  rcc_memory:         { qdrant: 'agent_memories', typeFilter: 'memory' },
  rcc_memory_sparky:  { qdrant: 'agent_memories', typeFilter: 'memory_sparky' },
  rcc_channel_memory: { qdrant: 'agent_memories', typeFilter: 'channel' },
  // pass-through for native collection names
  agent_memories:     { qdrant: 'agent_memories', typeFilter: null },
  rcc_queue_dedup:    { qdrant: 'rcc_queue_dedup', typeFilter: null },
  slack_history:      { qdrant: 'slack_history', typeFilter: null },
};

// ── Qdrant HTTP helpers ─────────────────────────────────────────────────────
function qdrantHeaders() {
  const h = { 'Content-Type': 'application/json' };
  if (QDRANT_KEY) h['api-key'] = QDRANT_KEY;
  return h;
}

async function qdrantRequest(method, path, body) {
  const url = `${QDRANT_URL}${path}`;
  const res = await fetch(url, {
    method,
    headers: qdrantHeaders(),
    body: body != null ? JSON.stringify(body) : undefined,
  });
  const json = await res.json();
  if (json.status !== 'ok' && !res.ok) {
    throw new Error(`Qdrant ${method} ${path} failed: ${JSON.stringify(json)}`);
  }
  return json;
}

// ── Embedding ───────────────────────────────────────────────────────────────
async function embed(text) {
  const res = await fetch(EMBED_API_URL, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${EMBED_API_KEY}`,
    },
    body: JSON.stringify({ model: EMBED_MODEL, input: [text], encoding_format: 'float' }),
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Embed API error ${res.status}: ${body}`);
  }
  const data = await res.json();
  return data.data[0].embedding;
}

// ── Collection setup ────────────────────────────────────────────────────────
const COLLECTION_SPECS = {
  agent_memories: {
    vectors: { size: EMBED_DIM, distance: 'Cosine' },
    payload_schema: {
      agent: { data_type: 'keyword' },
      type:  { data_type: 'keyword' },
      source: { data_type: 'keyword' },
    },
  },
  rcc_queue_dedup: {
    vectors: { size: EMBED_DIM, distance: 'Cosine' },
    payload_schema: {
      item_id: { data_type: 'keyword' },
      assignee: { data_type: 'keyword' },
    },
  },
  slack_history: {
    vectors: { size: EMBED_DIM, distance: 'Cosine' },
    payload_schema: {
      channel: { data_type: 'keyword' },
      user: { data_type: 'keyword' },
    },
  },
};

export async function ensureCollections() {
  const existing = await qdrantRequest('GET', '/collections');
  const names = (existing.result?.collections || []).map(c => c.name);

  for (const [name, spec] of Object.entries(COLLECTION_SPECS)) {
    if (!names.includes(name)) {
      console.log(`[vector] Creating Qdrant collection: ${name}`);
      await qdrantRequest('PUT', `/collections/${name}`, spec);
    }
  }
}

// ── Stable numeric ID from string key ───────────────────────────────────────
function stableId(key) {
  const hex = createHash('sha256').update(key).digest('hex').slice(0, 15);
  return parseInt(hex, 16);
}

// ── Core operations ──────────────────────────────────────────────────────────

/**
 * Upsert a vector into the named collection.
 * @param {string} collectionName - legacy or native collection name
 * @param {string} id - unique string key
 * @param {string} text - text to embed
 * @param {object} payload - extra metadata
 */
export async function vectorUpsert(collectionName, id, text, payload = {}) {
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) throw new Error(`Unknown collection: ${collectionName}`);

  const vector = await embed(text);
  const pointId = stableId(id);

  const point = {
    id: pointId,
    vector,
    payload: {
      ...payload,
      original_id: id,
      agent: payload.agent || AGENT_NAME,
      type: mapping.typeFilter || payload.type || collectionName,
      text: text.slice(0, 2000), // store truncated text for retrieval
      updated_at: new Date().toISOString(),
    },
  };

  await qdrantRequest('PUT', `/collections/${mapping.qdrant}/points`, {
    points: [point],
  });
}

/**
 * Batch upsert multiple records.
 */
export async function vectorUpsertBatch(collectionName, records) {
  // records: [{id, text, payload}]
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) throw new Error(`Unknown collection: ${collectionName}`);

  const points = [];
  for (const r of records) {
    const vector = await embed(r.text);
    points.push({
      id: stableId(r.id),
      vector,
      payload: {
        ...r.payload,
        original_id: r.id,
        agent: r.payload?.agent || AGENT_NAME,
        type: mapping.typeFilter || r.payload?.type || collectionName,
        text: r.text.slice(0, 2000),
        updated_at: new Date().toISOString(),
      },
    });
  }

  await qdrantRequest('PUT', `/collections/${mapping.qdrant}/points`, { points });
  return points.length;
}

/**
 * Semantic search.
 * @param {string} collectionName
 * @param {string} queryText
 * @param {number} topK
 * @returns {Array<{id, score, payload}>}
 */
export async function vectorSearch(collectionName, queryText, topK = 5) {
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) throw new Error(`Unknown collection: ${collectionName}`);

  const vector = await embed(queryText);

  const body = {
    vector,
    limit: topK,
    with_payload: true,
  };

  // If this logical collection maps to a type subset, add filter
  if (mapping.typeFilter) {
    body.filter = {
      must: [{ key: 'type', match: { value: mapping.typeFilter } }],
    };
  }

  const result = await qdrantRequest('POST', `/collections/${mapping.qdrant}/points/search`, body);
  return (result.result || []).map(hit => ({
    id: hit.payload?.original_id || String(hit.id),
    score: hit.score,
    payload: hit.payload,
  }));
}

/**
 * Delete a point by string key.
 */
export async function vectorDelete(collectionName, id) {
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) throw new Error(`Unknown collection: ${collectionName}`);

  const pointId = stableId(id);
  await qdrantRequest('POST', `/collections/${mapping.qdrant}/points/delete`, {
    points: [pointId],
  });
}

/**
 * Check if a key already exists (for dedup).
 */
export async function vectorExists(collectionName, id) {
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) return false;

  const pointId = stableId(id);
  try {
    const result = await qdrantRequest('GET', `/collections/${mapping.qdrant}/points/${pointId}`);
    return !!(result.result);
  } catch {
    return false;
  }
}

/**
 * Get collection stats.
 */
export async function collectionStats(collectionName) {
  const mapping = COLLECTION_MAP[collectionName];
  if (!mapping) throw new Error(`Unknown collection: ${collectionName}`);

  const result = await qdrantRequest('GET', `/collections/${mapping.qdrant}`);
  return {
    collection: mapping.qdrant,
    points_count: result.result?.points_count || 0,
    status: result.result?.status,
  };
}

// ── Compat shim for code that checks if Milvus is available ──────────────────
export const isMilvusAvailable = false;
export const isQdrantAvailable = true;
