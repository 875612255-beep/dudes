// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Crew totals: what the current crew has spent, added up from the rows the stream delivers.
//
// Mutation controls, each named by the test it turns red:
//   UNKNOWN_AS_ZERO  coerce an absent value to 0 in `count` ... "an agent that reports no spend is unknown…"
//   TOTAL_USD        sum `tokens` into the dollar total ........ "the crew's spend, tokens and turns add up…"

import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import test, { after, before, beforeEach } from 'node:test';

import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const root = resolve(import.meta.dirname, '..');
const ADA = '01JBQ8XK2M3N4P5Q6R7S8T9VWX';
const GRACE = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const LIN = '01M31CGZJMEFDYMJ4YBV76DJHQ';

const spend = (usd, tokens) => ({ usd, tokens, work_tokens: 0 });
const agent = (id, name, fields) => ({ id, name, role: 'builder', provider: 'claude', mode: 'managed', ...fields });

const CREW = [
  agent(ADA, 'Ada', { status: 'running', turns: 4, spent_today: spend(1_250_000, 12_000) }),
  agent(GRACE, 'Grace', { status: 'waiting', turns: 2, spent_today: spend(500_000, 3_400) }),
  agent(LIN, 'Lin', { status: 'running', turns: 0, spent_today: spend(0, 0) }),
];

let server;
let store;
let crewTotalsOf;
let crewTotalsText;
let CrewList;

before(async () => {
  server = await createServer({
    root,
    cacheDir: resolve(root, 'node_modules/.vite-crew-totals'),
    server: { middlewareMode: true, hmr: false },
    appType: 'custom',
    logLevel: 'silent',
  });
  store = await server.ssrLoadModule('/src/renderer/floor/store.ts');
  ({ crewTotalsOf, crewTotalsText } = await server.ssrLoadModule('/src/renderer/crewTotals.ts'));
  ({ CrewList } = await server.ssrLoadModule('/src/renderer/CrewList.tsx'));
});

after(async () => {
  await server?.close();
});

const text = (html) => html.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim();
const shown = (agents) => crewTotalsText(crewTotalsOf(agents));

test('the crew\'s spend, tokens and turns add up, and each status is counted', () => {
  const totals = crewTotalsOf(CREW);
  assert.deepEqual(totals.statuses, [{ status: 'running', count: 2 }, { status: 'waiting', count: 1 }]);
  assert.deepEqual(shown(CREW), { spent: '$1.75', tokens: '15,400', turns: '6' });
});

test('an agent that reports no spend is unknown, never a zero in the sum', () => {
  const rows = [CREW[0], agent(GRACE, 'Grace', { status: 'waiting' }), agent(LIN, 'Lin', { status: 'idle', turns: 'many', spent_today: { usd: -1 } })];
  assert.deepEqual(shown(rows), { spent: '$1.25 + 2 unknown', tokens: '12,000 + 2 unknown', turns: '4 + 2 unknown' });
  assert.deepEqual(shown([rows[1]]), { spent: 'unknown', tokens: 'unknown', turns: 'unknown' });
});

test('a real zero is a total, not unknown', () => {
  assert.deepEqual(shown([CREW[2]]), { spent: '$0.00', tokens: '0', turns: '0' });
});

test('the Crew panel shows the totals and follows the stream, retired agents left out', () => {
  store.resetFloorStore();
  const epoch = store.beginFloorEpoch();
  store.noteTransport('open');
  const retired = { id: '01HZX3Q4R5S6T7V8W9XYZ0ABCD', name: 'Old', role: 'builder', provider: 'claude',
    hired_at: 1, retired_at: 2, turns: 90, spend: spend(99_000_000, 9_999_999) };
  store.applyServerFrame(JSON.stringify({ type: 'floor.snapshot',
    projection: { lsn: 9, agents: CREW, retired_agents: [retired] } }), epoch);
  const panel = () => {
    const floor = store.getFloorState();
    return text(renderToStaticMarkup(createElement(CrewList, { agents: floor.agents, selectedId: null, selectable: true,
      stale: false, onSelectAgent() {}, onOpenCard() {} })).match(/<dl class="crew-totals"[\s\S]*?<\/dl>/)[0]);
  };
  assert.equal(panel(), 'Spent today $1.75 Tokens today 15,400 Turns 6');
  store.applyServerFrame(JSON.stringify({ type: 'floor.delta', delta: { lsn: 10,
    agents: [{ ...CREW[1], status: 'running', turns: 3, spent_today: spend(750_000, 5_000) }] } }), epoch);
  assert.equal(panel(), 'Spent today $2.00 Tokens today 17,000 Turns 7');
});
