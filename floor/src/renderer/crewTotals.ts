// SPDX-License-Identifier: MIT OR Apache-2.0

import type { AgentRow } from './scene/cast.ts';
import { fieldsOf, formatUsd } from './scene/label.ts';

/** A sum over the agents that reported a value, and how many did not. An absent value is never counted as zero. */
export interface Sum {
  readonly total: number;
  readonly unknown: number;
}

export interface CrewTotals {
  readonly agents: number;
  readonly statuses: readonly { readonly status: string; readonly count: number }[];
  /** Micro-dollars spent today. */
  readonly usd: Sum;
  /** Every token spent today, prompt-cache reads included. */
  readonly tokens: Sum;
  /** Turns issued over the agents' lives. */
  readonly turns: Sum;
}

const STATUS_ORDER = ['running', 'waiting', 'held', 'done', 'idle'];

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function count(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? Math.floor(value) : null;
}

function sumOf(values: readonly (number | null)[]): Sum {
  return values.reduce<Sum>((sum, value) => value === null
    ? { ...sum, unknown: sum.unknown + 1 } : { ...sum, total: sum.total + value }, { total: 0, unknown: 0 });
}

/** The crew's totals, from the current agents only. A retired agent's lifetime spend is a different period and has no row here. */
export function crewTotalsOf(agents: readonly AgentRow[]): CrewTotals {
  const counts = new Map<string, number>();
  for (const row of agents) {
    const status = fieldsOf(row).status || 'unknown';
    counts.set(status, (counts.get(status) ?? 0) + 1);
  }
  const rank = (status: string): number => {
    const at = STATUS_ORDER.indexOf(status);
    return at === -1 ? STATUS_ORDER.length : at;
  };
  const statuses = [...counts].map(([status, n]) => ({ status, count: n }))
    .sort((a, b) => rank(a.status) - rank(b.status) || (a.status < b.status ? -1 : 1));
  const spend = (row: AgentRow, field: string): number | null => count(record(row.spent_today)?.[field]);
  return {
    agents: agents.length,
    statuses,
    usd: sumOf(agents.map((row) => spend(row, 'usd'))),
    tokens: sumOf(agents.map((row) => spend(row, 'tokens'))),
    turns: sumOf(agents.map((row) => count(row.turns))),
  };
}

function withUnknown(known: string, sum: Sum, agents: number): string {
  if (sum.unknown === 0) return known;
  return sum.unknown === agents ? 'unknown' : `${known} + ${sum.unknown} unknown`;
}

/** The three figures as the Crew panel words them. */
export function crewTotalsText(totals: CrewTotals): { readonly spent: string; readonly tokens: string; readonly turns: string } {
  const number = (n: number): string => n.toLocaleString('en-US');
  return {
    spent: withUnknown(formatUsd(totals.usd.total), totals.usd, totals.agents),
    tokens: withUnknown(number(totals.tokens.total), totals.tokens, totals.agents),
    turns: withUnknown(number(totals.turns.total), totals.turns, totals.agents),
  };
}
