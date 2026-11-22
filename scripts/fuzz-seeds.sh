#!/usr/bin/env bash
# ファズの対象を組み、各対象の種（fuzz/seeds/<対象>/）を流し直す（落ちないこと。scripts/verify.sh と CI が呼ぶ）。
# 対象は `cargo fuzz list` から取り、種のない対象は誤りにする。nightly は fuzz/rust-toolchain.toml の 1 か所。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/fuzz"
# 固定した nightly（fuzz/rust-toolchain.toml）。外から渡された RUSTUP_TOOLCHAIN は外す。日付つきの名前で入っていなくても、
# 「nightly」が同じ日の channel ならそれを使う（取り直さない）
unset RUSTUP_TOOLCHAIN
pinned="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
tc="$pinned"
if ! rustup toolchain list | grep -q "^$pinned"; then
  date="${pinned#nightly-}"
  sysroot="$(rustc +nightly --print sysroot 2>/dev/null || true)"
  if [[ -n "$sysroot" ]] && grep -q "^date = \"$date\"" "$sysroot/lib/rustlib/multirust-channel-manifest.toml" 2>/dev/null; then
    tc="nightly"
  else
    echo "固定した $pinned がない：rustup toolchain install $pinned --profile minimal" >&2
    exit 1
  fi
fi
cargo_tc() { cargo "+$tc" "$@"; }
# fuzz/Cargo.lock がずれていないこと（cargo fuzz には --locked がない）
cargo_tc metadata --locked --format-version 1 > /dev/null
cargo_tc fuzz build
targets="$(cargo_tc fuzz list)"
if [[ -z "$targets" ]]; then
  echo "ファズの対象がない" >&2
  exit 1
fi
for t in $targets; do
  if [[ ! -d "seeds/$t" ]] || [[ -z "$(ls -A "seeds/$t")" ]]; then
    echo "対象 $t の種（fuzz/seeds/$t/）がない" >&2
    exit 1
  fi
  cargo_tc fuzz run "$t" "seeds/$t" -- -runs=0
done
echo "ファズの対象 $(wc -w <<< "$targets" | tr -d ' ') つの種を流し直した（$tc）"
