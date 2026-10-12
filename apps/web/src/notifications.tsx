"use client";
// Notifications: each definition with its triggers, pause/enable, subscribe, a test run and its
// events; and the JSON editor that checks a definition before the backend does.
import { useState } from "react";
import { Badge } from "@/components/silicon-ui/badge/badge";
import { Button } from "@/components/silicon-ui/button/button";
import { Switch } from "@/components/silicon-ui/switch/switch";
import { Textarea } from "@/components/silicon-ui/textarea/textarea";
import { api, type Notification, type NotificationEvent, type TestResult } from "../lib/api";
import { parseDefinition, testSummary } from "../lib/notification-def";
import { ago, Empty, ErrorText, Loading, Modal, useAction, useResource, when } from "./ui";
import { useLists, useWorkspace } from "./tabs";
import { Icon } from "./icons";

export function Notifications() {
  const ws = useWorkspace();
  const { tables } = useLists();
  const rows = useResource(() => api<Notification[]>("/notifications"), 15000);
  const [editing, setEditing] = useState<{ id?: string; text: string }>();
  const editable = (n: Notification) => {
    const { access: _legacyAccess, ...definition } = n.def as Notification["def"] & { access?: string[] };
    return JSON.stringify({ ...definition, enabled: n.enabled, recipients: n.recipients }, null, 2);
  };
  const template = () => {
    const table = tables.data?.[0]?.id || "orders",
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
        recipients: [`@${actor}`],
      },
      null,
      2,
    );
  };
  return (
    <div className="page-body">
      <header className="page-head">
        <div>
          <h1>Notifications</h1>
          <p className="muted">Watch incoming records or run on a schedule. Results go to subscribers and webhooks.</p>
        </div>
        <Button onClick={() => setEditing({ text: template() })}>
          <Icon name="plus" /> New notification
        </Button>
      </header>
      {!rows.data ? (
        <Loading error={rows.error} />
      ) : !rows.data.length ? (
        <div className="hero-empty">
          <Empty
            icon="bell"
            title="No notifications yet"
            description="A notification runs SQL when a table receives a record, or on a cron schedule, and delivers each new row once per cooldown."
            action={
              <Button size="lg" onClick={() => setEditing({ text: template() })}>
                <Icon name="plus" /> Create a notification
              </Button>
            }
          />
        </div>
      ) : (
        <div className="stack">
          {rows.data.map((n) => (
            <NotificationCard key={n.id} n={n} actor={ws.me.id} reload={rows.reload} edit={() => setEditing({ id: n.id, text: editable(n) })} />
          ))}
        </div>
      )}
      {editing && (
        <NotificationEditor
          initial={editing.text}
          id={editing.id}
          close={() => setEditing(undefined)}
          saved={() => {
            setEditing(undefined);
            rows.reload();
          }}
        />
      )}
    </div>
  );
}

