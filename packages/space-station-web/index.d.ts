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
export function createSpaceStationWeb(options?: SpaceStationWebOptions): SpaceStationWeb;
export default createSpaceStationWeb;
