# Space Windows

A Space Window is **Processor → Renderer**. The processor is JavaScript that turns records into
one small JSON document — the *SiliconJSON* — and exposes tools. The renderer is an HTML page in
a sandboxed iframe that shows it. Carbons look at the renderer; silicons read the SiliconJSON and
call the tools.

Create a window under **Space Windows** (a name under 20 characters and an access list), or with
`spacestation windows create "Orders" --access @alice,tech`. The window page shows a **prompt
for an agent** with everything an agent needs to write both files, and **Add code**, which
publishes a named version. Windows are version-managed; every version has a name and a
`created_by`, and `spacestation windows rm <id>` takes the window and all of them.

## The processor

```js
const subscriptions = {
  recent: {
    triggers: [{ table: "orders", where: "record.amount::Float64 > 5" }],   // where is optional
    sql: "SELECT record.id::String AS id, record.amount::Float64 AS amount FROM orders ORDER BY event_ts_ms DESC LIMIT 50",
    mode: "delta",                                                          // rows = only the new ones
    onTrigger: (json, rows) => ({ ...json, recent: [...rows, ...(json.recent ?? [])].slice(0, 50) }),
  },
  totals: {
    triggers: [{ table: "orders" }],
    sql: "SELECT toInt32(count()) AS n, sum(record.amount::Float64) AS revenue FROM orders",
    mode: "snapshot",                                                       // rows = the whole table
    onTrigger: (json, rows) => ({ ...json, totals: rows[0] }),
  },
};

// init runs every subscription's query once, through the same onTrigger, so a freshly published
// window shows data before the next row arrives. (An `init: (json) => json` shows nothing until then.)
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
});
```

Inside the processor there is `mission_control.query(sql)`, `mission_control.data` (cursors and
watermarks per table) and `mission_control.json`. Nothing else: no `fetch`, no imports, no
credentials, no DOM. Processor code is untrusted and runs in an opaque-origin iframe (in the
browser) or a permission-less child process (under Node).

`mission_control.query(sql)` returns a promise of the **rows** — an array of objects, exactly
what `onTrigger` receives — and not `{rows, watermarks}`: the runtime keeps the watermarks for
you in `mission_control.data.tables[<table>]` (`{cursor, watermark}`). A refused query rejects
with `{code, message}`. Column values are what [SQL](/docs/sql) describes: JSON paths need a
cast, and **every 64-bit integer arrives as a string** — `count()`, `countIf()`, `sum()` of an
integer, `::Int64` — except `cursor`, `event_ts_ms` and `registered_ts_ms`, which the runtime
turns into numbers before your code sees a row. Wrap the rest (`toInt32(count()) AS n`,
`toFloat64(sum(record.qty::Int64))`) or `Number()` it in JavaScript.

### init and cursors

When a window starts, `init(json)` runs before any subscription is registered. `json` is the
last SiliconJSON the server has for this window, or `{}`. Every query inside init is bounded to
the tables' current **watermarks** (the highest cursor written so far), and after init the
subscriptions continue from exactly those cursors — no row is seen twice and none is skipped.

Init is what a **freshly published window shows**. Subscriptions run only when a new row arrives,
so an init that returns its argument unchanged leaves a new window — or a new version — empty
until the next record, which looks like a broken window when the table is quiet. Init should
rebuild everything the subscriptions maintain: run each subscription's SQL once and feed it to
the same `onTrigger`, as `rebuild` does above, so a window is complete the moment it opens and a
live update and a cold start can never disagree about the shape of the document.

### Subscriptions: delta vs snapshot

A subscription's `triggers` say *when* to run: any new row in `table` that matches `where` (or
any row when `where` is absent). Its `sql` says *what* to fetch, in one of two modes:

- **`delta`** — the SQL sees only rows with `cursor` in `(last cursor, watermark]` for every
  trigger table. Ideal for "append the new orders". After a successful run the cursor moves.
- **`snapshot`** — the SQL sees the whole table. Ideal for totals and top-N.

Subscriptions run one at a time, in definition order; several triggers for the same subscription
while another is running are combined into one run. `onTrigger` (queries included) has 10 s. If
it throws, times out, returns something that is not an object or more than 64 KB, that run is a
dev error: the SiliconJSON and the cursors stay unchanged and the queue moves on.

If the connection drops, the runtime reconnects, re-subscribes and catches up from the cursors it
holds — cursor management is the client's job, and the runtime does it for you.

### Tools

`args` is a schema: `"string"`, `"number"` (finite), `"boolean"`, `"object"` (non-null,
non-array), `"array"`; a `?` suffix makes an argument optional. Unknown keys are refused. `run`
gets a copy of the SiliconJSON and its return value (any JSON ≤ 64 KB) is the result; a tool never
replaces the SiliconJSON. Tools share the subscription queue and the 10 s limit.

### The 64 KB rule

The SiliconJSON, tool arguments and tool results are each at most **64 KB** (UTF-8 bytes of the
JSON text). Keep aggregates and the few rows a view needs, never raw dumps. It is "as close to the
final render as possible" by design: the renderer should mostly display it.

## The renderer

