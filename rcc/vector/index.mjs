/**
 * rcc/vector/index.mjs — Milvus embedding + retrieval module
 *
 * Embed model: azure/openai/text-embedding-3-large (3072 dims)
 * Via NVIDIA inference gateway
 */

import { MilvusClient, DataType } from '@zilliz/milvus2-sdk-node';

// ── Config ──────────────────────────────────────────────────────────────────
const MILVUS_ADDRESS   = process.env.MILVUS_ADDRESS   || 'localhost:19530';
const NVIDIA_BASE_URL  = process.env.NVIDIA_BASE_URL  || 'https://inference-api.nvidia.com/v1';
const NVIDIA_API_KEY   = process.env.NVIDIA_API_KEY   || 'sk-4xpmLNElbTNhe20Or4eImQ';
const EMBED_MODEL      = process.env.EMBED_MODEL      || 'azure/openai/text-embedding-3-large';
const EMBED_DIMS       = 3072;

const COLLECTIONS = ['rcc_lessons', 'rcc_queue', 'rcc_memory', 'rcc_messages'];

// ── BigInt-safe JSON serializer ──────────────────────────────────────────────
function safeStringify(obj) {
  return JSON.stringify(obj, (_, v) => typeof v === 'bigint' ? v.toString() : v);
}

// ── Milvus client (lazy) ─────────────────────────────────────────────────────
let _client = null;
function getClient() {
  if (!_client) {
    _client = new MilvusClient({ address: MILVUS_ADDRESS });
  }
  return _client;
}

// ── Embedding ────────────────────────────────────────────────────────────────

/**
 * Embed text via NVIDIA gateway.
 * @param {string} text
 * @returns {Promise<Float32Array>} 3072-dim vector
 */
export async function embed(text) {
  const resp = await fetch(`${NVIDIA_BASE_URL}/embeddings`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${NVIDIA_API_KEY}`,
    },
    body: JSON.stringify({
      model: EMBED_MODEL,
      input: text,
    }),
  });

  if (!resp.ok) {
    const err = await resp.text();
    throw new Error(`Embed failed (${resp.status}): ${err}`);
  }

  const data = await resp.json();
  return new Float32Array(data.data[0].embedding);
}

// ── Collection schema ────────────────────────────────────────────────────────

function collectionSchema(name) {
  return {
    collection_name: name,
    fields: [
      {
        name: 'id',
        data_type: DataType.VarChar,
        max_length: 256,
        is_primary_key: true,
        auto_id: false,
      },
      {
        name: 'text',
        data_type: DataType.VarChar,
        max_length: 65535,
      },
      {
        name: 'metadata',
        data_type: DataType.VarChar,
        max_length: 65535,
      },
      {
        name: 'embedding',
        data_type: DataType.FloatVector,
        dim: EMBED_DIMS,
      },
      {
        name: 'created_at',
        data_type: DataType.Int64,
      },
    ],
  };
}

const HNSW_INDEX = {
  index_type: 'HNSW',
  metric_type: 'COSINE',
  params: { M: 16, efConstruction: 200 },
};

// ── ensureCollections ─────────────────────────────────────────────────────────

/**
 * Ensure all collections exist with correct schema + HNSW index.
 */
export async function ensureCollections() {
  const client = getClient();
  for (const name of COLLECTIONS) {
    const exists = await client.hasCollection({ collection_name: name });
    if (!exists.value) {
      await client.createCollection(collectionSchema(name));
      await client.createIndex({
        collection_name: name,
        field_name: 'embedding',
        ...HNSW_INDEX,
      });
      console.log(`[vector] Created collection: ${name}`);
    }
    // Load collection into memory for search
    await client.loadCollectionSync({ collection_name: name });
  }
}

// ── upsert ────────────────────────────────────────────────────────────────────

/**
 * Upsert a document into a collection.
 * @param {string} collection
 * @param {{ id: string, text: string, metadata: object }} doc
 */
export async function upsert(collection, { id, text, metadata }) {
  const client = getClient();
  const vector = await embed(text);

  const data = {
    id,
    text,
    metadata: typeof metadata === 'string' ? metadata : safeStringify(metadata),
    embedding: Array.from(vector),
    created_at: Date.now(),
  };

  await client.upsert({ collection_name: collection, data: [data] });
}

// ── search ────────────────────────────────────────────────────────────────────

/**
 * Search a collection.
 * @param {string} collection
 * @param {string} queryText
 * @param {{ k?: number, filter?: string }} opts
 * @returns {Promise<Array<{id, score, text, metadata}>>}
 */
export async function search(collection, queryText, { k = 10, filter } = {}) {
  const client = getClient();
  const vector = await embed(queryText);

  const params = {
    collection_name: collection,
    vectors: [Array.from(vector)],
    vector_type: DataType.FloatVector,
    limit: k,
    output_fields: ['id', 'text', 'metadata'],
    search_params: { anns_field: 'embedding', topk: k, metric_type: 'COSINE', params: JSON.stringify({ ef: 64 }) },
  };
  if (filter) params.filter = filter;

  const result = await client.search(params);
  const hits = result.results || [];

  return hits.map(hit => ({
    id: hit.id,
    score: hit.score,
    text: hit.text,
    metadata: (() => { try { return JSON.parse(hit.metadata); } catch { return hit.metadata; } })(),
  }));
}

// ── searchAll ─────────────────────────────────────────────────────────────────

/**
 * Search ALL collections, merge and sort results by score.
 * @param {string} queryText
 * @param {{ k?: number }} opts
 * @returns {Promise<Array<{collection, id, score, text, metadata}>>}
 */
export async function searchAll(queryText, { k = 10 } = {}) {
  // Embed once, then fan out
  const vector = await embed(queryText);
  const client = getClient();

  const searches = COLLECTIONS.map(async collection => {
    try {
      const params = {
        collection_name: collection,
        vectors: [Array.from(vector)],
        vector_type: DataType.FloatVector,
        limit: k,
        output_fields: ['id', 'text', 'metadata'],
        search_params: { anns_field: 'embedding', topk: k, metric_type: 'COSINE', params: JSON.stringify({ ef: 64 }) },
      };
      const result = await client.search(params);
      return (result.results || []).map(hit => ({
        collection,
        id: hit.id,
        score: hit.score,
        text: hit.text,
        metadata: (() => { try { return JSON.parse(hit.metadata); } catch { return hit.metadata; } })(),
      }));
    } catch (err) {
      console.warn(`[vector] searchAll failed for ${collection}: ${err.message}`);
      return [];
    }
  });

  const all = (await Promise.all(searches)).flat();
  all.sort((a, b) => b.score - a.score);
  return all.slice(0, k);
}

// ── collection stats helper ───────────────────────────────────────────────────

export async function collectionStats() {
  const client = getClient();
  const stats = await Promise.all(
    COLLECTIONS.map(async name => {
      try {
        const exists = await client.hasCollection({ collection_name: name });
        if (!exists.value) return { name, count: 0, exists: false };
        const stat = await client.getCollectionStatistics({ collection_name: name });
        const count = parseInt(stat.data?.row_count ?? stat.stats?.find(s => s.key === 'row_count')?.value ?? '0', 10);
        return { name, count, exists: true };
      } catch (err) {
        return { name, count: -1, exists: false, error: err.message };
      }
    })
  );
  return stats;
}
