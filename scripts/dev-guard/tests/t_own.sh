#!/bin/bash
# Tests for the ownership/lock protocol in scripts/dev-guard/lib.sh:
# dg_own_acquire DIR VARNAME, dg_own_release DIR TOKEN, dg_own_status DIR
# (design R5' / R5''). Sourced by test-guard.sh after helpers.sh and t_lib.sh.
#
# Safety: each worker is a separate bash script with a bounded hold (<= 20 s),
# at most 2 workers are alive at once (plus transient 0.2 s sleeps), and the
# EXIT trap kills ONLY pids recorded in DG_OWN_PIDS. No pkill/killall.
# Every test starts with _own_require, so a missing function fails fast.

DG_OWN_PIDS=""
_OWN_BG_PID=""
_OWN_DEAD_PID=""

# Body run by a worker: acquire, publish "rc token", hold while owner.
_OWN_ACQ='TOK=""; dg_own_acquire "$D" TOK >"$RES.msg" 2>&1; rc=$?; _res "$rc $TOK"; if [ "$rc" -eq 0 ]; then _hold; fi'
_OWN_ACQ_GATED="_gate; $_OWN_ACQ"

# Only the test-recorded pids are killed here.
_own_cleanup() {
  local p
  for p in $DG_OWN_PIDS; do
    if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then
      kill -KILL "$p" 2>/dev/null
    fi
  done
}
dg_on_exit '_own_cleanup'

# _own_require NAME : fails the test fast if lib.sh or any function is missing.
_own_require() {
  require_lib "$1" || return 1
  local f missing=""
  for f in dg_own_acquire dg_own_release dg_own_status; do
    if ! type "$f" >/dev/null 2>&1; then
      missing="$missing $f"
    fi
  done
  if [ -n "$missing" ]; then
    _report fail "$1" "lib.sh lacks:$missing"
    return 1
  fi
  return 0
}

# _own_setup : fresh private DG_HOME with locks/, short DG_LOCK_WAIT.
_own_setup() {
  new_dg_home || return 1
  mkdir -p "$DG_HOME/locks" || return 1
  export DG_LOCK_WAIT=2
  unset DG_PAUSE_after_mkdir DG_PAUSE_before_release DG_PAUSE_reclaim_judged DG_FAKE_PS_FAIL
}

_own_dir() { echo "$DG_HOME/locks/$1.lock"; }

# _own_lstart PID : the lstart the protocol records for PID.
_own_lstart() {
  LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null | sed -e 's/^ *//' -e 's/ *$//'
}

_own_owner_pid() { cut -d' ' -f1 "$1/owner" 2>/dev/null; }
_own_owner_tok() { cut -d' ' -f2 "$1/owner" 2>/dev/null; }

_own_res_rc() { cut -d' ' -f1 "$DG_HOME/$1.res" 2>/dev/null; }
_own_res_tok() { cut -d' ' -f2- "$DG_HOME/$1.res" 2>/dev/null; }

# _own_await_file PATH SECONDS : polls (0.2 s steps) until PATH exists.
_own_await_file() {
  local i=0 max=$(( $2 * 5 ))
  while [ ! -e "$1" ] && [ "$i" -lt "$max" ]; do
    sleep 0.2
    i=$((i + 1))
  done
  [ -e "$1" ]
}

# _own_stop NAME : tells worker NAME to leave its hold loop.
_own_stop() { : >"$DG_HOME/$1.hold"; }

# _own_reap PID : waits up to ~6 s for PID to exit, then KILLs it (recorded pid only).
_own_reap() {
  local i=0
  [ -n "$1" ] || return 0
  while kill -0 "$1" 2>/dev/null && [ "$i" -lt 30 ]; do
    sleep 0.2
    i=$((i + 1))
  done
  kill -KILL "$1" 2>/dev/null
  return 0
}

