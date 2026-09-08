/**
 * The browser side without a browser: the renderer bridge and the browser host run in a vm
 * context with just enough DOM (iframes, message events, fetch, WebSocket) to prove what the page
 * and the two sandboxes exchange, and what a renderer sees in `mission_control`.
 */
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { sleep, until, backend } = require("./support.js");
const { inject, SANDBOX } = require("../mission-control.js");

/** Values made inside a vm context have foreign prototypes; strict deep equality wants plain ones. */
const plain = v => JSON.parse(JSON.stringify(v));

/** The bridge alone, against a fake window whose `parent` records what it posts. */
function bridge(Vue) {
  const posted = [], handlers = {};
  const w = { parent: { postMessage: m => posted.push(m) }, addEventListener: (ev, cb) => (handlers[ev] = cb), Vue };
  w.window = w;
  vm.runInNewContext(/<script>(.*)<\/script>/s.exec(inject(""))[1], w);
  return { mc: () => w.mission_control, posted, from: (type, data) => handlers[type]({ source: w.parent, ...data }) };
}

test("renderer bridge", async t => {
  await t.test("inject puts the bridge after <head>, else after the doctype, else first, and never mistakes <header> for <head>", () => {
    assert.match(inject("<p>hi</p>"), /^<script>\(function rendererBridge\(\) \{.*\}\)\(\)<\/script><p>hi<\/p>$/s);
    assert.match(inject("<!doctype html><html><body></body></html>"), /^<!doctype html><script>/);
    assert.match(inject("<!doctype html><html><head><meta charset=utf-8></head></html>"), /<head><script>.*<\/script><meta charset=utf-8>/s);
    assert.match(inject('<header id="h">x</header><script>1</script>'), /^<script>.*<\/script><header id="h">x<\/header>/s);
  });

  await t.test("announces itself, mirrors json/metadata/stale/tools from update, and calls listeners (late ones at once)", () => {
    const r = bridge();
    assert.deepEqual(plain(r.posted), [{ type: "ready" }]);
    assert.equal(r.mc().stale, true);
    const seen = [];
    r.mc().on("update", (json, meta) => seen.push([json, meta]));
    r.mc().on("resize", () => seen.push("never"));
    r.from("message", { data: { type: "update", json: { a: 1 }, metadata: { is_live: true, produced_at: "t" }, tools: { echo: { text: "string" } } } });
    assert.deepEqual(r.mc().json, { a: 1 });
    assert.deepEqual(r.mc().metadata, { is_live: true, produced_at: "t" });
    assert.equal(r.mc().stale, false);
    assert.equal(typeof r.mc().tools.echo, "function");
    assert.deepEqual(seen, [[{ a: 1 }, { is_live: true, produced_at: "t" }]]);
    r.mc().on("update", json => seen.push(["late", json]));
    assert.deepEqual(seen.at(-1), ["late", { a: 1 }]);
    r.from("message", { source: {}, data: { type: "update", json: { stranger: true }, metadata: {}, tools: {} } });
    assert.deepEqual(r.mc().json, { a: 1 }, "a message from anyone but the parent is ignored");
    r.from("message", { data: { type: "update", json: {}, metadata: { is_live: false }, tools: {} } });
    assert.equal(r.mc().stale, true);
    assert.equal(seen.length, 4);
  });

  await t.test("tools are promises over tool / tool_result / tool_error with ids", async () => {
    const r = bridge();
    r.from("message", { data: { type: "update", json: {}, metadata: {}, tools: { echo: { text: "string" } } } });
    const ok = r.mc().tools.echo({ text: "x" }), bad = r.mc().tools.echo({});
    assert.deepEqual(plain(r.posted.slice(1)), [{ type: "tool", id: 1, name: "echo", args: { text: "x" } }, { type: "tool", id: 2, name: "echo", args: {} }]);
    r.from("message", { data: { type: "tool_result", id: 1, result: { ok: 1 } } });
    r.from("message", { data: { type: "tool_error", id: 2, code: "invalid_args", message: 'missing argument "text"' } });
    assert.deepEqual(await ok, { ok: 1 });
    await assert.rejects(bad, { code: "invalid_args", message: 'missing argument "text"' });
  });

  await t.test("mount creates a Vue app whose scope holds the reactive mission_control and a stale computed", () => {
    let app;
    const Vue = { reactive: o => new Proxy(o, {}), computed: fn => ({ get value() { return fn(); } }), createApp: def => ({ mount: sel => (app = { sel, scope: def.setup() }) }) };
    const r = bridge(Vue);
    const before = r.mc();
    r.mc().mount("#app");
    assert.equal(app.sel, "#app");
    assert.equal(app.scope.mission_control, r.mc());
    assert.notEqual(r.mc(), before, "mission_control is the reactive proxy after mount");
    r.from("message", { data: { type: "update", json: { n: 2 }, metadata: { is_live: true }, tools: {} } });
    assert.deepEqual(app.scope.mission_control.json, { n: 2 });
    assert.equal(app.scope.stale.value, false);
    assert.equal(bridge().mc().mount("#app"), undefined, "without Vue, mount does nothing");
  });

  await t.test("errors and unhandled rejections inside the renderer become renderer dev errors", () => {
    const r = bridge();
    r.from("error", { message: "boom", error: { stack: "Error: boom\n    at x" } });
    r.from("error", { target: { src: "https://cdn.example/x.js" } });
    r.from("unhandledrejection", { reason: new Error("rej") });
    assert.deepEqual(plain(r.posted.slice(1)).map(e => [e.source, e.message]), [["renderer", "boom"], ["renderer", "failed to load https://cdn.example/x.js"], ["renderer", "rej"]]);
    assert.equal(r.posted[1].detail, "Error: boom\n    at x");
  });
});

