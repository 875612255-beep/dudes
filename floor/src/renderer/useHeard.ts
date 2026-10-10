// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from 'react';

/** How long a hidden status line keeps what it said, so a reader browsing to it later does not meet old news. */
const HEARD_MS = 5_000;

/** Empties a status line five seconds after it says something. The function it returns cancels that, for when the line changes first. */
export function forgetLater(heard: string, forget: () => void): (() => void) | undefined {
  if (heard === '') return undefined;
  const timer = setTimeout(forget, HEARD_MS);
  return () => clearTimeout(timer);
}

/** A hidden status line that a screen reader hears when it changes, and that empties a few seconds after it speaks. */
export function useHeard(): readonly [string, (said: string) => void] {
  const [heard, setHeard] = useState('');
  useEffect(() => forgetLater(heard, () => setHeard('')), [heard]);
  return [heard, setHeard];
}
