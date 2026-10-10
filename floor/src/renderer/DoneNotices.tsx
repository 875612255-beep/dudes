// SPDX-License-Identifier: MIT OR Apache-2.0

import type { DoneNotice } from './done.ts';
import { dismissDoneNotice } from './floor/store.ts';
import { handFocusAfterLeave } from './focus.ts';

function dismissId(notice: DoneNotice): string {
  return `done-dismiss-${notice.key}`;
}

/**
 * The region is drawn even when empty, because a screen reader reads out only
 * what is added to a live region it already knows.
 */
export function DoneNotices({ notices, leaveFocusTo }: {
  readonly notices: readonly DoneNotice[];
  readonly leaveFocusTo: string;
}) {
  const more = notices.length - 1;
  return (
    <div className="done-notices" aria-live="polite">
      {notices.length === 0 ? null : (
        <ol aria-label="Goals done">
          {notices.map((notice, at) => {
            const neighbour = notices[at + 1] ?? notices[at - 1];
            return (
              <li key={notice.key} className="done-notice" data-done-agent-id={notice.agentId}>
                <p className="agent-detail-kicker">Done</p>
                <p className="done-notice-met"><strong>{notice.name}</strong> <span>met its goal.</span></p>
                <p className="done-notice-goal" title={notice.goal}>{notice.goal}</p>
                <button type="button" id={dismissId(notice)} ref={handFocusAfterLeave}
                  data-focus-on-leave={neighbour === undefined ? leaveFocusTo : dismissId(neighbour)}
                  aria-label={`Dismiss the Done notice for ${notice.name}: ${notice.goal}`}
                  onClick={() => dismissDoneNotice(notice.key)}>Dismiss</button>
              </li>
            );
          })}
        </ol>
      )}
      {more > 0 ? <p className="done-notices-more"><span aria-hidden="true">{`+${more} more`}</span>
        <span className="done-notices-more-label">{`${more} more ${more === 1 ? 'goal' : 'goals'} done`}</span></p> : null}
    </div>
  );
}
