#!/bin/sh
# Arm 1 of the gate — the loop closes with no human in it.
#
#   Two agents are hired, each with a role. One goal is set, carrying a command predicate. Nothing
#   else happens: no message is typed at anything, no question is answered, no operator intervenes.
#
# PASS when the goal reaches Done and the predicate result that caused it is one the daemon ran
# itself. FAIL when it reaches Done on the agent's own say-so, or never reaches Done at all.
#
# Run it from a clean checkout after `cargo build --release --workspace`.
set -u

. "$(dirname "$0")/lib.sh"

require_binaries
open_sandbox

printf 'Arm 1 — the loop closes with no human in it\n'
printf '  daemon    %s\n' "$DUDED"
printf '  cli       %s\n' "$DUDES_CLI"
printf '  sandbox   %s\n' "$SANDBOX"

# Which of the stand-in's runs does the goal's work. `2` is the arm: the predicate is observed
# false once and true once, so the arm can tell `Done` from a daemon that calls every goal done on
# sight. **Set it past the hop cap and this arm must go red** — that is the control for the whole
# script, and the README says so.
MARK_ON_RUN=${MARK_ON_RUN:-2}
start_daemon arm1

header "the daemon is up and answering the operator"
if dudes_ping; then
	pass "system.ping answered"
else
	fail "the daemon did not answer system.ping"
fi

hire_two_and_set_one_goal

# --- turn 1: the agent works and the predicate is observed to be false --------------------------

header "the goal starts a turn, and the turn is exactly one child"
if await_standin 1; then
	assert_one_child_is_the_agents 1
else
	fail "the goal never produced a turn"
	finish "arm 1"
fi
release_standin 1

# --- turn 2: the agent does the work, and the predicate is observed to be true -------------------

header "the false predicate did not end the goal: the loop takes another turn"
if await_standin 2; then
	assert_one_child_is_the_agents 2
else
	fail "the loop stopped after one turn instead of taking another"
	finish "arm 1"
fi
release_standin 2

header "the loop closes on its own"
if await "the goal to reach Done" 60 journal_shows '"kind":"done"'; then
	pass "the goal reached Done with nothing typed at anything"
else
	fail "the goal never reached Done"
fi

stop_daemon

# --- what the journal and the projection say, read offline with the shipped reader ----------------

dump="$GATE_OUT/arm1.journal"
store_export="$GATE_OUT/arm1.export.store"
dump_journal >"$dump" || fatal "could not read the journal back"
dump_export store >"$store_export" || fatal "could not export the projection store"
note "journal kept at $dump"
note "projection export kept at $store_export"

assert_done_is_observed "$dump" "$store_export"
assert_a_predicate_came_out_false "$dump"
assert_one_spawn_per_issued_turn "$dump"
assert_not_exhausted_by_hops "$dump"

finish "arm 1 — the loop closes with no human in it"
