// SPDX-License-Identifier: MIT OR Apache-2.0

import type { FloorState } from '../floor/store.ts';
import { hireUnavailable } from '../settings/SettingsView.tsx';

export function hireSwitchOn(floor: FloorState): boolean {
  return floor.settings?.switches.hire ?? floor.onboarding !== null;
}

/** Add agent, or why Hire is not offered. `folded` puts the reason behind a one-line disclosure. */
export function HireEntry({ floor, id, onAddAgent, folded = false }: {
  readonly floor: FloorState;
  readonly id: string;
  readonly onAddAgent: (opener: HTMLButtonElement) => void;
  readonly folded?: boolean;
}) {
  if (!hireSwitchOn(floor)) {
    const reason = <p className="hire-entry-off">{hireUnavailable(floor)}</p>;
    if (!folded) return reason;
    return <details className="hire-off">
      <summary>{floor.settings?.switches.hire === false ? 'Hire is off' : 'Hire is unavailable'}</summary>
      {reason}
    </details>;
  }
  return <button type="button" className="hire-entry" id={id}
    onClick={(event) => onAddAgent(event.currentTarget)}>Add agent</button>;
}
