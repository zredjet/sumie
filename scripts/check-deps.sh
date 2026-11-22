#!/usr/bin/env bash
# 本番の依存が crates.io の libm =0.2.16 だけで、libm の feature が入っていないかを確かめる（ADR-02・NFR-09・ADR-41 (9)）。
# すべてのターゲットの依存を見る（ほかの platform に限る依存もすり抜けない）。[patch] での差し替えも断る。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
# crates.io の依存は「名前 版」だけ、git・path の出どころには「 (…)」が付く。libm は括弧なし（crates.io）でなければならない
deps="$(cargo tree --locked -q -e normal,build --target all -p sumie --prefix none --format '{p}' | sort -u)"
libm="$(grep '^libm ' <<< "$deps" || true)"
others="$(grep -v -e '^libm ' -e '^sumie v0\.0\.0 (' <<< "$deps" || true)"
if [[ "$libm" != "libm v0.2.16" ]] || [[ -n "$others" ]]; then
  echo "本番の依存が crates.io の libm v0.2.16 だけでない：" >&2
  echo "$deps" >&2
  exit 1
fi
# libm の feature の行（`libm feature "…"`）が 1 つもないこと（`sumie feature "default"` は sumie 自身の印）。dev の辺は見ない
feats="$(cargo tree --locked -q -e normal,build,features --target all -i libm -p sumie)"
if grep -qE 'libm feature "' <<< "$feats"; then
  echo "libm の feature が入っている（default-features = false のはず）：" >&2
  echo "$feats" >&2
  exit 1
fi
# 差し替え（[patch]・[replace]・点つきの patch の鍵・.cargo/config の [patch]・[source]）がない
if grep -qE '^\s*\[(patch|replace)|^\s*patch\.' Cargo.toml; then
  echo "根の Cargo.toml に [patch]・[replace] がある（本番の依存の出どころが変わる）" >&2
  exit 1
fi
for c in .cargo/config.toml .cargo/config; do
  if [[ -f "$c" ]] && grep -qE '^\s*\[(patch|source)|^\s*(patch|source)\.' "$c"; then
    echo "$c に [patch]・[source] がある（本番の依存の出どころが変わる）" >&2
    exit 1
  fi
done
echo "本番の依存は crates.io の libm v0.2.16 だけ（feature なし）"
