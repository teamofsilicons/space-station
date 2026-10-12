"use client";
// Settings: your access token, webhooks, API keys, and this browser's appearance.
import { useState, type ReactNode } from "react";
import { Badge } from "@/components/silicon-ui/badge/badge";
import { Button } from "@/components/silicon-ui/button/button";
import { Checkbox } from "@/components/silicon-ui/checkbox/checkbox";
import { Input } from "@/components/silicon-ui/input/input";
import { api, type ApiKey, type Webhook } from "../lib/api";
import { ago, ErrorText, Loading, Secret, Segmented, useAction, useResource } from "./ui";
import { useWorkspace } from "./tabs";
import { AccessToken } from "./windows";
import { Icon } from "./icons";
import { setTheme, useTheme, type Theme } from "./theme";

function Row(p: { title: string; hint: string; children: ReactNode }) {
  return (
    <section className="setting">
      <div className="setting-label">
        <h2>{p.title}</h2>
        <p className="muted">{p.hint}</p>
      </div>
      <div className="setting-body">{p.children}</div>
    </section>
  );
}

export function Settings() {
  const ws = useWorkspace();
  const theme = useTheme();
  return (
    <div className="page-body">
      <header className="page-head">
        <div>
          <h1>Settings</h1>
          <p className="muted">Signed in as {ws.me.id}.</p>
        </div>
      </header>
      <Row title="Access token" hint="Mission control runs your queries with it while you develop a window or a notification. Tied to your account; keep it in .env, never in code.">
        <AccessToken hint={false} />
      </Row>
      <Webhooks />
      <Keys />
      <Row title="Appearance" hint="Follows your system unless you choose. Kept in this browser.">
        <Segmented<Theme> label="Theme" value={theme} options={["system", "light", "dark"]} names={{ system: "System", light: "Light", dark: "Dark" }} onChange={setTheme} />
      </Row>
    </div>
  );
}

function Webhooks() {
  const rows = useResource(() => api<Webhook[]>("/webhooks"));
  const a = useAction();
  const [url, setUrl] = useState("");
  const [made, setMade] = useState<{ id: string; secret: string }>();
  return (
    <Row title="Webhooks" hint="Signed notification deliveries to your HTTPS endpoint. Name one as webhook:<id> in a notification's recipients.">
      {!rows.data ? (
        <Loading error={rows.error} />
      ) : (
        <div className="items">
          {!rows.data.length && <p className="empty-line">No webhooks yet.</p>}
          {rows.data.map((h) => (
            <div key={h.id} className="item">
              <Icon name="link" />
              <div className="grow">
                <code>{h.id}</code>
                <small className="muted">{h.url}</small>
              </div>
              <small className="muted">
                @{h.created_by} · {ago(h.created_at)}
              </small>
              <Button
                variant="ghost"
                size="sm"
                className="icon danger"
                aria-label={`Delete webhook ${h.id}`}
                disabled={a.busy}
                onClick={() => {
                  if (confirm("Delete this webhook? Deliveries to it will stop."))
                    a.run(async () => {
                      await api("/webhooks/" + h.id, "DELETE");
                      await rows.reload();
                    });
                }}
              >
                <Icon name="trash" />
              </Button>
            </div>
          ))}
        </div>
      )}
      {made && (
        <div className="callout">
          <span>
            Signing secret for <code>{made.id}</code>
          </span>
          <Secret value={made.secret} once />
        </div>
      )}
      <form
        className="inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(await api<{ id: string; secret: string }>("/webhooks", "POST", { url: url.trim() }));
            setUrl("");
            await rows.reload();
          });
        }}
      >
        <Input label="Endpoint URL" type="url" required placeholder="https://example.com/notifications" value={url} onChange={(e) => setUrl(e.currentTarget.value)} />
        <Button type="submit" variant="secondary" disabled={a.busy}>
          <Icon name="plus" /> Add webhook
        </Button>
      </form>
      <ErrorText message={a.error} />
    </Row>
  );
}

function Keys() {
  const rows = useResource(() => api<ApiKey[]>("/api-keys"));
  const a = useAction();
  const [scopes, setScopes] = useState(["tables"]);
  const [made, setMade] = useState<{ id: string; key: string }>();
  return (
    <Row title="API keys" hint="Read tables and run queries, or read notifications, as your signed-in carbon or silicon.">
      {!rows.data ? (
        <Loading error={rows.error} />
      ) : (
        <div className="items">
          {!rows.data.length && <p className="empty-line">No API keys yet.</p>}
          {rows.data.map((k) => (
            <div key={k.id} className="item">
              <Icon name="key" />
              <div className="grow">
                <code>{k.id}</code>
                <small className="muted">
                  Last used {k.last_used_at ? ago(k.last_used_at) : "never"} · @{k.created_by}
                </small>
              </div>
              <span className="chips">
                {k.scopes.map((s) => (
                  <Badge key={s} tone="info" size="sm">
                    {s}
                  </Badge>
                ))}
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="icon danger"
                aria-label={`Delete API key ${k.id}`}
                disabled={a.busy}
                onClick={() => {
                  if (confirm("Delete this API key? Requests with it will fail."))
                    a.run(async () => {
                      await api("/api-keys/" + k.id, "DELETE");
                      await rows.reload();
                    });
                }}
              >
                <Icon name="trash" />
              </Button>
            </div>
          ))}
        </div>
      )}
      {made && (
        <div className="callout">
          <span>
            Key <code>{made.id}</code>
          </span>
          <Secret value={made.key} once />
        </div>
      )}
      <form
        className="inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(await api<{ id: string; key: string }>("/api-keys", "POST", { scopes }));
            await rows.reload();
          });
        }}
      >
        {["tables", "notifications"].map((s) => (
          <Checkbox key={s} label={s} checked={scopes.includes(s)} onCheckedChange={() => setScopes((old) => (old.includes(s) ? old.filter((x) => x !== s) : [...old, s]))} />
        ))}
        <Button type="submit" variant="secondary" disabled={a.busy || !scopes.length}>
          <Icon name="plus" /> Create key
        </Button>
      </form>
      <ErrorText message={a.error} />
    </Row>
  );
}
