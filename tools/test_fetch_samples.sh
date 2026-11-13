#!/usr/bin/env bash
#
# Self-test for tools/fetch_samples.sh. Runs no network: every case either
# targets a model that does not exist (so the driver loop resolves the run and
# then does nothing) or stubs curl and records the URLs the script would have
# fetched.
#
# What it is guarding is the date handling. The script has to work on GNU
# coreutils and on BSD/macOS date, and the failure mode when it does not is
# quiet: an unusable DATE leaves a hole in every URL, and all fourteen models
# fail with an unrelated-looking 404 or "no .idx". The BSD case is exercised
# here by putting a stub on PATH, so it is checked on a GNU host too.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$REPO/tools/fetch_samples.sh"
REAL_DATE="$(command -v date)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fails=0
check() { # $1 what, $2 expected, $3 actual
  if [ "$2" = "$3" ]; then printf ' ok  %s\n' "$1"
  else printf 'FAIL %s\n       expected: %s\n         actual: %s\n' "$1" "$2" "$3"; fails=$((fails + 1)); fi
}
contains() { # $1 what, $2 needle, $3 haystack
  case "$3" in
    *"$2"*) printf ' ok  %s\n' "$1" ;;
    *) printf 'FAIL %s\n       expected to contain: %s\n              in: %s\n' "$1" "$2" "$3"; fails=$((fails + 1)) ;;
  esac
}

# This file has to run on both date flavors too — a portability test that only
# runs on GNU would be no test at all for the host the bug was reported from.
# Same detection the script under test uses, kept separate so a mistake there
# cannot cancel out a matching mistake here.
if date --version >/dev/null 2>&1; then HOST_DATE=gnu; else HOST_DATE=bsd; fi
host_yesterday() {
  if [ "$HOST_DATE" = gnu ]; then "$REAL_DATE" -u -d yesterday +%Y%m%d
  else "$REAL_DATE" -u -v-1d +%Y%m%d; fi
}
host_doy() {
  if [ "$HOST_DATE" = gnu ]; then "$REAL_DATE" -u -d "$1" +%j
  else "$REAL_DATE" -u -j -f %Y%m%d "$1" +%j; fi
}

# A stand-in for BSD/macOS date(1): rejects --version and GNU's -d, and answers
# the -v / -j -f spellings that fetch_samples.sh must fall back to. On a BSD
# host those spellings are simply passed through to the real date; on a GNU one
# they are translated, since the point is to exercise the branch, not the tool.
mkdir -p "$TMP/bsd"
cat > "$TMP/bsd/date" <<EOF
#!/usr/bin/env bash
real="$REAL_DATE"
host="$HOST_DATE"
case " \$* " in
  *" --version "*) echo "date: illegal option -- -" >&2; exit 1 ;;
  *" -d "*)        echo "date: illegal time format" >&2; exit 1 ;;
esac
if [ "\$host" = bsd ]; then exec "\$real" "\$@"; fi
case "\$*" in
  "-u -v-1d +%Y%m%d")        exec "\$real" -u -d yesterday +%Y%m%d ;;
  "-u -j -f %Y%m%d "*" +%j") exec "\$real" -u -d "\$5" +%j ;;
  *) exec "\$real" "\$@" ;;
esac
EOF
chmod +x "$TMP/bsd/date"

# Records the URL of every request instead of making one. Failing keeps the
# script on its warn-and-continue path, so nothing downstream runs.
mkdir -p "$TMP/nocurl"
cat > "$TMP/nocurl/curl" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do case "\$a" in http*) echo "\$a" >> "$TMP/urls.txt" ;; esac; done
exit 1
EOF
chmod +x "$TMP/nocurl/curl"

banner() { # runs the script with no real model and returns its run banner
  "$@" bash "$SCRIPT" __no_such_model__ 2>/dev/null | sed -n '1p' | sed 's/\x1b\[[0-9;]*m//g'
}

yesterday="$(host_yesterday)"
yday_doy="$(host_doy "$yesterday")"

check "the host's own date resolves yesterday's 00Z run" \
  "==> run: ${yesterday} 00Z (day ${yday_doy})  ->  $REPO/samples" \
  "$(banner env)"

# The regression this file exists for: on BSD date the run used to come out
# blank ("run:  00Z"), and every model then 404'd.
check "BSD date resolves the same run" \
  "==> run: ${yesterday} 00Z (day ${yday_doy})  ->  $REPO/samples" \
  "$(banner env PATH="$TMP/bsd:$PATH")"

check "an explicit DATE is honored on BSD date" \
  "==> run: 20260629 00Z (day 180)  ->  $REPO/samples" \
  "$(banner env PATH="$TMP/bsd:$PATH" DATE=20260629)"

check "a one-digit CYCLE is padded, not passed through" \
  "==> run: 20260629 06Z (day 180)  ->  $REPO/samples" \
  "$(banner env DATE=20260629 CYCLE=6)"

