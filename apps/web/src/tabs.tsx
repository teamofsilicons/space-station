"use client";
// The workspace's tabs on screen: the controller over lib/tabs (persisted per actor, mirrored to the
// address bar and to browser back/forward), the strip, the panes that keep every tab mounted so a
// live window keeps running behind other tabs, and `Link`, which opens things the way Chrome does:
// click here, ⌘/Ctrl-click or middle-click behind, ⌘⇧-click in front, ⇧-click in a split.
import { Component, createContext, Fragment, memo, useContext, useEffect, useMemo, useRef, useState, type ComponentType, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/silicon-ui/button/button";
import { Tooltip } from "@/components/silicon-ui/tooltip/tooltip";
import * as T from "../lib/tabs";
import type { Me, SpaceWindow, Table } from "../lib/api";
import type { RunError } from "../lib/runtime";
import { ErrorText, openMenu, store, TabContext, useStore, type MenuItem, type Mark as TabMark, type Resource, type Store, type TabApi } from "./ui";
import { Icon, Mark } from "./icons";

export type Toast = { text: string; title?: string; tone?: "ok" | "bad" | "note"; action?: { label: string; run: () => void } };
/** The station a page works in; one object for the life of the workspace. */
export type Workspace = {
  actor: string;
  me: Me;
  tabs: Tabs;
  /** A toast; one with a `key` already shown is not shown again (every window tab hears each notification). */
  toast: (t: Toast & { key?: string }) => void;
  palette: () => void;
  create: (kind: "table" | "window") => void;
  dev: () => void;
  /** A processor or renderer error of a window running in this browser, for the developer drawer. */
  report: (window: string, e: RunError) => void;
};
export const WorkspaceContext = createContext<Workspace | undefined>(undefined);
export const useWorkspace = () => useContext(WorkspaceContext)!;
/** The actor's tables and windows, polled by the workspace; they change while the workspace stays. */
export const ListsContext = createContext<{ tables: Resource<Table[]>; windows: Resource<SpaceWindow[]> } | undefined>(undefined);
export const useLists = () => useContext(ListsContext)!;

export const KIND_ICON: Record<T.Kind, string> = {
  home: "home",
  tables: "table",
  table: "table",
  windows: "window",
  window: "window",
  code: "code",
  notifications: "bell",
  settings: "settings",
  docs: "docs",
};

type Nav = { tab?: string; i?: number } | null;
type State = { s: T.Tabs; marks: Record<string, TabMark>; revs: Record<string, number>; widths: number[] };
export type Tabs = ReturnType<typeof createTabs>;

/**
 * The tab controller, opened at `path`. Its actions read and write the store, never what a component
 * last rendered; making one touches nothing outside it, so the address bar follows from `attach`.
 */
export function createTabs(actor: string, home: string, first: string, context: string, path: string) {
  const key = `ss-tabs:${context}:${actor}`;
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(key);
  } catch {}
  const restored = T.restore(saved, actor) ?? T.initial(first);
  const s0 = T.route(path) ? T.arrive(restored, path) : restored;
  const state: Store<State> = store({ s: s0, marks: {}, revs: {}, widths: s0.panes.map(() => 1) });
  const now = () => state.get().s;
  const tab = (id: string) => now().tabs.find((t) => t.id === id);

  /** The address bar, the title and the saved strip follow the tabs. */
  const sync = (s: T.Tabs) => {
    try {
      localStorage.setItem(key, T.save(s));
    } catch {}
    const a = T.active(s);
    if (!a) return;
    document.title = `${a.title} · Space Station`;
    const nav = history.state as Nav;
    if (location.pathname !== a.path || nav?.tab !== a.id || nav?.i !== a.back.length) history.replaceState({ tab: a.id, i: a.back.length } satisfies Nav, "", a.path);
  };
  const set = (next: T.Tabs) => {
    state.set((old) => {
      const marks = { ...old.marks };
      // A tab put on screen has been seen.
      next.panes.forEach((id) => marks[id]?.activity && (marks[id] = { ...marks[id], activity: false }));
      // A new pane count starts from equal widths.
      const widths = next.panes.length === old.s.panes.length ? old.widths : next.panes.map(() => 1);
      return { ...old, s: next, marks, widths };
    });
    sync(next);
  };
  /** Every change goes through here; a navigation of a tab also becomes a browser history entry. */
  const commit = (next: T.Tabs, push = false) => {
    if (next === now()) return;
    const a = T.active(next);
    if (push && a && a.path !== location.pathname) history.pushState({ tab: a.id, i: a.back.length } satisfies Nav, "", a.path);
    set(next);
  };

  const self = {
    store: state,
    now,
    tab,
    active: () => T.active(now()),
    home,
    /** Browser back/forward: the entry names a tab. Another tab's entry brings that tab back as it is;
     * the focused tab's entry moves it through its own history to the entry's path. Returns the detach. */
    attach: () => {
      const pop = () => {
        const nav = history.state as Nav;
        const s = now();
        const t = nav?.tab ? tab(nav.tab) : undefined;
        if (t && T.active(s)?.id !== t.id) set(T.show(s, t.id));
        else if (t) set(T.seek(s, t.id, location.pathname, (nav!.i ?? 0) < t.back.length));
        else if (T.route(location.pathname)?.actor === actor) set(T.arrive(s, location.pathname));
      };
      addEventListener("popstate", pop);
      sync(now());
      return () => removeEventListener("popstate", pop);
    },
    setWidths: (widths: number[]) => state.set((old) => ({ ...old, widths })),
    mark: (id: string, m: TabMark) =>
      state.set((old) => {
        const was = old.marks[id];
        const next = { ...was, ...m, activity: m.activity === undefined ? was?.activity : m.activity && !old.s.panes.includes(id) };
        return was && was.live === next.live && was.activity === next.activity ? old : { ...old, marks: { ...old.marks, [id]: next } };
      }),
    /** Opens a path; `here` navigates the focused tab. */
    open: (path: string, how: T.How = "here", end = false) => commit(T.open(now(), path, how, end), how === "here"),
    /**
     * Switches to a tab already showing `path`; otherwise opens it as `how` says, where `here` means
     * a new tab unless the focused one is a new tab page — nothing already open is replaced.
     */
    reveal: (path: string, how: T.How = "here") => {
      const there = now().tabs.find((t) => t.path === path);
      if (there && how === "here") return commit(T.show(now(), there.id));
      self.open(path, how === "here" && T.route(T.active(now())?.path || "")?.kind !== "home" ? "tab" : how);
    },
    go: (id: string, path: string) => commit(T.open(T.show(now(), id), path, "here"), true),
    show: (id: string) => commit(T.show(now(), id)),
    focus: (pane: number) => pane !== now().focus && commit({ ...now(), focus: pane }),
    place: (id: string, pane: number, beside: boolean) => {
      const s = { ...now(), focus: pane };
      if (beside) return commit(T.split(s, id));
      if (s.panes.includes(id)) return commit(T.show(s, id));
      const panes = [...s.panes];
      panes[pane] = id;
      commit({ ...s, panes });
    },
    split: (id: string) => commit(T.split(now(), id)),
    /** Closes a pane; a new tab page in it goes too, since nothing was opened there yet. */
    unsplit: (pane: number) => {
      const id = now().panes[pane];
      commit(T.route(tab(id)?.path || "")?.kind === "home" && now().panes.length > 1 ? T.close(now(), [id], home) : T.unsplit(now(), pane));
    },
    close: (ids: string[]) => commit(T.close(now(), ids, home)),
    reopen: () => commit(T.reopen(now())),
    duplicate: (id: string) => commit(T.duplicate(now(), id)),
    move: (id: string, index: number) => commit(T.move(now(), id, index)),
    pin: (id: string) => commit(T.pin(now(), id)),
    walk: (id: string, steps: number) => commit(T.walk(T.show(now(), id), id, steps), true),
    /** Puts the focused tab at `path` without a history step: a redirect, not a navigation. */
    redirect: (path: string) => {
      const a = T.active(now());
      if (a) commit(T.seek(now(), a.id, path, true));
    },
    rename: (id: string, title: string) => commit(T.rename(now(), id, title)),
    reload: (id: string) => state.set((old) => ({ ...old, revs: { ...old.revs, [id]: (old.revs[id] || 0) + 1 } })),
    arrive: (path: string) => commit(T.arrive(now(), path)),
    /** Steps through the strip, wrapping. */
    cycle: (step: number) => {
      const s = now(),
        i = s.tabs.findIndex((t) => t.id === T.active(s)?.id);
      commit(T.show(s, s.tabs[(i + step + s.tabs.length) % s.tabs.length].id));
    },
  };
  return self;
}

