#!/usr/bin/env bash
# 凍結した版のゴールデン（crates/sumie/goldens/render-v<N>.jsonl、N ≥ 1）が、基準のコミットから追記だけかを確かめる
# （determinism.md §10・ADR-05・ADR-20）。版 0（render-v0.jsonl）と estimate.jsonl は見張らない。
#
#   scripts/check-goldens-append-only.sh [基準のコミット]   既定は main との分かれ目（なければ最初のコミット）
#   scripts/check-goldens-append-only.sh --self-test        この確かめ自身を、使い捨ての git で試す
#
# 作業の木（コミットしていない変更を含む）を基準と比べる：基準にあったファイルが消えていない・基準の中身が今の中身の
# 頭とバイトで同じ。
set -euo pipefail

dir="crates/sumie/goldens"

check() {
  local base="$1"
  local bad=0
  local files
  files="$(git ls-tree --name-only "$base" "$dir/" 2>/dev/null | grep -E '/render-v[0-9]+\.jsonl$' || true)"
  for f in $files; do
    case "$f" in
      */render-v0.jsonl) continue ;;
    esac
    if [[ ! -f "$f" ]]; then
      echo "消えた：$f（凍結した版のゴールデンは消さない）" >&2
      bad=1
      continue
    fi
    local old
    old="$(mktemp)"
    git show "$base:$f" > "$old"
    local n
    n="$(wc -c < "$old" | tr -d ' ')"
    # 今の中身の頭 n バイトが基準の中身と同じか（macOS の cmp -n は短い方の終わりで失敗するので head で切る）
    if ! head -c "$n" "$f" | cmp -s - "$old"; then
      echo "書き換えた：$f（$base からの追記だけにする。既存の行を変えない・消さない）" >&2
      bad=1
    fi
    rm -f "$old"
  done
  return "$bad"
}

default_base() {
  git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || git rev-list --max-parents=0 HEAD | tail -1
}

self_test() {
  local t
  t="$(mktemp -d)"
  trap 'rm -rf "$t"' RETURN
  (
    cd "$t"
    git init -q
    git config user.email t@example.invalid
    git config user.name t
    mkdir -p "$dir"
    printf '{"name":"a"}\n' > "$dir/render-v1.jsonl"
    printf '{"name":"x"}\n' > "$dir/render-v0.jsonl"
    git add . && git commit -qm base
    base="$(git rev-parse HEAD)"
    # 追記はよい・版 0 は書き換えてよい
    printf '{"name":"b"}\n' >> "$dir/render-v1.jsonl"
    printf '{"name":"y"}\n' > "$dir/render-v0.jsonl"
    check "$base" || { echo "self-test：追記を誤って断った" >&2; exit 1; }
    # 書き換えはだめ
    printf '{"name":"A"}\n' > "$dir/render-v1.jsonl"
    if check "$base" 2>/dev/null; then echo "self-test：書き換えを見逃した" >&2; exit 1; fi
    # 消すのはだめ
    rm "$dir/render-v1.jsonl"
    if check "$base" 2>/dev/null; then echo "self-test：消したのを見逃した" >&2; exit 1; fi
  )
  echo "ゴールデンの追記の確かめ：自己試験が通った"
}

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit 0
fi
base="${1:-$(default_base)}"
if check "$base"; then
  echo "ゴールデンは $base から追記だけ"
else
  exit 1
fi
