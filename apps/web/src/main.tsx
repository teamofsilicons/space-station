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
  createMemo,
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
export const loginUrl = (org: string, next: string) =>
  `/api/auth/login?org=${encodeURIComponent(org)}&next=${encodeURIComponent(next)}`;
/**
 * Browser IAM currently issues an org-bound session. Keep that protocol detail out of the
 * landing page: the app chooses the requested org (or the default workspace) and IAM handles
 * the carbon sign-in. Once signed in, `/orgs` supplies every organization known to the mirror.
 */
function Login(p: { bound?: Me; requestedOrg?: string }) {
  const org = () => p.requestedOrg || p.bound?.org || "tos";
  // On the landing route, return to the organization picker so `/orgs` can populate it.
  const destination = () => p.requestedOrg ? `/o/${org()}/tables` : "/";
  return (
    <main class="login landing">
      <div class="landing-art" aria-hidden="true"><span>╭────────────╮</span><span>│  ◌  ◌  ◌  │</span><span>│    ◇      │</span><span>╰────────────╯</span></div>
      <h1>Space Station<span class="pixel-dot">·</span></h1>
      <p class="landing-lede">A calm home for your records, live views, and notifications.</p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          location.assign(loginUrl(org(), destination()));
        }}
      >
        <button class="primary" type="submit">Log in with Silicon IAM</button>
      </form>
      <p>
        <A href="/docs">Read the docs →</A>
      </p>
    </main>
  );
}
function OrganizationPicker(p: { me: Me; orgs?: Org[] }) {
  const choices = createMemo(() => {
    const all = [{ id: p.me.org, name: p.me.org }, ...(p.orgs || [])];
    return all.filter((item, index) => all.findIndex((candidate) => candidate.id === item.id) === index);
  });
  return (
    <main class="org-picker">
      <h1>Choose an organization</h1>
      <p class="muted">Organizations available in Space Station.</p>
      <div class="org-list">
        <For each={choices()}>{(item) => (
          <a class="org-choice" href={item.id === p.me.org ? `/o/${item.id}/tables` : loginUrl(item.id, `/o/${item.id}/tables`)}>
            <strong>{item.name || item.id}</strong>
            <span>{item.id}</span>
          </a>
        )}</For>
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
  });
  const orgs = resource(async () => {
    if (publicPage()) return [];
    try {
      return await api<Org[]>("/orgs");
    } catch {
      return [];
    }
  });
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
  const availableOrgs = createMemo(() => {
    const values = [me.data()?.org, ...(orgs.data() || []).map((o) => o.id)].filter(Boolean) as string[];
    return [...new Set(values)];
  });
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
              <span class="actor">@{m().id}</span>
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
              <main>
                <Loading />
              </main>
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
              <Show when={me.data()} fallback={<Login requestedOrg={org()} />}>
                {(m) => (
                  <Show
                    when={org()}
                    fallback={<OrganizationPicker me={m()} orgs={orgs.data()} />}
                  >
                    <Show
                      when={org() === m().org}
                      fallback={
                        <main>
                          <p>This page belongs to {org()}.</p>
                          <a href={loginUrl(org(), loc.pathname)}>
                            Switch to {org()}
                          </a>
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
                            <select aria-label="Switch organization" value={m().org} onChange={(e) => location.assign(loginUrl(e.currentTarget.value, loc.pathname))}>
                              <For each={availableOrgs()}>{(id) => <option value={id}>{id === "carbon" ? "Carbon" : id}</option>}</For>
                            </select>
                          </div>
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
                          <A href="/">Switch organization</A>
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
    <Router root={Shell}>
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
