// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';

export function accessibleNameOfHeading(html, id) {
  const heading = html.match(new RegExp(`<(h[1-6]) id="${id}"([^>]*)>([\\s\\S]*?)</\\1>`));
  assert.ok(heading, `no heading #${id} in the markup`);
  assert.doesNotMatch(heading[2], /aria-label|aria-hidden/, `#${id} is named by an attribute, not by its text`);
  return heading[3].replace(/<[^>]*>/g, '').replace(/\s+/g, ' ').trim();
}
