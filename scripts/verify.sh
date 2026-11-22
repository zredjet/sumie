#!/usr/bin/env bash
# 手元の全チェック（CLAUDE.md の「全チェック」）。CI（.github/workflows/ci.yml）と同じことを 1 つのターゲットで回し、
# 手元だけのもの（target-cpu=native・Rosetta 2 の x86_64・ファズの組み立て）を足す。
#
#   scripts/verify.sh            全部
#   SUMIE_VERIFY_QUICK=1 scripts/verify.sh   release・native・x86_64・ファズを飛ばす
#   SUMIE_GOLDEN_BASE=<rev>      ゴールデンの追記の確かめの基準（既定は main との分かれ目）
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
quick="${SUMIE_VERIFY_QUICK:-}"

step() { printf '\n== %s\n' "$1"; }

step "整形"
cargo fmt --all -- --check
cargo fmt --manifest-path scripts/lint-probe/Cargo.toml -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check

step "clippy"
log="$(mktemp)"
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tee "$log"
if grep -q "does not refer to a reachable" "$log"; then
  echo "clippy が引けない口が clippy.toml にある" >&2
  exit 1
fi
rm -f "$log"

step "禁じる口の一覧（std の文書から作り直したものと同じか）"
cargo run -q -p sumie-tools -- gen-disallowed --check

step "禁じる口の試し"
cargo run -q -p sumie-tools -- check-lint-probe

step "試験（debug）"
cargo test --workspace

step "ゴールデンは追記だけ"
scripts/check-goldens-append-only.sh --self-test
scripts/check-goldens-append-only.sh ${SUMIE_GOLDEN_BASE:+"$SUMIE_GOLDEN_BASE"}

step "本番の依存"
scripts/check-deps.sh

if [[ -n "$quick" ]]; then
  echo "（SUMIE_VERIFY_QUICK：release・native・x86_64・ファズを飛ばした）"
  exit 0
fi

step "試験（release）"
cargo test -p sumie --release

step "試験（release・target-cpu=native。値がビルドの設定に依らない。determinism.md §1.2）"
RUSTFLAGS="-C target-cpu=native" CARGO_TARGET_DIR="$root/target/native" cargo test -p sumie --release

if rustup target list --installed 2>/dev/null | grep -qx "x86_64-apple-darwin"; then
  step "試験（x86_64-apple-darwin。Rosetta 2。約束の外の安い確かめ）"
  cargo test -p sumie --target x86_64-apple-darwin
else
  echo "（x86_64-apple-darwin のターゲットがないので飛ばした：rustup target add x86_64-apple-darwin）"
fi

if cargo +nightly fuzz --version >/dev/null 2>&1; then
  step "ファズの組み立て（nightly）"
  (cd fuzz && cargo +nightly fuzz build)
else
  echo "（nightly の cargo-fuzz がないので、ファズの組み立てを飛ばした）"
fi

printf '\n全チェックが通った\n'
