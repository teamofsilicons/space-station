// The runtime's host role as this app calls it (ARCHITECTURE "Runtime host API"), and a loader
// that puts `/mission-control.js` on the page once.

export type Status = { is_live: boolean; produced_at: string | null; connected: boolean };
/** A dev error of one run: `source` is host, processor, renderer or server. */
export type RunError = { source: string; message: string; detail: unknown };

export type HostOptions = {
  runtimeUrl: string;
  api: { base: string; headers?: Record<string, string> };
  ws: string;
  org: string;
  window: { id: string; name: string; version: { name: string; processor: string; renderer: string } | null };
  code?: { processor: string; renderer: string };
  mount: HTMLElement;
  onJson?: (json: unknown) => void;
  onStatus?: (status: Status) => void;
  onError?: (error: RunError) => void;
  onNotification?: (event: Notification) => void;
};

/** A notification whose recipients name the viewing actor, delivered live over mission control. */
export type Notification = {
  event_id: string;
  notification: string;
  name: string;
  dedup_key: string;
  text: string;
  metadata: unknown;
  fired_at: string;
};

export type Host = { errors: RunError[]; destroy(): void };

export type Runtime = { host(options: HostOptions): Host; SANDBOX: string };

declare global {
  interface Window {
    SpaceStation?: Runtime;
  }
}

let loading: Promise<Runtime> | undefined;

export function loadRuntime(): Promise<Runtime> {
  if (window.SpaceStation) return Promise.resolve(window.SpaceStation);
  loading ??= new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = "/mission-control.js";
    const fail = (message: string) => {
      script.remove();
      loading = undefined;
      reject(new Error(message));
    };
    script.onload = () => window.SpaceStation ? resolve(window.SpaceStation) : fail("/mission-control.js did not define SpaceStation");
    script.onerror = () => fail("could not load /mission-control.js");
    document.head.append(script);
  });
  return loading;
}
