// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The camera's decisions, kept pure: how much of the world fits the part of
// the canvas no panel covers, and how far the view leans toward the selected
// agent.

/** A point on the floor, in scene units. */
export interface Flat {
  readonly x: number;
  readonly z: number;
}

export const ZOOM_LIMITS = Object.freeze({ min: 0.8, max: 2.6 });
/** The view opens from the south-east, so it sees the south and east faces of what stands on the floor. */
export const DEFAULT_AZIMUTH = Math.PI / 4;
export const ELEVATION = 0.6;
const WALL_HEIGHT = 3.4;

/**
 * Half the view's height, in units, that fits a `width` by `depth` floor at
 * zoom 1 from any side: turning the office never zooms it in or out.
 */
export function fitHalfHeight(width: number, depth: number, aspect: number): number {
  const diagonal = Math.hypot(width, depth);
  const tall = (diagonal * Math.sin(ELEVATION) + WALL_HEIGHT * Math.cos(ELEVATION)) / 2;
  return 0.92 * Math.max(tall, diagonal / 2 / Math.max(aspect, 0.01));
}

/** How far panels floating over the canvas cover its left and right edges, in CSS pixels. */
export interface Insets {
  readonly left: number;
  readonly right: number;
}

export interface Rect {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
}

/**
 * The edges that panels cover: a panel taller than a third of the office
 * covers the side its middle is on, as far as it reaches in. At least `keep`
 * of the office's width stays uncovered, however wide the panels are: a third
 * for the camera, and none for what is laid over the office, which must stay
 * clear of the panels themselves.
 */
export function coveredInsets(office: Rect, panels: readonly Rect[], keep = 1 / 3): Insets {
  const width = office.right - office.left;
  const height = office.bottom - office.top;
  let left = 0;
  let right = 0;
  for (const panel of panels) {
    const overlap = Math.min(panel.bottom, office.bottom) - Math.max(panel.top, office.top);
    if (overlap < height / 3 || panel.right <= office.left || panel.left >= office.right) continue;
    if ((panel.left + panel.right) / 2 < (office.left + office.right) / 2) left = Math.max(left, panel.right - office.left);
    else right = Math.max(right, office.right - panel.left);
  }
  const spare = width * keep;
  const excess = Math.max(0, left + right - (width - spare));
  return { left: Math.max(0, left - excess / 2), right: Math.max(0, right - excess / 2) };
}

/** Below this free band beside the panels, in rem, the office's lines and view buttons move to the top bar. */
const NARROW_BELOW_REM = 12;
/** Once there, they come back only above this band, so a band near the edge does not swap them back and forth. */
const WIDE_ABOVE_REM = 13;

/**
 * Whether the office's lines and view buttons belong in the top bar, from the
 * office's width and the edges the panels cover: under 12rem of free band they
 * move there, and they stay until the band is wider than 13rem. A hidden
 * office, as in focus mode, has no band to fit into.
 */
export function narrowBand(width: number, edges: Insets, wasNarrow: boolean, rem: number): boolean {
  if (width <= 0) return false;
  return width - edges.left - edges.right < (wasNarrow ? WIDE_ABOVE_REM : NARROW_BELOW_REM) * rem;
}

/**
 * The camera's frustum for a canvas of `size`, fitted to its uncovered part
 * and shifted so the office centres there rather than behind a panel.
 */
export function frustum(width: number, depth: number, size: { readonly width: number; readonly height: number },
  insets: Insets, zoom: number): { left: number; right: number; top: number; bottom: number } {
  const aspect = size.width / size.height;
  const visible = Math.max(1, size.width - insets.left - insets.right) / size.height;
  const half = fitHalfHeight(width, depth, visible) / zoom;
  const shift = ((insets.left - insets.right) / 2 / size.width) * 2 * half * aspect;
  return { left: -half * aspect - shift, right: half * aspect - shift, top: half, bottom: -half };
}

/** Whether a point in CSS pixels lands on a canvas of this size, so whatever is laid over it there is seen. */
export function onCanvas(point: { readonly x: number; readonly y: number },
  size: { readonly width: number; readonly height: number }): boolean {
  return point.x >= 0 && point.y >= 0 && point.x <= size.width && point.y <= size.height;
}

export interface Size {
  readonly width: number;
  readonly height: number;
}

/** A name tag on screen: anchored at its bottom middle, `x` and `y` in CSS pixels. */
export interface TagBox {
  readonly id: string;
  readonly x: number;
  readonly y: number;
  /** With the engine and the activity line. */
  readonly full: Size;
  /** The name alone. */
  readonly compact: Size;
  /** Kept full however crowded, as the selected agent's tag is. */
  readonly whole: boolean;
}

export interface TagPlace {
  readonly bottom: number;
  readonly compact: boolean;
}

function edgesAt(x: number, bottom: number, size: Size): Rect {
  return { left: x - size.width / 2, right: x + size.width / 2, top: bottom - size.height, bottom };
}

