// The workspace's tabs on screen: the controller over lib/tabs (persisted per org, mirrored to the
// address bar and to browser back/forward), the strip, the panes that keep every tab mounted so a
// live window keeps running behind other tabs, and `Link`, which opens things the way Chrome does:
// click here, ⌘/Ctrl-click or middle-click behind, ⌘⇧-click in front, ⇧-click in a split.
import { createContext, createEffect, createMemo, createSignal, ErrorBoundary, For, on, onCleanup, Show, untrack, useContext, type Component, type JSX, type Accessor, type Resource } from "solid-js";
import { createStore } from "solid-js/store";
import * as T from "../lib/tabs";
import type { Me, Org, SpaceWindow, Table } from "../lib/api";
import type { RunError } from "../lib/runtime";
import { ErrorText, openMenu, TabContext, type MenuItem, type Mark as TabMark, type TabApi } from "./ui";
import { Icon, Mark } from "./icons";

export type Toast = { text: string; title?: string; tone?: "ok" | "bad" | "note"; action?: { label: string; run: () => void } };
export type Workspace = {
  org: string;
  me: Me;
  orgs: Accessor<Org[]>;
  tables: { data: Resource<Table[]>; reload: () => unknown };
  windows: { data: Resource<SpaceWindow[]>; reload: () => unknown };
  tabs: ReturnType<typeof createTabs>;
  /** A toast; one with a `key` already shown is not shown again (every window tab hears each notification). */
  toast: (t: Toast & { key?: string }) => void;
  palette: () => void;
  create: (kind: "table" | "window") => void;
  dev: () => void;
  /** A processor or renderer error of a window running in this browser, for the developer drawer. */
  report: (window: string, e: RunError) => void;
  runErrors: Accessor<{ window: string; error: RunError; at: number }[]>;
};
export const WorkspaceContext = createContext<Workspace>();
export const useWorkspace = () => useContext(WorkspaceContext)!;

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

export function createTabs(org: string, home: string, first: string) {
  const key = `ss-tabs:${org}`;
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(key);
  } catch {}
  const [state, set] = createSignal<T.Tabs>(T.restore(saved, org) ?? T.initial(first));
  const [marks, setMarks] = createStore<Record<string, TabMark>>({});
  const [revs, setRevs] = createStore<Record<string, number>>({});
  const [widths, setWidths] = createSignal<number[]>([1]);
  const ids = createMemo(() => state().tabs.map((t) => t.id), undefined, { equals: (a, b) => a.length === b.length && a.every((x, i) => x === b[i]) });
  const active = () => T.active(state());
  const tab = (id: string) => state().tabs.find((t) => t.id === id);
  /** Actions read the state untracked: one called from an effect must not subscribe that effect. */
  const now = () => untrack(state);

  /** Every change goes through here; a navigation of a tab also becomes a browser history entry. */
  const commit = (next: T.Tabs, push = false) => {
    if (next === now()) return;
    const a = T.active(next);
    if (push && a && a.path !== location.pathname) history.pushState({ tab: a.id, i: a.back.length } satisfies Nav, "", a.path);
    set(next);
  };

  createEffect(() => {
    const s = state();
    try {
      localStorage.setItem(key, T.save(s));
    } catch {}
    const a = T.active(s);
    if (!a) return;
    document.title = `${a.title} · Space Station`;
    const nav = history.state as Nav;
    if (location.pathname !== a.path || nav?.tab !== a.id || nav?.i !== a.back.length) history.replaceState({ tab: a.id, i: a.back.length } satisfies Nav, "", a.path);
  });
  // A tab put on screen has been seen.
  createEffect(on(() => state().panes, (panes) => panes.forEach((id) => marks[id]?.activity && setMarks(id, "activity", false))));
  // A new pane count starts from equal widths.
  createEffect(on(() => state().panes.length, (n) => setWidths(Array(n).fill(1))));
  // Browser back/forward: the entry names a tab. Another tab's entry brings that tab back as it is;
  // the focused tab's entry moves it through its own history to the entry's path.
  const pop = () => {
    const nav = history.state as Nav;
    const s = state();
    const t = nav?.tab ? tab(nav.tab) : undefined;
    if (t && T.active(s)?.id !== t.id) set(T.show(s, t.id));
    else if (t) set(T.seek(s, t.id, location.pathname, (nav!.i ?? 0) < t.back.length));
    else if (T.route(location.pathname)?.org === org) set(T.arrive(s, location.pathname));
  };
  addEventListener("popstate", pop);
  onCleanup(() => removeEventListener("popstate", pop));

  const self = {
    state,
    ids,
    tab,
    active,
    marks,
    widths,
    setWidths,
    home,
    rev: (id: string) => revs[id] || 0,
    mark: (id: string, m: TabMark) =>
      setMarks(id, (old) => ({ ...old, ...m, activity: m.activity === undefined ? old?.activity : m.activity && !now().panes.includes(id) })),
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
      commit(T.route(self.tab(id)?.path || "")?.kind === "home" && now().panes.length > 1 ? T.close(now(), [id], home) : T.unsplit(now(), pane));
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
    reload: (id: string) => setRevs(id, (r) => (r || 0) + 1),
    arrive: (path: string) => commit(T.arrive(now(), path)),
    /** Steps through the strip, wrapping. */
    cycle: (step: number) => {
      const s = now(),
        i = s.tabs.findIndex((t) => t.id === T.active(now())?.id);
      commit(T.show(s, s.tabs[(i + step + s.tabs.length) % s.tabs.length].id));
    },
  };
  return self;
}

