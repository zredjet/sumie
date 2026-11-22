#!/usr/bin/env bash
# 手元の全チェック（CLAUDE.md の「全チェック」）。CI（.github/workflows/ci.yml）と同じスクリプトを 1 つのターゲットで回し、
# 手元だけの段（Rosetta 2 の x86_64・ファズの組み立てと種の流し直し）を足す。
#
#   scripts/verify.sh                         全部（飛ばした段があれば最後に並べ、終わりの状態を 3 にする）
#   SUMIE_VERIFY_QUICK=1 scripts/verify.sh    release・native・x86_64・ファズを飛ばす（終わりの状態は 3）
#   SUMIE_GOLDEN_BASE=<rev> scripts/verify.sh ゴールデンの追記の確かめの基準（既定は main との分かれ目）
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
quick="${SUMIE_VERIFY_QUICK:-}"
skipped=()

step() { printf '\n== %s\n' "$1"; }

step "整形"
cargo fmt --all -- --check
cargo fmt --manifest-path scripts/lint-probe/Cargo.toml -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check

step "clippy（引けない口も拾う）"
scripts/clippy.sh

step "禁じる口の一覧（std の文書から作り直したものと同じか）"
cargo run --locked -q -p sumie-tools -- gen-disallowed --check

step "禁じる口の試し"
cargo run --locked -q -p sumie-tools -- check-lint-probe

step "試験（debug）"
cargo test --locked --workspace

step "ゴールデンは追記だけ"
scripts/check-goldens-append-only.sh --self-test
scripts/check-goldens-append-only.sh ${SUMIE_GOLDEN_BASE:+"$SUMIE_GOLDEN_BASE"}

step "本番の依存"
scripts/check-deps.sh

if [[ -n "$quick" ]]; then
  skipped+=("release・native・x86_64・ファズ（SUMIE_VERIFY_QUICK）")
else
  step "試験（release）"
  cargo test --locked -p sumie --release

  step "試験（release・target-cpu=native。値がビルドの設定に依らない。determinism.md §1.2）"
  RUSTFLAGS="-C target-cpu=native" CARGO_TARGET_DIR="$root/target/native" cargo test --locked -p sumie --release

  if ! rustup target list --installed 2>/dev/null | grep -qx "x86_64-apple-darwin"; then
    skipped+=("x86_64-apple-darwin（ターゲットがない：rustup target add x86_64-apple-darwin）")
  elif ! arch -x86_64 /usr/bin/true 2>/dev/null; then
    skipped+=("x86_64-apple-darwin（Rosetta 2 がない）")
  else
    step "試験（x86_64-apple-darwin。Rosetta 2。約束の外の安い確かめ）"
    cargo test --locked -p sumie --target x86_64-apple-darwin
  fi

  if cargo +nightly fuzz --version >/dev/null 2>&1; then
    step "ファズの組み立てと種の流し直し（nightly）"
    (
      cd fuzz
      cargo +nightly fuzz build
      for t in mem_ops math_ops; do
        cargo +nightly fuzz run "$t" "seeds/$t" -- -runs=0
      done
    )
  else
    skipped+=("ファズ（nightly の cargo-fuzz がない）")
  fi
fi

if [[ ${#skipped[@]} -gt 0 ]]; then
  printf '\n全チェックの残りは通った。飛ばした段：\n'
  printf '  - %s\n' "${skipped[@]}"
  exit 3
fi
printf '\n全チェックが通った（飛ばした段なし）\n'
