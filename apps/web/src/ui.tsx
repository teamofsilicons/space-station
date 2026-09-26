// The small pieces every page shares: a polled resource that rests while its tab is hidden, a busy
// action, errors and loading lines, copy and one-time secrets, dialogs, menus, access lists, and
// the tab a page lives in (`useTab`), which is how a page navigates, names itself and signals.
import {
  createContext,
  createEffect,
  createResource,
  createSignal,
  For,
  on,
  onCleanup,
  Show,
  useContext,
  type Accessor,
  type JSX,
  type Signal,
} from "solid-js";
import { createStore, reconcile, unwrap } from "solid-js/store";
import type { How, Route } from "../lib/tabs";
import { Icon } from "./icons";

/** A tab as its page sees it. `mark` sets what the strip shows beside the title. */
export type TabApi = {
  id: string;
  route: Accessor<Route>;
  visible: Accessor<boolean>;
  go: (path: string) => void;
  open: (path: string, how?: How) => void;
  title: (title: string) => void;
  mark: (m: Mark) => void;
};
export type Mark = { live?: "ok" | "stale" | "error"; activity?: boolean };
export const TabContext = createContext<TabApi>();
export const useTab = () => useContext(TabContext)!;

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
export const list = (s: string) => s.split(/[\s,]+/).filter(Boolean);
export const count = (n: number) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e4 ? Math.round(n / 1e3) + "k" : n.toLocaleString());

/**
 * Where a resource keeps its value: a list is reconciled by `id`, so a poll updates rows in place
 * and never rebuilds one (or closes what it has open); anything else is replaced whole, unwrapped.
 */
function kept<T>(init: T | undefined): Signal<T | undefined> {
  const [store, set] = createStore<{ v: T | undefined }>({ v: init });
  return [
    () => (Array.isArray(store.v) ? store.v : unwrap(store.v)),
    (v: unknown) => {
      const next = typeof v === "function" ? v(unwrap(store.v)) : v;
      if (Array.isArray(next)) set("v", reconcile(next) as never);
      else set("v", () => next as T);
      return next;
    },
  ] as Signal<T | undefined>;
}

/**
 * Loads once, again when `source` changes, and every `every` ms while its tab is on screen. A poll
 * that fails keeps the last value rather than throwing into whatever reads it.
 */
export function resource<T>(load: () => Promise<T>, every?: number, source?: Accessor<unknown>) {
  const [data, { refetch }] = createResource<T, unknown>(
    source || (() => true),
    async (_, info) => {
      try {
        return await load();
      } catch (e) {
        if (info.refetching && info.value !== undefined) return info.value as T;
        throw e;
      }
    },
    { storage: kept },
  );
  const tab = useContext(TabContext);
  if (every) {
    const timer = setInterval(() => tab?.visible() === false || refetch(), every);
    onCleanup(() => clearInterval(timer));
    if (tab) createEffect(on(tab.visible, (v) => v && refetch(), { defer: true }));
  }
  return { data, reload: refetch };
}

export function action() {
  const [busy, setBusy] = createSignal(false),
    [error, setError] = createSignal("");
  return {
    busy,
    error,
    run: async (fn: () => Promise<unknown>) => {
      if (busy()) return;
      setBusy(true);
      setError("");
      try {
        await fn();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
  };
}

export function ErrorText(p: { message?: string }) {
  return (
    <Show when={p.message}>
      <p class="error" role="alert">
        {p.message}
      </p>
    </Show>
  );
}

export function Loading(p: { error?: Error }) {
  return (
    <p class={p.error ? "error" : "loading"} role={p.error ? "alert" : "status"}>
      {p.error?.message || "Loading…"}
    </p>
  );
}

export function Copy(p: { text: string; label?: string }) {
  const [done, setDone] = createSignal<"" | "ok" | "fail">("");
  return (
    <button
      type="button"
      class="ghost small"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(p.text);
          setDone("ok");
        } catch {
          setDone("fail");
        }
        setTimeout(() => setDone(""), 1600);
      }}
    >
      <Icon name={done() === "ok" ? "check" : "copy"} />
      {done() === "ok" ? "Copied" : done() === "fail" ? "Select the text" : p.label || "Copy"}
    </button>
  );
}

export function Secret(p: { value: string; once?: boolean }) {
  const [visible, setVisible] = createSignal(!!p.once);
  return (
    <div class="secret">
      <code>{visible() ? p.value : "•".repeat(28)}</code>
      <div class="secret-actions">
        <button type="button" class="ghost small" onClick={() => setVisible(!visible())}>
          {visible() ? "Hide" : "Reveal"}
        </button>
        <Copy text={p.value} />
      </div>
      <Show when={p.once}>
        <small class="warn">Save this now. It is shown only once.</small>
      </Show>
    </div>
  );
}

/** A modal dialog. In a tab that leaves the screen it steps aside, and comes back with the tab. */
export function Modal(p: { title: string; close: () => void; children: JSX.Element; wide?: boolean }) {
  let dialog!: HTMLDialogElement;
  const tab = useContext(TabContext);
  const shown = () => !tab || tab.visible();
  createEffect(() => (shown() ? !dialog.open && dialog.showModal() : dialog.open && dialog.close()));
  return (
    <dialog ref={dialog} class={p.wide ? "wide" : ""} onClose={() => shown() && p.close()} onCancel={p.close}>
      <div class="dialog-head">
        <h2>{p.title}</h2>
        <button type="button" class="icon" aria-label="Close dialog" onClick={p.close}>
          <Icon name="x" />
        </button>
      </div>
      {p.children}
    </dialog>
  );
}

