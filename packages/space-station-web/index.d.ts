export type FlushResult = { sent: number; failed: number; dropped: number; queued: number };
export type SpaceStationWebOptions = {
  analyticsTable?: string;
  eventsTable?: string;
  endpoint?: string;
  fetch?: typeof globalThis.fetch;
  sampleRate?: number;
  enabled?: boolean;
};
export type SpaceStationWeb = {
  track(name: string, data?: unknown, metadata?: Record<string, unknown>): void;
  analytics(name: string, data?: unknown, metadata?: Record<string, unknown>): void;
  flush(table?: string): Promise<FlushResult>;
  setEnabled(enabled: boolean): boolean;
  isEnabled(): boolean;
  destroy(): Promise<void>;
};
export type IngestBatch = {
  batch_id: string;
  records: Array<{ key: string; metadata: { record_id: string; table_id: string; event_ts_ms: number }; record: { type: string; data: unknown; metadata: Record<string, unknown> } }>;
};
export function createSpaceStationWeb(options?: SpaceStationWebOptions): SpaceStationWeb;
export function toIngestBatch(input: { table: string; key: string; events: Array<{ id?: string; type: string; data: unknown; metadata?: Record<string, unknown> }>; batchId?: string }): IngestBatch;
export default createSpaceStationWeb;
