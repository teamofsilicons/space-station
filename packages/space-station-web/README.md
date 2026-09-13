# @teamofsilicons/space-station-web

Framework-agnostic browser telemetry for React, Solid, Next, vanilla JavaScript, and other web apps.

```sh
npm install @teamofsilicons/space-station-web
```

```js
import { createSpaceStationWeb } from '@teamofsilicons/space-station-web';

const telemetry = createSpaceStationWeb({
  analyticsTable: 'spacestationfrontendanalytics',
  eventsTable: 'spacestationfrontendevents',
  endpoint: '/api/web/telemetry',
});

telemetry.track('table_created', { table_kind: 'orders' });
await telemetry.flush();
```

`analyticsTable` enables automatic page views, SPA navigation, errors, scroll depth, navigation timing, network outcomes, and coarse device/browser context. `eventsTable` is for explicit `track()` events. Either stream can be disabled by omitting its table. Automatic sampling is controlled with `sampleRate` (default `1`).

The package never adds credentials or an authorization header. Automatic click records contain only the element tag, ARIA role, and an optional explicit `data-spacestation-event` marker; input contents, text, IDs, URLs with queries, and hashes are excluded. Event data is bounded and non-serializable or oversized values are replaced with a diagnostic marker.

Delivery is bounded: batches contain at most 40 events, the in-memory queue holds at most 200, and a failed batch is retried at most twice. `flush()` resolves with `{sent, failed, dropped, queued}` instead of rejecting, so telemetry cannot create an unhandled-rejection loop. Failed sends are retried after five seconds while data remains queued.

Set `enabled: false` initially or call `setEnabled(false)` to opt out and clear queued data. Call `destroy()` when the app is unmounted; it removes listeners, restores history/fetch hooks, cancels timers, and flushes what remains.

The endpoint receives `POST {endpoint}` with `{ table, events }`. Authentication and authorization remain the host application's responsibility.

Table IDs use the server's lowercase letters and digits grammar; the examples above follow it. Keep the `spacestation` prefix when creating shared Space Station tables.

Use the same setup in React, Solid, Next, or vanilla JavaScript. In a Next/Solid server route (or any small same-origin proxy), keep the table keys in server environment variables, allowlist the incoming table name, and translate the request to normal ingest:

```js
import { toIngestBatch } from '@teamofsilicons/space-station-web';

// POST /api/web/telemetry: body is { table, events }
const allowed = new Map([
  ['spacestationfrontendanalytics', process.env.SS_ANALYTICS_KEY],
  ['spacestationfrontendevents', process.env.SS_EVENTS_KEY],
]);
const { table, events } = await request.json();
const key = allowed.get(table);
if (!key || !Array.isArray(events)) return Response.json({ error: 'invalid telemetry table' }, { status: 400 });
const ingest = toIngestBatch({ table, key, events });
return fetch(`${process.env.SPACE_STATION_URL}/api/ingest`, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(ingest),
});
```

The adapter preserves each event's stable `id` as `metadata.record_id`, so normal ingest deduplication also works across retries. Never expose table keys in browser code. The built-in browser sender continues to use `/api/web/telemetry` by default; change `endpoint` only when your host provides another same-origin route.
