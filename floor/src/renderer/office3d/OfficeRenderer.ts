// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The voxel office in three.js: the furnished room batched by colour, paper
// materials with a faint grain, one warm sun with soft shadows, an
// orthographic three-quarter camera, and the crew. It draws what it is told
// and decides nothing about where anyone goes; `scene/motion.ts` does that.

import {
  BackSide, BoxGeometry, BufferAttribute, BufferGeometry, CanvasTexture, Color, ColorManagement, DataTexture, DirectionalLight, DoubleSide, Group,
  HemisphereLight, InstancedMesh, LinearSRGBColorSpace, Mesh, MeshBasicMaterial, MeshLambertMaterial, Object3D,
  OrthographicCamera, PCFShadowMap, PointLight, RepeatWrapping, RingGeometry, Scene, Sprite, SpriteMaterial, Vector3,
  WebGLRenderer,
} from 'three';
import type { Material, Texture } from 'three';

import type { Accessory, Look } from '../appearance.ts';
import type { CardNote } from '../scene/cardNotes.ts';
import type { Envelope } from '../scene/mailEnvelopes.ts';
import type { Posed } from '../scene/motion.ts';
import type { Badge } from '../scene/stations.ts';
import { BADGE_COLOURS, BADGE_SIZE, discPixels, drawBadge } from './badges.ts';
import type { Box } from './catalog.ts';
import { BELL_PARTS, TURN } from './catalog.ts';
import { PIVOTS, figureParts } from './figure.ts';
import type { Limb } from './figure.ts';
import type { PlacedFixture } from './furnish.ts';
import { furnish, scenePoint } from './furnish.ts';
import type { OfficeLayoutDefinition, PlanPoint } from './layout.ts';
import { DEFAULT_AZIMUTH, ELEVATION, ZOOM_LIMITS, followTarget, frustum } from './view.ts';
import type { Insets } from './view.ts';

// The look was tuned without colour management: colours go straight
// to the screen, and lights carry the intensities that look was tuned with.
ColorManagement.enabled = false;
export const LIGHT_SCALE = Math.PI;

export interface CrewMember {
  readonly id: string;
  readonly look: Look;
  readonly accessory: Accessory | null;
  readonly badge: Badge | null;
}

export interface OfficeFrame {
  readonly posed: readonly Posed[];
  readonly selectedId: string | null;
  readonly notes: readonly CardNote[];
  /** One colour per goal on the board. */
  readonly goals: readonly string[];
  readonly shipped: number;
  readonly envelopes: readonly Envelope[];
  /** False holds every figure still: reduced motion, or a view that is not live. */
  readonly animate: boolean;
}

export interface ScreenRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

const DISTANCE = 40;
export const OUTLINE = '#3b3229';
const ENVELOPE: readonly Box[] = [
  { colour: '#fbf7ee', size: { x: 0.36, y: 0.04, z: 0.26 }, at: { x: 0, y: 0, z: 0 } },
  { colour: '#e2574c', size: { x: 0.1, y: 0.05, z: 0.1 }, at: { x: 0, y: 0.02, z: 0 } },
];
const NOTE_COLOURS: Readonly<Record<string, string>> = { todo: '#f6d36b', claimed: '#9fd3ff', doing: '#9fd3ff',
  review: '#c3a6e8', blocked: '#e2574c', done: '#f3ece0' };

interface Figure {
  readonly root: Group;
  readonly geometries: readonly BufferGeometry[];
  readonly body: Group;
  readonly limbs: Readonly<Record<Exclude<Limb, 'body'>, Group>>;
  readonly badge: Sprite;
  heading: number;
  seated: boolean;
  member: CrewMember;
}

function sameLook(a: Look, b: Look): boolean {
  return a.skin === b.skin && a.hair === b.hair && a.style === b.style && a.pants === b.pants && a.shirt === b.shirt;
}

/**
 * The boxes of one rigid part of a figure as a single geometry, coloured per
 * vertex, so a figure costs one draw for each limb rather than one per box.
 */
