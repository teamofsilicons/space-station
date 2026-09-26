// Space Windows: the list, the create dialog, and one window — its live view (the runtime's host
// role: processor sandbox + renderer iframe, reporting status to the tab and notifications to the
// workspace), the prompt for an agent, the access token, and the code workbench with versions.
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { api, wsUrl, type SpaceWindow, type Table, type Version } from "../lib/api";
import { agentPrompt } from "../lib/agent-prompt";
import { trackFrontendEvent } from "../lib/telemetry";
import { loadRuntime, type Host, type RunError, type Status } from "../lib/runtime";
import { Access, action, ago, Copy, Empty, ErrorText, list, Loading, Modal, resource, Secret, useTab, when } from "./ui";
import { Link, useWorkspace } from "./tabs";
import { Icon } from "./icons";

export function NewWindow(p: { close: () => void }) {
  const ws = useWorkspace();
  const [name, setName] = createSignal(""),
    [access, setAccess] = createSignal("");
  const a = action();
  return (
    <Modal title="New Space Window" close={p.close}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            const w = await api<SpaceWindow>(`/orgs/${encodeURIComponent(ws.org)}/windows`, "POST", { name: name().trim(), access: list(access()) });
            trackFrontendEvent("window_created", { window: w.id });
            await ws.windows.reload();
            p.close();
            ws.tabs.reveal(`/o/${ws.org}/windows/${w.id}`, "tab");
          });
        }}
      >
        <label>
          Name
          <input required maxlength={19} value={name()} onInput={(e) => setName(e.currentTarget.value)} placeholder="Orders live" autofocus />
        </label>
        <label>
          Access <span class="muted">(optional)</span>
          <input value={access()} onInput={(e) => setAccess(e.currentTarget.value)} placeholder="@c:alice, engineering" />
        </label>
        <p class="hint">Shown as the window's title, under 20 characters. You are added to its access automatically.</p>
        <ErrorText message={a.error()} />
        <div class="dialog-foot">
          <button type="button" onClick={p.close}>Cancel</button>
          <button class="primary" disabled={a.busy()}>Create window</button>
        </div>
      </form>
    </Modal>
  );
}

export function Windows() {
  const ws = useWorkspace();
  return (
    <div class="page-body">
      <header class="page-head">
        <div>
          <h1>Space Windows</h1>
          <p class="muted">Live views and tools built from your tables. Open several side by side — each keeps running in its tab.</p>
        </div>
        <button class="primary" onClick={() => ws.create("window")}>
          <Icon name="plus" /> New window
        </button>
      </header>
      <Show when={ws.windows.data()} fallback={<Loading error={ws.windows.data.error} />}>
        {(items) => (
          <Show
            when={items().length}
            fallback={
              <div class="hero-empty">
                <Empty icon="window" title="Create your first Space Window">
                  <p>A processor turns records into a small JSON document; a renderer shows it live. Name it, then hand the prompt to an agent.</p>
                  <button class="primary large" onClick={() => ws.create("window")}>
                    <Icon name="plus" /> Create your first Space Window
                  </button>
                </Empty>
              </div>
            }
          >
            <div class="card-grid">
              <For each={items()}>
                {(w) => (
                  <Link href={`/o/${ws.org}/windows/${w.id}`} class="card window-card">
                    <span class="card-top">
                      <span class="title-icon"><Icon name="window" /></span>
                      <Show when={w.version} fallback={<span class="pill">No version</span>}>
                        <span class="pill live"><i />Live · {w.version!.name}</span>
                      </Show>
                    </span>
                    <strong>{w.name}</strong>
                    <Access value={w.access} />
                    <small class="muted">@{w.created_by} · {ago(w.created_at)}</small>
                  </Link>
                )}
              </For>
              <button class="card add-card" onClick={() => ws.create("window")}>
                <Icon name="plus" size={20} />
                New window
              </button>
            </div>
          </Show>
        )}
      </Show>
      <p class="tip">
        <Icon name="keyboard" /> ⌘-click a window to open it behind, ⇧-click to open it beside this one.
      </p>
    </div>
  );
}

