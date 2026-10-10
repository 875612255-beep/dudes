// SPDX-License-Identifier: MIT OR Apache-2.0

import { recordedTime } from './format.ts';
import { MAX_AGENT_TRACES } from './traces.ts';
import type { TurnTrace } from './traces.ts';

function cost(micros: string | null): string {
  if (micros === null) return 'Not reported';
  const value = BigInt(micros);
  const dollars = value / 1_000_000n;
  const fraction = (value % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '');
  return `$${dollars}.${fraction || '0'}`;
}

function duration(ms: number | null): string {
  if (ms === null) return 'Not reported';
  if (ms < 1_000) return `${ms} ms`;
  return `${(ms / 1_000).toFixed(1)} s`;
}

export function TracesPanel({ traces }: { readonly traces: readonly TurnTrace[] }) {
  return <section className="agent-detail-traces" aria-label="Traces">
    <h3>Traces</h3>
    <p>{`Up to ${MAX_AGENT_TRACES} recent turns, fewer when rows are long.`}</p>
    <p>Usage is charged to a turn. A later turn can include earlier unreported use.</p>
    {traces.length === 0 ? <p>No turn traces available.</p> : <ol>
      {[...traces].reverse().map((trace, index) => <li key={index}>
        <dl>
          <div><dt>Start observed</dt><dd>{trace.started_at === null ? 'Not recorded' : recordedTime(trace.started_at)}</dd></div>
          <div><dt>Duration</dt><dd>{duration(trace.duration_ms)}</dd></div>
          <div><dt>Outcome</dt><dd>{trace.outcome}</dd></div>
          <div><dt>Engine</dt><dd>{trace.engine}</dd></div>
          <div><dt>Tokens</dt><dd>{trace.tokens === null ? 'Not reported' : `${BigInt(trace.tokens).toLocaleString()} tokens`}</dd></div>
          <div><dt>Cost</dt><dd>{cost(trace.cost_usd_micros)}</dd></div>
        </dl>
      </li>)}
    </ol>}
  </section>;
}
