// SPDX-License-Identifier: MIT OR Apache-2.0
//
// What the panels floating over the office cover of it. It is measured from
// the page rather than from a drawing, so an office without WebGL still lays
// its lines beside the panels.

import { coveredInsets, narrowBand } from './view.ts';
import type { Insets } from './view.ts';

/** The panels that float over the office's edges. */
const FLOATING_PANELS = '.crew-list, .control-panel, .agent-drawer';

export interface Cover {
  /** What the camera keeps clear of: a third of the office always stays in view. */
  readonly camera: Insets;
  /** How far the panels really reach in, for what is laid over the office. */
  readonly edges: Insets;
  /** Whether the band between the panels is too narrow for the office's lines and view buttons. */
  readonly narrow: boolean;
}

function sameInsets(a: Insets, b: Insets): boolean {
  return a.left === b.left && a.right === b.right;
}

/**
 * Watches what the panels on `stage` cover of the office `box`: on every
 * resize of the office or of the stage's side column, once a frame after any
 * panel opens or closes, and on each `measure()`. The covered edges stay on
 * the office as `--covered-left` and `--covered-right`, and `onCover` hears
 * each cover that differs from the last.
 */
export function watchCover(box: HTMLElement, stage: Element, rem: number, onCover: (cover: Cover) => void):
  { readonly measure: () => Cover; readonly stop: () => void } {
  let shown: Cover | null = null;
  let frame: number | null = null;

  function measure(): Cover {
    const office = box.getBoundingClientRect();
    const panels = [...stage.querySelectorAll(FLOATING_PANELS)].map((panel) => panel.getBoundingClientRect());
    const edges = coveredInsets(office, panels, 0);
    const cover = { camera: coveredInsets(office, panels), edges, narrow: narrowBand(office.width, edges, shown?.narrow ?? false, rem) };
    if (shown !== null && sameInsets(shown.camera, cover.camera) && sameInsets(shown.edges, edges) && shown.narrow === cover.narrow) {
      return shown;
    }
    box.style.setProperty('--covered-left', `${edges.left}px`);
    box.style.setProperty('--covered-right', `${edges.right}px`);
    shown = cover;
    onCover(cover);
    return cover;
  }

  const resizes = new ResizeObserver(() => { measure(); });
  resizes.observe(box);
  const side = stage.querySelector('.stage-side');
  if (side !== null) resizes.observe(side);
  // The stage also holds the terminal's rows, so a burst of changes is measured once, in the next frame.
  const panels = new MutationObserver(() => {
    frame ??= requestAnimationFrame(() => {
      frame = null;
      measure();
    });
  });
  panels.observe(stage, { childList: true, subtree: true });
  return {
    measure,
    stop() {
      resizes.disconnect();
      panels.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
    },
  };
}
