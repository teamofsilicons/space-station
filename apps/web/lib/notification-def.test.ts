// What the notification definition check accepts, what it names, how it splits the body, and
// what a test run reads back.
import { test } from "node:test";
import assert from "node:assert/strict";
import { durationMs, parseDefinition, testSummary } from "./notification-def.ts";

const valid = {
  name: "Big order",
  triggers: [{ table: "orders", where: "record.amount::Float64 > 100" }, { schedule: "*/5 * * * *" }],
  sql: "SELECT 'k' AS dedup_key, 't' AS text, map() AS metadata FROM orders",
  access: ["@alice", "ops"],
  recipients: ["@alice", "webhook:wh1"],
};

const errorsOf = (patch: Record<string, unknown>) => parseDefinition(JSON.stringify({ ...valid, ...patch })).errors;

test("a valid definition becomes {def, recipients} with recipients split out of def", () => {
  const { body, errors } = parseDefinition(JSON.stringify(valid));
  assert.deepEqual(errors, []);
  assert.deepEqual(body?.recipients, ["@alice", "webhook:wh1"]);
  assert.equal("recipients" in (body?.def ?? {}), false);
  assert.equal(body?.def.name, "Big order");
});

test("recipients default to an empty array", () => {
  const { recipients: _, ...noRecipients } = valid;
  assert.deepEqual(parseDefinition(JSON.stringify(noRecipients)).body?.recipients, []);
});

test("invalid JSON and non-objects are refused with one error", () => {
  assert.match(parseDefinition("{ name: ").errors[0], /not JSON/);
  assert.equal(parseDefinition("[1]").errors.length, 1);
});

test("every required field and shape is named", () => {
  assert.match(errorsOf({ name: "" })[0], /^name/);
  assert.match(errorsOf({ sql: " " })[0], /^sql/);
  assert.match(errorsOf({ triggers: [] })[0], /^triggers/);
  assert.match(errorsOf({ triggers: [{ where: "x" }] })[0], /triggers\[0\]\.table/);
  assert.match(errorsOf({ triggers: [{ table: "Orders" }] })[0], /triggers\[0\]\.table/);
  assert.match(errorsOf({ triggers: [{ schedule: "* * *" }] })[0], /triggers\[0\]\.schedule/);
  assert.match(errorsOf({ access: "ops" })[0], /^access/);
  assert.match(errorsOf({ recipients: ["alice"] })[0], /^recipients/);
  assert.match(errorsOf({ enabled: "yes" })[0], /^enabled/);
  assert.match(errorsOf({ colour: "red" })[0], /unknown key "colour"/);
});

test("delay and cooldown follow the grammar and the caps", () => {
  assert.equal(durationMs("2s"), 2000);
  assert.equal(durationMs("10m"), 600_000);
  assert.equal(durationMs("1.5s"), null);
  assert.equal(durationMs("2 s"), null);
  assert.deepEqual(errorsOf({ delay: "1h", cooldown: "30d" }), []);
  assert.match(errorsOf({ delay: "61m" })[0], /delay: at most 1h/);
  assert.match(errorsOf({ cooldown: "31d" })[0], /cooldown: at most 30d/);
  assert.match(errorsOf({ delay: "soon" })[0], /^delay/);
});

test("several mistakes are all reported at once, and body is null", () => {
  const r = parseDefinition(JSON.stringify({ triggers: "x" }));
  assert.equal(r.body, null);
  assert.ok(r.errors.length >= 3, r.errors.join("; "));
});

test("a test run shows its error, its never-fired note and its rows, not just the first of them", () => {
  const summary = testSummary({ rows: [{ dedup_key: "k" }], last_trigger_at: null, error: "Code: 47. Unknown identifier `nope`" });
  assert.match(summary, /Unknown identifier/);
  assert.match(summary, /no trigger seen yet/);
  assert.match(summary, /dedup_key/);
  assert.equal(testSummary({ rows: [], last_trigger_at: "2026-01-01T00:00:00Z" }), "No new matching rows.");
});
