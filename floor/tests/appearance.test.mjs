// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';
import test from 'node:test';

import { accessoryOf, appearanceOf, hashId, lookOf } from '../src/renderer/appearance.ts';
import { crewRowOf } from '../src/renderer/crewRow.ts';
import { figureParts } from '../src/renderer/office3d/figure.ts';
import { applyServerFrame, beginFloorEpoch, getFloorState, noteTransport, resetFloorStore } from '../src/renderer/floor/store.ts';

const ADA = '01ARZ3NDEKTSV4RRFFQ69G5FAV';
const GRACE = '01M31CGZJMEFDYMJ4YBV76DJHQ';

test('legacy hires retain literal FNV choices and the neutral accent', () => {
  assert.equal(hashId(ADA), 1543523712);
  assert.equal(hashId(GRACE), 31062214);
  assert.deepEqual(appearanceOf({ id: ADA }, 10), { variant: 2, accent: null });
  assert.deepEqual(appearanceOf({ id: GRACE, character: null, accent: null }, 10), { variant: 4, accent: null });
  assert.deepEqual(appearanceOf({ id: ADA, accent: 'sky' }, 10), { variant: 2, accent: '#397ca9' });
  assert.deepEqual(appearanceOf({ id: ADA, character: 'h' }, 10), { variant: 5, accent: null });
});

test('HASH_LOOK mutant: hair and skin come from a stable hash of the AgentId, and the shirt is the Crew colour', () => {
  assert.deepEqual(lookOf({ id: ADA }), { skin: '#f2d3b5', hair: '#1d1d22', style: 'short', pants: '#2d3640', shirt: '#b87918' });
  assert.deepEqual(lookOf({ id: GRACE }), { skin: '#a8754f', hair: '#2a2a2a', style: 'short', pants: '#3b4a6b', shirt: '#8656ad' });
  assert.deepEqual(lookOf({ id: ADA }), lookOf({ id: ADA, name: 'renamed' }), 'the look follows the id, not the name');
  assert.equal(lookOf({ id: ADA, accent: 'coral' }).shirt, '#c45642');
  for (const row of [{ id: ADA }, { id: GRACE }, { id: ADA, accent: 'mint' }]) assert.equal(lookOf(row).shirt, crewRowOf(row).colour);
});

test('the live choice preview, office figure and Crew colour use the authored character and accent', () => {
  const row = { id: ADA, character: 'h', accent: 'sky', name: 'Ada', role: 'builder' };
  const look = lookOf(row);
  assert.deepEqual(look, { skin: '#e8b98f', hair: '#6b3b2a', style: 'long', pants: '#4a4a52', shirt: '#397ca9' });
  assert.equal(crewRowOf(row).colour, '#397ca9');
  const colours = new Set(figureParts(look, accessoryOf(row.role)).map((part) => part.box.colour));
  for (const colour of [look.skin, look.hair, look.pants, look.shirt, '#2d3640']) assert.ok(colours.has(colour), colour);
});

test('ACCESSORY_SUBSTRING mutant: the role names its accessory only as a whole word', () => {
  for (const [role, accessory] of [
    ['builder', 'headphones'], ['Senior Builder', 'headphones'], ['code reviewer', 'glasses'], ['Ops', 'cap'],
    ['ops on call', 'cap'], ['builder and reviewer', 'headphones'], ['reviewer, then ops', 'glasses'],
    ['DevOps lead', null], ['rebuilders', null], ['writer', null], ['', null], [undefined, null], [7, null],
  ]) assert.equal(accessoryOf(role), accessory, String(role));
  const extra = (accessory) => figureParts(lookOf({ id: ADA }), accessory).length - figureParts(lookOf({ id: ADA }), null).length;
  assert.deepEqual([extra('headphones'), extra('glasses'), extra('cap')], [3, 3, 2]);
});

test('malformed appearance rows never enter the Floor store', () => {
  for (const choice of [{ character: 'a' }, { character: '' }, { character: 5 },
    { accent: 'red' }, { accent: '#fff' }, { accent: '' }, { accent: ['sky'] }]) {
    resetFloorStore();
    const epoch = beginFloorEpoch();
    noteTransport('open');
    applyServerFrame(JSON.stringify({ type: 'floor.snapshot', projection: {
      lsn: 1, agents: [{ id: ADA, ...choice }, { id: GRACE, character: 'h', accent: 'sky' }],
    } }), epoch);
    assert.deepEqual(getFloorState().agents.map((row) => row.id), [GRACE]);
    assert.throws(() => appearanceOf({ id: ADA, ...choice }, 10), /unknown agent appearance/);
  }
});
