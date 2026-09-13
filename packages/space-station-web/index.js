'use strict';

const MAX_TEXT = 160;
const MAX_BATCH = 40;
const FLUSH_MS = 1000;

const clip = (value) => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, MAX_TEXT);
const randomId = () => {
  try { return globalThis.crypto?.randomUUID?.() || ''; } catch (_) { return ''; }
};
const id = () => randomId() || `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
const finiteRate = (value) => Math.max(0, Math.min(1, Number.isFinite(Number(value)) ? Number(value) : 1));

function jsonValue(value) {
  try { return JSON.parse(JSON.stringify(value)); } catch (_) { return { error: 'unserializable_value' }; }
}

function browserName(ua) {
  if (/Edg\//i.test(ua)) return 'edge';
  if (/Chrome\//i.test(ua)) return 'chrome';
  if (/Firefox\//i.test(ua)) return 'firefox';
  if (/Safari\//i.test(ua)) return 'safari';
  if (/AppleWebKit/i.test(ua)) return 'webkit';
  return 'unknown';
}

/**
 * Create a framework-agnostic browser telemetry sender.
 *
 * The endpoint receives `{table, events}`. No API key or Authorization header is ever added;
 * authentication, if needed, remains the host application's concern.
 */
function createSpaceStationWeb(options = {}) {
  const root = typeof globalThis === 'undefined' ? {} : globalThis;
  const win = root.window;
  const doc = win?.document;
  const nav = root.navigator || win?.navigator;
  const endpoint = options.endpoint || '/api/web/telemetry';
  const transport = options.fetch || root.fetch;
  const analyticsTable = options.analyticsTable;
  const eventsTable = options.eventsTable;
  const rate = finiteRate(options.sampleRate);
  const queues = new Map();
  const cleanups = [];
  let destroyed = false;
  let timer;
  const session = id();
  const ua = clip(nav?.userAgent || '');
  const base = {
    session_id: session,
    url: typeof root.location?.href === 'string' ? root.location.href : undefined,
    path: typeof root.location?.pathname === 'string' ? root.location.pathname : undefined,
    referrer: typeof doc?.referrer === 'string' ? doc.referrer : undefined,
    user_agent: ua || undefined,
    browser: browserName(ua),
    device: /Mobi|Android|iPhone|iPad/i.test(ua) ? 'mobile' : 'desktop',
    language: nav?.language,
    viewport: win ? { width: win.innerWidth, height: win.innerHeight } : undefined,
    screen: root.screen ? { width: root.screen.width, height: root.screen.height, pixel_ratio: root.devicePixelRatio || 1 } : undefined,
  };
  Object.keys(base).forEach((key) => base[key] === undefined && delete base[key]);

  function enqueue(table, type, data, metadata, sampled) {
    if (!table || destroyed || (sampled && Math.random() > rate)) return;
    const list = queues.get(table) || [];
    list.push({ type, data: jsonValue(data ?? {}), metadata: { ...base, ...jsonValue(metadata ?? {}), occurred_at: new Date().toISOString() } });
    queues.set(table, list);
    if (list.length >= MAX_BATCH) void flush(table);
    else if (!timer) timer = setTimeout(() => { timer = undefined; void flush(); }, FLUSH_MS);
  }

  function analytics(type, data, metadata) { enqueue(analyticsTable, type, data, metadata, true); }

  function track(name, data = {}, metadata = {}) {
    if (!name || typeof name !== 'string') throw new TypeError('track(name, data, metadata): name must be a non-empty string');
    enqueue(eventsTable, name, data, metadata, false);
  }

  async function send(table, events) {
    if (typeof transport !== 'function') throw new Error('Space Station web telemetry requires fetch or options.fetch');
    const response = await transport(endpoint, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ table, events }),
    });
    if (!response?.ok) throw new Error(`telemetry endpoint returned HTTP ${response?.status ?? 'unknown'}`);
  }

  async function flush(table) {
    const batches = table ? [table] : [...queues.keys()];
    const pending = [];
    for (const key of batches) {
      const events = queues.get(key);
      if (!events?.length) continue;
      queues.delete(key);
      pending.push(send(key, events).catch((error) => { queues.set(key, [...(queues.get(key) || []), ...events]); throw error; }));
    }
    return Promise.all(pending);
  }

  const on = (target, event, handler, opts) => {
    if (!target?.addEventListener) return;
    target.addEventListener(event, handler, opts);
    cleanups.push(() => target.removeEventListener(event, handler, opts));
  };

  if (win && analyticsTable) {
    analytics('page_view', {});
    on(doc, 'click', (event) => {
      const target = event.target?.closest?.('*') || event.target;
      analytics('click', {
        tag: target?.tagName?.toLowerCase(), id: target?.id || undefined,
        role: target?.getAttribute?.('role') || undefined, text: clip(target?.textContent),
        href: target?.href || undefined,
      });
    }, { passive: true });
    on(win, 'error', (event) => analytics('error', { message: clip(event.message), source: clip(event.filename), line: event.lineno, column: event.colno }));
    on(win, 'unhandledrejection', (event) => analytics('error', { kind: 'unhandledrejection', reason: clip(event.reason?.message || event.reason) }));
    let scrollTimer;
    on(win, 'scroll', () => {
      if (scrollTimer) return;
      scrollTimer = setTimeout(() => {
        scrollTimer = undefined;
        const height = Math.max(1, doc.documentElement?.scrollHeight || 1);
        analytics('scroll', { x: win.scrollX || 0, y: win.scrollY || 0, percent: Math.min(100, Math.round(((win.scrollY + win.innerHeight) / height) * 100)) });
      }, 250);
    }, { passive: true });
    const timing = () => {
      const entry = root.performance?.getEntriesByType?.('navigation')?.[0];
      if (entry) analytics('timing', { dns_ms: entry.domainLookupEnd - entry.domainLookupStart, connect_ms: entry.connectEnd - entry.connectStart, response_ms: entry.responseEnd - entry.responseStart, dom_content_loaded_ms: entry.domContentLoadedEventEnd, load_ms: entry.loadEventEnd });
    };
    if (doc.readyState === 'complete') timing(); else on(win, 'load', timing, { once: true });
    if (typeof win.fetch === 'function') {
      const original = win.fetch;
      win.fetch = function wrappedFetch(input, init) {
        const started = Date.now();
        const url = typeof input === 'string' ? input : input?.url || '';
        if (url === endpoint || url.endsWith(endpoint)) return original.apply(this, arguments);
        return original.apply(this, arguments).then((response) => { analytics('network', { method: init?.method || input?.method || 'GET', url: clip(url), status: response.status, duration_ms: Date.now() - started }); return response; }, (error) => { analytics('network_error', { method: init?.method || input?.method || 'GET', url: clip(url), duration_ms: Date.now() - started, error: clip(error?.message || error) }); throw error; });
      };
      cleanups.push(() => { win.fetch = original; });
    }
  }

  return {
    track,
    flush,
    analytics: (name, data, metadata) => analytics(name, data, metadata),
    destroy: async () => { if (destroyed) return; destroyed = true; cleanups.splice(0).forEach((cleanup) => cleanup()); if (timer) clearTimeout(timer); await flush(); },
  };
}

module.exports = { createSpaceStationWeb };
module.exports.default = createSpaceStationWeb;
