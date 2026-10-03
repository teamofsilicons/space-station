import { test } from "node:test";
import assert from "node:assert/strict";
import { beginLogin, cancelLogin, loginUrl } from "./login.ts";

test("popup login is bound to its origin, window and one-use attempt; fallback and cancellation do not report success", async (t) => {
  const origin = "https://station.example";
  let opened = "", blocked = false, reloads = 0, poll: () => void, timeout: () => void;
  const destinations: string[] = [];
  let popup = { closed: false, close() { this.closed = true; } };
  const browser = Object.assign(new EventTarget(), {
    location: { origin, pathname: "/", search: "", hash: "", reload: () => reloads++, assign: (path: string) => destinations.push(path) },
    open(url: string) {
      opened = url;
      popup = { closed: false, close() { this.closed = true; } };
      return blocked ? null : popup;
    },
  });
  Object.defineProperty(globalThis, "window", { value: browser, configurable: true });
  t.after(() => Reflect.deleteProperty(globalThis, "window"));
  t.mock.method(globalThis, "setInterval", (fn: () => void) => { poll = fn; return 1; });
  t.mock.method(globalThis, "setTimeout", (fn: () => void) => { timeout = fn; return 2; });
  t.mock.method(globalThis, "clearInterval", () => {});
  t.mock.method(globalThis, "clearTimeout", () => {});
  const send = (data: unknown, eventOrigin = origin, source: unknown = popup) => {
    const event = Object.assign(new Event("message"), { data, origin: eventOrigin, source });
    browser.dispatchEvent(event);
  };
  const message = (status = "success") => ({ type: "spacestation:login", attempt_id: new URL(opened, origin).searchParams.get("attempt_id"), status });

  const first = beginLogin("silicon", "/o/tos/windows", "tos");
  const query = new URL(opened, origin).searchParams;
  assert.equal(query.get("identity_kind"), "silicon");
  assert.equal(query.get("display"), "popup");
  assert.equal(query.get("org"), "tos");
  assert.match(query.get("attempt_id")!, /^[0-9a-f-]{36}$/);
  const success = message();
  send(success, "https://attacker.example");
  send(success, origin, {});
  send({ ...success, attempt_id: "a different attempt" });
  send({ ...success, status: "unknown" });
  send({ ...success, access_token: "must never be a window message" });
  send(null);
  assert.deepEqual(destinations, []);
  send(success);
  await first;
  assert.deepEqual(destinations, ["/o/tos/windows"]);
  send(success);
  assert.equal(destinations.length, 1, "completion is consumed only once");

  const samePage = beginLogin("carbon", "/#tables");
  send(message());
  await samePage;
  assert.equal(reloads, 1, "a return within the same document reloads authenticated state");
  assert.equal(browser.location.hash, "#tables");

  const closed = assert.rejects(beginLogin("carbon"), /cancelled/);
  popup.closed = true;
  poll!();
  await closed;
  const timedOut = assert.rejects(beginLogin("carbon"), /timed out/);
  timeout!();
  await timedOut;
  const denied = assert.rejects(beginLogin("carbon"), /not completed/);
  send(message("error"));
  await denied;
  const cancelled = assert.rejects(beginLogin("carbon"), /cancelled/);
  cancelLogin();
  await cancelled;
  assert.equal(destinations.length, 1, "cancellation never navigates to authenticated state");

  blocked = true;
  await beginLogin("carbon", "/o/tos", "tos");
  const fallback = new URL(destinations[1], origin);
  assert.equal(fallback.searchParams.get("identity_kind"), "carbon");
  assert.equal(fallback.searchParams.get("next"), "/o/tos");
  assert.equal(fallback.searchParams.has("attempt_id"), false);
  assert.equal(fallback.searchParams.has("display"), false);
  for (const unsafe of ["//attacker.example", "/\\attacker.example", "https://attacker.example", "/\n/attacker.example"])
    await assert.rejects(beginLogin("carbon", unsafe), /return destination/);
  assert.equal(destinations.length, 2);
  assert.equal(new URL(loginUrl("silicon", "/o/a?x=a b", "a b"), origin).searchParams.get("next"), "/o/a?x=a b");
});
