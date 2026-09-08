// The text a carbon hands to an agent so it can write a window's processor and renderer without
// reading the docs first. Pure: built from what the window page already has. The processor example
// is the one in docs/space-windows.md, word for word (agent-prompt.test.ts holds them together),
// over the org's table when it has exactly one.

export function agentPrompt(p: { org: string; window: { id: string; name: string }; tables: string[] }): string {
  const { org, window: w } = p;
  const tables = p.tables.length ? p.tables.join(", ") : "(none yet — create one under Tables first)";
  const table = p.tables.length === 1 ? p.tables[0] : "orders";
  return `You are writing a Space Window for Space Station. A Space Window is a processor (JavaScript)
that turns records into one small JSON document, plus a renderer (HTML) that shows it live.
Deliver two files: processor.js and renderer.html.

Org: ${org}. Window: "${w.name}" (id ${w.id}). Tables in this org: ${tables}.

## processor.js

${processorExample(table)}

Rules
- The JSON ("SiliconJSON") is a plain object of at most 64 KB (UTF-8 bytes of JSON.stringify).
  Keep aggregates and the few rows a view needs, never raw dumps. Tool args and results have the
  same 64 KB limit.
- init runs before any subscription and its queries are bounded to the tables' current
  watermarks; subscriptions continue from there. Subscriptions run only when a new row arrives,
  so init must load what is already there: run every subscription's query once through the same
  onTrigger, as rebuild does above — otherwise a new window stays empty until the next record.
- Subscriptions run one at a time, in definition order; repeated triggers of one subscription are
  coalesced into one run. onTrigger (queries included) has 10 s. If it throws, times out, returns
  a non-object or more than 64 KB, the JSON and the cursors stay where they were.
- Tool arg types: "string", "number" (finite), "boolean", "object", "array"; a "?" suffix makes it
  optional ("number?"). Unknown keys are rejected. run gets a copy of the JSON; it never replaces it.
- Inside the processor only mission_control.query(sql), mission_control.data (cursors and
  watermarks) and mission_control.json exist. No fetch, no imports, no credentials, no DOM.

## SQL
- FROM <table_id> for any table listed above (tables are logical; the server rewrites them).
  Read-only, exactly one SELECT. Columns: cursor, record_id, event_ts_ms, registered_ts_ms,
  metadata (JSON), record (JSON: what the app sent).
- JSON paths need a cast: record.amount::Float64, record.id::String, record.tags::Array(String).
  64-bit integers inside record arrive as JSON numbers, so big ids must be sent as strings.
- In result rows cursor, event_ts_ms and registered_ts_ms are numbers; every other 64-bit integer
  (count(), sum() of an integer, ::Int64) arrives as a string — wrap it (toInt32(count())) or Number() it.
- Arrays: cast the path to an array in a derived table, then ARRAY JOIN that — the cast cannot be
  written next to the ARRAY JOIN, and ARRAY JOIN record.items is a type error (a path is Dynamic):
  SELECT i.sku::String AS sku FROM (SELECT record.items::Array(JSON) AS items FROM ${table}) ARRAY JOIN items AS i
- Refused: SETTINGS, FORMAT, INTO OUTFILE, table functions (url, remote, file, ...), system.*,
  db.table names, anything that is not a SELECT.

## renderer.html
A sandboxed iframe: no cookies, any CDN or third-party request allowed. The preferred stack is
Vue 3 + D3 from a CDN. The global mission_control has: json, metadata {processor_version,
renderer_version, produced_at, is_live}, stale (= !metadata.is_live), tools.<name>(args)
(async, type-checked), on("update", cb) and mount(selector), which creates a Vue app with
mission_control and stale in scope when Vue is on the page. The renderer cannot query or subscribe.

<div id="app">
  <h1>${w.name}</h1>
  <p v-if="stale">as of {{ mission_control.metadata.produced_at }}</p>
  <ul><li v-for="o in mission_control.json.recent" :key="o.id">{{ o.id }} — {{ o.amount }}</li></ul>
  <svg id="chart" width="600" height="200"></svg>
</div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/vue/3.5.13/vue.global.prod.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js"></script>
<script>
  mission_control.mount("#app");
  mission_control.on("update", () => { /* d3.select("#chart") ... mission_control.json ... */ });
  // await mission_control.tools.order_detail({ order_id: "42" })
</script>

## Develop, then publish
- npm i -g @teamofsilicons/space-station. Put SPACE_STATION_URL, SPACE_STATION_ACCESS_TOKEN and
  SPACE_STATION_ORG in a .env next to processor.js and renderer.html (the token is on the window
  page; it belongs to the person, not the window). Run space-station-dev and open
  http://127.0.0.1:4747: the same runtime as production, reloading on change.
- Never put the access token (spacewindow-<32 hex>), a table key or an API key in the code, not
  even in a comment: the server refuses such a version (secret_in_code).
- Publish with: space-station-dev publish --window ${w.id} --name v1
  or paste both files into "Add code" on the window page with a version name.
`;
}

/** The processor of docs/space-windows.md, over `table`. */
export const processorExample = (table: string) => `const subscriptions = {
  recent: {
    triggers: [{ table: "${table}", where: "record.amount::Float64 > 5" }],   // where is optional
    sql: "SELECT record.id::String AS id, record.amount::Float64 AS amount FROM ${table} ORDER BY event_ts_ms DESC LIMIT 50",
    mode: "delta",                                                          // rows = only the new ones
    onTrigger: (json, rows) => ({ ...json, recent: [...rows, ...(json.recent ?? [])].slice(0, 50) }),
  },
  totals: {
    triggers: [{ table: "${table}" }],
    sql: "SELECT toInt32(count()) AS n, sum(record.amount::Float64) AS revenue FROM ${table}",
    mode: "snapshot",                                                       // rows = the whole table
    onTrigger: (json, rows) => ({ ...json, totals: rows[0] }),
  },
};

// init runs every subscription's query once, through the same onTrigger, so a freshly published
// window shows data before the next row arrives. (An \`init: (json) => json\` shows nothing until then.)
async function rebuild(json) {
  for (const sub of Object.values(subscriptions)) json = await sub.onTrigger(json, await mission_control.query(sub.sql));
  return json;
}

export default defineProcessor({
  init: () => rebuild({}),                    // runs first, once; mission_control.query(sql) resolves to the rows
  subscriptions,

  tools: {
    order_detail: {
      args: { order_id: "string" },
      run: (json, { order_id }) => json.recent?.find((r) => r.id === order_id) ?? null,
    },
    filter_orders: {
      args: { min_amount: "number", limit: "number?" },
      run: (json, { min_amount, limit = 10 }) => json.recent.filter((r) => r.amount >= min_amount).slice(0, limit),
    },
  },
});`;
