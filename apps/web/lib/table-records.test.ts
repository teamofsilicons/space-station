import { test } from "node:test";
import assert from "node:assert/strict";
import { pageQuery, openSnapshot } from "./table-records";

test("pagination keeps the entry boundary and uses exclusive cursors, including full UInt64 precision", () => {
  const boundary = "18446744073709551614";
  assert.match(
    pageQuery("orders", boundary, undefined, 25).sql,
    /cursor <= 18446744073709551614 ORDER BY cursor DESC LIMIT 25$/,
  );
  const next = pageQuery("orders", boundary, "18446744073709551580", 25).sql;
  assert.match(
    next,
    /cursor <= 18446744073709551614 AND cursor < 18446744073709551580/,
  );
  assert.match(next, /SELECT cursor, record_id/);
});
test("table identifiers are quoted and untrusted SQL inputs cannot enter pagination", () => {
  assert.match(pageQuery("123", "0", undefined, 1).sql, /FROM `123`/);
  for (const id of ["orders; DROP TABLE x", "a`", ""])
    assert.throws(() => pageQuery(id, "1", undefined, 25));
  assert.throws(() => pageQuery("orders", "1 OR 1=1", undefined, 25));
  assert.throws(() => pageQuery("orders", "1", "-1", 25));
  for (const n of [0, 101, 1.5])
    assert.throws(() => pageQuery("orders", "1", undefined, n));
});
test("opening captures a precise visible record boundary and table details, including empty tables", async () => {
  const original = globalThis.fetch;
  const calls: { url: string; body?: string }[] = [];
  let count = "40";
  globalThis.fetch = async (input, init) => {
    const url = String(input);
    calls.push({ url, body: init?.body as string });
    return new Response(
      JSON.stringify(
        url.endsWith("/tables")
          ? [{ id: "orders", access: ["@alice"] }]
          : {
              rows: [{ last: count === "0" ? "0" : "9007199254740993", count }],
            },
      ),
      { status: 200 },
    );
  };
  try {
    const snapshot = await openSnapshot("tos", "orders");
    assert.equal(snapshot.cursor, "9007199254740993");
    assert.equal(snapshot.count, 40);
    assert.deepEqual(snapshot.table.access, ["@alice"]);
    assert.match(JSON.parse(calls[1].body!).sql, /toString\(max\(cursor\)\)/);
    count = "0";
    assert.equal((await openSnapshot("tos", "orders")).count, 0);
  } finally {
    globalThis.fetch = original;
  }
});