# _own_bg NAME DIR PRE BODY : starts worker script NAME (own bash process).
# Sets _OWN_BG_PID and records it. Worker header sources lib.sh and defines
# D (dir), RES (result file), HOLD (release file), GO (start barrier file).
_own_bg() {
  local name="$1" script="$DG_HOME/w-$1.sh"
  {
    printf '#!/bin/bash\n'
    printf '. %q 2>/dev/null\n' "$DG_LIB_PATH"
    printf 'D=%q\n' "$2"
    printf 'RES=%q\n' "$DG_HOME/$name.res"
    printf 'HOLD=%q\n' "$DG_HOME/$name.hold"
    printf 'GO=%q\n' "$DG_HOME/go"
    cat <<'EOF'
_res() { printf '%s\n' "$*" >"$RES.tmp" && mv "$RES.tmp" "$RES"; }
_hold() { local i=0; while [ ! -e "$HOLD" ] && [ "$i" -lt 100 ]; do sleep 0.2; i=$((i + 1)); done; }
_gate() { local i=0; while [ ! -e "$GO" ] && [ "$i" -lt 300 ]; do sleep 0.05; i=$((i + 1)); done; }
EOF
    printf '%s\n' "$3"
    printf '%s\n' "$4"
  } >"$script"
  /bin/bash "$script" >"$DG_HOME/w-$name.log" 2>&1 &
  _OWN_BG_PID=$!
  DG_OWN_PIDS="$DG_OWN_PIDS $_OWN_BG_PID"
}

# _own_craft_dead_pid : sets _OWN_DEAD_PID to a pid that has exited.
_own_craft_dead_pid() {
  sleep 1 &
  _OWN_DEAD_PID=$!
  DG_OWN_PIDS="$DG_OWN_PIDS $_OWN_DEAD_PID"
  wait "$_OWN_DEAD_PID" 2>/dev/null
}

# _own_craft_stale DIR OLDTOK : dir with an owner record naming the dead pid.
_own_craft_stale() {
  mkdir -p "$1" && printf '%s %s %s\n' "$_OWN_DEAD_PID" "$2" "Mon Jan  1 00:00:00 2024" >"$1/owner"
}

# _own_count_zero RC_A RC_B : how many of the two results are 0.
_own_count_zero() {
  local n=0
  [ "$1" = 0 ] && n=$((n + 1))
  [ "$2" = 0 ] && n=$((n + 1))
  echo "$n"
}

# _own_race_check TAG DIR NAME_A PID_A NAME_B PID_B : one winner, one loser, single owner.
_own_race_check() {
  local tag="$1" d="$2" na="$3" pa="$4" nb="$5" pb="$6" ra rb w wp l
  _own_await_file "$DG_HOME/$na.res" 15
  _own_await_file "$DG_HOME/$nb.res" 15
  ra=$(_own_res_rc "$na")
  rb=$(_own_res_rc "$nb")
  assert_eq "$tag: exactly one acquirer wins" "1" "$(_own_count_zero "$ra" "$rb")"
  if [ "$ra" = 0 ]; then
    w=$na; wp=$pa; l=$nb
  else
    w=$nb; wp=$pb; l=$na
  fi
  assert_eq "$tag: loser returns 1" "1" "$(_own_res_rc "$l")"
  assert_eq "$tag: owner token is the winner token" "$(_own_res_tok "$w")" "$(_own_owner_tok "$d")"
  assert_eq "$tag: owner pid is the winner pid" "$wp" "$(_own_owner_pid "$d")"
  _own_stop "$w"
}

# ---------------------------------------------------------- basic protocol

