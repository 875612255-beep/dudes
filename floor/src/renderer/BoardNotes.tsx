// SPDX-License-Identifier: MIT OR Apache-2.0

import type { Ref } from 'react';

import './cardNotes.css';

/** The native button over the office's goal board, which opens the Board view. The office places it over the board each frame. */
export function BoardNotes({ count, hidden, goalsHidden = 0, buttonRef, onOpenTasks }: {
  readonly count: number;
  /** Board cards past the board's slots, which the board counts but does not draw. */
  readonly hidden: number;
  /** Goals past the board's goal notes, counted the same way. */
  readonly goalsHidden?: number;
  readonly buttonRef?: Ref<HTMLButtonElement>;
  readonly onOpenTasks: () => void;
}) {
  const cards = `${count} ${count === 1 ? 'card' : 'cards'}`;
  const goals = goalsHidden === 1 ? 'goal' : 'goals';
  // The name starts with the count the button shows, so a voice user can say what they see; a screen reader hears what it counts.
  const name = `${cards}${hidden === 0 ? '' : `, ${hidden} not on the board`}`
    + `${goalsHidden === 0 ? '' : `, ${goalsHidden} ${goals} not on the board`}, open Board`;
  return <button type="button" className="board-select" ref={buttonRef} aria-label={name}
    aria-controls="control-panel" title={`${cards} on Board`} onClick={onOpenTasks}>
    <span className="board-note-count">{cards}{hidden === 0 ? '' : ` · +${hidden}`}{goalsHidden === 0 ? '' : ` · +${goalsHidden} ${goals}`}</span>
  </button>;
}
