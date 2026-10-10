#!/usr/bin/env python3
"""Owned deterministic provider protocol for local conversation verification."""
import json
import os
from pathlib import Path
import re
import sys
import time
import uuid
from datetime import datetime, timezone

work = Path(os.environ["DUDES_CHAT_FIXTURE_WORK"])
provider = os.environ.get("DUDES_CHAT_FIXTURE_PROVIDER", "claude")
if provider not in ("claude", "pi"):
    raise SystemExit("unknown fixture provider")
args = sys.argv[1:]
separator = args.index("--") if "--" in args else len(args)
options = args[:separator]
argument_tasks = args[separator + 1:] if separator < len(args) else []
if provider == "pi":
    def option_value(name):
        positions = [index for index, value in enumerate(options) if value == name]
        if len(positions) != 1 or positions[0] + 1 >= len(options):
            raise SystemExit(f"fixture requires one {name} value")
        return options[positions[0] + 1]

    if option_value("--mode") != "json":
        raise SystemExit("Pi fixture requires JSON mode")
    session_directory = Path(option_value("--session-dir"))
    if not session_directory.is_dir():
        raise SystemExit("Pi fixture session directory does not exist")
    if "--resume" in options or "--session-id" in options:
        raise SystemExit("Pi fixture resumes only with --session")
    if "--session" in options:
        flag = "--session"
        session = option_value(flag)
    else:
        flag = "assigned"
        session = str(uuid.uuid4())
    if not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?", session) or session.endswith(".jsonl"):
        raise SystemExit("invalid Pi fixture session")
else:
    session_directory = work
    session_flags = [(index, value) for index, value in enumerate(options) if value in ("--resume", "--session-id")]
    if len(session_flags) != 1 or session_flags[0][0] + 1 >= len(options):
        raise SystemExit("fixture requires one explicit session")
    position, flag = session_flags[0]
    session = options[position + 1]
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", session):
        raise SystemExit("invalid fixture session")
stdin_task = sys.stdin.read(65537)
if len(argument_tasks) > 1 or (argument_tasks and stdin_task):
    raise SystemExit("ambiguous fixture task delivery")
if provider == "pi" and stdin_task:
    raise SystemExit("Pi fixture requires one task argument")
task = argument_tasks[0] if argument_tasks else stdin_task
if not task or len(task.encode("utf-8")) > 65536:
    raise SystemExit("fixture requires a nonempty bounded task")
state_file = session_directory / f"session-{session}.json"
resuming = flag in ("--resume", "--session")
if resuming and not state_file.exists():
    raise SystemExit("resumed fixture session does not exist")
if not resuming and state_file.exists():
    raise SystemExit("fresh fixture session already exists")
state = json.loads(state_file.read_text()) if resuming else {"messages": [], "word": None}
run = 1
while True:
    try:
        launched = work.joinpath(f"launched.{run}.json").open("x", encoding="utf-8")
        break
    except FileExistsError:
        run += 1
with launched:
    metadata = {"session": session, "flag": flag, "task": task}
    if provider == "pi":
        metadata.update({"provider": provider, "session_file": str(state_file)})
    json.dump(metadata, launched)
previous = len(state["messages"])
match = re.search(r"remember(?: the word)?\s+[\"']?([\w-]+)", task, re.IGNORECASE)
if match:
    state["word"] = match.group(1)
state["messages"].append(task)
state_file.write_text(json.dumps(state), encoding="utf-8")
if previous == 0:
    opening = f"I'll remember **{state['word']}** for our conversation." if state["word"] else "I received your first message. No remembered word was supplied."
    reply = (
        opening + "\n\nThis reply arrives as individual provider text deltas before completion. "
        "You can keep writing your next message while this one is still running. "
        "The daemon owns this single turn and keeps our confirmed session for your follow-up."
    )
else:
    opening = f"The word was **{state['word']}**." if state["word"] else "No remembered word is present in this session."
    noun = "message" if previous == 1 else "messages"
    reply = (
        f"{opening} I resumed the same session and can see {previous} earlier {noun}.\n\n"
        "Your follow-up is another finite turn. It will return to idle after this reply finishes."
    )

def emit(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)

def event(value):
    emit({"type": "stream_event", "session_id": session, "parent_tool_use_id": None, "event": value})

def wait_for(marker):
    deadline = time.monotonic() + 1200
    while not (work / marker).exists():
        if time.monotonic() >= deadline:
            raise SystemExit("fixture progress was not released")
        time.sleep(0.05)

zero_usage = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0, "cost": {"total": 0}}
final_usage = {"input": 10, "output": 20, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 30, "cost": {"total": 0.01}}
if provider == "pi":
    emit({"type": "session", "version": 3, "id": session, "cwd": os.getcwd(), "timestamp": datetime.now(timezone.utc).isoformat()})
    emit({"type": "agent_start"})
    emit({"type": "message_start", "message": {"role": "assistant", "content": [], "model": "fixture", "usage": zero_usage, "stopReason": "pending"}})
    emit({"type": "message_update", "usage": zero_usage, "assistantMessageEvent": {"type": "text_start", "contentIndex": 0}})
else:
    emit({"type": "system", "subtype": "init", "session_id": session, "tools": []})
    message = f"fixture-message-{run}"
    event({"type": "message_start", "message": {"id": message, "role": "assistant", "content": []}})
    event({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
for index, character in enumerate(reply):
    if provider == "pi":
        emit({"type": "message_update", "usage": zero_usage, "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": character}})
    else:
        event({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": character}})
    if index == 0:
        (work / f"ready.{run}").touch()
    if index == 31 and os.environ.get("DUDES_CHAT_FIXTURE_HOLD_PREFIX") == "1":
        (work / f"prefix_ready.{run}").touch()
        wait_for(f"stream.{run}")
    time.sleep(0.005)
(work / f"streamed.{run}").touch()
wait_for(f"release.{run}")
if provider == "pi":
    emit({"type": "message_update", "usage": final_usage, "assistantMessageEvent": {"type": "text_end", "contentIndex": 0, "content": reply}})
    emit({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": reply}], "model": "fixture", "usage": final_usage, "stopReason": "stop"}})
    emit({"type": "agent_end", "willRetry": False})
    emit({"type": "agent_settled"})
else:
    emit({"type": "assistant", "session_id": session, "message": {"id": message, "role": "assistant", "content": [{"type": "text", "text": reply}]}})
    event({"type": "content_block_stop", "index": 0})
    event({"type": "message_stop"})
    emit({"type": "result", "subtype": "success", "is_error": False, "session_id": session, "result": reply, "total_cost_usd": 0.01 * run, "modelUsage": {"fixture": {"inputTokens": 10 * run, "outputTokens": 20 * run, "costUSD": 0.01 * run}}})