/** A link inside the workspace; `reuse` switches to a tab already showing it instead of navigating. */
export function Link(p: { href: string; class?: string; classList?: Record<string, boolean>; title?: string; reuse?: boolean; children: JSX.Element }) {
  const ws = useWorkspace();
  const tab = useContext(TabContext);
  return (
    <a
      href={p.href}
      class={p.class}
      classList={p.classList}
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
    s = t.state(),
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
    { label: "Close", icon: "x", keys: "⌥W", run: () => t.close([id]) },
    { label: "Close other tabs", icon: "blank", disabled: s.tabs.length < 2, run: () => t.close(s.tabs.filter((x) => x.id !== id && !x.pinned).map((x) => x.id)) },
    { label: "Close tabs to the right", icon: "blank", disabled: i === s.tabs.length - 1, run: () => t.close(s.tabs.slice(i + 1).map((x) => x.id)) },
    { label: "Reopen closed tab", icon: "reload", keys: "⌥⇧T", disabled: !s.closed.length, run: t.reopen },
  ];
}

const [dragging, setDragging] = createSignal<string>();

export function TabStrip() {
  const ws = useWorkspace(),
    t = ws.tabs;
  const [over, setOver] = createSignal<{ id: string; after: boolean }>();
  return (
    <div class="strip" role="tablist" aria-label="Tabs" onDblClick={(e) => e.target === e.currentTarget && t.open(t.home, "tab", true)}>
      <For each={t.ids()}>
        {(id) => {
          const tab = () => t.tab(id)!;
          const kind = () => T.route(tab().path)?.kind;
          const pane = () => t.state().panes.indexOf(id);
          const selected = () => pane() === t.state().focus;
          return (
            <div
              role="tab"
              class="tab"
              classList={{
                selected: selected(),
                shown: pane() >= 0 && !selected(),
                pinned: !!tab().pinned,
                "drop-before": over()?.id === id && !over()!.after,
                "drop-after": over()?.id === id && over()!.after,
              }}
              aria-selected={selected()}
              tabIndex={selected() ? 0 : -1}
              onKeyDown={(e) => {
                const sib = e.key === "ArrowRight" ? e.currentTarget.nextElementSibling : e.key === "ArrowLeft" ? e.currentTarget.previousElementSibling : null;
                if (sib?.classList.contains("tab")) (e.preventDefault(), (sib as HTMLElement).focus());
                else if (e.key === "Enter" || e.key === " ") (e.preventDefault(), t.show(id));
                else if (e.key === "Delete") t.close([id]);
              }}
              title={tab().title}
              draggable="true"
              // On click, not mousedown: a drag must not first swap the tab into the focused pane.
              onClick={() => t.show(id)}
              onAuxClick={(e) => e.button === 1 && (e.preventDefault(), t.close([id]))}
              onContextMenu={(e) => openMenu(e, tabMenu(ws, id))}
              onDragStart={(e) => {
                e.dataTransfer!.setData("text/plain", location.origin + tab().path);
                e.dataTransfer!.effectAllowed = "move";
                setDragging(id);
              }}
              onDragEnd={() => {
                setDragging(undefined);
                setOver(undefined);
              }}
              onDragOver={(e) => {
                if (!dragging() || dragging() === id) return;
                e.preventDefault();
                const r = e.currentTarget.getBoundingClientRect();
                setOver({ id, after: e.clientX > r.left + r.width / 2 });
              }}
              onDragLeave={() => over()?.id === id && setOver(undefined)}
              onDrop={(e) => {
                e.preventDefault();
                const from = dragging(),
                  o = over();
                if (!from || !o) return;
                const rest = t.state().tabs.filter((x) => x.id !== from);
                t.move(from, rest.findIndex((x) => x.id === o.id) + (o.after ? 1 : 0));
                setOver(undefined);
              }}
            >
              <span class="tab-icon">
                <Show when={kind() === "home"} fallback={<Icon name={KIND_ICON[kind() || "home"]} />}>
                  <Mark size={13} />
                </Show>
                <Show when={t.marks[id]?.live}>
                  <i class={`live-dot ${t.marks[id]!.live}`} />
                </Show>
              </span>
              <Show when={!tab().pinned}>
                <span class="tab-title">{tab().title}</span>
                <Show when={t.marks[id]?.activity}>
                  <i class="activity" title="Updated while in the background" />
                </Show>
                <button
                  class="tab-close"
                  aria-label={`Close ${tab().title}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    t.close([id]);
                  }}
                >
                  <Icon name="x" size={14} />
                </button>
              </Show>
            </div>
          );
        }}
      </For>
      <button class="new-tab" aria-label="New tab" title="New tab (⌥T)" onClick={() => t.open(t.home, "tab", true)}>
        <Icon name="plus" />
      </button>
    </div>
  );
}

/** Crumbs for the pane toolbar: where this tab is, each step a link back up. */
function Crumbs(p: { path: string; title: string }) {
  const ws = useWorkspace();
  const steps = (): { label: string; href?: string }[] => {
    const r = T.route(p.path);
    const o = `/o/${ws.org}`;
    if (!r) return [{ label: "Not found" }];
    const name = ws.windows.data()?.find((w) => w.id === r.id)?.name || p.title;
    const up: Partial<Record<T.Kind, [string, string][]>> = {
      table: [["Tables", `${o}/tables`]],
      window: [["Space Windows", `${o}/windows`]],
      code: [["Space Windows", `${o}/windows`], [name, `${o}/windows/${r.id}`]],
      docs: r.id ? [["Docs", `${o}/docs`]] : [],
    };
    const here = { home: "New tab", table: r.id, window: name, code: "Code", docs: r.id ? p.title : "Docs" } as Partial<Record<T.Kind, string>>;
    return [...(up[r.kind] || []).map(([label, href]) => ({ label, href })), { label: here[r.kind] ?? T.titleOf(p.path) }];
  };
  return (
    <span class="crumbs">
      <span class="crumb-org">{ws.org}</span>
      <For each={steps()}>
        {(s) => (
          <>
            <span class="crumb-sep">/</span>
            <Show when={s.href} fallback={<span class="crumb-here">{s.label}</span>}>
              {(href) => (
                <Link href={href()} class="crumb">
                  {s.label}
                </Link>
              )}
            </Show>
          </>
        )}
      </For>
    </span>
  );
}

function Frame(p: { id: string; pages: Record<T.Kind, Component> }) {
  const ws = useWorkspace(),
    t = ws.tabs;
  const tab = () => t.tab(p.id)!;
  const pane = createMemo(() => t.state().panes.indexOf(p.id));
  const visible = createMemo(() => pane() >= 0);
  // By value: the strip changing around this tab must not look like this tab navigating.
  const r = createMemo(() => T.route(tab()?.path || ""), undefined, { equals: (a, b) => a?.kind === b?.kind && a?.id === b?.id && a?.org === b?.org });
  const [drop, setDrop] = createSignal<"here" | "beside">();
  const api: TabApi = {
    id: p.id,
    route: () => r()!,
    visible,
    go: (path) => t.go(p.id, path),
    open: (path, how = "tab") => (how === "here" ? t.go(p.id, path) : t.open(path, how)),
    title: (title) => t.rename(p.id, title),
    mark: (m) => t.mark(p.id, m),
  };
  // Window and code are one page, so the live view keeps running across the toggle.
  const key = createMemo(() => {
    const x = r();
    return x ? `${x.kind === "code" ? "window" : x.kind}:${x.id}:${t.rev(p.id)}` : `missing:${t.rev(p.id)}`;
  });
  let page!: HTMLDivElement;
  createEffect(on(key, () => page && (page.scrollTop = 0), { defer: true }));
  return (
    <TabContext.Provider value={api}>
    <section
      class="frame"
      classList={{ focused: visible() && pane() === t.state().focus }}
      hidden={!visible()}
      style={{ order: pane() * 2, flex: `${t.widths()[pane()] ?? 1} 1 0px` }}
      data-tab={p.id}
      onPointerDown={() => visible() && t.focus(pane())}
    >
      <div class="toolbar">
        <button class="icon" title="Back (⌥←)" disabled={!tab().back.length} onClick={() => t.walk(p.id, -1)}>
          <Icon name="back" />
        </button>
        <button class="icon" title="Forward (⌥→)" disabled={!tab().forward.length} onClick={() => t.walk(p.id, 1)}>
          <Icon name="forward" />
        </button>
        <button class="icon" title="Reload this tab" onClick={() => t.reload(p.id)}>
          <Icon name="reload" />
        </button>
        <div class="omnibox" onClick={(e) => !(e.target as HTMLElement).closest("a") && ws.palette()}>
          <Icon name={KIND_ICON[r()?.kind || "home"]} />
          <Crumbs path={tab().path} title={tab().title} />
          <kbd>⌘K</kbd>
        </div>
        <Show when={t.state().panes.length < T.MAX_PANES}>
          <button class="icon" title="Split view (⌥\)" onClick={() => t.open(t.home, "split")}>
            <Icon name="split" />
          </button>
        </Show>
        <Show when={t.state().panes.length > 1}>
          <button class="icon" title="Close this pane" onClick={() => t.unsplit(pane())}>
            <Icon name="x" />
          </button>
        </Show>
        <button class="icon" title="Tab menu" onClick={(e) => openMenu(e, tabMenu(ws, p.id))}>
          <Icon name="more" />
        </button>
      </div>
      <div class="page" ref={page}>
          <Show when={key()} keyed>
            {(_) => {
              const Page = r() ? p.pages[r()!.kind] : NotFound;
              return (
                <ErrorBoundary
                  fallback={(e, reset) => (
                    <div class="page-body">
                      <ErrorText message={e.message} />
                      <button onClick={reset}>Try again</button>
                    </div>
                  )}
                >
                  <Page />
                </ErrorBoundary>
              );
            }}
          </Show>
      </div>
      <Show when={dragging()}>
        <div
          class="drop"
          classList={{ [drop() || ""]: !!drop() }}
          onDragOver={(e) => {
            e.preventDefault();
            const rect = e.currentTarget.getBoundingClientRect();
            setDrop(e.clientX > rect.left + rect.width * 0.6 && t.state().panes.length < T.MAX_PANES ? "beside" : "here");
          }}
          onDragLeave={() => setDrop()}
          onDrop={(e) => {
            e.preventDefault();
            const id = dragging();
            if (id) t.place(id, pane(), drop() === "beside");
            setDrop();
            setDragging(undefined);
          }}
        >
          <span>{drop() === "beside" ? "Open in split view" : "Show here"}</span>
        </div>
      </Show>
    </section>
    </TabContext.Provider>
  );
}

function NotFound() {
  const ws = useWorkspace();
  return (
    <div class="page-body narrow">
      <h1>Page not found</h1>
      <p class="muted">Nothing lives at this address in {ws.org}.</p>
      <Link href={`/o/${ws.org}`}>Open a new tab page</Link>
    </div>
  );
}

export function Panes(p: { pages: Record<T.Kind, Component> }) {
  const t = useWorkspace().tabs;
  let box!: HTMLDivElement;
  const [resizing, setResizing] = createSignal(false);
  // Dragging the gap between panes i and i+1 trades width between just those two.
  const resize = (i: number) => (e: PointerEvent) => {
    const bar = e.currentTarget as HTMLElement;
    bar.setPointerCapture(e.pointerId);
    setResizing(true);
    const start = e.clientX,
      w = [...t.widths()],
      total = w.reduce((a, b) => a + b, 0),
      px = box.getBoundingClientRect().width / total,
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
  // Frames mount in the order tabs were opened and never move: CSS `order` places them, because
  // moving a node that holds an iframe reloads it — pinning or dragging a tab must not.
  const mounted = createMemo<string[]>((prev) => {
    const ids = t.ids();
    return [...prev.filter((id) => ids.includes(id)), ...ids.filter((id) => !prev.includes(id))];
  }, []);
  return (
    <div ref={box} class="panes" classList={{ split: t.state().panes.length > 1, resizing: resizing() }}>
      <For each={mounted()}>{(id) => <Frame id={id} pages={p.pages} />}</For>
      <For each={t.state().panes.slice(1)}>
        {(_, i) => <div class="resizer" role="separator" aria-orientation="vertical" style={{ order: i() * 2 + 1 }} onPointerDown={(e) => resize(i())(e)} onDblClick={() => t.setWidths(t.widths().map(() => 1))} />}
      </For>
    </div>
  );
}
