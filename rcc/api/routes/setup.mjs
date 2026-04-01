/**
 * rcc/api/routes/setup.mjs — Setup wizard SSE progress stream
 *
 * GET  /api/setup/progress  — SSE stream of wizard step events (public read)
 * POST /api/setup/step      — Push a step update (auth-required)
 * POST /api/setup/reset     — Clear wizard state and notify clients (auth-required)
 */

// In-memory wizard state (module-level so it survives across requests)
let wizardSteps = [];           // [{step, status, message, ts}]
const wizardSseClients = new Set(); // active SSE Response objects

// Heartbeat interval — keep connections alive every 5s
let _heartbeatInterval = null;
function ensureHeartbeat() {
  if (_heartbeatInterval) return;
  _heartbeatInterval = setInterval(() => {
    const line = 'event: heartbeat\ndata: {}\n\n';
    for (const client of wizardSseClients) {
      try { client.write(line); }
      catch { wizardSseClients.delete(client); }
    }
  }, 5000);
  // Don't block process exit
  if (_heartbeatInterval.unref) _heartbeatInterval.unref();
}

function sendEvent(client, event, data) {
  try {
    client.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  } catch {
    wizardSseClients.delete(client);
  }
}

function broadcastEvent(event, data) {
  const line = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
  for (const client of wizardSseClients) {
    try { client.write(line); }
    catch { wizardSseClients.delete(client); }
  }
}

export default function registerRoutes(app, state) {
  const { json, readBody, isAuthed } = state;

  // ── GET /api/setup/progress — SSE stream ─────────────────────────────────
  app.on('GET', '/api/setup/progress', async (req, res) => {
    res.writeHead(200, {
      'Content-Type':  'text/event-stream',
      'Cache-Control': 'no-cache',
      'Connection':    'keep-alive',
      'Access-Control-Allow-Origin': '*',
    });

    // Replay last 10 steps so latecomers get current state
    const replay = wizardSteps.slice(-10);
    for (const step of replay) {
      sendEvent(res, 'step', step);
    }

    wizardSseClients.add(res);
    ensureHeartbeat();

    req.on('close', () => wizardSseClients.delete(res));
    req.on('error', () => wizardSseClients.delete(res));

    return; // keep connection open
  });

  // ── POST /api/setup/step — push a wizard step update ─────────────────────
  app.on('POST', '/api/setup/step', async (req, res) => {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' });

    let body;
    try { body = await readBody(req); }
    catch { return json(res, 400, { error: 'Invalid JSON' }); }

    const { step, status, message } = body;
    if (!step || !status) return json(res, 400, { error: 'step and status are required' });

    const entry = { step, status, message: message || '', ts: new Date().toISOString() };
    wizardSteps.push(entry);

    broadcastEvent('step', entry);

    // If this step signals completion or fatal error, send terminal event
    if (status === 'done') broadcastEvent('done', { ts: entry.ts });
    if (status === 'error') broadcastEvent('error', entry);

    return json(res, 200, { ok: true, entry });
  });

  // ── POST /api/setup/reset — clear wizard state ────────────────────────────
  app.on('POST', '/api/setup/reset', async (req, res) => {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' });

    wizardSteps = [];
    broadcastEvent('reset', { ts: new Date().toISOString() });

    return json(res, 200, { ok: true });
  });
}
