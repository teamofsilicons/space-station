"use client";
// Space Windows: the list, the create dialog, and one window — its live view (the runtime's host
// role: processor sandbox + renderer iframe, reporting status to the tab and notifications to the
// workspace), the prompt for an agent, the access token, and the code workbench with versions.
import { useEffect, useMemo, useRef, useState } from "react";
import { Badge } from "@/components/silicon-ui/badge/badge";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { JsonViewer } from "@/components/silicon-ui/json-viewer/json-viewer";
import { api, currentContext, wsUrl, type SpaceWindow, type Table, type Version } from "../lib/api";
import { agentPrompt } from "../lib/agent-prompt";
import { loadRuntime, type Host, type RunError, type Status } from "../lib/runtime";
import { ago, Copy, Empty, ErrorText, Loading, Modal, Secret, useAction, useResource, useTab, useTitle, when } from "./ui";
import { Link, useLists, useWorkspace } from "./tabs";
import { Icon } from "./icons";

export function NewWindow(p: { close: () => void }) {
  const ws = useWorkspace();
  const { windows } = useLists();
  const [name, setName] = useState("");
  const a = useAction();
  return (
    <Modal title="New Space Window" close={p.close}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          a.run(async () => {
            const w = await api<SpaceWindow>(`/windows`, "POST", { name: name.trim() });
            await windows.reload();
            p.close();
            ws.tabs.reveal(`/a/${ws.actor}/windows/${w.id}`, "tab");
          });
        }}
      >
        <Input
          label="Name"
          description="Shown as the window's title, under 20 characters. Owned by your account."
          required
          maxLength={19}
          value={name}
          onChange={(e) => setName(e.currentTarget.value)}
          placeholder="Orders live"
          autoFocus
        />
        <ErrorText message={a.error} />
        <div className="dialog-foot">
          <Button type="button" variant="secondary" onClick={p.close}>
            Cancel
          </Button>
          <Button type="submit" loading={a.busy}>
            Create window
          </Button>
        </div>
      </form>
    </Modal>
  );
}

export function Windows() {
  const ws = useWorkspace();
  const { windows } = useLists();
  const items = windows.data;
  return (
    <div className="page-body">
      <header className="page-head">
        <div>
          <h1>Space Windows</h1>
          <p className="muted">Live views and tools built from your tables. Open several side by side — each keeps running in its tab.</p>
        </div>
        <Button onClick={() => ws.create("window")}>
          <Icon name="plus" /> New window
        </Button>
      </header>
      {!items ? (
        <Loading error={windows.error} />
      ) : !items.length ? (
        <div className="hero-empty">
          <Empty
            icon="window"
            title="Create your first Space Window"
            description="A processor turns records into a small JSON document; a renderer shows it live. Name it, then hand the prompt to an agent."
            action={
              <Button size="lg" onClick={() => ws.create("window")}>
                <Icon name="plus" /> Create your first Space Window
              </Button>
            }
          />
        </div>
      ) : (
        <div className="card-grid">
          {items.map((w) => (
            <Link key={w.id} href={`/a/${ws.actor}/windows/${w.id}`} className="card window-card">
              <span className="card-top">
                <span className="title-icon">
                  <Icon name="window" />
                </span>
                {w.version ? (
                  <Badge size="sm" tone="success" icon={<i className="live-dot ok" />}>{`Live · ${w.version.name}`}</Badge>
                ) : (
                  <Badge size="sm">No version</Badge>
                )}
              </span>
              <strong>{w.name}</strong>
              <Badge>{ws.me.id}</Badge>
              <small className="muted">
                @{w.created_by} · {ago(w.created_at)}
              </small>
            </Link>
          ))}
          <button className="card add-card" onClick={() => ws.create("window")}>
            <Icon name="plus" size={20} />
            New window
          </button>
        </div>
      )}
      <p className="tip">
        <Icon name="keyboard" /> ⌘-click a window to open it behind, ⇧-click to open it beside this one.
      </p>
    </div>
  );
}