# Bad input stops the run rather than fetching fourteen malformed URLs.
# (DATE= empty is not in this list: `${DATE:-…}` reads it as unset, so it
# legitimately falls back to yesterday rather than being rejected.)
for bad in "DATE=2026-06-29" "DATE=20260631x" "CYCLE=24" "CYCLE=noon"; do
  out="$(env "$bad" bash "$SCRIPT" __no_such_model__ 2>&1)"; rc=$?
  check "$bad exits 2" "2" "$rc"
  contains "$bad explains itself" "error" "$out"
done

# The URLs themselves, for the models whose paths are date- or cycle-derived.
: > "$TMP/urls.txt"
env PATH="$TMP/nocurl:$PATH" DATE=20260629 CYCLE=6 bash "$SCRIPT" gfs hrrr goes >/dev/null 2>&1
urls="$(cat "$TMP/urls.txt")"
contains "GFS URL carries the run"   "gfs.20260629/06/atmos&file=gfs.t06z" "$urls"
contains "HRRR URL carries the run"  "hrrr.20260629/conus/hrrr.t06z"      "$urls"
contains "GOES prefix carries year/day-of-year/hour" "ABI-L2-CMIPM/2026/180/06/" "$urls"

# An index larger than a pipe buffer, with the wanted record near the top. A
# reader that stops at the first match used to kill the download with SIGPIPE,
# and pipefail then reported the record missing (#1031). The stub answers the
# index and records the byte range it is asked for, then fails that request so
# nothing is written to samples/.
mkdir -p "$TMP/bigindex"
cat > "$TMP/bigindex/curl" <<EOF
#!/usr/bin/env bash
case "\$*" in
  *.index*|*.idx*)
    echo '{"param": "z", "_offset": 0, "_length": 10}'
    echo '{"param": "2t", "_offset": 1213794, "_length": 500}'
    echo '1:0:d=2026062900:TMP:surface:anl:'
    echo '2:900:d=2026062900:TMP:2 m above ground:anl:'
    for _ in \$(seq 1 4000); do echo '{"param": "pad", "_offset": 99999999, "_length": 99999999}'; done
    exit 0 ;;
esac
prev=""
for a in "\$@"; do [ "\$prev" = "-r" ] && echo "range \$a" >> "$TMP/ranges.txt"; prev="\$a"; done
exit 1
EOF
chmod +x "$TMP/bigindex/curl"
: > "$TMP/ranges.txt"
out="$(env PATH="$TMP/bigindex:$PATH" DATE=20260629 CYCLE=0 bash "$SCRIPT" ecmwf hrrr 2>&1)"
ranges="$(cat "$TMP/ranges.txt")"
contains "an ECMWF record early in a large index is found" "range 1213794-1214293" "$ranges"
contains "a wgrib2 record early in a large index is found" "range 0-899" "$ranges"
case "$out" in *"no param"*|*"no field matching"*) check "no record reported missing" "" "$out" ;; esac

# Every download that fails partway leaves no file. This stub answers every
# listing and index, then for each download writes a few bytes to the -o path
# and exits 18 (curl's "partial file"), as a dropped connection does. Every
# model runs into a scratch SAMPLES_OUT; afterwards only the two committed
# fixtures `reduced` copies and the `wrf` stand-in may be there.
mkdir -p "$TMP/partial" "$TMP/out"
cat > "$TMP/partial/curl" <<EOF
#!/usr/bin/env bash
out=""; prev=""
for a in "\$@"; do [ "\$prev" = "-o" ] && out="\$a"; prev="\$a"; done
if [ -n "\$out" ]; then printf 'GRIB-truncated' > "\$out"; exit 18; fi
case "\$*" in
  *.index*) echo '{"param": "2t", "_offset": 0, "_length": 500}' ;;
  *.idx*)   printf '1:0:d=2026062900:TMP:surface:anl:\n2:900:d=2026062900:TMP:2 m above ground:anl:\n3:1800:x\n' ;;
  *list-type=2*) echo '<Key>some/prefix/OR_ABI-L2-CMIPM1-M6C13_x.nc</Key>' ;;
esac
exit 0
EOF
chmod +x "$TMP/partial/curl"
env PATH="$TMP/partial:$PATH" SAMPLES_OUT="$TMP/out" DATE=20260629 CYCLE=0 bash "$SCRIPT" >/dev/null 2>&1
left="$(cd "$TMP/out" && find . -type f | sed 's|^\./||' | sort | tr '\n' ' ')"
check "a download cut off partway leaves no file" \
  "octahedral_gaussian_o32.grib2 reduced_gg_n32_smooth.grib1 wrf.nc " "$left"

if [ "$fails" -ne 0 ]; then printf '\n%d check(s) failed\n' "$fails" >&2; exit 1; fi
printf '\nall checks passed\n'
