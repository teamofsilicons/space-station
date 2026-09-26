// The workspace's tabs, kept the way Chrome keeps them: an ordered strip (pinned first), one to
// three panes side by side that each show one tab, a history per tab, and the last closed tabs.
// Pure — the shell holds the state and persists it per org; these functions say what comes next.

export type Kind = "home" | "tables" | "table" | "windows" | "window" | "code" | "notifications" | "settings" | "docs";
export type Route = { kind: Kind; org: string; id: string };
export type Tab = { id: string; path: string; title: string; pinned?: boolean; opener?: string; back: string[]; forward: string[] };
export type Tabs = { tabs: Tab[]; panes: string[]; focus: number; closed: Tab[] };
/** here: the focused tab navigates; tab: a new tab in front; background: a new tab behind; split: a new pane. */
export type How = "here" | "tab" | "background" | "split";

export const MAX_PANES = 3;

const ROUTES: [RegExp, Kind][] = [
  [/^\/o\/([^/]+)$/, "home"],
  [/^\/o\/([^/]+)\/tables$/, "tables"],
  [/^\/o\/([^/]+)\/tables\/([a-z0-9]{1,50})$/, "table"],
  [/^\/o\/([^/]+)\/windows$/, "windows"],
  [/^\/o\/([^/]+)\/windows\/([^/]+)$/, "window"],
  [/^\/o\/([^/]+)\/windows\/([^/]+)\/code$/, "code"],
  [/^\/o\/([^/]+)\/notifications$/, "notifications"],
  [/^\/o\/([^/]+)\/settings$/, "settings"],
  [/^\/o\/([^/]+)\/docs(?:\/([a-z0-9-]+))?$/, "docs"],
];

