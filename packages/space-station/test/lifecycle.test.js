/**
 * The processor lifecycle end to end: a real Node host, a real `--permission` child and a fake
 * backend. Every assertion is about what the backend received or what the host reported.
 */
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { HEX, TOKEN, sleep, until, backend, start } = require("./support.js");

const ORDERS = `
const load = async (json) => { json.seeded = await mission_control.query("SELECT 1"); return json; };
export default defineProcessor({
  init: load,
  subscriptions: {
    sub1: {
      triggers: [{ table: "orders", where: "record.price::Float64 > 5" }],
      sql: "SELECT * FROM orders", mode: "delta",
      onTrigger: (json, rows) => { if (rows[0]?.boom) throw new Error("boom"); json.rows = (json.rows ?? []).concat(rows); json.runs = (json.runs ?? 0) + 1; return json; },
    },
    sub2: { triggers: [{ table: "orders" }], sql: "SELECT count() FROM orders", mode: "snapshot", onTrigger: (json, rows) => ({ ...json, count: rows, runs: (json.runs ?? 0) + 1 }) },
  },
});`;

/** Answers queries from a script of `{rows, watermarks}` and can hold a response until released. */
function scripted() {
  const held = [];
  const q = { answers: [], hold: false, release: () => held.splice(0).forEach(r => r()) };
  q.handler = async () => {
    if (q.hold) await new Promise(r => held.push(r));
    return q.answers.shift() ?? { rows: [], watermarks: {} };
  };
  return q;
}

