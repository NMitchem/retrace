#!/bin/bash
# M48 Task 10 (spec §3j, §6, §1 part 5): the structural audit as greps over the committed tree.
# Each check prints its evidence and a VERDICT line; a FINDING is reported, never smoothed.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
S=/private/tmp/claude-501
cd "$W" || exit 2
BASE=e6caa65
C=crates/retrace-core/src/lib.rs
X=crates/retrace-box/src/lib.rs
v() { if [ "$1" = "$2" ]; then echo "VERDICT $3: ok ($1)"; else echo "VERDICT $3: FINDING (got $1, want $2)"; fi; }
echo "audit of $(git rev-parse --short HEAD) on $(date '+%Y-%m-%d %H:%M:%S %Z')"

echo "== 1. No trace-format change (H3)"
v "$(git diff $BASE..HEAD -- crates/retrace-trace | wc -l | tr -d ' ')" 0 "crates/retrace-trace diff lines since $BASE"
v "$(grep -c -F 'pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x0b";' crates/retrace-trace/src/lib.rs)" 1 "TRACE_MAGIC is RT\\x00\\x0b"

echo "== 2. Symmetry rule 1: guest_kevent and guest_psynch once in record_box and once in ReplaySession::advance, the record call before the generic arm; note_fd_effects twice in record_box (Task 4 Ruling K6) and once in advance"
ra=$(grep -n 'pub fn advance(&mut self)' $C | head -1 | cut -d: -f1)
g=$(grep -n 'reached the generic forward arm' $C | head -1 | cut -d: -f1)
echo "ReplaySession::advance starts at line $ra; the generic arm's first assert is at line $g"
for f in guest_kevent guest_psynch; do
  grep -n -A2 "\.$f(" $C
  set -- $(grep -n "\.$f(" $C | cut -d: -f1)
  if [ "$#" = 2 ] && [ "$1" -lt "$ra" ] && [ "$2" -gt "$ra" ]; then echo "VERDICT $f sides: ok (record $1, replay $2)"; else echo "VERDICT $f sides: FINDING (lines $*)"; fi
done
# Task 4 Ruling K6: the record-side console-close arm calls note_fd_effects too, because replay
# finishes that landmark through the generic mirror. So: two record calls (the console-close arm,
# then the generic arm), both before `advance`, and one replay call (the generic mirror) inside it.
grep -n -A2 "\.note_fd_effects(" $C
set -- $(grep -n "\.note_fd_effects(" $C | cut -d: -f1)
if [ "$#" = 3 ] && [ "$1" -lt "$ra" ] && [ "$2" -lt "$ra" ] && [ "$3" -gt "$ra" ]; then echo "VERDICT note_fd_effects sides: ok (record $1 and $2, replay $3)"; else echo "VERDICT note_fd_effects sides: FINDING (lines $*)"; fi
for f in guest_kevent guest_psynch; do
  r=$(grep -n "\.$f(" $C | head -1 | cut -d: -f1)
  if [ "$r" -lt "$g" ]; then echo "VERDICT $f before the generic arm: ok ($r < $g)"; else echo "VERDICT $f before the generic arm: FINDING ($r >= $g)"; fi
done
echo "(compare the argument lists printed above by eye: the same Box_ method with the same arguments on both sides)"

echo "== 3. The generic forward arm asserts every new number"
grep -n 'reached the generic forward arm' $C
v "$(grep -c 'kevent (363) reached the generic forward arm' $C)" 1 "the kevent assert"
v "$(grep -c 'psynch syscall {num} reached the generic forward arm' $C)" 1 "the psynch assert"

echo "== 4. Symmetry rule 2: the SPRR register and the JIT view live below the trace"
grep -n -i 'sprr\|jit' $C
v "$(grep -i 'sprr' $C | grep -c -v -E '^\s*(//|\*)')" 0 "SPRR in retrace-core code (comments aside)"

echo "== 5. Every new Box_ field at Task 4's six sites (Ruling T10-a); kq, M46's field, is the patterns' control"
for f in kq gkq psynch jit; do
  d=$(grep -c -E "^\s*(pub )?$f: [A-Za-z_:<>]+," $X)
  l=$(grep -c -E "[ ,{]$f: [A-Za-z_:]*::default\(\)" $X)
  c=$(grep -c -E "^\s*$f: self\.$f\.clone\(\)," $X)
  r=$(grep -c -E "^\s*$f: state\.$f\.clone\(\)," $X)
  s=$(grep -c -E "[ \"]$f=\{:\?\}" $X)
  v "decl=$d lit=$l ckpt=$c from=$r dbg=$s" "decl=2 lit=3 ckpt=1 from=1 dbg=1" "$f: Box_ and BoxState, the three literals, checkpoint(), from_checkpoint, dbg_internal_state"
done

echo "== 6. verify_thread keeps seven call sites (§11a item 1)"
v "$(grep -c 'self\.verify_thread(' $C)" 7 "verify_thread call sites"

