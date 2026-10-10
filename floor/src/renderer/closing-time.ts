// SPDX-License-Identifier: MIT OR Apache-2.0

export type ClosingTime = 'announced' | 'complete' | null;

const AGENT_ID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;

export function decodeClosingTime(value: unknown): { readonly value: ClosingTime; readonly rejected: 0 | 1 } {
  if (value === undefined) return { value: null, rejected: 0 };
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return { value: null, rejected: 1 };
  const phase = value as Record<string, unknown>;
  if (phase.kind === 'announced' || phase.kind === 'complete') return { value: phase.kind, rejected: 0 };
  if (phase.kind === 'ack' && typeof phase.agent === 'string' && phase.agent.length === 26 && AGENT_ID.test(phase.agent)) {
    return { value: 'announced', rejected: 0 };
  }
  return { value: null, rejected: 1 };
}
