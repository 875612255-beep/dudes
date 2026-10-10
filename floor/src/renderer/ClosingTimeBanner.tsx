// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ClosingTime } from './closing-time.ts';
import './closing-time.css';

export function ClosingTimeBanner({ closing, stale }: { readonly closing: ClosingTime; readonly stale: boolean }) {
  if (closing === null) return null;
  return <p className="closing-time-banner" role="status" aria-atomic="true">
    <strong>{stale ? 'Last observed: ' : ''}{closing === 'announced' ? 'Closing time announced' : 'Closing phase complete'}</strong>
    {' '}{closing === 'announced' ? 'The daemon is coordinating closure.' : 'All participants acknowledged or the grace period elapsed.'}
  </p>;
}
