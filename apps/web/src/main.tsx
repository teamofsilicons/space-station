import { TableView } from "./table-view";
import { render } from "solid-js/web";
import {
  Router,
  Route,
  A,
  Navigate,
  useLocation,
  useParams,
} from "@solidjs/router";
import {
  createSignal,
  onCleanup,
  Show,
  For,
  ErrorBoundary,
  type ParentProps,
} from "solid-js";
import { api, signedOut, type Me, type Org, type DevError } from "../lib/api";
import { action, ErrorText, resource, Loading, when } from "./ui";
import {
  Tables,
  Windows,
  WindowView,
  Notifications,
  Settings,
  Docs,
  Inspirations,
  AccessToken,
} from "./pages";
import "./style.css";
export const loginUrl = (next = "/", org?: string) =>
  `/api/auth/login?next=${encodeURIComponent(next)}${org ? `&org=${encodeURIComponent(org)}` : ""}`;
function Login() {
  return (
    <main class="login landing">
      <div class="landing-art" aria-hidden="true">
        <img src="/brand/iss-login.png" alt="" />
      </div>
      <div class="landing-copy">
        <h1>Space Station<span class="pixel-dot">·</span></h1>
        <p class="landing-lede">Choose your organization in Silicon IAM to continue.</p>
        <button class="primary login-link" type="button" onClick={() => location.assign(loginUrl())}>Log in with Silicon IAM</button>
      </div>
    </main>
  );
}
function Shell(p: ParentProps) {
  const loc = useLocation();
  const publicPage = () =>
    loc.pathname.startsWith("/docs") || loc.pathname.startsWith("/inspirations");
  const me = resource(async () => {
    if (publicPage()) return null;
    try {
      return await api<Me>("/me");
    } catch (e) {
      if ((e as { status: number }).status === 401) return null;
      throw e;
    }
  // Solid disables resource fetching when the source is false; both page kinds must run.
  }, undefined, () => publicPage() ? "public" : "session");
  const orgs = resource(async () => {
    if (publicPage()) return [] as Org[];
    try {
      return await api<Org[]>("/orgs");
    } catch (e) {
      if ((e as { status: number }).status === 401) return [];
      throw e;
    }
  }, undefined, () => publicPage() ? "public" : "session");
  const logout = action();
  const [debug, setDebug] = createSignal(false);
  const signout = () => me.reload();
  signedOut.addEventListener("signedout", signout);
  onCleanup(() => signedOut.removeEventListener("signedout", signout));
  const keyboard = (e: KeyboardEvent) => {
    if (e.altKey && e.shiftKey && e.code === "KeyD") {
      e.preventDefault();
      setDebug(!debug());
    }
  };
  window.addEventListener("keydown", keyboard);
  onCleanup(() => window.removeEventListener("keydown", keyboard));
  const org = () => loc.pathname.split("/")[2];
  const docs = publicPage;
  const [collapsed, setCollapsed] = createSignal(localStorage.getItem("ss-sidebar") === "collapsed");
  const toggleSidebar = () => {
    const next = !collapsed();
    setCollapsed(next);
    localStorage.setItem("ss-sidebar", next ? "collapsed" : "open");
  };
  return (
    <>
      <header class="app-header">
        <A class="brand" href="/">
          <img src="/brand/mark.svg" alt="" width="28" height="28" />
          Space Station
        </A>
        <nav class="header-nav" aria-label="Utility navigation">
          <A href="/docs">Docs</A>
          <A href="/inspirations">Inspirations</A>
        </nav>
        <span class="header-status"><i class="status-dot" aria-hidden="true" />STATION ONLINE</span>
        <Show when={me.data()}>
          {(m) => (
            <>
              <span class="actor">@{m().id} · {m().org}</span>
              <button
                disabled={logout.busy()}
                onClick={() =>
                  logout.run(async () => {
                    await api("/auth/logout", "POST");
                    location.assign("/");
                  })
                }
              >
                Sign out
              </button>
            </>
          )}
        </Show>
      </header>
      <ErrorText message={logout.error()} />
      <Show
        when={docs()}
        fallback={
          <Show
            when={!me.data.loading}
            fallback={
              <Login />
            }
          >
            <Show
              when={!me.data.error}
              fallback={
                <main>
                  <Loading error={me.data.error} />
                  <button onClick={() => me.reload()}>Retry</button>
                </main>
              }
            >
              <Show when={me.data()} fallback={<Login />}>
                {(m) => (
                  <Show
                    when={org()}
                    fallback={<Navigate href={`/o/${m().org}/tables`} />}
                  >
                    <Show
                      when={org() === m().org}
                      fallback={
                        <main>
                          <p>This page belongs to {org()}.</p>
                          <p>You are signed in to {m().org}. Sign out to use another organization.</p>
                          <A href={`/o/${m().org}/tables`}>Return to {m().org}</A>
                        </main>
                      }
                    >
                      <div class={`workspace ${collapsed() ? "sidebar-collapsed" : ""}`}>
                        <aside class="sidebar">
                          <button class="sidebar-toggle" type="button" onClick={toggleSidebar} aria-label={collapsed() ? "Expand sidebar" : "Collapse sidebar"}>
                            {collapsed() ? "→" : "←"}
                          </button>
                          <div class="org-switcher">
                            <span class="org-glyph">◎</span>
                            <strong>Organizations</strong>
                            <a class="org-add" href={loginUrl()} aria-label="Attach another organization">+</a>
                          </div>
                          <nav class="org-list-nav" aria-label="Organizations">
                            <For each={(() => {
                              const rows = orgs.data() || [];
                              return rows.some((o) => o.id === m().org)
                                ? rows
                                : [{ id: m().org, name: m().org }, ...rows];
                            })()}>
                              {(o) => (
                                <a
                                  href={loginUrl(`/o/${o.id}/tables`, o.id)}
                                  class={o.id === m().org ? "selected" : ""}
                                >
                                  {o.name || o.id}
                                </a>
                              )}
                            </For>
                          </nav>
                          <div class="sidebar-caption">Flight deck</div>
                          <nav>
                            <For
                              each={[
                                "tables",
                                "windows",
                                "notifications",
                                "settings",
                              ]}
                            >
                              {(tab) => (
                                <A
                                  href={`/o/${m().org}/${tab}`}
                                  activeClass="selected"
                                >
                                  {tab === "windows"
                                    ? "Space Windows"
                                    : tab[0].toUpperCase() + tab.slice(1)}
                                </A>
                              )}
                            </For>
                          </nav>
                          <details class="sidebar-token">
                            <summary>Access token</summary>
                            <AccessToken root={`/orgs/${encodeURIComponent(m().org)}`} />
                          </details>
                          <div class="sidebar-caption">Reference</div>
                          <div class="sidebar-foot">
                            <span class="muted">Telemetry is flowing.</span>
                            <span class="muted">⌘ K to search</span>
                          </div>
                          <button
                            class="quiet"
                            onClick={() => setDebug(!debug())}
                          >
                            Developer errors
                          </button>
                        </aside>
                        <main class="paper">
                          <ErrorBoundary
                            fallback={(e, reset) => (
                              <>
                                <ErrorText message={e.message} />
                                <button onClick={reset}>Try again</button>
                              </>
                            )}
                          >
                            {p.children}
                          </ErrorBoundary>
                          <Show when={debug()}>
                            <Debug org={m().org} />
                          </Show>
                        </main>
                      </div>
                    </Show>
                  </Show>
                )}
              </Show>
            </Show>
          </Show>
        }
      >
        {p.children}
      </Show>
    </>
  );
}
function Debug(p: { org: string }) {
  const errors = resource(
    () => api<DevError[]>(`/orgs/${encodeURIComponent(p.org)}/dev-errors`),
    5000,
  );
  return (
    <section>
      <h2>Developer errors</h2>
      <Show
        when={errors.data()}
        fallback={<Loading error={errors.data.error} />}
      >
        {(rows) => (
          <>
            <Show when={!rows().length}>
              <p class="muted">No server errors.</p>
            </Show>
            <For each={rows()}>
              {(e) => (
                <details>
                  <summary>
                    {e.source}: {e.message}
                  </summary>
                  <p>{when(e.created_at)}</p>
                  <pre>{JSON.stringify(e.detail, null, 2)}</pre>
                </details>
              )}
            </For>
          </>
        )}
      </Show>
    </section>
  );
}
function OrgHome() {
  const p = useParams();
  return <Navigate href={`/o/${p.org}/tables`} />;
}
render(
  () => (
    <Router root={Shell} explicitLinks>
      <Route path="/" component={() => null} />
      <Route path="/o/:org" component={OrgHome} />
      <Route path="/o/:org/tables" component={Tables} />
      <Route path="/o/:org/tables/:id" component={TableView} />
      <Route path="/o/:org/windows" component={Windows} />
      <Route path="/o/:org/windows/:id/code" component={WindowView} />
      <Route path="/o/:org/windows/:id" component={WindowView} />
      <Route path="/o/:org/notifications" component={Notifications} />
      <Route path="/o/:org/settings" component={Settings} />
      <Route path="/docs/:slug?" component={Docs} />
      <Route path="/inspirations" component={Inspirations} />
      <Route
        path="*"
        component={() => (
          <main>
            <h1>Page not found</h1>
            <A href="/">Return home</A>
          </main>
        )}
      />
    </Router>
  ),
  document.getElementById("root")!,
);
