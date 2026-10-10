// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ReactNode } from 'react';

type TextPart = { readonly kind: 'text' | 'code'; readonly text: string };

function splitFences(text: string): readonly TextPart[] {
  const parts: TextPart[] = [];
  let kind: TextPart['kind'] = 'text';
  let lines: string[] = [];
  for (const line of text.split('\n')) {
    const fence = kind === 'code' ? /^ {0,3}```[ \t\r]*$/ : /^ {0,3}```[a-zA-Z0-9_+.-]*[ \t\r]*$/;
    if (fence.test(line)) {
      if (lines.length > 0) parts.push({ kind, text: lines.join('\n') });
      lines = [];
      kind = kind === 'text' ? 'code' : 'text';
    } else lines.push(line);
  }
  if (lines.length > 0) parts.push({ kind, text: lines.join('\n') });
  return parts;
}

function inlineText(text: string): readonly ReactNode[] {
  return text.split(/(\\[\\*`]|`[^`\r\n]+`|\*\*(?=\S)[^*\r\n]*?[^\\\s*]\*\*)/g).map((part, index) => {
    if (index % 2 === 0) return part;
    if (part.startsWith('\\')) return part.slice(1);
    if (part.startsWith('`')) return <code className="chat-inline-code" key={index}>{part.slice(1, -1)}</code>;
    return <strong key={index}>{inlineText(part.slice(2, -2))}</strong>;
  });
}

export function ChatText({ text, final = false }: { readonly text: string; readonly final?: boolean }) {
  return <div className={`chat-text${final ? ' chat-final-reply' : ''}`}>
    {splitFences(text).map((part, index) => part.kind === 'code'
      ? <pre className="chat-code" key={index}><code>{part.text}</code></pre>
      : <span key={index}>{inlineText(part.text)}</span>)}
  </div>;
}
