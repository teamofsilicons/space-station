/**
 * The Node surface the Rust CLI and `space-station-dev` drive: `run`, `tool`, `dev publish`,
 * `dev notify` and `dev serve` (host/origin checks, proxying with the bearer, the WS relay).
 */
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const os = require("node:os");
const net = require("node:net");
const WebSocket = require("ws");
const { HEX, TOKEN, sleep, until, backend } = require("./support.js");

const RUNTIME = path.join(__dirname, "..", "mission-control.js");
const PROCESSOR = `defineProcessor({
  init: j => ({ ...j, ready: true }),
  subscriptions: { s: { triggers: [{ table: "orders" }], sql: "SELECT * FROM orders", onTrigger: (j, rows) => ({ ...j, rows }) } },
  tools: { echo: { args: { text: "string" }, run: (j, { text }) => ({ text, keys: Object.keys(j) }) } },
});`;

/** Spawns the runtime and collects its output lines; `done` resolves with the exit code. */
function cli(args, env = {}, cwd) {
  const child = spawn(process.execPath, [RUNTIME, ...args], { env: { PATH: process.env.PATH, ...env }, cwd });
  const out = { stdout: [], stderr: [], kill: () => child.kill() };
  for (const s of ["stdout", "stderr"]) require("node:readline").createInterface({ input: child[s] }).on("line", l => out[s].push(l));
  out.done = new Promise(r => child.on("exit", r));
  return out;
}

function tmpdir(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ss-dev-"));
  for (const [name, text] of Object.entries(files)) fs.writeFileSync(path.join(dir, name), text);
  return dir;
}

const freePort = () => new Promise(r => {
  const s = net.createServer().listen(0, "127.0.0.1", () => {
    const { port } = s.address();
    s.close(() => r(port));
  });
});