test_own_basic_cycle() {
  _own_require "own: acquire/release basic cycle" || return 0
  _own_setup
  local D TOK line rest rc out
  D=$(_own_dir basic)
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  assert_eq "own: acquire on free dir returns 0" "0" "$rc"
  assert_zero "own: token assigned to VARNAME" test -n "$TOK"
  assert_file_exists "own: owner record written" "$D/owner"
  line=$(cat "$D/owner" 2>/dev/null)
  rest=${line#* }
  assert_eq "own: owner record pid is caller pid" "$$" "${line%% *}"
  assert_eq "own: owner record token matches VARNAME" "$TOK" "${rest%% *}"
  assert_eq "own: owner record lstart matches ps" "$(_own_lstart $$)" "${rest#* }"
  out=$(dg_own_status "$D")
  assert_eq "own: status shows live owner" "owner $$ $TOK" "$out"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
  rc=$?
  assert_eq "own: release with matching token returns 0" "0" "$rc"
  assert_zero "own: dir removed after release" test ! -e "$D"
  out=$(dg_own_status "$D")
  assert_eq "own: status none after release" "none" "$out"
}

test_own_release_wrong_token() {
  _own_require "own: release with wrong token leaves dir" || return 0
  _own_setup
  local D TOK before rc
  D=$(_own_dir wrongtok)
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  before=$(cat "$D/owner" 2>/dev/null)
  dg_own_release "$D" "not-$TOK" >/dev/null 2>&1
  rc=$?
  assert_eq "own: release with wrong token returns 1" "1" "$rc"
  assert_zero "own: dir kept after wrong-token release" test -d "$D"
  assert_eq "own: owner record unchanged after wrong-token release" "$before" "$(cat "$D/owner" 2>/dev/null)"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
  assert_eq "own: correct release still works" "0" "$?"
}

test_own_second_acquire_while_owned() {
  _own_require "own: second acquire while owned returns 1" || return 0
  _own_setup
  local D TOK1 TOK2 before rc
  D=$(_own_dir owned)
  TOK1=""
  dg_own_acquire "$D" TOK1 >/dev/null 2>&1
  before=$(cat "$D/owner" 2>/dev/null)
  TOK2=""
  dg_own_acquire "$D" TOK2 >/dev/null 2>&1
  rc=$?
  assert_eq "own: second acquire returns 1" "1" "$rc"
  assert_eq "own: second acquire does not modify owner record" "$before" "$(cat "$D/owner" 2>/dev/null)"
  dg_own_release "$D" "$TOK1" >/dev/null 2>&1
  assert_eq "own: owner releases its own lock" "0" "$?"
}

test_own_simultaneous_acquirers() {
  _own_require "own: two simultaneous acquirers, one winner" || return 0
  _own_setup
  local D pa pb
  D=$(_own_dir race)
  _own_bg a "$D" "" "$_OWN_ACQ_GATED"; pa=$_OWN_BG_PID
  _own_bg b "$D" "" "$_OWN_ACQ_GATED"; pb=$_OWN_BG_PID
  : >"$DG_HOME/go"
  _own_race_check "own: simultaneous" "$D" a "$pa" b "$pb"
  _own_reap "$pa"
  _own_reap "$pb"
}

# ------------------------------------------------------ initialization

test_own_initializer_paused() {
  _own_require "own: paused initializer: second waits then 75" || return 0
  _own_setup
  local D P pi pw msg itok
  D=$(_own_dir init)
  P="$DG_HOME/pause.init"
  : >"$P"
  _own_bg init "$D" "export DG_PAUSE_after_mkdir=$(printf '%q' "$P")" "$_OWN_ACQ"
  pi=$_OWN_BG_PID
  _own_await_file "$D" 10
  assert_zero "own: paused initializer created the dir" test -d "$D"
  assert_zero "own: paused initializer has no owner record yet" test ! -e "$D/owner"
  _own_bg second "$D" "" "$_OWN_ACQ"
  pw=$_OWN_BG_PID
  _own_await_file "$DG_HOME/second.res" 15
  assert_eq "own: second acquirer returns 75 while initializing" "75" "$(_own_res_rc second)"
  msg=$(cat "$DG_HOME/second.res.msg" 2>/dev/null)
  assert_contains "own: 75 message names the lock" "$msg" "lock init has no owner record"
  assert_contains "own: 75 message gives manual recovery" "$msg" "manually"
  assert_zero "own: initializing dir not reclaimed" test -d "$D"
  assert_zero "own: second acquirer wrote no owner record" test ! -e "$D/owner"
  assert_eq "own: only the initializing dir exists" "init.lock" "$(ls -A "$DG_HOME/locks" | tr '\n' ' ' | sed -e 's/ *$//')"
  rm -f "$P"
  _own_await_file "$DG_HOME/init.res" 10
  assert_eq "own: resumed initializer returns 0" "0" "$(_own_res_rc init)"
  itok=$(_own_res_tok init)
  assert_eq "own: resumed initializer completes owner record (pid)" "$pi" "$(_own_owner_pid "$D")"
  assert_eq "own: resumed initializer completes owner record (token)" "$itok" "$(_own_owner_tok "$D")"
  _own_stop init
  _own_stop second
  _own_reap "$pi"
  _own_reap "$pw"
}

# ------------------------------------------------------- stale reclaim

test_own_dead_owner_reclaimed() {
  _own_require "own: dead owner pid is reclaimed" || return 0
  _own_setup
  local D TOK rc
  D=$(_own_dir dead)
  _own_craft_dead_pid
  _own_craft_stale "$D" OLDTOK
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  assert_eq "own: stale dead-owner dir is reclaimed (rc 0)" "0" "$rc"
  assert_zero "own: reclaimed token differs from old token" test -n "$TOK" -a "$TOK" != OLDTOK
  assert_eq "own: reclaimed owner pid is caller" "$$" "$(_own_owner_pid "$D")"
  assert_eq "own: reclaimed owner token is new" "$TOK" "$(_own_owner_tok "$D")"
  assert_eq "own: no dead sibling left after reclaim" "" "$(ls -A "$DG_HOME/locks" | grep '\.dead\.')"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
}

test_own_reclaim_race_single_owner() {
  _own_require "own: racing reclaimers, single owner" || return 0
  _own_setup
  local i D pa pb
  for i in 1 2 3; do
    D=$(_own_dir "rr$i")
    _own_craft_dead_pid
    _own_craft_stale "$D" "OLD$i"
    rm -f "$DG_HOME/go"
    _own_bg "a$i" "$D" "" "$_OWN_ACQ_GATED"; pa=$_OWN_BG_PID
    _own_bg "b$i" "$D" "" "$_OWN_ACQ_GATED"; pb=$_OWN_BG_PID
    : >"$DG_HOME/go"
    _own_race_check "own: reclaim race round $i" "$D" "a$i" "$pa" "b$i" "$pb"
    _own_reap "$pa"
    _own_reap "$pb"
  done
}

test_own_lstart_mismatch_reclaimed() {
  _own_require "own: live pid with wrong lstart is reclaimed" || return 0
  _own_setup
  local D TOK rc
  D=$(_own_dir lsm)
  mkdir "$D"
  printf '%s %s %s\n' "$$" OLDTOK "Mon Jan  1 00:00:00 1990" >"$D/owner"
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  assert_eq "own: lstart mismatch is stale, acquire returns 0" "0" "$rc"
  assert_zero "own: lstart mismatch token replaced" test -n "$TOK" -a "$TOK" != OLDTOK
  assert_eq "own: lstart mismatch owner token is new" "$TOK" "$(_own_owner_tok "$D")"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
}

test_own_identity_unknown_not_stale() {
  _own_require "own: identity read failure is not stale" || return 0
  _own_setup
  local D TOK rc before
  D=$(_own_dir ident)
  mkdir "$D"
  printf '%s %s %s\n' "$$" OLDTOK "Mon Jan  1 00:00:00 1990" >"$D/owner"
  before=$(cat "$D/owner")
  export DG_FAKE_PS_FAIL=1
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  unset DG_FAKE_PS_FAIL
  assert_eq "own: unknown identity => acquire returns 1" "1" "$rc"
  assert_eq "own: unknown identity => owner record unchanged" "$before" "$(cat "$D/owner" 2>/dev/null)"
  assert_zero "own: unknown identity => dir kept" test -d "$D"
  assert_zero "own: unknown identity => no reclaim lock" test ! -e "$DG_HOME/locks/.reclaim-ident"
  assert_zero "own: unknown identity => no dead sibling" test -z "$(ls -A "$DG_HOME/locks" | grep '\.dead\.')"
}

test_own_paused_reclaimer_not_displaced() {
  _own_require "own: paused reclaimer is not displaced" || return 0
  _own_setup
  local D P pa before tokb rcb
  D=$(_own_dir pr)
  P="$DG_HOME/pause.judged"
  _own_craft_dead_pid
  _own_craft_stale "$D" OLDTOK
  before=$(cat "$D/owner")
  : >"$P"
  _own_bg ra "$D" "export DG_PAUSE_reclaim_judged=$(printf '%q' "$P")" "$_OWN_ACQ"
  pa=$_OWN_BG_PID
  _own_await_file "$DG_HOME/locks/.reclaim-pr" 10
  sleep 3.5
  assert_zero "own: paused reclaimer still holds reclaim lock" test -d "$DG_HOME/locks/.reclaim-pr"
  tokb=""
  dg_own_acquire "$D" tokb >/dev/null 2>&1
  rcb=$?
  assert_zero "own: competing reclaimer returns non-zero" test "$rcb" -ne 0
  assert_eq "own: competing reclaimer leaves stale record intact" "$before" "$(cat "$D/owner" 2>/dev/null)"
  assert_zero "own: competing reclaimer kept the reclaim lock" test -d "$DG_HOME/locks/.reclaim-pr"
  rm -f "$P"
  _own_await_file "$DG_HOME/ra.res" 10
  assert_eq "own: resumed reclaimer acquires (rc 0)" "0" "$(_own_res_rc ra)"
  assert_eq "own: resumed reclaimer owner pid" "$pa" "$(_own_owner_pid "$D")"
  assert_eq "own: resumed reclaimer owner token" "$(_own_res_tok ra)" "$(_own_owner_tok "$D")"
  _own_stop ra
  _own_reap "$pa"
}

test_own_paused_releaser_not_renamed() {
  _own_require "own: delayed release does not rename replacement" || return 0
  _own_setup
  local D P ph pr tokh tokn rcn q
  D=$(_own_dir pl)
  P="$DG_HOME/pause.release"
  _own_bg hold "$D" "" "$_OWN_ACQ"
  ph=$_OWN_BG_PID
  _own_await_file "$DG_HOME/hold.res" 10
  tokh=$(_own_res_tok hold)
  assert_eq "own: holder acquired (rc 0)" "0" "$(_own_res_rc hold)"
  q=$(printf '%q' "$tokh")
  : >"$P"
  _own_bg rel "$D" "export DG_PAUSE_before_release=$(printf '%q' "$P")" \
    'dg_own_release "$D" '"$q"' >"$RES.msg" 2>&1; rc=$?; _res "$rc"'
  pr=$_OWN_BG_PID
  sleep 1.5
  assert_zero "own: releaser is frozen before its rename" kill -0 "$pr"
  assert_zero "own: releaser has not returned yet" test ! -e "$DG_HOME/rel.res"
  kill -TERM "$ph" 2>/dev/null
  _own_reap "$ph"
  tokn=""
  dg_own_acquire "$D" tokn >/dev/null 2>&1
  rcn=$?
  assert_eq "own: replacement acquired after holder death" "0" "$rcn"
  rm -f "$P"
  _own_await_file "$DG_HOME/rel.res" 10
  assert_eq "own: delayed release returns 1" "1" "$(_own_res_rc rel)"
  assert_zero "own: replacement dir intact" test -d "$D"
  assert_eq "own: replacement owner pid intact" "$$" "$(_own_owner_pid "$D")"
  assert_eq "own: replacement owner token intact" "$tokn" "$(_own_owner_tok "$D")"
  dg_own_release "$D" "$tokn" >/dev/null 2>&1
  _own_reap "$pr"
}

test_own_leftover_siblings_cleaned() {
  _own_require "own: leftover siblings cleaned on next acquire" || return 0
  _own_setup
  local D TOK rc
  D=$(_own_dir sib)
  mkdir -p "$DG_HOME/locks/sib.dead.4242.tok1" "$DG_HOME/locks/sib.rel.4343" \
    "$DG_HOME/locks/sib.tmp.4444" "$DG_HOME/locks/other.dead.1.x"
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  assert_eq "own: acquire with leftovers returns 0" "0" "$rc"
  assert_zero "own: leftover dead sibling removed" test ! -e "$DG_HOME/locks/sib.dead.4242.tok1"
  assert_zero "own: leftover rel sibling removed" test ! -e "$DG_HOME/locks/sib.rel.4343"
  assert_zero "own: leftover tmp sibling removed" test ! -e "$DG_HOME/locks/sib.tmp.4444"
  assert_zero "own: other lock's sibling untouched" test -d "$DG_HOME/locks/other.dead.1.x"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
}

test_own_spaces_in_home() {
  _own_require "own: spaces in DG_HOME and lock name" || return 0
  _own_setup
  local D TOK rc out
  DG_HOME="$DG_HOME/home with spaces"
  export DG_HOME
  mkdir -p "$DG_HOME/locks"
  D=$(_own_dir "with space")
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  rc=$?
  assert_eq "own: acquire under spaced path returns 0" "0" "$rc"
  assert_eq "own: spaced path owner pid" "$$" "$(_own_owner_pid "$D")"
  out=$(dg_own_status "$D")
  assert_eq "own: spaced path status owner" "owner $$ $TOK" "$out"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1
  assert_eq "own: spaced path release returns 0" "0" "$?"
  assert_zero "own: spaced path dir removed" test ! -e "$D"
}

test_own_status_outputs() {
  _own_require "own: status outputs owner/stale/initializing/none" || return 0
  _own_setup
  local D TOK out
  D=$(_own_dir st-none)
  out=$(dg_own_status "$D")
  assert_eq "own: status none" "none" "$out"
  assert_zero "own: status none returns 0" dg_own_status "$D"

  D=$(_own_dir st-owner)
  TOK=""
  dg_own_acquire "$D" TOK >/dev/null 2>&1
  out=$(dg_own_status "$D")
  assert_eq "own: status owner" "owner $$ $TOK" "$out"
  dg_own_release "$D" "$TOK" >/dev/null 2>&1

  D=$(_own_dir st-stale)
  _own_craft_dead_pid
  _own_craft_stale "$D" OLDTOK
  out=$(dg_own_status "$D")
  assert_eq "own: status stale" "stale" "$out"
  assert_zero "own: status stale does not reclaim" test -d "$D"

  D=$(_own_dir st-init)
  mkdir "$D"
  out=$(dg_own_status "$D")
  assert_eq "own: status initializing" "initializing" "$out"
  assert_zero "own: status initializing returns 0" dg_own_status "$D"
}

# ------------------------------------------------------------ run them

test_own_basic_cycle
test_own_release_wrong_token
test_own_second_acquire_while_owned
test_own_simultaneous_acquirers
test_own_initializer_paused
test_own_dead_owner_reclaimed
test_own_reclaim_race_single_owner
test_own_lstart_mismatch_reclaimed
test_own_identity_unknown_not_stale
test_own_paused_reclaimer_not_displaced
test_own_paused_releaser_not_renamed
test_own_leftover_siblings_cleaned
test_own_spaces_in_home
test_own_status_outputs