function mergeBoxes(template: BoxGeometry, boxes: readonly Box[], grow = 0): BufferGeometry {
  const position = template.getAttribute('position');
  const normal = template.getAttribute('normal');
  const uv = template.getAttribute('uv');
  const index = template.getIndex();
  const corners = position.count;
  const faces = index?.count ?? 0;
  const positions = new Float32Array(boxes.length * corners * 3);
  const normals = new Float32Array(boxes.length * corners * 3);
  const uvs = new Float32Array(boxes.length * corners * 2);
  const colours = new Float32Array(boxes.length * corners * 3);
  const indices = new Uint16Array(boxes.length * faces);
  const colour = new Color();
  boxes.forEach((b, k) => {
    colour.set(b.colour);
    for (let v = 0; v < corners; v += 1) {
      const at = (k * corners + v) * 3;
      positions.set([position.getX(v) * (b.size.x + grow) + b.at.x, position.getY(v) * (b.size.y + grow) + b.at.y,
        position.getZ(v) * (b.size.z + grow) + b.at.z], at);
      normals.set([normal.getX(v), normal.getY(v), normal.getZ(v)], at);
      colours.set([colour.r, colour.g, colour.b], at);
      uvs.set([uv.getX(v), uv.getY(v)], (k * corners + v) * 2);
    }
    for (let i = 0; i < faces; i += 1) indices[k * faces + i] = (index?.getX(i) ?? 0) + k * corners;
  });
  const merged = new BufferGeometry();
  merged.setAttribute('position', new BufferAttribute(positions, 3));
  merged.setAttribute('normal', new BufferAttribute(normals, 3));
  merged.setAttribute('uv', new BufferAttribute(uvs, 2));
  merged.setAttribute('color', new BufferAttribute(colours, 3));
  merged.setIndex(new BufferAttribute(indices, 1));
  return merged;
}

export function grainTexture(): DataTexture {
  const size = 64;
  const data = new Uint8Array(size * size * 4);
  let seed = 7;
  for (let i = 0; i < data.length; i += 4) {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    const value = 232 + (seed % 23);
    data[i] = value;
    data[i + 1] = value;
    data[i + 2] = value - 3;
    data[i + 3] = 255;
  }
  const texture = new DataTexture(data, size, size);
  texture.wrapS = RepeatWrapping;
  texture.wrapT = RepeatWrapping;
  texture.needsUpdate = true;
  return texture;
}

/** One figure's meshes: the body, and each limb on a pivot that turns. The caller frees the geometries. */
export function figureMeshes(look: Look, accessory: Accessory | null, unit: BoxGeometry, solid: Material, outline: Material): {
  readonly body: Group;
  readonly limbs: Readonly<Record<Exclude<Limb, 'body'>, Group>>;
  readonly geometries: readonly BufferGeometry[];
} {
  const body = new Group();
  const limbs = Object.fromEntries((Object.keys(PIVOTS) as (keyof typeof PIVOTS)[]).map((limb) => {
    const pivot = new Group();
    pivot.position.set(PIVOTS[limb].x, PIVOTS[limb].y, PIVOTS[limb].z);
    body.add(pivot);
    return [limb, pivot];
  })) as Record<keyof typeof PIVOTS, Group>;
  const parts = figureParts(look, accessory);
  const geometries: BufferGeometry[] = [];
  for (const limb of ['body', ...Object.keys(PIVOTS)] as Limb[]) {
    const own = parts.filter((part) => part.limb === limb);
    const parent = limb === 'body' ? body : limbs[limb];
    const shape = mergeBoxes(unit, own.map((part) => part.box));
    const edge = mergeBoxes(unit, own.filter((part) => part.outline).map((part) => part.box), 0.05);
    geometries.push(shape, edge);
    const mesh = new Mesh(shape, solid);
    mesh.castShadow = true;
    parent.add(mesh, new Mesh(edge, outline));
  }
  return { body, limbs, geometries };
}

