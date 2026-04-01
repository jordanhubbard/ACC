/**
 * routes/bus.mjs — SquirrelBus route handlers
 *
 * Extracted from api/index.mjs. Handles /bus/* and /api/bus/* routes.
 * Called via tryBusRoute(ctx) from the main handleRequest router.
 *
 * Requires shared state passed via ctx:
 *   _busReadMessages, _busAppend, _busSSEClients, _busPresence,
 *   _busSeq, _busAcks, _busDeadLetters, json, readBody, isAuthed
 */

export async function tryBusRoute({
  req, res, method, path, url, json, readBody, isAuthed,
  _busReadMessages, _busAppend, _busSSEClients, _busPresence,
  getBusSeq, _busAcks, _busDeadLetters,
}) {
  // Fast prefix check
  if (!path.startsWith('/bus/') && !path.startsWith('/api/bus/')) return false;

  // GET /bus/messages
  if (method === 'GET' && path === '/bus/messages') {
    const { from, to, limit, since, type } = Object.fromEntries(url.searchParams);
    const msgs = await _busReadMessages({ from, to, type, since, limit: limit ? parseInt(limit, 10) : 100 });
    return json(res, 200, msgs), true;
  }

  // POST /bus/send
  if (method === 'POST' && path === '/bus/send') {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const busBody = await readBody(req);
    const msg = await _busAppend(busBody);
    return json(res, 200, { ok: true, message: msg }), true;
  }

  // GET /bus/stream — SSE
  if (method === 'GET' && path === '/bus/stream') {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache',
      'Connection': 'keep-alive', 'Access-Control-Allow-Origin': '*',
    });
    res.write('data: {"type":"connected"}\n\n');
    _busSSEClients.add(res);
    req.on('close', () => _busSSEClients.delete(res));
    return true; // keep connection open
  }

  // POST /bus/heartbeat
  if (method === 'POST' && path === '/bus/heartbeat') {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const busHbBody = await readBody(req);
    const from = busHbBody.from;
    if (!from) return json(res, 400, { error: 'from required' }), true;
    _busPresence[from] = { agent: from, ts: new Date().toISOString(), status: 'online', ...busHbBody };
    await _busAppend({ from, to: 'all', type: 'heartbeat', body: JSON.stringify({ status: 'online', ...busHbBody }), mime: 'application/json' });
    return json(res, 200, { ok: true, presence: _busPresence }), true;
  }

  // GET /bus/presence
  if (method === 'GET' && path === '/bus/presence') {
    return json(res, 200, _busPresence), true;
  }

  // POST /bus/ack
  if (method === 'POST' && path === '/bus/ack') {
    if (!isAuthed(req)) return json(res, 401, { error: 'Unauthorized' }), true;
    const ackBody = await readBody(req);
    const { id, agent, result } = ackBody;
    if (!id) return json(res, 400, { error: 'id required' }), true;
    _busAcks.set(id, { id, agent, result, ts: new Date().toISOString() });
    return json(res, 200, { ok: true }), true;
  }

  // POST /bus/dead-letter
  if (method === 'POST' && path === '/bus/dead-letter') {
    const dlBody = await readBody(req);
    _busDeadLetters.push({ ...dlBody, _deadAt: new Date().toISOString() });
    if (_busDeadLetters.length > 500) _busDeadLetters.splice(0, _busDeadLetters.length - 500);
    return json(res, 200, { ok: true }), true;
  }

  // GET /bus/dead-letters
  if (method === 'GET' && path === '/bus/dead-letters') {
    return json(res, 200, _busDeadLetters), true;
  }

  // GET /bus/message/:id/status
  if (method === 'GET' && path.startsWith('/bus/message/') && path.endsWith('/status')) {
    const id = path.split('/')[3];
    const ack  = _busAcks.get(id) || null;
    const dead = _busDeadLetters.find(d => d.id === id) || null;
    const ackState = dead ? 'dead' : ack ? 'acked' : 'fire-and-forget';
    return json(res, 200, { id, ackState, ack, deadReason: dead?._deadReason ?? null }), true;
  }

  // GET /bus/replay — replay messages from watermark
  const busReplayMatch = path.match(/^\/bus\/replay\/([^/]+)$/);
  if (method === 'GET' && (path === '/bus/replay' || busReplayMatch)) {
    const agent = busReplayMatch?.[1] || url.searchParams.get('agent');
    const lastSeq = parseInt(url.searchParams.get('last_seq') || url.searchParams.get('watermark') || '0', 10);
    const limit   = parseInt(url.searchParams.get('limit') || '200', 10);
    const filter  = url.searchParams.get('filter');
    const msgs = await _busReadMessages({ limit: 2000 });
    const filtered = msgs
      .filter(m => m.seq > lastSeq)
      .filter(m => !filter || m.type === filter || m.to === agent || m.to === 'all')
      .slice(0, limit);
    return json(res, 200, { agent, replayed: filtered.length, messages: filtered, watermark: filtered.at(-1)?.seq ?? lastSeq }), true;
  }

  // GET /api/bus/messages — alias
  if (method === 'GET' && path === '/api/bus/messages') {
    const { from, to, limit, since, type } = Object.fromEntries(url.searchParams);
    const msgs = await _busReadMessages({ from, to, type, since, limit: limit ? parseInt(limit, 10) : 100 });
    return json(res, 200, msgs), true;
  }

  // GET /api/bus/status
  if (method === 'GET' && path === '/api/bus/status') {
    return json(res, 200, {
      ok: true,
      seq: getBusSeq(),
      client_count: _busSSEClients.size,
      presence: Object.fromEntries(Object.entries(_busPresence).map(([k, v]) => ([k, { ...v }]))),
      bus_seq: getBusSeq(),
      ts: new Date().toISOString(),
    }), true;
  }

  // POST /api/bus/receive — handle incoming SquirrelBus messages
  if (method === 'POST' && path === '/api/bus/receive') {
    // Note: lesson handling is done in the caller (index.mjs) before this module is called,
    // because it needs access to receiveLessonFromBus. Return false to let index.mjs handle it.
    return false;
  }

  return false; // not handled
}
