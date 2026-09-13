# @teamofsilicons/space-station-web

Framework-agnostic browser telemetry for React, Solid, Next, vanilla JavaScript, and other web apps.

```sh
npm install @teamofsilicons/space-station-web
```

```js
import { createSpaceStationWeb } from '@teamofsilicons/space-station-web';

const telemetry = createSpaceStationWeb({
  analyticsTable: 'spacestation-frontend-analytics',
  eventsTable: 'spacestation-frontend-events',
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
