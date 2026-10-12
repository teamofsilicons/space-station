/** Retained for existing imports; telemetry collection and delivery have been removed. */
function createSpaceStationWeb() {
  return {
    track() {},
    analytics() {},
    async flush() { return { sent: 0, failed: 0, dropped: 0, queued: 0 }; },
    setEnabled() { return false; },
    isEnabled() { return false; },
    async destroy() {},
  };
}

/** Retained for existing imports; no input data is read or included in a batch. */
function toIngestBatch() {
  return { batch_id: '', records: [] };
}

export { createSpaceStationWeb, toIngestBatch };
export default createSpaceStationWeb;