test("lifecycle", async t => {
  const q = scripted();
  const b = await backend({ tables: { orders: 130, users: 7 }, state: { json: { cached: true }, metadata: { produced_at: "2026-09-03T00:00:00Z", is_live: false } }, query: q.handler });
  t.after(() => b.close());

  await t.test("init queries carry restrict {to: watermark}, rows are coerced, and the result replaces the cached json", async () => {
    q.answers.push({ rows: [{ cursor: "131", event_ts_ms: "1725000000000", registered_ts_ms: "1725000000001", record: { a: 1 } }, { cursor: "9007199254740993", record: { exact: true } }], watermarks: { orders: 130 } });
    const h = start(b, ORDERS);
    t.after(() => h.mc.destroy());
    await until(() => h.jsons.length === 2, "init json");
    assert.deepEqual(h.jsons[0], { cached: true });
    assert.deepEqual(h.jsons[1], { cached: true, seeded: [{ cursor: 131, event_ts_ms: 1725000000000, registered_ts_ms: 1725000000001, record: { a: 1 } }, { cursor: "9007199254740993", record: { exact: true } }] });
    assert.deepEqual(b.seen.queries[0], { sql: "SELECT 1", restrict: { orders: { to: 130 } } });
    assert.ok(b.seen.requests.every(r => r.auth === `Bearer ${TOKEN}`), "every HTTP call carries the bearer");
    assert.equal(b.seen.upgrades[0].auth, `Bearer ${TOKEN}`);
    assert.equal(b.seen.upgrades[0].url, "/api/ws/mission-control?org=acme");
    Object.assign(t, { h });
  });

  const h = () => t.h;

  await t.test("subscribe frames go out in definition order and the state frame carries window, version and json", async () => {
    await until(() => b.seen.frames.filter(f => f.type === "state").length, "state frame");
    const subs = b.seen.frames.filter(f => f.type === "subscribe");
    assert.deepEqual(subs, [
      { type: "subscribe", id: "sub1", triggers: [{ table: "orders", where: "record.price::Float64 > 5" }] },
      { type: "subscribe", id: "sub2", triggers: [{ table: "orders" }] },
    ]);
    const state = b.seen.frames.find(f => f.type === "state");
    assert.deepEqual(state, { type: "state", window: "w1", version: "v1", json: h().jsons[1] });
    assert.deepEqual(h().statuses.at(-1), { is_live: true, produced_at: "2026-09-03T00:00:00Z", connected: true });
  });

  await t.test("a trigger runs a delta query from the cursor; the result replaces json and advances the cursor", async () => {
    q.answers.push({ rows: [{ cursor: "131", record: { price: 9 } }], watermarks: { orders: 135 } });
    b.push({ type: "trigger", id: "sub1" });
    await until(() => h().jsons.length === 3, "trigger json");
    assert.deepEqual(b.seen.queries[1], { sql: "SELECT * FROM orders", restrict: { orders: { from: 130 } } });
    assert.deepEqual(h().jsons[2].rows, [{ cursor: 131, record: { price: 9 } }]);
    q.answers.push({ rows: [], watermarks: { orders: 135 } });
    b.push({ type: "trigger", id: "sub1" });
    await until(() => b.seen.queries.length === 3, "second delta");
    assert.deepEqual(b.seen.queries[2].restrict, { orders: { from: 135 } });
    await until(() => h().jsons.length === 4, "second json");
  });

  await t.test("a snapshot subscription queries without restrict and leaves the other subscription's cursor alone", async () => {
    q.answers.push({ rows: [{ "count()": "2" }], watermarks: { orders: 140 } });
    b.push({ type: "trigger", id: "sub2" });
    await until(() => h().jsons.length === 5, "snapshot json");
    assert.deepEqual(b.seen.queries[3], { sql: "SELECT count() FROM orders" });
    assert.deepEqual(h().jsons[4].count, [{ "count()": "2" }]);
    q.answers.push({ rows: [], watermarks: { orders: 140 } });
    b.push({ type: "trigger", id: "sub1" });
    await until(() => b.seen.queries.length === 5, "delta after snapshot");
    assert.deepEqual(b.seen.queries[4].restrict, { orders: { from: 135 } }, "sub1 resumes from its own cursor, not sub2's");
    await until(() => h().jsons.length === 6, "json after snapshot");
  });

  await t.test("a throwing onTrigger leaves json and cursors alone and yields a dev_error", async () => {
    q.answers.push({ rows: [{ boom: true }], watermarks: { orders: 150 } });
    b.push({ type: "trigger", id: "sub1" });
    const err = await until(() => h().errors.find(e => /boom/.test(e.message)), "dev_error");
    assert.equal(err.source, "processor");
    assert.match(err.message, /^sub1: boom/);
    assert.match(err.detail, /Error: boom/);
    assert.equal(h().jsons.length, 6);
    q.answers.push({ rows: [], watermarks: { orders: 150 } });
    b.push({ type: "trigger", id: "sub1" });
    await until(() => b.seen.queries.length === 7, "delta after throw");
    assert.deepEqual(b.seen.queries[6].restrict, { orders: { from: 140 } }, "cursor did not move");
    await until(() => h().jsons.length === 7, "json after throw");
  });

  await t.test("pings that arrive while a subscription runs coalesce into a single further run", async () => {
    q.hold = true;
    b.push({ type: "trigger", id: "sub1" });
    await until(() => b.seen.queries.length === 8, "held query");
    b.push({ type: "trigger", id: "sub1" });
    b.push({ type: "trigger", id: "sub1" });
    b.push({ type: "trigger", id: "sub1" });
    await sleep(50);
    q.release();
    await until(() => b.seen.queries.length === 9, "the one coalesced run");
    q.release();
    await sleep(100);
    assert.equal(b.seen.queries.length, 9);
    q.hold = false;
    q.release();
    await until(() => h().jsons.length === 9, "two more jsons");
  });

  await t.test("when several subscriptions are ready the one higher in the definition runs first", async () => {
    q.hold = true;
    b.push({ type: "trigger", id: "sub2" });
    await until(() => b.seen.queries.length === 10, "held sub2 query");
    b.push({ type: "trigger", id: "sub2" });
    b.push({ type: "trigger", id: "sub1" });
    await sleep(50);
    q.hold = false;
    q.release();
    await until(() => b.seen.queries.length === 12, "both pending runs");
    assert.deepEqual(b.seen.queries.slice(9).map(x => x.sql), ["SELECT count() FROM orders", "SELECT * FROM orders", "SELECT count() FROM orders"]);
    await until(() => h().jsons.length === 12, "three more jsons");
  });

  await t.test("state frames are coalesced to one per second after several changes", async () => {
    const before = b.seen.frames.filter(f => f.type === "state").length;
    for (let i = 0; i < 5; i++) b.push({ type: "trigger", id: "sub1" });
    await until(() => h().jsons.length >= 13, "burst json");
    await sleep(1300);
    const states = b.seen.frames.filter(f => f.type === "state").slice(before);
    assert.ok(states.length >= 1 && states.length <= 2, `got ${states.length} state frames for a burst`);
    assert.ok(states.every(s => "json" in s), "changed json travels with the state");
  });

  await t.test("after the socket drops the host reconnects, re-subscribes, sends its json again and the processor catches up from its cursor", async () => {
    b.tables.orders = 170;
    const queries = b.seen.queries.length, last = h().jsons.at(-1);
    b.seen.sockets.forEach(ws => ws.close(1012, "restart"));
    await until(() => h().statuses.at(-1)?.connected === false, "disconnected status");
    assert.equal(h().statuses.at(-1).is_live, false);
    await until(() => b.seen.upgrades.length === 2, "reconnect", 4000);
    await until(() => b.seen.frames.filter(f => f.type === "subscribe" && f.id === "sub2").length === 2, "re-subscribe");
    const resubscribed = b.seen.frames.findLastIndex(f => f.type === "subscribe" && f.id === "sub2");
    await until(() => b.seen.frames[resubscribed + 1], "state after re-subscribe");
    assert.deepEqual(b.seen.frames[resubscribed + 1], { type: "state", window: "w1", version: "v1", json: last }, "the new socket gets the json the old one may have missed");
    await until(() => b.seen.queries.length >= queries + 2, "catch-up runs");
    const catchUp = b.seen.queries.slice(queries);
    assert.deepEqual(catchUp[0], { sql: "SELECT * FROM orders", restrict: { orders: { from: 150 } } }, "delta catch-up from the cursor");
    assert.deepEqual(catchUp[1], { sql: "SELECT count() FROM orders" }, "snapshot catch-up");
    assert.equal(h().statuses.at(-1).connected, true);
  });
});