export class OfficeRenderer {
  private readonly renderer: WebGLRenderer;
  private readonly scene = new Scene();
  private readonly camera = new OrthographicCamera(-10, 10, 6, -6, 0.1, 200);
  private readonly unit = new BoxGeometry(1, 1, 1);
  private readonly grain = grainTexture();
  private readonly materials = new Map<string, MeshLambertMaterial>();
  private readonly outline = new MeshBasicMaterial({ color: OUTLINE, side: BackSide });
  private readonly figureMaterial = new MeshLambertMaterial({ vertexColors: true, map: this.grain });
  private readonly hemi = new HemisphereLight(0xfffaf0, 0x9c8f7c, 0.85 * LIGHT_SCALE);
  private readonly sun = new DirectionalLight(0xffffff, 0.62 * LIGHT_SCALE);
  private readonly glass = new MeshLambertMaterial({ color: '#bcd8e6' });
  private readonly pane = new MeshLambertMaterial({ color: '#bfe3f0', transparent: true, opacity: 0.32 });
  private readonly bulb = new MeshBasicMaterial({ color: '#d8cfae' });
  private readonly lamps: PointLight[] = [];
  private serverLights: InstancedMesh | null = null;
  private readonly screens = new Map<number, Mesh[]>();
  private readonly steam: Mesh[] = [];
  private readonly clockHands: Group[] = [];
  private readonly bells: Group[] = [];
  private readonly shelf: Mesh[] = [];
  private readonly goalNotes: Mesh[] = [];
  private readonly noteMeshes: Mesh[] = [];
  private readonly envelopes: Mesh[] = [];
  private envelope: readonly [BufferGeometry, BufferGeometry] | null = null;
  private readonly badges = new Map<Badge, Texture>();
  /** What the furnished room made that holds GPU memory, freed when the layout changes or the renderer goes. */
  private readonly owned: { dispose(): void }[] = [];
  private room = new Group();
  private readonly figures = new Map<string, Figure>();
  private readonly ring: Mesh;
  private deskSeats: readonly PlanPoint[] = [];
  private board: readonly Vector3[] = [];
  private backdrop = { day: new Color(), night: new Color() };
  private night = false;
  private readonly target = new Vector3(0, 0.6, 0);
  private readonly scratch = new Vector3();
  private azimuth = DEFAULT_AZIMUTH;
  private zoom = 1;
  private css = { width: 1, height: 1 };
  private insets: Insets = { left: 0, right: 0 };
  private clock = 0;
  private last: number | null = null;

  constructor(canvas: HTMLCanvasElement, private layout: OfficeLayoutDefinition) {
    this.renderer = new WebGLRenderer({ canvas, antialias: true });
    this.renderer.outputColorSpace = LinearSRGBColorSpace;
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = PCFShadowMap;
    this.scene.add(this.hemi);
    this.sun.castShadow = true;
    this.sun.shadow.mapSize.set(2048, 2048);
    this.sun.shadow.bias = -0.0006;
    this.scene.add(this.sun);
    this.ring = new Mesh(new RingGeometry(0.5, 0.66, 32),
      new MeshBasicMaterial({ color: '#f6d36b', transparent: true, opacity: 0.95, side: DoubleSide }));
    this.ring.rotation.x = -Math.PI / 2;
    this.ring.position.y = 0.03;
    this.ring.visible = false;
    this.setLayout(layout);
  }

  /** Furnish the room for `layout`, after freeing everything the previous room held. */
  setLayout(layout: OfficeLayoutDefinition): void {
    this.scene.remove(this.room);
    for (const resource of this.owned.splice(0)) resource.dispose();
    for (const material of this.materials.values()) material.dispose();
    this.materials.clear();
    for (const list of [this.lamps, this.steam, this.clockHands, this.bells, this.shelf, this.goalNotes]) list.length = 0;
    this.screens.clear();
    this.layout = layout;
    this.room = new Group();
    this.scene.add(this.room);
    this.backdrop = { day: new Color(layout.style.backdrop.day), night: new Color(layout.style.backdrop.night) };
    const reach = Math.max(layout.width, layout.depth) * 0.75;
    this.sun.position.set(layout.width * 0.45, 18, layout.depth * 0.5);
    Object.assign(this.sun.shadow.camera, { left: -reach, right: reach, top: reach, bottom: -reach, near: 1, far: 60 });
    this.sun.shadow.camera.updateProjectionMatrix();
    for (const at of layout.lamps) {
      const lamp = new PointLight(0xffd59a, 0, 7, 1);
      const point = scenePoint(layout, at, at.height);
      lamp.position.set(point.x, point.y, point.z);
      this.lamps.push(lamp);
      this.room.add(lamp);
    }
    const furnished = furnish(layout);
    this.batch(furnished.boxes);
    this.serverLights = this.fixtures(furnished.fixtures);
    this.deskSeats = layout.desks.map((seat) => seat.at);
    this.board = this.boardCorners();
    this.setNight(this.night);
    this.placeCamera();
  }

