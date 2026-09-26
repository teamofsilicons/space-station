// Tables: the overview (count, records, average lag, the most active over a period, polled every
// 5 s), the list with access, key rotation and retirement, the create dialog that shows the key
// once, and one table's records — a snapshot, newest first, paged by cursor.
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { api, TABLE_ID, type Overview, type Table } from "../lib/api";
import { loadRecordPage, openSnapshot, type Snapshot, type TableRecord } from "../lib/table-records";
import { trackFrontendEvent } from "../lib/telemetry";
import { Access, action, ago, count, Empty, ErrorText, Loading, Modal, openMenu, resource, Secret, Segmented, useTab, when } from "./ui";
import { Link, useWorkspace } from "./tabs";
import { Icon } from "./icons";

const PERIODS = ["1m", "5m", "15m", "1h", "5h", "1d", "7d", "30d"] as const;

/** Creating a table, then its key, shown once; also where a rotated key is shown. */
export function NewTable(p: { close: () => void; rotated?: { table: string; key: string } }) {
  const ws = useWorkspace();
  const [id, setId] = createSignal("");
  const [made, setMade] = createSignal(p.rotated);
  const a = action();
  return (
    <Modal title={made() ? (p.rotated ? "New key" : "Table created") : "New table"} close={p.close}>
      <Show
        when={made()}
        fallback={
          <form
            onSubmit={(e) => {
              e.preventDefault();
              a.run(async () => {
                if (!TABLE_ID.test(id())) throw Error("Use 1–50 lowercase letters and digits.");
                const table = id();
                const { key } = await api<{ key: string }>(`/orgs/${encodeURIComponent(ws.org)}/tables`, "POST", { id: table });
                trackFrontendEvent("table_created", { table });
                setMade({ table, key });
                ws.tables.reload();
              });
            }}
          >
            <label>
              Table ID
              <input required pattern="[a-z0-9]{1,50}" maxlength={50} placeholder="orders" value={id()} onInput={(e) => setId(e.currentTarget.value.toLowerCase())} autofocus />
            </label>
            <p class="hint">Lowercase letters and digits, unique in {ws.org}. Records sent with the table's key land here.</p>
            <ErrorText message={a.error()} />
            <div class="dialog-foot">
              <button type="button" onClick={p.close}>Cancel</button>
              <button class="primary" disabled={a.busy()}>Create table</button>
            </div>
          </form>
        }
      >
        {(m) => (
          <>
            <p>
              The key for <code>{m().table}</code>. Only its hash is kept{p.rotated ? "; the old key has stopped working" : ""}.
            </p>
            <Secret value={m().key} once />
            <div class="dialog-foot">
              <a class="button ghost" href="/docs/getting-started" target="_blank">
                Send your first records <Icon name="external" />
              </a>
              <button
                class="primary"
                onClick={() => {
                  p.close();
                  ws.tabs.reveal(`/o/${ws.org}/tables/${m().table}`, "tab");
                }}
              >
                Open {m().table}
              </button>
            </div>
          </>
        )}
      </Show>
    </Modal>
  );
}

