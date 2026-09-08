# @teamofsilicons/space-station

The Space Station space-window runtime (`mission_control`) and `space-station-dev`, the dev
server that runs a window on your machine exactly as production runs it.

A **space window** is `processor.js` → SiliconJSON (a JSON object ≤ 64 KB) → `renderer.html`.
The processor turns records into that one document and exposes tools; the renderer shows it live
in a sandboxed iframe. Carbons look at the renderer, silicons read the SiliconJSON and call the
tools.

```
npm i -g @teamofsilicons/space-station
mkdir orders-board && cd orders-board        # processor.js + renderer.html + .env live here
space-station-dev                            # http://127.0.0.1:4747, reloads when a file changes
space-station-dev publish --window <id> --name v1
```

`.env` (never served, never read by your code):

```
SPACE_STATION_URL=https://space.example.com
SPACE_STATION_ACCESS_TOKEN=spacewindow-…     # the access token shown on every window page; yours, rotatable there
SPACE_STATION_ORG=tos                        # the org id
SPACE_STATION_WINDOW=w_01                    # optional: the window whose cached state seeds a dev run
```

Every key may also come from the environment. Nothing else is configurable.

## processor.js

```js
export default defineProcessor({
  init: async (json) => json,                 // runs first, once; may call mission_control.query(sql)

  subscriptions: {
    recent: {
      triggers: [{ table: "orders", where: "record.amount::Float64 > 5" }],   // where is optional
      sql: "SELECT record.id::String AS id, record.amount::Float64 AS amount FROM orders ORDER BY event_ts_ms DESC LIMIT 50",
      mode: "delta",                          // "delta" (default) or "snapshot"
      onTrigger: (json, rows) => ({ ...json, recent: rows }),   // return the new SiliconJSON
    },
    totals: {
      triggers: [{ table: "orders" }],
      sql: "SELECT count() AS n, sum(record.amount::Float64) AS revenue FROM orders",
      mode: "snapshot",
      onTrigger: (json, rows) => ({ ...json, totals: rows[0] }),
    },
  },

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

`export default` is optional; calling `defineProcessor(...)` is what counts. Inside the processor
there is exactly:

| `mission_control.…` | |
|---|---|
| `query(sql)` | runs a read-only SELECT over the org's tables; resolves to the rows (`cursor`, `event_ts_ms`, `registered_ts_ms` as numbers); rejects with `{code, message}` when the server refuses it |
| `data.tables[t]` | `{cursor, watermark}` per trigger table: the cursor every subscription on it has passed, and the highest watermark seen. Managed for you |
| `json` | the current SiliconJSON |

No `fetch`, no `WebSocket`, no imports, no credentials, no DOM. The processor never sees the
access token: it runs in an opaque-origin iframe whose CSP is `default-src 'none'` (browser) or
in a `node --permission` child with an empty environment (CLI).

### Lifecycle

1. The watermarks of every trigger table are fetched.
2. The SiliconJSON is seeded from the last state the server has for this window, or `{}`.
3. `init(json)` runs. Every `query` inside it is bounded to those watermarks (`restrict {t: {to}}`),
   so init and the subscriptions never see a row twice and never skip one. It must return a JSON
   object ≤ 64 KB.
4. Each subscription is registered, in definition order, and runs once if the table moved since
   the watermarks were read (this is also the catch-up after a reconnect).
5. On a trigger: `delta` queries `sql` with `cursor >` this subscription's own cursor for every
   trigger table (the server fills `to` with the watermark), `snapshot` queries `sql` whole. Each
   subscription keeps its own cursors, so two of them on one table never skip each other's rows. Then
   `onTrigger(json, rows)` with 10 s. Only after it settles with a JSON object ≤ 64 KB do the
   cursors move and the SiliconJSON change. A throw, a timeout, a non-object or an oversized result
   is a dev error and changes nothing.
6. Subscriptions and tools share one serial queue: repeated triggers of one subscription while it
   waits are combined into a single run; when several are ready the one higher in the definition
   goes first; tools go ahead of subscriptions.
7. The state is sent to the server on every change (at most once a second) and at least every
   10 s, so the window is `is_live` while a run is connected.
8. When the socket drops the runtime reconnects (1 s → 30 s backoff), re-subscribes, resends the
   SiliconJSON and catches up from its cursors. A close in the 4xxx range is the server refusing
   the credential (4401) or the actor (4403): its reason becomes a dev error and there is no
   retry, because the same credential would only be refused again.

### Tools

`args` is a schema; `run(json, args)` gets a copy of the SiliconJSON (a tool never replaces it)
and its return value (any JSON ≤ 64 KB) is the result. The runtime checks the arguments before
`run` is called:

| type | accepts |
|---|---|
| `string` | strings |
| `number` | finite numbers (`NaN`, `Infinity`, `"5"` are refused) |
| `boolean` | `true` or `false` |
| `object` | non-null, non-array objects |
| `array` | arrays |
| `T?` | `T`, or the argument absent; `null` is not absent |

Unknown keys are refused. Errors are `unknown_tool`, `invalid_args`, `timeout` (10 s) or
`failed` (the tool threw). Arguments and results are each ≤ 64 KB.

### SQL

`FROM <table_id>` for any table of the org; read-only, one SELECT. Columns: `cursor`,
`record_id`, `event_ts_ms`, `registered_ts_ms`, `metadata` (JSON) and `record` (JSON, what the
app sent). JSON paths need a cast (`record.amount::Float64`, `record.id::String`). Refused:
`SETTINGS`, `FORMAT`, `INTO OUTFILE`, table functions, `system.*`, `db.table` names.

## renderer.html

A plain HTML page shown in an `<iframe sandbox="allow-scripts">`: no cookies and no access to
the page around it, but any CDN and any third-party request. The runtime injects
`mission_control` before your first script:

| `mission_control.…` | |
|---|---|
| `json` | the SiliconJSON |
| `metadata` | `{processor_version, renderer_version, produced_at, is_live}` |
| `stale` | `!metadata.is_live` |
| `tools.<name>(args)` | runs a tool; a Promise, type-checked, rejects with `{code, message}` |
| `on("update", cb)` | `cb(json, metadata)` after every change, and at once if one already arrived |
| `mount(selector)` | creates a Vue 3 app over `selector` with `mission_control` and `stale` in scope, when `Vue` is on the page |

The first update arrives as soon as the processor has loaded — with the last SiliconJSON the
server had, while `init` still runs — so `tools` is complete from the first update on. A tool
called before `init` has finished runs right after it. The renderer cannot query or subscribe.
Errors thrown in it (and failed script loads) show up as renderer dev errors in the dev page and,
in the app, in the Option+Shift+D panel.

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

## space-station-dev

```
space-station-dev [serve] [--port 4747] [--dir .]     the dev page
space-station-dev publish --window <id> --name <version> [--dir .]
space-station-dev notify <notification-id> [--dir .]
```

**serve** binds `127.0.0.1` only and serves the same host page the app uses, with
`./processor.js` and `./renderer.html`; the page reloads when either changes. It is the only
thing that talks to Space Station: `/api/*` and the mission-control WebSocket are proxied with
`Authorization: Bearer <token>` added on the way out. Every request must carry a `Host` of
`localhost:<port>` or `127.0.0.1:<port>`; WebSocket upgrades and every non-GET request must also
carry an `Origin` equal to the page's own. `.env` is never served and the token is never on the
page. Dev runs never store state on the server (their version is `null`).

**publish** refuses either file when it contains anything shaped like a credential
(`spacewindow-…`, `apikey-…`, `whsec-…`, `table-{id}-…`, IAM `sat_`/`oat_`/… tokens — comments
included; the server runs the same scan and answers `secret_in_code`), then POSTs the pair as a
new version and prints the server's reply. The published version becomes the window's current
one.

**notify** runs a notification against its last known trigger and prints its rows, or
`no trigger seen yet`; a SQL error goes to stderr.

Any failure is one `{"error": {"code", "message"}}` line on stderr and exit code 1.

## Embedding the host

The same file, loaded from a CDN or served by the app, defines `window.SpaceStation`:

```js
const mc = SpaceStation.host({
  runtimeUrl,                 // absolute URL of mission-control.js, for the processor iframe's <script> and its CSP
  api: { base: "/api", headers: {} },   // fetch base (cookie in the app; the dev proxy adds the bearer)
  ws,                         // absolute ws(s) URL of /api/ws/mission-control?org=…
  org,
  window: { id, name, version: { name, processor, renderer } | null },
  code: { processor, renderer },        // optional dev code; version must be null when given
  mount,                      // HTMLElement: receives the renderer iframe and the hidden processor iframe
  onJson: (json) => {}, onStatus: ({ is_live, produced_at, connected }) => {}, onError: ({ source, message, detail }) => {},
  onNotification: ({ event_id, notification, name, dedup_key, text, metadata, fired_at }) => {},   // a notification naming this actor, live
});
mc.errors;      // dev errors of this run, newest last: source is host | processor | renderer | server
mc.destroy();
SpaceStation.SANDBOX;   // "allow-scripts"
```

Without `code` and with `window.version` null, the host fetches `GET /orgs/{org}/windows/{id}`
for the current version. Both iframes get `sandbox="allow-scripts"` and nothing more; the host
listens only to messages whose `source` is one of its own frames.

## Under Node (what the CLI runs)

```
node mission-control.js run  --url U --org O --window W                    # until 10 idle minutes → exit 0
node mission-control.js tool --url U --org O --window W --name N [--args JSON]
node mission-control.js processor                                          # the child; never call it yourself
```

`run` and `tool` read `SPACE_STATION_ACCESS_TOKEN` from the environment; `--url` is the server
origin. `run` prints every SiliconJSON as one JSON line on stdout (the cached one first) and
`{"status": …}` / dev-error lines on stderr; it exits 0 after ten minutes without a trigger, 1 when
the host cannot start or the processor child dies. `tool` seeds from the cached state, runs one
tool with no init and no subscriptions, prints the result on stdout and exits 0, or prints
`{"error": {"code", "message"}}` and exits 1.

The processor child is `node --permission --allow-fs-read=<this file> mission-control.js
processor` with an empty environment: it cannot read any other file, spawn, or load addons and
workers, and it has no `fetch`/`WebSocket`. It talks to the host over stdio only; its
`console` and `process.stdout` go to stderr so user code cannot forge bridge messages. Node's
permission model does not gate raw sockets, so a processor that goes out of its way can open
one — without any credential.

## Tests

`npm test` (`node --test`, under 20 s): a real Node host, a real `--permission` child and a fake
backend on 127.0.0.1 for the lifecycle, the tool type table, the 64 KB bounds, coalescing and
ordering, timeouts, reconnect and catch-up; the CLI surface (`run`, `tool`, `dev serve` with its
host/origin checks and proxies, `publish`, `notify`); and the browser side (host and renderer
bridge) in a vm context with a minimal DOM.
