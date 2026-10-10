// SPDX-License-Identifier: MIT OR Apache-2.0

import type { AcknowledgedNote } from './chat.ts';
import type { ChatTurn, ChatView, PendingNote } from './model.ts';

export type ChatEntry = { readonly kind: 'context'; readonly noteLsn: string }
  | { readonly kind: 'message'; readonly noteLsn: string; readonly text: string;
    readonly pending: PendingNote | null; readonly turn: ChatTurn | null };

export function conversationEntries(view: ChatView, agent: string, acknowledged: readonly AcknowledgedNote[]): readonly ChatEntry[] {
  const entries = new Map<string, ChatEntry>();
  for (const note of acknowledged) {
    if (note.agent_id === agent) entries.set(note.note_lsn, { kind: 'message', noteLsn: note.note_lsn,
      text: note.text, pending: null, turn: null });
  }
  for (const note of view.pending) {
    if (note.agent_id === agent) entries.set(note.note_lsn, { kind: 'message', noteLsn: note.note_lsn,
      text: note.text, pending: note, turn: null });
  }
  for (const turn of view.turns) {
    if (turn.agent_id !== agent) continue;
    const entry = entries.get(turn.note_lsn);
    entries.set(turn.note_lsn, { kind: 'message', noteLsn: turn.note_lsn, text: turn.text,
      pending: entry?.kind === 'message' ? entry.pending : null, turn });
  }
  for (const receipt of view.context_sent) {
    if (receipt.agent_id === agent) entries.set(receipt.note_lsn, { kind: 'context', noteLsn: receipt.note_lsn });
  }
  return [...entries.values()].sort((a, b) => BigInt(a.noteLsn) < BigInt(b.noteLsn) ? -1 : 1);
}