/** The canonical pair of apps/web/docs/space-windows.md — a delta and a snapshot on one table. */
const PAIR = `export default defineProcessor({
  subscriptions: {
    recent: {
      triggers: [{ table: "orders", where: "record.amount::Float64 > 5" }],
      sql: "SELECT record.id::String AS id FROM orders", mode: "delta",
      onTrigger: (json, rows) => ({ ...json, recent: (json.recent ?? []).concat(rows.map(r => r.id)) }),
    },
    totals: {
      triggers: [{ table: "orders" }],
      sql: "SELECT count() AS n FROM orders", mode: "snapshot",
      onTrigger: (json, rows) => ({ ...json, n: rows[0].n, seen: { ...mission_control.data.tables.orders } }),
    },
  },
});`;

test("two subscriptions on one table", async t => {
  const rows = [{ cursor: 131, id: "a", amount: 1 }, { cursor: 132, id: "b", amount: 9 }];
  let watermark = 130;
  const b = await backend({
    tables: { orders: 130 },
    query: q => ({
      rows: q.restrict ? rows.filter(r => r.cursor > q.restrict.orders.from && r.cursor <= watermark) : [{ n: rows.filter(r => r.cursor <= watermark).length }],
      watermarks: { orders: watermark },
    }),
  });
  t.after(() => b.close());
  const h = start(b, PAIR);
  t.after(() => h.mc.destroy());

  await t.test("the delta subscription sees every row exactly once even when the snapshot one runs without it", async () => {
    await until(() => b.seen.frames.filter(f => f.type === "subscribe").length === 2, "both subscribed");
    watermark = 131; // "a" does not match recent's where, so only totals is triggered
    b.push({ type: "trigger", id: "totals" });
    await until(() => h.jsons.at(-1).n === 1, "totals after the first flush");
    watermark = 132; // "b" matches, both are triggered
    b.push({ type: "trigger", id: "recent" });
    b.push({ type: "trigger", id: "totals" });
    await until(() => h.jsons.at(-1).n === 2 && h.jsons.at(-1).recent, "both after the second flush");
    assert.deepEqual(h.jsons.at(-1).recent, ["a", "b"], "the row the snapshot run passed over is still delivered");
    assert.deepEqual(b.seen.queries.filter(q => q.restrict).map(q => q.restrict), [{ orders: { from: 130 } }], "the delta ran once, from its own cursor");
    assert.deepEqual(h.errors, []);
  });

  await t.test("data.tables[t] is the per-table view: the cursor every subscription has passed and the highest watermark", () => {
    assert.deepEqual(h.jsons.at(-1).seen, { cursor: 131, watermark: 132 }, "recent had reached 132, totals only 131");
  });
});