function NotificationCard(p: { n: Notification; actor: string; reload: () => Promise<void>; edit: () => void }) {
  const a = useAction();
  const [events, setEvents] = useState<NotificationEvent[]>();
  const [result, setResult] = useState("");
  const path = "/notifications/" + p.n.id;
  const subscribed = p.n.recipients.includes("@" + p.actor);
  return (
    <section className={`panel notification${p.n.enabled ? "" : " paused"}`}>
      <div className="n-head">
        <span className="title-icon">
          <Icon name="bell" />
        </span>
        <div className="grow">
          <h2>{p.n.def.name}</h2>
          {p.n.def.description && <p className="muted">{p.n.def.description}</p>}
        </div>
        <Switch
          checked={p.n.enabled}
          label={p.n.enabled ? "On" : "Paused"}
          title={p.n.enabled ? "Pause" : "Enable"}
          disabled={a.busy}
          onCheckedChange={() =>
            a.run(async () => {
              await api(path, "PUT", { def: { ...p.n.def, enabled: !p.n.enabled }, recipients: p.n.recipients });
              await p.reload();
            })
          }
        />
      </div>
      <div className="n-facts">
        {p.n.def.triggers.map((t, i) => (
          <Badge key={i} size="sm" className="mono" icon={<Icon name={"schedule" in t ? "clock" : "table"} />}>
            {"schedule" in t ? t.schedule : t.table + (t.where ? ` where ${t.where}` : "")}
          </Badge>
        ))}
        <span className="muted">
          delay {p.n.def.delay || "2s"} · cooldown {p.n.def.cooldown || "10m"}
        </span>
        <span className="muted">
          {p.n.recipients.length} recipient{p.n.recipients.length === 1 ? "" : "s"}
        </span>
        <span className="muted" title={when(p.n.created_at)}>
          @{p.n.created_by} · {ago(p.n.created_at)}
        </span>
      </div>
      <div className="n-actions">
        <Button
          variant="secondary"
          size="sm"
          className={subscribed ? "on" : undefined}
          disabled={a.busy}
          onClick={() =>
            a.run(async () => {
              await api(path + "/subscribe", subscribed ? "DELETE" : "POST");
              await p.reload();
            })
          }
        >
          <Icon name={subscribed ? "check" : "bell"} /> {subscribed ? "Subscribed" : "Subscribe"}
        </Button>
        <Button variant="secondary" size="sm" disabled={a.busy} onClick={() => a.run(async () => setResult(testSummary(await api<TestResult>(path + "/test", "POST"))))}>
          <Icon name="play" /> Test
        </Button>
        <Button
          variant="secondary"
          size="sm"
          className={events ? "on" : undefined}
          disabled={a.busy}
          onClick={() => (events ? setEvents(undefined) : a.run(async () => setEvents(await api<NotificationEvent[]>(path + "/events"))))}
        >
          <Icon name="list" /> Events
        </Button>
        <Button variant="secondary" size="sm" disabled={a.busy} onClick={p.edit}>
          <Icon name="pencil" /> Edit
        </Button>
      </div>
      <ErrorText message={a.error} />
      {result && (
        <div className="n-result">
          <div className="row">
            <span className="kicker">Test run</span>
            <span className="grow" />
            <Button variant="ghost" size="sm" className="icon" aria-label="Close test result" onClick={() => setResult("")}>
              <Icon name="x" />
            </Button>
          </div>
          <pre>{result}</pre>
        </div>
      )}
      {events &&
        (events.length ? (
          <ol className="timeline">
            {events.map((e) => (
              <li key={e.id}>
                <time title={when(e.created_at)}>{ago(e.created_at)}</time>
                <span>{e.text}</span>
                <code>{e.dedup_key}</code>
              </li>
            ))}
          </ol>
        ) : (
          <p className="empty-line">No events yet.</p>
        ))}
    </section>
  );
}

function NotificationEditor(p: { initial: string; id?: string; close: () => void; saved: () => void }) {
  const [text, setText] = useState(p.initial);
  const a = useAction();
  const problems = parseDefinition(text).errors;
  return (
    <Modal title={p.id ? "Edit notification" : "New notification"} close={p.close} wide>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            const parsed = parseDefinition(text);
            if (!parsed.body) throw Error(parsed.errors.join("\n"));
            await api("/notifications" + (p.id ? "/" + p.id : ""), p.id ? "PUT" : "POST", parsed.body);
            p.saved();
          });
        }}
      >
        <p className="hint">
          SQL must return <code>dedup_key</code>, <code>text</code> and <code>metadata</code>. Recipients are @c:handle, @si:handle or webhook:&lt;id&gt;.{" "}
          <a href="/docs/notifications" target="_blank">
            Documentation <Icon name="external" />
          </a>
        </p>
        <Textarea label="Definition (JSON)" className="code" rows={20} spellCheck={false} value={text} onChange={(e) => setText(e.currentTarget.value)} />
        {problems.length ? (
          <ul className="problems">
            {problems.map((x, i) => (
              <li key={i}>{x}</li>
            ))}
          </ul>
        ) : (
          <p className="valid">
            <Icon name="check" /> Valid definition
          </p>
        )}
        <ErrorText message={a.error} />
        <div className="dialog-foot">
          <Button type="button" variant="secondary" onClick={p.close}>
            Cancel
          </Button>
          <Button type="submit" loading={a.busy}>
            Save notification
          </Button>
        </div>
      </form>
    </Modal>
  );
}
