"use client";
// The app's front door once the browser has it: the session decides — sign in, a page of another
// actor (offered as a switch), or the station itself. Public docs and inspirations are their own routes.
import { useEffect, useState } from "react";
import { Button } from "@/components/silicon-ui/button/button";
import { api, signedOut, type Me } from "../lib/api";
import { route } from "../lib/tabs";
import type { DocEntry } from "./docs-content";
import { DocsContext } from "./docs";
import { Workspace } from "./workspace";
import { LoginButtons } from "./login";
import { ErrorText } from "./ui";
import { Mark } from "./icons";

/** Who is signed in; null for nobody. Checks that overlap share one request, since the first `/me` fixes the page's account. */
let checking: Promise<Me | null> | undefined;
const whoAmI = () =>
  (checking ??= api<Me>("/me")
    .catch((e) => {
      if ((e as { status?: number }).status === 401) return null;
      throw e;
    })
    .finally(() => (checking = undefined)));

function Splash() {
  return (
    <div className="splash" role="status">
      <Mark size={28} />
    </div>
  );
}

function Login() {
  return (
    <div className="login">
      <div className="login-art" aria-hidden="true">
        <img src="/brand/iss-login.png" alt="" />
        <span>SPACE STATION · LOW ORBIT</span>
      </div>
      <div className="login-copy">
        <Mark size={34} />
        <h1>Space Station</h1>
        <p className="lede">Records, live views and notifications for carbons and silicons. Sign in with Silicon Accounts to continue.</p>
        <LoginButtons next={location.pathname + location.search + location.hash} />
        <p className="muted small-print">
          New here? Read <a href="/docs/getting-started">getting started</a>.
        </p>
      </div>
    </div>
  );
}

function Session() {
  const [me, setMe] = useState<{ me?: Me | null; error?: Error & { status?: number } }>({});
  const [checks, setChecks] = useState(0);
  const [path] = useState(() => location.pathname);
  useEffect(() => {
    let live = true;
    whoAmI().then(
      (m) => live && setMe({ me: m }),
      (error) => live && setMe({ error }),
    );
    return () => void (live = false);
  }, [checks]);
  useEffect(() => {
    const out = () => setChecks((n) => n + 1);
    signedOut.addEventListener("signedout", out);
    return () => signedOut.removeEventListener("signedout", out);
  }, []);
  if (me.error)
    return (
      <div className="center-card">
        <h1>The station is not answering</h1>
        <ErrorText message={me.error.message} />
        <Button onClick={() => (me.error!.status === 409 ? location.reload() : (setMe({}), setChecks((n) => n + 1)))}>Retry</Button>
      </div>
    );
  if (me.me === undefined) return <Splash />;
  if (!me.me) return <Login />;
  const wanted = route(path)?.actor;
  if (wanted && wanted !== me.me.uuid)
    return (
      <div className="center-card">
        <span className="kicker">Another account</span>
        <h1>This page belongs to another account</h1>
        <p className="muted">You are signed in to {me.me.id}. Switch to the page&apos;s account to open it. Your saved accounts stay available in the account switcher.</p>
        <div className="row">
          <LoginButtons next={path} />
          <a className="button" href={`/a/${me.me.uuid}`}>
            Return to {me.me.id}
          </a>
        </div>
      </div>
    );
  return <Workspace me={me.me} path={path} />;
}

/** The station needs the browser (location, storage, cookies), so the server sends the splash and the browser takes over. */
export function App(p: { docs: DocEntry[] }) {
  const [ready, setReady] = useState(false);
  useEffect(() => setReady(true), []);
  return <DocsContext.Provider value={p.docs}>{ready ? <Session /> : <Splash />}</DocsContext.Provider>;
}