  private own<T extends { dispose(): void }>(resource: T): T {
    this.owned.push(resource);
    return resource;
  }

  private material(colour: string): MeshLambertMaterial {
    let material = this.materials.get(colour);
    if (material === undefined) {
      material = new MeshLambertMaterial({ color: colour, map: this.grain });
      this.materials.set(colour, material);
    }
    return material;
  }

  private mesh(b: Box, material: MeshLambertMaterial | MeshBasicMaterial, parent: Object3D): Mesh {
    const mesh = new Mesh(this.unit, material);
    mesh.scale.set(b.size.x, b.size.y, b.size.z);
    mesh.position.set(b.at.x, b.at.y, b.at.z);
    parent.add(mesh);
    return mesh;
  }

  private batch(boxes: readonly Box[]): void {
    const byColour = new Map<string, Box[]>();
    for (const b of boxes) byColour.set(b.colour, [...(byColour.get(b.colour) ?? []), b]);
    const place = new Object3D();
    for (const [colour, list] of byColour) {
      const batch = this.own(new InstancedMesh(this.unit, this.material(colour), list.length));
      list.forEach((b, index) => {
        place.position.set(b.at.x, b.at.y, b.at.z);
        place.scale.set(b.size.x, b.size.y, b.size.z);
        place.updateMatrix();
        batch.setMatrixAt(index, place.matrix);
      });
      batch.castShadow = true;
      batch.receiveShadow = true;
      this.room.add(batch);
    }
  }

  private fixtures(fixtures: readonly PlacedFixture[]): InstancedMesh | null {
    const lights = fixtures.filter((fixture) => fixture.kind === 'server-light');
    let serverLights: InstancedMesh | null = null;
    if (lights.length > 0) {
      serverLights = this.own(new InstancedMesh(this.unit, this.own(new MeshBasicMaterial({ color: '#ffffff' })), lights.length));
      const place = new Object3D();
      lights.forEach((fixture, index) => {
        const b = fixture.box as Box;
        place.position.set(b.at.x, b.at.y, b.at.z);
        place.scale.set(b.size.x, b.size.y, b.size.z);
        place.updateMatrix();
        serverLights?.setMatrixAt(index, place.matrix);
        serverLights?.setColorAt(index, new Color('#5fd08a'));
      });
      this.room.add(serverLights);
    }
    for (const fixture of fixtures) {
      const b = fixture.box;
      switch (fixture.kind) {
        case 'screen-line': {
          if (b === undefined || fixture.desk === undefined) break;
          const line = this.mesh(b, this.own(new MeshBasicMaterial({ color: b.colour })), this.room);
          line.visible = false;
          this.screens.set(fixture.desk, [...(this.screens.get(fixture.desk) ?? []), line]);
          break;
        }
        case 'bulb':
          if (b !== undefined) this.mesh(b, this.bulb, this.room);
          break;
        case 'glass':
          if (b !== undefined) this.mesh(b, this.glass, this.room);
          break;
        case 'pane':
          if (b !== undefined) this.mesh(b, this.pane, this.room);
          break;
        case 'steam':
          for (let k = 0; k < 3; k += 1) {
            const puff = this.mesh({ colour: '#ffffff', size: { x: 0.09, y: 0.09, z: 0.09 }, at: fixture.at },
              this.own(new MeshBasicMaterial({ color: '#ffffff', transparent: true, opacity: 0.6 })), this.room);
            puff.userData = { k, origin: fixture.at };
            this.steam.push(puff);
          }
          break;
        case 'clock-hand': {
          const mount = new Group();
          mount.position.set(fixture.at.x, fixture.at.y, fixture.at.z);
          mount.rotation.y = fixture.turn;
          const hand = new Group();
          this.mesh({ colour: '#2d3640', size: { x: 0.04, y: 0.26, z: 0.03 }, at: { x: 0, y: 0.11, z: 0 } }, this.material('#2d3640'), hand);
          mount.add(hand);
          this.clockHands.push(hand);
          this.room.add(mount);
          break;
        }
        case 'bell': {
          const bell = new Group();
          bell.position.set(fixture.at.x, fixture.at.y, fixture.at.z);
          for (const part of BELL_PARTS) this.mesh(part, this.material(part.colour), bell).castShadow = true;
          this.bells.push(bell);
          this.room.add(bell);
          break;
        }
        case 'shelf-slot': {
          const parcel = this.mesh({ colour: '', size: { x: 0.32, y: 0.26, z: 0.36 }, at: fixture.at },
            this.material(this.shelf.length % 2 === 0 ? '#d8b98a' : '#c9a774'), this.room);
          parcel.rotation.y = fixture.turn;
          parcel.castShadow = true;
          parcel.visible = false;
          this.shelf.push(parcel);
          break;
        }
        case 'goal-note': {
          const note = this.mesh({ colour: '', size: { x: 0.5, y: 0.42, z: 0.06 }, at: fixture.at }, this.material('#f6d36b'), this.room);
          note.rotation.y = fixture.turn;
          note.visible = false;
          this.goalNotes.push(note);
          break;
        }
        default:
          break;
      }
    }
    return serverLights;
  }

