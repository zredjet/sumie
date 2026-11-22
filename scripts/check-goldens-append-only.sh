#!/usr/bin/env bash
# 凍結した版のゴールデン（crates/sumie/goldens/render-v<N>.jsonl）が、基準のコミットから変わっていないかを
# 確かめる（determinism.md §10・ADR-05・ADR-20・ADR-41 (7)・ADR-56 (3)）。
#   - 今の版（crates/sumie/src/lib.rs の RENDERER）のファイル：基準の中身が今の中身の頭とバイトで同じ（追記だけ）
#   - 今の版より古いファイル：基準とまったく同じ（凍結した記録。追記もしない）
#   - 基準にあったファイルが消えていない（名前を変えるのも消したのと同じ）
#   - 版 0（render-v0.jsonl）は、今の版が 0 の間だけ見張らない（作り直してよい。ADR-20）。今の版が 1 以上になったら
#     古い版として凍結する（ADR-56 (3)）。estimate.jsonl は見張らない（ADR-33）
# 作業の木（コミットしていない変更を含む）を基準と比べる。ファイルの名前と行の形は tests/golden.rs が見る。
#
#   scripts/check-goldens-append-only.sh [--strict] [基準のコミット]
#       基準の既定は origin/main との分かれ目（なければ main との分かれ目、それもなければ最初のコミット）。
#       --strict は、基準が HEAD と同じ（比べる相手がない）ときに落とす（CI の push で使う）
#   scripts/check-goldens-append-only.sh --history     履歴のすべてのコミットで、版のファイルが追記だけで、消して
#       いない（名前を変えていない）かを確かめる（基準に依らない。CI の取り消された回・失敗のあと・force push でも抜けない）。
#       版 0 のファイルは、親と子の両方で今の版が 0 のコミットだけ外す。コミットしていない変更も HEAD と比べる
#   scripts/check-goldens-append-only.sh --self-test   この確かめ自身を、使い捨ての git で試す
set -euo pipefail

dir="crates/sumie/goldens"

# 今の版の数（RENDERER の "sumie-render/<N>"）。
version_of() {
  sed -n 's/^pub const RENDERER: &str = "sumie-render\/\([0-9][0-9]*\)";$/\1/p'
}

current_version() {
  version_of < crates/sumie/src/lib.rs
}

# コミット <c> の版の数（lib.rs がない・読めないコミットは 0）。
version_at() {
  local v
  v="$(git show "$1:crates/sumie/src/lib.rs" 2>/dev/null | version_of || true)"
  echo "${v:-0}"
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
      estimate.jsonl) continue ;;
      render-v0.jsonl) [[ "$cur" == 0 ]] && continue ;;
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

# history：版のファイルに触れたすべてのコミットで、親から追記だけか・消していないかを確かめる。版 0 のファイルは、
# 親と子の両方で今の版が 0 のコミット（凍結の前）だけ外す。
history_check() {
  local bad=0 c p st f name old n v0_open
  while IFS= read -r c; do
    [[ -z "$c" ]] && continue
    p="$(git rev-parse --verify --quiet "$c^" || true)"
    [[ -z "$p" ]] && continue
    v0_open=""
    if [[ "$(version_at "$p")" == 0 ]] && [[ "$(version_at "$c")" == 0 ]]; then
      v0_open=1
    fi
    while IFS=$'\t' read -r st f; do
      name="${f##*/}"
      case "$name" in
        render-v0.jsonl) [[ -n "$v0_open" ]] && continue ;;
        render-v[0-9]*.jsonl) ;;
        *) continue ;;
      esac
      case "$st" in
        D*)
          echo "$(git rev-parse --short "$c") で消した（名前を変えた）：$f" >&2
          bad=1
          ;;
        M*)
          old="$(mktemp)"
          if ! git show "$p:$f" > "$old"; then
            rm -f "$old"
            return 2
          fi
          n="$(wc -c < "$old" | tr -d ' ')"
          if ! git show "$c:$f" | head -c "$n" | cmp -s - "$old"; then
            echo "$(git rev-parse --short "$c") で書き換えた：$f" >&2
            bad=1
          fi
          rm -f "$old"
          ;;
      esac
    done < <(git diff --no-renames --name-status "$p" "$c" -- "$dir/")
  done < <(git rev-list HEAD -- "$dir/")
  return "$bad"
}

default_base() {
  git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || git rev-list --max-parents=0 HEAD | tail -1
}

