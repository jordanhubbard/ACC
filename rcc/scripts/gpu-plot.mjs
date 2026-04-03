#!/usr/bin/env node
/**
 * gpu-plot.mjs — Visualize sparky GB10 GPU load history from gpu-metrics.jsonl
 * Usage: node gpu-plot.mjs [--days 7] [--field util_pct] [--jsonl /path/to/file]
 *
 * Fields: temp_c, power_w, util_pct, ram_used_mb, vram_used_mb
 */

import { readFileSync, existsSync } from 'fs';
import { createGunzip } from 'zlib';
import { createReadStream } from 'fs';
import { pipeline } from 'stream/promises';
import { createInterface } from 'readline';

const DEFAULTS = {
  days: 7,
  field: null, // null = all fields
  jsonlPath: `${process.env.HOME}/.openclaw/workspace/telemetry/gpu-metrics.jsonl`,
};

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = { ...DEFAULTS };
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--days' && args[i+1]) opts.days = parseInt(args[++i]);
    else if (args[i] === '--field' && args[i+1]) opts.field = args[++i];
    else if (args[i] === '--jsonl' && args[i+1]) opts.jsonlPath = args[++i];
    else if (args[i] === '--help') {
      console.log('Usage: node gpu-plot.mjs [--days N] [--field FIELD] [--jsonl PATH]');
      console.log('Fields: temp_c, power_w, util_pct, ram_used_mb, vram_used_mb');
      process.exit(0);
    }
  }
  return opts;
}

async function readJsonl(filePath) {
  if (!existsSync(filePath)) return [];
  const records = [];
  const rl = createInterface({ input: createReadStream(filePath), crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    try { records.push(JSON.parse(line)); } catch {}
  }
  return records;
}

function hourBucket(ts) {
  const d = new Date(ts);
  d.setMinutes(0, 0, 0);
  return d.toISOString();
}

function sparkline(values, min, max, width = 40) {
  const BLOCKS = ['▁','▂','▃','▄','▅','▆','▇','█'];
  const range = max - min || 1;
  return values.map(v => {
    if (v == null) return ' ';
    const idx = Math.min(7, Math.floor(((v - min) / range) * 8));
    return BLOCKS[idx];
  }).join('');
}

function barLine(label, value, max, unit, width = 30) {
  const pct = Math.min(1, value / (max || 1));
  const filled = Math.round(pct * width);
  const bar = '█'.repeat(filled) + '░'.repeat(width - filled);
  return `${label.padEnd(12)} [${bar}] ${String(Math.round(value)).padStart(6)}${unit}`;
}

function stats(arr) {
  const valid = arr.filter(v => v != null && !isNaN(v));
  if (!valid.length) return { min: 0, max: 0, avg: 0, last: 0 };
  return {
    min: Math.min(...valid),
    max: Math.max(...valid),
    avg: valid.reduce((a, b) => a + b, 0) / valid.length,
    last: valid[valid.length - 1],
  };
}

const FIELDS = [
  { key: 'util_pct',    label: 'GPU Util',  unit: '%',  maxHint: 100 },
  { key: 'temp_c',      label: 'Temp',      unit: '°C', maxHint: 100 },
  { key: 'power_w',     label: 'Power',     unit: 'W',  maxHint: 200 },
  { key: 'ram_used_mb', label: 'RAM Used',  unit: 'MB', maxHint: null },
  { key: 'vram_used_mb',label: 'vRAM Used', unit: 'MB', maxHint: null },
];

async function main() {
  const opts = parseArgs();
  const records = await readJsonl(opts.jsonlPath);

  if (!records.length) {
    console.log(`No data found in: ${opts.jsonlPath}`);
    console.log('Run ollama-watchdog.service for a few cycles to populate telemetry.');
    process.exit(0);
  }

  const cutoff = Date.now() - opts.days * 86400 * 1000;
  const filtered = records.filter(r => new Date(r.ts).getTime() >= cutoff);

  if (!filtered.length) {
    console.log(`No records in the last ${opts.days} days (${records.length} total records exist).`);
    process.exit(0);
  }

  // Group by hour
  const byHour = {};
  for (const r of filtered) {
    const h = hourBucket(r.ts);
    if (!byHour[h]) byHour[h] = [];
    byHour[h].push(r);
  }

  // Compute hourly averages
  const hourKeys = Object.keys(byHour).sort();
  const hourlyAvgs = hourKeys.map(h => {
    const rows = byHour[h];
    const avg = {};
    for (const f of FIELDS) {
      const vals = rows.map(r => r[f.key]).filter(v => v != null && !isNaN(v));
      avg[f.key] = vals.length ? vals.reduce((a,b)=>a+b,0)/vals.length : null;
    }
    avg.ts = h;
    return avg;
  });

  const fieldsToShow = opts.field
    ? FIELDS.filter(f => f.key === opts.field)
    : FIELDS;

  // Header
  const firstTs = new Date(filtered[0].ts);
  const lastTs = new Date(filtered[filtered.length - 1].ts);
  console.log(`\n🔥 sparky GB10 GPU Metrics — last ${opts.days}d (${filtered.length} samples)`);
  console.log(`   ${firstTs.toLocaleString()} → ${lastTs.toLocaleString()}\n`);

  // Per-field sparkline + stats
  for (const field of fieldsToShow) {
    const vals = hourlyAvgs.map(h => h[field.key]);
    const s = stats(vals.filter(v=>v!=null));
    const line = sparkline(vals, s.min, s.max, hourlyAvgs.length);
    const maxBar = field.maxHint || s.max || 1;

    console.log(`  ${field.label.padEnd(10)} ${field.unit}`);
    console.log(`  ${line}`);
    console.log(`  min:${Math.round(s.min)}  avg:${Math.round(s.avg)}  max:${Math.round(s.max)}  now:${Math.round(s.last)}`);
    console.log('');
  }

  // Current snapshot (last record)
  const last = filtered[filtered.length - 1];
  console.log('─'.repeat(50));
  console.log(`  Latest reading: ${new Date(last.ts).toLocaleString()}`);
  for (const f of fieldsToShow) {
    if (last[f.key] != null) {
      const maxBar = f.maxHint || (stats(filtered.map(r=>r[f.key]).filter(v=>v!=null)).max) || 1;
      console.log('  ' + barLine(f.label, last[f.key], maxBar, f.unit));
    }
  }

  if (last.ollama_models && last.ollama_models.length) {
    console.log(`\n  ollama models: ${last.ollama_models.join(', ')}`);
  }
  console.log('');
}

main().catch(e => { console.error(e.message); process.exit(1); });
