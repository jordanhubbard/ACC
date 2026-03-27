#!/usr/bin/env node
/**
 * render-queue.mjs — Overnight Blender Render Queue
 * Natasha / Sparky (RTX GPU)
 *
 * Scans render_queue/input/ for .blend files, renders each with Blender + RTX,
 * outputs frames/images to render_queue/output/<name>/, then moves .blend to done/.
 * Writes a render-log.json summary and optionally notifies jkh via Slack.
 *
 * Usage: node render-queue.mjs [--notify] [--frames N] [--format PNG|JPEG]
 *        node render-queue.mjs --status
 */

import { execSync, spawnSync } from 'child_process';
import { existsSync, readdirSync, mkdirSync, renameSync, readFileSync, writeFileSync } from 'fs';
import { join, basename, extname } from 'path';

const WORKSPACE = '/home/jkh/.openclaw/workspace';
const QUEUE_DIR = join(WORKSPACE, 'render_queue');
const INPUT_DIR = join(QUEUE_DIR, 'input');
const OUTPUT_DIR = join(QUEUE_DIR, 'output');
const DONE_DIR = join(QUEUE_DIR, 'done');
const LOG_FILE = join(QUEUE_DIR, 'render-log.json');

const args = process.argv.slice(2);
const NOTIFY_SLACK = args.includes('--notify');
const STATUS_ONLY = args.includes('--status');
const FRAME_COUNT = (() => {
  const idx = args.indexOf('--frames');
  return idx >= 0 ? parseInt(args[idx + 1]) || null : null;
})();
const FORMAT = (() => {
  const idx = args.indexOf('--format');
  return idx >= 0 ? (args[idx + 1] || 'PNG') : 'PNG';
})();

function log(msg) {
  const ts = new Date().toISOString();
  console.log(`[${ts}] ${msg}`);
}

function loadLog() {
  if (existsSync(LOG_FILE)) {
    try { return JSON.parse(readFileSync(LOG_FILE, 'utf8')); } catch { }
  }
  return { jobs: [], lastRun: null };
}

function saveLog(data) {
  writeFileSync(LOG_FILE, JSON.stringify(data, null, 2));
}

function getBlendFiles() {
  if (!existsSync(INPUT_DIR)) return [];
  return readdirSync(INPUT_DIR)
    .filter(f => extname(f).toLowerCase() === '.blend')
    .map(f => join(INPUT_DIR, f));
}

function renderBlendFile(blendPath, opts = {}) {
  const name = basename(blendPath, '.blend');
  const outDir = join(OUTPUT_DIR, name);
  mkdirSync(outDir, { recursive: true });

  const frameArgs = opts.frames ? ['-s', '1', '-e', String(opts.frames)] : [];
  const formatArg = opts.format || 'PNG';

  // Build blender command
  // -b = background, -a = render animation, -F = format, -o = output path
  const cmd = [
    'blender',
    '-b', blendPath,
    '-F', formatArg,
    '-o', join(outDir, `${name}_####`),
    ...frameArgs,
    '-a'  // render animation (all frames)
  ];

  log(`Rendering: ${name}`);
  log(`Command: ${cmd.join(' ')}`);

  const startTs = Date.now();
  const result = spawnSync(cmd[0], cmd.slice(1), {
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
    timeout: 4 * 60 * 60 * 1000,  // 4h max per file
    env: { ...process.env, DISPLAY: ':0' }  // may need display for some render paths
  });

  const elapsed = Math.round((Date.now() - startTs) / 1000);
  const success = result.status === 0;

  if (!success) {
    log(`ERROR rendering ${name} (exit ${result.status}): ${result.stderr?.slice(-500)}`);
  } else {
    log(`Done: ${name} in ${elapsed}s`);
  }

  // Count output files
  const outputs = existsSync(outDir)
    ? readdirSync(outDir).filter(f => /\.(png|jpg|jpeg|exr)$/i.test(f))
    : [];

  return {
    name,
    blendFile: blendPath,
    outDir,
    success,
    elapsedSec: elapsed,
    outputFiles: outputs.length,
    stdout: result.stdout?.slice(-1000) || '',
    stderr: result.stderr?.slice(-500) || '',
    exitCode: result.status
  };
}

