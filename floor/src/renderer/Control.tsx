// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ReactNode } from 'react';
import { useState } from 'react';

import { ArchivedAgents } from './ArchivedAgents.tsx';
import type { RetiredAgentRow } from './archive.ts';
import { LogPanel } from './LogPanel.tsx';
import { ReorderHandle, useAgentOrder } from './AgentOrder.tsx';
import { activeCard } from './board.ts';
import { AnswerForm } from './answer/AnswerPanel.tsx';
import { journalPosition } from './format.ts';
import { closesPanel, handFocusOnLeave } from './focus.ts';
import { StreamReadStatus } from './ReadError.tsx';
import type { FloorState } from './floor/store.ts';
import { openQuestions } from './questions.ts';
import type { OpenQuestion } from './questions.ts';
import { fieldsOf } from './scene/label.ts';
import { SetupView } from './setup/SetupView.tsx';
import { SettingsView } from './settings/SettingsView.tsx';
import { Board, CardDetail } from './BoardPanel.tsx';
import { workspaceGroups, workspaceLabel } from './workspace.ts';
import { HireEntry } from './onboarding/HireEntry.tsx';

export type ControlView = 'overview' | 'questions' | 'board' | 'log' | 'setup' | 'settings';

/** Control's views, in top bar order. Setup and Settings take a fresh snapshot when they open. */
export const CONTROL_VIEWS: readonly { readonly view: ControlView; readonly label: string; readonly refresh: boolean }[] = [
  { view: 'overview', label: 'Overview', refresh: false },
  { view: 'questions', label: 'Questions', refresh: false },
  { view: 'board', label: 'Board', refresh: false },
  { view: 'log', label: 'Log', refresh: false },
  { view: 'setup', label: 'Setup', refresh: true },
  { view: 'settings', label: 'Settings', refresh: true },
];

export const CONTROL_TITLE_ID = 'control-title';

/** A view button's target: no view when that view is already open beside the office, else the view itself. */
export function toggledView(open: ControlView | null, pressed: ControlView, focused: boolean): ControlView | null {
  return open === pressed && !focused ? null : pressed;
}

export function viewButtonId(view: ControlView): string {
  return `view-${view}`;
}

function labelOf(view: ControlView): string {
  return CONTROL_VIEWS.find((entry) => entry.view === view)?.label ?? view;
}

function value(value: unknown): string {
  return typeof value === 'string' && value !== '' ? value : '—';
}

function focusOnArrival(node: HTMLHeadingElement | null): void {
  node?.focus();
}

export function OverviewSummary({ floor, selectedId, onHireAgain }: {
  readonly floor: FloorState;
  readonly selectedId: string | null;
  readonly onHireAgain?: (row: RetiredAgentRow, opener: HTMLButtonElement) => void;
}) {
  return (
    <div className="overview-summary">
      <StreamReadStatus floor={floor} />
      <dl>
        <div><dt>Current agents</dt><dd>{floor.agents.length}</dd></div>
        <div><dt>Journal position</dt><dd className="data">{journalPosition(floor.lsn)}</dd></div>
      </dl>
      <p>{selectedId === null
        ? 'Select an agent in the office or in Crew to open its details.'
        : 'The selected agent’s current projection and read-only Live work are open in its details.'}</p>
      <ArchivedAgents floor={floor} onHireAgain={onHireAgain} />
    </div>
  );
}

