export type SpaceStationWebOptions = {
  analyticsTable?: string;
  eventsTable?: string;
  endpoint?: string;
  fetch?: typeof globalThis.fetch;
  sampleRate?: number;
};

export type SpaceStationWeb = {
  track(name: string, data?: unknown, metadata?: Record<string, unknown>): void;
  analytics(name: string, data?: unknown, metadata?: Record<string, unknown>): void;
  flush(): Promise<unknown>;
  destroy(): Promise<void>;
};

export function createSpaceStationWeb(options?: SpaceStationWebOptions): SpaceStationWeb;
export default createSpaceStationWeb;
