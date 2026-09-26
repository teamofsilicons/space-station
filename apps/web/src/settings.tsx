// Settings: your access token, webhooks, API keys, and this browser's preferences (appearance and
// frontend telemetry).
import { createSignal, For, Show } from "solid-js";
import { api, type ApiKey, type Webhook } from "../lib/api";
import { frontendTelemetryEnabled, setFrontendTelemetryEnabled, trackFrontendEvent } from "../lib/telemetry";
import { action, ago, ErrorText, Loading, resource, Secret, Segmented } from "./ui";
import { useWorkspace } from "./tabs";
import { AccessToken } from "./windows";
import { Icon } from "./icons";
import { theme, setTheme, type Theme } from "./theme";

function Row(p: { title: string; hint: string; children: any }) {
  return (
    <section class="setting">
      <div class="setting-label">
        <h2>{p.title}</h2>
        <p class="muted">{p.hint}</p>
      </div>
      <div class="setting-body">{p.children}</div>
    </section>
  );
}

export function Settings() {
  const ws = useWorkspace();
  const root = `/orgs/${encodeURIComponent(ws.org)}`;
  const [telemetry, setTelemetry] = createSignal(frontendTelemetryEnabled());
  return (
    <div class="page-body">
      <header class="page-head">
        <div>
          <h1>Settings</h1>
          <p class="muted">For {ws.org}, as @{ws.me.id}.</p>
        </div>
      </header>
      <Row title="Access token" hint="Mission control runs your queries with it while you develop a window or a notification. Tied to you and this org; keep it in .env, never in code.">
        <AccessToken root={root} hint={false} />
      </Row>
      <Webhooks root={root} />
      <Keys root={root} />
      <Row title="Appearance" hint="Follows your system unless you choose. Kept in this browser.">
        <Segmented<Theme> label="Theme" value={theme()} options={["system", "light", "dark"]} names={{ system: "System", light: "Light", dark: "Dark" }} onChange={setTheme} />
      </Row>
      <Row title="Frontend telemetry" hint="Optional analytics that help diagnose the Space Station interface.">
        <label class="check">
          <input
            type="checkbox"
            checked={telemetry()}
            onChange={(e) => {
              const enabled = e.currentTarget.checked;
              setTelemetry(enabled);
              setFrontendTelemetryEnabled(enabled);
              trackFrontendEvent("frontend_telemetry_setting_changed", { enabled });
            }}
          />
          Allow frontend analytics
        </label>
      </Row>
    </div>
  );
}

function Webhooks(p: { root: string }) {
  const rows = resource(() => api<Webhook[]>(p.root + "/webhooks")),
    a = action();
  const [url, setUrl] = createSignal(""),
    [made, setMade] = createSignal<{ id: string; secret: string }>();
  return (
    <Row title="Webhooks" hint="Signed notification deliveries to your HTTPS endpoint. Name one as webhook:<id> in a notification's recipients.">
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <div class="items">
            <Show when={!items().length}>
              <p class="empty-line">No webhooks yet.</p>
            </Show>
            <For each={items()}>
              {(h) => (
                <div class="item">
                  <Icon name="link" />
                  <div class="grow">
                    <code>{h.id}</code>
                    <small class="muted">{h.url}</small>
                  </div>
                  <small class="muted">@{h.created_by} · {ago(h.created_at)}</small>
                  <button
                    class="icon danger"
                    aria-label={`Delete webhook ${h.id}`}
                    disabled={a.busy()}
                    onClick={() => {
                      if (confirm("Delete this webhook? Deliveries to it will stop."))
                        a.run(async () => {
                          await api(p.root + "/webhooks/" + h.id, "DELETE");
                          await rows.reload();
                        });
                    }}
                  >
                    <Icon name="trash" />
                  </button>
                </div>
              )}
            </For>
          </div>
        )}
      </Show>
      <Show when={made()}>
        {(m) => (
          <div class="callout">
            <span>Signing secret for <code>{m().id}</code></span>
            <Secret value={m().secret} once />
          </div>
        )}
      </Show>
      <form
        class="inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(await api(p.root + "/webhooks", "POST", { url: url().trim() }));
            setUrl("");
            await rows.reload();
          });
        }}
      >
        <input type="url" required aria-label="Endpoint URL" placeholder="https://example.com/notifications" value={url()} onInput={(e) => setUrl(e.currentTarget.value)} />
        <button disabled={a.busy()}>
          <Icon name="plus" /> Add webhook
        </button>
      </form>
      <ErrorText message={a.error()} />
    </Row>
  );
}

function Keys(p: { root: string }) {
  const rows = resource(() => api<ApiKey[]>(p.root + "/api-keys")),
    a = action();
  const [scopes, setScopes] = createSignal(["tables"]),
    [made, setMade] = createSignal<{ id: string; key: string }>();
  return (
    <Row title="API keys" hint="Read tables and run queries, or read notifications, on behalf of the organization rather than a person.">
      <Show when={rows.data()} fallback={<Loading error={rows.data.error} />}>
        {(items) => (
          <div class="items">
            <Show when={!items().length}>
              <p class="empty-line">No API keys yet.</p>
            </Show>
            <For each={items()}>
              {(k) => (
                <div class="item">
                  <Icon name="key" />
                  <div class="grow">
                    <code>{k.id}</code>
                    <small class="muted">Last used {k.last_used_at ? ago(k.last_used_at) : "never"} · @{k.created_by}</small>
                  </div>
                  <span class="chips">
                    <For each={k.scopes}>{(s) => <span class="chip tag">{s}</span>}</For>
                  </span>
                  <button
                    class="icon danger"
                    aria-label={`Delete API key ${k.id}`}
                    disabled={a.busy()}
                    onClick={() => {
                      if (confirm("Delete this API key? Requests with it will fail."))
                        a.run(async () => {
                          await api(p.root + "/api-keys/" + k.id, "DELETE");
                          await rows.reload();
                        });
                    }}
                  >
                    <Icon name="trash" />
                  </button>
                </div>
              )}
            </For>
          </div>
        )}
      </Show>
      <Show when={made()}>
        {(m) => (
          <div class="callout">
            <span>Key <code>{m().id}</code></span>
            <Secret value={m().key} once />
          </div>
        )}
      </Show>
      <form
        class="inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            setMade(await api(p.root + "/api-keys", "POST", { scopes: scopes() }));
            await rows.reload();
          });
        }}
      >
        <For each={["tables", "notifications"]}>
          {(s) => (
            <label class="check">
              <input type="checkbox" checked={scopes().includes(s)} onChange={() => setScopes((old) => (old.includes(s) ? old.filter((x) => x !== s) : [...old, s]))} />
              {s}
            </label>
          )}
        </For>
        <button disabled={a.busy() || !scopes().length}>
          <Icon name="plus" /> Create key
        </button>
      </form>
      <ErrorText message={a.error()} />
    </Row>
  );
}