export function WindowView() {
  const tab = useTab();
  const ws = useWorkspace();
  const { windows } = useLists();
  const id = tab.route.id;
  const path = `/windows/${id}`;
  const win = useResource(() => api<SpaceWindow>(path)),
    versions = useResource(() => api<Version[]>(path + "/versions")),
    tables = useResource(() => api<Table[]>("/tables"));
  const code = tab.route.kind === "code";
  const w = win.data;
  useTitle(w && (code ? `${w.name} · code` : w.name));
  const prompt = useMemo(() => (w ? agentPrompt({ actor: ws.me.id, window: w, tables: (tables.data || []).map((t) => t.id) }) : ""), [w, tables.data, ws.me.id]);
  const [status, setStatus] = useState<Status>();
  const changed = async () => {
    await Promise.all([win.reload(), versions.reload(), windows.reload()]);
  };
  const toggle = () => tab.go(`/a/${ws.actor}/windows/${id}${code ? "" : "/code"}`);
  if (!w)
    return (
      <div className="page-body">
        <Loading error={win.error} />
      </div>
    );
  const live = !!status?.connected && !!status.is_live;
  return (
    <div className={code ? "window-page coding" : "window-page"}>
      <header className="window-head">
        <span className="title-icon">
          <Icon name="window" size={18} />
        </span>
        <h1>{w.name}</h1>
        {w.version ? (
          <Badge size="sm" tone={live ? "success" : status ? "warning" : "neutral"} icon={<i className={live ? "live-dot ok" : status ? "live-dot stale" : "live-dot"} />}>
            {`${status ? (status.connected ? (status.is_live ? "Live" : "Stale") : "Disconnected") : "Starting"} · ${w.version.name}`}
          </Badge>
        ) : (
          <Badge size="sm">No version</Badge>
        )}
        <Badge>{ws.me.id}</Badge>
        <span className="grow" />
        <details className="pop">
          <summary className="button ghost small" title="Access token">
            <Icon name="key" />
            <span className="label">Access token</span>
          </summary>
          <div className="pop-body">
            <AccessToken />
          </div>
        </details>
        <details className="pop">
          <summary className="button ghost small" title="Agent prompt">
            <Icon name="flask" />
            <span className="label">Agent prompt</span>
          </summary>
          <div className="pop-body wide">
            <div className="row">
              <strong>Prompt for an agent</strong>
              <span className="grow" />
              <Copy text={prompt} />
            </div>
            <pre className="prompt">{prompt}</pre>
          </div>
        </details>
        <Button variant={code ? "primary" : "secondary"} size="sm" onClick={toggle} title="Toggle the code workbench">
          <Icon name="code" />
          <span className="label">{code ? "Close code" : "Code"}</span>
        </Button>
      </header>
      <div className="window-body">
        {/* The live view holds this slot across the code toggle, so its iframes are never moved or rebuilt. */}
        {w.version ? (
          <Live window={w} onStatus={setStatus} onVersionChanged={changed} />
        ) : (
          !code && (
            <div className="page-body narrow">
              <Empty
                icon="flask"
                title="No live view yet"
                description="Give this prompt to any agent. It writes a processor and a renderer; paste them in and publish a version."
                action={
                  <div className="row center">
                    <Copy text={prompt} label="Copy prompt" />
                    <Button onClick={toggle}>
                      <Icon name="code" /> Add code
                    </Button>
                  </div>
                }
              />
              <pre className="prompt">{prompt}</pre>
            </div>
          )
        )}
        {code && <Workbench path={path} window={w} versions={versions.data || []} published={changed} />}
      </div>
    </div>
  );
}

