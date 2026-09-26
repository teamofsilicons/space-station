// The tab model: which page a path is, how a click opens it (here, a new tab, behind, a split),
// which tab takes over when one closes, pinning and moving, a tab's own history, and what a
// reload keeps of the strip.
import { test } from "node:test";
import assert from "node:assert/strict";
import { active, arrive, close, duplicate, initial, move, open, pin, reopen, restore, route, save, seek, show, split, unsplit, walk, type Tabs } from "./tabs.ts";

const paths = (s: Tabs) => s.tabs.map((t) => t.path);
const shown = (s: Tabs) => s.panes.map((id) => s.tabs.find((t) => t.id === id)!.path);

test("routing: every org page, and nothing else", () => {
  assert.deepEqual(route("/o/tos"), { kind: "home", org: "tos", id: "" });
  assert.deepEqual(route("/o/tos/tables/orders/"), { kind: "table", org: "tos", id: "orders" });
  assert.deepEqual(route("/o/tos/windows/w1/code?x=1"), { kind: "code", org: "tos", id: "w1" });
  assert.deepEqual(route("/o/tos/docs/cli"), { kind: "docs", org: "tos", id: "cli" });
  assert.equal(route("/o/tos/tables/Orders"), null, "a table id is lowercase");
  assert.equal(route("/docs/cli"), null, "public docs are not a workspace page");
  assert.equal(route("/"), null);
});

test("opening: here navigates, a new tab lines up behind its opener, background stays behind", () => {
  let s = initial("/o/tos/tables");
  s = open(s, "/o/tos/tables/orders", "here");
  assert.deepEqual(paths(s), ["/o/tos/tables/orders"]);
  s = open(s, "/o/tos/windows", "tab", true);
  s = show(s, s.tabs[0].id);
  s = open(s, "/o/tos/tables/a", "background");
  s = open(s, "/o/tos/tables/b", "background");
  assert.deepEqual(paths(s), ["/o/tos/tables/orders", "/o/tos/tables/a", "/o/tos/tables/b", "/o/tos/windows"]);
  assert.equal(active(s)!.path, "/o/tos/tables/orders", "background tabs do not take focus");
  s = open(s, "/o/tos/settings", "tab");
  assert.equal(active(s)!.path, "/o/tos/settings");
  assert.equal(paths(s).indexOf("/o/tos/settings"), 3, "after the tabs it already opened");
});

test("splitting: up to three panes, then the right pane is reused; unsplit keeps the tab", () => {
  let s = initial("/o/tos/tables");
  s = open(s, "/o/tos/windows", "split");
  s = open(s, "/o/tos/settings", "split");
  assert.deepEqual(shown(s), ["/o/tos/tables", "/o/tos/windows", "/o/tos/settings"]);
  assert.equal(s.focus, 2);
  s = open(s, "/o/tos/notifications", "split");
  assert.deepEqual(shown(s), ["/o/tos/tables", "/o/tos/windows", "/o/tos/notifications"]);
  s = unsplit(s, 1);
  assert.deepEqual(shown(s), ["/o/tos/tables", "/o/tos/notifications"]);
  assert.equal(s.focus, 1);
  assert.equal(s.tabs.length, 4, "the unsplit tab is still open");
  s = split(s, s.tabs.find((t) => t.path === "/o/tos/tables")!.id);
  assert.deepEqual(shown(s), ["/o/tos/notifications", "/o/tos/tables"], "a shown tab moves rather than doubles");
});

test("closing: the right neighbour takes over, a split pane folds, the last tab leaves a new one", () => {
  let s = initial("/o/tos/a");
  s = open(s, "/o/tos/tables", "tab", true);
  s = open(s, "/o/tos/windows", "tab", true);
  s = show(s, s.tabs[1].id);
  s = close(s, [s.tabs[1].id], "/o/tos");
  assert.equal(active(s)!.path, "/o/tos/windows");
  s = close(s, [active(s)!.id], "/o/tos");
  assert.equal(active(s)!.path, "/o/tos/a", "with nothing to the right, the left");
  s = open(s, "/o/tos/settings", "split");
  s = close(s, [active(s)!.id], "/o/tos");
  assert.deepEqual(shown(s), ["/o/tos/a"]);
  s = close(s, [active(s)!.id], "/o/tos");
  assert.deepEqual(paths(s), ["/o/tos"]);
  s = reopen(s);
  assert.equal(active(s)!.path, "/o/tos/a", "the last closed comes back first");
});

