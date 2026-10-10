// SPDX-License-Identifier: MIT OR Apache-2.0
//
// An agent's portrait as its own office figure, head and shoulders from the
// front. One offscreen renderer draws every look once and keeps a copy of the
// pixels, so a portrait on the page holds no WebGL context of its own.

import {
  BackSide, BoxGeometry, DirectionalLight, HemisphereLight, LinearSRGBColorSpace, MeshBasicMaterial, MeshLambertMaterial,
  OrthographicCamera, Scene, Vector3, WebGLRenderer,
} from 'three';

import { lookKey } from './appearance.ts';
import type { Accessory, Look } from './appearance.ts';
import { LIGHT_SCALE, OUTLINE, figureMeshes, grainTexture } from './office3d/OfficeRenderer.ts';

/** Twice the portrait's CSS size. */
const PIXELS = 96;
const HEAD = new Vector3(0, 1.24, 0);

interface Studio {
  readonly canvas: OffscreenCanvas;
  readonly copy: OffscreenCanvasRenderingContext2D;
  readonly renderer: WebGLRenderer;
  readonly scene: Scene;
  readonly camera: OrthographicCamera;
  readonly unit: BoxGeometry;
  readonly solid: MeshLambertMaterial;
  readonly outline: MeshBasicMaterial;
}

let studio: Studio | null = null;
/** Settled by the first paint; a renderer that cannot open makes it `unavailable` for good. */
let support: 'unknown' | 'ready' | 'unavailable' = 'unknown';
// Pixels rather than an ImageBitmap: a bitmap taken from a WebGL canvas goes blank when that context is lost.
const pictures = new Map<string, ImageData>();

function openStudio(): Studio {
  const canvas = new OffscreenCanvas(PIXELS, PIXELS);
  const copy = new OffscreenCanvas(PIXELS, PIXELS).getContext('2d', { willReadFrequently: true });
  if (copy === null) throw new Error('no 2D canvas to copy the portrait into');
  const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.outputColorSpace = LinearSRGBColorSpace;
  renderer.setClearColor(0x000000, 0);
  renderer.setSize(PIXELS, PIXELS, false);
  const scene = new Scene();
  const sun = new DirectionalLight(0xffffff, 0.62 * LIGHT_SCALE);
  sun.position.set(1.5, 3, 2.5);
  scene.add(new HemisphereLight(0xfffaf0, 0x9c8f7c, 0.85 * LIGHT_SCALE), sun);
  // Turned a little and seen from a little above, so the boxes read as solid rather than as a flat face.
  const camera = new OrthographicCamera(-0.45, 0.45, 0.45, -0.45, 0.1, 20);
  camera.position.setFromSphericalCoords(5, Math.PI / 2 - 0.21, 0.35).add(HEAD);
  camera.lookAt(HEAD);
  return {
    canvas, copy, renderer, scene, camera, unit: new BoxGeometry(1, 1, 1),
    solid: new MeshLambertMaterial({ vertexColors: true, map: grainTexture() }),
    outline: new MeshBasicMaterial({ color: OUTLINE, side: BackSide }),
  };
}

function closeStudio(open: Studio): void {
  open.solid.map?.dispose();
  for (const resource of [open.unit, open.solid, open.outline, open.renderer]) resource.dispose();
}

/**
 * Whether this page may draw in 3D. A browser with 3D switched off still gives
 * an OffscreenCanvas a WebGL context, so ask a detached page canvas, whose
 * context is let go at once.
 */
function supported(): boolean {
  if (support === 'unknown') {
    const probe = document.createElement('canvas').getContext('webgl2');
    support = probe === null ? 'unavailable' : 'ready';
    probe?.getExtension('WEBGL_lose_context')?.loseContext();
  }
  return support === 'ready';
}

function pictureOf(look: Look, accessory: Accessory | null): ImageData | null {
  const key = lookKey(look, accessory);
  const kept = pictures.get(key);
  if (kept !== undefined) return kept;
  if (!supported()) return null;
  try {
    if (studio?.renderer.getContext().isContextLost() === true) {
      closeStudio(studio);
      studio = null;
    }
    studio ??= openStudio();
  } catch {
    support = 'unavailable';
    return null;
  }
  const open = studio;
  const { body, geometries } = figureMeshes(look, accessory, open.unit, open.solid, open.outline);
  open.scene.add(body);
  try {
    open.renderer.render(open.scene, open.camera);
    open.copy.clearRect(0, 0, PIXELS, PIXELS);
    open.copy.drawImage(open.canvas, 0, 0);
    if (open.renderer.getContext().isContextLost()) return null;
    const picture = open.copy.getImageData(0, 0, PIXELS, PIXELS);
    pictures.set(key, picture);
    return picture;
  } catch {
    return null;
  } finally {
    open.scene.remove(body);
    for (const geometry of geometries) geometry.dispose();
  }
}

/** Paint the agent's figure onto `target` at the picture's size, or say this browser cannot draw it. */
export function paintFigurePortrait(target: HTMLCanvasElement, look: Look, accessory: Accessory | null): boolean {
  const context = target.getContext('2d');
  if (context === null) return false;
  const picture = pictureOf(look, accessory);
  if (picture === null) return false;
  target.width = PIXELS;
  target.height = PIXELS;
  context.putImageData(picture, 0, 0);
  return true;
}
