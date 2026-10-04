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