  private boardCorners(): readonly Vector3[] {
    const { wall, along, length } = this.layout.board;
    const half = length / 2 + 0.1;
    const ends = [along - half, along + half];
    return ends.flatMap((at) => [0.75, 2.75].map((y) => {
      const point = wall === 'north' ? scenePoint(this.layout, { x: at, y: 0 }, y) : scenePoint(this.layout, { x: 0, y: at }, y);
      return new Vector3(point.x, point.y, point.z);
    }));
  }

  private badge(kind: Badge): Texture {
    let texture = this.badges.get(kind);
    if (texture === undefined) {
      const context = typeof OffscreenCanvas === 'function' ? new OffscreenCanvas(BADGE_SIZE, BADGE_SIZE).getContext('2d') : null;
      if (context === null) {
        const size = 32;
        texture = new DataTexture(discPixels(BADGE_COLOURS[kind], size), size, size);
        texture.needsUpdate = true;
      } else {
        drawBadge(context, kind);
        texture = new CanvasTexture(context.canvas);
      }
      this.badges.set(kind, texture);
    }
    return texture;
  }

  private buildFigure(member: CrewMember): Figure {
    const root = new Group();
    const { body, limbs, geometries } = figureMeshes(member.look, member.accessory, this.unit, this.figureMaterial, this.outline);
    root.add(body);
    const badge = new Sprite(new SpriteMaterial({ depthTest: false }));
    badge.scale.set(0.62, 0.62, 1);
    badge.renderOrder = 10;
    badge.visible = false;
    root.add(badge);
    this.scene.add(root);
    return { root, geometries, body, limbs, badge, heading: 0, seated: false, member };
  }

  private disposeFigure(figure: Figure): void {
    this.scene.remove(figure.root);
    for (const geometry of figure.geometries) geometry.dispose();
    (figure.badge.material as SpriteMaterial).dispose();
  }

  /** Bring the figures in line with the roster: new hires get a figure, and a changed look gets a new one. */
  setCrew(members: readonly CrewMember[]): void {
    const keep = new Set(members.map((member) => member.id));
    for (const [id, figure] of this.figures) {
      if (!keep.has(id)) {
        this.disposeFigure(figure);
        this.figures.delete(id);
      }
    }
    for (const member of members) {
      let figure = this.figures.get(member.id);
      if (figure !== undefined && (!sameLook(figure.member.look, member.look) || figure.member.accessory !== member.accessory)) {
        const heading = figure.heading;
        this.disposeFigure(figure);
        figure = { ...this.buildFigure(member), heading };
      } else if (figure === undefined) {
        figure = this.buildFigure(member);
      }
      figure.member = member;
      const material = figure.badge.material as SpriteMaterial;
      figure.badge.visible = member.badge !== null;
      if (member.badge !== null && material.map !== this.badge(member.badge)) {
        material.map = this.badge(member.badge);
        material.needsUpdate = true;
      }
      this.figures.set(member.id, figure);
    }
  }

  setNight(night: boolean): void {
    this.night = night;
    this.scene.background = (night ? this.backdrop.night : this.backdrop.day).clone();
    this.hemi.color.set(night ? '#8fa2c8' : '#fffaf0');
    this.hemi.groundColor.set(night ? '#2b2a35' : '#9c8f7c');
    this.hemi.intensity = (night ? 0.45 : 0.85) * LIGHT_SCALE;
    this.sun.color.set(night ? '#9fb3e0' : '#ffffff');
    this.sun.intensity = (night ? 0.22 : 0.62) * LIGHT_SCALE;
    this.glass.color.set(night ? '#2c3e66' : '#bcd8e6');
    this.bulb.color.set(night ? '#ffd27a' : '#d8cfae');
    for (const lamp of this.lamps) lamp.intensity = night ? 1.6 * LIGHT_SCALE : 0;
  }

