"use client";
// The signed-in station: a sidebar of the actor's windows and tables, a strip of tabs over up to
// three panes, the ⌘K palette, a developer-errors drawer (⌥⇧D), toasts, and the keyboard.
import { useEffect, useMemo, useRef, useState, type ComponentType, type MouseEvent, type ReactNode } from "react";
import { Avatar } from "@/components/silicon-ui/avatar/avatar";
import { Button } from "@/components/silicon-ui/button/button";
import { JsonViewer } from "@/components/silicon-ui/json-viewer/json-viewer";
import { ToastStack, ToastStackProvider, useToastStack } from "@/components/silicon-ui/toast-stack/toast-stack";
import { Tooltip } from "@/components/silicon-ui/tooltip/tooltip";
import { UserMenu } from "@/components/silicon-ui/user-menu/user-menu";
import { api, type DevError, type Me, type SavedContext, type SpaceWindow, type Table } from "../lib/api";
import type { RunError } from "../lib/runtime";
import * as T from "../lib/tabs";
import { LoginButtons } from "./login";
import { createTabs, Link, ListsContext, Panes, TabStrip, useLists, useTabs, useWorkspace, WorkspaceContext, type Workspace as W } from "./tabs";
import { closeMenu, count, Loading, MenuHost, Modal, openMenu, useResource, when, type MenuItem } from "./ui";
import { Icon, Mark } from "./icons";
import { Palette } from "./palette";
import { Home } from "./home";
import { NewTable, Tables, TableView } from "./tables";
import { NewWindow, Windows, WindowView } from "./windows";
import { Notifications } from "./notifications";
import { Settings } from "./settings";
import { DocsPage } from "./docs";
import { setTheme, useTheme } from "./theme";

const PAGES: Record<T.Kind, ComponentType> = {
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

type RunErrors = { window: string; error: RunError; at: number; n: number }[];
let reports = 0;

export function Workspace(p: { me: Me; path: string }) {
  return (
    <ToastStackProvider duration={7000} limit={4}>
      <Station me={p.me} path={p.path} />
      <ToastStack label="Station notifications" hotkey={false} />
    </ToastStackProvider>
  );
}

function Station(p: { me: Me; path: string }) {
  const actor = p.me.uuid,
    home = `/a/${actor}`;
  const tables = useResource(() => api<Table[]>(`/tables`), 15000);
  const windows = useResource(() => api<SpaceWindow[]>(`/windows`), 30000);
  const [palette, setPalette] = useState(false);
  const [creating, setCreating] = useState<"table" | "window">();
  const [dev, setDev] = useState(false);
  const [runErrors, setRunErrors] = useState<RunErrors>([]);
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem("ss-sidebar") === "collapsed");
  const [nav, setNav] = useState(false);
  const toasts = useToastStack();
  const toast = useRef(toasts.toast);
  toast.current = toasts.toast;
  const paletteOpen = useRef(palette);
  paletteOpen.current = palette;
  // A first visit opens Space Windows once tables exist, Tables before that — once, never again.
  const [fresh] = useState(() => {
    const arrived = T.route(p.path);
    return !localStorage.getItem(`ss-tabs:${p.me.context_id}:${actor}`) && (!arrived || arrived.kind === "home");
  });
  const [ws] = useState<W>(() => {
    const arrived = T.route(p.path);
    const tabs = createTabs(actor, home, arrived && arrived.kind !== "home" ? p.path : `${home}/windows`, p.me.context_id, p.path);
    const seen = new Set<string>();
    return {
      actor,
      me: p.me,
      tabs,
      toast: (t) => {
        if (t.key && seen.has(t.key)) return;
        if (t.key) seen.add(t.key);
        toast.current({
          type: t.tone === "bad" ? "error" : t.tone === "ok" ? "success" : "info",
          title: t.title || t.text,
          description: t.title ? t.text : undefined,
          action: t.action && { label: t.action.label, onClick: () => t.action!.run() },
        });
      },
      palette: () => {
        closeMenu();
        setPalette(true);
      },
      create: setCreating,
      dev: () => setDev((d) => !d),
      report: (window, error) => setRunErrors((old) => [{ window, error, at: Date.now(), n: ++reports }, ...old].slice(0, 50)),
    };
  });
  const tabs = ws.tabs;
  const lists = useMemo(() => ({ tables, windows }), [tables, windows]);
  useEffect(() => tabs.attach(), [tabs]);
  const decided = useRef(!fresh);
  useEffect(() => {
    if (decided.current || !tables.data) return;
    decided.current = true;
    if (!tables.data.length && tabs.active()?.path === `${home}/windows`) tabs.redirect(`${home}/tables`);
  }, [tables.data, tabs, home]);

  useEffect(() => {
    const keys = (e: KeyboardEvent) => {
      const t = tabs.now(),
        cur = tabs.active();
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.code === "KeyK") {
        e.preventDefault();
        if (paletteOpen.current) setPalette(false);
        else ws.palette();
        return;
      }
      if (!e.altKey || e.metaKey || e.ctrlKey) return;
      // ⌥ also types characters ([ ] { } | on many layouts) and moves by word: leave fields and dialogs alone.
      if ((e.target as Element).closest?.("input, textarea, select, [contenteditable]") || document.querySelector('[role="dialog"][data-state="open"]')) return;
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
    // A popover closes on any click outside it.
    const pops = (e: PointerEvent) => document.querySelectorAll("details.pop[open]").forEach((d) => !d.contains(e.target as Node) && d.removeAttribute("open"));
    addEventListener("keydown", keys);
    addEventListener("blur", blur);
    addEventListener("pointerdown", pops);
    return () => {
      removeEventListener("keydown", keys);
      removeEventListener("blur", blur);
      removeEventListener("pointerdown", pops);
    };
  }, [tabs, ws, home]);

  const toggle = () => {
    setCollapsed(!collapsed);
    localStorage.setItem("ss-sidebar", collapsed ? "open" : "collapsed");
  };
  return (
    <WorkspaceContext.Provider value={ws}>
      <ListsContext.Provider value={lists}>
        <div
          className={["station", collapsed && "collapsed", nav && "nav-open"].filter(Boolean).join(" ")}
          onClick={(e) => nav && (e.target as HTMLElement).closest(".sidebar a, .main") && setNav(false)}
        >
          <Sidebar collapsed={collapsed} toggle={toggle} dev={dev} />
          <div className="main">
            <div className="strip-row">
              <Button variant="ghost" size="sm" className="icon strip-menu" aria-label="Open the sidebar" onClick={(e) => (e.stopPropagation(), setNav(true))}>
                <Icon name="panel" />
              </Button>
              <TabStrip />
              <Tooltip content="Search tabs and everything else (⌘K)" side="bottom">
                <Button variant="ghost" size="sm" className="icon strip-search" aria-label="Search" onClick={ws.palette}>
                  <Icon name="search" />
                </Button>
              </Tooltip>
            </div>
            <Panes pages={PAGES} />
            {dev && <DevDrawer close={() => setDev(false)} runErrors={runErrors} />}
          </div>
        </div>
        {palette && <Palette close={() => setPalette(false)} />}
        {creating === "table" && <NewTable close={() => setCreating(undefined)} />}
        {creating === "window" && <NewWindow close={() => setCreating(undefined)} />}
        <MenuHost />
      </ListsContext.Provider>
    </WorkspaceContext.Provider>
  );
}

