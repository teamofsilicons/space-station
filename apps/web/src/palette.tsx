"use client";
// Search and jump, like Chrome's omnibox: open tabs, windows, tables, pages, docs and actions in
// one Silicon UI command palette. ↵ switches to a tab already showing it (or opens it), ⌘↵ opens it
// behind, ⇧↵ in a split. The ⌘K overlay and the new tab page are the same finder.
import { useRef } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { CommandPalette } from "@/components/silicon-ui/command-palette/command-palette";
import * as T from "../lib/tabs";
import { KIND_ICON, useLists, useTabs, useWorkspace, type Workspace } from "./tabs";
import { useDocs } from "./docs";
import type { DocEntry } from "./docs-content";
import type { SpaceWindow, Table } from "../lib/api";
import { count } from "./ui";
import { Icon } from "./icons";

export type Item = { id?: string; label: string; group: string; icon: string; hint?: string; keys?: string; path?: string; run?: () => void };

export function items(ws: Workspace, lists: { tables?: Table[]; windows?: SpaceWindow[] }, docs: DocEntry[], s: T.Tabs): Item[] {
  const o = `/a/${ws.actor}`,
    t = ws.tabs,
    here = T.active(s)?.path;
  const page = (label: string, path: string, icon: string): Item => ({ label, group: "Pages", icon, path });
  const places: Item[] = [
    ...(lists.windows || []).map((w): Item => ({ label: w.name, group: "Space Windows", icon: "window", hint: w.version ? `live · ${w.version.name}` : "no version yet", path: `${o}/windows/${w.id}` })),
    ...(lists.tables || []).map((x): Item => ({ label: x.id, group: "Tables", icon: "table", hint: `${count(x.records)} records`, path: `${o}/tables/${x.id}` })),
    page("Tables", `${o}/tables`, "table"),
    page("Space Windows", `${o}/windows`, "window"),
    page("Notifications", `${o}/notifications`, "bell"),
    page("Settings", `${o}/settings`, "settings"),
    ...docs.map((d): Item => ({ label: d.title, group: "Docs", icon: "docs", path: `${o}/docs/${d.slug}` })),
  ].filter((x) => x.path !== here);
  // A place already open in a tab is offered as a switch; tabs showing anything else are listed as tabs.
  const open = new Set(s.tabs.map((x) => x.path));
  const known = new Set(places.map((x) => x.path));
  return [
    ...s.tabs
      .filter((x) => x.path !== here && !known.has(x.path))
      .map((x): Item => ({ id: `tab:${x.id}`, label: x.title, group: "Open tabs", icon: KIND_ICON[T.route(x.path)?.kind || "home"], hint: "Switch to tab", run: () => t.show(x.id) })),
    ...places.map((x) => (open.has(x.path!) ? { ...x, hint: "Switch to tab" } : x)),
    { label: "New table", group: "Actions", icon: "plus", run: () => ws.create("table") },
    { label: "New Space Window", group: "Actions", icon: "plus", run: () => ws.create("window") },
    { label: "New tab", group: "Actions", icon: "plus", keys: "⌥T", run: () => t.open(t.home, "tab", true) },
    { label: "Split view", group: "Actions", icon: "split", keys: "⌥\\", run: () => t.open(t.home, "split") },
    { label: "Reopen closed tab", group: "Actions", icon: "reload", keys: "⌥⇧T", run: t.reopen },
    { label: "Developer errors", group: "Actions", icon: "bug", keys: "⌥⇧D", run: ws.dev },
  ];
}

/** The finder: the command palette over everything this station can open, ⌘↵ and ⇧↵ included. */
export function Finder(p: { autoFocus?: boolean; onDone?: () => void; placeholder?: string }) {
  const ws = useWorkspace();
  const { tables, windows } = useLists();
  const { s } = useTabs();
  const docs = useDocs();
  const list = items(ws, { tables: tables.data, windows: windows.data }, docs, s);
  const byId = new Map(list.map((x) => [x.id || x.path || `${x.group}:${x.label}`, x]));
  // The palette reports only what was chosen; how it was chosen is read on the way in.
  const how = useRef<T.How | undefined>(undefined);
  const note = (e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }) => (how.current = e.metaKey || e.ctrlKey ? "background" : e.shiftKey ? "split" : undefined);
  return (
    <div className="finder" onKeyDownCapture={(e) => e.key === "Enter" && note(e)} onClickCapture={note}>
      <CommandPalette
        label="Search"
        autoFocus={p.autoFocus}
        placeholder={p.placeholder || "Search tabs, windows, tables, docs and actions"}
        items={[...byId].map(([id, x]) => ({ id, label: x.label, group: x.group, description: x.hint, shortcut: x.keys, icon: <Icon name={x.icon} /> }))}
        onClose={p.onDone}
        onSelect={(chosen) => {
          const item = byId.get(chosen.id);
          if (!item) return;
          p.onDone?.();
          if (item.run) item.run();
          else ws.tabs.reveal(item.path!, how.current);
        }}
      />
    </div>
  );
}

export function Palette(p: { close: () => void }) {
  return (
    <DialogPrimitive.Root open onOpenChange={(open) => !open && p.close()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="palette-scrim" />
        <DialogPrimitive.Content className="palette" aria-describedby={undefined}>
          <DialogPrimitive.Title className="visually-hidden">Search</DialogPrimitive.Title>
          <Finder autoFocus onDone={p.close} />
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
