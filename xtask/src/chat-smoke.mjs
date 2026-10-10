// SPDX-License-Identifier: MIT OR Apache-2.0

import assert from 'node:assert/strict';

const ID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const LSN = /^[1-9][0-9]{0,19}$/;
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));

export async function runChatChrome({ cdp, fixture }) {
  const agents = await fixture.prepare();
  for (const name of ['managed', 'completed', 'attached', 'queued', 'held']) {
    assert.match(agents[name], ID, `Chat fixture ${name} identity`);
  }
  const cases = [];
  const until = async (expression, message) => {
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline) {
      const result = await cdp.evaluate(expression);
      if (result) return result;
      await wait(100);
    }
    throw new Error(message);
  };
  const click = async selector => {
    assert.equal(await cdp.evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element || element.disabled || element.getAttribute('aria-disabled') === 'true') return false;
      element.click(); return true;
    })()`), true, 'Chat control is unavailable');
  };
  const fill = async text => {
    assert.equal(await cdp.evaluate(`(() => {
      const element = document.querySelector('#chat-text');
      if (!element || element.disabled) return false;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(element, ${JSON.stringify(text)});
      element.dispatchEvent(new Event('input', { bubbles: true })); return true;
    })()`), true, 'Chat draft is unavailable');
  };
  const select = async agent => {
    await until(`Boolean(document.querySelector('.crew-row[data-agent-id="${agent}"]'))`, 'Chat fixture agent missing');
    await click(`.crew-row[data-agent-id="${agent}"]`);
    await until(`Boolean(document.querySelector('#agent-detail-tab-chat'))`, 'fresh snapshot did not offer Chat');
    await click('#agent-detail-tab-chat');
    await until(`document.querySelector('.chat-panel')?.dataset.chatAgentId === '${agent}'`, 'Chat did not select the agent');
  };
  const receipt = async expected => {
    const result = await until(`(() => {
      const element = document.querySelector('.chat-receipt');
      return element && element.dataset.chatOutcome !== 'sending'
        ? { state: element.dataset.chatOutcome, lsn: element.dataset.chatReceiptNoteLsn,
          reason: element.dataset.chatRefusalReason, text: element.textContent } : null;
    })()`, 'Chat request did not settle');
    assert.equal(result.state, expected, 'Chat request returned an unexpected outcome');
    return result;
  };
  const dismiss = async () => click('.chat-dismiss');
  const queue = async (agent, text, keyboard = false) => {
    await select(agent);
    await fill(text);
    if (keyboard) {
      assert.equal(await cdp.evaluate(`(() => {
        const element = document.querySelector('#chat-text');
        element.focus(); element.setSelectionRange(element.value.length, element.value.length); return true;
      })()`), true);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter',
        windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13, modifiers: 8, text: '\r' }, cdp.session);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter',
        windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13, modifiers: 8 }, cdp.session);
      assert.equal(await cdp.evaluate(`document.querySelector('#chat-text')?.value`), `${text}\n`, 'Shift+Enter did not preserve a newline');
      assert.equal(await cdp.evaluate(`Boolean(document.querySelector('.chat-receipt'))`), false, 'Shift+Enter queued a note');
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter',
        windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13, text: '\r' }, cdp.session);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter',
        windowsVirtualKeyCode: 13, nativeVirtualKeyCode: 13 }, cdp.session);
    } else {
      await click('.chat-send');
    }
    const result = await receipt('queued');
    assert.match(result.lsn, LSN, 'queued receipt has no exact note identity');
    assert.match(result.text, /Recorded in the journal.*delivery is not confirmed/s, 'queued receipt overstated delivery');
    await dismiss();
    return result.lsn;
  };
  const pending = lsn => `.chat-queue [data-chat-note-lsn="${lsn}"]`;
  const cancel = async (lsn, state = 'cancelled') => {
    await until(`Boolean(document.querySelector(${JSON.stringify(`${pending(lsn)} .chat-cancel`)}))`, 'pending note is not cancellable');
    await click(`${pending(lsn)} .chat-cancel`);
    const result = await receipt(state);
    assert.equal(result.lsn, lsn, 'cancellation receipt names another note');
    await dismiss();
    return result;
  };
  const text = () => cdp.evaluate(`document.querySelector('.chat-panel')?.textContent ?? ''`);

  fixture.caseStarted('managed');
  const midTurnText = 'A mid-turn browser note 🙂';
  const midTurn = await queue(agents.managed, midTurnText, true);
  fixture.caseStarted('managed_midturn');
  await fixture.assertNoMidTurnDispatch(agents.managed, midTurn);
  await until(`Boolean(document.querySelector(${JSON.stringify(pending(midTurn))}))`, 'mid-turn pending projection missing');
  assert.match(await text(), /Queued\. Delivery has not started/, 'mid-turn note overstated delivery');
  fixture.caseStarted('managed_task');
  const ordinary = await fixture.finishOrdinary(agents.managed);
  assert.match(ordinary.task, /\n\n\[browser note\]\nA mid-turn browser note 🙂\n\[end browser note\]/, 'ordinary task omitted the browser-note section');
  await until(`Boolean(document.querySelector('.chat-thread [data-chat-turn-state="running"]'))`, 'note did not join an ordinary turn');
  fixture.caseStarted('managed_preview');
  await fixture.emitPreview(agents.managed, 'A provisional preview');
  await until(`document.querySelector('.chat-preview')?.textContent.includes('A provisional preview')`, 'matching live preview missing');
  assert.equal(await cdp.evaluate(`document.querySelectorAll('.chat-final-reply').length`), 0, 'preview was promoted to final');
  await fixture.finishNote(agents.managed, 'A natural final ending…');
  await until(`document.querySelector('.chat-final-reply')?.textContent === 'A natural final ending…'`, 'durable Managed final reply missing');
  assert.match(await text(), /Final reply completeness is not recorded/);
  assert.doesNotMatch(await text(), /Final reply was truncated/);
  cases.push('Enter queues and Shift+Enter adds a newline', 'managed mid-turn ordinary delivery and final reply', 'provisional preview and natural ellipsis');

  fixture.caseStarted('completed_goal');
  const completedText = 'One completed-goal note';
  const completedNote = await queue(agents.completed, completedText);
  const completedLaunch = await fixture.waitNoteLaunch(agents.completed, completedNote);
  assert.equal(completedLaunch.task, completedText, 'completed-note task carries ordinary context');
  await fixture.assertGoalAchieved(agents.completed);
  await fixture.openNoteQuestion(agents.completed, 'Choose one option?');
  await until(`Boolean(document.querySelector('textarea[name="answer"]'))`, 'note-only question has no existing answer form');
  assert.equal(await cdp.evaluate(`(() => {
    const element = document.querySelector('textarea[name="answer"]');
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(element, 'Use the existing option');
    element.dispatchEvent(new Event('input', { bubbles: true })); return true;
  })()`), true);
  await click('.answer-send');
  await until(`document.querySelector('.answer-continuation')?.textContent ===
    'Answer saved for a future ordinary goal turn; note-only turns do not carry answers'`, 'note-only answer falsely implies delivery');
  await fixture.assertAnswerPending(agents.completed, 'Use the existing option');
  await fixture.finishNote(agents.completed, null);
  await until(`document.querySelector('.chat-thread')?.textContent.includes('No final reply was reported')`, 'absent final reply was fabricated');
  await fixture.assertGoalAchieved(agents.completed);
  cases.push('completed-goal note-only task and saved answer', 'absent final reply');

  fixture.caseStarted('attached');
  const attachedNote = await queue(agents.attached, 'Attached context note');
  const emission = await fixture.emitAttached(agents.attached);
  assert.equal(emission.noteLsn, attachedNote);
  assert.match(emission.context, /Attached context note/);
  await until(`Boolean(document.querySelector('[data-chat-context-lsn="${attachedNote}"]'))`, 'Attached context receipt missing');
  assert.match(await text(), /Sent as context/);
  assert.match(await text(), /Attached replies are unavailable/);
  assert.equal(await cdp.evaluate(`document.querySelectorAll('.chat-final-reply,.chat-preview').length`), 0, 'Attached reply was exposed');
  cases.push('Attached context delivery and reply limit');

  fixture.caseStarted('queued_cancel');
  const queuedNote = await queue(agents.queued, 'Cancel while queued');
  const queuedReceipt = await cancel(queuedNote);
  assert.match(queuedReceipt.text, /Cancelled before delivery/);
  await fixture.assertCancelled(agents.queued, queuedNote);
  cases.push('queued cancellation');

  fixture.caseStarted('held_cancel');
  await fixture.holdPreparation(agents.held);
  const heldNote = await queue(agents.held, 'Cancel held preparation');
  await until(`document.querySelector(${JSON.stringify(pending(heldNote))})?.dataset.chatPendingState === 'reserved'`, 'held preparation not reserved');
  await fixture.waitPreparation(agents.held);
  await cancel(heldNote);
  await fixture.releasePreparation(agents.held);
  await fixture.assertNeverDispatched(agents.held, heldNote);
  cases.push('reserved preparation cancellation');

  fixture.caseStarted('refusals');
  const refusals = await fixture.refusals();
  assert.ok(refusals.length > 0, 'Chat refusal fixture is empty');
  let openQuestion = false;
  for (const refusal of refusals) {
    await refusal.enter();
    if (refusal.delivery) {
      const noteLsn = await queue(refusal.agent, 'A blocked browser note');
      await until(`document.querySelector(${JSON.stringify(`${pending(noteLsn)} [data-chat-reason]`)})?.dataset.chatReason ===
        ${JSON.stringify(refusal.reason)}`, 'queued note did not show its delivery refusal');
      assert.match(await text(), refusal.expectedText);
      await refusal.assertDeliveryBlocked(noteLsn);
    } else {
      await select(refusal.agent);
      await fill('A refused browser note');
      await click('.chat-send');
      const result = await receipt('refused');
      assert.equal(result.reason, refusal.reason);
      assert.match(result.text, refusal.expectedText);
      await refusal.assertNoEffect();
      await dismiss();
    }
    openQuestion ||= refusal.reason === 'open_question';
    cases.push(`refusal ${refusal.reason}`);
  }
  assert.equal(openQuestion, true, 'open-question refusal was not exercised');

  fixture.caseStarted('stale_epoch');
  const staleNote = await queue(agents.queued, 'A cancelled note must stay cancelled');
  await fixture.holdNextSnapshot();
  await click(`.crew-row[data-agent-id="${agents.attached}"]`);
  await fixture.waitHeldSnapshot();
  await fixture.closeHeldSocket();
  await fixture.waitFreshSnapshotAfterHeld();
  await select(agents.queued);
  await cancel(staleNote);
  await fixture.assertCancelled(agents.queued, staleNote);
  await fixture.releaseHeldSnapshot();
  assert.equal(await cdp.evaluate(`Boolean(document.querySelector(${JSON.stringify(pending(staleNote))}))`), false,
    'old connection callback restored a cancelled Chat note');
  cases.push('old native snapshot callback cannot restore cancelled Chat');

  fixture.caseStarted('off_restart');
  const retainedText = 'Pending note survives Chat off';
  const retainedNote = await queue(agents.queued, retainedText);
  await fixture.assertPending(agents.queued, retainedText);
  const restartingOff = fixture.restart(false);
  await until(`!document.querySelector('#agent-detail-tab-chat,.chat-panel,.chat-cancel,.chat-preview,.chat-final-reply')`, 'Chat history or controls survived disconnect');
  await restartingOff;
  await until(`!document.querySelector('.stream-notice') && Boolean(document.querySelector('.crew-row'))`, 'disabled authenticated snapshot missing');
  assert.equal(await cdp.evaluate(`Boolean(document.querySelector('#agent-detail-tab-chat,.chat-panel,.chat-cancel,.chat-preview,.chat-final-reply'))`), false);
  await fixture.assertPending(agents.queued, retainedText);
  await fixture.restart(true);
  await select(agents.queued);
  await until(`document.querySelector(${JSON.stringify(pending(retainedNote))})?.textContent.includes(${JSON.stringify(retainedText)})`, 're-enabled Chat lost the durable pending note');
  cases.push('Chat off withdraws browser state and re-enable restores durable note');

  fixture.caseStarted('drafts');
  await fill('Unsent tab draft');
  await select(agents.attached);
  assert.equal(await cdp.evaluate(`document.querySelector('#chat-text')?.value`), '');
  await select(agents.queued);
  assert.equal(await cdp.evaluate(`document.querySelector('#chat-text')?.value`), 'Unsent tab draft');
  await fixture.reloadAuthenticated();
  await select(agents.queued);
  assert.equal(await cdp.evaluate(`document.querySelector('#chat-text')?.value`), '');
  cases.push('per-agent in-memory drafts and reload');
  return { cases };
}
