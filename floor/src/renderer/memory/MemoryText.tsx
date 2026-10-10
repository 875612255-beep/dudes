// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Memory text drawn inert: React text children only, never markup, Markdown or a link. A
// character that draws nothing or changes how its neighbours draw is shown as a marker instead,
// one marker per run, each code point written `\u{…}` in lowercase hex.

import type { ReactNode } from 'react';

/**
 * Controls other than LF and TAB, format characters, default-ignorable, private-use and
 * unassigned code points, and the two Unicode line separators: the characters that hide text,
 * reorder it or vanish.
 */
const HIDDEN_RUN = /(?:(?![\n\t])[\p{Cc}\p{Cf}\p{Default_Ignorable_Code_Point}\p{Co}\p{Cn}\u2028\u2029])+/gu;

function escaped(run: string): string {
  let out = '';
  for (const character of run) out += `\\u{${(character.codePointAt(0) ?? 0).toString(16)}}`;
  return out;
}

export function MemoryText({ text }: { readonly text: string }) {
  const parts: ReactNode[] = [];
  let at = 0;
  for (const run of text.matchAll(HIDDEN_RUN)) {
    if (run.index > at) parts.push(text.slice(at, run.index));
    parts.push(<span key={run.index} className="memory-marker">{escaped(run[0])}</span>);
    at = run.index + run[0].length;
  }
  if (at < text.length) parts.push(text.slice(at));
  return <div className="memory-text" dir="ltr">{parts}</div>;
}
