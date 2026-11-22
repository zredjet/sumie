#!/usr/bin/env bash
# 本番の依存が crates.io の libm =0.2.16 だけで、libm の feature が入っていないかを確かめる（ADR-02・NFR-09・ADR-41 (9)）。
# すべてのターゲットの依存を見る（ほかの platform に限る依存もすり抜けない）。[patch] での差し替えも断る。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
deps="$(cargo tree --locked -q -e normal,build --target all -p sumie --prefix none --format '{p}' | sed 's/ (.*)//' | sort -u)"
want="$(printf 'libm v0.2.16\nsumie v0.0.0\n')"
if [[ "$deps" != "$want" ]]; then
  echo "本番の依存が libm だけでない：" >&2
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
# 差し替え（[patch]・[replace]）がなく、Cargo.lock の libm の出どころが crates.io
if grep -qE '^\[(patch|replace)' Cargo.toml; then
  echo "根の Cargo.toml に [patch]・[replace] がある（本番の依存の出どころが変わる）" >&2
  exit 1
fi
src="$(awk '/^name = "libm"$/ { getline; getline; print; exit }' Cargo.lock)"
if [[ "$src" != 'source = "registry+https://github.com/rust-lang/crates.io-index"' ]]; then
  echo "Cargo.lock の libm の出どころが crates.io でない：$src" >&2
  exit 1
fi
echo "本番の依存は crates.io の libm v0.2.16 だけ（feature なし）"
