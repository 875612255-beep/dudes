// SPDX-License-Identifier: MIT OR Apache-2.0
//
// How an agent looks: one of ten voxel looks, chosen at hire or by a stable
// hash of its id, with its Crew colour as its shirt and its role as an
// accessory. The office figure and every portrait read the same look.

import looks from './looks.json' with { type: 'json' };

/** The wire's authored characters, in order. Each names one look; the daemon's own list is checked against this file. */
export const CAST_CHARACTERS: readonly string[] = Object.freeze([...looks.characters]);
export const ACCENT_COLOURS = Object.freeze({
  amber: '#b87918', coral: '#c45642', mint: '#27846f',
  sky: '#397ca9', violet: '#8656ad', rose: '#b94e79',
});
export type AccentColour = keyof typeof ACCENT_COLOURS;

export type HairStyle = 'short' | 'long' | 'bun';

export interface Look {
  readonly skin: string;
  readonly hair: string;
  readonly style: HairStyle;
  readonly pants: string;
  readonly shirt: string;
}

const HAIR_STYLES: readonly HairStyle[] = ['short', 'long', 'bun'];

/** One look per authored character, `c` first. */
export const LOOKS: readonly Omit<Look, 'shirt'>[] = Object.freeze(looks.looks.map((look) => {
  const style = HAIR_STYLES.find((known) => known === look.style);
  if (style === undefined) throw new Error(`looks.json names an unknown hair style ${look.style}`);
  return Object.freeze({ ...look, style });
}));

export type Accessory = 'headphones' | 'glasses' | 'cap';

/** Checked in this order, each as a whole word of the role text, case aside. */
const ACCESSORIES: readonly (readonly [RegExp, Accessory])[] = [
  [/\bbuilder\b/i, 'headphones'],
  [/\breviewer\b/i, 'glasses'],
  [/\bops\b/i, 'cap'],
];

export interface AppearanceRow {
  readonly id: string;
  readonly character?: unknown;
  readonly accent?: unknown;
}

export function isCastCharacter(value: unknown): value is string {
  return typeof value === 'string' && CAST_CHARACTERS.includes(value);
}

export function isAccentColour(value: unknown): value is AccentColour {
  return typeof value === 'string' && Object.hasOwn(ACCENT_COLOURS, value);
}

export function validAppearance(row: { readonly character?: unknown; readonly accent?: unknown }): boolean {
  return (row.character == null || isCastCharacter(row.character))
    && (row.accent == null || isAccentColour(row.accent));
}

/** The legacy FNV-1a mapping uses the full AgentId text. */
export function hashId(id: string): number {
  let hash = 0x811c_9dc5;
  for (let at = 0; at < id.length; at += 1) {
    hash ^= id.charCodeAt(at);
    hash = Math.imul(hash, 0x0100_0193) >>> 0;
  }
  return hash >>> 0;
}

export function variantOf(agentId: string, variants: number): number {
  if (!Number.isInteger(variants) || variants < 1) {
    throw new Error(`there must be at least one look, received ${String(variants)}`);
  }
  return hashId(agentId) % variants;
}

const ACCENT_ORDER: readonly string[] = Object.values(ACCENT_COLOURS);

/** The agent's hire accent, or a stable one chosen from its AgentId when it has none or an unknown one. */
export function accentOf(row: AppearanceRow): string {
  return isAccentColour(row.accent) ? ACCENT_COLOURS[row.accent]
    : ACCENT_ORDER[hashId(row.id) % ACCENT_ORDER.length] ?? ACCENT_COLOURS.sky;
}

export function appearanceOf(row: AppearanceRow, variants: number): { readonly variant: number; readonly accent: string | null } {
  if (!validAppearance(row)) throw new TypeError('unknown agent appearance');
  const automatic = variantOf(row.id, variants);
  const variant = isCastCharacter(row.character) ? CAST_CHARACTERS.indexOf(row.character) : automatic;
  if (variant >= variants) throw new TypeError('chosen character is absent from the looks');
  return { variant, accent: isAccentColour(row.accent) ? ACCENT_COLOURS[row.accent] : null };
}

/** The look of a row: its authored character, or the one its id hashes to, in its Crew colour. */
export function lookOf(row: AppearanceRow): Look {
  const { variant } = appearanceOf(row, LOOKS.length);
  return Object.freeze({ ...LOOKS[variant], shirt: accentOf(row) });
}

/** One string per distinct figure: its look and the accessory it wears. */
export function lookKey(look: Look, accessory: Accessory | null): string {
  return [look.skin, look.hair, look.style, look.pants, look.shirt, accessory ?? ''].join(' ');
}

/** The accessory the role text names, or `null` when it names none. */
export function accessoryOf(role: unknown): Accessory | null {
  if (typeof role !== 'string') return null;
  return ACCESSORIES.find(([word]) => word.test(role))?.[1] ?? null;
}
