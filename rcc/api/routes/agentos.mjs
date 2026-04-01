/**
 * routes/agentos.mjs — agentOS-specific API routes
 *
 * Extracted from api/index.mjs.  Called by the main handleRequest router via
 * tryAgentOSRoute(ctx).  Returns true if the request was handled, false to
 * fall through to the next route group.
 *
 * Context object shape (passed from index.mjs):
 *   { req, res, method, path, url, json, readBody, isAuthed }
 *
 * All agentOS routes match /api/agentos/*, /api/mesh, or related prefixes.
 */

export async function tryAgentOSRoute({ req, res, method, path, url, json, readBody, isAuthed }) {
  // Fast prefix check — skip immediately for non-agentos paths
  if (!path.startsWith('/api/agentos') && path !== '/api/mesh') return false;

  // ── GET /api/agentos/slots — VibeEngine slot health + swap metrics ──────────
  // 5-minute cache. Polls AgentFS /health and returns synthesized slot state.
  if (method === 'GET' && path === '/api/agentos/slots') {
    const AGENTOS_CACHE_TTL = 5 * 60 * 1000;
    const now = Date.now();
    if (!global._agentosSlotCache) global._agentosSlotCache = { data: null, ts: 0 };
    const cache = global._agentosSlotCache;
    if (cache.data && (now - cache.ts) < AGENTOS_CACHE_TTL) {
      return json(res, 200, cache.data), true;
    }
    // Probe AgentFS on sparky (content-addressed WASM store)
    const AGENTFS_URL  = process.env.AGENTFS_URL  || 'http://100.87.229.125:8791';
    let agentfsHealth  = null;
    try {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 2000);
      const r = await fetch(`${AGENTFS_URL}/health`, { signal: ctrl.signal });
      clearTimeout(timer);
      agentfsHealth = r.ok ? await r.json().catch(() => ({ ok: true })) : null;
    } catch { /* AgentFS offline */ }

    // MAX_SWAP_SLOTS=4 (from agentos.h), AGENT_POOL_SIZE=8 workers
    const seed = Math.floor(now / 60000);
    function sr(n, s2) { return ((n * 1337 + s2 * 7919) % 997) / 997; }
    const SLOT_STATES = ['running','idle','suspended','evicted'];
    const AGENT_NAMES = ['init_agent','vibe_engine','mesh_agent','debug_bridge','quota_pd','fault_handler'];
    const slots = Array.from({ length: 8 }, (_, i) => ({
      slot_id:        i,
      state:          SLOT_STATES[Math.floor(sr(i, seed) * SLOT_STATES.length)],
      heap_used_kb:   Math.floor(sr(i + 10, seed) * 16384),
      heap_cap_kb:    16384,
      age_ticks:      Math.floor(sr(i + 20, seed) * 0x100000),
      priority:       64 + Math.floor(sr(i + 30, seed) * 192),
      pinned:         i === 0, // slot 0 (init_agent) always pinned
      agent_name:     AGENT_NAMES[i % AGENT_NAMES.length],
      wasm_hash:      `sha256:${[...Array(8)].map((_,j)=>('0'+Math.floor(sr(i*10+j,seed)*256).toString(16)).slice(-2)).join('')}`,
      last_eviction:  sr(i, seed) < 0.15 ? new Date(now - Math.floor(sr(i+50,seed)*3600000)).toISOString() : null,
    }));
    const swap_slots = Array.from({ length: 4 }, (_, i) => ({
      swap_id:    i,
      occupied:   sr(i + 100, seed) > 0.5,
      slot_ref:   sr(i + 100, seed) > 0.5 ? Math.floor(sr(i + 110, seed) * 8) : null,
      saved_at:   sr(i + 100, seed) > 0.5 ? new Date(now - Math.floor(sr(i+120,seed)*7200000)).toISOString() : null,
    }));
    const result = {
      slots, swap_slots,
      agentfs: agentfsHealth ? { online: true, ...agentfsHealth } : { online: false },
      total_heap_budget_kb: 8 * 16384,
      used_heap_kb: slots.reduce((s,sl) => s + sl.heap_used_kb, 0),
      ts: new Date(now).toISOString(),
    };
    cache.data = result; cache.ts = now;
    return json(res, 200, result), true;
  }

  // ── GET /api/agentos/debug/sessions — list active debug bridge sessions ────
  if (method === 'GET' && path === '/api/agentos/debug/sessions') {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    if (!global._debugSessions) global._debugSessions = new Map();
    const sessions = [...global._debugSessions.entries()].map(([id, s]) => ({
      session_id: id, slot_id: s.slot_id, agent: s.agent,
      attached_at: s.attached_at, state: s.state || 'attached',
    }));
    return json(res, 200, { sessions, count: sessions.length }), true;
  }

  // ── POST /api/agentos/debug/attach — attach debugger to a WASM slot ────────
  const debugAttachMatch = path.match(/^\/api\/agentos\/debug\/attach$/);
  if (method === 'POST' && debugAttachMatch) {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const body = await readBody(req);
    const slot_id = parseInt(body.slot_id ?? 0, 10);
    if (slot_id < 0 || slot_id > 7) return json(res, 400, { error: 'slot_id 0-7 required' }), true;
    if (!global._debugSessions) global._debugSessions = new Map();
    const session_id = `dbg-${Date.now()}-${slot_id}`;
    global._debugSessions.set(session_id, {
      slot_id, agent: body.agent || 'unknown',
      attached_at: new Date().toISOString(), state: 'attached',
    });
    return json(res, 200, { ok: true, session_id, slot_id }), true;
  }

  // ── POST /api/agentos/debug/detach — detach debugger from a slot ──────────
  const debugDetachMatch = path.match(/^\/api\/agentos\/debug\/detach$/);
  if (method === 'POST' && debugDetachMatch) {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const body = await readBody(req);
    if (!global._debugSessions) global._debugSessions = new Map();
    const deleted = global._debugSessions.delete(body.session_id);
    return json(res, 200, { ok: deleted }), true;
  }

  // ── POST /api/agentos/debug/breakpoint — set/clear breakpoint ─────────────
  const debugBpMatch = path.match(/^\/api\/agentos\/debug\/breakpoint$/);
  if (method === 'POST' && debugBpMatch) {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const body = await readBody(req);
    if (!global._debugBreakpoints) global._debugBreakpoints = [];
    if (body.clear) {
      global._debugBreakpoints = global._debugBreakpoints.filter(b => b.id !== body.id);
    } else {
      global._debugBreakpoints.push({ id: `bp-${Date.now()}`, ...body, set_at: new Date().toISOString() });
    }
    return json(res, 200, { ok: true, breakpoints: global._debugBreakpoints }), true;
  }

  // ── POST /api/agentos/debug/step — single-step a suspended slot ───────────
  const debugStepMatch = path.match(/^\/api\/agentos\/debug\/step$/);
  if (method === 'POST' && debugStepMatch) {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const body = await readBody(req);
    // Synthetic response — real impl would IPC to debug_bridge PD
    return json(res, 200, {
      ok: true, slot_id: body.slot_id,
      pc: `0x${(0x10000 + Math.floor(Math.random() * 0x1000)).toString(16)}`,
      instruction: 'i32.add', stepped: true,
    }), true;
  }

  // ── GET /api/agentos/console/:slot — console_mux ring output ──────────────
  const consoleGetMatch = path.match(/^\/api\/agentos\/console\/(\d+)$/);
  if (method === 'GET' && consoleGetMatch) {
    const slot = parseInt(consoleGetMatch[1], 10);
    if (!global._consoleMuxRings) global._consoleMuxRings = {};
    const ring = global._consoleMuxRings[slot] || [];
    return json(res, 200, { slot, lines: ring, count: ring.length }), true;
  }

  // ── POST /api/agentos/console/attach/:slot — send attach command ──────────
  const consoleAttachMatch = path.match(/^\/api\/agentos\/console\/attach\/(\d+)$/);
  if (method === 'POST' && consoleAttachMatch) {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const slot = parseInt(consoleAttachMatch[1], 10);
    return json(res, 200, { ok: true, slot, message: `Attach command sent to slot ${slot}` }), true;
  }

  // ── POST /api/agentos/console/push — console_mux ring ingest (internal) ───
  if (method === 'POST' && path === '/api/agentos/console/push') {
    const body = await readBody(req);
    const slot = parseInt(body.slot ?? 0, 10);
    if (!global._consoleMuxRings) global._consoleMuxRings = {};
    if (!global._consoleMuxRings[slot]) global._consoleMuxRings[slot] = [];
    const ring = global._consoleMuxRings[slot];
    ring.push({ ts: new Date().toISOString(), text: body.text || '' });
    if (ring.length > 200) ring.splice(0, ring.length - 200);
    return json(res, 200, { ok: true, slot, ring_len: ring.length }), true;
  }

  // ── GET /api/agentos/shell — dev_shell Terminal tab (SSE output stream) ────
  if (method === 'GET' && path === '/api/agentos/shell') {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache',
      'Connection': 'keep-alive', 'Access-Control-Allow-Origin': '*',
    });
    res.write('data: {"type":"connected","msg":"dev_shell ready"}\n\n');
    if (!global._shellSSEClients) global._shellSSEClients = new Set();
    global._shellSSEClients.add(res);
    req.on('close', () => global._shellSSEClients?.delete(res));
    return true; // keep open
  }

  // ── POST /api/agentos/shell/cmd — write a command to dev_shell ────────────
  if (method === 'POST' && path === '/api/agentos/shell/cmd') {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const body = await readBody(req);
    const cmd = (body.cmd || '').trim();
    if (!cmd) return json(res, 400, { error: 'cmd required' }), true;
    // Echo back synthetic response — real impl sends IPC to dev_shell PD
    const output = `[dev_shell] $ ${cmd}\n(QEMU bridge not connected — synthetic echo)\n`;
    if (global._shellSSEClients) {
      for (const client of global._shellSSEClients) {
        try { client.write(`data: ${JSON.stringify({ type: 'output', text: output })}\n\n`); }
        catch { global._shellSSEClients.delete(client); }
      }
    }
    return json(res, 200, { ok: true, cmd, queued: true }), true;
  }

  // ── POST /api/agentos/shell/push — QEMU bridge pushes dev_shell output ────
  if (method === 'POST' && path === '/api/agentos/shell/push') {
    const body = await readBody(req);
    const text = body.text || '';
    if (global._shellSSEClients) {
      for (const client of global._shellSSEClients) {
        try { client.write(`data: ${JSON.stringify({ type: 'output', text })}\n\n`); }
        catch { global._shellSSEClients.delete(client); }
      }
    }
    return json(res, 200, { ok: true, clients: global._shellSSEClients?.size ?? 0 }), true;
  }

  // Note: /api/agentos/cap-events and /api/agentos/events and /api/agentos/timeline
  // are handled later in index.mjs (they were added in later commits).
  // They will be migrated here in a follow-up pass.

  return false; // not handled — fall through
}
