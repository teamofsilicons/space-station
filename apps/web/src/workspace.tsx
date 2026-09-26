// The signed-in station: a sidebar of the org's windows and tables, a strip of tabs over up to
// three panes, the ⌘K palette, a developer-errors drawer (⌥⇧D), toasts, and the keyboard.
import { createEffect, createSignal, For, on, onCleanup, Show, type Accessor, type Component } from "solid-js";
import { api, type DevError, type Me, type Org, type SpaceWindow, type Table } from "../lib/api";
import type { RunError } from "../lib/runtime";
import * as T from "../lib/tabs";
import { trackFrontendEvent } from "../lib/telemetry";
import { createTabs, Link, Panes, TabStrip, useWorkspace, WorkspaceContext, type Toast, type Workspace as W } from "./tabs";
import { closeMenu, count, Loading, MenuHost, openMenu, resource, when, type MenuItem } from "./ui";
import { Icon, Mark } from "./icons";
import { Palette } from "./palette";
import { Home } from "./home";
import { NewTable, Tables, TableView } from "./tables";
import { NewWindow, Windows, WindowView } from "./windows";
import { Notifications } from "./notifications";
import { Settings } from "./settings";
import { DocsPage } from "./docs";
import { setTheme, theme } from "./theme";

const PAGES: Record<T.Kind, Component> = {
  home: Home,
  tables: Tables,
  table: TableView,
  windows: Windows,
  window: WindowView,
  code: WindowView,
  notifications: Notifications,
  settings: Settings,
  docs: DocsPage,
};

export const loginUrl = (next = "/", org?: string) => `/api/auth/login?next=${encodeURIComponent(next)}${org ? `&org=${encodeURIComponent(org)}` : ""}`;

