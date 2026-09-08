import { api, TABLE_ID, type Table } from "./api";

export type TableRecord = {
  cursor: string;
  record_id: string;
  event_ts_ms: string;
  registered_ts_ms: string;
  metadata: unknown;
  record: unknown;
};
type Query<T> = { rows: T[] };
export type Snapshot = {
  table: Table;
  cursor: string;
  count: number;
  openedAt: Date;
};
const integer = (value: string) => {
  if (!/^\d+$/.test(value)) throw Error("Invalid record cursor.");
  return value;
};
const tableName = (id: string) => {
  if (!TABLE_ID.test(id)) throw Error("Invalid table name.");
  return `\`${id}\``;
};

export async function openSnapshot(org: string, id: string): Promise<Snapshot> {
  const table = tableName(id);
  const root = `/orgs/${encodeURIComponent(org)}`;
  const openedAt = new Date();
  const [tables, result] = await Promise.all([
    api<Table[]>(`${root}/tables`),
    api<Query<{ last: string; count: string }>>(`${root}/query`, "POST", {
      sql: `SELECT toString(max(cursor)) AS last, toString(count()) AS count FROM ${table}`,
    }),
  ]);
  const details = tables.find((t) => t.id === id);
  if (!details)
    throw Error("This table does not exist or you no longer have access.");
  const row = result.rows[0];
  if (!row) throw Error("The table snapshot could not be loaded.");
  return {
    table: details,
    cursor: integer(row.last),
    count: Number(integer(row.count)),
    openedAt,
  };
}

export function pageQuery(
  id: string,
  snapshot: string,
  before: string | undefined,
  limit: number,
) {
  if (!Number.isInteger(limit) || limit < 1 || limit > 100)
    throw Error("Invalid page size.");
  return {
    sql: `SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM ${tableName(id)} WHERE cursor <= ${integer(snapshot)}${before === undefined ? "" : ` AND cursor < ${integer(before)}`} ORDER BY cursor DESC LIMIT ${limit}`,
  };
}

export async function loadRecordPage(
  org: string,
  id: string,
  snapshot: string,
  before: string | undefined,
  limit: number,
) {
  const result = await api<Query<TableRecord>>(
    `/orgs/${encodeURIComponent(org)}/query`,
    "POST",
    pageQuery(id, snapshot, before, limit),
  );
  return result.rows;
}
