#!/usr/bin/env bash
# tests/svg/ と corpus/ の SVG（16 KiB 以下）を、生の SVG を読むファズの対象（render・tree）のコーパス（git の外の
# fuzz/corpus/<対象>）に流し込む（ADR-100 (3) (d)）。render の入力は頭の 4 バイト（倍率と範囲）を付ける：倍率 1・0.5・
# 0.25 の 3 つ。名前は中身の SHA-256 の頭（同じ中身は 1 つ。数えるのも違う中身の数）。
#
#   scripts/fuzz-corpus.sh
#   cd fuzz && cargo fuzz run render corpus/render seeds/render -- -dict=dict/svg.dict -max_total_time=3600 -rss_limit_mb=4096 -timeout=20
#   cd fuzz && cargo fuzz run tree corpus/tree seeds/tree -- -dict=dict/svg.dict -max_total_time=3600 -rss_limit_mb=4096 -timeout=20
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p fuzz/corpus/render fuzz/corpus/tree
while IFS= read -r -d '' f; do
    size=$(wc -c < "$f")
    if [ "$size" -gt 16384 ]; then
        continue
    fi
    h=$(shasum -a 256 "$f" | cut -c1-32)
    cp "$f" "fuzz/corpus/tree/svg-$h"
    { printf '\000\005\011\067'; cat "$f"; } > "fuzz/corpus/render/svg-$h-1"
    { printf '\001\021\041\234'; cat "$f"; } > "fuzz/corpus/render/svg-$h-2"
    { printf '\003\002\003\063'; cat "$f"; } > "fuzz/corpus/render/svg-$h-3"
done < <(find tests/svg corpus -name '*.svg' -print0 | sort -z)
n=$(find fuzz/corpus/tree -name 'svg-*' | wc -l | tr -d ' ')
echo "違う中身 $n 件を fuzz/corpus/render（倍率 3 つ）・fuzz/corpus/tree に入れた（前に入れたものを含む）"
