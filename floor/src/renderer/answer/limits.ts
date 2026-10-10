// SPDX-License-Identifier: MIT OR Apache-2.0
//
// The answer limits a snapshot carries when the operator turned answering on.
//
// They ride one optional `answer` field beside `projection` on a
// `floor.snapshot`, present exactly when the daemon routes the answer action
// and absent on the default configuration. They are display metadata, never
// authority: the daemon checks every answer again. So this reader fails
// closed. Anything absent, malformed or over the daemon's own bound means no
// answer form, and it never closes the stream.

export interface AnswerLimits {
  /** Most UTF-8 bytes one answer carries, measured after trimming ASCII whitespace. */
  readonly maxTextBytes: number;
}

/** The daemon's bound on one answer's text. A sibling that promises more is not believed. */
const MOST_TEXT_BYTES = 4_096;

/** Read the optional `answer` sibling of one snapshot, or `null` for anything but its one shape. */
export function decodeAnswerLimits(value: unknown): AnswerLimits | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== 'max_text_bytes') return null;
  const bytes = (value as { readonly max_text_bytes: unknown }).max_text_bytes;
  if (typeof bytes !== 'number' || !Number.isSafeInteger(bytes) || bytes < 1 || bytes > MOST_TEXT_BYTES) {
    return null;
  }
  return Object.freeze({ maxTextBytes: bytes });
}