/** A segmented control: one choice of a few, always visible. */
export function Segmented<T extends string>(p: { value: T; options: readonly T[]; label: string; onChange: (v: T) => void; names?: Partial<Record<T, string>> }) {
  return (
    <div class="segmented" role="radiogroup" aria-label={p.label}>
      <For each={p.options}>
        {(o) => (
          <button type="button" role="radio" aria-checked={o === p.value} classList={{ on: o === p.value }} onClick={() => p.onChange(o)}>
            {p.names?.[o] ?? o}
          </button>
        )}
      </For>
    </div>
  );
}

// ─── Menus: one at a time, anywhere, closed by any click outside, Escape or a blur ────────────────

export type MenuItem = { label: string; icon?: string; keys?: string; run: () => void; danger?: boolean; disabled?: boolean } | "-";
const [menu, setMenu] = createSignal<{ x: number; y: number; items: MenuItem[] }>();
export const closeMenu = () => setMenu(undefined);

/** Opens a menu at the pointer, or under the element that was clicked. */
export function openMenu(e: MouseEvent, items: MenuItem[]) {
  e.preventDefault();
  e.stopPropagation();
  const r = e.type === "contextmenu" ? null : (e.currentTarget as HTMLElement).getBoundingClientRect();
  setMenu({ x: r ? r.left : e.clientX, y: r ? r.bottom + 4 : e.clientY, items });
}

export function MenuHost() {
  let el: HTMLDivElement | undefined;
  const outside = (e: Event) => el && !el.contains(e.target as Node) && closeMenu();
  const key = (e: KeyboardEvent) => e.key === "Escape" && closeMenu();
  addEventListener("pointerdown", outside, true);
  addEventListener("keydown", key);
  addEventListener("blur", closeMenu);
  onCleanup(() => {
    removeEventListener("pointerdown", outside, true);
    removeEventListener("keydown", key);
    removeEventListener("blur", closeMenu);
  });
  return (
    <Show when={menu()}>
      {(m) => (
        <div
          ref={(node) => {
            el = node;
            // Keep the menu on screen: flip left or up when it would spill over an edge.
            queueMicrotask(() => {
              node.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
              const r = node.getBoundingClientRect();
              if (r.right > innerWidth - 8) node.style.left = Math.max(8, innerWidth - r.width - 8) + "px";
              if (r.bottom > innerHeight - 8) node.style.top = Math.max(8, m().y - r.height) + "px";
            });
          }}
          class="menu"
          role="menu"
          style={{ left: m().x + "px", top: m().y + "px" }}
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
            e.preventDefault();
            const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
            const i = items.indexOf(document.activeElement as HTMLButtonElement);
            items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
          }}
        >
          <For each={m().items}>
            {(item) =>
              item === "-" ? (
                <hr />
              ) : (
                <button
                  role="menuitem"
                  classList={{ danger: item.danger }}
                  disabled={item.disabled}
                  onClick={() => {
                    closeMenu();
                    item.run();
                  }}
                >
                  <Icon name={item.icon || "blank"} />
                  <span>{item.label}</span>
                  <Show when={item.keys}>
                    <kbd>{item.keys}</kbd>
                  </Show>
                </button>
              )
            }
          </For>
        </div>
      )}
    </Show>
  );
}

/** Actor and tag chips; an editor behind them when `save` is given. */
export function Access(p: { value: string[]; save?: (v: string[]) => Promise<unknown> }) {
  const [editing, setEditing] = createSignal(false),
    [text, setText] = createSignal(p.value.join(", "));
  const a = action();
  const chips = (
    <span class="chips">
      <Show when={p.value.length} fallback={<span class="chip muted">Creator only</span>}>
        <For each={p.value}>{(v) => <span class="chip" classList={{ tag: !v.startsWith("@") }}>{v}</span>}</For>
      </Show>
    </span>
  );
  return (
    <Show when={p.save} fallback={chips}>
      <button
        type="button"
        class="access-edit"
        title="Edit access"
        onClick={() => {
          setText(p.value.join(", "));
          setEditing(true);
        }}
      >
        {chips}
        <Icon name="pencil" />
      </button>
      <Show when={editing()}>
        <Modal title="Edit access" close={() => setEditing(false)}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              a.run(async () => {
                await p.save!(list(text()));
                setEditing(false);
              });
            }}
          >
            <label>
              Actors and tags
              <input value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder="@c:alice, engineering" autofocus />
            </label>
            <p class="hint">Separate @c:handle, @si:handle and tags with commas. Access is the union of all of them.</p>
            <ErrorText message={a.error()} />
            <div class="dialog-foot">
              <button type="button" onClick={() => setEditing(false)}>Cancel</button>
              <button class="primary" disabled={a.busy()}>Save access</button>
            </div>
          </form>
        </Modal>
      </Show>
    </Show>
  );
}

export function Empty(p: { icon: string; title: string; children?: JSX.Element }) {
  return (
    <div class="empty">
      <span class="empty-icon"><Icon name={p.icon} size={22} /></span>
      <strong>{p.title}</strong>
      {p.children}
    </div>
  );
}
