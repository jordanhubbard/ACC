/**
 * rcc/memory/assembler.mjs — WorkingMemoryAssembler
 *
 * Token-budgeted context assembly. Given a query/trigger, searches multiple
 * memory sources and returns structured context that fits within a token budget.
 *
 * Sources:
 *   - Episodic digests  (recentDigests from episodic.mjs)
 *   - Semantic search   (vectorSearch from vector/index.mjs, graceful fallback)
 *   - Relationship/entity context (future — returns empty string if unavailable)
 *
 * Usage:
 *   import { assemble } from './assembler.mjs';
 *   const ctx = await assemble('bootstrap failures', { budget: { total: 1500 } });
 *   // prepend ctx.summary to your prompt
 */

import { recentDigests } from './episodic.mjs';

// ── Token budget defaults ───────────────────────────────────────────────────
const DEFAULT_BUDGET = {
  knowledge:     800,  // tokens for semantic/Milvus results
  episodes:      400,  // tokens for episodic digests
  relationships: 300,  // tokens for person/entity context
  total:        2000,
};

// ── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Rough token estimate: ~4 chars per token.
 * @param {string} text
 * @returns {number}
 */
export function estimateTokens(text) {
  return Math.ceil((text || '').length / 4);
}

/**
 * Apply tiered rendering to fit items within a token budget.
 *
 * Tier 1 (top 3): full content
 * Tier 2 (4-10): compact — first sentence + score
 * Tier 3 (rest): name-only
 *
 * @param {Array<{id:string, content:string, score?:number}>} items
 * @param {number} tokenBudget
 * @returns {string} rendered block
 */
export function budgetSection(items, tokenBudget) {
  if (!items || items.length === 0) return '';

  const lines = [];
  let used = 0;

  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    let rendered;

    if (i < 3) {
      // Full content
      rendered = item.content || item.text || item.summary || String(item.id);
    } else if (i < 10) {
      // Compact: first sentence + score
      const full = item.content || item.text || item.summary || '';
      const firstSentence = full.split(/[.!?]\s/)[0] || full.slice(0, 120);
      const scoreStr = item.score != null ? ` (score: ${Number(item.score).toFixed(2)})` : '';
      rendered = `${firstSentence}${scoreStr}`;
    } else {
      // Name-only
      rendered = `- ${item.id || item.name || 'item'}`;
    }

    const tokens = estimateTokens(rendered);
    if (used + tokens > tokenBudget) {
      // Add a truncation note and stop
      lines.push(`[… ${items.length - i} more result(s) omitted by token budget]`);
      break;
    }

    lines.push(rendered);
    used += tokens;
  }

  return lines.join('\n\n');
}

// ── Vector search with graceful fallback ────────────────────────────────────

async function tryVectorSearch(query, limit = 10) {
  try {
    const { vectorSearch } = await import('../vector/index.mjs');
    // Search rcc_memory collection for relevant snippets
    const hits = await vectorSearch('rcc_memory', query, limit);
    return (hits || []).map(h => ({
      id:      h.id,
      content: h.content || h.text || '',
      score:   h.score,
    }));
  } catch {
    // Milvus unavailable — degrade gracefully
    return [];
  }
}

// ── Episode rendering ────────────────────────────────────────────────────────

function renderDigest(digest) {
  const lines = [];
  lines.push(`[${digest.endTime?.slice(0, 16) || '?'}] ${digest.agentName || 'agent'} — ${digest.summary}`);
  if (digest.actionsToken?.length) lines.push(`  actions: ${digest.actionsToken.join(', ')}`);
  if (digest.themes?.length)       lines.push(`  themes: ${digest.themes.join(', ')}`);
  if (digest.learnings?.length)    lines.push(`  learnings: ${digest.learnings.join(' | ')}`);
  return lines.join('\n');
}

// ── Main assembler ───────────────────────────────────────────────────────────

/**
 * Assemble token-budgeted context for a query.
 *
 * @param {string} query   — the question or trigger
 * @param {object} [opts]
 * @param {object} [opts.budget]         — override budget sections
 * @param {number} [opts.episodeHours=24] — how far back to look for digests
 * @param {number} [opts.vectorLimit=10]  — max vector search results
 * @returns {Promise<{knowledge:string, episodes:string, relationships:string, totalTokens:number, summary:string}>}
 */
export async function assemble(query, opts = {}) {
  const budget = { ...DEFAULT_BUDGET, ...(opts.budget || {}) };
  const episodeHours = opts.episodeHours ?? 24;
  const vectorLimit  = opts.vectorLimit  ?? 10;

  // ── Fetch all sources in parallel ──────────────────────────────────────────
  const [knowledgeItems, digests] = await Promise.all([
    tryVectorSearch(query, vectorLimit),
    recentDigests(episodeHours),
  ]);

  // ── Render episodes ────────────────────────────────────────────────────────
  const episodeItems = digests.map(d => ({
    id:      d.id,
    content: renderDigest(d),
    score:   d.significance / 10,
  }));
  const episodes = budgetSection(episodeItems, budget.episodes);

  // ── Render knowledge ───────────────────────────────────────────────────────
  const knowledge = budgetSection(knowledgeItems, budget.knowledge);

  // ── Relationships (placeholder — extend when entity graph exists) ──────────
  const relationships = '';

  // ── Token accounting ───────────────────────────────────────────────────────
  const knowledgeTokens     = estimateTokens(knowledge);
  const episodeTokens       = estimateTokens(episodes);
  const relationshipTokens  = estimateTokens(relationships);
  const totalTokens = knowledgeTokens + episodeTokens + relationshipTokens;

  // ── Build summary header ───────────────────────────────────────────────────
  const parts = [];
  if (episodes)       parts.push(`## Recent Activity (last ${episodeHours}h)\n${episodes}`);
  if (knowledge)      parts.push(`## Relevant Knowledge\n${knowledge}`);
  if (relationships)  parts.push(`## Relationships\n${relationships}`);

  const summary = parts.length
    ? `<!-- WorkingMemory: ${totalTokens} tokens -->\n${parts.join('\n\n')}`
    : '';

  return { knowledge, episodes, relationships, totalTokens, summary };
}