async function notifySlack(summary) {
  const SLACK_CHANNEL = 'UDYR7H4SC';  // jkh DM
  const msg = [
    `🎬 *Overnight Render Queue — Complete*`,
    `Jobs: ${summary.total} | ✅ ${summary.succeeded} OK | ❌ ${summary.failed} failed`,
    `Total time: ${Math.round(summary.totalElapsed / 60)}m`,
    summary.results.map(r =>
      `  • *${r.name}*: ${r.success ? `✅ ${r.outputFiles} frame(s) in ${r.elapsedSec}s` : `❌ exit ${r.exitCode}`}`
    ).join('\n'),
    summary.failed > 0 ? `\nCheck render-log.json for details.` : ''
  ].filter(Boolean).join('\n');

  // Use openclaw message tool via subshell
  log(`Slack notify: ${msg.slice(0, 100)}...`);
  // (In cron context, we write a notification file that the main session can pick up)
  const notifFile = join(QUEUE_DIR, 'render-notify.txt');
  writeFileSync(notifFile, msg);
  log(`Slack notification queued to ${notifFile}`);
}

async function main() {
  log('=== Overnight Render Queue ===');

  const queueLog = loadLog();

  if (STATUS_ONLY) {
    const files = getBlendFiles();
    console.log('\n=== Render Queue Status ===');
    console.log(`Input queue: ${files.length} file(s)`);
    files.forEach(f => console.log(`  - ${basename(f)}`));
    console.log(`\nLast run: ${queueLog.lastRun || 'never'}`);
    console.log(`Total jobs processed: ${queueLog.jobs.length}`);
    const recent = queueLog.jobs.slice(-5);
    if (recent.length) {
      console.log('\nRecent jobs:');
      recent.forEach(j => console.log(`  [${j.ts?.slice(0,16)}] ${j.name}: ${j.success ? `✅ ${j.outputFiles} frames` : '❌ failed'}`));
    }
    return;
  }

  const blendFiles = getBlendFiles();

  if (blendFiles.length === 0) {
    log('No .blend files in queue. Nothing to render.');
    console.log('\nTo queue a render, drop a .blend file into:');
    console.log(`  ${INPUT_DIR}`);
    return;
  }

  log(`Found ${blendFiles.length} file(s) to render.`);

  const results = [];
  const runTs = new Date().toISOString();

  for (const blendPath of blendFiles) {
    const result = renderBlendFile(blendPath, {
      frames: FRAME_COUNT,
      format: FORMAT
    });

    results.push({ ...result, ts: runTs });

    if (result.success) {
      // Move to done/
      const donePath = join(DONE_DIR, basename(blendPath));
      renameSync(blendPath, donePath);
      log(`Moved ${basename(blendPath)} → done/`);
    } else {
      log(`Leaving ${basename(blendPath)} in input/ (render failed)`);
    }
  }

  // Summary
  const succeeded = results.filter(r => r.success).length;
  const failed = results.length - succeeded;
  const totalElapsed = results.reduce((a, r) => a + r.elapsedSec, 0);

  const summary = {
    total: results.length,
    succeeded,
    failed,
    totalElapsed,
    results,
    runTs
  };

  // Update log
  queueLog.jobs.push(...results);
  queueLog.lastRun = runTs;
  // Keep last 100 jobs
  if (queueLog.jobs.length > 100) queueLog.jobs = queueLog.jobs.slice(-100);
  saveLog(queueLog);

  log(`\n=== Done ===`);
  log(`${succeeded}/${results.length} succeeded | ${totalElapsed}s total`);
  results.forEach(r => log(`  ${r.name}: ${r.success ? `✅ ${r.outputFiles} frame(s)` : `❌ exit ${r.exitCode}`}`));

  if (NOTIFY_SLACK && results.length > 0) {
    await notifySlack(summary);
  }
}

main().catch(e => {
  console.error('Fatal error:', e);
  process.exit(1);
});