  resize(width: number, height: number, ratio: number): void {
    this.css = { width: Math.max(1, width), height: Math.max(1, height) };
    this.renderer.setPixelRatio(Math.min(ratio > 0 ? ratio : 1, 2));
    this.renderer.setSize(this.css.width, this.css.height, false);
    this.placeCamera();
  }

  /** Keep the office in the part of the canvas the floating panels leave uncovered. */
  setInsets(insets: Insets): void {
    if (insets.left === this.insets.left && insets.right === this.insets.right) return;
    this.insets = insets;
    this.placeCamera();
  }

  turnBy(radians: number): void {
    this.azimuth += radians;
    this.placeCamera();
  }

  zoomBy(factor: number): void {
    this.zoom = Math.max(ZOOM_LIMITS.min, Math.min(ZOOM_LIMITS.max, this.zoom * factor));
    this.placeCamera();
  }

  get view(): { readonly zoom: number; readonly azimuth: number } {
    return { zoom: this.zoom, azimuth: this.azimuth };
  }

  private placeCamera(): void {
    Object.assign(this.camera, frustum(this.layout.width, this.layout.depth, this.css, this.insets, this.zoom));
    this.camera.updateProjectionMatrix();
    this.camera.position.set(
      this.target.x + Math.sin(this.azimuth) * Math.cos(ELEVATION) * DISTANCE,
      this.target.y + Math.sin(ELEVATION) * DISTANCE,
      this.target.z + Math.cos(this.azimuth) * Math.cos(ELEVATION) * DISTANCE,
    );
    this.camera.lookAt(this.target);
  }

  private planToScene(point: PlanPoint, y = 0): Vector3 {
    const at = scenePoint(this.layout, point, y);
    return this.scratch.set(at.x, at.y, at.z);
  }

  /** CSS pixels from the canvas's top-left corner. */
  private toScreen(point: Vector3): { x: number; y: number } {
    const ndc = point.clone().project(this.camera);
    return { x: ((ndc.x + 1) / 2) * this.css.width, y: ((1 - ndc.y) / 2) * this.css.height };
  }

  /** A figure's body on screen, never narrower than a fingertip. */
  figureRect(id: string): ScreenRect | null {
    const figure = this.figures.get(id);
    if (figure?.root.visible !== true) return null;
    const { x, z } = figure.root.position;
    const foot = this.toScreen(new Vector3(x, 0, z));
    const head = this.toScreen(new Vector3(x, figure.seated ? 1.25 : 1.65, z));
    const side = this.toScreen(new Vector3(x, 0, z).addScaledVector(this.camera.getWorldDirection(new Vector3()).cross(new Vector3(0, 1, 0)).normalize(), 0.35));
    const width = Math.max(44, Math.abs(side.x - foot.x) * 2);
    return { left: foot.x - width / 2, top: head.y, width, height: Math.max(1, foot.y - head.y) };
  }

  tagPoint(id: string): { x: number; y: number } | null {
    const figure = this.figures.get(id);
    if (figure?.root.visible !== true) return null;
    return this.toScreen(new Vector3(figure.root.position.x, figure.badge.position.y + 0.45, figure.root.position.z));
  }

  /** The badge over a figure's head on screen, never smaller than a fingertip. */
  badgeRect(id: string): ScreenRect | null {
    const figure = this.figures.get(id);
    if (figure?.root.visible !== true) return null;
    const { x, y, z } = figure.root.position;
    const centre = this.toScreen(new Vector3(x, y + figure.badge.position.y, z));
    const size = Math.max(44, (figure.badge.scale.y * this.css.height) / (this.camera.top - this.camera.bottom));
    return { left: centre.x - size / 2, top: centre.y - size / 2, width: size, height: size };
  }

