// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Whether a snapshot says the operator turned a Floor switch on.
//
// The daemon sends an optional sibling beside `projection` on a
// `floor.snapshot`, an empty object, exactly when it routes the actions the
// operator enabled: `control` for pause and resume, `terminal` for the
// terminal channel, `retire` for retiring an agent. The default configuration
// sends none of them. A sibling is display metadata, never authority: the
// daemon checks every request again. So this reader fails closed. Anything
// but that one empty object means off, and it never closes the stream.

/** Read one optional switch sibling of a snapshot: `true` only for `{}`. */
export function decodeSwitchSibling(value: unknown): boolean {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  return Object.keys(value).length === 0;
}
