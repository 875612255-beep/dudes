// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The goal-change limits a snapshot carries when the operator turned goal
// change on.
//
// They ride one optional `goal` field beside `projection` on a
// `floor.snapshot`, present exactly when the daemon routes the goal action and
// absent on the default configuration. They are display metadata, never
// authority: the daemon checks every change again. So this reader fails
// closed. Anything absent, malformed or over the daemon's own bound means no
// goal form, and it never closes the stream.
//
// When the operator also turned renewal on, the sibling carries a `renew`
// member: the allowance each enrolled workspace gives a new goal. A malformed
// `renew` fails the whole sibling, so a page never offers a renewal beside a
// change it could not read the limits of.

/** What a new goal gets in one enrolled workspace. Decimals stay canonical text, as the daemon sent them. */
export interface RenewAllowance {
  readonly workspaceId: string;
  /** Turns, from "1" to "4294967295". */
  readonly hopCap: string;
  /** The goal's dollar cap in micro-dollars, or `null` for no cap. */
  readonly usd: string | null;
  /** The goal's token cap, or `null` for no cap. */
  readonly tokens: string | null;
}

export interface GoalLimits {
  /** Most UTF-8 bytes one goal text carries, measured after trimming ASCII whitespace. */
  readonly maxTextBytes: number;
  /** The enrolled workspaces' allowances, present exactly when the operator turned renewal on. */
  readonly renew?: readonly RenewAllowance[];
}

/** The daemon's bound on one goal's text. A sibling that promises more is not believed. */
const MOST_TEXT_BYTES = 2_048;

const MOST_WORKSPACES = 32;
const MOST_HOPS = 4_294_967_295n;
const MOST_U64 = 18_446_744_073_709_551_615n;
const WORKSPACE_ID = /^[A-Za-z0-9_-]{1,128}$/;
const CANONICAL = /^(?:0|[1-9][0-9]{0,19})$/;
const ENTRY_KEYS = ['workspace_id', 'hop_cap', 'goal_usd_micros', 'goal_tokens'];

function exactly(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const own = Object.keys(value);
  return own.length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

/** A canonical decimal `u64`, or `null` for the JSON null that means no cap. `undefined` is neither. */
function ceiling(value: unknown): string | null | undefined {
  if (value === null) return null;
  return typeof value === 'string' && CANONICAL.test(value) && BigInt(value) <= MOST_U64 ? value : undefined;
}

function allowance(value: unknown): RenewAllowance | null {
  if (!exactly(value, ENTRY_KEYS)) return null;
  const { workspace_id: id, hop_cap: hops } = value;
  const usd = ceiling(value.goal_usd_micros);
  const tokens = ceiling(value.goal_tokens);
  if (typeof id !== 'string' || !WORKSPACE_ID.test(id) || typeof hops !== 'string' || !CANONICAL.test(hops)
    || BigInt(hops) < 1n || BigInt(hops) > MOST_HOPS || usd === undefined || tokens === undefined) return null;
  return Object.freeze({ workspaceId: id, hopCap: hops, usd, tokens });
}

function decodeRenew(value: unknown): readonly RenewAllowance[] | null {
  if (!exactly(value, ['workspaces']) || !Array.isArray(value.workspaces)
    || value.workspaces.length > MOST_WORKSPACES) return null;
  const rows = value.workspaces.map(allowance);
  if (rows.some((row) => row === null)) return null;
  const workspaces = rows as RenewAllowance[];
  if (new Set(workspaces.map((row) => row.workspaceId)).size !== workspaces.length) return null;
  return Object.freeze(workspaces);
}

/** Read the optional `goal` sibling of one snapshot, or `null` for anything but its two shapes. */
export function decodeGoalLimits(value: unknown): GoalLimits | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const keys = Object.keys(value);
  const renewing = keys.length === 2 && keys.includes('renew');
  if (!keys.includes('max_text_bytes') || (keys.length !== 1 && !renewing)) return null;
  const { max_text_bytes: bytes, renew } = value as { readonly max_text_bytes: unknown; readonly renew?: unknown };
  if (typeof bytes !== 'number' || !Number.isSafeInteger(bytes) || bytes < 1 || bytes > MOST_TEXT_BYTES) {
    return null;
  }
  if (!renewing) return Object.freeze({ maxTextBytes: bytes });
  const workspaces = decodeRenew(renew);
  return workspaces === null ? null : Object.freeze({ maxTextBytes: bytes, renew: workspaces });
}
