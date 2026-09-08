import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { A, useParams } from "@solidjs/router";
import {
  loadRecordPage,
  openSnapshot,
  type Snapshot,
  type TableRecord,
} from "../lib/table-records";
import { ErrorText, when } from "./ui";

export function TableView() {
  const params = useParams<{ org: string; id: string }>();
  const [snapshot, setSnapshot] = createSignal<Snapshot>();
  const [pages, setPages] = createSignal<TableRecord[][]>([]);
  const [page, setPage] = createSignal(0);
  const [limit, setLimit] = createSignal(100);
  const [draftLimit, setDraftLimit] = createSignal("100");
  const [size, setSize] = createSignal(25);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [retryPage, setRetryPage] = createSignal<number>();
  let generation = 0;
  onCleanup(() => {
    generation++;
  });
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
      const previous = index ? pages()[index - 1] : undefined;
      const before = previous?.at(-1)?.cursor;
      if (index && !before) throw Error("No more records in this snapshot.");
      const result = total()
        ? await loadRecordPage(
            params.org,
            params.id,
            current.cursor,
            before,
            Math.min(size(), total() - index * size()),
          )
        : [];
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

  async function enter(org: string, id: string) {
    const token = ++generation;
    setSnapshot(undefined);
    setPages([]);
    setPage(0);
    setBusy(true);
    setError("");
    try {
      const result = await openSnapshot(org, id);
      if (token !== generation) return;
      setSnapshot(result);
      await go(0, token);
    } catch (e) {
      if (token === generation)
        setError(String(e instanceof Error ? e.message : e));
    } finally {
      if (token === generation) setBusy(false);
    }
  }
  createEffect(() => {
    void enter(params.org, params.id);
  });

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

  return (
    <>
      <A href={`/o/${params.org}/tables`}>← Tables</A>
      <h1 class="table-title">{params.id}</h1>
      <Show when={snapshot()}>
        {(s) => (
          <>
            <p class="muted">
              Snapshot taken {when(s().openedAt.toISOString())}. New records
              will appear when you reopen this table.
            </p>
            <section class="stats">
              <div>
                <b>{s().count.toLocaleString()}</b>
                <span>records in snapshot</span>
              </div>
              <div>
                <b>@{s().table.created_by}</b>
                <span>created by · {when(s().table.created_at)}</span>
              </div>
              <div>
                <b>{s().table.access.join(", ") || "Creator only"}</b>
                <span>access</span>
              </div>
            </section>
            <section>
              <h2>Recent records</h2>
              <form
                class="row record-controls"
                onSubmit={(e) => {
                  e.preventDefault();
                  apply();
                }}
              >
                <label>
                  Last N records
                  <input
                    aria-label="Last N records"
                    type="number"
                    min="1"
                    max="10000"
                    step="1"
                    value={draftLimit()}
                    onInput={(e) => setDraftLimit(e.currentTarget.value)}
                    disabled={busy()}
                  />
                </label>
                <button type="submit" disabled={busy()}>
                  Apply
                </button>
                <label>
                  Per page
                  <select
                    aria-label="Records per page"
                    value={size()}
                    disabled={busy()}
                    onChange={(e) => apply(Number(e.currentTarget.value))}
                  >
                    <For each={[25, 50, 100]}>
                      {(n) => <option value={n}>{n}</option>}
                    </For>
                  </select>
                </label>
                <span class="muted">Newest first</span>
              </form>
              <Show
                when={rows().length}
                fallback={
                  <Show when={!busy() && !error()}>
                    <p class="empty">No records in this snapshot.</p>
                  </Show>
                }
              >
                <div class="table-scroll">
                  <table class="record-table">
                    <thead>
                      <tr>
                        <th>Received</th>
                        <th>Event time</th>
                        <th>Record</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={rows()}>
                        {(row) => (
                          <tr>
                            <td>
                              {when(
                                new Date(
                                  Number(row.registered_ts_ms),
                                ).toISOString(),
                              )}
                            </td>
                            <td>
                              {when(
                                new Date(Number(row.event_ts_ms)).toISOString(),
                              )}
                            </td>
                            <td>
                              <details>
                                <summary>
                                  <code>{JSON.stringify(row.record)}</code>
                                </summary>
                                <h3>Record</h3>
                                <pre>{JSON.stringify(row.record, null, 2)}</pre>
                                <h3>Metadata</h3>
                                <pre>
                                  {JSON.stringify(row.metadata, null, 2)}
                                </pre>
                                <small>
                                  Record ID: {row.record_id} · Cursor:{" "}
                                  {row.cursor}
                                </small>
                              </details>
                            </td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                </div>
              </Show>
              <div class="row record-pagination" aria-label="Record pagination">
                <button
                  disabled={busy() || page() === 0}
                  onClick={() => void go(page() - 1)}
                >
                  Previous
                </button>
                <span>
                  Page {page() + 1} of {pageCount()} ·{" "}
                  {total() ? page() * size() + 1 : 0}–
                  {Math.min(page() * size() + rows().length, total())} of{" "}
                  {total().toLocaleString()}
                </span>
                <button
                  disabled={
                    busy() ||
                    page() + 1 >= pageCount() ||
                    rows().length < size()
                  }
                  onClick={() => void go(page() + 1)}
                >
                  Next
                </button>
              </div>
            </section>
          </>
        )}
      </Show>
      <Show when={busy()}>
        <p role="status">Loading records…</p>
      </Show>
      <ErrorText message={error()} />
      <Show
        when={error() && !busy() && (!snapshot() || retryPage() !== undefined)}
      >
        <button
          onClick={() =>
            snapshot()
              ? void go(retryPage()!)
              : void enter(params.org, params.id)
          }
        >
          Retry
        </button>
      </Show>
    </>
  );
}