  boardRect(): ScreenRect {
    const corners = this.board.map((corner) => this.toScreen(corner));
    const xs = corners.map((corner) => corner.x);
    const ys = corners.map((corner) => corner.y);
    return { left: Math.min(...xs), top: Math.min(...ys), width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
  }

  render(frame: OfficeFrame, now: number): void {
    const dt = this.last === null || !frame.animate ? 0 : Math.min(0.05, (now - this.last) / 1000);
    this.last = now;
    this.clock += dt;
    const t = this.clock;

    for (const lines of this.screens.values()) for (const line of lines) line.visible = false;
    let ringing = false;
    // A figure the frame does not pose, such as a hire still outside the door, is not drawn.
    for (const figure of this.figures.values()) figure.root.visible = false;
    for (const posed of frame.posed) {
      const figure = this.figures.get(posed.agentId);
      if (figure === undefined) continue;
      figure.root.visible = true;
      this.pose(figure, posed, frame, t, dt);
      if (posed.ringing) ringing = true;
    }
    this.followSelection(frame);

    if (this.serverLights !== null) {
      const colour = new Color();
      for (let k = 0; k < this.serverLights.count; k += 1) {
        this.serverLights.setColorAt(k, colour.set(Math.sin(t * 3 + k * 1.7) > 0.6 ? '#e0a12c' : '#5fd08a'));
      }
      if (this.serverLights.instanceColor !== null) this.serverLights.instanceColor.needsUpdate = true;
    }
    for (const hand of this.clockHands) hand.rotation.z = -t * 0.4;
    for (const bell of this.bells) bell.rotation.z = ringing && frame.animate ? Math.sin(t * 30) * 0.25 : 0;
    for (const puff of this.steam) {
      const { k, origin } = puff.userData as { k: number; origin: { x: number; y: number; z: number } };
      const phase = (t * 0.5 + k / 3) % 1;
      puff.position.set(origin.x + Math.sin(phase * 6 + k) * 0.05, origin.y + phase * 0.8, origin.z);
      (puff.material as MeshBasicMaterial).opacity = 0.6 * (1 - phase);
    }
    this.shelf.forEach((parcel, index) => { parcel.visible = index < frame.shipped; });
    this.goalNotes.forEach((note, index) => {
      const colour = frame.goals[index];
      note.visible = colour !== undefined;
      if (colour !== undefined) note.material = this.material(colour);
    });
    this.placeNotes(frame.notes);
    this.placeEnvelopes(frame.envelopes, t);
    if (this.ring.visible) this.ring.rotation.z = t * 0.8;
    this.renderer.render(this.scene, this.camera);
  }

  private placeNotes(notes: readonly CardNote[]): void {
    while (this.noteMeshes.length < notes.length) {
      const mesh = new Mesh(this.unit, this.material('#f6d36b'));
      mesh.castShadow = true;
      this.scene.add(mesh);
      this.noteMeshes.push(mesh);
    }
    this.noteMeshes.forEach((mesh, index) => {
      const note = notes[index];
      mesh.visible = note !== undefined;
      if (note === undefined) return;
      mesh.position.set(note.at.x, note.at.y, note.at.z);
      const flat = note.place.kind !== 'board';
      const onWest = this.layout.board.wall === 'west';
      if (flat) mesh.scale.set(0.3, 0.02, 0.22);
      else if (onWest) mesh.scale.set(0.06, 0.42, 0.5);
      else mesh.scale.set(0.5, 0.42, 0.06);
      mesh.material = this.material(NOTE_COLOURS[note.card.status] ?? '#f6d36b');
    });
  }

  private placeEnvelopes(envelopes: readonly Envelope[], t: number): void {
    this.envelope ??= [mergeBoxes(this.unit, ENVELOPE), mergeBoxes(this.unit, ENVELOPE, 0.05)];
    while (this.envelopes.length < envelopes.length) {
      const [paper, edge] = this.envelope;
      const envelope = new Mesh(paper, this.figureMaterial);
      envelope.add(new Mesh(edge, this.outline));
      this.scene.add(envelope);
      this.envelopes.push(envelope);
    }
    this.envelopes.forEach((group, index) => {
      const envelope = envelopes[index];
      group.visible = envelope !== undefined;
      if (envelope === undefined) return;
      const at = this.planToScene(envelope.point, 1.4 + envelope.lift * 2.2);
      group.position.copy(at);
      group.rotation.y = t * 3;
    });
  }

  private followSelection(frame: OfficeFrame): void {
    const selected = frame.selectedId === null ? undefined : this.figures.get(frame.selectedId);
    const lean = followTarget(selected === undefined ? null : { x: selected.root.position.x, z: selected.root.position.z }, this.zoom);
    const goal = new Vector3(lean.x, 0.6, lean.z);
    const ease = frame.animate ? 0.12 : 1;
    if (goal.distanceTo(this.target) > 0.001) {
      this.target.lerp(goal, ease);
      this.placeCamera();
    }
    if (selected === undefined) {
      this.ring.removeFromParent();
      this.ring.visible = false;
    } else {
      if (this.ring.parent !== selected.root) selected.root.add(this.ring);
      (this.ring.material as MeshBasicMaterial).color.set(selected.member.look.shirt);
      this.ring.visible = true;
    }
  }

  private pose(figure: Figure, posed: Posed, frame: OfficeFrame, t: number, dt: number): void {
    const live = frame.animate;
    const phase = t + (posed.agentId.charCodeAt(posed.agentId.length - 1) % 7);
    const at = this.planToScene(posed.foot);
    const walking = posed.walking && live;
    figure.root.position.set(at.x, walking ? Math.abs(Math.sin(phase * 10)) * 0.06 : 0, at.z);
    const goal = TURN[posed.facing];
    const turnLeft = ((goal - figure.heading + Math.PI * 3) % (Math.PI * 2)) - Math.PI;
    figure.heading += live ? turnLeft * Math.min(1, dt * 10) : turnLeft;
    figure.root.rotation.y = figure.heading;

    const seat = posed.walking ? null : posed.seat;
    const seated = seat !== null;
    figure.seated = seated;
    const atDesk = this.deskSeats.findIndex((spot) => spot.x === posed.foot.x && spot.y === posed.foot.y);
    figure.body.position.y = seat === null ? 0 : seat - 0.55 + 0.04;
    figure.body.position.z = seated ? -0.05 : 0;
    const { limbs } = figure;
    [limbs['leg-left'], limbs['leg-right']].forEach((leg, i) => {
      leg.rotation.x = seated ? -Math.PI / 2 : walking ? Math.sin(phase * 10 + i * Math.PI) * 0.6 : 0;
    });
    const sway = (speed: number, size: number, offset = 0) => (live ? Math.sin(phase * speed + offset) * size : 0);
    let left = walking ? sway(10, 0.5, Math.PI) : 0;
    let right = walking ? sway(10, 0.5) : 0;
    if (!posed.walking) {
      switch (posed.pose) {
        case 'type':
          left = -1.15 + sway(14, 0.08);
          right = -1.15 + sway(14, 0.08, 1.3);
          break;
        case 'point':
          right = -2.3 + sway(2, 0.25);
          break;
        case 'raise':
          right = -2.8;
          break;
        case 'ring':
          right = posed.ringing ? -1.6 + sway(18, 0.4) : 0;
          break;
        case 'ponder':
          right = -1.4 + sway(1.5, 0.15);
          break;
        case 'visit':
          right = -2.0 + sway(3, 0.3);
          left = -0.4 + sway(3, 0.3, Math.PI);
          break;
        default:
          break;
      }
    }
    limbs['arm-left'].rotation.x = left;
    limbs['arm-right'].rotation.x = right;
    if (posed.pose === 'type' && seated && atDesk >= 0) {
      (this.screens.get(atDesk) ?? []).forEach((line, k) => {
        line.visible = true;
        if (live) line.scale.x = 0.12 + 0.18 * Math.abs(Math.sin(t * 2 + k));
      });
    }
    figure.badge.position.y = (seated ? 1.9 : 2.25) + sway(2.5, 0.05);
  }

  dispose(): void {
    for (const figure of this.figures.values()) this.disposeFigure(figure);
    this.figures.clear();
    for (const material of this.materials.values()) material.dispose();
    for (const texture of this.badges.values()) texture.dispose();
    for (const geometry of this.envelope ?? []) geometry.dispose();
    for (const resource of [this.outline, this.figureMaterial, this.glass, this.pane, this.bulb, this.ring.geometry,
      this.ring.material as MeshBasicMaterial, ...this.owned]) resource.dispose();
    this.grain.dispose();
    this.unit.dispose();
    this.renderer.dispose();
  }
}
