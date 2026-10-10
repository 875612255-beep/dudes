// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Whether the office can be drawn. A browser that cannot start WebGL, or
// loses its context, gets no drawing and a short line instead; Crew still
// lists every agent.

export type OfficeSupport = 'loading' | 'ready' | 'unavailable' | 'lost';

export const OFFICE_FALLBACK: Readonly<Record<'unavailable' | 'lost', string>> = Object.freeze({
  unavailable: 'This browser cannot draw the 3D office, so it is hidden. Crew still lists every agent.',
  lost: 'The browser stopped drawing the 3D office. Retry to draw it again. Crew still lists every agent.',
});

/** Start a renderer, or say the office is unavailable when the browser refuses one. */
export function startRenderer<T>(create: () => T): { readonly support: 'ready'; readonly renderer: T }
  | { readonly support: 'unavailable'; readonly renderer: null } {
  try {
    return { support: 'ready', renderer: create() };
  } catch {
    return { support: 'unavailable', renderer: null };
  }
}