function Sidebar(p: { collapsed: boolean; toggle: () => void; dev: boolean }) {
  const ws = useWorkspace();
  const { tables, windows } = useLists();
  const { s, marks } = useTabs();
  const theme = useTheme();
  const o = `/a/${ws.actor}`;
  const here = T.active(s)?.path;
  const [addingAccount, setAddingAccount] = useState(false);
  const item = (path: string, icon: string, label: string, extra?: ReactNode) => (
    <Link key={path} href={path} reuse className={["side-link", here === path && "on", here !== path && s.tabs.some((t) => t.path === path) && "open"].filter(Boolean).join(" ")} title={label}>
      <Icon name={icon} />
      <span className="side-label">{label}</span>
      {extra}
    </Link>
  );
  const accountMenu = async (e: MouseEvent<HTMLButtonElement>) => {
    const anchor = e.currentTarget;
    try {
      const rows = await api<SavedContext[]>("/auth/contexts");
      openMenu(
        e,
        [
          ...rows.map(
            (x): MenuItem => ({
              label: x.actor,
              icon: x.selected ? "check" : "blank",
              run: async () => {
                if (x.selected) return;
                try {
                  await api("/auth/context", "POST", { context_id: x.context_id });
                  location.assign(`/a/${encodeURIComponent(x.uuid)}`);
                } catch (error) {
                  ws.toast({ tone: "bad", title: "Account switch failed", text: (error as Error).message });
                }
              },
            }),
          ),
          "-",
          { label: "Add account", icon: "plus", run: () => setAddingAccount(true) },
        ],
        anchor,
      );
    } catch (error) {
      ws.toast({ tone: "bad", title: "Accounts unavailable", text: (error as Error).message });
    }
  };
  const signOut = async () => {
    try {
      await api("/auth/logout", "POST");
      location.assign("/");
    } catch (e) {
      ws.toast({ tone: "bad", title: "Sign out failed", text: (e as Error).message });
    }
  };
  return (
    <>
      <aside className="sidebar" aria-label="Station">
        <div className="side-head">
          <a className="brand" href={o} onClick={(e) => (e.preventDefault(), ws.tabs.reveal(o, "tab"))} title="Space Station">
            <Mark size={18} />
            <span className="side-label">Space Station</span>
          </a>
          <Button variant="ghost" size="sm" className="icon side-toggle" onClick={p.toggle} aria-label={p.collapsed ? "Expand sidebar" : "Collapse sidebar"} title={p.collapsed ? "Expand sidebar" : "Collapse sidebar"}>
            <Icon name="panel" />
          </Button>
        </div>
        <button className="actor-button" onClick={accountMenu} title="Switch account">
          <Avatar name={ws.me.id.replace(/^(c|si):/, "")} size="sm" />
          <span className="side-label">{ws.me.id}</span>
          <Icon name="chevron" />
        </button>
        <button className="side-search" onClick={ws.palette} title="Search (⌘K)">
          <Icon name="search" />
          <span className="side-label">Search</span>
          <kbd>⌘K</kbd>
        </button>
        <nav className="side-nav">
          {item(o, "home", "New tab page")}
          {item(`${o}/windows`, "window", "Space Windows")}
          {item(`${o}/tables`, "table", "Tables")}
          {item(`${o}/notifications`, "bell", "Notifications")}
          {item(`${o}/settings`, "settings", "Settings")}
        </nav>
        <div className="side-scroll">
          <div className="side-section">
            <div className="side-caption">
              <span>Windows</span>
              <Button variant="ghost" size="sm" className="icon" aria-label="New Space Window" title="New Space Window" onClick={() => ws.create("window")}>
                <Icon name="plus" />
              </Button>
            </div>
            {windows.data?.length ? (
              windows.data.map((w) => {
                const path = `${o}/windows/${w.id}`;
                const tab = s.tabs.find((t) => t.path === path || t.path === path + "/code");
                return item(path, "window", w.name, <i className={`live-dot ${tab ? marks[tab.id]?.live || "" : w.version ? "idle" : "none"}`} />);
              })
            ) : (
              <p className="side-empty">None yet</p>
            )}
          </div>
          <div className="side-section">
            <div className="side-caption">
              <span>Tables</span>
              <Button variant="ghost" size="sm" className="icon" aria-label="New table" title="New table" onClick={() => ws.create("table")}>
                <Icon name="plus" />
              </Button>
            </div>
            {tables.data?.length ? (
              tables.data.map((t) => item(`${o}/tables/${t.id}`, "table", t.id, <span className="side-count">{count(t.records)}</span>))
            ) : (
              <p className="side-empty">None yet</p>
            )}
          </div>
        </div>
        <div className="side-foot">
          {item(`${o}/docs`, "docs", "Documentation")}
          <button className={`side-link${p.dev ? " on" : ""}`} onClick={ws.dev} title="Developer errors (⌥⇧D)">
            <Icon name="bug" />
            <span className="side-label">Developer errors</span>
            <kbd>⌥⇧D</kbd>
          </button>
          <div className="side-account">
            <UserMenu
              user={{ name: ws.me.id, email: `${ws.me.kind} · ${ws.me.app}` }}
              showName={!p.collapsed}
              align="start"
              theme={theme}
              onThemeChange={setTheme}
              items={[
                { label: "Settings", icon: <Icon name="settings" />, onSelect: () => ws.tabs.reveal(`${o}/settings`) },
                { label: "Documentation", icon: <Icon name="docs" />, onSelect: () => ws.tabs.reveal(`${o}/docs`, "tab") },
              ]}
              onSignOut={signOut}
            />
          </div>
        </div>
      </aside>
      {addingAccount && (
        <Modal title="Add account" description="Sign in as a carbon or silicon. Your saved accounts stay available." close={() => setAddingAccount(false)}>
          <LoginButtons next="/" />
        </Modal>
      )}
    </>
  );
}

