import {
  createResource,
  createSignal,
  onCleanup,
  Show,
  type Accessor,
  type JSX,
} from "solid-js";
export const when = (s: string | null) =>
  s ? new Date(s).toLocaleString() : "never";
export const list = (s: string) => s.split(/[\s,]+/).filter(Boolean);
export function resource<T>(
  load: () => Promise<T>,
  every?: number,
  source?: Accessor<unknown>,
) {
  const [data, { refetch }] = createResource(source || (() => true), load);
  if (every) {
    const timer = setInterval(refetch, every);
    onCleanup(() => clearInterval(timer));
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
    <p class={p.error ? "error" : "muted"} role={p.error ? "alert" : "status"}>
      {p.error?.message || "Loading…"}
    </p>
  );
}
export function Copy(p: { text: string }) {
  const [label, setLabel] = createSignal("Copy");
  return (
    <button
      type="button"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(p.text);
          setLabel("Copied");
        } catch {
          setLabel("Copy failed — select the text");
        }
      }}
    >
      {label()}
    </button>
  );
}
export function Secret(p: { value: string; once?: boolean }) {
  const [visible, setVisible] = createSignal(false);
  return (
    <span class="secret">
      <code>{visible() ? p.value : "••••••••••••••••"}</code>
      <button type="button" onClick={() => setVisible(!visible())}>
        {visible() ? "Hide" : "Reveal"}
      </button>
      <Copy text={p.value} />
      <Show when={p.once}>
        <small>Save this now. It is shown only once.</small>
      </Show>
    </span>
  );
}
export function Modal(p: {
  title: string;
  close: () => void;
  children: JSX.Element;
}) {
  let dialog!: HTMLDialogElement;
  queueMicrotask(() => dialog.showModal());
  return (
    <dialog ref={dialog} onClose={p.close} onCancel={p.close}>
      <div class="row">
        <h2>{p.title}</h2>
        <button
          type="button"
          class="right"
          aria-label="Close dialog"
          onClick={p.close}
        >
          ×
        </button>
      </div>
      {p.children}
    </dialog>
  );
}
export function Access(p: {
  value: string[];
  save: (v: string[]) => Promise<unknown>;
}) {
  const [editing, setEditing] = createSignal(false),
    [text, setText] = createSignal(p.value.join(", "));
  const a = action();
  return (
    <>
      <button
        class="quiet"
        onClick={() => {
          setText(p.value.join(", "));
          setEditing(true);
        }}
      >
        {p.value.join(", ") || "No access"}
      </button>
      <Show when={editing()}>
        <Modal title="Edit access" close={() => setEditing(false)}>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              a.run(async () => {
                await p.save(list(text()));
                setEditing(false);
              });
            }}
          >
            <label>
              Actors and tags
              <input
                value={text()}
                onInput={(e) => setText(e.currentTarget.value)}
                placeholder="@alice, #engineering"
              />
            </label>
            <p class="muted">Separate @actors and #tags with commas.</p>
            <ErrorText message={a.error()} />
            <button class="primary" disabled={a.busy()}>
              Save access
            </button>
          </form>
        </Modal>
      </Show>
    </>
  );
}
