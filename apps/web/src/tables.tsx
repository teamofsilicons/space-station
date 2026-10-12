"use client";
// Tables: the overview (count, records, average lag, the most active over a period, polled every
// 5 s), the list with ownership, key rotation and retirement, the create dialog that shows the key
// once, and one table's records — a snapshot, newest first, paged by cursor.
import { useEffect, useRef, useState, type MouseEvent } from "react";
import { Badge } from "@/components/silicon-ui/badge/badge";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { JsonViewer } from "@/components/silicon-ui/json-viewer/json-viewer";
import { api, TABLE_ID, type Overview, type Table } from "../lib/api";
import { loadRecordPage, openSnapshot, type Snapshot, type TableRecord } from "../lib/table-records";
import { ago, count, Empty, ErrorText, Loading, Modal, openMenu, Secret, Segmented, useAction, useResource, useTab, useTitle, when } from "./ui";
import { Link, useLists, useWorkspace } from "./tabs";
import { Icon } from "./icons";

const PERIODS = ["1m", "5m", "15m", "1h", "5h", "1d", "7d", "30d"] as const;

/** Creating a table, then its key, shown once; also where a rotated key is shown. */
export function NewTable(p: { close: () => void; rotated?: { table: string; key: string } }) {
  const ws = useWorkspace();
  const lists = useLists();
  const [id, setId] = useState("");
  const [made, setMade] = useState(p.rotated);
  const a = useAction();
  return (
    <Modal title={made ? (p.rotated ? "New key" : "Table created") : "New table"} close={p.close}>
      {made ? (
        <>
          <p>
            The key for <code>{made.table}</code>. Only its hash is kept{p.rotated ? "; the old key has stopped working" : ""}.
          </p>
          <Secret value={made.key} once />
          <div className="dialog-foot">
            <a className="button ghost" href="/docs/getting-started" target="_blank">
              Send your first records <Icon name="external" />
            </a>
            <Button
              onClick={() => {
                p.close();
                ws.tabs.reveal(`/a/${ws.actor}/tables/${made.table}`, "tab");
              }}
            >
              Open {made.table}
            </Button>
          </div>
        </>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            a.run(async () => {
              if (!TABLE_ID.test(id)) throw Error("Use 1–50 lowercase letters and digits.");
              const { key } = await api<{ key: string }>(`/tables`, "POST", { id });
              setMade({ table: id, key });
              lists.tables.reload();
            });
          }}
        >
          <Input
            label="Table ID"
            description="Lowercase letters and digits, unique to your account. Records sent with the table's key land here."
            required
            pattern="[a-z0-9]{1,50}"
            maxLength={50}
            placeholder="orders"
            value={id}
            onChange={(e) => setId(e.currentTarget.value.toLowerCase())}
            autoFocus
          />
          <ErrorText message={a.error} />
          <div className="dialog-foot">
            <Button type="button" variant="secondary" onClick={p.close}>
              Cancel
            </Button>
            <Button type="submit" loading={a.busy}>
              Create table
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

export function Tables() {
  const ws = useWorkspace();
  const lists = useLists();
  const [retired, setRetired] = useState(false);
  const [period, setPeriod] = useState<(typeof PERIODS)[number]>("5h");
  const tables = useResource(() => api<Table[]>(`/tables?retired=${retired}`), 5000, [retired]);
  const overview = useResource(() => api<Overview>(`/tables/overview?window=${period}&retired=${retired}`), 5000, [period, retired]);
  const [rotated, setRotated] = useState<{ table: string; key: string }>();
  const a = useAction();
  const reload = () => Promise.all([tables.reload(), lists.tables.reload()]);
  const rows = tables.data?.filter((t) => Boolean(t.retired_at) === retired);
  const top = Math.max(1, ...(overview.data?.top.map((t) => t.records) || []));
  const menu = (e: MouseEvent, t: Table) =>
    openMenu(e, [
      { label: "Open in new tab", icon: "external", run: () => ws.tabs.open(`/a/${ws.actor}/tables/${t.id}`, "tab") },
      { label: "Open in split view", icon: "split", run: () => ws.tabs.open(`/a/${ws.actor}/tables/${t.id}`, "split") },
      "-",
      ...(retired
        ? [{ label: "Restore", icon: "reload", run: () => a.run(async () => { await api(`/tables/${t.id}/unretire`, "POST"); await reload(); }) }]
        : [
            {
              label: "Rotate key",
              icon: "key",
              run: () =>
                confirm(`Rotate the key for ${t.id}? The current key will stop working.`) &&
                a.run(async () => setRotated({ table: t.id, key: (await api<{ key: string }>(`/tables/${t.id}/rotate-key`, "POST")).key })),
            },
            {
              label: "Retire",
              icon: "archive",
              danger: true,
              run: () =>
                confirm(`Retire ${t.id}? Its key will stop accepting new records, while existing records remain queryable.`) &&
                a.run(async () => { await api(`/tables/${t.id}/retire`, "POST"); await reload(); }),
            },
          ]),
    ]);
  const o = overview.data;
  return (
    <div className="page-body">
      <header className="page-head">
        <div>
          <h1>{retired ? "Retired tables" : "Tables"}</h1>
          <p className="muted">Records arrive with a table&apos;s key. Space Windows and notifications read them with SQL.</p>
        </div>
        {!retired && (
          <Button onClick={() => ws.create("table")}>
            <Icon name="plus" /> New table
          </Button>
        )}
      </header>
      {!retired && rows && !rows.length ? (
        <div className="hero-empty">
          <Empty
            icon="table"
            title="No tables yet"
            description="A table is where an app's records land. It only needs an id; its key is shown once."
            action={
              <Button size="lg" onClick={() => ws.create("table")}>
                <Icon name="plus" /> Create your first table
              </Button>
            }
          />
          <p className="hero-foot">
            <Button variant="ghost" className="link" onClick={() => setRetired(true)}>
              Show retired tables
            </Button>
          </p>
        </div>
      ) : (
        <>
          <section className="panel">
            <div className="panel-head">
              <h2>Activity</h2>
              <Segmented label="Activity time range" value={period} options={PERIODS} onChange={setPeriod} />
            </div>
            {!o ? (
              <Loading error={overview.error} />
            ) : (
              <div className="overview">
                <div className="stats">
                  <div className="stat">
                    <span>Tables</span>
                    <b>{o.tables.toLocaleString()}</b>
                  </div>
                  <div className="stat">
                    <span>Records</span>
                    <b>{o.records.toLocaleString()}</b>
                  </div>
                  <div className="stat">
                    <span>Ingestion lag</span>
                    <b>
                      {o.avg_lag_ms == null ? "—" : Math.round(o.avg_lag_ms).toLocaleString()}
                      <small>{o.avg_lag_ms == null ? "" : " ms"}</small>
                    </b>
                    <em>average of the last 100</em>
                  </div>
                </div>
                <div className="top">
                  <span className="kicker">Most active · last {period}</span>
                  {o.top.length ? (
                    o.top.map((t) => (
                      <Link key={t.id} href={`/a/${ws.actor}/tables/${t.id}`} className="bar-row">
                        <code>{t.id}</code>
                        <span className="bar">
                          <i style={{ width: `${(t.records / top) * 100}%` }} />
                        </span>
                        <span className="num">{count(t.records)}</span>
                      </Link>
                    ))
                  ) : (
                    <p className="muted">Nothing arrived in this period.</p>
                  )}
                </div>
              </div>
            )}
          </section>
          <div className="list-head">
            <Segmented label="Table status" value={retired ? "retired" : "active"} options={["active", "retired"] as const} names={{ active: "Active", retired: "Retired" }} onChange={(v) => setRetired(v === "retired")} />
            <ErrorText message={a.error} />
          </div>
          {!rows ? (
            <Loading error={tables.error} />
          ) : rows.length ? (
            <div className="grid-table" role="table">
              <div className="gt-head" role="row">
                <span>Table</span>
                <span className="num">Records</span>
                <span className="wide">Owner</span>
                <span className="wide">Created</span>
                <span />
              </div>
              {rows.map((t) => (
                <div key={t.id} className="gt-row" role="row" onContextMenu={(e) => menu(e, t)}>
                  <Link href={`/a/${ws.actor}/tables/${t.id}`} className="gt-main">
                    <Icon name="table" />
                    <code>{t.id}</code>
                  </Link>
                  <span className="num">{t.records.toLocaleString()}</span>
                  <span className="wide">
                    <Badge>{ws.me.id}</Badge>
                  </span>
                  <span className="wide muted" title={when(t.created_at)}>
                    @{t.created_by} · {ago(t.created_at)}
                  </span>
                  <Button variant="ghost" size="sm" className="icon" aria-label={`Actions for ${t.id}`} disabled={a.busy} onClick={(e) => menu(e, t)}>
                    <Icon name="more" />
                  </Button>
                </div>
              ))}
            </div>
          ) : (
            <p className="empty-line">{retired ? "No retired tables." : "No tables yet."}</p>
          )}
        </>
      )}
      {rotated && <NewTable key={rotated.key} rotated={rotated} close={() => setRotated(undefined)} />}
    </div>
  );
}

/** A record at a glance: its top-level fields as key · value, cut short; anything else as JSON. */
function Preview(p: { value: unknown }) {
  const short = (v: unknown) => {
    const s = typeof v === "string" ? v : JSON.stringify(v);
    return s.length > 48 ? s.slice(0, 47) + "…" : s;
  };
  return (
    <span className="rec-preview">
      {p.value && typeof p.value === "object" && !Array.isArray(p.value)
        ? Object.entries(p.value as Record<string, unknown>)
            .slice(0, 14)
            .map(([k, v]) => (
              <span key={k} className="kv">
                <i>{k}</i>
                {short(v)}
              </span>
            ))
        : short(p.value)}
    </span>
  );
}

/** What a page of records is cut from; a load is handed the one it starts, which may be newer than this render. */
type View = { snapshot?: Snapshot; pages: TableRecord[][]; limit: number; size: number };
const totalOf = (v: View) => Math.min(v.snapshot?.count ?? 0, v.limit);
const pageCountOf = (v: View) => Math.max(1, Math.ceil(totalOf(v) / v.size));

export function TableView() {
  const tab = useTab();
  const ws = useWorkspace();
  const id = tab.route.id;
  useTitle(id);
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [pages, setPages] = useState<TableRecord[][]>([]);
  const [page, setPage] = useState(0);
  const [limit, setLimit] = useState(100);
  const [draftLimit, setDraftLimit] = useState("100");
  const [size, setSize] = useState(25);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [retryPage, setRetryPage] = useState<number>();
  const [open, setOpen] = useState<string>();
  const generation = useRef(0);
  const view: View = { snapshot, pages, limit, size };
  const total = totalOf(view);
  const pageCount = pageCountOf(view);
  const rows = pages[page] ?? [];

  async function go(index: number, token = generation.current, at = view) {
    const current = at.snapshot;
    if (!current || index < 0 || index >= pageCountOf(at)) return;
    if (at.pages[index]) {
      setPage(index);
      setError("");
      return;
    }
    setBusy(true);
    setError("");
    setRetryPage(undefined);
    try {
      const before = index ? at.pages[index - 1]?.at(-1)?.cursor : undefined;
      if (index && !before) throw Error("No more records in this snapshot.");
      const total = totalOf(at);
      const result = total ? await loadRecordPage(id, current.cursor, before, Math.min(at.size, total - index * at.size)) : [];
      if (token !== generation.current) return;
      setPages((old) => {
        const next = [...old];
        next[index] = result;
        return next;
      });
      setPage(index);
    } catch (e) {
      if (token === generation.current) {
        setRetryPage(index);
        setError(String(e instanceof Error ? e.message : e));
      }
    } finally {
      if (token === generation.current) setBusy(false);
    }
  }

  async function enter() {
    const token = ++generation.current;
    setSnapshot(undefined);
    setPages([]);
    setPage(0);
    setBusy(true);
    setError("");
    try {
      const result = await openSnapshot(id);
      if (token !== generation.current) return;
      setSnapshot(result);
      await go(0, token, { snapshot: result, pages: [], limit, size });
    } catch (e) {
      if (token === generation.current) setError(String(e instanceof Error ? e.message : e));
    } finally {
      if (token === generation.current) setBusy(false);
    }
  }
  useEffect(() => {
    void enter();
    return () => void generation.current++;
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  function apply(nextSize = size) {
    const value = Number(draftLimit);
    if (!Number.isInteger(value) || value < 1 || value > 10000) {
      setError("Choose between 1 and 10,000 recent records.");
      return;
    }
    ++generation.current;
    setLimit(value);
    setSize(nextSize);
    setPages([]);
    setPage(0);
    void go(0, generation.current, { snapshot, pages: [], limit: value, size: nextSize });
  }
  const time = (ms: string) => new Date(Number(ms));

  return (
    <div className="page-body">
      <header className="page-head">
        <div>
          <div className="title-row">
            <span className="title-icon">
              <Icon name="table" size={18} />
            </span>
            <h1 className="mono">{id}</h1>
            {snapshot?.table.retired_at && <Badge>Retired</Badge>}
          </div>
          {snapshot ? (
            <p className="meta">
              <span>
                <b>{snapshot.count.toLocaleString()}</b> records
              </span>
              <span>
                created by @{snapshot.table.created_by} · {ago(snapshot.table.created_at)}
              </span>
              <Badge>{ws.me.id}</Badge>
            </p>
          ) : (
            <p className="muted">Opening a snapshot…</p>
          )}
        </div>
        <Button variant="secondary" disabled={busy} onClick={() => void enter()} title="Take a new snapshot to see records that arrived since">
          <Icon name="reload" /> Refresh
        </Button>
      </header>
      {snapshot && (
        <p className="notice">
          <Icon name="clock" />
          {snapshot.table.retired_at
            ? "This table is retired. New records are blocked; its history remains available."
            : `Snapshot taken ${snapshot.openedAt.toLocaleTimeString()}. Refresh to include records that arrived since.`}
        </p>
      )}
      <section className="panel flush">
        <form
          className="records-bar"
          onSubmit={(e) => {
            e.preventDefault();
            apply();
          }}
        >
          <label className="inline">
            Last
            <input aria-label="Last N records" type="number" min="1" max="10000" step="1" value={draftLimit} onChange={(e) => setDraftLimit(e.currentTarget.value)} onBlur={() => draftLimit !== String(limit) && apply()} disabled={busy} />
            records
          </label>
          <label className="inline">
            <select aria-label="Records per page" value={size} disabled={busy} onChange={(e) => apply(Number(e.currentTarget.value))}>
              {[25, 50, 100].map((n) => (
                <option key={n} value={n}>
                  {n} per page
                </option>
              ))}
            </select>
          </label>
          <span className="muted grow newest">Newest first</span>
          <span className="pager" aria-label="Record pagination">
            <span className="muted">
              {total ? page * size + 1 : 0}–{Math.min(page * size + rows.length, total)} of {total.toLocaleString()}
            </span>
            <Button type="button" variant="ghost" size="sm" className="icon" aria-label="Previous page" disabled={busy || page === 0} onClick={() => void go(page - 1)}>
              <Icon name="back" />
            </Button>
            <Button type="button" variant="ghost" size="sm" className="icon" aria-label="Next page" disabled={busy || page + 1 >= pageCount || rows.length < size} onClick={() => void go(page + 1)}>
              <Icon name="forward" />
            </Button>
          </span>
        </form>
        {rows.length ? (
          <div className="records" role="table">
            <div className="rec-head" role="row">
              <span>Received</span>
              <span>Event time</span>
              <span>Record</span>
            </div>
            {rows.map((row) => {
              const shown = open === row.record_id;
              return (
                <div key={row.cursor} className={["rec", shown && "open"].filter(Boolean).join(" ")} role="row">
                  <button type="button" className="rec-line" onClick={() => setOpen(shown ? undefined : row.record_id)} aria-expanded={shown}>
                    <span title={time(row.registered_ts_ms).toLocaleString()}>
                      {time(row.registered_ts_ms).toLocaleTimeString()}
                      <small>{time(row.registered_ts_ms).toLocaleDateString()}</small>
                    </span>
                    <span title={time(row.event_ts_ms).toLocaleString()}>
                      {time(row.event_ts_ms).toLocaleTimeString()}
                      <small>+{Math.max(0, Number(row.registered_ts_ms) - Number(row.event_ts_ms)).toLocaleString()} ms</small>
                    </span>
                    <Preview value={row.record} />
                  </button>
                  {shown && (
                    <div className="rec-detail">
                      <div>
                        <span className="kicker">Record</span>
                        <JsonViewer data={row.record} rootName="record" label="Record" defaultExpandDepth={Infinity} maxHeight="28rem" />
                      </div>
                      <div>
                        <span className="kicker">Metadata</span>
                        <JsonViewer data={row.metadata} rootName="metadata" label="Metadata" defaultExpandDepth={Infinity} maxHeight="28rem" />
                      </div>
                      <small className="muted">
                        Record ID {row.record_id} · cursor {row.cursor}
                      </small>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ) : (
          !busy && !error && <p className="empty-line">No records in this snapshot.</p>
        )}
        {busy && (
          <p className="loading" role="status">
            Loading records…
          </p>
        )}
        <ErrorText message={error} />
        {error && !busy && (!snapshot || retryPage !== undefined) && (
          <Button variant="secondary" onClick={() => (snapshot ? void go(retryPage!) : void enter())}>
            Retry
          </Button>
        )}
      </section>
    </div>
  );
}
