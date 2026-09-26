// Notifications: each definition with its triggers, pause/enable, subscribe, a test run and its
// events; and the JSON editor that checks a definition before the backend does.
import { createSignal, For, Show } from "solid-js";
import { api, type Notification, type NotificationEvent, type TestResult } from "../lib/api";
import { parseDefinition, testSummary } from "../lib/notification-def";
import { action, ago, Empty, ErrorText, Loading, Modal, resource, when } from "./ui";
import { useWorkspace } from "./tabs";
import { Icon } from "./icons";

export function Notifications() {
  const ws = useWorkspace();
  const root = `/orgs/${encodeURIComponent(ws.org)}`,
    rows = resource(() => api<Notification[]>(root + "/notifications"), 15000);
  const [editing, setEditing] = createSignal<{ id?: string; text: string }>();
  const template = () => {
    const table = ws.tables.data()?.[0]?.id || "orders",
      actor = ws.me.id;
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
    <div class="page-body">
      <header class="page-head">
        <div>
          <h1>Notifications</h1>
          <p class="muted">Watch incoming records or run on a schedule. Results go to subscribers and webhooks.</p>
        </div>
        <button class="primary" onClick={() => setEditing({ text: template() })}>
          <Icon name="plus" /> New notification
        </button>
      </header>
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <Show
            when={items().length}
            fallback={
              <div class="hero-empty">
                <Empty icon="bell" title="No notifications yet">
                  <p>A notification runs SQL when a table receives a record, or on a cron schedule, and delivers each new row once per cooldown.</p>
                  <button class="primary large" onClick={() => setEditing({ text: template() })}>
                    <Icon name="plus" /> Create a notification
                  </button>
                </Empty>
              </div>
            }
          >
            <div class="stack">
              <For each={items()}>
                {(n) => (
                  <NotificationCard
                    root={root}
                    n={n}
                    actor={ws.me.id}
                    reload={rows.reload}
                    edit={() => setEditing({ id: n.id, text: JSON.stringify({ ...n.def, enabled: n.enabled, recipients: n.recipients }, null, 2) })}
                  />
                )}
              </For>
            </div>
          </Show>
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
    </div>
  );
}

function NotificationCard(p: { root: string; n: Notification; actor: string; reload: () => unknown; edit: () => void }) {
  const a = action(),
    [events, setEvents] = createSignal<NotificationEvent[]>(),
    [result, setResult] = createSignal("");
  const path = () => p.root + "/notifications/" + p.n.id;
  const subscribed = () => p.n.recipients.includes("@" + p.actor);
  return (
    <section class="panel notification" classList={{ paused: !p.n.enabled }}>
      <div class="n-head">
        <span class="title-icon"><Icon name="bell" /></span>
        <div class="grow">
          <h2>{p.n.def.name}</h2>
          <Show when={p.n.def.description}>
            <p class="muted">{p.n.def.description}</p>
          </Show>
        </div>
        <button
          class="switch"
          role="switch"
          aria-checked={p.n.enabled}
          title={p.n.enabled ? "Pause" : "Enable"}
          disabled={a.busy()}
          onClick={() =>
            a.run(async () => {
              await api(path(), "PUT", { def: { ...p.n.def, enabled: !p.n.enabled }, recipients: p.n.recipients });
              await p.reload();
            })
          }
        >
          <i />
          {p.n.enabled ? "On" : "Paused"}
        </button>
      </div>
      <div class="n-facts">
        <For each={p.n.def.triggers}>
          {(t) => (
            <span class="chip mono">
              <Icon name={"schedule" in t ? "clock" : "table"} />
              {"schedule" in t ? t.schedule : t.table + (t.where ? ` where ${t.where}` : "")}
            </span>
          )}
        </For>
        <span class="muted">delay {p.n.def.delay || "2s"} · cooldown {p.n.def.cooldown || "10m"}</span>
        <span class="muted">{p.n.recipients.length} recipient{p.n.recipients.length === 1 ? "" : "s"}</span>
        <span class="muted" title={when(p.n.created_at)}>@{p.n.created_by} · {ago(p.n.created_at)}</span>
      </div>
      <div class="n-actions">
        <button
          class="small"
          classList={{ on: subscribed() }}
          disabled={a.busy()}
          onClick={() =>
            a.run(async () => {
              await api(path() + "/subscribe", subscribed() ? "DELETE" : "POST");
              await p.reload();
            })
          }
        >
          <Icon name={subscribed() ? "check" : "bell"} /> {subscribed() ? "Subscribed" : "Subscribe"}
        </button>
        <button class="small" disabled={a.busy()} onClick={() => a.run(async () => setResult(testSummary(await api<TestResult>(path() + "/test", "POST"))))}>
          <Icon name="play" /> Test
        </button>
        <button
          class="small"
          classList={{ on: !!events() }}
          disabled={a.busy()}
          onClick={() => (events() ? setEvents(undefined) : a.run(async () => setEvents(await api<NotificationEvent[]>(path() + "/events"))))}
        >
          <Icon name="list" /> Events
        </button>
        <button class="small" disabled={a.busy()} onClick={p.edit}>
          <Icon name="pencil" /> Edit
        </button>
      </div>
      <ErrorText message={a.error()} />
      <Show when={result()}>
        <div class="n-result">
          <div class="row">
            <span class="kicker">Test run</span>
            <span class="grow" />
            <button class="icon" aria-label="Close test result" onClick={() => setResult("")}><Icon name="x" /></button>
          </div>
          <pre>{result()}</pre>
        </div>
      </Show>
      <Show when={events()}>
        {(items) => (
          <Show when={items().length} fallback={<p class="empty-line">No events yet.</p>}>
            <ol class="timeline">
              <For each={items()}>
                {(e) => (
                  <li>
                    <time title={when(e.created_at)}>{ago(e.created_at)}</time>
                    <span>{e.text}</span>
                    <code>{e.dedup_key}</code>
                  </li>
                )}
              </For>
            </ol>
          </Show>
        )}
      </Show>
    </section>
  );
}

function NotificationEditor(p: { root: string; initial: string; id?: string; close: () => void; saved: () => void }) {
  const [text, setText] = createSignal(p.initial),
    a = action();
  const check = () => parseDefinition(text()).errors;
  return (
    <Modal title={p.id ? "Edit notification" : "New notification"} close={p.close} wide>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            const parsed = parseDefinition(text());
            if (!parsed.body) throw Error(parsed.errors.join("\n"));
            await api(p.root + "/notifications" + (p.id ? "/" + p.id : ""), p.id ? "PUT" : "POST", parsed.body);
            p.saved();
          });
        }}
      >
        <p class="hint">
          SQL must return <code>dedup_key</code>, <code>text</code> and <code>metadata</code>. Recipients are @c:handle, @si:handle or webhook:&lt;id&gt;.{" "}
          <a href="/docs/notifications" target="_blank">
            Documentation <Icon name="external" />
          </a>
        </p>
        <textarea class="code" aria-label="Definition (JSON)" rows={20} spellcheck={false} value={text()} onInput={(e) => setText(e.currentTarget.value)} />
        <Show when={check().length} fallback={<p class="valid"><Icon name="check" /> Valid definition</p>}>
          <ul class="problems">
            <For each={check()}>{(x) => <li>{x}</li>}</For>
          </ul>
        </Show>
        <ErrorText message={a.error()} />
        <div class="dialog-foot">
          <button type="button" onClick={p.close}>Cancel</button>
          <button class="primary" disabled={a.busy()}>Save notification</button>
        </div>
      </form>
    </Modal>
  );
}
