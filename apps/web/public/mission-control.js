/**
 * mission_control — the Space Station space-window runtime: one file, three roles.
 *
 * host      holds the credentials, talks HTTP/WS to the backend and relays between a
 *           credential-less processor sandbox (iframe in the browser, `--permission` child under
 *           Node) and the renderer iframe. Browser: `SpaceStation.host(opts)`. Node: `run`/`tool`.
 * processor runs the untrusted window code: `init`, the subscriptions' serial queue, the tools.
 * renderer  the bridge injected into the renderer iframe that is `mission_control` there.
 *
 * Loaded from a CDN it defines `window.SpaceStation`; under Node it is also the CLI
 * (`run`, `tool`, `dev`, `processor`). No top-level imports: Node modules are required lazily
 * inside the Node-only paths so the same bytes run in both places.
 */
(function () {
  "use strict";

  // The renderer and processor iframes get exactly this. Never added: allow-same-origin (would
  // give the frame our origin and cookies), allow-top-navigation, allow-forms, allow-popups,
  // allow-modals, allow-downloads, allow-pointer-lock.
  const SANDBOX = "allow-scripts";
  const MAX = 64 * 1024; // SiliconJSON, tool args and tool results (shared::limits::SILICON_JSON_MAX)
  const TIMEOUT = 10_000; // onTrigger and tools (shared::limits::TRIGGER_TIMEOUT_MS)
  const IDLE = 600_000; // `run` exits after this long without a trigger (shared::limits::WINDOW_IDLE_MS)
  // shared::secrets::find_secret, duplicated here because the browser has no Rust: our secrets, a
  // terminal's own `sscli-` session, and every IAM credential.
  const SECRET = /\b(spacewindow|apikey|whsec|stk|table-[a-z0-9]{1,50})-[0-9a-f]{32}\b|\b(?:(sat|cat|rft|oat|ort|ask)_|(sscli)-)[A-Za-z0-9_-]{43}\b/;
  const NUMERIC = ["cursor", "event_ts_ms", "registered_ts_ms"]; // quoted 64-bit ints from ClickHouse

  const utf8 = s => new TextEncoder().encode(s).length;
  const isObject = v => typeof v === "object" && v !== null && !Array.isArray(v);
  class Timeout extends Error {}
  const within = (fn, ms, what) =>
    new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Timeout(`${what} timed out after ${ms} ms`)), ms);
      new Promise(r => r(fn())).then(resolve, reject).finally(() => clearTimeout(t));
    });
  /** The JSON text of `value`, or a throw like "init returned 70000 bytes, the limit is 65536". */
  function bounded(value, what) {
    const text = JSON.stringify(value) ?? "null", size = utf8(text);
    if (size > MAX) throw new Error(`${what} ${size} bytes, the limit is ${MAX}`);
    return text;
  }
  /** One JSON round trip; an `{error: {code, message}}` reply becomes an Error carrying `code`. */
  async function request(url, headers, body) {
    const res = await fetch(url, { method: body ? "POST" : "GET", redirect: "error", headers: { ...headers, ...(body && { "content-type": "application/json" }) }, body: body && JSON.stringify(body) });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw Object.assign(new Error(data.error?.message ?? `HTTP ${res.status}`), { code: data.error?.code ?? `http_${res.status}` });
    return data;
  }

  // ─── tools: the type table ───────────────────────────────────────────────────────────────────

  const TYPES = {
    string: v => typeof v === "string",
    number: v => Number.isFinite(v),
    boolean: v => typeof v === "boolean",
    object: isObject,
    array: Array.isArray,
  };

  /** What is wrong with `args` against a tool's `args` spec, or null. A `?` suffix permits absent only. */
  function argsError(spec, args) {
    if (!isObject(args)) return "args must be an object";
    for (const k of Object.keys(args)) if (!Object.hasOwn(spec, k)) return `unknown argument "${k}"`;
    for (const [k, t] of Object.entries(spec)) {
      const optional = t.endsWith("?"), type = optional ? t.slice(0, -1) : t;
      if (args[k] === undefined) {
        if (optional) continue;
        return `missing argument "${k}"`;
      }
      if (!TYPES[type]?.(args[k])) return `argument "${k}" must be ${type}`;
    }
    return null;
  }

  // ─── processor ───────────────────────────────────────────────────────────────────────────────

  /** The processor role. `send` posts to the host; the returned function takes the host's messages. */
  function processor(send) {
    let def, json = {}, sent, timeout = TIMEOUT, restrict, ids = 0, busy = false, dirty = false, lastState = 0, stateTimer;
    // Every subscription keeps its own cursor per trigger table (`cursors[id][t]`), because two
    // subscriptions on one table are triggered by different rows; `data.tables` is their view.
    const data = { tables: {} }, cursors = {}, replies = new Map(), pending = new Set(), toolQueue = [];

    const ask = msg => new Promise((resolve, reject) => {
      const id = ++ids;
      replies.set(id, { resolve, reject });
      send({ ...msg, id });
    });
    const mission_control = { query: sql => ask({ type: "query", sql: String(sql), restrict }).then(r => r.rows), get json() { return json; }, data };
    const devError = (message, e) => send({ type: "dev_error", source: "processor", message, detail: e?.stack ?? null });
    const subIds = () => Object.keys(def?.subscriptions ?? {});
    const triggerTables = id => def.subscriptions[id].triggers.map(t => t.table);

    /** init and onTrigger must settle with a JSON object within 64 KB; what they return is kept as pure JSON. */
    function checked(value, what) {
      if (!isObject(value)) throw new Error(`${what} must return a JSON object`);
      return JSON.parse(bounded(value, `${what} returned`));
    }

    /** Runs the module with `defineProcessor` in scope (`export default defineProcessor(…)` is allowed) and checks its shape. */
    function evaluate(code) {
      let d;
      try {
        const compile = source => new Function("defineProcessor", "mission_control", source);
        let run;
        try {
          run = compile(code);
        } catch (original) {
          // Let the JavaScript parser identify the real export: removing text inside a string,
          // comment or regexp cannot make an actual export statement compile as a function.
          // Compile candidates without executing any; the selected processor runs exactly once.
          let error = original;
          for (const m of code.matchAll(/\bexport\s+default\s+/g)) {
            try {
              run = compile(code.slice(0, m.index) + code.slice(m.index + m[0].length));
              break;
            } catch (e) { error = e; }
          }
          if (!run) throw error;
        }
        run(x => (d = x), mission_control);
      } catch (e) {
        throw Object.assign(new Error(`processor code: ${e?.message ?? e}`), { stack: e?.stack });
      }
      if (!isObject(d)) throw new Error("processor code never called defineProcessor");
      for (const [id, s] of Object.entries(d.subscriptions ?? {})) if (!Array.isArray(s.triggers) || typeof s.sql !== "string" || typeof s.onTrigger !== "function") throw new Error(`subscription "${id}" needs triggers, sql and onTrigger`);
      for (const [name, t] of Object.entries(d.tools ?? {})) {
        if (!t || typeof t.run !== "function") throw new Error(`tool "${name}" needs run`);
        if (t.args !== undefined && !isObject(t.args)) throw new Error(`tool "${name}" args must be an object`);
        for (const [arg, type] of Object.entries(t.args ?? {})) {
          if (typeof type !== "string" || !/^(string|number|boolean|object|array)\??$/.test(type)) throw new Error(`tool "${name}" argument "${arg}" has an invalid type`);
        }
      }
      return d;
    }

    /** The host hears `json` only when the serialised document differs from the last one it heard: a
     *  run that leaves SiliconJSON as it was is not a new document, so it is no `json` line and no
     *  `json` in the next state. */
    function publish() {
      const text = JSON.stringify(json);
      if (text === sent) return;
      sent = text;
      send({ type: "json", json });
      dirty = true;
      arm(Math.max(0, lastState + 1000 - Date.now()));
    }
    function arm(ms) {
      clearTimeout(stateTimer);
      stateTimer = setTimeout(sendState, ms);
    }
    /** At most one state a second, at least one every ten; json travels only when it changed. */
    function sendState() {
      lastState = Date.now();
      send(dirty ? { type: "state", json } : { type: "state" });
      dirty = false;
      arm(10_000);
    }

    async function load(m) {
      json = m.json ?? {};
      sent = JSON.stringify(json); // the seed is the host's own: it reported it already
      timeout = m.timeout ?? TIMEOUT;
      busy = true; // tools and triggers wait until init is done
      try {
        def = evaluate(m.code);
        send({ type: "loaded", tools: Object.fromEntries(Object.entries(def.tools ?? {}).map(([k, t]) => [k, t.args ?? {}])) });
        if (m.oneshot) return;
        const { tables: W } = await ask({ type: "tables" });
        for (const id of subIds()) {
          cursors[id] = {};
          for (const t of triggerTables(id)) {
            cursors[id][t] = W[t] ?? 0;
            data.tables[t] = { cursor: W[t] ?? 0, watermark: W[t] ?? 0 };
          }
        }
        if (def.init) {
          restrict = Object.fromEntries(Object.entries(data.tables).map(([t, d]) => [t, { to: d.cursor }]));
          try {
            json = checked(await def.init(structuredClone(json)), "init");
          } finally {
            restrict = undefined;
          }
        }
        publish();
        arm(0); // the first state, json or not: from here the heartbeat keeps the window live
        for (const id of subIds()) send({ type: "subscribe", id, triggers: def.subscriptions[id].triggers });
      } catch (e) {
        devError(e?.message ?? String(e), e);
      } finally {
        busy = false;
        pump();
      }
    }

    function subscribed({ id, watermarks }) {
      for (const t of triggerTables(id)) {
        const d = data.tables[t];
        d.watermark = Math.max(d.watermark, watermarks[t] ?? 0);
        if (d.watermark > cursors[id][t]) pending.add(id);
      }
      pump();
    }

    /** One subscription run: query, onTrigger, then cursors and SiliconJSON move together or not at all. */
    async function run(id) {
      const sub = def.subscriptions[id], tables = triggerTables(id);
      try {
        const delta = sub.mode === "snapshot" ? undefined : Object.fromEntries(tables.map(t => [t, { from: cursors[id][t] }]));
        const { rows, watermarks = {} } = await ask({ type: "query", sql: sub.sql, restrict: delta });
        json = checked(await within(() => sub.onTrigger(structuredClone(json), rows), timeout, `${id}.onTrigger`), `${id}.onTrigger`);
        for (const t of tables) if (t in watermarks) {
          cursors[id][t] = watermarks[t];
          const d = data.tables[t]; // the table's cursor is how far every subscription on it has come
          d.watermark = Math.max(d.watermark, watermarks[t]);
          d.cursor = Math.min(...subIds().map(s => cursors[s][t] ?? Infinity));
        }
        publish();
      } catch (e) {
        devError(`${id}: ${e?.message ?? e}`, e);
      }
    }

    function tool(m) {
      const fail = (code, message) => send({ type: "tool_error", id: m.id, code, message });
      const t = def?.tools?.[m.name], args = m.args ?? {};
      if (!t) return fail("unknown_tool", `no tool "${m.name}"`);
      const bad = utf8(JSON.stringify(args)) > MAX ? `args over ${MAX} bytes` : argsError(t.args ?? {}, args);
      if (bad) return fail("invalid_args", bad);
      toolQueue.push(async () => {
        try {
          send({ type: "tool_result", id: m.id, result: JSON.parse(bounded(await within(() => t.run(structuredClone(json), args), timeout, m.name), "result is")) });
        } catch (e) {
          fail(e instanceof Timeout ? "timeout" : "failed", e?.message ?? String(e));
        }
      });
      pump();
    }

    /** Tools first, then the pending subscription that is highest in the definition. */
    function next() {
      if (toolQueue.length) return toolQueue.shift();
      for (const id of subIds()) if (pending.delete(id)) return () => run(id);
    }
    function pump() {
      const job = busy ? undefined : next();
      if (!job) return;
      busy = true;
      job().finally(() => {
        busy = false;
        pump();
      });
    }

    /** Only replies settle a pending `ask`: a `tool {id}` from the renderer numbers its own calls. */
    const settle = (m, how, value) => {
      const r = replies.get(m.id);
      replies.delete(m.id);
      r?.[how](value);
    };
    return function receive(m) {
      switch (m.type) {
        case "load": return load(m);
        case "tables": case "result": return settle(m, "resolve", m);
        case "error": return settle(m, "reject", Object.assign(new Error(m.message), { code: m.code }));
        case "subscribed": return subscribed(m);
        case "trigger": pending.add(m.id); return pump();
        case "tool": return tool(m);
      }
    };
  }

  /** Browser processor iframe: `SpaceStation.processorRole()` after the runtime loads. */
  function processorRole() {
    const receive = processor(m => parent.postMessage(m, "*"));
    addEventListener("message", e => {
      if (e.source === parent) receive(e.data);
    });
    parent.postMessage({ type: "ready" }, "*");
  }

  /** Node processor child: JSON lines on stdio. Only the bridge writes to stdout; user output goes to stderr. */
  function processorMain() {
    const out = process.stdout, lines = require("node:readline").createInterface({ input: process.stdin });
    Object.defineProperty(process, "stdout", { value: process.stderr });
    globalThis.console = new console.Console(process.stderr);
    for (const k of ["fetch", "WebSocket"]) delete globalThis[k]; // no I/O, as in the browser's CSP
    const receive = processor(m => out.write(JSON.stringify(m) + "\n"));
    lines.on("line", line => receive(JSON.parse(line)));
    lines.on("close", () => process.exit(0));
    out.write('{"type":"ready"}\n');
  }

  // ─── renderer bridge ─────────────────────────────────────────────────────────────────────────

  /** Injected as `(${rendererBridge})()` before any renderer script; must stay self-contained. */
  function rendererBridge() {
    const listeners = [], calls = new Map();
    let n = 0, received = false;
    const post = m => parent.postMessage(m, "*");
    let mc = {
      json: {}, metadata: {}, stale: true, tools: {},
      /** `cb(json, metadata)` after every update, and at once when one already arrived. */
      on(event, cb) {
        if (event !== "update") return;
        listeners.push(cb);
        if (received) cb(mc.json, mc.metadata);
      },
      /** A Vue 3 app over `selector` with `mission_control` and `stale` in scope, when Vue is on the page. */
      mount(selector) {
        if (!window.Vue) return;
        mc = window.mission_control = Vue.reactive(mc);
        Vue.createApp({ setup: () => ({ mission_control: mc, stale: Vue.computed(() => mc.stale) }) }).mount(selector);
      },
    };
    const tool = name => args => new Promise((resolve, reject) => {
      calls.set(++n, { resolve, reject });
      post({ type: "tool", id: n, name, args });
    });
    addEventListener("message", e => {
      if (e.source !== parent) return;
      const m = e.data, call = calls.get(m.id);
      calls.delete(m.id);
      if (m.type === "update") {
        mc.json = m.json;
        mc.metadata = m.metadata;
        mc.stale = !m.metadata.is_live;
        mc.tools = Object.fromEntries(Object.keys(m.tools).map(name => [name, tool(name)]));
        received = true;
        listeners.forEach(cb => cb(mc.json, mc.metadata));
      } else if (m.type === "tool_result") call?.resolve(m.result);
      else if (m.type === "tool_error") call?.reject(Object.assign(new Error(m.message), { code: m.code }));
    });
    const report = (message, detail) => post({ type: "dev_error", source: "renderer", message: String(message), detail: detail ?? null });
    addEventListener("error", e => report(e.message ?? `failed to load ${e.target.src || e.target.href}`, e.error?.stack), true);
    addEventListener("unhandledrejection", e => report(e.reason?.message ?? e.reason, e.reason?.stack));
    window.mission_control = mc;
    post({ type: "ready" });
  }

  /** The bridge goes right after `<head>`, else right after the doctype, else in front of everything. */
  function inject(html) {
    const tag = `<script>(${rendererBridge})()</script>`, m = /<head(\s[^>]*)?>/i.exec(html) ?? /^\s*<!doctype[^>]*>/i.exec(html);
    const at = m ? m.index + m[0].length : 0;
    return html.slice(0, at) + tag + html.slice(at);
  }

  // ─── host ────────────────────────────────────────────────────────────────────────────────────

  /**
   * The host role. `env` is what differs between the browser and Node: `sandbox(receive)` and
   * `renderer(html, receive)` each give `{send, destroy}`; `wsOptions`, `timeout`, `oneshot`
   * (no init, no subscriptions: `tool`), `activity()` (a trigger or tool call happened).
   */
  function host(opts, env) {
    const { api, org } = opts, errors = [], subs = new Map();
    const meta = { processor_version: "dev", renderer_version: "dev", produced_at: null, is_live: false };
    let json = {}, tools, version = null, published = false, load, proc, rend, ws, retry = 1000, timer, closed = false;

    const devError = ({ source, message, detail = null }) => {
      const e = { source, message, detail };
      errors.push(e);
      opts.onError?.(e);
    };
    const failed = e => !closed && devError({ source: "host", message: e.message, detail: e.stack });
    const call = (path, body) => request(api.base + path, api.headers, body);
    /** The renderer hears nothing until the processor is loaded, so every update carries the tools. */
    const update = () => tools && rend?.send({ type: "update", json, metadata: { ...meta }, tools });
    const wsSend = m => ws?.readyState === 1 && ws.send(JSON.stringify(m));
    /** The server keeps the last json it saw, so json rides along when it changed and once per fresh socket. */
    const state = withJson => opts.window.id && wsSend({ type: "state", window: opts.window.id, version, ...(withJson && { json }) });
    const status = connected => {
      meta.is_live = connected;
      opts.onStatus?.({ is_live: connected, produced_at: meta.produced_at, connected });
      update();
    };

    async function start() {
      let code = opts.code;
      if (!code) {
        code = opts.window.version ?? (await call(`/orgs/${org}/windows/${opts.window.id}`)).version;
        if (closed) return;
        if (!code) throw new Error("this window has no published version");
        version = code.name;
      }
      meta.processor_version = meta.renderer_version = version ?? "dev";
      const cached = opts.window.id ? await call(`/orgs/${org}/windows/${opts.window.id}/state`) : {};
      if (closed) return;
      json = cached.json ?? {};
      meta.produced_at = cached.metadata?.produced_at ?? null;
      meta.is_live = cached.metadata?.is_live ?? false;
      opts.onJson?.(json);
      load = { type: "load", code: code.processor, json, timeout: env.timeout, oneshot: env.oneshot };
      rend = env.renderer?.(code.renderer, fromRenderer);
      proc = env.sandbox(m => fromProcessor(m).catch(failed));
      if (!env.oneshot) connect();
    }

    function connect() {
      ws = new WebSocket(opts.ws, env.wsOptions);
      ws.onopen = () => {
        status(true);
        for (const [id, triggers] of subs) wsSend({ type: "subscribe", id, triggers });
        if (published) state(true);
      };
      // The handshake completes before the server authenticates, so only a socket that carried a
      // frame counts as one that worked; anything else keeps doubling the backoff.
      ws.onmessage = e => {
        try {
          retry = 1000;
          fromServer(JSON.parse(e.data));
        } catch (error) {
          devError({ source: "host", message: `invalid server message: ${error?.message ?? error}`, detail: error?.stack ?? null });
        }
      };
      ws.onclose = e => {
        if (closed) return;
        status(false);
        // A 4xxx close is the server refusing this credential (4401) or this actor (4403), with a
        // reason: it is a dev error and reconnecting with the same credential cannot help.
        if (e.code >= 4000) return devError({ source: "host", message: `mission control refused the socket: ${e.code} ${e.reason || "no reason given"}` });
        timer = setTimeout(connect, retry);
        retry = Math.min(retry * 2, 30_000);
      };
    }

    function fromServer(m) {
      if (m.type === "trigger") env.activity?.();
      if (m.type === "subscribed" || m.type === "trigger") proc.send(m);
      else if (m.type === "error") devError({ source: "server", message: `${m.code}: ${m.message}` });
      else if (m.type === "notification") opts.onNotification?.(m); // a notification this actor is a recipient of
    }

    /** `tables` and `query` are answered, or refused with `error {id, code, message}`. */
    async function answer(m, fn) {
      try {
        return { id: m.id, ...(await fn()) };
      } catch (e) {
        return { type: "error", id: m.id, code: e.code ?? "failed", message: e.message };
      }
    }

    async function fromProcessor(m) {
      if (closed) return;
      switch (m.type) {
        case "ready": return proc.send(load);
        case "loaded": tools = m.tools; return update();
        case "tables":
          return proc.send(await answer(m, async () => ({ type: "tables", tables: Object.fromEntries((await call(`/orgs/${org}/tables`)).map(t => [t.id, t.watermark])) })));
        case "query":
          return proc.send(await answer(m, async () => {
            const { rows, watermarks } = await call(`/orgs/${org}/query`, { sql: m.sql, restrict: m.restrict });
            for (const r of rows) for (const k of NUMERIC) if (k in r) {
              const n = Number(r[k]);
              if (Number.isSafeInteger(n)) r[k] = n;
            }
            return { type: "result", rows, watermarks };
          }));
        case "subscribe": subs.set(m.id, m.triggers); return wsSend(m);
        case "state": return state("json" in m);
        case "json":
          json = m.json;
          published = true;
          meta.produced_at = new Date().toISOString();
          opts.onJson?.(json);
          return update();
        case "tool_result": case "tool_error": return rend?.send(m);
        case "dev_error": return devError(m);
      }
    }

    function fromRenderer(m) {
      if (m.type === "ready") update();
      else if (m.type === "tool") {
        env.activity?.();
        proc.send(m);
      } else if (m.type === "dev_error") devError(m);
    }

    start().catch(failed);
    return {
      errors,
      destroy() {
        closed = true;
        clearTimeout(timer);
        ws?.close();
        proc?.destroy();
        rend?.destroy();
      },
    };
  }

  /** Browser: both sandboxes are `<iframe sandbox="allow-scripts" srcdoc>` inside `opts.mount`. */
  function browserEnv(opts) {
    function frame(srcdoc, receive, hidden) {
      const f = document.createElement("iframe");
      f.title = hidden ? "Space Window processor" : "Space Window view";
      f.setAttribute("sandbox", SANDBOX);
      f.hidden = hidden;
      f.srcdoc = srcdoc;
      const onMessage = e => e.source === f.contentWindow && receive(e.data);
      addEventListener("message", onMessage);
      opts.mount.appendChild(f);
      return {
        send: m => f.contentWindow.postMessage(m, "*"),
        destroy() {
          removeEventListener("message", onMessage);
          f.remove();
        },
      };
    }
    const runtime = new URL(opts.runtimeUrl, location.href);
    return {
      timeout: TIMEOUT,
      sandbox: receive => frame(
        `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src ${runtime.origin} 'unsafe-inline' 'unsafe-eval'">` +
          `<script src="${runtime.href}"></script><script>SpaceStation.processorRole()</script>`,
        receive, true),
      renderer: (html, receive) => frame(inject(html), receive, false),
    };
  }

  /** Node: the processor is a `--permission` child that may read only this file and sees no env. */
  function nodeEnv(token, oneshot) {
    const { spawn } = require("node:child_process"), readline = require("node:readline");
    return {
      wsOptions: { headers: { Authorization: `Bearer ${token}`, "User-Agent": "space-station-runtime/0.1.0" } },
      timeout: Number(process.env.SPACE_STATION_TEST_TIMEOUT_MS) || TIMEOUT,
      oneshot,
      sandbox(receive) {
        const child = spawn(process.execPath, ["--permission", `--allow-fs-read=${__filename}`, __filename, "processor"], { env: {}, stdio: ["pipe", "pipe", "inherit"] });
        let killed = false;
        child.stdin.on("error", () => {});
        readline.createInterface({ input: child.stdout }).on("line", line => receive(JSON.parse(line)));
        child.on("exit", code => killed || receive({ type: "dev_error", source: "host", message: `processor exited with code ${code}` }));
        return {
          send: m => child.stdin.write(JSON.stringify(m) + "\n"),
          destroy() {
            killed = true;
            child.kill();
          },
        };
      },
    };
  }

  // ─── Node CLI ────────────────────────────────────────────────────────────────────────────────

  const USAGE = `usage:
  mission-control.js run  --url U --org O --window W
  mission-control.js tool --url U --org O --window W --name N [--args JSON]
  mission-control.js dev  [serve [--port 4747] [--dir .]] | publish --window W --name V | notify <notification-id>
  mission-control.js processor`;

  const line = (stream, v) => stream.write((typeof v === "string" ? v : JSON.stringify(v)) + "\n");
  function fail(message) {
    line(process.stderr, message);
    process.exit(1);
  }
  function flags(argv) {
    const f = { _: [] };
    for (let i = 0; i < argv.length; i++) argv[i].startsWith("--") ? (f[argv[i].slice(2)] = argv[++i]) : f._.push(argv[i]);
    return f;
  }

  /** The Node entry point; any failure is one `{error: {code, message}}` line on stderr and exit 1. */
  const main = argv => dispatch(argv).catch(e => fail({ error: { code: e.code ?? "failed", message: e.message } }));

  async function dispatch(argv) {
    const [cmd, ...rest] = argv, f = flags(rest), token = process.env.SPACE_STATION_ACCESS_TOKEN ?? "";
    if (cmd === "processor") return processorMain();
    if (cmd === "dev") return dev(f);
    if (!["run", "tool"].includes(cmd) || !f.url || !f.org || !f.window || (cmd === "tool" && !f.name)) return fail(USAGE);
    const base = f.url.replace(/\/$/, ""), env = nodeEnv(token, cmd === "tool");
    let idle;
    env.activity = () => {
      clearTimeout(idle);
      idle = setTimeout(() => {
        mc.destroy();
        process.exit(0);
      }, Number(process.env.SPACE_STATION_TEST_IDLE_MS) || IDLE);
    };
    if (cmd === "tool") {
      const args = JSON.parse(f.args ?? "{}");
      env.renderer = (_, receive) => ({
        destroy() {},
        send(m) {
          if (m.type === "update") return receive({ type: "tool", id: 1, name: f.name, args });
          mc.destroy();
          if (m.type === "tool_result") line(process.stdout, m.result);
          else fail({ error: { code: m.code, message: m.message } });
          process.exit(0);
        },
      });
    }
    const mc = host({
      api: { base: `${base}/api`, headers: { Authorization: `Bearer ${token}`, "User-Agent": "space-station-runtime/0.1.0" } },
      ws: `${base.replace(/^http/, "ws")}/api/ws/mission-control?org=${encodeURIComponent(f.org)}`,
      org: f.org,
      window: { id: f.window },
      onJson: cmd === "run" ? j => line(process.stdout, j) : undefined,
      onStatus: s => line(process.stderr, { status: s }),
      onError: e => {
        line(process.stderr, e);
        if (cmd === "tool" || e.source === "host") process.exit(1);
      },
    }, env);
    if (cmd === "run") env.activity();
  }

  /** `dev serve | publish | notify`: `.env` in `--dir` holds the URL, token, org and window. */
  async function dev(f) {
    const fs = require("node:fs"), path = require("node:path");
    const dir = path.resolve(f.dir ?? "."), envFile = path.join(dir, ".env");
    const dotenv = fs.existsSync(envFile) ? require("node:util").parseEnv(fs.readFileSync(envFile, "utf8")) : {};
    const setting = k => process.env[k] ?? dotenv[k];
    const url = (setting("SPACE_STATION_URL") ?? "").replace(/\/$/, ""), token = setting("SPACE_STATION_ACCESS_TOKEN"), org = setting("SPACE_STATION_ORG");
    if (!url || !token || !org) return fail(`${envFile} needs SPACE_STATION_URL, SPACE_STATION_ACCESS_TOKEN and SPACE_STATION_ORG`);
    const read = name => fs.readFileSync(path.join(dir, name), "utf8");
    const post = (p, body) => request(`${url}/api/orgs/${encodeURIComponent(org)}${p}`, { authorization: `Bearer ${token}` }, body);
    switch (f._[0] ?? "serve") {
      case "serve":
        return serve({ port: Number(f.port ?? 4747), dir, url, token, org, window: setting("SPACE_STATION_WINDOW") ?? null, read });
      case "publish": {
        if (!f.window || !f.name) return fail(USAGE);
        const files = { processor: read("processor.js"), renderer: read("renderer.html") };
        for (const [k, text] of Object.entries(files)) {
          const m = SECRET.exec(text);
          if (m) return fail(`${k} contains a ${m[1] ?? m[2] ?? m[3]} secret; remove it before publishing`);
        }
        return line(process.stdout, await post(`/windows/${f.window}/versions`, { name: f.name, ...files }));
      }
      case "notify": {
        if (!f._[1]) return fail(USAGE);
        const r = await post(`/notifications/${f._[1]}/test`, {});
        if (r.error) return fail({ error: { code: "query_failed", message: r.error } });
        if (r.last_trigger_at === null) line(process.stdout, "no trigger seen yet");
        for (const row of r.rows ?? []) line(process.stdout, row);
        return;
      }
      default: return fail(USAGE);
    }
  }

  /** The dev host page on 127.0.0.1 plus the only door to the backend: an HTTP and WS proxy adding the bearer. */
  function serve({ port, dir, url, token, org, window, read }) {
    const http = require("node:http"), fs = require("node:fs"), path = require("node:path"), WebSocket = require("ws");
    const hosts = new Set([`localhost:${port}`, `127.0.0.1:${port}`]);
    const trusted = (req, strict) => hosts.has(req.headers.host) && (!strict || req.headers.origin === `http://${req.headers.host}`);
    const version = () => ["processor.js", "renderer.html"].map(n => { try { return fs.statSync(path.join(dir, n)).mtimeMs; } catch { return 0; } }).join(":");
    const page = `<!doctype html><meta charset="utf-8"><title>Space Station dev</title>
<style>html,body,#mount,iframe{margin:0;width:100%;height:100%;border:0}#errors{position:fixed;left:0;right:0;bottom:0;max-height:40%;overflow:auto;margin:0;padding:8px;background:#300;color:#fbb;font:12px/1.4 monospace;white-space:pre-wrap}#errors:empty{display:none}</style>
<div id="mount"></div><pre id="errors"></pre>
<script src="/mission-control.js"></script>
<script>
(async () => {
  const errors = document.getElementById("errors"), ORG = ${JSON.stringify(org)}, WINDOW = ${JSON.stringify(window)};
  const text = async u => { const r = await fetch(u); if (!r.ok) throw new Error(u + ": " + await r.text()); return r.text(); };
  try {
    const [processor, renderer, version] = await Promise.all([text("/processor.js"), text("/renderer.html"), text("/version")]);
    SpaceStation.host({
      runtimeUrl: location.origin + "/mission-control.js", api: { base: "/api" },
      ws: "ws://" + location.host + "/api/ws/mission-control?org=" + encodeURIComponent(ORG),
      org: ORG, window: { id: WINDOW, name: "dev", version: null }, code: { processor, renderer }, mount: document.getElementById("mount"),
      onError: e => errors.append(e.source + ": " + e.message + "\\n" + (e.detail ? e.detail + "\\n" : "")),
    });
    setInterval(async () => { if (await text("/version") !== version) location.reload(); }, 2000);
  } catch (e) { errors.append("dev: " + e.message + "\\n"); }
})();
</script>`;

    async function proxy(req, res) {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const body = chunks.length ? Buffer.concat(chunks) : undefined;
      const r = await fetch(url + req.url, { method: req.method, redirect: "error", headers: { authorization: `Bearer ${token}`, ...(body && { "content-type": req.headers["content-type"] ?? "application/json" }) }, body });
      res.writeHead(r.status, { "content-type": r.headers.get("content-type") ?? "application/json" });
      res.end(Buffer.from(await r.arrayBuffer()));
    }

    const server = http.createServer(async (req, res) => {
      const reply = (status, type, body) => {
        res.writeHead(status, { "content-type": type });
        res.end(body);
      };
      if (!trusted(req, !["GET", "HEAD"].includes(req.method))) return reply(403, "text/plain", "forbidden");
      const p = req.url.split("?")[0];
      try {
        if (p.startsWith("/api/")) return await proxy(req, res);
        if (req.method !== "GET") return reply(405, "text/plain", "method not allowed");
        if (p === "/") return reply(200, "text/html; charset=utf-8", page);
        if (p === "/mission-control.js") return reply(200, "text/javascript; charset=utf-8", fs.readFileSync(__filename));
        if (p === "/processor.js" || p === "/renderer.html") return reply(200, "text/plain; charset=utf-8", read(p.slice(1)));
        if (p === "/version") return reply(200, "text/plain", version());
        reply(404, "text/plain", "not found");
      } catch (e) {
        reply(502, "text/plain", e.message);
      }
    });
    const wss = new WebSocket.Server({ noServer: true });
    server.on("upgrade", (req, socket, head) => {
      if (!trusted(req, true) || !req.url.startsWith("/api/ws/")) return socket.destroy();
      const backend = new WebSocket(url.replace(/^http/, "ws") + req.url, { headers: { authorization: `Bearer ${token}`, "User-Agent": "space-station-runtime/0.1.0" } });
      backend.on("error", () => socket.destroy());
      backend.on("open", () => wss.handleUpgrade(req, socket, head, client => {
        client.on("error", () => {});
        client.on("message", (d, binary) => backend.send(d, { binary }));
        backend.on("message", (d, binary) => client.send(d, { binary }));
        client.on("close", () => backend.close());
        backend.on("close", (code, reason) => client.close(code === 1000 || (code >= 3000 && code < 5000) ? code : 1011, reason));
      }));
    });
    server.listen(port, "127.0.0.1", () => line(process.stderr, `space-station-dev on http://127.0.0.1:${port} serving ${dir}`));
  }

  // ─── exports ─────────────────────────────────────────────────────────────────────────────────

  if (typeof window !== "undefined") window.SpaceStation = { SANDBOX, host: opts => host(opts, browserEnv(opts)), processorRole };
  if (typeof module !== "undefined") {
    module.exports = { SANDBOX, argsError, host, nodeEnv, inject, main };
    if (require.main === module) main(process.argv.slice(2));
  }
})();
