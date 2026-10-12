"use client";
// The small pieces every page shares: a store any component can follow, a polled resource that rests
// while its tab is hidden, a busy action, errors and loading lines, copy and one-time secrets,
// dialogs, menus, and the tab a page lives in (`useTab`), which is how a page navigates, names
// itself and signals. The surfaces are Silicon UI's.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode, type MouseEvent as ReactMouseEvent } from "react";
import { Alert } from "@/components/silicon-ui/alert/alert";
import { Button } from "@/components/silicon-ui/button/button";
import { CopyButton } from "@/components/silicon-ui/copy-button/copy-button";
import { Dialog, DialogContent } from "@/components/silicon-ui/dialog/dialog";
import { DropdownMenuAt, type DropdownItem } from "@/components/silicon-ui/dropdown-menu/dropdown-menu";
import { EmptyState } from "@/components/silicon-ui/empty-state/empty-state";
import SegmentedControl from "@/components/silicon-ui/segmented-control/segmented-control";
import type { How, Route } from "../lib/tabs";
import { Icon } from "./icons";

// ─── A store: a value, and whoever follows it re-renders when it changes ──────────────────────────

export type Store<T> = { get: () => T; set: (next: T | ((old: T) => T)) => void; subscribe: (fn: () => void) => () => void };
export function store<T>(value: T): Store<T> {
  const listeners = new Set<() => void>();
  return {
    get: () => value,
    set: (next) => {
      const v = typeof next === "function" ? (next as (old: T) => T)(value) : next;
      if (Object.is(v, value)) return;
      value = v;
      listeners.forEach((fn) => fn());
    },
    subscribe: (fn) => (listeners.add(fn), () => void listeners.delete(fn)),
  };
}
export const useStore = <T,>(s: Store<T>) => useSyncExternalStore(s.subscribe, s.get, s.get);

// ─── The tab a page lives in ──────────────────────────────────────────────────────────────────────

/** A tab as its page sees it. `mark` sets what the strip shows beside the title. */
export type TabApi = {
  id: string;
  route: Route;
  visible: boolean;
  go: (path: string) => void;
  open: (path: string, how?: How) => void;
  title: (title: string) => void;
  mark: (m: Mark) => void;
};
export type Mark = { live?: "ok" | "stale" | "error"; activity?: boolean };
export const TabContext = createContext<TabApi | undefined>(undefined);
export const useTab = () => useContext(TabContext)!;
/** Names the tab this page is in. */
export function useTitle(title: string | undefined) {
  const tab = useContext(TabContext);
  useEffect(() => void (title && tab?.title(title)), [tab, title]);
}

export const when = (s: string | null) => (s ? new Date(s).toLocaleString() : "never");
/** "just now", "5m ago", "3h ago", "2d ago", then the date. */
export function ago(s: string | number | null) {
  if (s === null) return "never";
  const t = typeof s === "number" ? s : Date.parse(s);
  const m = Math.round((Date.now() - t) / 6e4);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  if (m < 1440) return `${Math.round(m / 60)}h ago`;
  if (m < 43200) return `${Math.round(m / 1440)}d ago`;
  return new Date(t).toLocaleDateString();
}
export const count = (n: number) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e4 ? Math.round(n / 1e3) + "k" : n.toLocaleString());

// ─── Loading and acting ───────────────────────────────────────────────────────────────────────────

export type Resource<T> = { data: T | undefined; error: Error | undefined; reload: () => Promise<void> };

/**
 * Loads once, again when `deps` change (showing the last value meanwhile), and every `every` ms while its
 * tab is on screen (and when it comes back on screen). A reload that fails keeps the last value rather
 * than showing an error; a load for new `deps` that fails shows it.
 */
export function useResource<T>(load: () => Promise<T>, every?: number, deps: readonly unknown[] = []): Resource<T> {
  const [state, setState] = useState<{ data?: T; error?: Error }>({});
  const latest = useRef(load);
  latest.current = load;
  const seq = useRef(0);
  const had = useRef(false);
  const fetch = useCallback(async (fresh: boolean) => {
    const n = ++seq.current;
    if (fresh) had.current = false;
    try {
      const data = await latest.current();
      if (n !== seq.current) return;
      had.current = true;
      setState({ data });
    } catch (e) {
      if (n !== seq.current) return;
      const error = e instanceof Error ? e : new Error(String(e));
      setState((old) => (had.current ? old : { error }));
    }
  }, []);
  const reload = useCallback(() => fetch(false), [fetch]);
  useEffect(() => void fetch(true), deps); // eslint-disable-line react-hooks/exhaustive-deps
  const visible = useContext(TabContext)?.visible !== false;
  const shown = useRef(visible);
  useEffect(() => {
    if (every && visible && !shown.current) reload();
    shown.current = visible;
    if (!every || !visible) return;
    const timer = setInterval(reload, every);
    return () => clearInterval(timer);
  }, [every, visible, reload]);
  return useMemo(() => ({ data: state.data, error: state.error, reload }), [state, reload]);
}

