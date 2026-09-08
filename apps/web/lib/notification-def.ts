// Checks a pasted notification definition (the shape in UNDERSTANDING.md) before it becomes
// `POST /notifications {def, recipients}`. The backend is the gate; this only saves a round trip
// and names the mistake next to the textarea. Also reads back what `POST …/test` answers.
import type { TestResult } from "./api.ts";

export type Trigger = { table: string; where?: string } | { schedule: string };

export type NotificationDef = {
  name: string;
  description?: string;
  enabled?: boolean;
  triggers: Trigger[];
  sql: string;
  delay?: string;
  cooldown?: string;
  access: string[];
};

export type Body = { def: NotificationDef; recipients: string[] };

const KEYS = ["name", "description", "enabled", "triggers", "sql", "delay", "cooldown", "access", "recipients"];
const UNIT_MS: Record<string, number> = { ms: 1, s: 1e3, m: 6e4, h: 36e5, d: 864e5 };
const HOUR = 36e5;
const MONTH = 30 * 864e5;

/** `"2s"` → 2000; `null` unless it matches `^[0-9]+(ms|s|m|h|d)$`. */
export function durationMs(s: unknown): number | null {
  const m = typeof s === "string" ? /^([0-9]+)(ms|s|m|h|d)$/.exec(s) : null;
  return m ? Number(m[1]) * UNIT_MS[m[2]] : null;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isStrings = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === "string");

/** `body` is set only when `errors` is empty. */
export function parseDefinition(text: string): { body: Body | null; errors: string[] } {
  let d: unknown;
  try {
    d = JSON.parse(text);
  } catch (e) {
    return { body: null, errors: [`not JSON: ${(e as Error).message}`] };
  }
  if (!isObject(d)) return { body: null, errors: ["the definition must be a JSON object"] };
  const errors: string[] = [];
  const bad = (msg: string) => void errors.push(msg);

  for (const k of Object.keys(d)) if (!KEYS.includes(k)) bad(`unknown key "${k}"`);
  if (typeof d.name !== "string" || !d.name.trim()) bad("name: a non-empty string");
  if (d.description !== undefined && typeof d.description !== "string") bad("description: a string");
  if (d.enabled !== undefined && typeof d.enabled !== "boolean") bad("enabled: true or false");
  if (typeof d.sql !== "string" || !d.sql.trim()) bad("sql: a SELECT returning dedup_key, text, metadata");
  if (!Array.isArray(d.triggers) || d.triggers.length === 0) bad("triggers: a non-empty array");
  else
    d.triggers.forEach((t: unknown, i: number) => {
      if (!isObject(t)) return bad(`triggers[${i}]: {table, where?} or {schedule}`);
      if ("schedule" in t) {
        if (typeof t.schedule !== "string" || t.schedule.trim().split(/\s+/).length !== 5) bad(`triggers[${i}].schedule: a 5-field cron expression`);
      } else if (typeof t.table !== "string" || !/^[a-z0-9]{1,50}$/.test(t.table)) bad(`triggers[${i}].table: a table id (^[a-z0-9]{1,50}$)`);
      else if (t.where !== undefined && typeof t.where !== "string") bad(`triggers[${i}].where: a SQL condition string`);
    });
  for (const [k, max, label] of [
    ["delay", HOUR, "1h"],
    ["cooldown", MONTH, "30d"],
  ] as const) {
    if (d[k] === undefined) continue;
    const ms = durationMs(d[k]);
    if (ms === null) bad(`${k}: like "2s", "10m" or "1h"`);
    else if (ms > max) bad(`${k}: at most ${label}`);
  }
  if (!isStrings(d.access)) bad('access: an array of "@actor" ids and tag names');
  if (d.recipients !== undefined && (!isStrings(d.recipients) || !d.recipients.every((r) => /^(@|webhook:)./.test(r))))
    bad('recipients: an array of "@actor" or "webhook:<id>"');

  if (errors.length) return { body: null, errors };
  const { recipients = [], ...def } = d as NotificationDef & { recipients?: string[] };
  return { body: { def, recipients }, errors };
}

/**
 * Everything a test run answered, the way the CLIs print it: the SQL error if it failed, a note
 * when the notification has never fired, then the rows. The three are independent.
 */
export const testSummary = (r: TestResult): string =>
  [r.error, r.last_trigger_at === null ? "no trigger seen yet" : null, r.rows.length ? JSON.stringify(r.rows, null, 2) : "No new matching rows."].filter(Boolean).join("\n\n");
