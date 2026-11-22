#!/usr/bin/env bash
# clippy（scripts/verify.sh と CI の両方が呼ぶ）。`-D warnings` でも、clippy.toml の口を引けない・関数でない物を指す
# （does not refer to a reachable function・expected a function, found a struct など）診断は警告のまま通るので、
# 出力から拾って落とす（その口は禁止が効いていない）。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
status=0
cargo clippy --locked --workspace --all-targets -- -D warnings 2>&1 | tee "$log" || status=$?
# 設定の場所を指す診断（引けない口・関数でない物を指す口 expected a function, found … など）は、その口を黙って無視した印
# 色の印（CI の CARGO_TERM_COLOR=always）を除いてから見る
plain="$(mktemp)"
trap 'rm -f "$log" "$plain"' EXIT
sed -E $'s/\x1b\\[[0-9;]*[mK]//g' "$log" > "$plain"
if grep -qE "does not refer to a reachable|clippy\.toml:[0-9]+:[0-9]+" "$plain"; then
  echo "clippy が crates/sumie/clippy.toml の口を引けない・関数として扱えない（禁止が効いていない）：" >&2
  grep -E -B2 "does not refer to a reachable|clippy\.toml:[0-9]+:[0-9]+" "$plain" >&2 || true
  exit 1
fi
exit "$status"
