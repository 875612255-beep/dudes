// SPDX-License-Identifier: MIT OR Apache-2.0

export interface CompletionCheck {
  readonly id: string;
  readonly revision: string;
  readonly label: string;
}

/** How the goal's check ran before the agent's first turn, as the daemon recorded it. */
export type Baseline = 'failed' | 'may_already_pass';

const UTF8 = new TextEncoder();

/** One check's id, revision and label, or `null` unless each is within the daemon's bounds. */
export function checkIdentityOf(value: Record<string, unknown>): CompletionCheck | null {
  const { id, revision, label } = value;
  if (typeof id !== 'string' || id.length > 64 || !/^[a-z][a-z0-9_-]*$/.test(id)
    || typeof revision !== 'string' || !/^[0-9a-f]{64}$/.test(revision)
    || typeof label !== 'string' || label.length === 0 || /^[ \t\r\n]|[ \t\r\n]$/.test(label)
    || /\p{Cc}/u.test(label) || UTF8.encode(label).length > 128) return null;
  return { id, revision, label };
}

export function completionCheckOf(goal: Record<string, unknown>): CompletionCheck | null {
  const raw = goal.completion_check;
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null;
  return checkIdentityOf(raw as Record<string, unknown>);
}

/** The goal's recorded baseline, or `null` when it has none or its value is not one the daemon sends. */
export function baselineOf(goal: Record<string, unknown>): Baseline | null {
  const { baseline } = goal;
  return baseline === 'failed' || baseline === 'may_already_pass' ? baseline : null;
}
