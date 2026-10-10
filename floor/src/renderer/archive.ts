// SPDX-License-Identifier: MIT OR Apache-2.0

import { PROVIDERS } from './setup/daemon.ts';
import type { ProviderName } from './setup/daemon.ts';
import { isAccentColour, isCastCharacter } from './appearance.ts';
import type { AccentColour } from './appearance.ts';
import { decodeWorktree } from './worktree.ts';
import type { Worktree } from './worktree.ts';

const ID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const UTF8 = new TextEncoder();
export const MAX_RETIRED_AGENTS = 64;
export const MAX_RETIRED_AGENT_BYTES = 32_768;

export interface RetiredAgentRow {
  readonly id: string;
  readonly name: string;
  readonly role: string;
  readonly character?: string;
  readonly accent?: AccentColour;
  readonly provider: ProviderName;
  readonly workspace_label?: string;
  readonly worktree?: Worktree;
  readonly hired_at: number;
  readonly retired_at: number;
  readonly turns: number;
  readonly spend: { readonly usd: number; readonly tokens: number; readonly work_tokens: number };
}

function object(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
}
function count(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}
function rowOf(value: unknown): RetiredAgentRow | null {
  const source = object(value);
  const provider = PROVIDERS.find((name) => name === source?.provider);
  if (source === null || typeof source.id !== 'string' || !ID.test(source.id)
    || typeof source.name !== 'string' || typeof source.role !== 'string'
    || (source.character !== undefined && !isCastCharacter(source.character))
    || (source.accent !== undefined && !isAccentColour(source.accent))
    || provider === undefined
    || (source.workspace_label !== undefined && typeof source.workspace_label !== 'string')
    || (source.worktree !== undefined && decodeWorktree(source.worktree) === null)
    || typeof source.hired_at !== 'number' || !Number.isSafeInteger(source.hired_at)
    || typeof source.retired_at !== 'number' || !Number.isSafeInteger(source.retired_at)
    || !count(source.turns)) return null;
  const spend = object(source.spend);
  if (spend === null || !count(spend.usd) || !count(spend.tokens) || !count(spend.work_tokens)) return null;
  return Object.freeze({ id: source.id, name: source.name, role: source.role, provider,
    ...(source.character === undefined ? {} : { character: source.character as string }),
    ...(source.accent === undefined ? {} : { accent: source.accent as AccentColour }),
    ...(source.workspace_label === undefined ? {} : { workspace_label: source.workspace_label as string }),
    ...(source.worktree === undefined ? {} : { worktree: decodeWorktree(source.worktree) as Worktree }),
    hired_at: source.hired_at, retired_at: source.retired_at, turns: source.turns,
    spend: Object.freeze({ usd: spend.usd, tokens: spend.tokens, work_tokens: spend.work_tokens }) });
}

export function decodeRetiredAgents(value: unknown): { readonly rows: readonly RetiredAgentRow[]; readonly rejected: number } {
  if (!Array.isArray(value)) return { rows: Object.freeze([]), rejected: value === undefined ? 0 : 1 };
  if (value.length > MAX_RETIRED_AGENTS || UTF8.encode(JSON.stringify(value)).length > MAX_RETIRED_AGENT_BYTES) {
    return { rows: Object.freeze([]), rejected: value.length || 1 };
  }
  const rows: RetiredAgentRow[] = [];
  const ids = new Set<string>();
  let rejected = 0;
  for (const raw of value) {
    const row = rowOf(raw);
    if (row === null || ids.has(row.id)) rejected += 1;
    else { rows.push(row); ids.add(row.id); }
  }
  return { rows: Object.freeze(rows), rejected };
}