export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const running = useRef(false);
  const run = useCallback(async (fn: () => Promise<unknown>) => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      running.current = false;
      setBusy(false);
    }
  }, []);
  return { busy, error, run };
}

export function ErrorText(p: { message?: string; title?: string }) {
  if (!p.message) return null;
  return (
    <Alert tone="danger" title={p.title || "Something went wrong"} style={{ whiteSpace: "pre-wrap" }}>
      {p.message}
    </Alert>
  );
}

export function Loading(p: { error?: Error }) {
  if (p.error) return <ErrorText message={p.error.message} />;
  return (
    <p className="loading" role="status">
      Loading…
    </p>
  );
}

// ─── Copy and secrets ─────────────────────────────────────────────────────────────────────────────

export function Copy(p: { text: string; label?: string }) {
  return <CopyButton value={p.text} label={p.label || "Copy"} variant="plain" />;
}

export function Secret(p: { value: string; once?: boolean }) {
  const [visible, setVisible] = useState(!!p.once);
  return (
    <div className="secret">
      <code>{visible ? p.value : "•".repeat(28)}</code>
      <div className="secret-actions">
        <Button type="button" variant="ghost" size="sm" onClick={() => setVisible(!visible)}>
          {visible ? "Hide" : "Reveal"}
        </Button>
        <Copy text={p.value} />
      </div>
      {p.once && <small className="warn">Save this now. It is shown only once.</small>}
    </div>
  );
}

// ─── Dialogs, choices, empty states ───────────────────────────────────────────────────────────────

/** A modal dialog. In a tab that leaves the screen it steps aside, and comes back with the tab. */
export function Modal(p: { title: string; description?: string; close: () => void; children: ReactNode; wide?: boolean }) {
  const tab = useContext(TabContext);
  return (
    <Dialog open={!tab || tab.visible} onOpenChange={(open) => !open && p.close()}>
      <DialogContent
        title={p.title}
        description={p.description}
        className={p.wide ? "wide" : undefined}
        // Like a native modal: Escape or the close button, never a stray click outside (a key shown once must survive one).
        onPointerDownOutside={(e) => e.preventDefault()}
        // A dialog with a field opens on that field, not on its close button.
        onOpenAutoFocus={(e) => {
          const field = (e.currentTarget as HTMLElement).querySelector<HTMLElement>("input:not([type=hidden]):not(:disabled), textarea:not(:disabled), select:not(:disabled)");
          if (field) (e.preventDefault(), field.focus());
        }}
      >
        {p.children}
      </DialogContent>
    </Dialog>
  );
}

/** A segmented control: one choice of a few, always visible. */
export function Segmented<T extends string>(p: { value: T; options: readonly T[]; label: string; onChange: (v: T) => void; names?: Partial<Record<T, string>> }) {
  return <SegmentedControl label={p.label} value={p.value} onValueChange={(v) => p.onChange(v as T)} options={p.options.map((o) => ({ value: o, label: p.names?.[o] ?? o }))} />;
}

export function Empty(p: { icon: string; title: string; description: string; action?: ReactNode }) {
  return <EmptyState icon={<Icon name={p.icon} size={22} />} title={p.title} description={p.description} action={p.action} />;
}

// ─── Menus: one at a time, anywhere, at the pointer or under what was clicked ─────────────────────

export type MenuItem = { label: string; icon?: string; run: () => void; danger?: boolean; disabled?: boolean } | "-";
const menu = store<{ at: { x: number; y: number }; items: MenuItem[]; n: number } | null>(null);
let opened = 0;
export const closeMenu = () => menu.set(null);

/** Opens a menu at the pointer for a right click, else under the element that was clicked. */
export function openMenu(e: ReactMouseEvent | MouseEvent, items: MenuItem[], anchor = e.currentTarget as HTMLElement) {
  e.preventDefault();
  e.stopPropagation();
  const r = e.type === "contextmenu" ? null : anchor.getBoundingClientRect();
  menu.set({ at: r ? { x: r.left, y: r.bottom } : { x: e.clientX, y: e.clientY }, items, n: ++opened });
}

export function MenuHost() {
  const m = useStore(menu);
  // A click into an iframe (a live window) reaches no listener here; the window's blur does.
  useEffect(() => (addEventListener("blur", closeMenu), () => removeEventListener("blur", closeMenu)), []);
  // A closing menu keeps its items while it fades out.
  const last = useRef(m);
  if (m) last.current = m;
  const items: DropdownItem[] = [];
  (last.current?.items || []).forEach((item, i, all) => {
    if (item === "-") return;
    items.push({ label: item.label, icon: <Icon name={item.icon || "blank"} />, onSelect: item.run, destructive: item.danger, disabled: item.disabled, separatorBefore: all[i - 1] === "-" });
  });
  return <DropdownMenuAt key={last.current?.n} at={m?.at ?? null} items={items} onClose={closeMenu} />;
}
