"use client";
// Signing in: a saved account, a carbon through Silicon Accounts (in a popup, or this page), or a
// silicon with a short-lived token from its CLI.
import { useEffect, useState, type FormEvent } from "react";
import { Button } from "@/components/silicon-ui/button/button";
import { Input } from "@/components/silicon-ui/input/input";
import { api, currentContext, type SavedContext } from "../lib/api";
import { beginLogin, cancelLogin, loginUrl } from "../lib/login";
import { route } from "../lib/tabs";
import { ErrorText, useResource } from "./ui";
import { Icon } from "./icons";

export function LoginButtons(p: { next: string }) {
  const saved = useResource(() => api<SavedContext[]>("/auth/contexts").catch(() => []));
  const [pending, setPending] = useState(false);
  const [silicon, setSilicon] = useState(false);
  const [slt, setSlt] = useState("");
  const [error, setError] = useState("");
  useEffect(() => cancelLogin, []);
  const attempt = async (fn: () => Promise<unknown>) => {
    setError("");
    setPending(true);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setPending(false);
    }
  };
  const start = () => {
    setSilicon(false);
    setSlt("");
    attempt(() => beginLogin("carbon", p.next));
  };
  const exchange = (e: FormEvent) => {
    e.preventDefault();
    attempt(async () => {
      await api("/auth/session", "POST", { slt: slt.trim(), browser: true, identity_kind: "silicon" });
      setSlt("");
      const next = new URL(p.next, location.origin);
      location.assign(next.origin === location.origin ? next.href : "/");
    });
  };
  const select = (account: SavedContext) =>
    attempt(async () => {
      await api("/auth/context", "POST", { context_id: account.context_id });
      location.assign(route(p.next)?.actor === account.uuid ? p.next : `/a/${account.uuid}`);
    });
  const others = (saved.data || []).filter((account) => !account.selected || !currentContext());
  return (
    <>
      {others.length > 0 && (
        <div className="saved-accounts">
          <p className="muted">Continue with a saved account</p>
          <div className="row">
            {others.map((account) => (
              <Button key={account.context_id} variant="secondary" disabled={pending} onClick={() => select(account)}>
                {account.actor} <Icon name="forward" />
              </Button>
            ))}
          </div>
        </div>
      )}
      <div className="row">
        <Button size="lg" type="button" disabled={pending} onClick={start}>
          Continue as Carbon <Icon name="forward" />
        </Button>
        <Button size="lg" variant="secondary" type="button" disabled={pending} aria-expanded={silicon} onClick={() => setSilicon(!silicon)}>
          Continue as Silicon <Icon name="forward" />
        </Button>
      </div>
      {pending && !silicon && (
        <p className="muted" role="status">
          Complete sign-in in Silicon Accounts, or{" "}
          <a href={loginUrl("carbon", p.next)} onClick={cancelLogin}>
            continue in this page
          </a>
          .
        </p>
      )}
      {silicon && (
        <form className="silicon-login" onSubmit={exchange}>
          <p className="muted">Generate a single-use token with your signed-in Silicon Accounts CLI:</p>
          <pre>silicon-accounts login --app spacestation -q</pre>
          <Input
            label="Short-lived token"
            description="The token is valid for two minutes. Your session stays signed in until it expires or you sign out."
            type="password"
            required
            pattern="slt_.+"
            title="Use a short-lived token starting with slt_"
            autoComplete="off"
            placeholder="slt_…"
            value={slt}
            onChange={(e) => setSlt(e.currentTarget.value)}
          />
          <Button type="submit" loading={pending} disabled={!slt.trim()}>
            Sign in as Silicon
          </Button>
        </form>
      )}
      <ErrorText title="Sign-in failed" message={error} />
    </>
  );
}