test("unchanged JSON is emitted only once while subscription cursors continue to advance", async t => {
  const b = await backend({ query: () => ({ rows: [], watermarks: { orders: 131 } }) });
  const h = start(b, `defineProcessor({ init: j => j, subscriptions: {
    s: { triggers: [{ table: "orders" }], sql: "SELECT * FROM orders", onTrigger: j => j }
  } })`);
  t.after(() => { h.mc.destroy(); b.close(); });
  await until(() => b.seen.frames.some(f => f.type === "subscribe"), "subscribed");
  b.push({ type: "trigger", id: "s" });
  await until(() => b.seen.queries.length === 1, "first query");
  await sleep(30);
  b.push({ type: "trigger", id: "s" });
  await until(() => b.seen.queries.length === 2, "second query");
  assert.deepEqual(b.seen.queries[1].restrict, { orders: { from: 131 } });
  assert.deepEqual(h.jsons, [{}]);
});

test("guards", async t => {
  const b = await backend({ query: () => ({ rows: [], watermarks: { orders: 130 } }) });
  t.after(() => b.close());

  await t.test("default export works after a same-line helper and preserves export text in literals", async () => {
    const h = start(b, `const label = "export default text"; const pattern = /export default /; let runs = 0; const subscriptions = {}; export default defineProcessor({ subscriptions, init: () => ({ label, pattern: pattern.source, runs: ++runs }) });`);
    t.after(() => h.mc.destroy());
    await until(() => h.jsons.length === 2 || h.errors.length, "inline module loaded");
    assert.deepEqual(h.errors, []);
    assert.deepEqual(h.jsons.at(-1), { label: "export default text", pattern: "export default ", runs: 1 });
  });

  await t.test("an init result over 64 KB is refused, json stays seeded and nothing is subscribed", async () => {
    const h = start(b, `defineProcessor({ init: () => ({ big: "x".repeat(65536) }), subscriptions: { s: { triggers: [{ table: "orders" }], sql: "SELECT 1", onTrigger: j => j } } })`);
    t.after(() => h.mc.destroy());
    const err = await until(() => h.errors[0], "dev_error");
    assert.match(err.message, /^init returned 655\d\d bytes, the limit is 65536$/);
    assert.deepEqual(h.jsons, [{}]);
    await sleep(100);
    assert.equal(b.seen.frames.filter(f => f.type === "subscribe").length, 0);
  });

  await t.test("an onTrigger result over 64 KB, or not an object, is a dev_error that changes nothing", async () => {
    const h = start(b, `defineProcessor({ subscriptions: { s: { triggers: [{ table: "orders" }], sql: "SELECT 1", onTrigger: (j, rows) => rows.length ? ["not", "an", "object"] : { big: "x".repeat(65536) } } } })`);
    t.after(() => h.mc.destroy());
    await until(() => b.seen.frames.some(f => f.type === "subscribe"), "subscribed");
    b.push({ type: "trigger", id: "s" });
    await until(() => h.errors.length === 1, "oversize dev_error");
    assert.match(h.errors[0].message, /^s: s\.onTrigger returned 655\d\d bytes/);
    b.query = () => ({ rows: [{ cursor: "131" }], watermarks: { orders: 131 } });
    b.push({ type: "trigger", id: "s" });
    await until(() => h.errors.length === 2, "non-object dev_error");
    assert.equal(h.errors[1].message, "s: s.onTrigger must return a JSON object");
    assert.deepEqual(h.jsons, [{}]);
    b.query = () => ({ rows: [], watermarks: { orders: 131 } });
  });

  await t.test("onTrigger is cut off at the timeout (50 ms here through SPACE_STATION_TEST_TIMEOUT_MS)", async () => {
    process.env.SPACE_STATION_TEST_TIMEOUT_MS = "50";
    const h = start(b, `defineProcessor({ subscriptions: { slow: { triggers: [{ table: "orders" }], sql: "SELECT 1", onTrigger: () => new Promise(() => {}) } } })`);
    delete process.env.SPACE_STATION_TEST_TIMEOUT_MS;
    t.after(() => h.mc.destroy());
    await until(() => b.seen.frames.some(f => f.type === "subscribe" && f.id === "slow"), "subscribed");
    const at = Date.now();
    b.push({ type: "trigger", id: "slow" });
    const err = await until(() => h.errors[0], "timeout dev_error");
    assert.equal(err.message, "slow: slow.onTrigger timed out after 50 ms");
    assert.ok(Date.now() - at < 2000);
  });

  await t.test("a tool called while the processor is still loading runs after init, on the initialised json, without disturbing the load", async () => {
    const replies = [];
    let asked = false;
    const h = start(b, `defineProcessor({ init: async j => ({ ...j, ready: await mission_control.query("SELECT 1") }), tools: { peek: { args: {}, run: j => Object.keys(j) } } })`, {
      renderer: (_html, receive) => ({
        destroy() {},
        send(m) {
          if (m.type !== "update") return replies.push(m);
          if (asked) return;
          asked = true;
          receive({ type: "tool", id: 1, name: "peek", args: {} }); // id 1 is also the processor's first pending request
        },
      }),
    });
    t.after(() => h.mc.destroy());
    assert.deepEqual(await until(() => replies[0], "tool reply"), { type: "tool_result", id: 1, result: ["ready"] });
    assert.equal(h.jsons.length, 2, "init finished first");
    assert.deepEqual(h.errors, []);
  });

  await t.test("a module that never calls defineProcessor, or does not parse, is reported with the code's error", async () => {
    const h1 = start(b, "const x = 1;");
    const h2 = start(b, "export default defineProcessor({");
    t.after(() => { h1.mc.destroy(); h2.mc.destroy(); });
    assert.equal((await until(() => h1.errors[0], "no defineProcessor")).message, "processor code never called defineProcessor");
    assert.match((await until(() => h2.errors[0], "syntax error")).message, /^processor code: /);
  });

  await t.test("a query the backend refuses rejects mission_control.query with the server's code and message", async () => {
    b.query = () => { throw new Error("FROM must name a table of this org"); };
    const h = start(b, `defineProcessor({ init: async j => { try { await mission_control.query("SELECT * FROM system.tables"); } catch (e) { j.code = e.code; j.message = e.message; } return j; } })`);
    t.after(() => h.mc.destroy());
    await until(() => h.jsons.length === 2, "init done");
    assert.deepEqual(h.jsons[1], { code: "sql_rejected", message: "FROM must name a table of this org" });
    assert.equal(h.errors.length, 0, "a caught query error is not a dev error");
  });
});

