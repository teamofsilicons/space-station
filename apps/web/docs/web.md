# Browser SDK retirement

Space Station no longer collects or sends frontend analytics, product events, or diagnostic
telemetry. There is no telemetry setting to enable, and the browser telemetry endpoint has
been removed.

The `@teamofsilicons/space-station-web` package is retired. Version 0.1.2 retains its public API
for existing applications, but every method is inert: it reads no browser or device data,
stores no events, installs no hooks or timers, and sends no requests. `enabled: true` and
`setEnabled(true)` cannot turn collection back on.

Existing applications should remove the package and its calls, or upgrade to 0.1.2 while
removing the integration. Earlier installed versions must be upgraded or removed to stop
their collection. Remove any telemetry proxy and its table-key configuration from your host
application as well.

Explicitly submitted application records remain available through the [HTTP API](/docs/api),
[CLI](/docs/cli), and [Rust package](/docs/rust). The client adds only the record ID, table ID,
and event timestamp; it does not collect host, device, or resource-usage information.
