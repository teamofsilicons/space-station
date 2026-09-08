/**
 * Tools: the type table row by row, then real tool calls through a one-shot host and its
 * `--permission` child (argument checks, the 64 KB result bound, timeouts, immutability).
 */
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { until, backend, tools, runtime } = require("./support.js");

test("type table", async t => {
  const ok = (spec, args) => assert.equal(runtime.argsError(spec, args), null, JSON.stringify(args));
  const bad = (spec, args, re) => assert.match(runtime.argsError(spec, args) ?? "null", re, JSON.stringify(args));

  await t.test("string accepts only strings", () => {
    ok({ a: "string" }, { a: "" });
    bad({ a: "string" }, { a: 1 }, /"a" must be string/);
  });
  await t.test("number accepts only finite numbers: NaN, Infinity and numeric strings are refused", () => {
    ok({ n: "number" }, { n: 0 });
    ok({ n: "number" }, { n: -1.5 });
    bad({ n: "number" }, { n: NaN }, /"n" must be number/);
    bad({ n: "number" }, { n: Infinity }, /"n" must be number/);
    bad({ n: "number" }, { n: "5" }, /"n" must be number/);
  });
  await t.test("boolean accepts only true or false", () => {
    ok({ b: "boolean" }, { b: false });
    bad({ b: "boolean" }, { b: "true" }, /"b" must be boolean/);
    bad({ b: "boolean" }, { b: 0 }, /"b" must be boolean/);
  });
  await t.test("object accepts non-null non-array objects", () => {
    ok({ o: "object" }, { o: {} });
    bad({ o: "object" }, { o: [] }, /"o" must be object/);
    bad({ o: "object" }, { o: null }, /"o" must be object/);
  });
  await t.test("array accepts only arrays", () => {
    ok({ l: "array" }, { l: [1] });
    bad({ l: "array" }, { l: {} }, /"l" must be array/);
  });
  await t.test("a ? suffix permits absent or undefined, never null", () => {
    ok({ s: "string?" }, {});
    ok({ s: "string?" }, { s: undefined });
    ok({ s: "string?" }, { s: "x" });
    bad({ s: "string?" }, { s: null }, /"s" must be string/);
    bad({ s: "string" }, {}, /missing argument "s"/);
    bad({ s: "string" }, { s: null }, /"s" must be string/);
  });
  await t.test("unknown keys, inherited names and non-object args are invalid", () => {
    bad({ a: "string" }, { a: "x", b: 1 }, /unknown argument "b"/);
    bad({ a: "string" }, { a: "x", constructor: 1 }, /unknown argument "constructor"/);
    bad({}, [], /args must be an object/);
    bad({}, null, /args must be an object/);
    ok({}, {});
  });
});

const CODE = `export default defineProcessor({
  init: j => j,
  tools: {
    order_detail: { args: { order_id: "string", verbose: "boolean?" }, run: (json, { order_id, verbose }) => ({ order_id, verbose: verbose ?? null, seen: json.orders }) },
    mutate: { args: {}, run: json => { json.orders.push("x"); return json.orders.length; } },
    big: { args: { n: "number" }, run: (_, { n }) => "x".repeat(n) },
    throws: { args: {}, run: () => { throw new Error("bad tool"); } },
    slow: { args: {}, run: () => new Promise(() => {}) },
    nothing: { args: {}, run: () => undefined },
    query: { args: {}, run: () => mission_control.query("SELECT 2") },
  },
});`;

test("tool calls", async t => {
  const b = await backend({ state: { json: { orders: ["o1"] }, metadata: {} }, query: () => ({ rows: [{ two: 2 }], watermarks: {} }) });
  t.after(() => b.close());
  process.env.SPACE_STATION_TEST_TIMEOUT_MS = "50";
  const h = tools(b, CODE);
  delete process.env.SPACE_STATION_TEST_TIMEOUT_MS;
  t.after(() => h.mc.destroy());
  await until(() => h.ready, "tools loaded");

  await t.test("a one-shot host runs no init and no subscriptions", () => {
    assert.equal(b.seen.upgrades.length, 0);
    assert.deepEqual(b.seen.requests.map(r => r.url), ["/api/orgs/acme/windows/w1/state"]);
  });
  await t.test("run gets SiliconJSON and typed args; an optional argument arrives as undefined", async () => {
    assert.deepEqual(await h.call("order_detail", { order_id: "o1" }), { type: "tool_result", id: 1, result: { order_id: "o1", verbose: null, seen: ["o1"] } });
  });
  await t.test("unknown tools and invalid args are refused before anything runs", async () => {
    assert.deepEqual(await h.call("nope", {}), { type: "tool_error", id: 2, code: "unknown_tool", message: 'no tool "nope"' });
    assert.deepEqual(await h.call("order_detail", { order_id: 5 }), { type: "tool_error", id: 3, code: "invalid_args", message: 'argument "order_id" must be string' });
    assert.deepEqual(await h.call("order_detail", { order_id: "o1", extra: 1 }), { type: "tool_error", id: 4, code: "invalid_args", message: 'unknown argument "extra"' });
    assert.equal((await h.call("order_detail", { order_id: "x".repeat(65536) })).message, "args over 65536 bytes");
  });
  await t.test("a tool sees a clone: SiliconJSON is never replaced by a tool", async () => {
    assert.equal((await h.call("mutate", {})).result, 2);
    assert.deepEqual((await h.call("order_detail", { order_id: "o1" })).result.seen, ["o1"]);
  });
  await t.test("results are bounded at 64 KB; a throw is failed; hanging is timeout; undefined is null", async () => {
    assert.equal((await h.call("big", { n: 65534 })).type, "tool_result");
    const big = await h.call("big", { n: 65535 });
    assert.equal(big.code, "failed");
    assert.match(big.message, /^result is 65537 bytes, the limit is 65536$/);
    assert.deepEqual(await h.call("throws", {}), { type: "tool_error", id: 10, code: "failed", message: "bad tool" });
    assert.deepEqual(await h.call("slow", {}), { type: "tool_error", id: 11, code: "timeout", message: "slow timed out after 50 ms" });
    assert.deepEqual((await h.call("nothing", {})).result, null);
  });
  await t.test("a tool may query through the host, which holds the credential", async () => {
    assert.deepEqual((await h.call("query", {})).result, [{ two: 2 }]);
    assert.deepEqual(b.seen.queries, [{ sql: "SELECT 2" }]);
  });
});
