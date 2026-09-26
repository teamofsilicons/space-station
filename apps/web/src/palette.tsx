// Search and jump, like Chrome's omnibox: open tabs, windows, tables, pages, docs and actions in
// one list. ↵ switches to a tab already showing it (or opens it), ⌘↵ opens it behind, ⇧↵ in a
// split. The ⌘K overlay and the new tab page are the same finder.
import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import * as T from "../lib/tabs";
import { KIND_ICON, useWorkspace, type Workspace } from "./tabs";
import { DOCS } from "./docs";
import { count } from "./ui";
import { Icon } from "./icons";

export type Item = { label: string; group: string; icon: string; hint?: string; keys?: string; path?: string; run?: () => void; open?: boolean };

export function items(ws: Workspace, extra: Item[] = []): Item[] {
  const o = `/o/${ws.org}`,
    t = ws.tabs,
    here = t.active()?.path;
  const page = (label: string, path: string, icon: string): Item => ({ label, group: "Pages", icon, path });
  const places: Item[] = [
    ...(ws.windows.data() || []).map((w): Item => ({ label: w.name, group: "Space Windows", icon: "window", hint: w.version ? `live · ${w.version.name}` : "no version yet", path: `${o}/windows/${w.id}` })),
    ...(ws.tables.data() || []).map((x): Item => ({ label: x.id, group: "Tables", icon: "table", hint: `${count(x.records)} records`, path: `${o}/tables/${x.id}` })),
    page("Tables", `${o}/tables`, "table"),
    page("Space Windows", `${o}/windows`, "window"),
    page("Notifications", `${o}/notifications`, "bell"),
    page("Settings", `${o}/settings`, "settings"),
    ...DOCS.map((d): Item => ({ label: d.title, group: "Docs", icon: "docs", path: `${o}/docs/${d.slug}` })),
  ].filter((x) => x.path !== here);
  // A place already open in a tab is offered as a switch; tabs showing anything else are listed as tabs.
  const open = new Set(t.state().tabs.map((x) => x.path));
  const known = new Set(places.map((x) => x.path));
  return [
    ...t.state().tabs.filter((x) => x.path !== here && !known.has(x.path)).map((x): Item => ({
      label: x.title,
      group: "Open tabs",
      icon: KIND_ICON[T.route(x.path)?.kind || "home"],
      hint: "Switch to tab",
      open: true,
      run: () => t.show(x.id),
    })),
    ...places.map((x) => (open.has(x.path!) ? { ...x, hint: "Switch to tab", open: true } : x)),
    { label: "New table", group: "Actions", icon: "plus", run: () => ws.create("table") },
    { label: "New Space Window", group: "Actions", icon: "plus", run: () => ws.create("window") },
    { label: "New tab", group: "Actions", icon: "plus", keys: "⌥T", run: () => t.open(t.home, "tab", true) },
    { label: "Split view", group: "Actions", icon: "split", keys: "⌥\\", run: () => t.open(t.home, "split") },
    { label: "Reopen closed tab", group: "Actions", icon: "reload", keys: "⌥⇧T", run: t.reopen },
    { label: "Developer errors", group: "Actions", icon: "bug", keys: "⌥⇧D", run: ws.dev },
    ...extra,
  ];
}

/** Higher is better: an exact start, then a word start, then anywhere, then the letters in order. */
export function score(text: string, q: string) {
  const s = text.toLowerCase();
  if (!q) return 1;
  if (s.startsWith(q)) return 100 - s.length / 100;
  const i = s.indexOf(q);
  if (i > 0) return (/[\s/_-]/.test(s[i - 1]) ? 80 : 60) - i / 100;
  let at = 0;
  for (const c of q) {
    at = s.indexOf(c, at) + 1;
    if (!at) return 0;
  }
  return 20 - at / 100;
}

export function Finder(p: { autofocus?: boolean; onDone?: () => void; extra?: Item[]; placeholder?: string; empty?: boolean }) {
  const ws = useWorkspace();
  const [q, setQ] = createSignal("");
  const [sel, setSel] = createSignal(0);
  let input!: HTMLInputElement,
    listEl!: HTMLDivElement;
  const results = createMemo(() => {
    const query = q().trim().toLowerCase();
    const all = items(ws, p.extra);
    if (!query) return p.empty === false ? [] : [...all.filter((x) => x.open), ...all.filter((x) => !x.open && (x.group === "Pages" || x.group === "Actions"))].slice(0, 14);
    return all
      .map((x) => ({ x, s: Math.max(score(x.label, query), score(`${x.group} ${x.label}`, query) * 0.9) }))
      .filter((r) => r.s > 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 30)
      .map((r) => r.x);
  });
  const pick = (item: Item, how?: T.How) => {
    p.onDone?.();
    setQ("");
    if (item.run) return item.run();
    ws.tabs.reveal(item.path!, how);
  };
  onMount(() => p.autofocus && input.focus());
  return (
    <div class="finder">
      <label class="finder-input">
        <Icon name="search" />
        <input
          ref={input}
          aria-label="Search"
          placeholder={p.placeholder || "Search tabs, windows, tables, docs and actions"}
          value={q()}
          onInput={(e) => {
            setQ(e.currentTarget.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            const n = results().length;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setSel((sel() + (e.key === "ArrowDown" ? 1 : -1) + n) % Math.max(n, 1));
              listEl.querySelector(`[data-i="${sel()}"]`)?.scrollIntoView({ block: "nearest" });
            } else if (e.key === "Enter" && results()[sel()]) {
              e.preventDefault();
              pick(results()[sel()], e.metaKey || e.ctrlKey ? "background" : e.shiftKey ? "split" : undefined);
            } else if (e.key === "Escape") p.onDone?.();
          }}
        />
      </label>
      <div class="finder-list" ref={listEl} role="listbox">
        <For each={results()}>
          {(item, i) => (
            <button
              type="button"
              role="option"
              data-i={i()}
              aria-selected={i() === sel()}
              classList={{ sel: i() === sel() }}
              onMouseMove={() => setSel(i())}
              onClick={(e) => pick(item, e.metaKey || e.ctrlKey ? "background" : e.shiftKey ? "split" : undefined)}
            >
              <Icon name={item.icon} />
              <span class="finder-label">{item.label}</span>
              <span class="finder-hint">{item.hint || item.group}</span>
              <Show when={item.keys}>
                <kbd>{item.keys}</kbd>
              </Show>
            </button>
          )}
        </For>
        <Show when={q() && !results().length}>
          <p class="finder-none">Nothing matches “{q()}”.</p>
        </Show>
      </div>
      <Show when={p.empty !== false || q()}>
        <div class="finder-foot">
          <span><kbd>↵</kbd> open</span>
          <span><kbd>⌘↵</kbd> in background</span>
          <span><kbd>⇧↵</kbd> in split</span>
          <span><kbd>esc</kbd> close</span>
        </div>
      </Show>
    </div>
  );
}

export function Palette(p: { close: () => void }) {
  return (
    <div class="palette-scrim" onPointerDown={(e) => e.target === e.currentTarget && p.close()}>
      <div class="palette" role="dialog" aria-label="Search">
        <Finder autofocus onDone={p.close} />
      </div>
    </div>
  );
}
