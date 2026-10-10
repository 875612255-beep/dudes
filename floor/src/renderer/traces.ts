// SPDX-License-Identifier: MIT OR Apache-2.0

import { PROVIDERS } from './setup/daemon.ts';
import type { ProviderName } from './setup/daemon.ts';

const ID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const DECIMAL = /^(0|[1-9][0-9]*)$/;
const MAX_U64 = 18_446_744_073_709_551_615n;
const FIELDS = ['agent_id', 'started_at', 'duration_ms', 'outcome', 'engine', 'tokens', 'cost_usd_micros'];
const OUTCOMES = new Set(['ended', 'failed', 'held', 'killed']);
const ENGINES = new Set<string>(PROVIDERS);
const UTF8 = new TextEncoder();
export const MAX_AGENT_TRACES = 8;
export const MAX_TRACES = 64;
export const MAX_AGENT_TRACE_BYTES = 1_200;
export const MAX_TRACE_BYTES = 11_520;

export interface TurnTrace {
  readonly agent_id: string;
  readonly started_at: number | null;
  readonly duration_ms: number | null;
  readonly outcome: 'ended' | 'failed' | 'held' | 'killed';
  readonly engine: ProviderName;
  readonly tokens: string | null;
  readonly cost_usd_micros: string | null;
}

function unsigned(value: unknown): value is string {
  return typeof value === 'string' && DECIMAL.test(value) && BigInt(value) <= MAX_U64;
}

export function decodeTraces(value: unknown): {
  readonly rows: readonly TurnTrace[];
  readonly rejected: number;
} {
  const empty = (rejected: number) => ({ rows: Object.freeze([]), rejected });
  if (value === undefined) return empty(0);
  if (!Array.isArray(value)) return empty(1);
  if (value.length > MAX_TRACES || UTF8.encode(JSON.stringify(value)).length > MAX_TRACE_BYTES) {
    return empty(value.length || 1);
  }
  const rows: TurnTrace[] = [];
  const counts = new Map<string, number>();
  for (const raw of value) {
    if (typeof raw !== 'object' || raw === null || Array.isArray(raw)
      || Object.keys(raw).length !== FIELDS.length || !FIELDS.every((field) => Object.hasOwn(raw, field))) return empty(value.length || 1);
    const source = raw as Record<string, unknown>;
    if (typeof source.agent_id !== 'string' || !ID.test(source.agent_id)
      || (source.started_at !== null && (!Number.isSafeInteger(source.started_at)))
      || (source.duration_ms !== null && (!Number.isSafeInteger(source.duration_ms) || (source.duration_ms as number) < 0))
      || typeof source.outcome !== 'string' || !OUTCOMES.has(source.outcome)
      || typeof source.engine !== 'string' || !ENGINES.has(source.engine)
      || (source.tokens !== null && !unsigned(source.tokens))
      || (source.cost_usd_micros !== null && !unsigned(source.cost_usd_micros))) return empty(value.length || 1);
    const count = (counts.get(source.agent_id) ?? 0) + 1;
    const own = [...rows.filter((row) => row.agent_id === source.agent_id), source];
    if (count > MAX_AGENT_TRACES || UTF8.encode(JSON.stringify(own)).length > MAX_AGENT_TRACE_BYTES) {
      return empty(value.length || 1);
    }
    counts.set(source.agent_id, count);
    rows.push(Object.freeze({ agent_id: source.agent_id, started_at: source.started_at,
      duration_ms: source.duration_ms, outcome: source.outcome, engine: source.engine,
      tokens: source.tokens, cost_usd_micros: source.cost_usd_micros }) as TurnTrace);
  }
  return { rows: Object.freeze(rows), rejected: 0 };
}
