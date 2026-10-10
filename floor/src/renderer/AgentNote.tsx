// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from 'react';
import { MAX_AGENT_NOTE, readAgentNote, writeAgentNote } from './view.ts';

export function AgentNote({ agent }: { readonly agent: string }) {
  const [text, setText] = useState('');
  useEffect(() => { setText(readAgentNote(agent)); }, [agent]);
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState(true);
  function editNote(value: string): void {
    setText(value);
    setSaved(writeAgentNote(agent, value));
  }
  return <section className="agent-private-note" aria-label="Private note">
    <button type="button" aria-expanded={open} aria-controls={`private-note-${agent}`}
      onClick={() => setOpen((current) => !current)}>Private note{text === '' ? '' : ' · has text'}</button>
    <p>This note lives only in this browser. It is never sent to the daemon or the agent.</p>
    {open ? <div id={`private-note-${agent}`}>
      <label htmlFor={`private-note-field-${agent}`}>Your private note</label>
      <textarea id={`private-note-field-${agent}`} name="private_note" rows={4} maxLength={MAX_AGENT_NOTE}
        value={text} onChange={(event) => editNote(event.target.value)} />
      {saved ? null : <p role="status">Browser storage is unavailable. This edit lasts until this view closes.</p>}
    </div> : null}
  </section>;
}
