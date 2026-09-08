/**
 * Test support: a fake Space Station backend (HTTP + WS on 127.0.0.1) that answers exactly the
 * calls the runtime makes and records everything it saw, plus a host launcher and small waits.
 */
"use strict";
const http = require("node:http");
const { WebSocketServer } = require("ws");
const runtime = require("../mission-control.js");

const HEX = "0123456789abcdef0123456789abcdef";
const TOKEN = `spacewindow-${HEX}`;
const sleep = ms => new Promise(r => setTimeout(r, ms));

/** Polls `fn` every 5 ms until it is truthy and returns that value. */
async function until(fn, what = "condition", ms = 5000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(5);
  }
}

/** `b.query(q, n)` answers `/query`; it may return a promise to hold the runtime. Mutate `b.tables` to move watermarks, set `b.refuse = {code, reason}` to close every new socket. */
async function backend({ tables = { orders: 130 }, version = null, state = { json: null, metadata: {} }, query } = {}) {
  const seen = { requests: [], queries: [], frames: [], upgrades: [], sockets: new Set() };
  const b = { tables, version, state, seen, query: query ?? (() => ({ rows: [], watermarks: {} })) };
  const server = http.createServer(async (req, res) => {
    let body = "";
    for await (const c of req) body += c;
    seen.requests.push({ method: req.method, url: req.url, auth: req.headers.authorization, body });
    const send = (status, v) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(v));
    };
    const rest = req.url.match(/^\/api\/orgs\/[^/]+(\/.*)$/)?.[1] ?? "";
    if (rest === "/tables") return send(200, Object.entries(b.tables).map(([id, watermark]) => ({ id, records: watermark, watermark, access: ["@alice"], created_by: "alice", created_at: "2026-09-03T00:00:00Z" })));
    if (/^\/windows\/[^/]+$/.test(rest)) return send(200, { id: rest.split("/")[2], name: "Orders", version: b.version });
    if (/^\/windows\/[^/]+\/state$/.test(rest)) return send(200, b.state);
    if (/^\/windows\/[^/]+\/versions$/.test(rest)) {
      const { name } = JSON.parse(body);
      return name === "refused" ? send(400, { error: { code: "secret_in_code", message: "the processor contains a secret" } }) : send(200, { id: "ver1", name });
    }
    if (/^\/notifications\/[^/]+\/test$/.test(rest)) return send(200, b.notificationTest ?? { rows: [{ dedup_key: "k1", text: "hello", metadata: {} }], last_trigger_at: null });
    if (rest === "/query") {
      const q = JSON.parse(body);
      seen.queries.push(q);
      try {
        return send(200, await b.query(q, seen.queries.length));
      } catch (e) {
        return send(400, { error: { code: "sql_rejected", message: e.message } });
      }
    }
    send(404, { error: { code: "not_found", message: `no route ${req.method} ${req.url}` } });
  });
  const wss = new WebSocketServer({ noServer: true });
  server.on("upgrade", (req, socket, head) => {
    seen.upgrades.push({ url: req.url, auth: req.headers.authorization, origin: req.headers.origin });
    wss.handleUpgrade(req, socket, head, ws => {
      if (b.refuse) return ws.close(b.refuse.code, b.refuse.reason); // as live.rs does: 101, then a close frame
      seen.sockets.add(ws);
      ws.on("close", () => seen.sockets.delete(ws));
      ws.on("message", d => {
        const m = JSON.parse(d);
        seen.frames.push(m);
        if (m.type === "subscribe") ws.send(JSON.stringify({ type: "subscribed", id: m.id, watermarks: Object.fromEntries(m.triggers.map(t => [t.table, b.tables[t.table] ?? 0])) }));
      });
    });
  });
  await new Promise(r => server.listen(0, "127.0.0.1", r));
  b.url = `http://127.0.0.1:${server.address().port}`;
  b.push = frame => seen.sockets.forEach(ws => ws.send(JSON.stringify(frame)));
  b.close = () => {
    seen.sockets.forEach(ws => ws.terminate());
    server.close();
  };
  return b;
}

/** A Node host against `b` running `processor` as version v1, with everything it reports collected. */
function start(b, processor, extra = {}) {
  const out = { jsons: [], errors: [], statuses: [], notifications: [] };
  const env = runtime.nodeEnv(TOKEN, extra.oneshot);
  if (extra.renderer) env.renderer = extra.renderer;
  out.mc = runtime.host({
    api: { base: `${b.url}/api`, headers: { Authorization: `Bearer ${TOKEN}` } },
    ws: `${b.url.replace("http", "ws")}/api/ws/mission-control?org=acme`,
    org: "acme",
    window: { id: "w1", name: "Orders", version: processor && { name: "v1", processor, renderer: "<div></div>" } },
    onJson: j => out.jsons.push(j),
    onError: e => out.errors.push(e),
    onStatus: s => out.statuses.push(s),
    onNotification: n => out.notifications.push(n),
  }, env);
  return out;
}

/** A one-shot host (no init, no subscriptions) whose `call(name, args)` resolves to the tool reply. */
function tools(b, processor) {
  const waiting = new Map();
  let receive, n = 0;
  const out = start(b, processor, {
    oneshot: true,
    renderer: (_html, r) => {
      receive = r;
      return {
        send(m) {
          if (m.type === "update") out.ready = true;
          else waiting.get(m.id)(m);
        },
        destroy() {},
      };
    },
  });
  out.call = (name, args) => new Promise(resolve => {
    waiting.set(++n, resolve);
    receive({ type: "tool", id: n, name, args });
  });
  return out;
}

module.exports = { HEX, TOKEN, sleep, until, backend, start, tools, runtime };
