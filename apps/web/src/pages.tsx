import { createSignal, createEffect, createMemo, onCleanup, Show, For } from "solid-js";
import { A, useParams, useNavigate, useLocation } from "@solidjs/router";
import { marked } from "marked";
import {
  api,
  wsUrl,
  TABLE_ID,
  type Table,
  type Overview,
  type SpaceWindow,
  type Version,
  type ApiKey,
  type Webhook,
  type Notification,
  type NotificationEvent,
  type TestResult,
  type Me,
} from "../lib/api";
import { agentPrompt } from "../lib/agent-prompt";
import { parseDefinition, testSummary } from "../lib/notification-def";
import {
  loadRuntime,
  type Host,
  type Status,
  type RunError,
  type Notification as LiveNotification,
} from "../lib/runtime";
import {
  Access,
  Copy,
  Secret,
  Modal,
  Loading,
  ErrorText,
  action,
  resource,
  list,
  when,
} from "./ui";
const base = () =>
  `/orgs/${encodeURIComponent(useParams<{ org: string; id: string; slug: string }>().org)}`;
export function Tables() {
  const params = useParams();
  const root = base();
  const tables = resource(() => api<Table[]>(root + "/tables"), 5000);
  const [period, setPeriod] = createSignal("5h");
  const overview = resource(
    () => api<Overview>(root + "/tables/overview?window=" + period()),
    5000,
    period,
  );
  const [creating, setCreating] = createSignal(false),
    [id, setId] = createSignal(""),
    [key, setKey] = createSignal<{ table: string; key: string }>();
  const a = action();
  const got = (t: string, k: string) => {
    setKey({ table: t, key: k });
    setCreating(false);
    tables.reload();
  };
  return (
    <>
      <div class="row">
        <h1>Tables</h1>
        <button class="primary right" onClick={() => setCreating(true)}>
          New table
        </button>
      </div>
      <Show when={key()}>
        {(k) => (
          <section role="status">
            <h2>Key for {k().table}</h2>
            <Secret value={k().key} once />
            <p>
              <A href="/docs/getting-started">Send your first records →</A>
            </p>
          </section>
        )}
      </Show>
      <section>
        <div class="row">
          <h2>Activity</h2>
          <select
            aria-label="Activity time range"
            value={period()}
            onChange={(e) => setPeriod(e.currentTarget.value)}
          >
            <For each={["1m", "5m", "15m", "1h", "5h", "1d", "7d", "30d"]}>
              {(v) => <option>{v}</option>}
            </For>
          </select>
        </div>
        <Show
          when={overview.data()}
          fallback={<Loading error={overview.data.error} />}
        >
          {(o) => (
            <div class="stats">
              <div>
                <b>{o().tables.toLocaleString()}</b>
                <span>tables</span>
              </div>
              <div>
                <b>{o().records.toLocaleString()}</b>
                <span>records</span>
              </div>
              <div>
                <b>
                  {o().avg_lag_ms == null
                    ? "—"
                    : Math.round(o().avg_lag_ms!) + " ms"}
                </b>
                <span>average ingestion lag</span>
              </div>
              <div>
                <span>Most active</span>
                <For each={o().top}>
                  {(t) => (
                    <p>
                      <A href={`/o/${params.org}/tables/${t.id}`}><code>{t.id}</code></A> · {t.records.toLocaleString()}
                    </p>
                  )}
                </For>
                <Show when={!o().top.length}>
                  <p>Nothing in this period</p>
                </Show>
              </div>
            </div>
          )}
        </Show>
      </section>
      <Show
        when={tables.data()}
        fallback={<Loading error={tables.data.error} />}
      >
        {(rows) => (
          <Show
            when={rows().length}
            fallback={
              <p class="empty">
                No tables yet. Create a table, then send records with its key.
              </p>
            }
          >
            <div class="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Table</th>
                    <th>Records</th>
                    <th>Access</th>
                    <th>Created by</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={rows()}>
                    {(t) => (
                      <tr>
                        <td>
                          <A href={`/o/${params.org}/tables/${t.id}`}><code>{t.id}</code></A>
                          <small>{when(t.created_at)}</small>
                        </td>
                        <td>{t.records.toLocaleString()}</td>
                        <td>
                          <Access
                            value={t.access}
                            save={async (access) => {
                              await api(root + "/tables/" + t.id, "PUT", {
                                access,
                              });
                              await tables.reload();
                            }}
                          />
                        </td>
                        <td>@{t.created_by}</td>
                        <td>
                          <button
                            disabled={a.busy()}
                            onClick={() => {
                              if (
                                confirm(
                                  `Rotate the key for ${t.id}? The current key will stop working.`,
                                )
                              )
                                a.run(async () =>
                                  got(
                                    t.id,
                                    (
                                      await api<{ key: string }>(
                                        root +
                                          "/tables/" +
                                          t.id +
                                          "/rotate-key",
                                        "POST",
                                      )
                                    ).key,
                                  ),
                                );
                            }}
                          >
                            Rotate key
                          </button>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </Show>
        )}
      </Show>
      <ErrorText message={a.error()} />
      <Show when={creating()}>
        <Modal title="New table" close={() => setCreating(false)}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              a.run(async () => {
                if (!TABLE_ID.test(id()))
                  throw Error("Use 1–50 lowercase letters and digits.");
                got(
                  id(),
                  (
                    await api<{ key: string }>(root + "/tables", "POST", {
                      id: id(),
                    })
                  ).key,
                );
                setId("");
              });
            }}
          >
            <label>
              Table ID
              <input
                required
                pattern="[a-z0-9]{1,50}"
                maxlength={50}
                placeholder="orders"
                value={id()}
                onInput={(e) => setId(e.currentTarget.value)}
                autofocus
              />
            </label>
            <p class="muted">
              Lowercase letters and digits, unique in this organization.
            </p>
            <ErrorText message={a.error()} />
            <button class="primary" disabled={a.busy()}>
              Create table
            </button>
          </form>
        </Modal>
      </Show>
    </>
  );
}
export function Windows() {
  const p = useParams<{ org: string; id: string; slug: string }>(),
    root = base(),
    nav = useNavigate();
  const rows = resource(() => api<SpaceWindow[]>(root + "/windows"));
  const [creating, setCreating] = createSignal(false),
    [name, setName] = createSignal(""),
    [access, setAccess] = createSignal("");
  const a = action();
  return (
    <>
      <div class="row">
        <h1>Space Windows</h1>
        <button class="primary right" onClick={() => setCreating(true)}>
          New window
        </button>
      </div>
      <p class="muted">Live views and tools built from your tables.</p>
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <Show
            when={items().length}
            fallback={
              <p class="empty">
                Create your first Space Window to turn records into a live view.
              </p>
            }
          >
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Access</th>
                  <th>Created by</th>
                </tr>
              </thead>
              <tbody>
                <For each={items()}>
                  {(w) => (
                    <tr>
                      <td>
                        <A href={`/o/${p.org}/windows/${w.id}`}>{w.name}</A>
                        <small>{when(w.created_at)}</small>
                      </td>
                      <td>
                        <Access
                          value={w.access}
                          save={async (access) => {
                            await api(root + "/windows/" + w.id, "PUT", {
                              access,
                            });
                            await rows.reload();
                          }}
                        />
                      </td>
                      <td>@{w.created_by}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        )}
      </Show>
      <Show when={creating()}>
        <Modal title="New Space Window" close={() => setCreating(false)}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              a.run(async () => {
                const w = await api<SpaceWindow>(root + "/windows", "POST", {
                  name: name().trim(),
                  access: list(access()),
                });
                nav(`/o/${p.org}/windows/${w.id}`);
              });
            }}
          >
            <label>
              Name
              <input
                required
                maxlength={19}
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
              />
            </label>
            <label>
              Access
              <input
                value={access()}
                onInput={(e) => setAccess(e.currentTarget.value)}
                placeholder="@alice, #engineering"
              />
            </label>
            <p class="muted">You are added automatically.</p>
            <ErrorText message={a.error()} />
            <button class="primary" disabled={a.busy()}>
              Create window
            </button>
          </form>
        </Modal>
      </Show>
    </>
  );
}
export function WindowView() {
  const p = useParams<{ org: string; id: string; slug: string }>(),
    root = base(),
    path = root + "/windows/" + p.id;
  const location = useLocation();
  const win = resource(() => api<SpaceWindow>(path)),
    versions = resource(() => api<Version[]>(path + "/versions")),
    tables = resource(() => api<Table[]>(root + "/tables")),
    windows = resource(() => api<SpaceWindow[]>(root + "/windows"));
  const codeMode = () => location.pathname.endsWith("/code");
  const [name, setName] = createSignal(""),
    [processor, setProcessor] = createSignal(""),
    [renderer, setRenderer] = createSignal("");
  const prompt = createMemo(() => {
    const w = win.data();
    return w
      ? agentPrompt({
          org: p.org,
          window: w,
          tables: (tables.data() || []).map((t) => t.id),
        })
      : "";
  });
  createEffect(() => {
    setProcessor(win.data()?.version?.processor || "");
    setRenderer(win.data()?.version?.renderer || "");
  });
  const a = action();
  return (
    <Show when={win.data()} fallback={<Loading error={win.data.error} />}>
      {(w) => (
        <>
          <div class="window-head">
            <div class="window-title-row">
              <A class="back-link" href={`/o/${p.org}/windows`}>← Windows</A>
              <span class="window-kicker">SPACE WINDOW</span>
            </div>
            <div class="row">
              <h1>{w().name}</h1>
              <Show when={w().version}><span class="live-pill">● Live</span></Show>
              <Access
                value={w().access}
                save={async (access) => {
                  await api(path, "PUT", { access });
                  await win.reload();
                }}
              />
              <A class="code-button" href={`/o/${p.org}/windows/${p.id}${codeMode() ? "" : "/code"}`}>
                {codeMode() ? "← Live view" : "View code ↗"}
              </A>
            </div>
          </div>
          <div class="window-tabs" role="tablist" aria-label="Space Windows">
            <For each={windows.data() || []}>
              {(item) => <A role="tab" aria-selected={item.id === p.id} class={item.id === p.id ? "active" : ""} href={`/o/${p.org}/windows/${item.id}`}>{item.name}</A>}
            </For>
            <A class="new-tab" href={`/o/${p.org}/windows`}>＋ New window</A>
          </div>
          <Show when={codeMode()} fallback={<>
            <Show when={w().version} fallback={<p class="empty">No live view yet. Use the prompt below to create a processor and renderer.</p>}>
              <Live org={p.org} window={w()} onVersionChanged={async () => { await Promise.all([win.reload(), versions.reload()]); }} />
            </Show>
            <details open={!w().version}>
              <summary>Prompt for an agent <span class="muted">· copy-ready</span></summary>
              <Copy text={prompt()} />
              <pre>{prompt()}</pre>
            </details>
          </>}>
          <div class="window-workbench">
            <Show when={w().version} fallback={<p class="empty">No code yet. Publish a version to open the workbench.</p>}>
              <Live org={p.org} window={w()} onVersionChanged={async () => { await Promise.all([win.reload(), versions.reload()]); }} />
            </Show>
          <section class="code-panel">
            <h2>Code</h2>
            <label>
              Load a version
              <select
                onChange={(e) => {
                  const v = versions
                    .data()
                    ?.find((v) => v.id === e.currentTarget.value);
                  if (v) {
                    setProcessor(v.processor);
                    setRenderer(v.renderer);
                  }
                }}
              >
                <option value="">Select a saved version</option>
                <For each={versions.data()}>
                  {(v) => (
                    <option value={v.id}>
                      {v.name}
                      {v.name === w().version?.name ? " (current)" : ""}
                    </option>
                  )}
                </For>
              </select>
            </label>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                a.run(async () => {
                  await api(path + "/versions", "POST", {
                    name: name(),
                    processor: processor(),
                    renderer: renderer(),
                  });
                  await Promise.all([win.reload(), versions.reload()]);
                  setName("");
                });
              }}
            >
              <label>
                New version name
                <input
                  required
                  value={name()}
                  onInput={(e) => setName(e.currentTarget.value)}
                  placeholder="v1"
                />
              </label>
              <CodeEditor label="processor.js" value={processor()} onInput={setProcessor} />
              <CodeEditor label="renderer.html" value={renderer()} onInput={setRenderer} />
              <ErrorText message={a.error()} />
              <button class="primary" disabled={a.busy()}>
                Publish version
              </button>
            </form>
          </section>
          </div>
          </Show>
        </>
      )}
    </Show>
  );
}
function CodeEditor(p: { label: string; value: string; onInput: (value: string) => void }) {
  const escaped = () => p.value.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]!));
  const highlighted = () => escaped().replace(/(\/\/[^\n]*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\b(?:const|let|var|return|export|default|function|async|await|if|else|new|true|false|null)\b)/g, '<span class="tok">$1</span>');
  return <label class="editor-label">{p.label}<div class="editor"><pre aria-hidden="true" innerHTML={highlighted() + "\n"} /><textarea required class="code" spellcheck={false} value={p.value} onInput={(e) => p.onInput(e.currentTarget.value)} /></div></label>;
}
export function AccessToken(p: { root: string }) {
  const token = resource(() =>
    api<{ token: string; last_used_at: string | null }>(
      p.root + "/access-token",
    ),
  );
  const a = action();
  return (
    <section>
      <h2>Your access token</h2>
      <Show when={token.data()} fallback={<Loading error={token.data.error} />}>
        {(t) => (
          <>
            <Secret value={t().token} />
            <p class="muted">Last used {when(t().last_used_at)}</p>
          </>
        )}
      </Show>
      <button
        disabled={a.busy()}
        onClick={() => {
          if (
            confirm(
              "Rotate your access token? Its current value will stop working everywhere.",
            )
          )
            a.run(async () => {
              await api(p.root + "/access-token/rotate", "POST");
              await token.reload();
            });
        }}
      >
        Rotate token
      </button>
      <ErrorText message={a.error()} />
    </section>
  );
}
function Live(p: { org: string; window: SpaceWindow; onVersionChanged: () => Promise<void> }) {
  let mount!: HTMLDivElement;
  const [status, setStatus] = createSignal<Status>(),
    [errors, setErrors] = createSignal<RunError[]>([]),
    [notifications, setNotifications] = createSignal<LiveNotification[]>([]),
    [attempt, setAttempt] = createSignal(0);
  createEffect(() => {
    const w = p.window;
    attempt();
    let gone = false,
      refreshing = false,
      host: Host | undefined;
    setErrors([]);
    setStatus(undefined);
    loadRuntime()
      .then((runtime) => {
        if (gone) return;
        host = runtime.host({
          runtimeUrl: location.origin + "/mission-control.js",
          api: { base: "/api" },
          ws: wsUrl(p.org),
          org: p.org,
          window: w,
          mount,
          onStatus: setStatus,
          onError: (e) => {
            if (e.source === "server" && e.message.startsWith("version_not_current:")) {
              if (!refreshing) {
                refreshing = true;
                p.onVersionChanged().catch((error) => {
                  refreshing = false;
                  setErrors([{ source: "host", message: String(error), detail: null }]);
                });
              }
              return;
            }
            setErrors((old) => [...old, e].slice(-10));
          },
          onNotification: (n) =>
            setNotifications((old) => [n, ...old].slice(0, 5)),
        });
      })
      .catch((e) =>
        setErrors([{ source: "host", message: e.message, detail: null }]),
      );
    onCleanup(() => {
      gone = true;
      host?.destroy();
    });
  });
  return (
    <section>
      <div class="row">
        <h2>Live view</h2>
        <span class="muted">
          {status()
            ? `${status()!.connected ? "Connected" : "Disconnected"} · ${status()!.is_live ? "Live" : "Stale"}`
            : "Starting…"}
        </span>
      </div>
      <For each={notifications()}>
        {(n) => (
          <p class="note">
            <strong>{n.name}</strong> {n.text}
            <button
              onClick={() =>
                setNotifications((old) => old.filter((x) => x !== n))
              }
            >
              Dismiss
            </button>
          </p>
        )}
      </For>
      <Show when={errors().length}>
        <div role="alert">
          <For each={errors()}>
            {(e) => (
              <details>
                <summary class="error">
                  {e.source}: {e.message}
                </summary>
                <pre>{JSON.stringify(e.detail, null, 2)}</pre>
              </details>
            )}
          </For>
          <button onClick={() => setAttempt(attempt() + 1)}>
            Restart window
          </button>
        </div>
      </Show>
      <div class="live" ref={mount} />
    </section>
  );
}
export function Notifications() {
  const root = base(),
    rows = resource(() => api<Notification[]>(root + "/notifications")),
    me = resource(() => api<Me>("/me")),
    tables = resource(() => api<Table[]>(root + "/tables"));
  const [editing, setEditing] = createSignal<{ id?: string; text: string }>();
  const template = () => {
    const table = tables.data()?.[0]?.id || "orders",
      actor = me.data()?.id;
    return JSON.stringify(
      {
        name: `New ${table} record`,
        description: "Notify when a record arrives.",
        enabled: true,
        triggers: [{ table }],
        sql: `SELECT toString(record_id) AS dedup_key, 'New record in ${table}' AS text, map('table', '${table}') AS metadata FROM ${table}`,
        delay: "2s",
        cooldown: "10m",
        access: [`@${actor}`],
        recipients: [`@${actor}`],
      },
      null,
      2,
    );
  };
  return (
    <>
      <div class="row">
        <h1>Notifications</h1>
        <button
          class="primary right"
          disabled={!me.data()}
          onClick={() => setEditing({ text: template() })}
        >
          New notification
        </button>
      </div>
      <p class="muted">
        Watch incoming records or schedule checks. Send results to subscribers
        and webhooks.
      </p>
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <>
            <Show when={!items().length}>
              <p class="empty">No notifications yet.</p>
            </Show>
            <For each={items()}>
              {(n) => (
                <NotificationRow
                  root={root}
                  n={n}
                  actor={me.data()?.id || ""}
                  reload={rows.reload}
                  edit={() =>
                    setEditing({
                      id: n.id,
                      text: JSON.stringify(
                        {
                          ...n.def,
                          enabled: n.enabled,
                          recipients: n.recipients,
                        },
                        null,
                        2,
                      ),
                    })
                  }
                />
              )}
            </For>
          </>
        )}
      </Show>
      <Show when={editing()} keyed>
        {(edit) => (
          <NotificationEditor
            root={root}
            initial={edit.text}
            id={edit.id}
            close={() => setEditing(undefined)}
            saved={() => {
              setEditing(undefined);
              rows.reload();
            }}
          />
        )}
      </Show>
    </>
  );
}
function NotificationRow(p: {
  root: string;
  n: Notification;
  actor: string;
  reload: () => unknown;
  edit: () => void;
}) {
  const a = action(),
    [events, setEvents] = createSignal<NotificationEvent[]>(),
    [result, setResult] = createSignal("");
  const path = () => p.root + "/notifications/" + p.n.id;
  const subscribed = () => p.n.recipients.includes("@" + p.actor);
  return (
    <section>
      <div class="row">
        <h2>{p.n.def.name}</h2>
        <span class="badge">{p.n.enabled ? "Enabled" : "Paused"}</span>
      </div>
      <p class="muted">{p.n.def.description}</p>
      <p>
        <For each={p.n.def.triggers}>
          {(t) => (
            <code class="tag">{"schedule" in t ? t.schedule : t.table}</code>
          )}
        </For>
      </p>
      <div class="row">
        <button
          disabled={a.busy()}
          onClick={() =>
            events()
              ? setEvents(undefined)
              : a.run(async () =>
                  setEvents(await api<NotificationEvent[]>(path() + "/events")),
                )
          }
        >
          {events() ? "Hide events" : "Events"}
        </button>
        <button
          disabled={a.busy()}
          onClick={() =>
            a.run(async () => {
              await api(
                path() + "/subscribe",
                subscribed() ? "DELETE" : "POST",
              );
              await p.reload();
            })
          }
        >
          {subscribed() ? "Unsubscribe" : "Subscribe"}
        </button>
        <button
          disabled={a.busy()}
          onClick={() =>
            a.run(async () =>
              setResult(
                testSummary(await api<TestResult>(path() + "/test", "POST")),
              ),
            )
          }
        >
          Test
        </button>
        <button disabled={a.busy()} onClick={p.edit}>
          Edit
        </button>
        <button
          disabled={a.busy()}
          onClick={() =>
            a.run(async () => {
              await api(path(), "PUT", {
                def: { ...p.n.def, enabled: !p.n.enabled },
                recipients: p.n.recipients,
              });
              await p.reload();
            })
          }
        >
          {p.n.enabled ? "Pause" : "Enable"}
        </button>
      </div>
      <ErrorText message={a.error()} />
      <Show when={result()}>
        <pre>{result()}</pre>
      </Show>
      <Show when={events()}>
        {(items) => (
          <Show
            when={items().length}
            fallback={<p class="muted">No events yet.</p>}
          >
            <For each={items()}>
              {(e) => (
                <p>
                  <small>{when(e.created_at)}</small>
                  {e.text} <code>{e.dedup_key}</code>
                </p>
              )}
            </For>
          </Show>
        )}
      </Show>
    </section>
  );
}
function NotificationEditor(p: {
  root: string;
  initial: string;
  id?: string;
  close: () => void;
  saved: () => void;
}) {
  const [text, setText] = createSignal(p.initial),
    a = action();
  return (
    <Modal
      title={p.id ? "Edit notification" : "New notification"}
      close={p.close}
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            const parsed = parseDefinition(text());
            if (!parsed.body) throw Error(parsed.errors.join("\n"));
            await api(
              p.root + "/notifications" + (p.id ? "/" + p.id : ""),
              p.id ? "PUT" : "POST",
              parsed.body,
            );
            p.saved();
          });
        }}
      >
        <p class="muted">
          SQL must return dedup_key, text, and metadata. Recipients are @actors
          or webhook:&lt;id&gt;.{" "}
          <A href="/docs/notifications" target="_blank">
            Documentation
          </A>
        </p>
        <label>
          Definition (JSON)
          <textarea
            class="code"
            rows={20}
            value={text()}
            onInput={(e) => setText(e.currentTarget.value)}
          />
        </label>
        <ErrorText message={a.error()} />
        <button class="primary" disabled={a.busy()}>
          Save notification
        </button>
      </form>
    </Modal>
  );
}
export function Settings() {
  const root = base();
  return (
    <>
      <h1>Settings</h1>
      <Webhooks root={root} />
      <Keys root={root} />
    </>
  );
}
function Webhooks(p: { root: string }) {
  const rows = resource(() => api<Webhook[]>(p.root + "/webhooks")),
    a = action();
  const [url, setUrl] = createSignal(""),
    [made, setMade] = createSignal<{ id: string; secret: string }>();
  return (
    <section>
      <h2>Webhooks</h2>
      <p class="muted">
        Signed notification deliveries to your HTTPS endpoint.
      </p>
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <>
            <Show when={!items().length}>
              <p>No webhooks yet.</p>
            </Show>
            <For each={items()}>
              {(h) => (
                <div class="list-row">
                  <div>
                    <code>{h.id}</code>
                    <p>{h.url}</p>
                  </div>
                  <button
                    disabled={a.busy()}
                    onClick={() => {
                      if (
                        confirm(
                          "Delete this webhook? Deliveries to it will stop.",
                        )
                      )
                        a.run(async () => {
                          await api(p.root + "/webhooks/" + h.id, "DELETE");
                          await rows.reload();
                        });
                    }}
                  >
                    Delete
                  </button>
                </div>
              )}
            </For>
          </>
        )}
      </Show>
      <Show when={made()}>
        {(m) => (
          <p>
            Secret for <code>{m().id}</code>: <Secret value={m().secret} once />
          </p>
        )}
      </Show>
      <form
        class="row"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(
              await api(p.root + "/webhooks", "POST", { url: url().trim() }),
            );
            setUrl("");
            await rows.reload();
          });
        }}
      >
        <label class="grow">
          Endpoint URL
          <input
            type="url"
            required
            placeholder="https://example.com/notifications"
            value={url()}
            onInput={(e) => setUrl(e.currentTarget.value)}
          />
        </label>
        <button disabled={a.busy()}>Add webhook</button>
      </form>
      <ErrorText message={a.error()} />
    </section>
  );
}
function Keys(p: { root: string }) {
  const rows = resource(() => api<ApiKey[]>(p.root + "/api-keys")),
    a = action();
  const [scopes, setScopes] = createSignal(["tables"]),
    [made, setMade] = createSignal<{ id: string; key: string }>();
  return (
    <section>
      <h2>API keys</h2>
      <p class="muted">
        Allow an integration to read tables and run queries, or read
        notifications.
      </p>
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <>
            <Show when={!items().length}>
              <p>No API keys yet.</p>
            </Show>
            <For each={items()}>
              {(k) => (
                <div class="list-row">
                  <div>
                    <code>{k.id}</code>
                    <p>
                      {k.scopes.join(", ")} · @{k.created_by}
                    </p>
                    <small>Last used {when(k.last_used_at)}</small>
                  </div>
                  <button
                    disabled={a.busy()}
                    onClick={() => {
                      if (
                        confirm(
                          "Delete this API key? Requests with it will fail.",
                        )
                      )
                        a.run(async () => {
                          await api(p.root + "/api-keys/" + k.id, "DELETE");
                          await rows.reload();
                        });
                    }}
                  >
                    Delete
                  </button>
                </div>
              )}
            </For>
          </>
        )}
      </Show>
      <Show when={made()}>
        {(m) => (
          <p>
            Key <code>{m().id}</code>: <Secret value={m().key} once />
          </p>
        )}
      </Show>
      <form
        class="row"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(
              await api(p.root + "/api-keys", "POST", { scopes: scopes() }),
            );
            await rows.reload();
          });
        }}
      >
        <For each={["tables", "notifications"]}>
          {(s) => (
            <label class="check">
              <input
                type="checkbox"
                checked={scopes().includes(s)}
                onChange={() =>
                  setScopes((old) =>
                    old.includes(s) ? old.filter((x) => x !== s) : [...old, s],
                  )
                }
              />
              {s}
            </label>
          )}
        </For>
        <button disabled={a.busy() || !scopes().length}>Create key</button>
      </form>
      <ErrorText message={a.error()} />
    </section>
  );
}
const documents = import.meta.glob("../docs/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const documentBySlug = Object.fromEntries(
  Object.entries(documents).map(([path, value]) => [
    path.split("/").pop()!.replace(/\.md$/, ""),
    value,
  ]),
) as Record<string, string>;
const slugs = [
  "getting-started",
  "space-windows",
  "sql",
  "notifications",
  "rust",
  "cli",
  "credentials",
  "api",
];
export function Docs() {
  const p = useParams<{ org: string; id: string; slug: string }>(),
    slug = () => p.slug || "getting-started",
    md = () => documentBySlug[slug()] || documentBySlug["getting-started"];
  return (
    <div class="workspace docs">
      <aside>
        <h2>Documentation</h2>
        <nav>
          <For each={slugs}>
            {(s) => (
              <A activeClass="selected" href={"/docs/" + s}>
                {/^#\s+(.+)$/m.exec(documentBySlug[s] || "")?.[1] || s}
              </A>
            )}
          </For>
        </nav>
      </aside>
      <main class="prose">
        <Show when={md()} fallback={<h1>Document not found</h1>}>
          <article
            innerHTML={marked.parse(md() || "", { async: false }) as string}
          />
        </Show>
      </main>
    </div>
  );
}

export function Inspirations() {
  const cards = [
    ["See the whole orbit", "Heart Aerospace", "SPACE / CLARITY", "A single calm frame can carry a complex system. We borrow the confidence: one strong surface, one next move."],
    ["Explain by unfolding", "Postevand", "FLOW / CONTEXT", "Product stories feel clearer when each chapter earns its place. Space Station turns ingest, shape, and notify into a visible route."],
    ["Make progress legible", "X Business", "SEQUENCE / MOMENTUM", "A numbered path reduces hesitation. Every table, window, and notification should feel like the next instrument on a flight deck."],
    ["Let the grid breathe", "Cosmos", "COLLECT / CONTRAST", "Small marks become a field. Dither, orbit lines, and quiet hover states give the interface a sense of depth without adding noise."],
    ["Use the edge as a cue", "Arc", "SURFACE / FOCUS", "The sidebar belongs to the room; the paper belongs to the work. A little separation makes the central canvas feel held."],
    ["Tell the truth plainly", "Making Software", "WORDS / WEIGHT", "The best line is often the one that says exactly what happened: records arrived, a view changed, a message went out."],
  ] as const;
  return (
    <main class="inspirations-page">
      <div class="window-title-row">
        <span class="window-kicker">FIELD NOTES / 06</span>
      </div>
      <h1>Inspirations for the station</h1>
      <p class="muted prose">A small atlas of the ideas shaping Space Station: systems that feel capable, calm, and a little unexpected.</p>
      <div class="inspiration-grid">
        <For each={cards}>{(card) => (
          <section class="inspiration-card">
            <span class="signal">{card[2]}</span>
            <div>
              <strong>{card[0]}</strong>
              <p class="muted">{card[3]}</p>
            </div>
            <small>{card[1]}</small>
          </section>
        )}</For>
      </div>
    </main>
  );
}
