// The new tab page: search everything, or pick a window, a table or a page to open in this tab.
import { For, Show } from "solid-js";
import { Finder } from "./palette";
import { Link, useWorkspace } from "./tabs";
import { count, useTab } from "./ui";
import { Icon, Mark } from "./icons";

const SHORTCUTS = [
  ["⌘K", "Search and jump"],
  ["⌥T", "New tab"],
  ["⌥W", "Close tab"],
  ["⌥⇧T", "Reopen closed tab"],
  ["⌥1–9", "Go to tab"],
  ["⌥[ ⌥]", "Previous / next tab"],
  ["⌥← ⌥→", "Back / forward in tab"],
  ["⌥\\", "Split view"],
  ["⌥⇧D", "Developer errors"],
  ["⌘-click", "Open behind"],
  ["⇧-click", "Open in split"],
] as const;

export function Home() {
  const ws = useWorkspace();
  const tab = useTab();
  tab.title("New tab");
  const o = `/o/${ws.org}`;
  const hour = new Date().getHours();
  return (
    <div class="page-body home">
      <div class="home-hero">
        <Mark size={30} />
        <h1>
          {hour < 5 ? "Night shift" : hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening"}, {ws.me.id.replace(/^(c|si):/, "")}
        </h1>
        <p class="muted">You are aboard {ws.org}. Open anything in this tab, or ⌘-click to keep it for later.</p>
        <div class="home-search">
          <Finder autofocus={tab.visible()} empty={false} placeholder="Search windows, tables, docs and actions" />
        </div>
      </div>
      <section class="home-section">
        <div class="section-head">
          <span class="kicker">Space Windows</span>
          <Link href={`${o}/windows`} class="more-link">All windows</Link>
        </div>
        <Show when={ws.windows.data()?.length} fallback={<button class="tile add" onClick={() => ws.create("window")}><Icon name="plus" /> Create your first Space Window</button>}>
          <div class="tiles">
            <For each={ws.windows.data()!.slice(0, 8)}>
              {(w) => (
                <Link href={`${o}/windows/${w.id}`} class="tile">
                  <span class="title-icon"><Icon name="window" /></span>
                  <span class="tile-text">
                    <strong>{w.name}</strong>
                    <small class="muted">{w.version ? `Live · ${w.version.name}` : "No version yet"}</small>
                  </span>
                  <Show when={w.version}><i class="live-dot ok" /></Show>
                </Link>
              )}
            </For>
          </div>
        </Show>
      </section>
      <section class="home-section">
        <div class="section-head">
          <span class="kicker">Tables</span>
          <Link href={`${o}/tables`} class="more-link">Overview</Link>
        </div>
        <Show when={ws.tables.data()?.length} fallback={<button class="tile add" onClick={() => ws.create("table")}><Icon name="plus" /> Create your first table</button>}>
          <div class="tiles">
            <For each={ws.tables.data()!.slice(0, 12)}>
              {(t) => (
                <Link href={`${o}/tables/${t.id}`} class="tile">
                  <span class="title-icon"><Icon name="table" /></span>
                  <span class="tile-text">
                    <strong class="mono">{t.id}</strong>
                    <small class="muted">{count(t.records)} records</small>
                  </span>
                </Link>
              )}
            </For>
          </div>
        </Show>
      </section>
      <section class="home-section">
        <div class="section-head">
          <span class="kicker">Jump to</span>
        </div>
        <div class="tiles compact">
          <Link href={`${o}/notifications`} class="tile"><Icon name="bell" /> Notifications</Link>
          <Link href={`${o}/settings`} class="tile"><Icon name="settings" /> Settings</Link>
          <Link href={`${o}/docs`} class="tile"><Icon name="docs" /> Documentation</Link>
          <button class="tile" onClick={() => ws.create("table")}><Icon name="plus" /> New table</button>
          <button class="tile" onClick={() => ws.create("window")}><Icon name="plus" /> New window</button>
        </div>
      </section>
      <section class="home-section">
        <div class="section-head">
          <span class="kicker">Keyboard</span>
        </div>
        <dl class="shortcuts">
          <For each={SHORTCUTS}>
            {([k, what]) => (
              <div>
                <dt><kbd>{k}</kbd></dt>
                <dd>{what}</dd>
              </div>
            )}
          </For>
        </dl>
      </section>
    </div>
  );
}
