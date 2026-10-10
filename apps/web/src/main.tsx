// The app's front door: public docs and inspirations for anyone; otherwise the session decides —
// sign in, a page of another actor (offered as a switch), or the station itself.
import { render } from "solid-js/web";
import { createResource, Match, onCleanup, Switch } from "solid-js";
import { api, signedOut, type Me } from "../lib/api";
import { route } from "../lib/tabs";
import { Workspace } from "./workspace";
import { LoginButtons } from "./login";
import { Inspirations, PublicDocs } from "./docs";
import { Mark } from "./icons";
import "./theme";
import "./style.css";
import "./silicon-ui/foundation.css";
import "./refinement.css";

const orNull = async <T,>(call: Promise<T>, empty: T) => {
  try {
    return await call;
  } catch (e) {
    if ((e as { status?: number }).status === 401) return empty;
    throw e;
  }
};

function Login() {
  return (
    <div class="login">
      <div class="login-art" aria-hidden="true">
        <img src="/brand/iss-login.png" alt="" />
        <span>SPACE STATION · LOW ORBIT</span>
      </div>
      <div class="login-copy">
        <Mark size={34} />
        <h1>Space Station</h1>
        <p class="lede">Records, live views and notifications for carbons and silicons. Sign in with Silicon Accounts to continue.</p>
        <LoginButtons next={location.pathname + location.search + location.hash} />
        <p class="muted small-print">
          New here? Read <a href="/docs/getting-started">getting started</a>.
        </p>
      </div>
    </div>
  );
}

function Session() {
  const [me, { refetch }] = createResource(() => orNull(api<Me>("/me"), null));
  const out = () => refetch();
  signedOut.addEventListener("signedout", out);
  onCleanup(() => signedOut.removeEventListener("signedout", out));
  const path = location.pathname;
  const wanted = route(path)?.actor;
  return (
    <Switch>
      <Match when={me.loading}>
        <div class="splash" role="status">
          <Mark size={28} />
        </div>
      </Match>
      <Match when={me.error}>
        <div class="center-card">
          <h1>The station is not answering</h1>
          <p class="error">{(me.error as Error).message}</p>
          <button class="primary" onClick={() => (me.error as {status?: number}).status === 409 ? location.reload() : refetch()}>
            Retry
          </button>
        </div>
      </Match>
      <Match when={!me()}>
        <Login />
      </Match>
      <Match when={wanted && wanted !== me()!.uuid}>
        <div class="center-card">
          <span class="kicker">Another account</span>
          <h1>This page belongs to another account</h1>
          <p class="muted">
            You are signed in to {me()!.id}. Switch to the page's account to open it. Your saved accounts stay available in the account switcher.
          </p>
          <div class="row">
            <LoginButtons next={path} />
            <a class="button" href={`/a/${me()!.uuid}`}>
              Return to {me()!.id}
            </a>
          </div>
        </div>
      </Match>
      <Match when={me()}>{(m) => <Workspace me={m()} path={path} />}</Match>
    </Switch>
  );
}

function App() {
  const path = location.pathname;
  if (path === "/docs" || path.startsWith("/docs/")) return <PublicDocs />;
  if (path === "/inspirations") return <Inspirations />;
  return <Session />;
}

render(() => <App />, document.getElementById("root")!);
