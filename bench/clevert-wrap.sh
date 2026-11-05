#!/bin/bash
# Adapts the classic ECT command line (used by bench.py and the
# conformance harness) to clevert-app/ect. Usage: CLEVERT=<binary> clevert-wrap.sh [classic args]
# - -1 does not exist in clevert (levels 2-9): mapped to -2
# - -strip/-quiet -> --strip/--quiet; --mt-file/-keep/--disable-* dropped
# - -gzip FILE: clevert --gzip only compresses stdin, so each non-gzip file
#   is piped to FILE.gz; existing .gz files are optimized in place
# - -zip on existing archives: in-place rewrite + --compress-stored (ECT -zip
#   deflates stored entries unless --strict)
bin="$CLEVERT"; level=-3; gz=0; zip=0; strict=0; opts=(); files=()
for a in "$@"; do
  case "$a" in
    -1) level=-2 ;; -[2-9]) level="$a" ;;
    -strip) opts+=(--strip) ;; -quiet) opts+=(--quiet) ;;
    --strict) strict=1; opts+=(--strict) ;;
    -gzip) gz=1 ;; -zip) zip=1 ;;
    --mt-file*|-keep|--disable-*|-recurse) ;;
    --) ;;
    *) files+=("$a") ;;
  esac
done
[ $zip = 1 ] && [ $strict = 0 ] && opts+=(--compress-stored)
rc=0; inplace=()
for f in "${files[@]}"; do
  if [ $gz = 1 ] && [ $zip = 0 ] && [ "$(head -c2 "$f" | od -An -tx1 | tr -d ' ')" != "1f8b" ]; then
    if [ -e "$f.gz" ]; then echo "$f.gz exists" >&2; rc=1; continue; fi
    "$bin" "$level" "${opts[@]}" --gzip -o "$f.gz" < "$f" || rc=$?
  else
    inplace+=("$f")
  fi
done
if [ ${#inplace[@]} -gt 0 ]; then "$bin" "$level" "${opts[@]}" "${inplace[@]}" || rc=$?; fi
exit $rc