/** Focus mode's roster: every current agent by workspace, in this browser's own order. */
export function AgentsRoster({ floor, selectedId, onSelectAgent, onHireAgain, onAddAgent }: {
  readonly floor: FloorState;
  readonly selectedId: string | null;
  readonly onSelectAgent: (id: string) => void;
  readonly onHireAgain?: (row: RetiredAgentRow, opener: HTMLButtonElement) => void;
  readonly onAddAgent?: (opener: HTMLButtonElement) => void;
}) {
  const selectable = floor.phase === 'live' && !floor.stale;
  const { ordered, move } = useAgentOrder(floor.agents, 'focus');
  const groups = workspaceGroups(ordered);
  return (
    <div className="agents-roster">
      <StreamReadStatus floor={floor} />
      {floor.agents.length === 0 ? <p>No current agents.</p> : (
        groups.map((group, at) => <section key={group.id === null ? 'unrecorded' : `workspace-${group.id}`}
          className="workspace-roster-group" aria-labelledby={`workspace-roster-${at}`}>
          <h3 id={`workspace-roster-${at}`}>{group.label}</h3>
          {group.labelsDiffer ? <p className="workspace-roster-note">This workspace has agents with different labels.</p> : null}
          <ul aria-label={`Agents in ${group.label}`}>
            {group.agents.map((row) => {
              const fields = fieldsOf(row);
              return (
                <li key={row.id} data-reorder-agent={row.id} data-reorder-scope={`focus-${group.id ?? 'unrecorded'}`}>
                  <ReorderHandle agent={row.id} name={value(row.name)}
                    ids={group.agents.map((agent) => agent.id)} scope={`focus-${group.id ?? 'unrecorded'}`} onMove={move} />
                  <button type="button" className="agents-roster-row" data-agent-id={row.id}
                    aria-label={`Inspect ${value(row.name)} in focus`}
                    aria-pressed={selectedId === row.id}
                    aria-controls={selectedId === row.id ? 'selected-agent-detail' : undefined}
                    disabled={!selectable} onClick={() => onSelectAgent(row.id)}>
                    <strong>{value(row.name)}</strong>
                    <span>{value(row.role)} · {value(row.provider)}</span>
                    <span className="agents-roster-workspace">{workspaceLabel(row)}</span>
                    <span>{value(fields.status)}</span>
                    <span>{fields.goal === '' ? 'No goal' : fields.goal}</span>
                    <span>{fields.cost}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>)
      )}
      {onAddAgent === undefined ? null : <HireEntry floor={floor} id="hire-open-focus" onAddAgent={onAddAgent} />}
      <ArchivedAgents floor={floor} onHireAgain={onHireAgain} />
    </div>
  );
}

export function FocusRoster({ collapsed, onToggle, ...roster }: Parameters<typeof AgentsRoster>[0] & {
  readonly collapsed: boolean;
  readonly onToggle: () => void;
}) {
  return (
    <section className="focus-roster" aria-label="Focus roster">
      <button type="button" className="focus-roster-toggle" aria-expanded={!collapsed}
        aria-controls="focus-roster-list" onClick={onToggle}>
        {collapsed ? 'Show roster' : 'Hide roster'}
      </button>
      {collapsed ? null : <div id="focus-roster-list"><AgentsRoster {...roster} /></div>}
    </section>
  );
}

function Questions({ floor, questions }: {
  readonly floor: FloorState;
  readonly questions: readonly OpenQuestion[];
}) {
  return (
    <div className="questions">
      <StreamReadStatus floor={floor} />
      {questions.length === 0 ? <p>No agent is waiting on a question.</p> : (
        <>
          <p>{questions.length === 1 ? '1 open question' : `${questions.length} open questions`}, newest first.</p>
          <ol aria-label="Open questions">
            {questions.map((question) => (
              <li key={question.row.id} className="questions-entry" data-question-agent-id={question.row.id}>
                <h3>{question.name}</h3>
                <p>{question.text}</p>
                <dl className="questions-context">
                  <div><dt>Current goal</dt><dd>{fieldsOf(question.row).goal || 'No goal'}</dd></div>
                  <div><dt>Current active card</dt><dd>{activeCard(floor.cards, question.row.id)?.title ?? 'No active card'}</dd></div>
                </dl>
                <AnswerForm floor={floor} row={question.row} leaveFocusTo={CONTROL_TITLE_ID} />
              </li>
            ))}
          </ol>
        </>
      )}
      <section className="questions-history" aria-label="Recent answered questions">
        <h3>Recent answered questions</h3>
        <p>A bounded recent window. Answer text is not shown.</p>
        {floor.questionHistory.length === 0 ? <p>No recent answered questions.</p> : floor.agents.map((row) => {
          const history = floor.questionHistory.filter((question) => question.agent_id === row.id).slice().reverse();
          return history.length === 0 ? null : <section key={row.id} data-history-agent-id={row.id}>
            <h4>{value(row.name)}</h4>
            <ol aria-label={`Recent answered questions for ${value(row.name)}`}>
              {history.map((question) => <li key={question.question_lsn}>
                <p>{question.question}</p><span>Answered</span>
              </li>)}
            </ol>
          </section>;
        })}
      </section>
    </div>
  );
}

export function ControlNav({ floor, view, onToggle }: {
  readonly floor: FloorState;
  readonly view: ControlView | null;
  readonly onToggle: (view: ControlView) => void;
}) {
  const questions = openQuestions(floor.agents).length;
  return (
    <nav className="control-nav" aria-label="Views">
      {CONTROL_VIEWS.map((entry) => <button key={entry.view} type="button" id={viewButtonId(entry.view)}
        aria-pressed={view === entry.view} aria-controls={view === entry.view ? 'control-panel' : undefined}
        onClick={() => onToggle(entry.view)}>
        {entry.label}{entry.view === 'questions' && questions !== 0 ? ` (${questions})` : null}
      </button>)}
    </nav>
  );
}

export function Control({ floor, view, selectedId, onClose, onView, onHireAgain, onGuide }: {
  readonly floor: FloorState;
  readonly view: ControlView;
  readonly selectedId: string | null;
  readonly onClose: () => void;
  readonly onView: (view: ControlView) => void;
  readonly onHireAgain?: (row: RetiredAgentRow, opener: HTMLButtonElement) => void;
  readonly onGuide?: () => void;
}) {
  const [selectedCardId, setSelectedCardId] = useState<string | null>(null);
  const selectedCard = (floor.cards ?? []).find((card) => card.id === selectedCardId) ?? null;
  const panels: Record<ControlView, ReactNode> = {
    overview: <OverviewSummary floor={floor} selectedId={selectedId} onHireAgain={onHireAgain} />,
    questions: <Questions floor={floor} questions={openQuestions(floor.agents)} />,
    board: <Board floor={floor} selectedId={selectedCardId} onSelect={setSelectedCardId} />,
    log: <LogPanel floor={floor} />,
    setup: <SetupView floor={floor} onGuide={onGuide} />,
    settings: <SettingsView floor={floor} onSetup={() => onView('setup')} />,
  };
  const label = labelOf(view);
  return (
    <section className="control-panel" id="control-panel" data-view={view} aria-labelledby={CONTROL_TITLE_ID}
      ref={handFocusOnLeave} data-focus-on-leave={viewButtonId(view)}
      onKeyDown={(event) => { if (closesPanel(event)) { event.stopPropagation(); onClose(); } }}>
      <header className="control-panel-head">
        <h2 id={CONTROL_TITLE_ID} tabIndex={-1} ref={focusOnArrival}>{label}</h2>
        <button type="button" className="panel-close" aria-label={`Close ${label}`} title="Close (Escape)"
          onClick={onClose}>×</button>
      </header>
      <div className="control-view">{panels[view]}</div>
      {view === 'board' ? <div className="control-detail" id="selected-card-detail" key={selectedCardId ?? 'card-none'}
        ref={handFocusOnLeave} data-focus-on-leave={viewButtonId('board')}>
        <CardDetail floor={floor} card={selectedCard} />
      </div> : null}
    </section>
  );
}
