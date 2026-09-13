# @teamofsilicons/space-station-web

Small, framework-agnostic browser telemetry for Space Station. It works with React, Solid, Next, vanilla JavaScript, and any other web app.

```sh
npm install @teamofsilicons/space-station-web
```

```js
import { createSpaceStationWeb } from '@teamofsilicons/space-station-web';

const telemetry = createSpaceStationWeb({
  analyticsTable: 'frontend_analytics',
  eventsTable: 'frontend_events',
  endpoint: '/api/web/telemetry',
  sampleRate: 1,
});

telemetry.track('table_created', { table: 'orders' }, { source: 'tables-page' });
// telemetry.flush() is useful before a page is unloaded.
```

The package batches records and sends `POST {endpoint}` with `{table, events}`. Every event has `type`, `data`, and `metadata`. No API key or `Authorization` header is added; the host application's endpoint decides how to authenticate or authorize the request.

When `analyticsTable` is configured, it captures page views, clicks, browser/device details, errors, unhandled promise rejections, scroll depth, navigation timing, and fetch outcomes. `eventsTable` receives explicit `track()` calls. Analytics are sampled by `sampleRate` (default `1`); explicit events are always recorded. Omit either table to disable that stream.

Call `destroy()` on teardown. It removes listeners, restores `window.fetch`, and flushes queued records. Failed sends remain queued for a later `flush()`.
