/**
 * rcc/memory/episodic.mjs — EpisodicMemory module
 *
 * Two-level episodic memory system:
 *
 *   ActivityDigest  — captures a discrete work chunk (task complete, topic shift, etc.)
 *   SessionSynthesis — end-of-session roll-up across all digests
 *
 * Storage layout:
 *   ~/.rcc/memory/episodic/digests/<YYYY-MM-DD>/digest-<ts>.json
 *   ~/.rcc/memory/episodic/synthesis/<YYYY-MM-DD>.json
 */

import { readFile, writeFile, mkdir, readdir } from 'fs/promises';
import { existsSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';

// ── Paths ───────────────────────────────────────────────────────────────────
const BASE_DIR     = join(homedir(), '.rcc', 'memory', 'episodic');
const DIGEST_DIR   = join(BASE_DIR, 'digests');
const SYNTH_DIR    = join(BASE_DIR, 'synthesis');

function dateStr(d = new Date()) {
  return d.toISOString().slice(0, 10); // YYYY-MM-DD
}

async function ensureDir(dir) {
  if (!existsSync(dir)) await mkdir(dir, { recursive: true });
}

// ── ActivityDigest ──────────────────────────────────────────────────────────

/**
 * Save an ActivityDigest to disk.
 * @param {object} digest
 * @param {string} digest.id            — digest-<timestamp>
 * @param {string} digest.agentName
 * @param {string} digest.startTime     — ISO
 * @param {string} digest.endTime       — ISO
 * @param {string} digest.summary       — 1-3 sentence summary
 * @param {string[]} digest.actionsToken
 * @param {string[]} digest.entitiesCreated
 * @param {string[]} digest.learnings
 * @param {number} digest.significance  — 1-10
 * @param {string[]} digest.themes
 * @param {string} digest.boundarySignal — task_complete|topic_shift|session_end|time_threshold
 */
export async function saveDigest(digest) {
  const date = dateStr(new Date(digest.endTime || Date.now()));
  const dir  = join(DIGEST_DIR, date);
  await ensureDir(dir);
  const path = join(dir, `${digest.id}.json`);
  await writeFile(path, JSON.stringify(digest, null, 2));
  return path;
}

/**
 * Read one digest by ID, searching today then all dates.
 * @param {string} id — digest-<timestamp>
 */
export async function getDigest(id) {
  // Try today first
  const today = join(DIGEST_DIR, dateStr(), `${id}.json`);
  if (existsSync(today)) return JSON.parse(await readFile(today, 'utf8'));

  // Search all date dirs
  if (!existsSync(DIGEST_DIR)) return null;
  const dates = await readdir(DIGEST_DIR).catch(() => []);
  for (const date of dates.sort().reverse()) {
    const p = join(DIGEST_DIR, date, `${id}.json`);
    if (existsSync(p)) return JSON.parse(await readFile(p, 'utf8'));
  }
  return null;
}

/**
 * List digests for a given date (default: today).
 * @param {string} [date] — YYYY-MM-DD
 * @returns {object[]} array of digest objects sorted by endTime ascending
 */
export async function listDigests(date) {
  const dir = join(DIGEST_DIR, date || dateStr());
  if (!existsSync(dir)) return [];
  const files = await readdir(dir).catch(() => []);
  const digests = await Promise.all(
    files
      .filter(f => f.endsWith('.json'))
      .map(f => readFile(join(dir, f), 'utf8').then(JSON.parse).catch(() => null))
  );
  return digests
    .filter(Boolean)
    .sort((a, b) => (a.endTime || '').localeCompare(b.endTime || ''));
}

/**
 * List digests from the last N hours (default 24).
 * @param {number} [hours=24]
 * @returns {object[]} sorted by endTime ascending
 */
export async function recentDigests(hours = 24) {
  const cutoff = new Date(Date.now() - hours * 3600 * 1000);
  if (!existsSync(DIGEST_DIR)) return [];

  const dates = await readdir(DIGEST_DIR).catch(() => []);
  // Only load dates that could contain relevant digests
  const relevant = dates.filter(d => d >= dateStr(cutoff));

  const all = [];
  for (const date of relevant) {
    const items = await listDigests(date);
    all.push(...items.filter(d => new Date(d.endTime || 0) >= cutoff));
  }
  return all.sort((a, b) => (a.endTime || '').localeCompare(b.endTime || ''));
}

// ── SessionSynthesis ─────────────────────────────────────────────────────────

/**
 * Save a SessionSynthesis to disk.
 * @param {object} synthesis
 * @param {string} synthesis.id            — synthesis-<timestamp>
 * @param {string} synthesis.agentName
 * @param {string} synthesis.sessionDate   — ISO
 * @param {string[]} synthesis.keyOutcomes
 * @param {string[]} synthesis.allLearnings
 * @param {number} synthesis.significance  — 1-10
 * @param {string[]} synthesis.themes
 * @param {string[]} synthesis.followUp
 */
export async function saveSynthesis(synthesis) {
  await ensureDir(SYNTH_DIR);
  const date = dateStr(new Date(synthesis.sessionDate || Date.now()));
  const path = join(SYNTH_DIR, `${date}.json`);
  await writeFile(path, JSON.stringify(synthesis, null, 2));
  return path;
}

/**
 * Read synthesis for a given date (default: today).
 * @param {string} [date] — YYYY-MM-DD
 */
export async function getSynthesis(date) {
  const path = join(SYNTH_DIR, `${date || dateStr()}.json`);
  if (!existsSync(path)) return null;
  return JSON.parse(await readFile(path, 'utf8'));
}

/**
 * List recent syntheses, newest first.
 * @param {number} [limit=7]
 * @returns {object[]}
 */
export async function listSyntheses(limit = 7) {
  if (!existsSync(SYNTH_DIR)) return [];
  const files = await readdir(SYNTH_DIR).catch(() => []);
  const sorted = files.filter(f => f.endsWith('.json')).sort().reverse().slice(0, limit);
  const results = await Promise.all(
    sorted.map(f => readFile(join(SYNTH_DIR, f), 'utf8').then(JSON.parse).catch(() => null))
  );
  return results.filter(Boolean);
}