test("pinning and moving: pinned tabs stay first, duplicates sit beside the original", () => {
  let s = initial("/o/tos/a");
  s = open(s, "/o/tos/tables", "tab", true);
  s = open(s, "/o/tos/windows", "tab", true);
  s = pin(s, s.tabs[2].id);
  assert.deepEqual(paths(s), ["/o/tos/windows", "/o/tos/a", "/o/tos/tables"]);
  s = move(s, s.tabs[2].id, 0);
  assert.deepEqual(paths(s), ["/o/tos/windows", "/o/tos/tables", "/o/tos/a"], "nothing moves ahead of a pinned tab");
  s = duplicate(s, s.tabs[1].id);
  assert.deepEqual(paths(s), ["/o/tos/windows", "/o/tos/tables", "/o/tos/tables", "/o/tos/a"]);
  assert.equal(active(s)!.id, s.tabs[2].id);
});

test("history: each tab walks its own back and forward", () => {
  let s = initial("/o/tos/tables");
  s = open(s, "/o/tos/tables/orders", "here");
  s = open(s, "/o/tos/tables/signups", "here");
  const id = active(s)!.id;
  s = walk(s, id, -2);
  assert.equal(active(s)!.path, "/o/tos/tables");
  s = walk(s, id, 1);
  assert.equal(active(s)!.path, "/o/tos/tables/orders");
  s = open(s, "/o/tos/windows", "here");
  assert.deepEqual(active(s)!.forward, [], "a new navigation drops the forward stack");
  assert.equal(walk(s, id, 5).tabs[0], s.tabs[0], "walking past the end changes nothing");
});

test("seeking: browser back/forward find a path in the tab's own history; an unknown one is a redirect", () => {
  let s = initial("/o/tos/tables");
  for (const p of ["/o/tos/tables/a", "/o/tos/tables/b", "/o/tos/tables/c"]) s = open(s, p, "here");
  const id = active(s)!.id;
  s = seek(s, id, "/o/tos/tables/a", true);
  assert.equal(active(s)!.path, "/o/tos/tables/a");
  assert.deepEqual(active(s)!.forward, ["/o/tos/tables/b", "/o/tos/tables/c"]);
  s = seek(s, id, "/o/tos/tables/c", false);
  assert.equal(active(s)!.path, "/o/tos/tables/c");
  const before = active(s)!.back;
  s = seek(s, id, "/o/tos/windows", true);
  assert.equal(active(s)!.path, "/o/tos/windows");
  assert.deepEqual(active(s)!.back, before, "a redirect leaves history alone");
  assert.equal(seek(s, id, "/o/tos/windows", true), s, "already there changes nothing");
});

test("reload: the strip and panes survive for the same org only; the address bar is honoured", () => {
  let s = initial("/o/tos/tables");
  s = open(s, "/o/tos/windows", "split");
  s = pin(s, s.tabs[1].id);
  const back = restore(save(s), "tos")!;
  assert.deepEqual(paths(back), ["/o/tos/windows", "/o/tos/tables"]);
  assert.deepEqual(back.panes, s.panes);
  assert.equal(restore(save(s), "acme"), null, "another org's strip is not this one's");
  assert.equal(restore("{not json", "tos"), null);
  let r = arrive(back, "/o/tos/tables");
  assert.equal(active(r)!.path, "/o/tos/tables", "an open tab is shown, not opened twice");
  r = arrive(back, "/o/tos");
  assert.equal(r, back, "the org root keeps whatever was open");
  r = arrive(back, "/o/tos/tables/orders");
  assert.equal(active(r)!.path, "/o/tos/tables/orders");
});