self_test() {
  local t
  t="$(mktemp -d)"
  (
    trap 'rm -rf "$t"' EXIT
    cd "$t"
    # 利用者の大域の設定（署名・フック）を受け継がない
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git init -q
    git config user.email t@example.invalid
    git config user.name t
    git config commit.gpgsign false
    mkdir -p "$dir" crates/sumie/src
    printf 'pub const RENDERER: &str = "sumie-render/0";\n' > crates/sumie/src/lib.rs
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
    # 追記はよい
    printf '{"name":"b"}\n' >> "$dir/render-v2.jsonl"
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
    # 今の版が 0 なら版 0 を書き換えてよく、1 以上なら版 0 も凍結（ADR-56 (3)）
    printf '{"name":"y"}\n' > "$dir/render-v0.jsonl"
    expect 0 "$base" 0
    expect 1 "$base" 1
    git checkout -q -- "$dir/render-v0.jsonl"
    # 履歴：追記のコミットは通り、書き換えのコミットと消すコミットを拾う
    printf '{"name":"b"}\n' >> "$dir/render-v2.jsonl"
    git commit -qam append
    history_check 2>/dev/null || { echo "self-test：履歴の追記を誤って断った" >&2; exit 1; }
    printf '{"name":"Z"}\n' > "$dir/render-v2.jsonl"
    git commit -qam rewrite
    if history_check 2>/dev/null; then echo "self-test：履歴の書き換えを見逃した" >&2; exit 1; fi
    git reset -q --hard HEAD~1
    git rm -q "$dir/render-v1.jsonl"
    git commit -qm delete
    if history_check 2>/dev/null; then echo "self-test：履歴の消すコミットを見逃した" >&2; exit 1; fi
    git reset -q --hard HEAD~1
    # 履歴：版 0 の間の版 0 の書き換えは通り、版を 1 に上げたあと（と上げるコミット）の書き換えを拾う
    printf '{"name":"w"}\n' > "$dir/render-v0.jsonl"
    git commit -qam v0-rewrite
    history_check 2>/dev/null || { echo "self-test：版 0 の間の版 0 の書き換えを誤って断った" >&2; exit 1; }
    printf 'pub const RENDERER: &str = "sumie-render/1";\n' > crates/sumie/src/lib.rs
    printf '{"name":"v"}\n' > "$dir/render-v0.jsonl"
    git commit -qam bump-and-rewrite
    if history_check 2>/dev/null; then echo "self-test：版を上げるコミットの版 0 の書き換えを見逃した" >&2; exit 1; fi
    git reset -q --hard HEAD~1
    printf 'pub const RENDERER: &str = "sumie-render/1";\n' > crates/sumie/src/lib.rs
    git commit -qam bump
    history_check 2>/dev/null || { echo "self-test：版を上げるだけのコミットを誤って断った" >&2; exit 1; }
    printf '{"name":"u"}\n' > "$dir/render-v0.jsonl"
    git commit -qam frozen-rewrite
    if history_check 2>/dev/null; then echo "self-test：凍結した版 0 の書き換えを見逃した" >&2; exit 1; fi
  )
  echo "ゴールデンの追記の確かめ：自己試験が通った"
}

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit 0
fi
if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
  echo "浅い clone では履歴と基準を比べられない（CI は fetch-depth: 0 にする）" >&2
  exit 2
fi
if [[ "${1:-}" == "--history" ]]; then
  status=0
  history_check || status=$?
  # コミットしていない変更は HEAD と比べる
  if [[ "$status" == 0 ]]; then
    check HEAD "$(current_version)" || status=$?
  fi
  [[ "$status" == 0 ]] && echo "ゴールデンは履歴のすべてのコミットで追記だけ（コミットしていない変更も）"
  exit "$status"
fi
strict=""
if [[ "${1:-}" == "--strict" ]]; then
  strict=1
  shift
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
if [[ "$(git rev-parse --verify --quiet "${base}^{commit}" || true)" == "$(git rev-parse HEAD)" ]]; then
  echo "注意：基準が HEAD と同じなので、コミットしていない変更だけを見た（コミットした書き換えは --history で見る）" >&2
fi
status=0
check "$base" "$cur" || status=$?
if [[ "$status" == 0 ]]; then
  echo "ゴールデンは $base（$(git rev-parse --short "$base")）から追記だけ（今の版 $cur）"
fi
exit "$status"