function Workbench(p: { path: string; window: SpaceWindow; versions: Version[]; published: () => Promise<void> }) {
  const [name, setName] = useState(""),
    [processor, setProcessor] = useState(p.window.version?.processor || ""),
    [renderer, setRenderer] = useState(p.window.version?.renderer || ""),
    [file, setFile] = useState<"processor" | "renderer">("processor");
  const a = useAction();
  return (
    <form
      className="workbench"
      onSubmit={(e) => {
        e.preventDefault();
        a.run(async () => {
          // One editor is always hidden, so the browser cannot point at an empty one; say which.
          const empty = !processor.trim() ? "processor" : !renderer.trim() ? "renderer" : null;
          if (empty) {
            setFile(empty);
            throw Error(`${empty === "processor" ? "processor.js" : "renderer.html"} is empty.`);
          }
          await api(p.path + "/versions", "POST", { name, processor, renderer });
          await p.published();
          setName("");
        });
      }}
    >
      <div className="wb-bar">
        <div className="wb-files" role="tablist">
          <button type="button" role="tab" aria-selected={file === "processor"} className={file === "processor" ? "on" : undefined} onClick={() => setFile("processor")}>
            processor.js
          </button>
          <button type="button" role="tab" aria-selected={file === "renderer"} className={file === "renderer" ? "on" : undefined} onClick={() => setFile("renderer")}>
            renderer.html
          </button>
        </div>
        {/* Picking a version loads it into the editors; the picker itself stays on its prompt. */}
        <select
          aria-label="Load a version"
          value=""
          onChange={(e) => {
            const v = p.versions.find((v) => v.id === e.currentTarget.value);
            if (v) {
              setProcessor(v.processor);
              setRenderer(v.renderer);
            }
          }}
        >
          <option value="">Load a version…</option>
          {p.versions.map((v) => (
            <option key={v.id} value={v.id}>
              {`${v.name}${v.name === p.window.version?.name ? " (current)" : ""} · @${v.created_by} · ${ago(v.created_at)}`}
            </option>
          ))}
        </select>
      </div>
      <CodeEditor label="processor.js" value={processor} onInput={setProcessor} hidden={file !== "processor"} />
      <CodeEditor label="renderer.html" value={renderer} onInput={setRenderer} hidden={file !== "renderer"} />
      <div className="wb-foot">
        <input required aria-label="New version name" value={name} onChange={(e) => setName(e.currentTarget.value)} placeholder="Version name, e.g. v2" />
        <Button type="submit" loading={a.busy}>
          Publish version
        </Button>
      </div>
      <ErrorText message={a.error} />
    </form>
  );
}