test("run and tool", async t => {
  const b = await backend({ version: { name: "v2", processor: PROCESSOR, renderer: "<p>hi</p>" }, state: { json: { cached: 1 }, metadata: {} }, query: () => ({ rows: [{ cursor: "131" }], watermarks: { orders: 131 } }) });
  t.after(() => b.close());

  await t.test("run fetches the current version, prints each json as one line and keeps going on triggers", async () => {
    const r = cli(["run", "--url", b.url, "--org", "acme", "--window", "w9"], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    t.after(() => r.kill());
    await until(() => r.stdout.length === 2, "cached + init json");
    assert.deepEqual(r.stdout.map(JSON.parse), [{ cached: 1 }, { cached: 1, ready: true }]);
    assert.ok(b.seen.requests.some(x => x.url === "/api/orgs/acme/windows/w9" && x.auth === `Bearer ${TOKEN}`));
    const state = await until(() => b.seen.frames.find(f => f.type === "state"), "state");
    assert.deepEqual(state, { type: "state", window: "w9", version: "v2", json: { cached: 1, ready: true } });
    b.push({ type: "trigger", id: "s" });
    await until(() => r.stdout.length === 3, "trigger json");
    assert.deepEqual(JSON.parse(r.stdout[2]), { cached: 1, ready: true, rows: [{ cursor: 131 }] });
    assert.deepEqual(JSON.parse(r.stderr[0]), { status: { is_live: true, produced_at: null, connected: true } });
  });

  await t.test("tool runs one tool on the cached state and exits 0 with the result", async () => {
    const r = cli(["tool", "--url", b.url, "--org", "acme", "--window", "w9", "--name", "echo", "--args", '{"text":"yo"}'], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    assert.equal(await r.done, 0);
    assert.deepEqual(r.stdout.map(JSON.parse), [{ text: "yo", keys: ["cached"] }]);
  });

  await t.test("tool exits 1 with the error code on invalid args", async () => {
    const r = cli(["tool", "--url", b.url, "--org", "acme", "--window", "w9", "--name", "echo", "--args", '{"text":1}'], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    assert.equal(await r.done, 1);
    assert.deepEqual(JSON.parse(r.stderr[0]), { error: { code: "invalid_args", message: 'argument "text" must be string' } });
  });

  await t.test("run exits 0 after the idle period, which every trigger restarts (1 s here through SPACE_STATION_TEST_IDLE_MS)", async () => {
    const r = cli(["run", "--url", b.url, "--org", "acme", "--window", "w9"], { SPACE_STATION_ACCESS_TOKEN: TOKEN, SPACE_STATION_TEST_IDLE_MS: "1000" });
    await until(() => r.stdout.length === 2, "started");
    await sleep(300);
    const at = Date.now();
    b.push({ type: "trigger", id: "s" });
    assert.equal(await r.done, 0, r.stderr.join("\n"));
    assert.ok(Date.now() - at >= 990, "the trigger restarted the idle timer");
  });

  // The timeout is the assertion's other half: a run that loops on a refused socket never exits.
  await t.test("a run whose socket is refused exits 1 with the server's reason instead of looping", { timeout: 5000 }, async () => {
    b.refuse = { code: 4401, reason: "unknown access token" };
    const upgrades = b.seen.upgrades.length;
    const r = cli(["run", "--url", b.url, "--org", "acme", "--window", "w9"], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    t.after(() => r.kill());
    assert.equal(await r.done, 1);
    delete b.refuse;
    assert.deepEqual(r.stderr.map(JSON.parse).find(l => l.source), { source: "host", message: "mission control refused the socket: 4401 unknown access token", detail: null });
    assert.equal(b.seen.upgrades.length - upgrades, 1, "the refused socket was tried once, not once a second");
  });

  await t.test("a processor whose child dies ends the run with exit 1 and says so", async () => {
    b.version = { name: "v3", processor: "defineProcessor({ init: () => process.exit(3) })", renderer: "" };
    const r = cli(["run", "--url", b.url, "--org", "acme", "--window", "w9"], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    assert.equal(await r.done, 1);
    assert.deepEqual(r.stderr.map(JSON.parse).find(l => l.source), { source: "host", message: "processor exited with code 3", detail: null });
  });

  await t.test("a rejected credential exits 1 with the server's error", async () => {
    b.version = null;
    const r = cli(["run", "--url", b.url, "--org", "acme", "--window", "w9"], { SPACE_STATION_ACCESS_TOKEN: TOKEN });
    assert.equal(await r.done, 1);
    assert.equal(JSON.parse(r.stderr[0]).message, "this window has no published version");
    const usage = cli(["run", "--url", b.url]);
    assert.equal(await usage.done, 1);
    assert.match(usage.stderr[0], /^usage:/);
  });
});

test("dev", async t => {
  const b = await backend({ query: q => ({ rows: [{ echo: q.sql }], watermarks: {} }) });
  t.after(() => b.close());
  const env = `SPACE_STATION_URL=${b.url}\nSPACE_STATION_ACCESS_TOKEN=${TOKEN}\nSPACE_STATION_ORG=acme\nSPACE_STATION_WINDOW=w1\n`;

  await t.test("publish refuses code carrying a secret without echoing it, then posts the clean pair", async () => {
    const dir = tmpdir({ ".env": env, "processor.js": `// token: spacewindow-${HEX}\n${PROCESSOR}`, "renderer.html": "<p>hi</p>" });
    const bad = cli(["dev", "publish", "--window", "w1", "--name", "v3", "--dir", dir]);
    assert.equal(await bad.done, 1);
    assert.equal(bad.stderr[0], "processor contains a spacewindow secret; remove it before publishing");
    assert.ok(!bad.stderr.join().includes(HEX));
    assert.equal(b.seen.requests.length, 0);
    fs.writeFileSync(path.join(dir, "processor.js"), PROCESSOR);
    fs.writeFileSync(path.join(dir, "renderer.html"), `<p>sat_${"a".repeat(43)}</p>`);
    const bad2 = cli(["dev", "publish", "--window", "w1", "--name", "v3", "--dir", dir]);
    assert.equal(await bad2.done, 1);
    assert.equal(bad2.stderr[0], "renderer contains a sat secret; remove it before publishing");
    fs.writeFileSync(path.join(dir, "renderer.html"), `<!-- stk-${HEX} -->`);
    const bad3 = cli(["dev", "publish", "--window", "w1", "--name", "v3", "--dir", dir]);
    assert.equal(await bad3.done, 1);
    assert.equal(bad3.stderr[0], "renderer contains a stk secret; remove it before publishing");
    fs.writeFileSync(path.join(dir, "renderer.html"), `<p>ask_${"k".repeat(43)}</p>`);
    const bad4 = cli(["dev", "publish", "--window", "w1", "--name", "v3", "--dir", dir]);
    assert.equal(await bad4.done, 1);
    assert.equal(bad4.stderr[0], "renderer contains a ask secret; remove it before publishing");
    assert.equal(b.seen.requests.length, 0, "a silicon's token and an Application secret never leave the machine");
    fs.writeFileSync(path.join(dir, "renderer.html"), "<p>hi</p>");
    const ok = cli(["dev", "publish", "--window", "w1", "--name", "v3", "--dir", dir]);
    assert.equal(await ok.done, 0);
    assert.deepEqual(JSON.parse(ok.stdout[0]), { id: "ver1", name: "v3" });
    const post = b.seen.requests[0];
    assert.equal(post.url, "/api/orgs/acme/windows/w1/versions");
    assert.equal(post.auth, `Bearer ${TOKEN}`);
    assert.deepEqual(JSON.parse(post.body), { name: "v3", processor: PROCESSOR, renderer: "<p>hi</p>" });
  });

  await t.test("a version the server refuses is one {error} line and exit 1", async () => {
    const dir = tmpdir({ ".env": env, "processor.js": PROCESSOR, "renderer.html": "<p>hi</p>" });
    const r = cli(["dev", "publish", "--window", "w1", "--name", "refused", "--dir", dir]);
    assert.equal(await r.done, 1);
    assert.deepEqual(JSON.parse(r.stderr[0]), { error: { code: "secret_in_code", message: "the processor contains a secret" } });
  });

  await t.test("notify prints the rows and says when no trigger was seen", async () => {
    const dir = tmpdir({ ".env": env });
    const r = cli(["dev", "notify", "n1", "--dir", dir]);
    assert.equal(await r.done, 0);
    assert.deepEqual(r.stdout, ["no trigger seen yet", '{"dedup_key":"k1","text":"hello","metadata":{}}']);
    assert.equal(b.seen.requests.at(-1).url, "/api/orgs/acme/notifications/n1/test");
  });

  await t.test("notify exits 1 when its SQL failed instead of reporting a successful empty run", async () => {
    b.notificationTest = { rows: [], last_trigger_at: null, error: "unknown column amount" };
    const r = cli(["dev", "notify", "n1", "--dir", tmpdir({ ".env": env })]);
    assert.equal(await r.done, 1);
    assert.deepEqual(r.stdout, []);
    assert.deepEqual(JSON.parse(r.stderr[0]), { error: { code: "query_failed", message: "unknown column amount" } });
    delete b.notificationTest;
  });

  await t.test("dev without a complete .env says which keys it needs", async () => {
    const r = cli(["dev", "publish", "--window", "w1", "--name", "v", "--dir", tmpdir({ ".env": "SPACE_STATION_URL=http://x\n" })]);
    assert.equal(await r.done, 1);
    assert.match(r.stderr[0], /needs SPACE_STATION_URL, SPACE_STATION_ACCESS_TOKEN and SPACE_STATION_ORG$/);
  });

  await t.test("serve: host and origin checks, no .env, the page, live files, the proxy with the bearer, the WS relay", async () => {
    const dir = tmpdir({ ".env": env, "processor.js": PROCESSOR, "renderer.html": "<p>hi</p>" });
    const port = await freePort();
    const s = cli(["dev", "serve", "--port", String(port), "--dir", dir]);
    t.after(() => s.kill());
    await until(() => s.stderr.some(l => l.includes(`http://127.0.0.1:${port}`)), "serve banner");
    const origin = `http://127.0.0.1:${port}`;
    // node:http rather than fetch: fetch refuses to send a caller-chosen Host header.
    const get = (p, headers = {}, method = "GET", body) => new Promise((resolve, reject) => {
      const req = http.request({ host: "127.0.0.1", port, path: p, method, headers: { host: `127.0.0.1:${port}`, ...headers } }, res => {
        let text = "";
        res.on("data", c => (text += c));
        res.on("end", () => resolve({ status: res.statusCode, text, json: JSON.parse.bind(null, text) }));
      });
      req.on("error", reject);
      req.end(body);
    });

    assert.equal((await get("/", { host: "evil.example:80" })).status, 403, "bad Host");
    assert.equal((await get("/", { host: `localhost:${port + 1}` })).status, 403, "other port");
    assert.equal((await get("/.env")).status, 404);
    assert.equal((await get("/..%2F.env")).status, 404);
    assert.equal((await get("/api/orgs/acme/query", {}, "POST", "{}")).status, 403, "mutating without Origin");
    assert.equal((await get("/api/orgs/acme/query", { origin: "http://evil.example" }, "POST", "{}")).status, 403, "wrong Origin");

    const page = (await get("/")).text;
    assert.match(page, /SpaceStation\.host\(/);
    assert.match(page, /"acme"/);
    assert.match(page, /"w1"/);
    assert.ok(!page.includes(TOKEN) && !page.includes(HEX), "the token is never on the page");
    assert.equal((await get("/mission-control.js")).text, fs.readFileSync(RUNTIME, "utf8"));
    assert.equal((await get("/processor.js")).text, PROCESSOR);
    assert.equal((await get("/renderer.html")).text, "<p>hi</p>");
    const v1 = (await get("/version")).text;
    fs.utimesSync(path.join(dir, "renderer.html"), new Date(), new Date(Date.now() + 5000));
    assert.notEqual((await get("/version")).text, v1, "version follows mtime");

    const tables = await get("/api/orgs/acme/tables");
    assert.equal(tables.status, 200);
    assert.deepEqual(tables.json().map(x => x.id), ["orders"]);
    const q = await get("/api/orgs/acme/query", { origin, "content-type": "application/json" }, "POST", JSON.stringify({ sql: "SELECT 7" }));
    assert.deepEqual(q.json(), { rows: [{ echo: "SELECT 7" }], watermarks: {} });
    assert.ok(b.seen.requests.every(r => r.auth === `Bearer ${TOKEN}`), "the proxy injects the bearer");
    assert.equal((await get("/api/orgs/acme/nothing")).status, 404, "backend status passes through");

    const rejected = new WebSocket(`ws://127.0.0.1:${port}/api/ws/mission-control?org=acme`, { headers: { origin: "http://evil.example" } });
    await new Promise(r => rejected.on("error", r));
    const ws = new WebSocket(`ws://127.0.0.1:${port}/api/ws/mission-control?org=acme`, { headers: { origin } });
    await new Promise(r => ws.on("open", r));
    ws.send(JSON.stringify({ type: "subscribe", id: "s", triggers: [{ table: "orders" }] }));
    const reply = await new Promise(r => ws.once("message", d => r(JSON.parse(d))));
    assert.deepEqual(reply, { type: "subscribed", id: "s", watermarks: { orders: 130 } });
    assert.deepEqual(b.seen.upgrades.at(-1), { url: "/api/ws/mission-control?org=acme", auth: `Bearer ${TOKEN}`, origin: undefined });
    ws.close();
  });
});
