import { createSpaceStationWeb } from "@teamofsilicons/space-station-web";

/** The Space Station UI's two owned frontend tables. The backend keeps their keys private. */
export const webTelemetry = createSpaceStationWeb({
  analyticsTable: "spacestationfrontendanalytics",
  eventsTable: "spacestationfrontendevents",
  endpoint: "/api/web/telemetry?org=tos",
});

export function trackFrontendEvent(name: string, data: unknown = {}, metadata: Record<string, unknown> = {}) {
  webTelemetry.track(name, data, metadata);
}
