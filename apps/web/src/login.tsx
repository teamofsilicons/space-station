import { createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { api, currentContext, type SavedContext } from "../lib/api";
import { beginLogin, cancelLogin, loginUrl } from "../lib/login";
import { SiliconButton, SiliconInput } from "./silicon-ui";
import { Icon } from "./icons";
import { route } from "../lib/tabs";

export function LoginButtons(p: { next: string }) {
  const [saved] = createResource(() => api<SavedContext[]>("/auth/contexts").catch(() => []));
  const [pending, setPending] = createSignal(false);
  const [silicon, setSilicon] = createSignal(false);
  const [slt, setSlt] = createSignal("");
  const [error, setError] = createSignal("");
  const start = async () => {
    setSilicon(false);
    setSlt("");
    setError("");
    setPending(true);
    try { await beginLogin("carbon", p.next); }
    catch (error) { setError((error as Error).message); }
    finally { setPending(false); }
  };
  const exchange = async (event: SubmitEvent) => {
    event.preventDefault();
    setError("");
    setPending(true);
    try {
      await api("/auth/session", "POST", { slt: slt().trim(), browser: true, identity_kind: "silicon" });
      setSlt("");
      const next = new URL(p.next, location.origin);
      location.assign(next.origin === location.origin ? next.href : "/");
    } catch (error) { setError((error as Error).message); }
    finally { setPending(false); }
  };
  const select = async (account: SavedContext) => {
    setError("");
    setPending(true);
    try {
      await api("/auth/context", "POST", { context_id: account.context_id });
      location.assign(route(p.next)?.actor === account.uuid ? p.next : `/a/${account.uuid}`);
    } catch (error) { setError((error as Error).message); }
    finally { setPending(false); }
  };
  onCleanup(cancelLogin);
  return (
    <>
      <Show when={saved()?.some((account) => !account.selected || !currentContext())}>
        <div class="saved-accounts">
          <p class="muted">Continue with a saved account</p>
          <div class="row">
            <For each={saved()?.filter((account) => !account.selected || !currentContext())}>
              {(account) => <SiliconButton disabled={pending()} onClick={() => select(account)}>{account.actor} <Icon name="forward" /></SiliconButton>}
            </For>
          </div>
        </div>
      </Show>
      <div class="row">
        <SiliconButton class="primary large" type="button" disabled={pending()} onClick={start}>
          Continue as Carbon <Icon name="forward" />
        </SiliconButton>
        <SiliconButton class="large" type="button" disabled={pending()} aria-expanded={silicon()} onClick={() => setSilicon(!silicon())}>
          Continue as Silicon <Icon name="forward" />
        </SiliconButton>
      </div>
      <Show when={pending() && !silicon()}>
        <p class="muted" role="status">Complete sign-in in Silicon Accounts, or <a href={loginUrl("carbon", p.next)} onClick={cancelLogin}>continue in this page</a>.</p>
      </Show>
      <Show when={silicon()}>
        <form class="silicon-login" onSubmit={exchange}>
          <p class="muted">Generate a single-use token with your signed-in Silicon Accounts CLI:</p>
          <pre>silicon-accounts login --app spacestation -q</pre>
          <label>
            Short-lived token
            <SiliconInput type="password" required pattern="slt_.+" title="Use a short-lived token starting with slt_" autocomplete="off" placeholder="slt_…" value={slt()} onInput={(event) => setSlt(event.currentTarget.value)} />
          </label>
          <SiliconButton class="primary" disabled={pending() || !slt().trim()}>{pending() ? "Signing in…" : "Sign in as Silicon"}</SiliconButton>
          <p class="hint">The token is valid for two minutes. Your session stays signed in until it expires or you sign out.</p>
        </form>
      </Show>
      <Show when={error()}><p class="error" role="alert">{error()}</p></Show>
    </>
  );
}
