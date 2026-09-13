'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { createSpaceStationWeb } = require('../');

test('tracks events into the configured table and sends a batch', async () => {
  const requests = [];
  const telemetry = createSpaceStationWeb({ eventsTable: 'events', fetch: async (url, init) => { requests.push({ url, init }); return { ok: true, status: 202 }; } });
  telemetry.track('signed_in', { plan: 'free' }, { source: 'test' });
  await telemetry.flush();
  assert.equal(requests.length, 1);
  assert.equal(requests[0].url, '/api/web/telemetry');
  const body = JSON.parse(requests[0].init.body);
  assert.equal(body.table, 'events');
  assert.equal(body.events[0].type, 'signed_in');
  assert.equal(body.events[0].data.plan, 'free');
  assert.equal(body.events[0].metadata.source, 'test');
});

test('does not require browser globals and keeps failed batches for retry', async () => {
  let attempts = 0;
  const telemetry = createSpaceStationWeb({ eventsTable: 'events', fetch: async () => { attempts++; return { ok: attempts > 1, status: 500 }; } });
  telemetry.track('once');
  await assert.rejects(telemetry.flush(), /HTTP 500/);
  await telemetry.flush();
  assert.equal(attempts, 2);
});

test('analytics is opt-in by table and sample rate zero drops automatic records', async () => {
  const requests = [];
  const telemetry = createSpaceStationWeb({ analyticsTable: 'analytics', sampleRate: 0, fetch: async (_, init) => { requests.push(init); return { ok: true }; } });
  telemetry.analytics('manual', { ok: true });
  await telemetry.flush();
  assert.equal(requests.length, 0);
});