export function WindowView() {
  const tab = useTab();
  const ws = useWorkspace();
  const id = tab.route().id;
  const root = `/orgs/${encodeURIComponent(ws.org)}`,
    path = `${root}/windows/${id}`;
  const win = resource(() => api<SpaceWindow>(path)),
    versions = resource(() => api<Version[]>(path + "/versions")),
    tables = resource(() => api<Table[]>(root + "/tables"));
  const code = () => tab.route().kind === "code";
  createEffect(() => win.data() && tab.title(code() ? `${win.data()!.name} · code` : win.data()!.name));
  const prompt = createMemo(() => {
    const w = win.data();
    return w ? agentPrompt({ org: ws.org, window: w, tables: (tables.data() || []).map((t) => t.id) }) : "";
  });
  const [status, setStatus] = createSignal<Status>();
  const changed = async () => {
    await Promise.all([win.reload(), versions.reload(), ws.windows.reload()]);
  };
  const toggle = () => tab.go(`/o/${ws.org}/windows/${id}${code() ? "" : "/code"}`);
  return (
    <Show when={win.data()} fallback={<div class="page-body"><Loading error={win.data.error} /></div>}>
      {(w) => (
        <div class="window-page" classList={{ coding: code() }}>
          <header class="window-head">
            <span class="title-icon"><Icon name="window" size={18} /></span>
            <h1>{w().name}</h1>
            <Show when={w().version} fallback={<span class="pill">No version</span>}>
              <span class="pill" classList={{ live: !!status()?.connected && !!status()?.is_live, stale: !!status() && !(status()!.connected && status()!.is_live) }}>
                <i />
                {status() ? (status()!.connected ? (status()!.is_live ? "Live" : "Stale") : "Disconnected") : "Starting"} · {w().version!.name}
              </span>
            </Show>
            <Access value={w().access} save={async (access) => { await api(path, "PUT", { access }); trackFrontendEvent("window_access_updated", { window: w().id }); await changed(); }} />
            <span class="grow" />
            <details class="pop">
              <summary class="button ghost small" title="Access token"><Icon name="key" /><span class="label">Access token</span></summary>
              <div class="pop-body">
                <AccessToken root={root} />
              </div>
            </details>
            <details class="pop">
              <summary class="button ghost small" title="Agent prompt"><Icon name="flask" /><span class="label">Agent prompt</span></summary>
              <div class="pop-body wide">
                <div class="row"><strong>Prompt for an agent</strong><span class="grow" /><Copy text={prompt()} /></div>
                <pre class="prompt">{prompt()}</pre>
              </div>
            </details>
            <button class={code() ? "primary small" : "small"} onClick={toggle} title="Toggle the code workbench">
              <Icon name="code" /><span class="label">{code() ? "Close code" : "Code"}</span>
            </button>
          </header>
          <div class="window-body">
            <Show
              when={w().version}
              fallback={
                <Show when={!code()}>
                  <div class="page-body narrow">
                    <Empty icon="flask" title="No live view yet">
                      <p>Give this prompt to any agent. It writes a processor and a renderer; paste them in and publish a version.</p>
                      <div class="row center">
                        <Copy text={prompt()} label="Copy prompt" />
                        <button class="primary" onClick={toggle}><Icon name="code" /> Add code</button>
                      </div>
                    </Empty>
                    <pre class="prompt">{prompt()}</pre>
                  </div>
                </Show>
              }
            >
              <Live org={ws.org} window={w()} onStatus={setStatus} onVersionChanged={changed} />
            </Show>
            <Show when={code()}>
              <Workbench path={path} window={w()} versions={versions.data() || []} published={changed} />
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}

function Workbench(p: { path: string; window: SpaceWindow; versions: Version[]; published: () => Promise<void> }) {
  const [name, setName] = createSignal(""),
    [processor, setProcessor] = createSignal(p.window.version?.processor || ""),
    [renderer, setRenderer] = createSignal(p.window.version?.renderer || ""),
    [file, setFile] = createSignal<"processor" | "renderer">("processor");
  const a = action();
  return (
    <form
      class="workbench"
      onSubmit={(e) => {
        e.preventDefault();
        a.run(async () => {
          // One editor is always hidden, so the browser cannot point at an empty one; say which.
          const empty = !processor().trim() ? "processor" : !renderer().trim() ? "renderer" : null;
          if (empty) {
            setFile(empty);
            throw Error(`${empty === "processor" ? "processor.js" : "renderer.html"} is empty.`);
          }
          await api(p.path + "/versions", "POST", { name: name(), processor: processor(), renderer: renderer() });
          await p.published();
          setName("");
        });
      }}
    >
      <div class="wb-bar">
        <div class="wb-files" role="tablist">
          <button type="button" role="tab" aria-selected={file() === "processor"} classList={{ on: file() === "processor" }} onClick={() => setFile("processor")}>processor.js</button>
          <button type="button" role="tab" aria-selected={file() === "renderer"} classList={{ on: file() === "renderer" }} onClick={() => setFile("renderer")}>renderer.html</button>
        </div>
        <select
          aria-label="Load a version"
          onChange={(e) => {
            const v = p.versions.find((v) => v.id === e.currentTarget.value);
            if (v) {
              setProcessor(v.processor);
              setRenderer(v.renderer);
            }
            e.currentTarget.value = "";
          }}
        >
          <option value="">Load a version…</option>
          <For each={p.versions}>
            {(v) => (
              <option value={v.id}>
                {v.name}
                {v.name === p.window.version?.name ? " (current)" : ""} · @{v.created_by} · {ago(v.created_at)}
              </option>
            )}
          </For>
        </select>
      </div>
      <CodeEditor label="processor.js" value={processor()} onInput={setProcessor} hidden={file() !== "processor"} />
      <CodeEditor label="renderer.html" value={renderer()} onInput={setRenderer} hidden={file() !== "renderer"} />
      <div class="wb-foot">
        <input required aria-label="New version name" value={name()} onInput={(e) => setName(e.currentTarget.value)} placeholder="Version name, e.g. v2" />
        <button class="primary" disabled={a.busy()}>Publish version</button>
      </div>
      <ErrorText message={a.error()} />
    </form>
  );
}

function CodeEditor(p: { label: string; value: string; onInput: (value: string) => void; hidden?: boolean }) {
  const escaped = () => p.value.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]!);
  const highlighted = () =>
    escaped().replace(
      /(\/\/[^\n]*|&lt;!--[\s\S]*?--&gt;)|("(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`)|\b(const|let|var|return|export|default|function|async|await|if|else|new|true|false|null|for|of|in|while|import|from)\b|(&lt;\/?[a-zA-Z][\w-]*)/g,
      (m, comment, str, kw, tag) => `<span class="${comment ? "t-c" : str ? "t-s" : kw ? "t-k" : tag ? "t-t" : ""}">${m}</span>`,
    );
  let pre!: HTMLPreElement;
  return (
    <div class="editor" hidden={p.hidden}>
      <pre ref={pre} aria-hidden="true" innerHTML={highlighted() + "\n"} />
      <textarea
        aria-label={p.label}
        spellcheck={false}
        value={p.value}
        onInput={(e) => p.onInput(e.currentTarget.value)}
        onScroll={(e) => {
          pre.scrollTop = e.currentTarget.scrollTop;
          pre.scrollLeft = e.currentTarget.scrollLeft;
        }}
        onKeyDown={(e) => {
          if (e.key !== "Tab") return;
          e.preventDefault();
          const t = e.currentTarget;
          t.setRangeText("  ", t.selectionStart, t.selectionEnd, "end");
          p.onInput(t.value);
        }}
      />
    </div>
  );
}

export function AccessToken(p: { root: string; hint?: boolean }) {
  const token = resource(() => api<{ token: string; last_used_at: string | null }>(p.root + "/access-token"));
  const a = action();
  return (
    <div class="token">
      <Show when={p.hint !== false}>
        <p class="hint">Lets mission control run your queries during development. Tied to you and this org; keep it in <code>.env</code>, never in code.</p>
      </Show>
      <Show when={token.data()} fallback={<Loading error={token.data.error} />}>
        {(t) => (
          <>
            <Secret value={t().token} />
            <small class="muted">Last used {when(t().last_used_at)}</small>
          </>
        )}
      </Show>
      <button
        class="small"
        disabled={a.busy()}
        onClick={() => {
          if (confirm("Rotate your access token? Its current value will stop working everywhere."))
            a.run(async () => {
              await api(p.root + "/access-token/rotate", "POST");
              await token.reload();
            });
        }}
      >
        <Icon name="reload" /> Rotate token
      </button>
      <ErrorText message={a.error()} />
    </div>
  );
}

function Live(p: { org: string; window: SpaceWindow; onStatus: (s: Status) => void; onVersionChanged: () => Promise<void> }) {
  const tab = useTab();
  const ws = useWorkspace();
  let mount!: HTMLDivElement;
  const [errors, setErrors] = createSignal<RunError[]>([]),
    [attempt, setAttempt] = createSignal(0);
  createEffect(() => {
    const w = p.window;
    attempt();
    let gone = false,
      refreshing = false,
      host: Host | undefined;
    setErrors([]);
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
          onJson: () => tab.mark({ activity: true }),
          onStatus: (s) => {
            p.onStatus(s);
            tab.mark({ live: s.connected && s.is_live ? "ok" : "stale" });
          },
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
            tab.mark({ live: "error" });
            ws.report(w.name, e);
            setErrors((old) => [...old, e].slice(-10));
          },
          onNotification: (n) =>
            ws.toast({
              key: n.event_id,
              title: n.name,
              text: n.text,
              tone: "note",
              action: tab.visible() ? undefined : { label: `Open ${w.name}`, run: () => ws.tabs.show(tab.id) },
            }),
        });
      })
      .catch((e) => setErrors([{ source: "host", message: e.message, detail: null }]));
    onCleanup(() => {
      gone = true;
      host?.destroy();
      tab.mark({ live: undefined, activity: false });
    });
  });
  return (
    <div class="live">
      <div class="live-mount" ref={mount} />
      <Show when={errors().length}>
        <div class="live-errors" role="alert">
          <div class="row">
            <Icon name="bug" />
            <strong>{errors().length} error{errors().length > 1 ? "s" : ""} in this window</strong>
            <span class="grow" />
            <button class="small" onClick={() => setAttempt(attempt() + 1)}>
              <Icon name="reload" /> Restart window
            </button>
          </div>
          <For each={errors()}>
            {(e) => (
              <details>
                <summary>
                  <code>{e.source}</code> {e.message}
                </summary>
                <pre>{JSON.stringify(e.detail, null, 2)}</pre>
              </details>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