echo "== 7. Restore parity: each new state has a seek test, and checkpointparity has M48's"
for t in kq_e2e condvar_e2e jitwp_e2e; do
  grep -n -E 'fn [a-z_]*seek[a-z_]*\(' crates/retrace/tests/$t.rs
  n=$(grep -c -E 'fn [a-z_]*seek[a-z_]*\(' crates/retrace/tests/$t.rs)
  if [ "$n" -ge 1 ]; then echo "VERDICT $t seek tests: ok ($n)"; else echo "VERDICT $t seek tests: FINDING (none)"; fi
done
grep -n 'M48' crates/retrace-box/tests/checkpointparity.rs | head -5

echo "== 8. Review Focus: every pinned test is defined exactly once"
for n in an_event_list_aliasing_the_change_list_is_read_before_it_is_written \
         the_runtime_detection_probe_returns_natives_one_event \
         a_wake_of_the_current_thread_writes_the_vcpu \
         a_timeout_on_the_only_thread_answers_on_the_vcpu \
         a_timed_wait_on_the_only_waiter_answers_on_the_vcpu \
         the_sequence_window_wraps_as_synch_internal_h_computes \
         a_signal_across_the_sequence_wrap_wakes_the_waiter \
         the_stamped_extents_are_the_ranges_minus_their_noaccess_extents \
         an_unprotect_inside_a_jit_range_is_restamped_by_the_view_not_left_data \
         the_v8_shape_none_mapped_then_mprotected_rwx_runs_its_code \
         a_kevent_refused_on_replay_is_a_divergence_naming_it_not_a_panic \
         a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic; do
  v "$(git grep -c -E "fn $n\(" -- crates | awk -F: '{s+=$2} END {print s+0}')" 1 "$n"
done

echo "== 9. Every refusal prefix has a test file that names it"
for p in 'M48: kevent ' 'M48: pipe ' 'M48: psynch ' 'M48: SPRR ' 'M48: MAP_JIT ' 'M48: a signal is pending on thread '; do
  s=$(git grep -c -F "$p" -- 'crates/*/src/*.rs' | awk -F: '{s+=$2} END {print s+0}')
  t=$(git grep -l -F "$p" -- 'crates/*/tests/*.rs' | wc -l | tr -d ' ')
  echo "prefix [$p]: source lines $s, test files $t"
  if [ "$t" -ge 1 ]; then echo "VERDICT [$p] tested: ok"; else echo "VERDICT [$p] tested: FINDING (no test file names it; check the src unit tests by hand)"; fi
done

echo "== 10. Spec §1 part 5: every syscall number the final node walks reached is in CENSUS"
perl -0ne 'if (/CENSUS: &\[i64\] = &\[(.*?)\];/s) { print "$_\n" for ($1 =~ /-?\d+/g) }' crates/retrace-arch/tests/census.rs | sort -u > $S/m48-t10-census.txt
grep -a -h '^nums=' $E/walk-*.census | sed 's/^nums=//' | tr ' ' '\n' | grep -v '^$' | sort -u > $S/m48-t10-walknums.txt
echo "walk numbers: $(wc -l < $S/m48-t10-walknums.txt | tr -d ' '); CENSUS: $(wc -l < $S/m48-t10-census.txt | tr -d ' ')"
comm -23 $S/m48-t10-walknums.txt $S/m48-t10-census.txt | sed 's/^/MISSING /'
v "$(comm -23 $S/m48-t10-walknums.txt $S/m48-t10-census.txt | wc -l | tr -d ' ')" 0 "walk numbers missing from CENSUS"
echo "psynch numbers the walks reached (only 303, 304 and 305 are modelled):"
grep -a -h -E '^num (29[7-9]|30[0-9]|312): [1-9]' $E/walk-*.census | sort | uniq -c

echo "== 11. Nine #[ignore] lines, none new (H2)"
v "$(git grep -c -E '^\s*#\[ignore' -- crates | awk -F: '{s+=$2} END {print s}')" 9 "#[ignore] lines"

echo "== Known soft spots (Task 10 addendum B): stated with the file and line of each, not fixed here"
NC=crates/retrace/tests/node_crash_e2e.rs
NE=crates/retrace/tests/node_e2e.rs
U=crates/retrace/tests/util/mod.rs
# The file:line of the Nth (default first) line of $1 holding the literal $2, or a FINDING naming the
# anchor that was lost, so an edit that moves a cited line moves the citation and one that removes it fails.
at() { n=$(grep -n -F -- "$2" "$1" | sed -n "${3:-1}p" | cut -d: -f1); if [ -n "$n" ]; then echo "$1:$n"; else echo "$1:? (anchor [$2] #${3:-1} not found: FINDING)"; fi; }
k1=$(at $C 'self.b.guest_kevent(args)'); k2=$(at $X 'self.wake_kevent_waiters().map_err(fail)' 1)
k3=$(at $X 'self.wake_kevent_waiters().map_err(fail)' 2)
k4=$(at $X 'M48: kevent event list of thread'); k5=$(at $X 'which records no such waiter')
j1=$(at $NC '// 5. The store is JIT code.'); j2=$(at $NC 'assert_eq!(field(m, "opt="), native_opt')
d1=$(at $NC 'Event::SignalDelivery { sig: 11'); d2=$(at $NC 'let (di, si_addr, dthread) = delivery'); d3=$(at $NC 're-raises')
u1=$(at $NE 'util::assert_rung_records_and_replays_env('); u2=$(at $NE 'util::assert_rung_records_and_replays(&exe')
u3=$(at $NC 'util::record_dynamic_args(exe, &args)'); u4=$(at $NC 'let rp = util::replay(&trace);')
u5=$(at $U 'let out = Command::new(bin()).args(args).output().unwrap();'); u6=$(at $U 'let out = c.output().unwrap();')
u7=$(at $NC 'const DEBUG_SECS: u64'); u8=$(at $U 'pub fn debug_bounded(')
k6=$(at $X 'self.wake_due_threads();' 1); k7=$(at $X '=> self.kevent_timed_out(tid, kq),')
t1=$(at $NE 'assert_eq!(s.current_thread(), 0,'); t2=$(at $NE 'stopped short of main'); t3=$(at $NE 'assert!(write > j,')
t4=$(at $NE 'assert!(excess.is_multiple_of(STRIDE)'); t5=$(at $NE 'no landmark moved the clock by a second')
t6=$(at $NE 'with no timed kevent of a second or more')
r1=$(at $X 'debug_assert!(!self.threads.needs_reschedule(),' 1); r2=$(at $X 'M14: DEADLOCK — no runnable thread.')
echo "1. Two panics are reachable on replay after an EARLIER silent divergence (Task 4 review minor 3): one"
echo "   from the kevent mirror, one below the trace. From the mirror ($k1): Box_::guest_kevent's"
echo "   wake_kevent_waiters ($k2; the generic mirror's Box_::note_fd_effects reaches it too, $k3) goes"
echo "   through deliver_kevent to deliver_wake, which panics if a waiter's event list stopped translating"
echo "   ($k4). Not from the mirror: schedule_after_block's wake_due_threads ($k6) calls kevent_timed_out"
echo "   ($k7), which panics on a missing waiter ($k5). Review Focus 5's 'never a panic' holds for a shape"
echo "   the recording accepted and replay refuses, not for an earlier silent divergence: the full-memory"
echo "   comparison at exit is what would have caught that."
echo "2. Assertion 5 of node_crash_e2e proves JIT code, not TurboFan (Task 8 review minor 2). It ($j1)"
echo "   passes for any store whose pc lies in a MAP_JIT range, Sparkplug or Maglev code included;"
echo "   assertion 1 ($j2) compares the marker's opt= bits only with native's own, and nothing decodes them."
echo "3. The delivery assertion is node-version-specific (Task 8 review minor 3). The match ($d1) and its"
echo "   expect ($d2) rely on node 25.6.1's own SIGSEGV handler returning and the instruction re-faulting;"
echo "   the module doc's 're-raises' ($d3) is looser than measured, and no sigreturn or resume_pc is asserted."
echo "4. Record and replay are unbounded for every node gate (Task 8 review minor 5). node_e2e's rungs"
echo "   ($u1, $u2) and node_crash_e2e's record ($u3) and replays ($u4) end in util's run and run_env, an"
echo "   unbounded Command::output() ($u5, $u6). Only node_crash_e2e's debug session is bounded"
echo "   ($u7, through $u8). A hang in record or replay stalls the gate instead of failing it."
echo "5. node_timer_replays has three assertions no control reaches (Task 10 controls A1(c); fix round 1)."
echo "   current_thread() == 0 ($t1) is redundant with the exactness assertion, and the recorder's M15 R1"
echo "   debug_assert ($r1, debug builds only) fires first under a wrong-thread pick (A1(c), measured)."
echo "   'stopped short' ($t2) is shadowed by the recorder's M14 DEADLOCK panic ($r2): a jump short of the"
echo "   deadline wakes nobody. BY READING of schedule_after_block, not measured. write > j ($t3) is shadowed"
echo "   by the test's own earlier panics, 'no landmark moved the clock by a second' ($t5) and 'the clock"
echo "   jumped ... with no timed kevent' ($t6). BY READING, not measured. None is proven able to fail; they"
echo "   are defence in depth behind the exactness assertion ($t4, proven live by A1(a): the jump"
echo "   0x4801 = 2 x STRIDE + 1 ticks past fails there), M14 DEADLOCK, the test's earlier panics and M15 R1."
lost=$(printf '%s\n' "$k1" "$k2" "$k3" "$k4" "$k5" "$k6" "$k7" "$j1" "$j2" "$d1" "$d2" "$d3" "$u1" "$u2" "$u3" "$u4" "$u5" "$u6" "$u7" "$u8" \
  "$t1" "$t2" "$t3" "$t4" "$t5" "$t6" "$r1" "$r2" | grep -c 'FINDING')
v "$lost" 0 "Known soft spots: anchors not found (of 28)"
