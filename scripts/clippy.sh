#!/usr/bin/env bash
# clippy（scripts/verify.sh と CI の両方が呼ぶ）。`-D warnings` でも、clippy.toml の引けない口（does not refer to a
# reachable function）は警告のまま通るので、出力から拾って落とす（引けない口は禁止が効いていない）。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
status=0
cargo clippy --locked --workspace --all-targets -- -D warnings 2>&1 | tee "$log" || status=$?
if grep -q "does not refer to a reachable" "$log"; then
  echo "clippy が引けない口が crates/sumie/clippy.toml にある（禁止が効いていない）" >&2
  exit 1
fi
exit "$status"