function CodeEditor(p: { label: string; value: string; onInput: (value: string) => void; hidden?: boolean }) {
  const escaped = p.value.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]!);
  const highlighted = escaped.replace(
    /(\/\/[^\n]*|&lt;!--[\s\S]*?--&gt;)|("(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`)|\b(const|let|var|return|export|default|function|async|await|if|else|new|true|false|null|for|of|in|while|import|from)\b|(&lt;\/?[a-zA-Z][\w-]*)/g,
    (m, comment, str, kw, tag) => `<span class="${comment ? "t-c" : str ? "t-s" : kw ? "t-k" : tag ? "t-t" : ""}">${m}</span>`,
  );
  const pre = useRef<HTMLPreElement>(null);
  return (
    <div className="editor" hidden={p.hidden}>
      <pre ref={pre} aria-hidden="true" dangerouslySetInnerHTML={{ __html: highlighted + "\n" }} />
      <textarea
        aria-label={p.label}
        spellCheck={false}
        value={p.value}
        onChange={(e) => p.onInput(e.currentTarget.value)}
        onScroll={(e) => {
          pre.current!.scrollTop = e.currentTarget.scrollTop;
          pre.current!.scrollLeft = e.currentTarget.scrollLeft;
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

export function AccessToken(p: { hint?: boolean }) {
  const token = useResource(() => api<{ token: string; last_used_at: string | null }>("/access-token"));
  const a = useAction();
  return (
    <div className="token">
      {p.hint !== false && (
        <p className="hint">
          Lets mission control run your queries during development. Tied to your account; keep it in <code>.env</code>, never in code.
        </p>
      )}
      {token.data ? (
        <>
          <Secret value={token.data.token} />
          <small className="muted">Last used {when(token.data.last_used_at)}</small>
        </>
      ) : (
        <Loading error={token.error} />
      )}
      <Button
        variant="secondary"
        size="sm"
        loading={a.busy}
        onClick={() => {
          if (confirm("Rotate your access token? Its current value will stop working everywhere."))
            a.run(async () => {
              await api("/access-token/rotate", "POST");
              await token.reload();
            });
        }}
      >
        <Icon name="reload" /> Rotate token
      </Button>
      <ErrorText message={a.error} />
    </div>
  );
}

/** A row per error object, kept as older errors drop off the front of the list. */
const errorKeys = new WeakMap<RunError, number>();
let errorKey = 0;
const keyOf = (e: RunError) => errorKeys.get(e) ?? (errorKeys.set(e, ++errorKey), errorKey);

/**
 * The runtime's host, in a node only it fills. One host per window version and restart: it starts in
 * an effect and is destroyed in that effect's cleanup, so it lives on while the tab is hidden and
 * across the code toggle, and a StrictMode double mount never leaves a second one behind.
 */
function Live(p: { window: SpaceWindow; onStatus: (s: Status) => void; onVersionChanged: () => Promise<void> }) {
  const tab = useTab();
  const ws = useWorkspace();
  const mount = useRef<HTMLDivElement>(null);
  const [errors, setErrors] = useState<RunError[]>([]),
    [attempt, setAttempt] = useState(0);
  // The host outlives renders; what it calls back is read from the latest one.
  const latest = useRef({ tab, p });
  latest.current = { tab, p };
  useEffect(() => {
    const w = p.window;
    let gone = false,
      refreshing = false,
      host: Host | undefined;
    setErrors([]);
    loadRuntime()
      .then((runtime) => {
        if (gone) return;
        host = runtime.host({
          runtimeUrl: location.origin + "/mission-control.js",
          api: { base: "/api", headers: { "x-spacestation-context": currentContext()! } },
          ws: wsUrl(),
          window: w,
          mount: mount.current!,
          onJson: () => latest.current.tab.mark({ activity: true }),
          onStatus: (s) => {
            latest.current.p.onStatus(s);
            latest.current.tab.mark({ live: s.connected && s.is_live ? "ok" : "stale" });
          },
          onError: (e) => {
            if (e.source === "server" && e.message.startsWith("version_not_current:")) {
              if (!refreshing) {
                refreshing = true;
                latest.current.p.onVersionChanged().catch((error) => {
                  refreshing = false;
                  setErrors([{ source: "host", message: String(error), detail: null }]);
                });
              }
              return;
            }
            latest.current.tab.mark({ live: "error" });
            ws.report(w.name, e);
            setErrors((old) => [...old, e].slice(-10));
          },
          onNotification: (n) => {
            const t = latest.current.tab;
            ws.toast({
              key: n.event_id,
              title: n.name,
              text: n.text,
              tone: "note",
              action: t.visible ? undefined : { label: `Open ${w.name}`, run: () => ws.tabs.show(t.id) },
            });
          },
        });
      })
      .catch((e) => setErrors([{ source: "host", message: e.message, detail: null }]));
    return () => {
      gone = true;
      host?.destroy();
      latest.current.tab.mark({ live: undefined, activity: false });
    };
  }, [p.window, attempt, ws]);
  return (
    <div className="live">
      <div className="live-mount" ref={mount} />
      {errors.length > 0 && (
        <div className="live-errors" role="alert">
          <div className="row">
            <Icon name="bug" />
            <strong>
              {errors.length} error{errors.length > 1 ? "s" : ""} in this window
            </strong>
            <span className="grow" />
            <Button variant="secondary" size="sm" onClick={() => setAttempt(attempt + 1)}>
              <Icon name="reload" /> Restart window
            </Button>
          </div>
          {errors.map((e) => (
            <details key={keyOf(e)}>
              <summary>
                <code>{e.source}</code> {e.message}
              </summary>
              <JsonViewer data={e.detail} rootName="detail" defaultExpandDepth={2} copyable />
            </details>
          ))}
        </div>
      )}
    </div>
  );
}