/** The tabs as they are now; the component re-renders when they change. */
export const useTabs = () => useStore(useWorkspace().tabs.store);

/** A link inside the workspace; `reuse` switches to a tab already showing it instead of navigating. */
export function Link(p: { href: string; className?: string; title?: string; reuse?: boolean; children: ReactNode }) {
  const ws = useWorkspace();
  const tab = useContext(TabContext);
  return (
    <a
      href={p.href}
      className={p.className}
      title={p.title}
      onClick={(e) => {
        if (e.defaultPrevented || e.button !== 0 || e.altKey) return;
        e.preventDefault();
        const how: T.How = e.metaKey || e.ctrlKey ? (e.shiftKey ? "tab" : "background") : e.shiftKey ? "split" : "here";
        if (how === "here" && tab) tab.go(p.href);
        else if (p.reuse) ws.tabs.reveal(p.href, how);
        else ws.tabs.open(p.href, how);
      }}
      onAuxClick={(e) => {
        if (e.button !== 1) return;
        e.preventDefault();
        ws.tabs.open(p.href, "background");
      }}
    >
      {p.children}
    </a>
  );
}

/** What a tab's menu offers, in the strip and behind a pane's ⋯. */
export function tabMenu(ws: Workspace, id: string): MenuItem[] {
  const t = ws.tabs,
    s = t.now(),
    tab = t.tab(id)!,
    i = s.tabs.findIndex((x) => x.id === id),
    pane = s.panes.indexOf(id);
  return [
    { label: "New tab to the right", icon: "plus", run: () => t.open(t.home, "tab") },
    { label: "Reload", icon: "reload", run: () => t.reload(id) },
    { label: "Duplicate", icon: "duplicate", run: () => t.duplicate(id) },
    { label: tab.pinned ? "Unpin" : "Pin", icon: "pin", run: () => t.pin(id) },
    pane >= 0 && s.panes.length > 1
      ? { label: "Close split pane", icon: "split", run: () => t.unsplit(pane) }
      : { label: "Open in split view", icon: "split", disabled: pane >= 0, run: () => t.split(id) },
    { label: "Copy link", icon: "link", run: () => navigator.clipboard?.writeText(location.origin + tab.path) },
    { label: "Open in a browser tab", icon: "external", run: () => window.open(tab.path, "_blank") },
    "-",
    { label: "Close", icon: "x", run: () => t.close([id]) },
    { label: "Close other tabs", disabled: s.tabs.length < 2, run: () => t.close(s.tabs.filter((x) => x.id !== id && !x.pinned).map((x) => x.id)) },
    { label: "Close tabs to the right", disabled: i === s.tabs.length - 1, run: () => t.close(s.tabs.slice(i + 1).map((x) => x.id)) },
    { label: "Reopen closed tab", icon: "reload", disabled: !s.closed.length, run: t.reopen },
  ];
}