export function Workspace(p: { me: Me; orgs: Accessor<Org[]>; path: string }) {
  const org = p.me.org,
    home = `/o/${org}`,
    root = `/orgs/${encodeURIComponent(org)}`;
  const tables = resource(() => api<Table[]>(`${root}/tables`), 15000);
  const windows = resource(() => api<SpaceWindow[]>(`${root}/windows`), 30000);
  const arrived = T.route(p.path);
  const fresh = !localStorage.getItem(`ss-tabs:${org}`) && (!arrived || arrived.kind === "home");
  const tabs = createTabs(org, home, arrived && arrived.kind !== "home" ? p.path : `${home}/windows`);
  if (arrived) tabs.arrive(p.path);
  // A first visit opens Space Windows once tables exist, Tables before that — once, never again.
  let decided = !fresh;
  createEffect(
    on(tables.data, (d) => {
      if (decided || !d) return;
      decided = true;
      if (!d.length && tabs.active()?.path === `${home}/windows`) tabs.redirect(`${home}/tables`);
    }),
  );
  const [palette, setPalette] = createSignal(false);
  const [creating, setCreating] = createSignal<"table" | "window">();
  const [dev, setDev] = createSignal(false);
  const [toasts, setToasts] = createSignal<(Toast & { id: number })[]>([]);
  const [runErrors, setRunErrors] = createSignal<{ window: string; error: RunError; at: number }[]>([]);
  const [collapsed, setCollapsed] = createSignal(localStorage.getItem("ss-sidebar") === "collapsed");
  const [nav, setNav] = createSignal(false);
  let toastId = 0;
  const seen = new Set<string>();
  const ws: W = {
    org,
    me: p.me,
    orgs: p.orgs,
    tables,
    windows,
    tabs,
    toast: (t) => {
      if (t.key && seen.has(t.key)) return;
      if (t.key) seen.add(t.key);
      const id = ++toastId;
      setToasts((old) => [...old.slice(-3), { ...t, id }]);
      setTimeout(() => setToasts((old) => old.filter((x) => x.id !== id)), 7000);
    },
    palette: () => {
      closeMenu();
      setPalette(true);
    },
    create: setCreating,
    dev: () => setDev(!dev()),
    report: (window, error) => setRunErrors((old) => [{ window, error, at: Date.now() }, ...old].slice(0, 50)),
    runErrors,
  };

  const keys = (e: KeyboardEvent) => {
    const t = tabs.state(),
      cur = tabs.active();
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.code === "KeyK") {
      e.preventDefault();
      palette() ? setPalette(false) : ws.palette();
      return;
    }
    if (!e.altKey || e.metaKey || e.ctrlKey) return;
    // ⌥ also types characters ([ ] { } | on many layouts) and moves by word: leave fields and dialogs alone.
    if ((e.target as Element).closest?.("input, textarea, select, [contenteditable]") || document.querySelector("dialog:modal")) return;
    const act = (fn: () => unknown) => {
      e.preventDefault();
      fn();
    };
    if (e.shiftKey) {
      if (e.code === "KeyD") act(ws.dev);
      else if (e.code === "KeyT") act(tabs.reopen);
      return;
    }
    const digit = /^Digit([1-9])$/.exec(e.code);
    if (digit) act(() => tabs.show(t.tabs[digit[1] === "9" ? t.tabs.length - 1 : Math.min(Number(digit[1]) - 1, t.tabs.length - 1)].id));
    else if (e.code === "KeyT") act(() => tabs.open(home, "tab", true));
    else if (e.code === "KeyW" && cur) act(() => tabs.close([cur.id]));
    else if (e.code === "BracketLeft") act(() => tabs.cycle(-1));
    else if (e.code === "BracketRight") act(() => tabs.cycle(1));
    else if (e.code === "ArrowLeft" && cur) act(() => tabs.walk(cur.id, -1));
    else if (e.code === "ArrowRight" && cur) act(() => tabs.walk(cur.id, 1));
    else if (e.code === "Backslash") act(() => (t.panes.length > 1 ? tabs.unsplit(t.focus) : tabs.open(home, "split")));
  };
  // Clicking into a live window's iframe focuses its pane; the page only sees its own window blur.
  const blur = () =>
    setTimeout(() => {
      const id = document.activeElement?.closest?.(".frame")?.getAttribute("data-tab");
      if (id) tabs.show(id);
    });
  addEventListener("keydown", keys);
  addEventListener("blur", blur);
  // A popover closes on any click outside it.
  const pops = (e: PointerEvent) => document.querySelectorAll("details.pop[open]").forEach((d) => !d.contains(e.target as Node) && d.removeAttribute("open"));
  addEventListener("pointerdown", pops);
  onCleanup(() => {
    removeEventListener("keydown", keys);
    removeEventListener("blur", blur);
    removeEventListener("pointerdown", pops);
  });

  return (
    <WorkspaceContext.Provider value={ws}>
      <div class="station" classList={{ collapsed: collapsed(), "nav-open": nav() }} onClick={(e) => nav() && (e.target as HTMLElement).closest(".sidebar a, .main") && setNav(false)}>
        <Sidebar
          collapsed={collapsed()}
          toggle={() => {
            setCollapsed(!collapsed());
            localStorage.setItem("ss-sidebar", collapsed() ? "collapsed" : "open");
          }}
          dev={dev()}
        />
        <div class="main">
          <div class="strip-row">
            <button class="icon strip-menu" aria-label="Open the sidebar" onClick={(e) => (e.stopPropagation(), setNav(true))}>
              <Icon name="panel" />
            </button>
            <TabStrip />
            <button class="icon strip-search" title="Search tabs and everything else (⌘K)" onClick={ws.palette}>
              <Icon name="search" />
            </button>
          </div>
          <Panes pages={PAGES} />
          <Show when={dev()}>
            <DevDrawer close={() => setDev(false)} />
          </Show>
        </div>
      </div>
      <Show when={palette()}>
        <Palette close={() => setPalette(false)} />
      </Show>
      <Show when={creating() === "table"}>
        <NewTable close={() => setCreating()} />
      </Show>
      <Show when={creating() === "window"}>
        <NewWindow close={() => setCreating()} />
      </Show>
      <div class="toasts" aria-live="polite">
        <For each={toasts()}>
          {(t) => (
            <div class={`toast ${t.tone || ""}`}>
              <Icon name={t.tone === "bad" ? "bug" : t.tone === "ok" ? "check" : "bell"} />
              <div class="grow">
                <Show when={t.title}>
                  <strong>{t.title}</strong>
                </Show>
                <span>{t.text}</span>
              </div>
              <Show when={t.action}>
                <button class="small" onClick={() => (t.action!.run(), setToasts((old) => old.filter((x) => x !== t)))}>
                  {t.action!.label}
                </button>
              </Show>
              <button class="icon" aria-label="Dismiss" onClick={() => setToasts((old) => old.filter((x) => x !== t))}>
                <Icon name="x" />
              </button>
            </div>
          )}
        </For>
      </div>
      <MenuHost />
    </WorkspaceContext.Provider>
  );
}

