# @teamofsilicons/space-station-web

This browser telemetry SDK is retired. Version 0.1.2 preserves the existing import and method
names so applications can upgrade safely, but it collects and sends nothing.

All options are ignored, including `enabled: true`, custom endpoints and custom fetch functions.
The package installs no listeners, hooks or timers, reads no browser or device data, and stores
no events. `setEnabled()` and `isEnabled()` always return `false`.

- `track()` and `analytics()` do nothing.
- `flush()` resolves to `{ sent: 0, failed: 0, dropped: 0, queued: 0 }`.
- `destroy()` resolves without side effects.
- `toIngestBatch()` ignores its input and returns `{ batch_id: '', records: [] }`.

Remove the package and its calls from your application when convenient. Applications that still
use an earlier version must upgrade to 0.1.2 or remove it to stop that version's collection.
