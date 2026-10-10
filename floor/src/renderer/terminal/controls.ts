// SPDX-License-Identifier: MIT OR Apache-2.0

import type { Terminal } from './grid.ts';
import { fitGrid, resizeTerminal } from './terminal.ts';

export const MIN_FONT_SIZE = 10;
export const MAX_FONT_SIZE = 32;
export const DEFAULT_FONT_SIZE = 14;
const FONT_SIZE_KEY = 'dudes.terminal.font-size';

export type ZoomCommand = 'zoom-in' | 'zoom-out' | 'zoom-reset';
export type TerminalCommand = ZoomCommand | 'copy' | 'expand';
export type TerminalPlatform = 'mac' | 'other';
export type CopyResult = { readonly kind: 'copied' | 'empty' }
  | { readonly kind: 'refused'; readonly selection: string };
type StoragePort = () => Pick<Storage, 'getItem' | 'setItem'> | null;
type ClipboardPort = () => Pick<Clipboard, 'writeText'> | null;

export function zoomFontSize(current: number, command: ZoomCommand): number {
  if (command === 'zoom-reset') return DEFAULT_FONT_SIZE;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, current + (command === 'zoom-in' ? 1 : -1)));
}

function browserStorage(): Storage | null {
  return typeof window === 'undefined' ? null : window.localStorage;
}

export function readFontSize(storage: StoragePort = browserStorage): number {
  try {
    const value = storage()?.getItem(FONT_SIZE_KEY);
    if (value !== undefined && value !== null && /^\d+$/.test(value)) {
      const size = Number(value);
      if (Number.isInteger(size) && size >= MIN_FONT_SIZE && size <= MAX_FONT_SIZE) return size;
    }
  } catch {}
  return DEFAULT_FONT_SIZE;
}

export function writeFontSize(size: number, storage: StoragePort = browserStorage): void {
  try {
    if (Number.isInteger(size) && size >= MIN_FONT_SIZE && size <= MAX_FONT_SIZE) {
      storage()?.setItem(FONT_SIZE_KEY, String(size));
    }
  } catch {}
}

export function terminalShortcut(event: {
  readonly code: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
}, platform: TerminalPlatform, scope: 'grid' | 'header'): Exclude<TerminalCommand, 'expand'> | null {
  if (event.altKey) return null;
  const modifier = platform === 'mac'
    ? event.metaKey && !event.ctrlKey && (!event.shiftKey || event.code === 'Equal')
    : scope === 'header' && event.ctrlKey && event.shiftKey && !event.metaKey;
  if (!modifier) return null;
  switch (event.code) {
    case 'Equal': return 'zoom-in';
    case 'Minus': return 'zoom-out';
    case 'Digit0': return 'zoom-reset';
    case 'KeyC': return 'copy';
    default: return null;
  }
}

export function copyTerminalSelection(screen: Pick<Terminal, 'getSelection'> | null,
  clipboard: ClipboardPort = () => navigator.clipboard ?? null): Promise<CopyResult> {
  const selection = screen?.getSelection() ?? '';
  if (selection === '') return Promise.resolve({ kind: 'empty' });
  const refused: CopyResult = { kind: 'refused', selection };
  try {
    const target = clipboard();
    if (target === null) return Promise.resolve(refused);
    return target.writeText(selection).then(() => ({ kind: 'copied' } as const), () => refused);
  } catch {
    return Promise.resolve(refused);
  }
}

export function fitTerminal(term: Pick<Terminal, 'element' | 'cols' | 'rows' | 'resize'>,
  host: Pick<HTMLElement, 'getBoundingClientRect'>): void {
  const drawn = term.element?.querySelector('.xterm-screen')?.getBoundingClientRect();
  if (drawn === undefined) return;
  const size = fitGrid(host.getBoundingClientRect(),
    { cols: term.cols, rows: term.rows, width: drawn.width, height: drawn.height });
  if (size !== null && (size.cols !== term.cols || size.rows !== term.rows)) term.resize(size.cols, size.rows);
  resizeTerminal();
}