function Sidebar(p: { collapsed: boolean; toggle: () => void; dev: boolean }) {
  const ws = useWorkspace();
  const o = `/o/${ws.org}`;
  const here = () => ws.tabs.active()?.path;
  const open = (path: string) => ws.tabs.state().tabs.some((t) => t.path === path);
  const item = (path: string, icon: string, label: string, extra?: any) => (
    <Link href={path} reuse class="side-link" classList={{ on: here() === path, open: open(path) && here() !== path }} title={label}>
      <Icon name={icon} />
      <span class="side-label">{label}</span>
      {extra}
    </Link>
  );
  const orgMenu = (e: MouseEvent) => {
    const rows = ws.orgs();
    const all = rows.some((x) => x.id === ws.org) ? rows : [{ id: ws.org, name: ws.org }, ...rows];
    openMenu(e, [
      ...all.map((x): MenuItem => ({ label: x.name || x.id, icon: x.id === ws.org ? "check" : "blank", run: () => x.id !== ws.org && location.assign(loginUrl(`/o/${x.id}`, x.id)) })),
      "-",
      { label: "Add an organization", icon: "plus", run: () => location.assign(loginUrl()) },
    ]);
  };
  const account = (e: MouseEvent) =>
    openMenu(e, [
      { label: `Theme: ${theme()}`, icon: theme() === "dark" ? "moon" : "sun", run: () => setTheme(theme() === "system" ? "light" : theme() === "light" ? "dark" : "system") },
      { label: "Settings", icon: "settings", run: () => ws.tabs.reveal(`${o}/settings`) },
      { label: "Documentation", icon: "docs", run: () => ws.tabs.reveal(`${o}/docs`, "tab") },
      "-",
      {
        label: "Sign out",
        icon: "logout",
        run: async () => {
          trackFrontendEvent("sign_out");
          try {
            await api("/auth/logout", "POST");
            location.assign("/");
          } catch (e) {
            ws.toast({ tone: "bad", title: "Sign out failed", text: (e as Error).message });
          }
        },
      },
    ]);
  return (
    <aside class="sidebar" aria-label="Station">
      <div class="side-head">
        <a class="brand" href={o} onClick={(e) => (e.preventDefault(), ws.tabs.reveal(o, "tab"))} title="Space Station">
          <Mark size={18} />
          <span class="side-label">Space Station</span>
        </a>
        <button class="icon side-toggle" onClick={p.toggle} aria-label={p.collapsed ? "Expand sidebar" : "Collapse sidebar"} title={p.collapsed ? "Expand sidebar" : "Collapse sidebar"}>
          <Icon name="panel" />
        </button>
      </div>
      <button class="org-button" onClick={orgMenu} title="Switch organization">
        <span class="org-badge">{ws.org.slice(0, 1).toUpperCase()}</span>
        <span class="side-label">{ws.orgs().find((x) => x.id === ws.org)?.name || ws.org}</span>
        <Icon name="chevron" />
      </button>
      <button class="side-search" onClick={ws.palette} title="Search (⌘K)">
        <Icon name="search" />
        <span class="side-label">Search</span>
        <kbd>⌘K</kbd>
      </button>
      <nav class="side-nav">
        {item(o, "home", "New tab page")}
        {item(`${o}/windows`, "window", "Space Windows")}
        {item(`${o}/tables`, "table", "Tables")}
        {item(`${o}/notifications`, "bell", "Notifications")}
        {item(`${o}/settings`, "settings", "Settings")}
      </nav>
      <div class="side-scroll">
        <div class="side-section">
          <div class="side-caption">
            <span>Windows</span>
            <button class="icon" aria-label="New Space Window" title="New Space Window" onClick={() => ws.create("window")}>
              <Icon name="plus" />
            </button>
          </div>
          <For each={ws.windows.data() || []} fallback={<p class="side-empty">None yet</p>}>
            {(w) => {
              const path = `${o}/windows/${w.id}`;
              const tab = () => ws.tabs.state().tabs.find((t) => t.path === path || t.path === path + "/code");
              return item(path, "window", w.name, <i class={`live-dot ${tab() ? ws.tabs.marks[tab()!.id]?.live || "" : w.version ? "idle" : "none"}`} />);
            }}
          </For>
        </div>
        <div class="side-section">
          <div class="side-caption">
            <span>Tables</span>
            <button class="icon" aria-label="New table" title="New table" onClick={() => ws.create("table")}>
              <Icon name="plus" />
            </button>
          </div>
          <For each={ws.tables.data() || []} fallback={<p class="side-empty">None yet</p>}>
            {(t) => item(`${o}/tables/${t.id}`, "table", t.id, <span class="side-count">{count(t.records)}</span>)}
          </For>
        </div>
      </div>
      <div class="side-foot">
        {item(`${o}/docs`, "docs", "Documentation")}
        <button class="side-link" classList={{ on: p.dev }} onClick={ws.dev} title="Developer errors (⌥⇧D)">
          <Icon name="bug" />
          <span class="side-label">Developer errors</span>
          <kbd>⌥⇧D</kbd>
        </button>
        <button class="side-link account" onClick={account} title={`@${ws.me.id}`}>
          <span class="avatar">{ws.me.id.replace(/^(c|si):/, "").slice(0, 1).toUpperCase()}</span>
          <span class="side-label">
            @{ws.me.id}
            <small>{ws.me.kind}</small>
          </span>
          <Icon name="more" />
        </button>
      </div>
    </aside>
  );
}

