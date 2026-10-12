"use client";
// The new tab page: search everything, or pick a window, a table or a page to open in this tab.
import { Finder } from "./palette";
import { Link, useLists, useWorkspace } from "./tabs";
import { count, useTab, useTitle } from "./ui";
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
  const { tables, windows } = useLists();
  const tab = useTab();
  useTitle("New tab");
  const o = `/a/${ws.actor}`;
  const hour = new Date().getHours();
  return (
    <div className="page-body home">
      <div className="home-hero">
        <Mark size={30} />
        <h1>
          {hour < 5 ? "Night shift" : hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening"}, {ws.me.id.replace(/^(c|si):/, "")}
        </h1>
        <p className="muted">You are signed in as {ws.me.id}. Open anything in this tab, or ⌘-click to keep it for later.</p>
        <div className="home-search">
          <Finder autoFocus={tab.visible} placeholder="Search windows, tables, docs and actions" />
        </div>
      </div>
      <section className="home-section">
        <div className="section-head">
          <span className="kicker">Space Windows</span>
          <Link href={`${o}/windows`} className="more-link">
            All windows
          </Link>
        </div>
        {windows.data?.length ? (
          <div className="tiles">
            {windows.data.slice(0, 8).map((w) => (
              <Link key={w.id} href={`${o}/windows/${w.id}`} className="tile">
                <span className="title-icon">
                  <Icon name="window" />
                </span>
                <span className="tile-text">
                  <strong>{w.name}</strong>
                  <small className="muted">{w.version ? `Live · ${w.version.name}` : "No version yet"}</small>
                </span>
                {w.version && <i className="live-dot ok" />}
              </Link>
            ))}
          </div>
        ) : (
          <button className="tile add" onClick={() => ws.create("window")}>
            <Icon name="plus" /> Create your first Space Window
          </button>
        )}
      </section>
      <section className="home-section">
        <div className="section-head">
          <span className="kicker">Tables</span>
          <Link href={`${o}/tables`} className="more-link">
            Overview
          </Link>
        </div>
        {tables.data?.length ? (
          <div className="tiles">
            {tables.data.slice(0, 12).map((t) => (
              <Link key={t.id} href={`${o}/tables/${t.id}`} className="tile">
                <span className="title-icon">
                  <Icon name="table" />
                </span>
                <span className="tile-text">
                  <strong className="mono">{t.id}</strong>
                  <small className="muted">{count(t.records)} records</small>
                </span>
              </Link>
            ))}
          </div>
        ) : (
          <button className="tile add" onClick={() => ws.create("table")}>
            <Icon name="plus" /> Create your first table
          </button>
        )}
      </section>
      <section className="home-section">
        <div className="section-head">
          <span className="kicker">Jump to</span>
        </div>
        <div className="tiles compact">
          <Link href={`${o}/notifications`} className="tile">
            <Icon name="bell" /> Notifications
          </Link>
          <Link href={`${o}/settings`} className="tile">
            <Icon name="settings" /> Settings
          </Link>
          <Link href={`${o}/docs`} className="tile">
            <Icon name="docs" /> Documentation
          </Link>
          <button className="tile" onClick={() => ws.create("table")}>
            <Icon name="plus" /> New table
          </button>
          <button className="tile" onClick={() => ws.create("window")}>
            <Icon name="plus" /> New window
          </button>
        </div>
      </section>
      <section className="home-section">
        <div className="section-head">
          <span className="kicker">Keyboard</span>
        </div>
        <dl className="shortcuts">
          {SHORTCUTS.map(([k, what]) => (
            <div key={k}>
              <dt>
                <kbd>{k}</kbd>
              </dt>
              <dd>{what}</dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  );
}
