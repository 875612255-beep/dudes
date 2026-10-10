// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The answer module loads through the same Vite SSR server as the views, or the
// drafts typed here would land in a second module instance the forms never read.

import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import test, { after, before, beforeEach } from 'node:test';

import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

import { loadStage } from './helpers/page.mjs';

const root = resolve(import.meta.dirname, '..');
const ADA = '01JBQ8XK2M3N4P5Q6R7S8T9VWX';
const GRACE = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const MAE = '01M31CGZJMEFDYMJ4YBV76DJHQ';
const LINUS = '01HZX8Q4W7N3K2M5P6R9T0VBCD';
const SIBLING = Object.freeze({ max_text_bytes: 4096 });

function waiting(id, name, question, questionLsn) {
  return {
    id, name, role: 'backend engineer', department: 'engineering', provider: 'claude',
    status: 'waiting', mode: 'managed', turns: 1, question, question_lsn: questionLsn,
  };
}

const ada = () => waiting(ADA, 'Ada', 'Ship 7f3a9c1e or wait for review?', '9');
const grace = () => waiting(GRACE, 'Grace', 'Which tag goes on the release?', '10');

let server;
let answer;
let store;
let openQuestions;
let stage;
let ControlNav;

before(async () => {
  server = await createServer({
    root,
    cacheDir: resolve(root, 'node_modules/.vite-ask-me'),
    server: { middlewareMode: true, hmr: false },
    appType: 'custom',
    logLevel: 'silent',
  });
  answer = await server.ssrLoadModule('/src/renderer/answer/answer.ts');
  store = await server.ssrLoadModule('/src/renderer/floor/store.ts');
  ({ openQuestions } = await server.ssrLoadModule('/src/renderer/questions.ts'));
  stage = await loadStage(server);
  ({ ControlNav } = await server.ssrLoadModule('/src/renderer/Control.tsx'));
});

after(async () => {
  await server?.close();
});

beforeEach(() => {
  store.resetFloorStore();
  answer.resetAnswer();
});

function subscribe(agents) {
  const epoch = store.beginFloorEpoch();
  store.noteTransport('open');
  const frame = { type: 'floor.snapshot', projection: { lsn: 12, agents }, answer: SIBLING };
  store.applyServerFrame(JSON.stringify(frame), epoch);
}

/** The stage with `view` open in Control, or none, and the agent named by `selectedId` in the drawer. */
function center(view, selectedId = null) {
  const floor = store.getFloorState();
  return stage({ floor, view, row: floor.agents.find((agent) => agent.id === selectedId) ?? null });
}

/** The top bar's Control buttons with `view` open. */
function nav(view) {
  return renderToStaticMarkup(createElement(ControlNav, { floor: store.getFloorState(), view, onToggle() {} }));
}

function entries(html) {
  return [...html.matchAll(/<li class="questions-entry" data-question-agent-id="([^"]+)">([\s\S]*?)<\/li>/g)]
    .map(([, agent, body]) => ({
      agent,
      name: body.match(/<h3>([^<]*)<\/h3>/)?.[1],
      question: body.match(/<\/h3><p>([^<]*)<\/p>/)?.[1],
      field: body.match(/<textarea[^>]*>([^<]*)<\/textarea>/)?.[1] ?? null,
    }));
}

test('open questions come newest first by journal position read as a whole number, unplaced ones last', () => {
  const rows = [
    waiting(ADA, 'Ada', 'Ship?', '9'),
    waiting(GRACE, 'Grace', 'Which tag?', '10'),
    waiting(MAE, 'Mae', 'Rebase first?', undefined),
    { ...waiting(LINUS, 'Linus', 'Asked before it ran?', '11'), status: 'running' },
    waiting('01KV5T3N8E2F4G6H7J9K0M1N2P', '', 'Merge now?', String(2n ** 53n + 1n)),
    waiting('01KV5T3N8E2F4G6H7J9K0M1N2Q', 'Kay', 'Wait for CI?', String(2n ** 53n)),
    waiting('01KV5T3N8E2F4G6H7J9K0M1N2R', 'Empty', '', '12'),
    waiting('01KV5T3N8E2F4G6H7J9K0M1N2S', 'Zed', 'Padded position?', '010'),
  ];
  assert.deepEqual(openQuestions(rows).map(({ row, name, text, lsn }) => [row.id, name, text, lsn]), [
    ['01KV5T3N8E2F4G6H7J9K0M1N2P', '01KV5T3N8E2F4G6H7J9K0M1N2P', 'Merge now?', 2n ** 53n + 1n],
    ['01KV5T3N8E2F4G6H7J9K0M1N2Q', 'Kay', 'Wait for CI?', 2n ** 53n],
    [LINUS, 'Linus', 'Asked before it ran?', 11n],
    [GRACE, 'Grace', 'Which tag?', 10n],
    [ADA, 'Ada', 'Ship?', 9n],
    [MAE, 'Mae', 'Rebase first?', null],
    ['01KV5T3N8E2F4G6H7J9K0M1N2S', 'Zed', 'Padded position?', null],
  ]);
});

