// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The `checks` sibling: each enrolled workspace's completion checks, read
// strictly. Anything off its shape is "no checks known", never a partial list.

import assert from 'node:assert/strict';
import test from 'node:test';

import { decodeCheckCatalog } from '../src/renderer/onboarding/checks.ts';

const REV_A = 'a'.repeat(64);
const REV_B = '0123456789abcdef'.repeat(4);
const REV_C = 'c'.repeat(64);

const CHECKS = Object.freeze({ workspaces: [
  { workspace_id: 'repo_1', checks: [
    { id: 'default', label: 'Parser tests pass', revision: REV_A, may_already_pass: false },
    { id: 'contributing', label: 'CONTRIBUTING.md exists', revision: REV_B, may_already_pass: true },
  ] },
  { workspace_id: 'repo_2', checks: [
    { id: 'default', label: 'Site builds', revision: REV_C, may_already_pass: false },
  ] },
] });

const bytes = (value) => new TextEncoder().encode(JSON.stringify(value)).length;

/** A one-workspace sibling whose serialized form is exactly `target` bytes. */
function siblingOfSize(target) {
  const checks = [];
  const value = { workspaces: [{ workspace_id: 'w', checks }] };
  while (bytes(value) < target) checks.push({ id: `c${checks.length}`, label: 'x', revision: REV_A, may_already_pass: false });
  checks.pop();
  for (const check of checks) {
    if (bytes(value) >= target) break;
    check.label += 'x'.repeat(Math.min(target - bytes(value), 128 - check.label.length));
  }
  assert.equal(bytes(value), target, 'the fixture is not the size it claims');
  return value;
}

test('the checks sibling decodes exactly, in the daemon’s order, first check first', () => {
  assert.deepEqual(decodeCheckCatalog(structuredClone(CHECKS)), { workspaces: [
    { workspaceId: 'repo_1', checks: [
      { id: 'default', label: 'Parser tests pass', revision: REV_A, mayAlreadyPass: false },
      { id: 'contributing', label: 'CONTRIBUTING.md exists', revision: REV_B, mayAlreadyPass: true },
    ] },
    { workspaceId: 'repo_2', checks: [
      { id: 'default', label: 'Site builds', revision: REV_C, mayAlreadyPass: false },
    ] },
  ] });
  assert.deepEqual(decodeCheckCatalog({ workspaces: [] }), { workspaces: [] }, 'no enrolled workspace is a list');
});

test('every bound is the daemon’s, and a value just inside it decodes', () => {
  const one = (check) => decodeCheckCatalog({ workspaces: [{ workspace_id: 'repo_1', checks: [
    { id: 'default', label: 'Parser tests pass', revision: REV_A, may_already_pass: false, ...check }] }] });
  assert.equal(one({ id: 'a'.repeat(64) })?.workspaces[0].checks[0].id, 'a'.repeat(64));
  assert.equal(one({ id: 'a1_-' })?.workspaces[0].checks[0].id, 'a1_-');
  assert.equal(one({ label: 'é'.repeat(64) })?.workspaces[0].checks[0].label, 'é'.repeat(64), '128 UTF-8 bytes');
  assert.equal(one({ label: 'a b' })?.workspaces[0].checks[0].label, 'a b', 'an inner space');
  assert.equal(decodeCheckCatalog({ workspaces: [{ workspace_id: 'W'.repeat(128), checks: CHECKS.workspaces[1].checks }] })
    ?.workspaces[0].workspaceId, 'W'.repeat(128));
  assert.notEqual(decodeCheckCatalog(siblingOfSize(16_384)), null, '16,384 bytes is inside the bound');
  assert.equal(decodeCheckCatalog(siblingOfSize(16_385)), null, '16,385 bytes is over it');
});

test('anything absent, malformed, out of order, duplicated, over a bound or carrying another key is refused whole', () => {
  const workspace = (patch) => ({ workspaces: [{ ...structuredClone(CHECKS.workspaces[0]), ...patch }] });
  const check = (patch) => workspace({ checks: [{ ...CHECKS.workspaces[0].checks[0], ...patch }] });
  const without = (key) => {
    const { [key]: _gone, ...rest } = CHECKS.workspaces[0].checks[0];
    return workspace({ checks: [rest] });
  };
  const malformed = {
    absent: undefined,
    null: null,
    'an array': [],
    'a string': 'repo_1',
    'no workspaces key': {},
    'workspaces not a list': { workspaces: {} },
    'an extra top-level key': { ...structuredClone(CHECKS), cwd: '/srv/repo' },
    'an extra workspace key': workspace({ label: 'Parser repository' }),
    'no checks key': { workspaces: [{ workspace_id: 'repo_1' }] },
    'checks not a list': workspace({ checks: CHECKS.workspaces[0].checks[0] }),
    'a workspace with no checks': workspace({ checks: [] }),
    'a numeric workspace id': workspace({ workspace_id: 1 }),
    'a workspace id with a slash': workspace({ workspace_id: '../repo' }),
    'an empty workspace id': workspace({ workspace_id: '' }),
    'a 129-byte workspace id': workspace({ workspace_id: 'W'.repeat(129) }),
    'workspaces out of order': { workspaces: [...structuredClone(CHECKS).workspaces].reverse() },
    'a duplicate workspace': { workspaces: [CHECKS.workspaces[0], CHECKS.workspaces[0]] },
    'a check carrying its command': check({ argv: ['test', '-f', 'hello.txt'] }),
    'a check carrying its timeout': check({ timeout_s: 60 }),
    'a check carrying its directory': check({ cwd: '/srv/repo' }),
    'a check without an id': without('id'),
    'a check without a label': without('label'),
    'a check without a revision': without('revision'),
    'a check without may_already_pass': without('may_already_pass'),
    'a check that is not an object': workspace({ checks: ['default'] }),
    'an upper-case check id': check({ id: 'Default' }),
    'a check id starting with a digit': check({ id: '1default' }),
    'a check id starting with a dash': check({ id: '-default' }),
    'an empty check id': check({ id: '' }),
    'a 65-byte check id': check({ id: 'a'.repeat(65) }),
    'a check id with a space': check({ id: 'the default' }),
    'a numeric check id': check({ id: 7 }),
    'a duplicate check id': workspace({ checks: [CHECKS.workspaces[0].checks[0],
      { ...CHECKS.workspaces[0].checks[1], id: 'default' }] }),
    'an empty label': check({ label: '' }),
    'a leading space': check({ label: ' Parser tests pass' }),
    'a trailing space': check({ label: 'Parser tests pass ' }),
    'a trailing newline': check({ label: 'Parser tests pass\n' }),
    'a tab inside': check({ label: 'Parser\ttests' }),
    'a control character': check({ label: 'Parser\u0007' }),
    'a 129-byte label': check({ label: `${'é'.repeat(64)}a` }),
    'a numeric label': check({ label: 5 }),
    'an upper-case revision': check({ revision: 'A'.repeat(64) }),
    'a 63-character revision': check({ revision: 'a'.repeat(63) }),
    'a 65-character revision': check({ revision: 'a'.repeat(65) }),
    'a revision that is not hex': check({ revision: 'g'.repeat(64) }),
    'a string may_already_pass': check({ may_already_pass: 'false' }),
    'a numeric may_already_pass': check({ may_already_pass: 0 }),
    'a null may_already_pass': check({ may_already_pass: null }),
    'over 16 KiB': siblingOfSize(16_385),
  };
  for (const [name, value] of Object.entries(malformed)) {
    assert.equal(decodeCheckCatalog(value), null, `${name} was accepted`);
  }
});
