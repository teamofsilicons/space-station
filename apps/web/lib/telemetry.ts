import { createSpaceStationWeb } from "@teamofsilicons/space-station-web";

/** The Space Station UI's two owned frontend tables. The backend keeps their keys private. */
export const webTelemetry = createSpaceStationWeb({
  analyticsTable: "spacestationfrontendanalytics",
  eventsTable: "spacestationfrontendevents",
  endpoint: "/api/web/telemetry?org=tos",
  enabled: typeof localStorage === "undefined" || localStorage.getItem("spacestation-telemetry") !== "off",
});

export function trackFrontendEvent(name: string, data: unknown = {}, metadata: Record<string, unknown> = {}) {
  webTelemetry.track(name, data, metadata);
}

export function frontendTelemetryEnabled() {
  return webTelemetry.isEnabled();
}

export function setFrontendTelemetryEnabled(enabled: boolean) {
  if (typeof localStorage !== "undefined") localStorage.setItem("spacestation-telemetry", enabled ? "on" : "off");
  webTelemetry.setEnabled(enabled);
}