export function Tables() {
  const ws = useWorkspace();
  const root = `/orgs/${encodeURIComponent(ws.org)}`;
  const [retired, setRetired] = createSignal(false);
  const [period, setPeriod] = createSignal<(typeof PERIODS)[number]>("5h");
  const tables = resource(() => api<Table[]>(`${root}/tables?retired=${retired()}`), 5000, () => (retired() ? "retired" : "active"));
  const overview = resource(() => api<Overview>(`${root}/tables/overview?window=${period()}&retired=${retired()}`), 5000, () => `${period()}:${retired()}`);
  const [rotated, setRotated] = createSignal<{ table: string; key: string }>();
  const a = action();
  const reload = () => Promise.all([tables.reload(), ws.tables.reload()]);
  const rows = () => tables.data()?.filter((t) => Boolean(t.retired_at) === retired());
  const top = () => Math.max(1, ...(overview.data()?.top.map((t) => t.records) || []));
  const menu = (e: MouseEvent, t: Table) =>
    openMenu(e, [
      { label: "Open in new tab", icon: "external", run: () => ws.tabs.open(`/o/${ws.org}/tables/${t.id}`, "tab") },
      { label: "Open in split view", icon: "split", run: () => ws.tabs.open(`/o/${ws.org}/tables/${t.id}`, "split") },
      "-",
      ...(retired()
        ? [{ label: "Restore", icon: "reload", run: () => a.run(async () => { await api(`${root}/tables/${t.id}/unretire`, "POST"); trackFrontendEvent("table_restored", { table: t.id }); await reload(); }) }]
        : [
            {
              label: "Rotate key",
              icon: "key",
              run: () =>
                confirm(`Rotate the key for ${t.id}? The current key will stop working.`) &&
                a.run(async () => setRotated({ table: t.id, key: (await api<{ key: string }>(`${root}/tables/${t.id}/rotate-key`, "POST")).key })),
            },
            {
              label: "Retire",
              icon: "archive",
              danger: true,
              run: () =>
                confirm(`Retire ${t.id}? Its key will stop accepting new records, while existing records remain queryable.`) &&
                a.run(async () => { await api(`${root}/tables/${t.id}/retire`, "POST"); trackFrontendEvent("table_retired", { table: t.id }); await reload(); }),
            },
          ]),
    ]);
  return (
    <div class="page-body">
      <header class="page-head">
        <div>
          <h1>{retired() ? "Retired tables" : "Tables"}</h1>
          <p class="muted">Records arrive with a table's key. Space Windows and notifications read them with SQL.</p>
        </div>
        <Show when={!retired()}>
          <button class="primary" onClick={() => ws.create("table")}>
            <Icon name="plus" /> New table
          </button>
        </Show>
      </header>
      <Show when={!retired() && rows() && !rows()!.length} fallback={
        <>
          <section class="panel">
            <div class="panel-head">
              <h2>Activity</h2>
              <Segmented label="Activity time range" value={period()} options={PERIODS} onChange={setPeriod} />
            </div>
            <Show when={overview.data()} fallback={<Loading error={overview.data.error} />}>
              {(o) => (
                <div class="overview">
                  <div class="stats">
                    <div class="stat">
                      <span>Tables</span>
                      <b>{o().tables.toLocaleString()}</b>
                    </div>
                    <div class="stat">
                      <span>Records</span>
                      <b>{o().records.toLocaleString()}</b>
                    </div>
                    <div class="stat">
                      <span>Ingestion lag</span>
                      <b>{o().avg_lag_ms == null ? "—" : Math.round(o().avg_lag_ms!).toLocaleString()}<small>{o().avg_lag_ms == null ? "" : " ms"}</small></b>
                      <em>average of the last 100</em>
                    </div>
                  </div>
                  <div class="top">
                    <span class="kicker">Most active · last {period()}</span>
                    <Show when={o().top.length} fallback={<p class="muted">Nothing arrived in this period.</p>}>
                      <For each={o().top}>
                        {(t) => (
                          <Link href={`/o/${ws.org}/tables/${t.id}`} class="bar-row">
                            <code>{t.id}</code>
                            <span class="bar"><i style={{ width: `${(t.records / top()) * 100}%` }} /></span>
                            <span class="num">{count(t.records)}</span>
                          </Link>
                        )}
                      </For>
                    </Show>
                  </div>
                </div>
              )}
            </Show>
          </section>
          <div class="list-head">
            <Segmented label="Table status" value={retired() ? "retired" : "active"} options={["active", "retired"] as const} names={{ active: "Active", retired: "Retired" }} onChange={(v) => setRetired(v === "retired")} />
            <ErrorText message={a.error()} />
          </div>
          <Show when={rows()} fallback={<Loading error={tables.data.error} />}>
            {(list) => (
              <Show when={list().length} fallback={<p class="empty-line">{retired() ? "No retired tables." : "No tables yet."}</p>}>
                <div class="grid-table" role="table">
                  <div class="gt-head" role="row">
                    <span>Table</span>
                    <span class="num">Records</span>
                    <span class="wide">Access</span>
                    <span class="wide">Created</span>
                    <span />
                  </div>
                  <For each={list()}>
                    {(t) => (
                      <div class="gt-row" role="row" onContextMenu={(e) => menu(e, t)}>
                        <Link href={`/o/${ws.org}/tables/${t.id}`} class="gt-main">
                          <Icon name="table" />
                          <code>{t.id}</code>
                        </Link>
                        <span class="num">{t.records.toLocaleString()}</span>
                        <span class="wide">
                          <Access value={t.access} save={async (access) => { await api(`${root}/tables/${t.id}`, "PUT", { access }); await reload(); }} />
                        </span>
                        <span class="wide muted" title={when(t.created_at)}>
                          @{t.created_by} · {ago(t.created_at)}
                        </span>
                        <button class="icon" aria-label={`Actions for ${t.id}`} disabled={a.busy()} onClick={(e) => menu(e, t)}>
                          <Icon name="more" />
                        </button>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            )}
          </Show>
        </>
      }>
        <div class="hero-empty">
          <Empty icon="table" title="No tables yet">
            <p>A table is where an app's records land. It only needs an id; its key is shown once.</p>
            <button class="primary large" onClick={() => ws.create("table")}>
              <Icon name="plus" /> Create your first table
            </button>
          </Empty>
          <p class="hero-foot">
            <button class="link" onClick={() => setRetired(true)}>Show retired tables</button>
          </p>
        </div>
      </Show>
      <Show when={rotated()} keyed>
        {(r) => <NewTable rotated={r} close={() => setRotated()} />}
      </Show>
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
    <span class="rec-preview">
      <Show when={p.value && typeof p.value === "object" && !Array.isArray(p.value)} fallback={short(p.value)}>
        <For each={Object.entries(p.value as Record<string, unknown>).slice(0, 14)}>
          {([k, v]) => (
            <span class="kv">
              <i>{k}</i>
              {short(v)}
            </span>
          )}
        </For>
      </Show>
    </span>
  );
}

export function TableView() {
  const tab = useTab();
  const ws = useWorkspace();
  const id = () => tab.route().id;
  tab.title(id());
  const [snapshot, setSnapshot] = createSignal<Snapshot>();
  const [pages, setPages] = createSignal<TableRecord[][]>([]);
  const [page, setPage] = createSignal(0);
  const [limit, setLimit] = createSignal(100);
  const [draftLimit, setDraftLimit] = createSignal("100");
  const [size, setSize] = createSignal(25);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [retryPage, setRetryPage] = createSignal<number>();
  const [open, setOpen] = createSignal<string>();
  let generation = 0;
  onCleanup(() => generation++);
  const total = () => Math.min(snapshot()?.count ?? 0, limit());
  const pageCount = () => Math.max(1, Math.ceil(total() / size()));
  const rows = () => pages()[page()] ?? [];

  async function go(index: number, token = generation) {
    const current = snapshot();
    if (!current || index < 0 || index >= pageCount()) return;
    if (pages()[index]) {
      setPage(index);
      setError("");
      return;
    }
    setBusy(true);
    setError("");
    setRetryPage(undefined);
    try {
      const before = index ? pages()[index - 1]?.at(-1)?.cursor : undefined;
      if (index && !before) throw Error("No more records in this snapshot.");
      const result = total() ? await loadRecordPage(ws.org, id(), current.cursor, before, Math.min(size(), total() - index * size())) : [];
      if (token !== generation) return;
      setPages((old) => {
        const next = [...old];
        next[index] = result;
        return next;
      });
      setPage(index);
    } catch (e) {
      if (token === generation) {
        setRetryPage(index);
        setError(String(e instanceof Error ? e.message : e));
      }
    } finally {
      if (token === generation) setBusy(false);
    }
  }

  async function enter() {
    const token = ++generation;
    setSnapshot(undefined);
    setPages([]);
    setPage(0);
    setBusy(true);
    setError("");
    try {
      const result = await openSnapshot(ws.org, id());
      if (token !== generation) return;
      setSnapshot(result);
      await go(0, token);
    } catch (e) {
      if (token === generation) setError(String(e instanceof Error ? e.message : e));
    } finally {
      if (token === generation) setBusy(false);
    }
  }
  createEffect(() => void enter());

  function apply(nextSize = size()) {
    const value = Number(draftLimit());
    if (!Number.isInteger(value) || value < 1 || value > 10000) {
      setError("Choose between 1 and 10,000 recent records.");
      return;
    }
    ++generation;
    setLimit(value);
    setSize(nextSize);
    setPages([]);
    setPage(0);
    void go(0);
  }
  const time = (ms: string) => new Date(Number(ms));

  return (
    <div class="page-body">
      <header class="page-head">
        <div>
          <div class="title-row">
            <span class="title-icon"><Icon name="table" size={18} /></span>
            <h1 class="mono">{id()}</h1>
            <Show when={snapshot()?.table.retired_at}>
              <span class="pill">Retired</span>
            </Show>
          </div>
          <Show when={snapshot()} fallback={<p class="muted">Opening a snapshot…</p>}>
            {(s) => (
              <p class="meta">
                <span><b>{s().count.toLocaleString()}</b> records</span>
                <span>created by @{s().table.created_by} · {ago(s().table.created_at)}</span>
                <Access value={s().table.access} />
              </p>
            )}
          </Show>
        </div>
        <button disabled={busy()} onClick={() => void enter()} title="Take a new snapshot to see records that arrived since">
          <Icon name="reload" /> Refresh
        </button>
      </header>
      <Show when={snapshot()}>
        {(s) => (
          <p class="notice">
            <Icon name="clock" />
            {s().table.retired_at
              ? "This table is retired. New records are blocked; its history remains available."
              : `Snapshot taken ${s().openedAt.toLocaleTimeString()}. Refresh to include records that arrived since.`}
          </p>
        )}
      </Show>
      <section class="panel flush">
        <form
          class="records-bar"
          onSubmit={(e) => {
            e.preventDefault();
            apply();
          }}
        >
          <label class="inline">
            Last
            <input aria-label="Last N records" type="number" min="1" max="10000" step="1" value={draftLimit()} onInput={(e) => setDraftLimit(e.currentTarget.value)} onBlur={() => draftLimit() !== String(limit()) && apply()} disabled={busy()} />
            records
          </label>
          <label class="inline">
            <select aria-label="Records per page" value={size()} disabled={busy()} onChange={(e) => apply(Number(e.currentTarget.value))}>
              <For each={[25, 50, 100]}>{(n) => <option value={n}>{n} per page</option>}</For>
            </select>
          </label>
          <span class="muted grow newest">Newest first</span>
          <span class="pager" aria-label="Record pagination">
            <span class="muted">
              {total() ? page() * size() + 1 : 0}–{Math.min(page() * size() + rows().length, total())} of {total().toLocaleString()}
            </span>
            <button type="button" class="icon" aria-label="Previous page" disabled={busy() || page() === 0} onClick={() => void go(page() - 1)}>
              <Icon name="back" />
            </button>
            <button type="button" class="icon" aria-label="Next page" disabled={busy() || page() + 1 >= pageCount() || rows().length < size()} onClick={() => void go(page() + 1)}>
              <Icon name="forward" />
            </button>
          </span>
        </form>
        <Show when={rows().length} fallback={<Show when={!busy() && !error()}><p class="empty-line">No records in this snapshot.</p></Show>}>
          <div class="records" role="table">
            <div class="rec-head" role="row">
              <span>Received</span>
              <span>Event time</span>
              <span>Record</span>
            </div>
            <For each={rows()}>
              {(row) => (
                <div class="rec" classList={{ open: open() === row.record_id }} role="row">
                  <button type="button" class="rec-line" onClick={() => setOpen(open() === row.record_id ? undefined : row.record_id)} aria-expanded={open() === row.record_id}>
                    <span title={time(row.registered_ts_ms).toLocaleString()}>{time(row.registered_ts_ms).toLocaleTimeString()}<small>{time(row.registered_ts_ms).toLocaleDateString()}</small></span>
                    <span title={time(row.event_ts_ms).toLocaleString()}>{time(row.event_ts_ms).toLocaleTimeString()}<small>+{Math.max(0, Number(row.registered_ts_ms) - Number(row.event_ts_ms)).toLocaleString()} ms</small></span>
                    <Preview value={row.record} />
                  </button>
                  <Show when={open() === row.record_id}>
                    <div class="rec-detail">
                      <div>
                        <span class="kicker">Record</span>
                        <pre>{JSON.stringify(row.record, null, 2)}</pre>
                      </div>
                      <div>
                        <span class="kicker">Metadata</span>
                        <pre>{JSON.stringify(row.metadata, null, 2)}</pre>
                      </div>
                      <small class="muted">Record ID {row.record_id} · cursor {row.cursor}</small>
                    </div>
                  </Show>
                </div>
              )}
            </For>
          </div>
        </Show>
        <Show when={busy()}>
          <p class="loading" role="status">Loading records…</p>
        </Show>
        <ErrorText message={error()} />
        <Show when={error() && !busy() && (!snapshot() || retryPage() !== undefined)}>
          <button onClick={() => (snapshot() ? void go(retryPage()!) : void enter())}>Retry</button>
        </Show>
      </section>
    </div>
  );
}
