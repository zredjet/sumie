# ファズ

cargo-fuzz の対象（nightly。根の workspace とは別の workspace）。パーサーごとに対象を足す（CLAUDE.md・
architecture.md §9）。種は `seeds/<対象>/`（コミットする）、育ったコーパスは `corpus/<対象>/`（git の外）。

| 対象 | 中身 | 入り口 |
|---|---|---|
| `mem_ops` | mem の入れ物（MVec・SortedVec・SortedMap・stable_sort）を上限つきで動かし、不変の条件と、操作ごとの手間と確保の山 ≤ `cost` の式（見積もり）を確かめる。ヒープの実際の量との照らしは確保を数える確保器が要るので `crates/sumie/tests/mutate.rs` が行う | `sumie::fuzzing::mem_ops` |
| `math_ops` | math の max・min・clamp・powi の性質と、超越関数が panic しないこと | `sumie::fuzzing::math_ops` |
| `xml` | XML の段（数える段と作る段）。作る段が数える段の見積もりを超えないこと・1 回の払いが中止の間隔を超えないこと・節の鎖と属性の範囲 | `sumie::fuzzing::xml_parse` |
| `dtd` | 入力を DTD の内部のサブセットにして、実体を本文と属性から参照する文書を `xml` と同じに確かめる（頭の 1 バイトが参照する実体の名前。英小文字ならそのまま） | `sumie::fuzzing::dtd_parse` |
| `css` | 入力を CSS として、決まった文書（型・id・class・言語・名前空間・リンク・空の要素・深い入れ子）の `<style>` に入れ、`document` と同じに確かめる。あわせて鍵の索引の照合が索引を使わない照合と同じ並びを返すか（ADR-77 (2)） | `sumie::fuzzing::css_parse` |
| `values` | 値の読みの口（style の値の確かめ・values の色・塗り・長さ・変形・viewBox・preserveAspectRatio・`font`・`@media`・`@supports`）。頭の 1 バイトでプロパティと属性かを選ぶ。1 回の払い・同じ入力で同じ結果・手間が長さにおおよそ比例すること | `sumie::fuzzing::values_parse` |
| `document` | 読む段の全体（公開の `estimate_parse`・`parse`）。2 つの誤りが同じ（D2）・見積もりが通れば読める・持ち続ける量が見積もりと同じ・数える段と本体の勘定（1 回の払い・返しすぎ）・全体の手間と山 ≤ 見積もり・上限を見積もりのちょうどにすれば読め、1 少なければ同じ `Limit` | `sumie::fuzzing::document_parse` |
| `path` | パスのデータと `points` の読み（`values::path`）と平坦化（`geom::flatten`）。頭の 1 バイトで読む口（パスのデータか `points`）と変形を選ぶ。置き場に書く量が入力の 1 バイトあたり `cost::PATH_GEOMETRY_BYTES` 以下・1 回の払い・同じ入力で同じ結果・平坦化の点の数が分ける数の和（見積もりと同じ式）・点が区間の厳密な外接矩形の中 | `sumie::fuzzing::path_ops` |

種は `crates/sumie/tests/mutate.rs` の `MEM_SEEDS` と同じにする（試験 `fuzz_seeds_match_mem_seeds` が照らす）。
`xml`・`dtd`・`css`・`values`・`document`・`path` の種は `crates/sumie/tests/mutate.rs` の種つきの変異の試験も使う（`dtd` は DTD の、`css` は
CSS の目印の字句で変異させる）。libFuzzer の既定の長さ（4096 バイト）
では、長い名前・大きな実体のような長い入力の場合に届かないので、それは `crates/sumie/src/xml/tests.rs` の試験が受け持つ。
Kura のファズのコーパス（`/Users/redjet/aruma/repos/zredjet/kura/fuzz/corpus/decode_svg/`。
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
