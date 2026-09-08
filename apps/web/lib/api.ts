// The one way pages talk to the backend: same-origin `/api`, JSON in, JSON out, `{error}` thrown
// as `ApiError`. Also the response shapes the pages rely on and the two hooks every page uses.

import type { NotificationDef } from "./notification-def";

export class ApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

type Envelope = { error?: { code: string; message: string } };

/** Fires on every 401, whichever widget asked: the session is gone. The shell listens and re-checks /me. */
export const signedOut = new EventTarget();

/** The id shape a table may have; the backend refuses anything else. */
export const TABLE_ID = /^[a-z0-9]{1,50}$/;

export async function api<T = void>(
  path: string,
  method = "GET",
  body?: unknown,
): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers:
      body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 401 && path !== "/me" && path !== "/orgs")
    signedOut.dispatchEvent(new Event("signedout"));
  const data =
    res.status === 204
      ? undefined
      : ((await res.json().catch(() => undefined)) as
          (T & Envelope) | undefined);
  if (res.ok) return data as T;
  // Every backend error carries `{error}`; a bare status is the API proxy (or a proxy) answering for a backend that did not.
  const { code, message } = data?.error ?? {
    code: "backend_unreachable",
    message: "the backend did not answer",
  };
  throw new ApiError(code, message, res.status);
}

export function wsUrl(org: string, override?: string) {
  const base =
    override || import.meta.env?.VITE_WS_URL ||
    (location.hostname === "localhost"
      ? "ws://localhost:8080/api/ws"
      : "wss://backend.spacestation.teamofsilicons.com/api/ws");
  return `${base}/mission-control?org=${encodeURIComponent(org)}`;
}

/** An organization: the handle people type and URLs carry, and its name once IAM's events have told the mirror one. */
export type Org = { id: string; name?: string };
/** Who this session is: an actor, bound to one org; `app` is where the UI lives. */
export type Me = {
  id: string;
  kind: "carbon" | "silicon";
  org: string;
  app: string;
};
/** GET /orgs/{org}/me: the actor as that org sees it, with the tags IAM has told the mirror. */
export type Identity = {
  id: string;
  kind: "carbon" | "silicon";
  org: string;
  tags: string[];
};
export type Table = {
  id: string;
  records: number;
  watermark: number;
  access: string[];
  created_by: string;
  created_at: string;
};
export type Overview = {
  tables: number;
  records: number;
  top: { id: string; records: number }[];
  avg_lag_ms: number | null;
};
/** One named pair of processor and renderer; `version` on a window is the current one, inline. */
export type Code = { name: string; processor: string; renderer: string };
export type Version = Code & {
  id: string;
  created_by: string;
  created_at: string;
};
export type SpaceWindow = {
  id: string;
  name: string;
  access: string[];
  created_by: string;
  created_at: string;
  version: Code | null;
};
export type Notification = {
  id: string;
  def: NotificationDef;
  recipients: string[];
  enabled: boolean;
  created_by: string;
  created_at: string;
};
export type NotificationEvent = {
  id: number;
  dedup_key: string;
  text: string;
  metadata: unknown;
  created_at: string;
};
export type TestResult = {
  rows: unknown[];
  error?: string;
  last_trigger_at: string | null;
};
export type Webhook = {
  id: string;
  url: string;
  created_by: string;
  created_at: string;
};
export type ApiKey = {
  id: string;
  scopes: string[];
  created_by: string;
  created_at: string;
  last_used_at: string | null;
};
export type DevError = {
  id: number;
  source: string;
  ref: string;
  message: string;
  detail: unknown;
  created_at: string;
};
