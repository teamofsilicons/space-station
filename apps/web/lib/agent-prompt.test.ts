// The agent prompt carries what an agent needs: the window, the tables, both APIs and the rules.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { agentPrompt, processorExample } from "./agent-prompt.ts";

const prompt = agentPrompt({ org: "acme", window: { id: "w-1", name: "Orders board" }, tables: ["orders", "signups"] });

test("names the org, the window and every table", () => {
  for (const s of ["acme", "w-1", "Orders board", "orders, signups"]) assert.ok(prompt.includes(s), s);
});

test("covers the processor API, the renderer API and the rules that get code refused", () => {
  for (const s of [
    "defineProcessor(",
    "init:",
    "const subscriptions = {",
    '"delta"',
    '"snapshot"',
    "onTrigger",
    "tools:",
    "mission_control.query(",
    "64 KB",
    "10 s",
    "record.amount::Float64",
    "ARRAY JOIN items AS i",
    "mission_control.mount(",
    "mission_control.on(",
    "vue.global.prod.min.js",
    "d3.min.js",
    "secret_in_code",
    "space-station-dev publish --window w-1",
  ])
    assert.ok(prompt.includes(s), s);
});

test("says when the org has no tables yet", () => {
  assert.match(agentPrompt({ org: "acme", window: { id: "w", name: "n" }, tables: [] }), /none yet/);
});

test("never contains a secret-shaped token", () => {
  assert.doesNotMatch(prompt, /\b(spacewindow|apikey|whsec|table-[a-z0-9]{1,50})-[0-9a-f]{32}\b/);
});

test("the array recipe casts in a derived table, since ARRAY JOIN over a JSON path is a type error", () => {
  assert.match(prompt, /FROM \(SELECT record\.items::Array\(JSON\) AS items FROM orders\) ARRAY JOIN items AS i/);
  assert.doesNotMatch(prompt, /FROM orders ARRAY JOIN record\./);
});

test("with exactly one table the examples query that table; with several they keep the generic orders", () => {
  const one = agentPrompt({ org: "acme", window: { id: "w", name: "n" }, tables: ["signups"] });
  assert.doesNotMatch(one, /FROM orders/);
  assert.doesNotMatch(one, /table: "orders"/);
  assert.match(one, /triggers: \[\{ table: "signups", where/);
  assert.match(one, /FROM signups ORDER BY event_ts_ms/);
  assert.match(one, /FROM \(SELECT record\.items::Array\(JSON\) AS items FROM signups\) ARRAY JOIN/);
  assert.match(prompt, /FROM orders ORDER BY event_ts_ms/);
});

test("the sample processor's init loads existing data through the subscriptions' own queries", () => {
  assert.match(prompt, /async function rebuild\(json\)/);
  assert.match(prompt, /init: \(\) => rebuild\(\{\}\)/);
  assert.match(prompt, /mission_control\.query\(sub\.sql\)/);
  assert.doesNotMatch(prompt, /init: async \(json\) => json/);
});

test("the sample processor is the one in docs/space-windows.md, word for word", () => {
  const doc = readFileSync(new URL("../docs/space-windows.md", import.meta.url), "utf8");
  const block = /```js\n([\s\S]*?)\n```/.exec(doc)?.[1];
  assert.ok(block, "docs/space-windows.md has a ```js block");
  assert.equal(processorExample("orders"), block);
});
