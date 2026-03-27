/**
 * rcc/vector/ingest.mjs — Ingest helpers for the Milvus RAG pipeline
 *
 * All public functions follow the fire-and-forget pattern:
 * failures are logged but never thrown.
 */

import { readFile } from 'fs/promises';
import { createHash } from 'crypto';
import { upsert } from './index.mjs';

// ── ID helpers ────────────────────────────────────────────────────────────────

function hashId(prefix, text) {
  return `${prefix}-${createHash('sha256').update(text).digest('hex').slice(0, 16)}`;
}

// ── Text chunking ─────────────────────────────────────────────────────────────

/**
 * Chunk markdown by headings and paragraphs, max ~500 words per chunk.
 */
export function chunkMarkdown(text, maxWords = 500) {
  // Split at heading boundaries first
  const sections = text.split(/(?=^#{1,4} )/m);
  const chunks = [];

  for (const section of sections) {
    const trimmed = section.trim();
    if (!trimmed) continue;

    const words = trimmed.split(/\s+/).filter(Boolean);
    if (words.length <= maxWords) {
      if (trimmed.length > 10) chunks.push(trimmed);
      continue;
    }

    // Split large sections by double-newline (paragraphs)
    const paragraphs = trimmed.split(/\n\n+/);
    let current = [];
    let wordCount = 0;

    for (const para of paragraphs) {
      const pWords = para.trim().split(/\s+/).filter(Boolean);
      if (!pWords.length) continue;

      if (wordCount + pWords.length > maxWords && current.length) {
        chunks.push(current.join('\n\n'));
        current = [];
        wordCount = 0;
      }
      current.push(para.trim());
      wordCount += pWords.length;
    }
    if (current.length) chunks.push(current.join('\n\n'));
  }

  return chunks.filter(c => c.length > 10);
}

// ── ingestMemoryFile ──────────────────────────────────────────────────────────

/**
 * Read a .md file, chunk it, upsert each chunk to rcc_memory.
 * @param {string} filePath  Absolute path to the markdown file
 */
export async function ingestMemoryFile(filePath) {
  try {
    const text = await readFile(filePath, 'utf8');
    const chunks = chunkMarkdown(text);

    let ok = 0;
    for (let i = 0; i < chunks.length; i++) {
      const chunk = chunks[i];
      const id = `mem:${filePath}:${i}`;
      try {
        await upsert('rcc_memory', {
          id,
          text: chunk,
          metadata: { source: filePath, type: 'memory', chunk_index: i },
        });
        ok++;
      } catch (err) {
        console.warn(`[ingest] chunk upsert failed (${filePath}#${i}): ${err.message}`);
      }
    }
    console.log(`[ingest] ${filePath} → ${ok}/${chunks.length} chunks`);
  } catch (err) {
    console.warn(`[ingest] ingestMemoryFile failed (${filePath}): ${err.message}`);
  }
}

// ── ingestQueueItem ───────────────────────────────────────────────────────────

/**
 * Upsert a queue item to rcc_queue.
 * @param {{ id: string, text: string, status?: string, assignee?: string }} item
 */
export async function ingestQueueItem(item) {
  try {
    const { id, text, status, assignee } = item;
    if (!id || !text) throw new Error('item requires id and text');
    await upsert('rcc_queue', {
      id: `q-${id}`,
      text,
      metadata: { id, status: status || 'unknown', assignee: assignee || '' },
    });
  } catch (err) {
    console.warn(`[ingest] ingestQueueItem failed: ${err.message}`);
  }
}

// ── ingestLesson ──────────────────────────────────────────────────────────────

/**
 * Upsert a lesson to rcc_lessons.
 * @param {{ id?: string, text: string, [key: string]: any }} lesson
 */
export async function ingestLesson(lesson) {
  try {
    const { text, ...rest } = lesson;
    if (!text) throw new Error('lesson requires text');
    const id = lesson.id || hashId('lesson', text.slice(0, 128));
    await upsert('rcc_lessons', { id, text, metadata: rest });
  } catch (err) {
    console.warn(`[ingest] ingestLesson failed: ${err.message}`);
  }
}

// ── ingestMessage ─────────────────────────────────────────────────────────────

/**
 * Upsert a SquirrelChat message to rcc_memory.
 * @param {{ id?: string|number, ts?: number, from_agent: string, text: string, channel?: string }} msg
 */
export async function ingestMessage(msg) {
  try {
    const { id, ts, from_agent, text, channel = 'chat' } = msg;
    if (!text || !from_agent) throw new Error('message requires from_agent and text');
    const docId = id != null
      ? `sc-${id}`
      : hashId('sc', `${from_agent}:${ts || Date.now()}:${text.slice(0, 64)}`);
    await upsert('rcc_memory', {
      id: docId,
      text,
      metadata: { type: 'squirrelchat', from_agent, channel, ts: ts || Date.now() },
    });
  } catch (err) {
    console.warn(`[ingest] ingestMessage failed: ${err.message}`);
  }
}
