'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { createSpaceStationWeb } = require('../');

test('tracks events into the configured table and sends a batch', async () => {
  const requests = [];
  const telemetry = createSpaceStationWeb({ eventsTable: 'events', fetch: async (url, init) => { requests.push({ url, init }); return { ok: true, status: 202 }; } });
  telemetry.track('signed_in', { plan: 'free' }, { source: 'test' });
  const result = await telemetry.flush();
  assert.deepEqual(result, { sent: 1, failed: 0, dropped: 0, queued: 0 });
  const body = JSON.parse(requests[0].init.body);
  assert.equal(body.table, 'events');
  assert.equal(body.events[0].type, 'signed_in');
  assert.equal(body.events[0].data.plan, 'free');
  assert.equal(body.events[0].metadata.source, 'test');
});

test('bounds failed batches and retries without rejecting callers', async () => {
  let attempts = 0;
  const telemetry = createSpaceStationWeb({ eventsTable: 'events', fetch: async () => { attempts++; return { ok: attempts > 1, status: 500 }; } });
  telemetry.track('once');
  assert.deepEqual(await telemetry.flush(), { sent: 0, failed: 1, dropped: 0, queued: 1 });
  assert.deepEqual(await telemetry.flush(), { sent: 1, failed: 0, dropped: 0, queued: 0 });
  assert.equal(attempts, 2);
});

test('opt-out drops queued data and analytics sample rate zero drops automatic records', async () => {
  const requests = [];
  const telemetry = createSpaceStationWeb({ analyticsTable: 'analytics', eventsTable: 'events', sampleRate: 0, fetch: async (_, init) => { requests.push(init); return { ok: true }; } });
  telemetry.analytics('manual', { ok: true });
  telemetry.track('secret');
  telemetry.setEnabled(false);
  assert.equal(telemetry.isEnabled(), false);
  const result = await telemetry.flush();
  assert.equal(result.queued, 0);
  assert.equal(requests.length, 0);
});