function crosses(a: Rect, b: Rect): boolean {
  return a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
}

/**
 * Where each tag sits once crowded tags make room. A tag whose full box would
 * cross another's at their figures shows the name alone; then the tag nearest
 * the viewer, lowest on screen, keeps its place, and each one behind it rises
 * until it clears every tag already placed. No tag is dropped.
 */
export function spreadTags(tags: readonly TagBox[], gap = 2): ReadonlyMap<string, TagPlace> {
  const crowded = new Set(tags.filter((tag) => !tag.whole && tags.some((other) => other !== tag
    && crosses(edgesAt(tag.x, tag.y, tag.full), edgesAt(other.x, other.y, other.full)))).map((tag) => tag.id));
  const placed: Rect[] = [];
  const places = new Map<string, TagPlace>();
  for (const tag of [...tags].sort((a, b) => b.y - a.y || (a.id < b.id ? -1 : 1))) {
    const compact = crowded.has(tag.id);
    const size = compact ? tag.compact : tag.full;
    let box = edgesAt(tag.x, tag.y, size);
    for (let lifted = true; lifted;) {
      lifted = false;
      for (const other of placed) {
        if (crosses(box, other)) {
          box = edgesAt(tag.x, other.top - gap, size);
          lifted = true;
        }
      }
    }
    placed.push(box);
    places.set(tag.id, { bottom: box.bottom, compact });
  }
  return places;
}

/** A tag stays whole however crowded while its agent is selected, or while the agent's Inspect button has the keyboard. */
export function keepsWhole(tag: { readonly dataset: { selected?: string } }, inspect: Overlay | undefined): boolean {
  return tag.dataset.selected === 'true' || (inspect !== undefined && inspect.ownerDocument.activeElement === inspect);
}

/** A name tag element as the frame writes it: which form it shows, and where it stands. */
export interface TagElement {
  readonly dataset: { compact?: string };
  readonly style: { left: string; top: string };
}

/**
 * Spread crowded tags apart, then write each one's form and place. A tag that
 * shows the name alone says `data-compact="true"`, which the stylesheet reads.
 */
export function layTags(tags: readonly (TagBox & { readonly element: TagElement })[]): void {
  const spread = spreadTags(tags);
  for (const { id, element, x, y } of tags) {
    const at = spread.get(id);
    const compact = String(at?.compact === true);
    if (element.dataset.compact !== compact) element.dataset.compact = compact;
    element.style.left = `${x}px`;
    element.style.top = `${at?.bottom ?? y}px`;
  }
}

/** At zoom 1 the view centres the office; zoomed in, it leans toward the selected agent, all the way at full zoom. */
export function followTarget(selected: Flat | null, zoom: number): Flat {
  if (selected === null || zoom <= 1) return { x: 0, z: 0 };
  const pull = Math.max(0, Math.min(1, (zoom - 1) / (ZOOM_LIMITS.max - 1)));
  return { x: selected.x * pull, z: selected.z * pull };
}

/** One press of a turn key or button turns the view a sixteenth of a full turn. */
export const TURN_STEP = Math.PI / 8;

/** How far a key turns the view: the left arrow turns the office left, the right arrow right, and any modifier leaves the key alone. */
export function turnKey(event: Pick<KeyboardEvent, 'key' | 'altKey' | 'ctrlKey' | 'metaKey' | 'shiftKey'>): number | null {
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return null;
  return event.key === 'ArrowLeft' ? TURN_STEP : event.key === 'ArrowRight' ? -TURN_STEP : null;
}

const LINE_PIXELS = 40;
const ZOOM_PER_PIXEL = 0.002;

/**
 * The zoom a wheel turn asks for, or `null` to leave it to the page, which
 * scrolls. Only a wheel turned with Ctrl or Command held zooms the office; a
 * trackpad pinch arrives that way too.
 */
export function wheelZoom(event: Pick<WheelEvent, 'deltaY' | 'deltaMode' | 'ctrlKey' | 'metaKey'>): number | null {
  if (!event.ctrlKey && !event.metaKey) return null;
  return Math.exp(-(event.deltaMode === 0 ? event.deltaY : event.deltaY * LINE_PIXELS) * ZOOM_PER_PIXEL);
}

/** An element laid over the office that can take the keyboard. */
export interface Overlay {
  hidden: boolean | string;
  readonly ownerDocument: { readonly activeElement: unknown };
}

/**
 * Show or hide an overlay. One that hides while it has the keyboard, say a
 * figure the view just turned off the canvas, first hands the keyboard to
 * `fallback`, so focus never drops to the page.
 */
export function showOverlay(overlay: Overlay, shown: boolean, fallback: { focus(): void } | null | undefined): void {
  if (!shown && overlay.hidden !== true && overlay.ownerDocument.activeElement === overlay) fallback?.focus();
  overlay.hidden = !shown;
}
