// SPDX-License-Identifier: MIT OR Apache-2.0

export type GoalResultView =
  | { readonly kind: 'missing' }
  | { readonly kind: 'unavailable' }
  | { readonly kind: 'not_run'; readonly position: number }
  | { readonly kind: 'observed'; readonly outcome: 'passed' | 'failed'; readonly position: number }
  | { readonly kind: 'inferred'; readonly outcome: 'passed' | 'failed'; readonly position: number };

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : null;
}

export function goalResultOf(goal: unknown): GoalResultView | null {
  const current = record(goal);
  if (current === null) return null;
  if (current.last_result === undefined || current.last_result === null) return { kind: 'missing' };
  const result = record(current.last_result);
  if (result === null || typeof result.pass !== 'boolean'
    || (result.provenance !== 'observed' && result.provenance !== 'inferred')
    || typeof result.lsn !== 'number' || !Number.isSafeInteger(result.lsn) || result.lsn < 0
    || (result.not_run !== undefined && typeof result.not_run !== 'boolean')
    || (result.not_run === true && result.pass)) {
    return { kind: 'unavailable' };
  }
  if (result.not_run === true) return { kind: 'not_run', position: result.lsn };
  return {
    kind: result.provenance,
    outcome: result.pass ? 'passed' : 'failed',
    position: result.lsn,
  };
}
