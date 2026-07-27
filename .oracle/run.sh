#!/usr/bin/env bash
#
# Run the oracle against every XMIR in the parser's 1-parse output tree and
# write a Markdown report.
#
#   ./.oracle/run.sh [source-dir] [output-file]
#
# Defaults:
#   source-dir  = $EO_HOME/eo-runtime/target/eo/1-parse
#   output-file = eo-type-inference-report.md   (in the repository root)
#
# This repository does not contain EO. Point EO_HOME at an objectionary/eo
# checkout, or pass its 1-parse directory as $1. The XMIRs must exist first;
# see section 4 of the README for generating them.

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
py="$here/eo_type_inference.py"
src="${1:-${EO_HOME:+$EO_HOME/eo-runtime/target/eo/1-parse}}"
out="${2:-$root/eo-type-inference-report.md}"

if [ ! -f "$py" ]; then
  echo "checker not found: $py" >&2
  exit 1
fi
if [ ! -d "$src" ]; then
  echo "no XMIR found at: ${src:-<unset>}" >&2
  echo "set EO_HOME to an EO checkout or pass its 1-parse directory as \$1" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
files="$work/files"
results="$work/results"
reasons="$work/reasons"
: > "$results"
: > "$reasons"

find "$src" -name '*.xmir' | sort > "$files"

total=0
ok=0
rej=0
cur=""
while IFS= read -r line; do
  case "$line" in
    "== "*" ==")
      cur="${line#== }"; cur="${cur% ==}"; cur="${cur#"$src"/}"
      ;;
    *" : "*)
      total=$((total + 1))
      res="${line#* : }"
      name="${line%% : *}"; name="${name#  }"
      printf '%-32s %s\n' "$cur" "$name : $res" >> "$results"
      if printf '%s' "$res" | grep -q 'REJECT\|unmodelled'; then
        rej=$((rej + 1))
        printf '%s\n' "${res#REJECT -- }" >> "$reasons"
      else
        ok=$((ok + 1))
      fi
      ;;
  esac
done < <(xargs python3 "$py" < "$files" 2>&1)

# --- second pass: object-level incompleteness smells (opt-in rule) ----------
smells="$work/smells"
: > "$smells"
smellcount=0
cur=""
while IFS= read -r line; do
  case "$line" in
    "== "*" ==")
      cur="${line#== }"; cur="${cur% ==}"; cur="${cur#"$src"/}"
      ;;
    *" : "*)
      res="${line#* : }"
      if printf '%s' "$res" | grep -q 'REJECT'; then
        smellcount=$((smellcount + 1))
        printf '%-26s %s\n' "$cur" "${res#REJECT -- }" >> "$smells"
      fi
      ;;
  esac
done < <(EO_INCOMPLETE=1 xargs python3 "$py" < "$files" 2>&1)

{
  echo "# EO type-inference run"
  echo
  echo "- Checker: \`$(basename "$py")\`"
  echo "- Source: \`$src\`"
  echo "- Generated: $(date '+%Y-%m-%d %H:%M:%S')"
  echo
  echo "## Summary"
  echo
  echo "| metric | count |"
  echo "| --- | --- |"
  echo "| objects | $total |"
  echo "| typed OK | $ok |"
  echo "| rejected | $rej |"
  echo "| incompleteness smells | $smellcount |"
  echo
  if [ "$smellcount" -gt 0 ]; then
    echo "## Design smells: incomplete objects (object-level fragility)"
    echo
    echo 'Under the object-level rule -- *an object with any unset attribute is fragile* --'
    echo "these $smellcount object(s) dispatch on a half-built object. Each must set the"
    echo 'missing attribute, use `?.`, or be recovered. These are design smells, not shape'
    echo 'errors: they all pass the shape check above.'
    echo
    echo '```'
    cat "$smells"
    echo '```'
    echo
  fi
  if [ "$rej" -gt 0 ]; then
    echo "## Rejections (grouped, most frequent first)"
    echo
    sort "$reasons" | uniq -c | sort -rn \
      | sed -E 's/^ *([0-9]+) (.*)$/- **\1x** \2/'
    echo
  fi
  echo "## Results"
  echo
  echo '```'
  cat "$results"
  echo '```'
} > "$out"

echo "wrote $out  ($ok/$total typed OK, $rej rejected, $smellcount incompleteness smell(s))"