/** The tab being dragged, shared by the strip and the panes' drop targets. */
const dragging = store<string | undefined>(undefined);

export function TabStrip() {
  const ws = useWorkspace(),
    t = ws.tabs;
  const { s, marks } = useTabs();
  const drag = useStore(dragging);
  const [over, setOver] = useState<{ id: string; after: boolean }>();
  return (
    <div className="strip" role="tablist" aria-label="Tabs" onDoubleClick={(e) => e.target === e.currentTarget && t.open(t.home, "tab", true)}>
      {s.tabs.map((tab) => {
        const id = tab.id;
        const kind = T.route(tab.path)?.kind;
        const pane = s.panes.indexOf(id);
        const selected = pane === s.focus;
        const mark = marks[id];
        const cls = ["tab", selected && "selected", pane >= 0 && !selected && "shown", tab.pinned && "pinned", over?.id === id && (over.after ? "drop-after" : "drop-before")];
        return (
          <div
            key={id}
            role="tab"
            className={cls.filter(Boolean).join(" ")}
            aria-selected={selected}
            tabIndex={selected ? 0 : -1}
            onKeyDown={(e) => {
              const sib = e.key === "ArrowRight" ? e.currentTarget.nextElementSibling : e.key === "ArrowLeft" ? e.currentTarget.previousElementSibling : null;
              if (sib?.classList.contains("tab")) (e.preventDefault(), (sib as HTMLElement).focus());
              else if (e.key === "Enter" || e.key === " ") (e.preventDefault(), t.show(id));
              else if (e.key === "Delete") t.close([id]);
            }}
            title={tab.title}
            draggable
            // On click, not mousedown: a drag must not first swap the tab into the focused pane.
            onClick={() => t.show(id)}
            onAuxClick={(e) => e.button === 1 && (e.preventDefault(), t.close([id]))}
            onContextMenu={(e) => openMenu(e, tabMenu(ws, id))}
            onDragStart={(e) => {
              e.dataTransfer.setData("text/plain", location.origin + tab.path);
              e.dataTransfer.effectAllowed = "move";
              dragging.set(id);
            }}
            onDragEnd={() => {
              dragging.set(undefined);
              setOver(undefined);
            }}
            onDragOver={(e) => {
              if (!drag || drag === id) return;
              e.preventDefault();
              const r = e.currentTarget.getBoundingClientRect();
              setOver({ id, after: e.clientX > r.left + r.width / 2 });
            }}
            onDragLeave={() => over?.id === id && setOver(undefined)}
            onDrop={(e) => {
              e.preventDefault();
              if (!drag || !over) return;
              const rest = t.now().tabs.filter((x) => x.id !== drag);
              t.move(drag, rest.findIndex((x) => x.id === over.id) + (over.after ? 1 : 0));
              setOver(undefined);
            }}
          >
            <span className="tab-icon">
              {kind === "home" ? <Mark size={13} /> : <Icon name={KIND_ICON[kind || "home"]} />}
              {mark?.live && <i className={`live-dot ${mark.live}`} />}
            </span>
            {!tab.pinned && (
              <>
                <span className="tab-title">{tab.title}</span>
                {mark?.activity && <i className="activity" title="Updated while in the background" />}
                <button
                  className="tab-close"
                  aria-label={`Close ${tab.title}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    t.close([id]);
                  }}
                >
                  <Icon name="x" size={14} />
                </button>
              </>
            )}
          </div>
        );
      })}
      <Tooltip content="New tab (⌥T)" side="bottom">
        <button className="new-tab" aria-label="New tab" onClick={() => t.open(t.home, "tab", true)}>
          <Icon name="plus" />
        </button>
      </Tooltip>
    </div>
  );
}

/** Crumbs for the pane toolbar: where this tab is, each step a link back up. */
function Crumbs(p: { path: string; title: string }) {
  const ws = useWorkspace();
  const windows = useContext(ListsContext)?.windows.data;
  const o = `/a/${ws.actor}`;
  const r = T.route(p.path);
  let steps: { label: string; href?: string }[] = [{ label: "Not found" }];
  if (r) {
    const name = windows?.find((w) => w.id === r.id)?.name || p.title;
    const up: Partial<Record<T.Kind, [string, string][]>> = {
      table: [["Tables", `${o}/tables`]],
      window: [["Space Windows", `${o}/windows`]],
      code: [["Space Windows", `${o}/windows`], [name, `${o}/windows/${r.id}`]],
      docs: r.id ? [["Docs", `${o}/docs`]] : [],
    };
    const here = { home: "New tab", table: r.id, window: name, code: "Code", docs: r.id ? p.title : "Docs" } as Partial<Record<T.Kind, string>>;
    steps = [...(up[r.kind] || []).map(([label, href]) => ({ label, href })), { label: here[r.kind] ?? T.titleOf(p.path) }];
  }
  return (
    <span className="crumbs">
      <span className="crumb-actor">{ws.me.id}</span>
      {steps.map((s, i) => (
        <Fragment key={i}>
          <span className="crumb-sep">/</span>
          {s.href ? (
            <Link href={s.href} className="crumb">
              {s.label}
            </Link>
          ) : (
            <span className="crumb-here">{s.label}</span>
          )}
        </Fragment>
      ))}
    </span>
  );
}

/** A page that throws shows its error in its own tab, with a way to try again. */
class Boundary extends Component<{ children: ReactNode }, { error?: Error }> {
  state: { error?: Error } = {};
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="page-body">
        <ErrorText message={this.state.error.message} />
        <Button variant="secondary" onClick={() => this.setState({ error: undefined })}>
          Try again
        </Button>
      </div>
    );
  }
}

/** The page itself only re-renders for its own reasons, not because the strip changed around it. */
const PageHost = memo(function PageHost(p: { page: ComponentType }) {
  return <p.page />;
});

function ToolButton(p: { label: string; icon: string; disabled?: boolean; onClick: (e: React.MouseEvent<HTMLButtonElement>) => void }) {
  return (
    <Tooltip content={p.label} side="bottom">
      <Button variant="ghost" size="sm" className="icon" aria-label={p.label} disabled={p.disabled} onClick={p.onClick}>
        <Icon name={p.icon} />
      </Button>
    </Tooltip>
  );
}

const Frame = memo(function Frame(p: { id: string; pages: Record<T.Kind, ComponentType> }) {
  const ws = useWorkspace(),
    t = ws.tabs;
  const { s, widths, revs } = useTabs();
  const drag = useStore(dragging);
  const tab = s.tabs.find((x) => x.id === p.id)!;
  const pane = s.panes.indexOf(p.id);
  const visible = pane >= 0;
  const r = T.route(tab.path);
  const [drop, setDrop] = useState<"here" | "beside">();
  // By value: the strip changing around this tab must not look like this tab navigating.
  const api = useMemo<TabApi>(
    () => ({
      id: p.id,
      route: r!,
      visible,
      go: (path) => t.go(p.id, path),
      open: (path, how = "tab") => (how === "here" ? t.go(p.id, path) : t.open(path, how)),
      title: (title) => t.rename(p.id, title),
      mark: (m) => t.mark(p.id, m),
    }),
    [p.id, r?.kind, r?.id, r?.actor, visible], // eslint-disable-line react-hooks/exhaustive-deps
  );
  // Window and code are one page, so the live view keeps running across the toggle.
  const rev = revs[p.id] || 0;
  const key = r ? `${r.kind === "code" ? "window" : r.kind}:${r.id}:${rev}` : `missing:${rev}`;
  const page = useRef<HTMLDivElement>(null);
  const first = useRef(true);
  useEffect(() => {
    if (first.current) first.current = false;
    else if (page.current) page.current.scrollTop = 0;
  }, [key]);
  return (
    <TabContext.Provider value={api}>
      <section
        className={`frame${visible && pane === s.focus ? " focused" : ""}`}
        hidden={!visible}
        style={{ order: pane * 2, flex: `${widths[pane] ?? 1} 1 0px` }}
        data-tab={p.id}
        onPointerDown={() => visible && t.focus(pane)}
      >
        <div className="toolbar">
          <ToolButton label="Back (⌥←)" icon="back" disabled={!tab.back.length} onClick={() => t.walk(p.id, -1)} />
          <ToolButton label="Forward (⌥→)" icon="forward" disabled={!tab.forward.length} onClick={() => t.walk(p.id, 1)} />
          <ToolButton label="Reload this tab" icon="reload" onClick={() => t.reload(p.id)} />
          <div className="omnibox" onClick={(e) => !(e.target as HTMLElement).closest("a") && ws.palette()}>
            <Icon name={KIND_ICON[r?.kind || "home"]} />
            <Crumbs path={tab.path} title={tab.title} />
            <kbd>⌘K</kbd>
          </div>
          {s.panes.length < T.MAX_PANES && <ToolButton label="Split view (⌥\)" icon="split" onClick={() => t.open(t.home, "split")} />}
          {s.panes.length > 1 && <ToolButton label="Close this pane" icon="x" onClick={() => t.unsplit(pane)} />}
          <ToolButton label="Tab menu" icon="more" onClick={(e) => openMenu(e, tabMenu(ws, p.id))} />
        </div>
        <div className="page" ref={page}>
          <Boundary key={key}>
            <PageHost page={r ? p.pages[r.kind] : NotFound} />
          </Boundary>
        </div>
        {drag && (
          <div
            className={`drop ${drop || ""}`}
            onDragOver={(e) => {
              e.preventDefault();
              const rect = e.currentTarget.getBoundingClientRect();
              setDrop(e.clientX > rect.left + rect.width * 0.6 && t.now().panes.length < T.MAX_PANES ? "beside" : "here");
            }}
            onDragLeave={() => setDrop(undefined)}
            onDrop={(e) => {
              e.preventDefault();
              t.place(drag, pane, drop === "beside");
              setDrop(undefined);
              dragging.set(undefined);
            }}
          >
            <span>{drop === "beside" ? "Open in split view" : "Show here"}</span>
          </div>
        )}
      </section>
    </TabContext.Provider>
  );
});

function NotFound() {
  const ws = useWorkspace();
  return (
    <div className="page-body narrow">
      <h1>Page not found</h1>
      <p className="muted">Nothing lives at this address for {ws.me.id}.</p>
      <Link href={`/a/${ws.actor}`}>Open a new tab page</Link>
    </div>
  );
}

export function Panes(p: { pages: Record<T.Kind, ComponentType> }) {
  const t = useWorkspace().tabs;
  const { s, widths } = useTabs();
  const box = useRef<HTMLDivElement>(null);
  const [resizing, setResizing] = useState(false);
  // Frames mount in the order tabs were opened and never move: CSS `order` places them, because
  // moving a node that holds an iframe reloads it — pinning or dragging a tab must not.
  const mounted = useRef<string[]>([]);
  const ids = s.tabs.map((x) => x.id);
  mounted.current = [...mounted.current.filter((id) => ids.includes(id)), ...ids.filter((id) => !mounted.current.includes(id))];
  // Dragging the gap between panes i and i+1 trades width between just those two.
  const resize = (i: number) => (e: React.PointerEvent<HTMLDivElement>) => {
    const bar = e.currentTarget;
    bar.setPointerCapture(e.pointerId);
    setResizing(true);
    const start = e.clientX,
      w = [...widths],
      total = w.reduce((a, b) => a + b, 0),
      px = box.current!.getBoundingClientRect().width / total,
      pair = w[i] + w[i + 1];
    const move = (m: PointerEvent) => {
      const next = [...w];
      next[i] = Math.max(pair * 0.15, Math.min(pair * 0.85, w[i] + (m.clientX - start) / px));
      next[i + 1] = pair - next[i];
      t.setWidths(next);
    };
    const up = () => {
      setResizing(false);
      bar.removeEventListener("pointermove", move);
      bar.removeEventListener("pointerup", up);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up);
  };
  return (
    <div ref={box} className={["panes", s.panes.length > 1 && "split", resizing && "resizing"].filter(Boolean).join(" ")}>
      {mounted.current.map((id) => (
        <Frame key={id} id={id} pages={p.pages} />
      ))}
      {s.panes.slice(1).map((_, i) => (
        <div key={`resizer-${i}`} className="resizer" role="separator" aria-orientation="vertical" style={{ order: i * 2 + 1 }} onPointerDown={resize(i)} onDoubleClick={() => t.setWidths(widths.map(() => 1))} />
      ))}
    </div>
  );
}
