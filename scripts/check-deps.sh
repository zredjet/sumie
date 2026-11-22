#!/usr/bin/env bash
# 本番の依存が libm だけで、libm の arch・既定の feature が入っていないかを確かめる（ADR-02・NFR-09・ADR-41 (9)）。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
deps="$(cargo tree -q -e normal,build -p sumie --prefix none --format '{p}' | sed 's/ (.*)//' | sort -u)"
want="$(printf 'libm v0.2.16\nsumie v0.0.0\n')"
if [[ "$deps" != "$want" ]]; then
  echo "本番の依存が libm だけでない：" >&2
  echo "$deps" >&2
  exit 1
fi
feats="$(cargo tree -q -e features -i libm -p sumie)"
# libm の feature の行（`libm feature "…"`）が 1 つもないこと（sumie feature "default" は sumie 自身の印）
if grep -qE 'libm feature "' <<<"$feats"; then
  echo "libm の feature が入っている（default-features = false のはず）：" >&2
  echo "$feats" >&2
  exit 1
fi
echo "本番の依存は libm v0.2.16 だけ（feature なし）"
