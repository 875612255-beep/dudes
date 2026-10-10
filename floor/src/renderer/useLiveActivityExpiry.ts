// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from 'react';
import type { AgentRow } from './scene/cast.ts';
import { nextLiveActivityExpiry } from './liveActivity.ts';

/** Re-render at the next label expiry even when the stream sends no new frame. */
export function useLiveActivityExpiry(rows: readonly AgentRow[], live: boolean): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (!live) return;
    const now = performance.now();
    const expiry = nextLiveActivityExpiry(rows, now, Date.now());
    if (expiry === null) return;
    const timer = setTimeout(() => setVersion((current) => current + 1), Math.max(1, expiry - now));
    return () => clearTimeout(timer);
  }, [rows, live, version]);
  return version;
}