function DevError(p: { source: string; title: ReactNode; at: string; detail: unknown }) {
  return (
    <details className="dev-error">
      <summary>
        <code className="src">{p.source}</code>
        <span className="grow">{p.title}</span>
        <time className="muted">{p.at}</time>
      </summary>
      <JsonViewer data={p.detail} rootName="detail" defaultExpandDepth={2} copyable />
    </details>
  );
}

function DevDrawer(p: { close: () => void; runErrors: RunErrors }) {
  const ws = useWorkspace();
  const errors = useResource(() => api<DevError[]>(`/dev-errors`), 5000);
  return (
    <section className="drawer" aria-label="Developer errors">
      <div className="drawer-head">
        <Icon name="bug" />
        <strong>Developer errors</strong>
        <span className="muted">this browser&apos;s open windows, then the server&apos;s for {ws.me.id}; newest first</span>
        <span className="grow" />
        <Button variant="ghost" size="sm" className="icon" aria-label="Refresh" onClick={() => errors.reload()}>
          <Icon name="reload" />
        </Button>
        <Button variant="ghost" size="sm" className="icon" aria-label="Close developer errors" onClick={p.close}>
          <Icon name="x" />
        </Button>
      </div>
      <div className="drawer-body">
        {p.runErrors.map((r) => (
          <DevError key={r.n} source={r.error.source} title={<><b>{r.window}</b> {r.error.message}</>} at={new Date(r.at).toLocaleString()} detail={r.error.detail} />
        ))}
        {!errors.data ? (
          <Loading error={errors.error} />
        ) : errors.data.length || p.runErrors.length ? (
          errors.data.map((e) => <DevError key={e.id} source={e.source} title={e.message} at={when(e.created_at)} detail={e.detail} />)
        ) : (
          <p className="empty-line">No errors.</p>
        )}
      </div>
    </section>
  );
}
