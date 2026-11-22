# ファズ

cargo-fuzz の対象（nightly。根の workspace とは別の workspace）。パーサーごとに対象を足す（CLAUDE.md・
architecture.md §9）。種は `seeds/<対象>/`（コミットする）、育ったコーパスは `corpus/<対象>/`（git の外）。

| 対象 | 中身 | 入り口 |
|---|---|---|
| `mem_ops` | mem の入れ物（MVec・SortedVec・SortedMap・stable_sort）を上限つきで動かし、不変の条件と、操作ごとの手間と確保の山 ≤ `cost` の式（見積もり）を確かめる。ヒープの実際の量との照らしは確保を数える確保器が要るので `crates/sumie/tests/mutate.rs` が行う | `sumie::fuzzing::mem_ops` |
| `math_ops` | math の max・min・clamp・powi の性質と、超越関数が panic しないこと | `sumie::fuzzing::math_ops` |

種は `crates/sumie/tests/mutate.rs` の `MEM_SEEDS` と同じにする（試験 `fuzz_seeds_match_mem_seeds` が照らす）。
M1 から `xml`・`dtd`・`css`・`values`・`document` を足す。Kura のファズのコーパス（`/Users/redjet/aruma/repos/zredjet/kura/fuzz/corpus/decode_svg/`。
約 8,000 件。読むだけ）は SVG の対象の種に使う（ADR-21。写さずに、読む場所として渡す）。

## 回し方

nightly は `fuzz/rust-toolchain.toml`（`nightly-2026-10-16`）の 1 か所で決め、`fuzz/` の中で `cargo fuzz` を回すと
それが使われる（`+nightly` は付けない）。組み立てと種の流し直しは `scripts/fuzz-seeds.sh`（全チェックと CI が呼ぶ）。

```bash
cd fuzz
cargo fuzz build
mkdir -p corpus/mem_ops
cargo fuzz run mem_ops corpus/mem_ops seeds/mem_ops -- -max_total_time=3600 -rss_limit_mb=4096 -timeout=20
# 種を流し直すだけ（CI と scripts/verify.sh）
cargo fuzz run mem_ops seeds/mem_ops -- -runs=0
```

`cargo fuzz` の既定（debug-assertions と overflow-checks が入る）で回し、示しそこなったあふれを panic として見つける
（determinism.md §3）。見つかったものは直して、`crates/sumie/tests/` の試験に足す。