test("sandbox", async t => {
  const b = await backend();
  t.after(() => b.close());

  await t.test("SANDBOX is exactly allow-scripts", () => {
    assert.equal(require("../mission-control.js").SANDBOX, "allow-scripts");
  });

  await t.test("the processor child sees no environment and no token, has no fetch/WebSocket or stdout, and cannot read files or spawn", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ss-secret-"));
    fs.writeFileSync(path.join(dir, ".env"), `SPACE_STATION_ACCESS_TOKEN=${TOKEN}\n`);
    const h = start(b, `defineProcessor({ init: () => {
      const tryIt = fn => { try { fn(); return "allowed"; } catch (e) { return e.code; } };
      const fs = process.getBuiltinModule("node:fs");
      return {
        env: Object.keys(process.env).filter(k => !k.startsWith("__CF_")),
        tokenVisible: JSON.stringify([process.argv, process.execArgv, process.title]).includes(${JSON.stringify(HEX)}),
        fetch: typeof fetch, ws: typeof WebSocket, stdoutIsStderr: process.stdout === process.stderr,
        dotenv: tryIt(() => fs.readFileSync(${JSON.stringify(path.join(dir, ".env"))})),
        etc: tryIt(() => fs.readFileSync("/etc/hosts")),
        home: tryIt(() => fs.readdirSync(${JSON.stringify(os.homedir())})),
        spawn: tryIt(() => process.getBuiltinModule("node:child_process").execSync("true")),
      };
    } })`);
    t.after(() => h.mc.destroy());
    await until(() => h.jsons.length === 2, "init json");
    assert.deepEqual(h.jsons[1], {
      env: [], tokenVisible: false, fetch: "undefined", ws: "undefined", stdoutIsStderr: true,
      dotenv: "ERR_ACCESS_DENIED", etc: "ERR_ACCESS_DENIED", home: "ERR_ACCESS_DENIED", spawn: "ERR_ACCESS_DENIED",
    });
  });

  await t.test("a definition with a broken subscription or tool is refused with its name before anything runs", async () => {
    const h1 = start(b, `defineProcessor({ subscriptions: { s: { triggers: [{ table: "orders" }], sql: "SELECT 1" } } })`);
    const h2 = start(b, `defineProcessor({ tools: { t: { args: {} } } })`);
    t.after(() => { h1.mc.destroy(); h2.mc.destroy(); });
    assert.equal((await until(() => h1.errors[0], "subscription error")).message, 'subscription "s" needs triggers, sql and onTrigger');
    assert.equal((await until(() => h2.errors[0], "tool error")).message, 'tool "t" needs run');
    assert.equal(b.seen.frames.filter(f => f.type === "subscribe").length, 0);
  });
});

