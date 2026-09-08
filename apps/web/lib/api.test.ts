// The api() helper: how a backend error and a bare status (the rewrite answering for a backend
// that did not) become ApiError; and where wsUrl sends the socket in development, in production
// and under the explicit override; the table id shape the create form checks; the 401 event.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ApiError, TABLE_ID, api, signedOut, wsUrl } from "./api.ts";

const answering = (status: number, body: string | null, type = "text/plain") => {
  globalThis.fetch = async () => new Response(body, { status, headers: { "content-type": type } });
};
const fails = (call: Promise<unknown>) => call.then(() => assert.fail("resolved"), (e: ApiError) => e);

test("a backend error keeps its code, message and status", async () => {
  answering(403, JSON.stringify({ error: { code: "not_a_member", message: "not a member of this org" } }), "application/json");
  const err = await fails(api("/orgs/tos/tables"));
  assert.ok(err instanceof ApiError);
  assert.deepEqual([err.code, err.message, err.status], ["not_a_member", "not a member of this org", 403]);
});

test("a status without the {error} envelope means the backend did not answer", async () => {
  answering(500, "Internal Server Error");
  const err = await fails(api("/me"));
  assert.deepEqual([err.code, err.message, err.status], ["backend_unreachable", "the backend did not answer", 500]);
});

test("a 401 from any call fires the signed-out event before it throws", async () => {
  let fired = 0;
  const on = () => fired++;
  signedOut.addEventListener("signedout", on);
  answering(401, JSON.stringify({ error: { code: "unauthorized", message: "log in" } }), "application/json");
  const err = await fails(api("/orgs/tos/tables/overview?window=5h"));
  signedOut.removeEventListener("signedout", on);
  assert.equal(fired, 1);
  assert.equal(err.status, 401);
  answering(403, JSON.stringify({ error: { code: "forbidden", message: "no" } }), "application/json");
  await fails(api("/x"));
  assert.equal(fired, 1, "a 403 is not a sign-out");
});

test("a table id is 1 to 50 lowercase letters and digits", () => {
  for (const ok of ["orders", "a", "t1", "0", "x".repeat(50)]) assert.ok(TABLE_ID.test(ok), ok);
  for (const bad of ["", "my-table", "Orders", "a_b", "a b", "x".repeat(51), "orders\n"]) assert.ok(!TABLE_ID.test(bad), JSON.stringify(bad));
});

test("204 resolves to nothing and a JSON body to itself", async () => {
  answering(204, null);
  assert.equal(await api("/auth/logout", "POST"), undefined);
  answering(200, JSON.stringify({ id: "alice", kind: "carbon", org: "tos", app: "x" }), "application/json");
  assert.equal((await api<{ id: string }>("/me")).id, "alice");
});

test("local WebSockets connect directly to the native backend", () => {
  globalThis.location = {hostname: "localhost"} as Location;
  assert.equal(wsUrl("tos"), "ws://localhost:8080/api/ws/mission-control?org=tos");
});

test("an explicit WebSocket base is used and the org is encoded", () => {
  assert.equal(wsUrl("a b", "wss://ss.example/api/ws"), "wss://ss.example/api/ws/mission-control?org=a%20b");
});

test("production WebSockets bypass Vercel and reach the backend subdomain", () => {
  globalThis.location = {hostname: "spacestation.teamofsilicons.com"} as Location;
  assert.equal(wsUrl("tos"), "wss://backend.spacestation.teamofsilicons.com/api/ws/mission-control?org=tos");
});

test("session discovery does not recursively trigger its own reload on 401", async () => {
  let fired=0;
  const on=()=>fired++;
  signedOut.addEventListener("signedout",on);
  answering(401, JSON.stringify({error:{code:"unauthorized",message:"log in"}}));
  await fails(api("/me"));
  await fails(api("/orgs"));
  signedOut.removeEventListener("signedout",on);
  assert.equal(fired,0);
});