function DevDrawer(p: { close: () => void }) {
  const ws = useWorkspace();
  const errors = resource(() => api<DevError[]>(`/orgs/${encodeURIComponent(ws.org)}/dev-errors`), 5000);
  return (
    <section class="drawer" aria-label="Developer errors">
      <div class="drawer-head">
        <Icon name="bug" />
        <strong>Developer errors</strong>
        <span class="muted">this browser's open windows, then the server's for {ws.org}; newest first</span>
        <span class="grow" />
        <button class="icon" aria-label="Refresh" onClick={() => errors.reload()}>
          <Icon name="reload" />
        </button>
        <button class="icon" aria-label="Close developer errors" onClick={p.close}>
          <Icon name="x" />
        </button>
      </div>
      <div class="drawer-body">
        <For each={ws.runErrors()}>
          {(r) => (
            <details class="dev-error">
              <summary>
                <code class="src">{r.error.source}</code>
                <span class="grow">
                  <b>{r.window}</b> {r.error.message}
                </span>
                <time class="muted">{new Date(r.at).toLocaleString()}</time>
              </summary>
              <pre>{JSON.stringify(r.error.detail, null, 2)}</pre>
            </details>
          )}
        </For>
        <Show when={errors.data()} fallback={<Loading error={errors.data.error} />}>
          {(rows) => (
            <Show when={rows().length || ws.runErrors().length} fallback={<p class="empty-line">No errors.</p>}>
              <For each={rows()}>
                {(e) => (
                  <details class="dev-error">
                    <summary>
                      <code class="src">{e.source}</code>
                      <span class="grow">{e.message}</span>
                      <time class="muted">{when(e.created_at)}</time>
                    </summary>
                    <pre>{JSON.stringify(e.detail, null, 2)}</pre>
                  </details>
                )}
              </For>
            </Show>
          )}
        </Show>
      </div>
    </section>
  );
}
