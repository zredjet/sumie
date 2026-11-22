#!/usr/bin/env bash
# 凍結した版のゴールデン（crates/sumie/goldens/render-v<N>.jsonl、N ≥ 1）が、基準のコミットから変わっていないかを
# 確かめる（determinism.md §10・ADR-05・ADR-20・ADR-41 (7)）。
#   - 今の版（crates/sumie/src/lib.rs の RENDERER）のファイル：基準の中身が今の中身の頭とバイトで同じ（追記だけ）
#   - 今の版より古いファイル：基準とまったく同じ（凍結した記録。追記もしない）
#   - 基準にあったファイルが消えていない（名前を変えるのも消したのと同じ）
#   - 版 0（render-v0.jsonl）と estimate.jsonl は見張らない（ADR-20・ADR-33）
# 作業の木（コミットしていない変更を含む）を基準と比べる。ファイルの名前と行の形は tests/golden.rs が見る。
#
#   scripts/check-goldens-append-only.sh [--strict] [基準のコミット]
#       基準の既定は origin/main との分かれ目（なければ main との分かれ目、それもなければ最初のコミット）。
#       --strict は、基準が HEAD と同じ（比べる相手がない）ときに落とす（CI の push で使う）
#   scripts/check-goldens-append-only.sh --self-test   この確かめ自身を、使い捨ての git で試す
set -euo pipefail

dir="crates/sumie/goldens"

# 今の版の数（RENDERER の "sumie-render/<N>"）。
current_version() {
  sed -n 's/^pub const RENDERER: &str = "sumie-render\/\([0-9][0-9]*\)";$/\1/p' crates/sumie/src/lib.rs
}

# check <基準> <今の版>：0 は通る、1 は変えた、2 は確かめられない（基準がない・読めない）
check() {
  local base="$1" cur="$2"
  if ! git rev-parse --verify --quiet "${base}^{commit}" >/dev/null; then
    echo "基準のコミット $base がない（打ち間違いか、浅い clone）" >&2
    return 2
  fi
  local listing
  if ! listing="$(git ls-tree --name-only "$base" "$dir/")"; then
    echo "基準 $base のファイルの一覧を読めない" >&2
    return 2
  fi
  local bad=0 f name v old n
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    name="${f##*/}"
    case "$name" in
      render-v0.jsonl | estimate.jsonl) continue ;;
      render-v[0-9]*.jsonl) ;;
      *) continue ;;
    esac
    v="${name#render-v}"
    v="${v%.jsonl}"
    if [[ ! -f "$f" ]]; then
      echo "消えた：$f（凍結した版のゴールデンは消さない・名前を変えない）" >&2
      bad=1
      continue
    fi
    old="$(mktemp)"
    if ! git show "$base:$f" > "$old"; then
      echo "基準の $f を読めない" >&2
      rm -f "$old"
      return 2
    fi
    if [[ "$v" == "$cur" ]]; then
      n="$(wc -c < "$old" | tr -d ' ')"
      # 今の中身の頭 n バイトが基準の中身と同じか（macOS の cmp -n は短い方の終わりで失敗するので head で切る）
      if ! head -c "$n" "$f" | cmp -s - "$old"; then
        echo "書き換えた：$f（$base からの追記だけにする。既存の行を変えない・消さない）" >&2
        bad=1
      fi
    elif ! cmp -s "$old" "$f"; then
      echo "古い版を変えた：$f（今の版は $cur。古い版のファイルは記録として変えない）" >&2
      bad=1
    fi
    rm -f "$old"
  done <<< "$listing"
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
    # 利用者の大域の設定（署名・フック）を受け継がない
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git init -q
    git config user.email t@example.invalid
    git config user.name t
    git config commit.gpgsign false
    mkdir -p "$dir"
    printf '{"name":"a"}\n' > "$dir/render-v2.jsonl"
    printf '{"name":"o"}\n' > "$dir/render-v1.jsonl"
    printf '{"name":"x"}\n' > "$dir/render-v0.jsonl"
    git add . && git commit -qm base
    base="$(git rev-parse HEAD)"
    expect() {
      local want="$1" got=0
      shift
      check "$@" 2>/dev/null || got=$?
      if [[ "$got" != "$want" ]]; then
        echo "self-test：$* の結果が $got（期待 $want）" >&2
        exit 1
      fi
    }
    # 追記はよい・版 0 は書き換えてよい
    printf '{"name":"b"}\n' >> "$dir/render-v2.jsonl"
    printf '{"name":"y"}\n' > "$dir/render-v0.jsonl"
    expect 0 "$base" 2
    # 今の版の書き換え・末尾を切る
    printf '{"name":"A"}\n' > "$dir/render-v2.jsonl"
    expect 1 "$base" 2
    printf '{"name":' > "$dir/render-v2.jsonl"
    expect 1 "$base" 2
    git checkout -q -- "$dir/render-v2.jsonl"
    # 古い版への追記もだめ
    printf '{"name":"p"}\n' >> "$dir/render-v1.jsonl"
    expect 1 "$base" 2
    git checkout -q -- "$dir/render-v1.jsonl"
    # 消す・名前を変えるのはだめ
    git mv "$dir/render-v2.jsonl" "$dir/render-v3.jsonl"
    expect 1 "$base" 2
    git mv "$dir/render-v3.jsonl" "$dir/render-v2.jsonl"
    rm "$dir/render-v1.jsonl"
    expect 1 "$base" 2
    git checkout -q -- "$dir/render-v1.jsonl"
    # 基準がない
    expect 2 no-such-rev 2
    expect 0 "$base" 2
  )
  echo "ゴールデンの追記の確かめ：自己試験が通った"
}

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit 0
fi
strict=""
if [[ "${1:-}" == "--strict" ]]; then
  strict=1
  shift
fi
if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
  echo "浅い clone では基準を比べられない（CI は fetch-depth: 0 にする）" >&2
  exit 2
fi
base="${1:-$(default_base)}"
cur="$(current_version)"
if [[ -z "$cur" ]]; then
  echo "crates/sumie/src/lib.rs の RENDERER から今の版を読めない" >&2
  exit 2
fi
if [[ -n "$strict" ]] && [[ "$(git rev-parse --verify --quiet "${base}^{commit}" || true)" == "$(git rev-parse HEAD)" ]]; then
  echo "基準 $base が HEAD と同じで、比べる相手がない（--strict）" >&2
  exit 2
fi
status=0
check "$base" "$cur" || status=$?
if [[ "$status" == 0 ]]; then
  echo "ゴールデンは $base（$(git rev-parse --short "$base")）から追記だけ（今の版 $cur）"
fi
exit "$status"