The renderer is an `<iframe sandbox="allow-scripts">`: it has no cookies and no access to the
parent page, but it may load any CDN and call any third-party service. It gets the SiliconJSON,
the tools and the metadata, and an event when the JSON changes. Inside it:

| `mission_control.…` | |
|---|---|
| `json` | the SiliconJSON |
| `metadata` | `{processor_version, renderer_version, produced_at, is_live}` |
| `stale` | `!metadata.is_live` |
| `tools.<name>(args)` | runs a tool; async, type-checked against the schema |
| `on("update", cb)` | called after every change |
| `mount(selector)` | creates a Vue 3 app with `mission_control` and `stale` in scope (when `Vue` is on the page) |

The renderer cannot `query` or `subscribe`.

**Default and preferred stack: Vue 3 + D3, both from a CDN.**

```html
<div id="app">
  <h1>{{ mission_control.json.totals?.n }} orders</h1>
  <p v-if="stale">as of {{ mission_control.metadata.produced_at }}</p>
  <ul><li v-for="o in mission_control.json.recent" :key="o.id">{{ o.id }} — ${{ o.amount }}</li></ul>
  <svg id="chart" width="600" height="160"></svg>
</div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/vue/3.5.13/vue.global.prod.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js"></script>
<script>
  mission_control.mount("#app");

  mission_control.on("update", () => {
    const rows = mission_control.json.recent ?? [];
    const x = d3.scaleBand().domain(rows.map((r) => r.id)).range([0, 600]).padding(0.1);
    const y = d3.scaleLinear().domain([0, d3.max(rows, (r) => r.amount) ?? 0]).range([160, 0]);
    d3.select("#chart").selectAll("rect").data(rows, (r) => r.id).join("rect")
      .attr("x", (r) => x(r.id)).attr("width", x.bandwidth())
      .attr("y", (r) => y(r.amount)).attr("height", (r) => 160 - y(r.amount));
  });

  // await mission_control.tools.filter_orders({ min_amount: 5 });
</script>
```

## Developing locally

The npm package `@teamofsilicons/space-station` ships the runtime and `space-station-dev`, a dev
server that mimics production exactly: the same runtime, the same processor/renderer boundary,
the same queries — only the credential differs.

```
npm i -g @teamofsilicons/space-station        # until it is published: npm i -g ./packages/space-station from a checkout
mkdir orders-board && cd orders-board
cat > .env <<'EOF'
SPACE_STATION_URL=https://space.example.com
SPACE_STATION_ACCESS_TOKEN=spacewindow-…
SPACE_STATION_ORG=tos
SPACE_STATION_WINDOW=win_…          # optional: seeds the page from a published window
EOF
space-station-dev            # serves ./processor.js + ./renderer.html at http://127.0.0.1:4747
```

`SPACE_STATION_ORG` is the organization the queries run in. `SPACE_STATION_ACCESS_TOKEN` is the
**access token** shown at the top of every window page. It
is yours (one per person per org, rotatable there), not the window's, and it lets mission control
run queries on your behalf. The dev server is the only thing that holds it: your code never sees
it, the page reloads on change, and `.env` is never served. Errors from the processor and the
renderer show in the dev page; in the app, **Option+Shift+D** opens the same panel.

`space-station-dev notify <notification-id>` runs a notification against its last known trigger
(or tells you there was none) — see [Notifications](/docs/notifications).

## Publishing and the secret check

```
space-station-dev publish --window <window-id> --name v1
spacestation windows publish <window-id> --name v1 --processor processor.js --renderer renderer.html
```

or paste both files into **Add code** on the window page with a version name. Either way the
server scans the code for anything shaped like a credential — `spacewindow-…`, `apikey-…`,
`whsec-…`, `table-{id}-…` and IAM tokens — including inside comments, and refuses the version
with `secret_in_code`. Dev code that reads the token from anywhere is not publishable by
construction: the runtime injects credentials, your code never touches them.

The published version becomes the window's current version. Anyone with access who opens the
window runs it in their browser; the server keeps the last SiliconJSON the window produced (one
per window, whichever version wrote it last) so a fresh open shows the latest state at once (with
`is_live` false until the run is connected).

## From a terminal

A window is a renderer *and* a document, and the document half needs no browser:

```
spacestation windows run <id>              run the published processor here; idle 10 min → inactive
spacestation windows json <id>             the cached SiliconJSON and its metadata
spacestation windows tool <id> filter_orders '{"min_amount": 5}'
spacestation windows code <id>             the current version's processor and renderer
spacestation windows open <id>             print this window's page, and open it
```

`run` starts the same host the app runs, with the processor in a credential-less Node child, and
keeps the server's copy of the SiliconJSON current while it runs. `json` and `tool` read that
copy without starting anything. `ls`, `get` and `edit` print a summary — id, name, access, the
current version's name and author — and `code` prints the code, so a listing stays readable and a
silicon that wants to read a window it has access to asks for exactly that. The renderer is the
one part a terminal cannot show, so `open` hands it to a browser instead — see the
[CLI](/docs/cli) and the [Rust package](/docs/rust).

## The prompt for an agent

The window page has a copyable prompt that packs this page and [SQL](/docs/sql) into
instructions for an agent — the window's id and name, the org's table ids, both APIs, the
limits, the dev loop and the publish command. Hand it to any agent; paste what comes back.
