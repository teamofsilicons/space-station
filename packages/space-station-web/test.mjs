import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import * as esm from './index.js';

const cjs = createRequire(import.meta.url)('./index.cjs');
const unreadable = new Proxy({}, { get() { throw new Error('event or option data was read'); } });

for (const [format, sdk] of [['ESM', esm], ['CommonJS', cjs]]) {
  test(`${format} stays inert even when explicitly enabled`, async () => {
    let requests = 0;
    const client = sdk.createSpaceStationWeb({
      enabled: true,
      analyticsTable: 'analytics',
      eventsTable: 'events',
      endpoint: 'https://example.invalid/collect',
      fetch: async () => { requests++; return { ok: true }; },
    });
    assert.equal(sdk.default, sdk.createSpaceStationWeb);
    assert.equal(sdk.createSpaceStationWeb(unreadable).isEnabled(), false);
    for (const enabled of [true, false, true]) {
      assert.equal(client.setEnabled(enabled), false);
      client.track('event', unreadable, unreadable);
      client.analytics('page_view', unreadable, unreadable);
      assert.equal(client.isEnabled(), false);
      assert.deepEqual(await client.flush(), { sent: 0, failed: 0, dropped: 0, queued: 0 });
    }
    await client.destroy();
    client.track('after_destroy', unreadable);
    assert.deepEqual(await client.flush('events'), { sent: 0, failed: 0, dropped: 0, queued: 0 });
    assert.deepEqual(sdk.toIngestBatch(unreadable), { batch_id: '', records: [] });
    assert.equal(requests, 0);
  });
}

test('loading and using the SDK never touches browser APIs or schedules work', async () => {
  const sandbox = { module: { exports: {} } };
  for (const name of ['window', 'document', 'navigator', 'location', 'screen', 'performance',
    'crypto', 'localStorage', 'sessionStorage', 'fetch', 'setTimeout', 'setInterval',
    'addEventListener', 'XMLHttpRequest', 'WebSocket']) {
    Object.defineProperty(sandbox, name, { get() { throw new Error(`accessed ${name}`); } });
  }
  runInNewContext(readFileSync(new URL('./index.cjs', import.meta.url), 'utf8'), sandbox);
  const client = sandbox.module.exports.createSpaceStationWeb({ enabled: true, analyticsTable: 'analytics', eventsTable: 'events' });
  client.setEnabled(true);
  client.track('event', unreadable);
  client.analytics('page_view', unreadable);
  await client.flush();
  await client.destroy();
});