/** A DOM that is exactly what `SpaceStation.host` touches: iframes under `mount`, message events, fetch and WebSocket. */
function page(b) {
  const frames = [], listeners = new Set();
  const w = {
    location: { href: "http://app.test/o/acme/windows/w1" },
    document: {
      createElement() {
        const f = { attrs: {}, sent: [], setAttribute: (k, v) => (f.attrs[k] = v), contentWindow: { postMessage: m => f.sent.push(m) } };
        f.remove = () => frames.includes(f) && frames.splice(frames.indexOf(f), 1);
        return f;
      },
    },
    mount: { appendChild: f => frames.push(f) },
    addEventListener: (ev, cb) => ev === "message" && listeners.add(cb),
    removeEventListener: (ev, cb) => listeners.delete(cb),
    fetch: (url, init) => fetch(b.url + url, init),
    WebSocket, URL, setTimeout, clearTimeout,
    frames, deliver: (source, data) => listeners.forEach(cb => cb({ source, data })),
  };
  w.window = w;
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "..", "mission-control.js"), "utf8"), w);
  return w;
}

test("browser host", async t => {
  const b = await backend({ state: { json: { cached: 1 }, metadata: { produced_at: "2026-09-03T00:00:00Z", is_live: false } } });
  t.after(() => b.close());
  const w = page(b), out = { jsons: [], statuses: [], errors: [] };
  const renderer = "<p>hi</p><script>mission_control.mount('#x')</script>";
  const mc = w.SpaceStation.host({
    runtimeUrl: "/mission-control.js",
    api: { base: "/api" },
    ws: `${b.url.replace("http", "ws")}/api/ws/mission-control?org=acme`,
    org: "acme",
    window: { id: "w1", name: "Orders", version: { name: "v1", processor: "defineProcessor({})", renderer } },
    mount: w.mount,
    onJson: j => out.jsons.push(j),
    onStatus: s => out.statuses.push(s),
    onError: e => out.errors.push(e),
  });
  t.after(() => mc.destroy());

  await t.test("appends a visible renderer iframe and a hidden processor iframe to mount, sandbox=allow-scripts, bridge and CSP in place", async () => {
    await until(() => w.frames.length === 2, "two iframes");
    assert.equal(w.SpaceStation.SANDBOX, SANDBOX);
    assert.deepEqual(w.frames.map(f => [f.attrs.sandbox, f.hidden]), [["allow-scripts", false], ["allow-scripts", true]]);
    assert.equal(w.frames[0].srcdoc, inject(renderer));
    assert.equal(w.frames[1].srcdoc, `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src http://app.test 'unsafe-inline' 'unsafe-eval'"><script src="http://app.test/mission-control.js"></script><script>SpaceStation.processorRole()</script>`);
    assert.deepEqual(plain(out.jsons), [{ cached: 1 }], "the cached state is reported before anything runs");
    assert.deepEqual(b.seen.requests.map(r => r.url), ["/api/orgs/acme/windows/w1/state"], "a window that brings its version needs no other fetch");
  });

  await t.test("accepts a message only from the frame it belongs to and relays each side's verbs", async () => {
    const [rend, proc] = w.frames;
    await until(() => out.statuses.at(-1)?.connected, "connected");
    w.deliver({}, { type: "ready" });
    w.deliver(rend.contentWindow, { type: "ready" });
    assert.equal(proc.sent.length, 0, "a stranger's ready loads nothing");
    assert.equal(rend.sent.length, 0, "the renderer hears nothing before the processor is loaded");
    w.deliver(proc.contentWindow, { type: "ready" });
    assert.deepEqual(plain(proc.sent), [{ type: "load", code: "defineProcessor({})", json: { cached: 1 }, timeout: 10000 }]);
    w.deliver(proc.contentWindow, { type: "loaded", tools: { echo: { text: "string" } } });
    assert.deepEqual(plain(rend.sent), [{ type: "update", json: { cached: 1 }, metadata: { processor_version: "v1", renderer_version: "v1", produced_at: "2026-09-03T00:00:00Z", is_live: true }, tools: { echo: { text: "string" } } }]);
    w.deliver(proc.contentWindow, { type: "json", json: { n: 1 } });
    assert.deepEqual(plain(out.jsons.at(-1)), { n: 1 });
    assert.deepEqual(plain(rend.sent.at(-1).json), { n: 1 });
    w.deliver(proc.contentWindow, { type: "subscribe", id: "s", triggers: [{ table: "orders" }] });
    assert.deepEqual(plain(await until(() => proc.sent.find(m => m.type === "subscribed"), "subscribed relayed")), { type: "subscribed", id: "s", watermarks: { orders: 130 } });
    w.deliver(proc.contentWindow, { type: "state", json: { n: 1 } });
    assert.deepEqual(await until(() => b.seen.frames.find(f => f.type === "state"), "state frame"), { type: "state", window: "w1", version: "v1", json: { n: 1 } });
    w.deliver(rend.contentWindow, { type: "tool", id: 7, name: "echo", args: { text: "x" } });
    assert.deepEqual(plain(proc.sent.at(-1)), { type: "tool", id: 7, name: "echo", args: { text: "x" } });
    w.deliver(proc.contentWindow, { type: "tool_result", id: 7, result: "x" });
    assert.deepEqual(plain(rend.sent.at(-1)), { type: "tool_result", id: 7, result: "x" });
    w.deliver(rend.contentWindow, { type: "dev_error", source: "renderer", message: "oops", detail: null });
    w.deliver(proc.contentWindow, { type: "dev_error", source: "processor", message: "bad", detail: "stack" });
    assert.deepEqual(plain(mc.errors), [{ source: "renderer", message: "oops", detail: null }, { source: "processor", message: "bad", detail: "stack" }]);
    assert.equal(out.errors.length, 2);
  });

  await t.test("destroy removes both frames, closes the socket and stops listening", async () => {
    const [, proc] = w.frames;
    mc.destroy();
    assert.equal(w.frames.length, 0);
    await until(() => b.seen.sockets.size === 0, "socket closed");
    w.deliver(proc.contentWindow, { type: "json", json: { late: 1 } });
    assert.deepEqual(plain(out.jsons.at(-1)), { n: 1 });
  });
});

test("navigating away while the window loads leaves no late iframe or processor connection", async t => {
  const b = await backend();
  t.after(() => b.close());
  const w = page(b), jsons = [];
  let release;
  w.fetch = () => new Promise(resolve => { release = resolve; });
  const mc = w.SpaceStation.host({
    runtimeUrl: "/mission-control.js", api: { base: "/api" }, org: "acme",
    ws: `${b.url.replace("http", "ws")}/api/ws/mission-control?org=acme`,
    window: { id: "w1", version: { name: "v1", processor: "defineProcessor({})", renderer: "<p>hi</p>" } },
    mount: w.mount, onJson: j => jsons.push(j),
  });
  t.after(() => mc.destroy());
  mc.destroy();
  release({ ok: true, json: async () => ({ json: { late: true } }) });
  await sleep(30);
  assert.deepEqual(jsons, []);
  assert.equal(w.frames.length, 0);
  assert.equal(b.seen.upgrades.length, 0);
});