/** What a workspace path shows; `null` for anything that is not an org page. */
export function route(path: string): Route | null {
  const clean = path.split(/[?#]/)[0].replace(/\/+$/, "") || "/";
  for (const [re, kind] of ROUTES) {
    const m = re.exec(clean);
    if (m) return { kind, org: decodeURIComponent(m[1]), id: m[2] ? decodeURIComponent(m[2]) : "" };
  }
  return null;
}

/** The title a tab carries until its page names it. */
export function titleOf(path: string): string {
  const r = route(path);
  if (!r) return "Not found";
  return {
    home: "New tab",
    tables: "Tables",
    table: r.id,
    windows: "Space Windows",
    window: "Space Window",
    code: "Code",
    notifications: "Notifications",
    settings: "Settings",
    docs: "Docs",
  }[r.kind];
}

let seq = 0;
const newTab = (path: string, opener?: string): Tab => ({
  id: `t${Date.now().toString(36)}${(seq++).toString(36)}`,
  path,
  title: titleOf(path),
  opener,
  back: [],
  forward: [],
});

export const active = (s: Tabs) => s.tabs.find((t) => t.id === s.panes[s.focus]);
const at = (s: Tabs, id: string) => s.tabs.findIndex((t) => t.id === id);
/** Pinned tabs stay ahead of the rest, whatever moved. */
const ordered = (tabs: Tab[]) => [...tabs.filter((t) => t.pinned), ...tabs.filter((t) => !t.pinned)];

export function initial(path: string): Tabs {
  const t = newTab(path);
  return { tabs: [t], panes: [t.id], focus: 0, closed: [] };
}

/** Opens `path` the way a click asked; `end` puts a new tab last (the + button) instead of after its opener. */
export function open(s: Tabs, path: string, how: How, end = false): Tabs {
  const cur = active(s);
  if (how === "here" && cur) {
    if (cur.path === path) return s;
    const moved = { ...cur, path, title: titleOf(path), back: [...cur.back, cur.path], forward: [] };
    return { ...s, tabs: s.tabs.map((t) => (t === cur ? moved : t)) };
  }
  const t = newTab(path, end ? undefined : cur?.id);
  let i = end || !cur ? s.tabs.length : at(s, cur.id) + 1;
  // Chrome's order: tabs opened from one tab line up behind it, oldest first.
  while (!end && cur && s.tabs[i]?.opener === cur.id) i++;
  const tabs = ordered([...s.tabs.slice(0, i), t, ...s.tabs.slice(i)]);
  if (how === "background") return { ...s, tabs };
  if (how === "split") return split({ ...s, tabs }, t.id);
  const panes = [...s.panes];
  panes[s.focus] = t.id;
  return { ...s, tabs, panes };
}

/** Shows a tab: focuses its pane if it has one, else puts it in the focused pane. */
export function show(s: Tabs, id: string): Tabs {
  if (at(s, id) < 0) return s;
  const pane = s.panes.indexOf(id);
  if (pane >= 0) return pane === s.focus ? s : { ...s, focus: pane };
  const panes = [...s.panes];
  panes[s.focus] = id;
  return { ...s, panes };
}

/** Shows a tab in a new pane right of the focused one; with every pane taken, in that right pane. */
export function split(s: Tabs, id: string): Tabs {
  const panes = s.panes.filter((p) => p !== id);
  const focus = Math.min(s.focus, panes.length - 1);
  if (panes.length >= MAX_PANES) {
    const right = Math.min(focus + 1, panes.length - 1);
    panes[right] = id;
    return { ...s, panes, focus: right };
  }
  panes.splice(focus + 1, 0, id);
  return { ...s, panes, focus: focus + 1 };
}

/** Closes a pane; its tab stays in the strip. */
export function unsplit(s: Tabs, pane: number): Tabs {
  if (s.panes.length < 2) return s;
  const panes = s.panes.filter((_, i) => i !== pane);
  return { ...s, panes, focus: Math.min(s.focus > pane ? s.focus - 1 : s.focus, panes.length - 1) };
}

/** Closes tabs; a pane that showed one takes its right neighbour (else left), or goes when others remain. */
export function close(s: Tabs, ids: string[], home: string): Tabs {
  const gone = new Set(ids);
  const closed = [...s.tabs.filter((t) => gone.has(t.id)).reverse(), ...s.closed].slice(0, 10);
  const tabs = s.tabs.filter((t) => !gone.has(t.id));
  if (!tabs.length) return { ...initial(home), closed };
  let panes = [...s.panes];
  let focus = s.focus;
  for (let p = panes.length - 1; p >= 0; p--) {
    if (!gone.has(panes[p])) continue;
    if (panes.length > 1) {
      panes.splice(p, 1);
      if (focus > p || focus === panes.length) focus--;
      continue;
    }
    const was = at(s, panes[p]);
    const pick = [...s.tabs.slice(was + 1), ...s.tabs.slice(0, was).reverse()].find((t) => !gone.has(t.id))!;
    panes = [pick.id];
  }
  return { tabs, panes, focus: Math.max(0, Math.min(focus, panes.length - 1)), closed };
}

/** Brings back the last closed tab, in front. */
export function reopen(s: Tabs): Tabs {
  const [t, ...closed] = s.closed;
  if (!t) return s;
  const panes = [...s.panes];
  panes[s.focus] = t.id;
  return { ...s, tabs: ordered([...s.tabs, t]), panes, closed };
}

export function duplicate(s: Tabs, id: string): Tabs {
  const t = s.tabs.find((x) => x.id === id);
  if (!t) return s;
  const copy = { ...newTab(t.path), title: t.title, back: [...t.back] };
  const i = at(s, id) + 1;
  return show({ ...s, tabs: ordered([...s.tabs.slice(0, i), copy, ...s.tabs.slice(i)]) }, copy.id);
}

/** Moves a tab to `index` in the strip; a pinned tab stays among the pinned and the rest after them. */
export function move(s: Tabs, id: string, index: number): Tabs {
  const t = s.tabs.find((x) => x.id === id);
  if (!t) return s;
  const rest = s.tabs.filter((x) => x !== t);
  rest.splice(Math.max(0, Math.min(index, rest.length)), 0, t);
  return { ...s, tabs: ordered(rest) };
}

export function pin(s: Tabs, id: string): Tabs {
  return { ...s, tabs: ordered(s.tabs.map((t) => (t.id === id ? { ...t, pinned: !t.pinned } : t))) };
}

/** Walks a tab's own history by `steps` (negative is back). */
export function walk(s: Tabs, id: string, steps: number): Tabs {
  return {
    ...s,
    tabs: s.tabs.map((t) => {
      if (t.id !== id) return t;
      let { path, back, forward } = t;
      for (; steps < 0 && back.length; steps++) [forward, path, back] = [[path, ...forward], back.at(-1)!, back.slice(0, -1)];
      for (; steps > 0 && forward.length; steps--) [back, path, forward] = [[...back, path], forward[0], forward.slice(1)];
      return path === t.path ? t : { ...t, path, title: titleOf(path), back, forward };
    }),
  };
}

/**
 * Takes a tab to `path` through its own history — the nearest step back or forward, whichever
 * `backFirst` prefers. A path it never had is simply where it is now, with its history untouched.
 */
export function seek(s: Tabs, id: string, path: string, backFirst: boolean): Tabs {
  const t = s.tabs.find((x) => x.id === id);
  if (!t || t.path === path) return s;
  const b = t.back.lastIndexOf(path),
    f = t.forward.indexOf(path);
  const back = b >= 0 ? b - t.back.length : 0,
    ahead = f >= 0 ? f + 1 : 0;
  const steps = backFirst ? back || ahead : ahead || back;
  if (steps) return walk(s, id, steps);
  return { ...s, tabs: s.tabs.map((x) => (x === t ? { ...t, path, title: titleOf(path) } : x)) };
}

export function rename(s: Tabs, id: string, title: string): Tabs {
  const t = s.tabs.find((x) => x.id === id);
  return !t || t.title === title ? s : { ...s, tabs: s.tabs.map((x) => (x === t ? { ...t, title } : x)) };
}

/** What survives a reload: the strip, the panes, the focus; history and closed tabs do not. */
export const save = (s: Tabs) =>
  JSON.stringify({ tabs: s.tabs.map(({ id, path, title, pinned }) => ({ id, path, title, pinned })), panes: s.panes, focus: s.focus });

/** A saved strip, kept only as far as it is still a strip of this org's pages. */
export function restore(text: string | null, org: string): Tabs | null {
  try {
    const d = JSON.parse(text || "null");
    const tabs: Tab[] = (Array.isArray(d?.tabs) ? d.tabs : [])
      .filter((t: Tab) => typeof t?.id === "string" && typeof t.path === "string" && route(t.path)?.org === org)
      .map((t: Tab) => ({ id: t.id, path: t.path, title: typeof t.title === "string" ? t.title : titleOf(t.path), pinned: !!t.pinned, back: [], forward: [] }));
    if (!tabs.length) return null;
    const ids = new Set(tabs.map((t) => t.id));
    const panes = (Array.isArray(d.panes) ? d.panes : []).filter((p: string) => ids.has(p)).slice(0, MAX_PANES);
    if (!panes.length) panes.push(tabs[0].id);
    return { tabs: ordered(tabs), panes, focus: Math.max(0, Math.min(Number(d.focus) || 0, panes.length - 1)), closed: [] };
  } catch {
    return null;
  }
}

/**
 * Where the address bar says to be: the tab already showing `path` if one is, else `path` in a
 * new tab. An org root only opens a tab when nothing is open.
 */
export function arrive(s: Tabs, path: string): Tabs {
  const cur = active(s);
  if (cur?.path === path) return s;
  const there = s.tabs.find((t) => t.path === path);
  if (there) return show(s, there.id);
  if (route(path)?.kind === "home" && cur) return s;
  return open(s, path, "tab", true);
}
