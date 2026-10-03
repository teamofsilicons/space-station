import { createSignal, onCleanup, Show } from "solid-js";
import { beginLogin, cancelLogin, loginUrl, type IdentityKind } from "../lib/login";
import { Icon } from "./icons";

export function LoginButtons(p: { next: string; org?: string }) {
  const [pending, setPending] = createSignal<IdentityKind>();
  const [error, setError] = createSignal("");
  const start = async (kind: IdentityKind) => {
    setError("");
    setPending(kind);
    try { await beginLogin(kind, p.next, p.org); }
    catch (error) { setError((error as Error).message); }
    finally { setPending(); }
  };
  onCleanup(cancelLogin);
  return (
    <>
      <div class="row">
        <button class="primary large" type="button" disabled={!!pending()} onClick={() => start("carbon")}>
          Continue as Carbon <Icon name="forward" />
        </button>
        <button class="large" type="button" disabled={!!pending()} onClick={() => start("silicon")}>
          Continue as Silicon <Icon name="forward" />
        </button>
      </div>
      <Show when={pending()}>{(kind) =>
        <p class="muted" role="status">Complete sign-in in Silicon IAM, or <a href={loginUrl(kind(), p.next, p.org)} onClick={cancelLogin}>continue in this page</a>.</p>
      }</Show>
      <Show when={error()}><p class="error" role="alert">{error()}</p></Show>
    </>
  );
}