test('Questions lists every waiting agent by name, newest first, each with its own answer field and a count', () => {
  subscribe([ada(), grace(), { ...waiting(LINUS, 'Linus', '', undefined), status: 'running' }]);
  const html = center('questions');
  assert.match(nav('questions'),
    /<button type="button" id="view-questions" aria-pressed="true" aria-controls="control-panel">Questions \(2\)<\/button>/);
  assert.match(html, /<h2 id="control-title" tabindex="-1">Questions<\/h2>/);
  assert.match(html, /2 open questions, newest first\./);
  assert.deepEqual(entries(html).map(({ agent, name, question }) => [agent, name, question]), [
    [GRACE, 'Grace', 'Which tag goes on the release?'],
    [ADA, 'Ada', 'Ship 7f3a9c1e or wait for review?'],
  ]);
  const fields = [...html.matchAll(/<label for="([^"]+)">Your answer<\/label><textarea id="([^"]+)" name="answer"/g)];
  assert.equal(fields.length, 2, 'each question does not have its own labelled field');
  assert.ok(fields.every(([, label, id]) => label === id), 'a label does not name its own field');
  assert.notEqual(fields[0][2], fields[1][2], 'two fields share one id, so the second label names the first field');
  assert.equal([...html.matchAll(/<textarea[^>]*data-focus-on-leave="control-title"/g)].length, 2,
    'a field that leaves Questions while focused does not hand the keyboard to the Questions heading');
  assert.match(html, /<section class="control-panel" id="control-panel" data-view="questions" aria-labelledby="control-title"/);
});

test('with nothing waiting, Questions says so and its top bar button carries no count', () => {
  subscribe([{ ...waiting(LINUS, 'Linus', '', undefined), status: 'running' }]);
  const html = center('questions');
  assert.match(nav('questions'), /id="view-questions" aria-pressed="true"[^>]*>Questions<\/button>/);
  assert.match(html, /<p>No agent is waiting on a question\.<\/p>/);
  assert.deepEqual(entries(html), []);
});

test('a question with no journal position is listed read-only, as the detail shows it', () => {
  subscribe([ada(), waiting(MAE, 'Mae', 'Rebase first?', undefined)]);
  assert.deepEqual(entries(center('questions')).map(({ agent, question, field }) => [agent, question, field]), [
    [ADA, 'Ship 7f3a9c1e or wait for review?', ''],
    [MAE, 'Rebase first?', null],
  ]);
  const detail = center(null, MAE);
  assert.ok(detail.includes('Rebase first?'), 'the detail does not show the question either');
  assert.doesNotMatch(detail, /<textarea/);
});

test('an unconfirmed view keeps each question on the list and takes every answer field away', () => {
  subscribe([ada(), grace()]);
  store.noteStale(true);
  const html = center('questions');
  assert.match(html, /View unconfirmed — waiting for the stream/);
  assert.deepEqual(entries(html).map(({ agent, field }) => [agent, field]), [[GRACE, null], [ADA, null]]);
});

test('a draft is the same draft in Questions and in the agent detail, whichever view is shown', () => {
  subscribe([ada(), grace()]);
  answer.editAnswerDraft(ADA, '9', 'use main');
  assert.deepEqual(entries(center('questions')).map(({ agent, field }) => [agent, field]),
    [[GRACE, ''], [ADA, 'use main']]);
  const detail = center(null, ADA);
  assert.match(detail, /data-selected-agent-id="01JBQ8XK2M3N4P5Q6R7S8T9VWX"[\s\S]*<textarea[^>]*>use main<\/textarea>/);

  answer.editAnswerDraft(ADA, '9', 'use main, then tag it');
  const both = center('questions', ADA);
  assert.deepEqual(entries(both).map(({ agent, field }) => [agent, field]),
    [[GRACE, ''], [ADA, 'use main, then tag it']]);
  const ids = [...both.matchAll(/<textarea id="([^"]+)"[^>]*>use main, then tag it<\/textarea>/g)].map(([, id]) => id);
  assert.equal(ids.length, 2, 'Ada\'s question is not drawn both in Questions and in her detail');
  assert.notEqual(ids[0], ids[1], 'the two fields for one question share an id');
});