test("connection", async t => {
  await t.test("a 4401 close is reported with the server's reason and is not retried", async () => {
    const b = await backend();
    t.after(() => b.close());
    const h = start(b, "defineProcessor({})");
    t.after(() => h.mc.destroy());
    await until(() => h.statuses.at(-1)?.connected === true, "connected");
    b.seen.sockets.forEach(ws => ws.close(4401, "log in")); // live.rs, once the credential has gone bad
    const err = await until(() => h.errors[0], "dev error");
    assert.deepEqual(err, { source: "host", message: "mission control refused the socket: 4401 log in", detail: null });
    assert.equal(h.statuses.at(-1).is_live, false);
    await sleep(1200);
    assert.equal(b.seen.upgrades.length, 1, "a refused credential is not retried once a second");
  });

  await t.test("a socket that closes before it carries a frame doubles the backoff", async () => {
    const b = await backend();
    b.refuse = { code: 1012, reason: "restart" };
    t.after(() => b.close());
    const h = start(b, "defineProcessor({})");
    t.after(() => h.mc.destroy());
    const at = [];
    for (let n = 1; n <= 3; n++) {
      await until(() => b.seen.upgrades.length === n, `upgrade ${n}`, 6000);
      at.push(Date.now());
    }
    assert.ok(at[2] - at[1] > 1800, `waited ${at[2] - at[1]} ms before the third attempt, not the doubled 2 s`);
    assert.deepEqual(h.errors, [], "a transport close is not a dev error");
  });

  await t.test("a notification frame for this actor reaches onNotification", async () => {
    const b = await backend();
    t.after(() => b.close());
    const h = start(b, "defineProcessor({})");
    t.after(() => h.mc.destroy());
    await until(() => h.statuses.at(-1)?.connected === true, "connected");
    const frame = { type: "notification", event_id: 7, notification: "n1", name: "New order", dedup_key: "k1", text: "hello", metadata: { amount: 9 }, fired_at: "2026-09-03T00:00:00Z" };
    b.push(frame);
    assert.deepEqual(await until(() => h.notifications[0], "notification"), frame);
    assert.deepEqual(h.errors, []);
  });
});
