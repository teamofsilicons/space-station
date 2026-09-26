// The app's front door: public docs and inspirations for anyone; otherwise the session decides —
// sign in, a page of another org (offered as a switch), or the station itself.
import { render } from "solid-js/web";
import { createResource, Match, onCleanup, Switch } from "solid-js";
import { api, signedOut, type Me, type Org } from "../lib/api";
import { route } from "../lib/tabs";
import { Workspace, loginUrl } from "./workspace";
import { Inspirations, PublicDocs } from "./docs";
import { Icon, Mark } from "./icons";
import "./theme";
import "./style.css";

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
        <p class="lede">Records, live views and notifications for carbons and silicons. Choose your organization in Silicon IAM to continue.</p>
        <button class="primary large" type="button" onClick={() => location.assign(loginUrl())}>
          Log in with Silicon IAM <Icon name="forward" />
        </button>
        <p class="muted small-print">
          New here? Read <a href="/docs/getting-started">getting started</a>.
        </p>
      </div>
    </div>
  );
}

function Session() {
  const [me, { refetch }] = createResource(() => orNull(api<Me>("/me"), null));
  const [orgs] = createResource(() => orNull(api<Org[]>("/orgs"), [] as Org[]));
  const out = () => refetch();
  signedOut.addEventListener("signedout", out);
  onCleanup(() => signedOut.removeEventListener("signedout", out));
  const path = location.pathname;
  const wanted = route(path)?.org;
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
          <button class="primary" onClick={() => refetch()}>
            Retry
          </button>
        </div>
      </Match>
      <Match when={!me()}>
        <Login />
      </Match>
      <Match when={wanted && wanted !== me()!.org}>
        <div class="center-card">
          <span class="kicker">Another organization</span>
          <h1>This page belongs to {wanted}</h1>
          <p class="muted">
            You are signed in to {me()!.org}. A session holds one organization at a time; switching signs you in to {wanted} through Silicon IAM.
          </p>
          <div class="row">
            <a class="button primary" href={loginUrl(path, wanted)}>
              Switch to {wanted}
            </a>
            <a class="button" href={`/o/${me()!.org}`}>
              Return to {me()!.org}
            </a>
          </div>
        </div>
      </Match>
      <Match when={me()}>{(m) => <Workspace me={m()} orgs={() => orgs() || []} path={path} />}</Match>
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

