// SPDX-License-Identifier: MIT OR Apache-2.0

import type { AgentRow } from './scene/cast.ts';
import { crewTotalsOf, crewTotalsText } from './crewTotals.ts';

/** What the current crew has spent, added up from the rows the Floor already holds. */
export function CrewTotals({ agents }: { readonly agents: readonly AgentRow[] }) {
  if (agents.length === 0) return null;
  const totals = crewTotalsOf(agents);
  const text = crewTotalsText(totals);
  return (
    <dl className="crew-totals" aria-label="Crew totals">
      <div><dt>Spent today</dt><dd data-total="spent">{text.spent}</dd></div>
      <div><dt>Tokens today</dt><dd data-total="tokens">{text.tokens}</dd></div>
      <div><dt>Turns</dt><dd data-total="turns">{text.turns}</dd></div>
    </dl>
  );
}

/** The top bar's counts: the crew, then each status some agent has, by the records' own status words. */
export function CrewStats({ agents }: { readonly agents: readonly AgentRow[] }) {
  return (
    <ul className="crew-stats" aria-label="Crew by status">
      <li className="crew-stats-total">{`${agents.length} crew`}</li>
      {crewTotalsOf(agents).statuses.map(({ status, count }) => <li key={status} data-status={status}>
        {`${count} ${status}`}
      </li>)}
    </ul>
  );
}
