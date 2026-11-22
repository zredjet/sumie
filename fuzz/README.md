# ファズ

cargo-fuzz の対象（nightly。根の workspace とは別の workspace）。パーサーごとに対象を足す（CLAUDE.md・
architecture.md §9）。種は `seeds/<対象>/`（コミットする）、育ったコーパスは `corpus/<対象>/`（git の外）。

| 対象 | 中身 | 入り口 |
|---|---|---|
| `mem_ops` | mem の入れ物（MVec・SortedVec・SortedMap・stable_sort）を上限つきで動かし、不変の条件・見積もり ≥ 実際を確かめる | `sumie::fuzzing::mem_ops` |
| `math_ops` | math の max・min・clamp・powi の性質と、超越関数が panic しないこと | `sumie::fuzzing::math_ops` |

M1 から `xml`・`dtd`・`css`・`values`・`document` を足す。Kura のファズのコーパス（`/Users/redjet/aruma/repos/zredjet/kura/fuzz/corpus/decode_svg/`。
約 8,000 件。読むだけ）は SVG の対象の種に使う（ADR-21。写さずに、読む場所として渡す）。

## 回し方

```bash
cd fuzz
cargo +nightly fuzz build
mkdir -p corpus/mem_ops
cargo +nightly fuzz run mem_ops corpus/mem_ops seeds/mem_ops -- -max_total_time=3600 -rss_limit_mb=4096 -timeout=20
```

`cargo fuzz` の既定（debug-assertions と overflow-checks が入る）で回し、示しそこなったあふれを panic として見つける
（determinism.md §3）。見つかったものは直して、`crates/sumie/tests/` の試験に足す。
