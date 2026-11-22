# sumie — 決定的で安全な SVG の描画器

静的な SVG を読み、RGBA8（乗算済み）の画素に描く Rust のライブラリ。Kura（Mac 用のメディアライブラリ）の編集のエンジンが
resvg・別のプロセスの働き手・外からの見積もり・SVG のアンカーをやめられるようにする。本番の依存は `libm` だけで、
XML・CSS・パス・ラスタライズ・フォント・文字の組版まで自作する。ライセンスは MIT OR Apache-2.0（ADR-12）。

返答・文書・コミットのメッセージはすべて日本語で書く。

## 最優先の原則

1. **値の再現（D1・D2）**：同じ入力の組（SVG のバイト列・フォントのバイト列と鍵と順・設定・版）なら、aarch64-apple-darwin・
   x86_64・aarch64 の Linux のどれでも、画素・大きさ・版に入る報告がビット単位で同じ（D1。版ごと）。見積もり・断るかどうか・
   警告も、同じ rev なら同じ（D2）。規則は `docs/determinism.md`
2. **安全**：どんな入力（SVG とフォントの両方）でも、panic・スタックのあふれ・終わらないループ・上限を超える確保をしない。
   途中で止められる。手間と確保を前もって見積もれる
3. **完全な自作**：resvg・usvg・tiny-skia などのコードを写さない。W3C の SVG 1.1・SVG 2・CSS・OpenType・Apple の TrueType（AAT）・
   Unicode の仕様から書く。resvg は dev 依存の照合の相手（`=0.48.1`）として使うだけ
4. **仕様は docs が正**：実装と docs が食い違ったら、勝手に仕様を変えない。仕様の判断は推測せずに質問し、決まったら
   `docs/decisions.md` に追記する

## 技術スタック

- Rust の edition 2024・stable（`rust-toolchain.toml` で 1.99.0 に固定）。ファズだけ nightly
- 本番の依存：`libm = { version = "=0.2.16", default-features = false }` だけ。ほかを足すときは理由を書いて私に聞く
- dev の依存（照合・計測の道具 `sumie-tools` と試験）：resvg `=0.48.1`（Kura と同じ feature：`text` だけ）・BLAKE3・確保を
  数える確保器のクレートなど。ライブラリの `cargo test` には重い dev の依存を入れない。試験にも `unsafe` を書かない
- OS に依存しない。Linux でも `cargo test` が通る。crates.io に公開しない（`publish = false`）
- CI：GitHub Actions（公開のリポジトリ `zredjet/sumie`。ADR-54。`ubuntu-latest`・`ubuntu-24.04-arm`・`macos-15`（arm64）。ADR-22・ADR-44）。Kura は git の rev を
  固定した依存で取り込む（ADR-18）

## リポジトリ構成

```
Cargo.toml                 # workspace。[workspace.lints]（受け継ぐのは crates/sumie だけ）・profile
rust-toolchain.toml        # ツールチェーンの固定
crates/sumie/              # ライブラリ（本番の依存は libm だけ）。段ごとのモジュールは docs/architecture.md §2
crates/sumie/clippy.toml   # 決定性と安全の厳しい規則（disallowed-methods・disallowed-types・disallowed-macros）。ライブラリだけ
crates/sumie-testkit/      # 試験の共通の道具（自前の ChaCha8・種つきの変異・確保を数える確保器の包み）
crates/sumie/tests/        # 結合テスト（rules.rs：src の走査、golden.rs、region.rs、stack.rs、limits.rs、alloc.rs …）
crates/sumie/goldens/      # render-v<N>.jsonl（凍結した版は追記だけ）
crates/sumie-tools/        # 照合（resvg・Chrome）・計測・重い入力の生成・Unicode の表の生成（本番に入らない）
fuzz/                      # cargo-fuzz（独立の workspace）
tests/svg/                 # 自作の試験の SVG
tests/fonts/               # 試験用のフォント（写しを許すライセンスのものだけ。出どころの一覧つき。M5 で作る）
corpus/                    # 照合の集まり（取り込んだ試験の SVG と実例の SVG。出どころ・版・ライセンスの写しつき。ADR-21・ADR-59）
scripts/                   # verify.sh・clippy.sh・check-deps.sh・check-goldens-append-only.sh・fuzz-seeds.sh・lint-probe/
docs/                      # 正本の文書
```

（M0 で作った。`tests/fonts/` は M5 から。ほかに `.github/workflows/ci.yml`、git の外の `local/`（W3C の試験・重い入力・照合の結果）。`scripts/lint-probe/` は禁じる口の試しのクレート）

## コマンド

| 目的 | コマンド |
|---|---|
| 全チェック | `scripts/verify.sh`（整形・clippy・禁じる口の照らしと試し・テスト（debug・release・target-cpu=native・x86_64）・ゴールデンの追記だけの検査・依存の検査・ファズの組み立てと種の流し直し。飛ばした段があれば並べて終わりの状態 3。`SUMIE_VERIFY_QUICK=1` で速い方、`SUMIE_GOLDEN_BASE=<rev>` で基準） |
| 整形 | `cargo fmt --all -- --check` |
| lint | `scripts/clippy.sh`（`cargo clippy --workspace --all-targets -- -D warnings` に、clippy.toml の引けない口の検出を足したもの） |
| 禁じる口の一覧 | `cargo run -p sumie-tools -- gen-disallowed [--check]`（std の文書から作る）・`cargo run -p sumie-tools -- check-lint-probe`（本当に拾われるか） |
| テスト | `cargo test --workspace` |
| ゴールデン | `cargo test -p sumie --test golden` |
| ゴールデンは追記だけ | `scripts/check-goldens-append-only.sh [--strict] [基準のコミット]`（`--history` で履歴のすべてのコミット、`--self-test` で自己試験） |
| 本番の依存が libm だけ | `scripts/check-deps.sh`（`cargo tree -e normal,build --target all -p sumie` と libm の feature・出どころ） |
| x86_64 の命令で | `cargo test -p sumie --target x86_64-apple-darwin`（Rosetta 2。先に `rustup target add x86_64-apple-darwin`） |
| ファズ | `cd fuzz && cargo fuzz run <対象> corpus/<対象> seeds/<対象> -- -max_total_time=3600 -rss_limit_mb=4096 -timeout=20`（先に `mkdir -p corpus/<対象>`。nightly は `fuzz/rust-toolchain.toml` の 1 か所）・種の流し直しは `scripts/fuzz-seeds.sh` |
| 照合 | `cargo run -p sumie-tools --release -- compare [--root corpus] [--filter <道の一部>] [--min-side 200] [--out local/compare/<名前>]`（M0 は resvg と Chrome。差の一覧の HTML と分布） |
| 計測 | `cargo run -p sumie-tools --release -- bench [--root corpus\|local/heavy] [--filter <道の一部>] [--repeat N] [--timeout 秒] [--out <file>]`（resvg 単体の時間と確保の山） |

## 実装ルール（全体）

### 決定性（詳細は docs/determinism.md）

- 超越関数（sin・cos・tan・atan2・exp・ln・pow・cbrt・hypot など）は `crate::math` だけ（中身は libm）。std の `f32`・`f64`
  の超越関数は clippy で禁止。`sqrt` は std のままでよい
- 浮動小数の `mul_add`・`powi`・`max`・`min`・`clamp`・`to_degrees`・`to_radians`・`midpoint` を使わない（`math` の自前の
  ものを使う。整数の `max`・`min` はよい）。浮動小数の演算の順を決め、並べ替えない。和は決めた順で足す
- `HashMap`・`HashSet`・`RandomState`・`DefaultHasher` を使わない。順序の要る表は `mem` の `SortedVec`・`SortedMap`
  （自前の `lower_bound`）、並べ替えは `mem::stable_sort` だけ。std の `BTreeMap`・`BTreeSet`・`slice::sort*` も使わない
  （std の中で数えずに確保する。ADR-31）。`sort_unstable*`・`select_nth_unstable*`・`binary_search*` を使わない。並べ替えの
  比べ方は全順序（浮動小数は `total_cmp` か整数の鍵）
- std の Unicode の表に依る関数（`char::is_whitespace`・`is_alphabetic`・`to_lowercase`、`str::trim`・`split_whitespace`
  など）を使わない。`is_ascii_*`・`trim_ascii` と、版を固定して生成した自前の表だけを使う
- NaN・±∞ を値に入れない。外から来た数は解析の段で確かめる
- 乱数を使わない（要るなら種つきの ChaCha8 を自作し、決め方を decisions.md に書く）
- 明示の SIMD を書かない。`cfg(target_arch)`・`cfg(target_os)`・`cfg(target_feature)` で処理を分けない
- スレッドを作らない。時刻・環境変数・コマンドの引数・ロケールを読まない
- 範囲の描画のため、画素の値はその画素の位置・木・設定だけで決まるように書く。平坦化の点・線の外形・破線の位相は
  切る矩形に依らずに決める（ADR-30）。走る和は整数で
- 値を変える変更は版（`sumie-render/<N>`）を上げる（determinism.md §8）

### 安全

- `#![forbid(unsafe_code)]`
- 本番のコードで panic しうるものを書かない（一覧は M0 で std の文書の Panics の節と確保する口から機械的に作り、`crates/sumie/clippy.toml`
  に置く。結合テストは各ファイルの頭で要る口を `allow` する）：
  - `[workspace.lints.clippy]` で deny（`lints.workspace = true` で受け継ぐのは `crates/sumie` だけ。道具・testkit・fuzz は自分の
    ゆるい lints を持つ）：`unwrap_used`・`expect_used`・`panic`・`unreachable`・`todo`・`unimplemented`・`exit`・
    `indexing_slicing`（落ちうる添字）・`string_slice`（`&str` の位置での切り出し）・`arithmetic_side_effects`（あふれうる素の
    整数の演算）・`large_stack_arrays`・`large_stack_frames`
  - `disallowed-macros`：`assert!`・`assert_eq!`・`assert_ne!`・`debug_assert*!`（debug だけで panic し、ビルドで振る舞いが
    分かれる）・`vec!`・`format!`（確保）
  - `disallowed-methods`：長さで panic する口（`copy_from_slice`・`clone_from_slice`・`split_at`・`chunks`・`windows`・
    `step_by`・`Vec::remove`・`insert`・`swap_remove`・`drain`・`split_off`・`slice::swap`・`rotate_left`・`rotate_right`、
    `copy_within`・`split_at_mut`・`chunks_exact*`・`rchunks*`・`swap_with_slice`・`Vec::extend_from_within`・`splice`、
    `str::split_at`・`String::truncate`・`insert`・`remove`・`drain`・`replace_range`、`RefCell::borrow*`、`char::to_digit`・
    `from_digit`）は確かめた包みの関数を使う。整数の `pow`・`abs`・`div_euclid`・`rem_euclid`・`div_ceil`・`next_multiple_of`・
    `ilog2`・`ilog10`・`isqrt`・`next_power_of_two`、`Iterator::sum`・`product` は `checked_*` で書く
  - ビルドで分かれるもの：`cfg(debug_assertions)`・`cfg!(debug_assertions)`・ライブラリの `[features]` を持たない（`tests/rules.rs`）
- 再帰しない（`tests/rules.rs` が自分の名前を呼ぶ関数を拾う走査と、レビューの観点で見張る）。木のたどり・CSS のセレクタの照合・参照の展開・フィルタの連鎖・CFF の副手続き・複合の glyf は、明示の
  積み上げで書く
- 確保は `mem` モジュールの口だけ（中で `try_reserve_exact`。捨てるときに `Budget` へ返す。段の結果は `Frozen` に移す）。
  `Vec::with_capacity`・`push` での伸長・`collect`・`to_vec`・`Clone::clone`（すべて。`Arc` の共有は `mem::share`）・
  `Extend::extend`・`extend_from_slice`・`resize`・`String::from`・`Vec::from`・`to_string`・`Box::new`・`Arc::new` などを `mem`
  の外で使わない。`MVec`・`Frozen` に入れる型は `#[repr(C)]` か基本の型だけ。段に入る前に「見積もり ≤ 上限」を確かめ、
  `Budget` の上限は見積もりの値にする。漏れは確保を数える確保器の試験で見つける。見積もりは実際に確保する大きさ・回る数に
  合わせる（キャンバスとの比でなく絶対の量）。手間の式は `cost` に 1 か所だけ書き、見積もりと勘定の両方がそれを呼ぶ
- 長く回るループは `CANCEL_INTERVAL` 未満の塊に分け、塊ごとに回る前に `Budget::spend` で手間を払う（確保のあとの 0 埋めも
  同じ）。中止の印は `spend` の中で決まった間隔ごとに見る
- 外から来たバイト列はバイト列のまま走査する。走査のループは、どの枝でも必ず 1 バイト以上進める
- DTD は内部の一般の実体だけを、展開の量に上限を付けて扱う。外部の実体・パラメタ実体は断る
- フォントも信用しない入力として読む（範囲を確かめる読みの口だけを使う）。字形ごとの上限と組版の上限（当てはめの回数・
  `morx`・`kerx` の歩数・入れ子の lookup の深さ）は版に入る定数。字形ごとの上限を超えた字形は輪郭を空にして警告し、組版の
  上限を超えたらそれ以上当てはめずに続ける（ADR-34）
- 新しいパーサーには、壊れた入力のテスト（種つきの変異）と cargo-fuzz の対象を足す

### フォント

- ライブラリはファイルを開かず、フォントを探さない。呼び手が `FontSet`（`Arc` で共有するバイト列と鍵）を渡す
- OpenType（GSUB・GPOS）と AAT（`morx`・`kerx`・Apple の `kern`・`trak`）の両方の組版を持つ（macOS のシステムのフォントの
  約 2 割（787 書体のうち 165）は AAT だけ。組版の方式の選び方は HarfBuzz と同じ。ADR-29・ADR-36）
- 報告に使ったフォント（鍵と書体の番号）と見つからなかった文字を載せる。文字のある SVG の値は、フォントの集まり全体（鍵と
  順）に依る（代わりのフォントは集まりの順に探す）
- 内部の覚え（キャッシュ）で見積もり・断るかどうか・値が変わらないようにする

### 試験

- 新しいロジックには必ずテストを書く
- 見積もり ≥ 実際（手間・確保）と、ヒープの増え ≤ 数えた量を、照合の集まりと敵対的な入力で確かめる
- ゴールデンは全ターゲットで同じファイルを照らす（フォントは `tests/fonts/` のものだけ）。凍結した版のファイルは追記だけ
- 照合の相手の描いたもの（resvg・Chrome）はゴールデンにしない（差の一覧にするだけ）

## 作業の進め方

- マイルストーン（docs/roadmap.md）ごとに「計画 → 私の承認 → 実装 → 全チェック → 報告」。計画を出したら承認を待つ
- **レビューを 2 回（絶対。ADR-28）**：作業の単位ごとに、まっさらなレビュー用のセッション（この会話を引き継がない新しい
  セッション）を立ててレビューさせ、指摘を直す。これを 2 回くり返す。2 回目で「重大」の指摘が出たとき、または直しが
  作り直しに近い大きさになったとき（明らかに品質が悪いとき）だけ、3 回目を行う。単位は作業する Claude が決める（目安：docs の改訂・
  マイルストーンの計画・意味のまとまった実装の塊）。レビューのセッションは読むだけ（ファイルを変えない）で、指摘を重さ
  つきで返す。次の回も新しいセッションにし、前の回の指摘は渡さない。各回の指摘と、直したか・直さなかった理由を報告に書く
- 仕様の判断は推測せずに質問する（推奨を添える）。決まったら `docs/decisions.md` に追記する
- コミットは小さく、`feat(path): 弧を 3 次曲線に分ける` の形（`feat`・`fix`・`test`・`docs`・`refactor`・`chore`、
  かっこの中はモジュール）。本文は日本語
- 報告では、完了条件を 1 つずつ確かめた結果と、確かめられなかったことを分けて書く

## ドキュメント（必要なときに読む）

| 文書 | 中身 |
|---|---|
| `docs/requirements.md` | 対応する SVG の範囲・上限・非機能・対象外・受け入れの条件 |
| `docs/architecture.md` | 段の分け方・型・公開の口・描画の方式・照合の道具・Kura との境目 |
| `docs/determinism.md` | 決定性と版の規則・ゴールデン |
| `docs/roadmap.md` | マイルストーンと完了条件・依頼文 |
| `docs/decisions.md` | 決定の記録（ADR の表。追記だけ） |

Kura の参考（読むだけ）：`/Users/redjet/aruma/repos/zredjet/kura` の `crates/kura-engine/src/codec/svg.rs`・`svg_text.rs`・
`fonts.rs`（今の resvg の使い方と、外から足している見積もり。攻撃の例が試験にある）、`crates/kura-engine/clippy.toml`、
`.claude/rules/engine.md`、`docs/editing.md` の §8・§9.4・§9.5・§12、`docs/decisions.md` の ADR-136・ADR-164、
`crates/kura-core/src/codec_ext/svg_worker/`、`crates/kura-devtools/src/bench_svg.rs`、`fuzz/`。

## 用語

| 用語 | 意味 |
|---|---|
| D1 | 同じ入力の組（版を含む）なら、対象のどのターゲットでも画素・大きさ・版に入る報告がビット単位で同じ（determinism.md §1） |
| D2 | 同じ入力の組・上限・rev なら、見積もり・断るかどうか・警告が同じ |
| 形 | Kura の言葉で、値を描かずに大きさと受け付けるかを読むこと。`parse` → `estimate_build` → `build` → `Tree::estimate`（ADR-37）。大きさだけなら `parse` → `Document::pixel_size`（大きさを中身で決める文書は木が要る。ADR-58） |
| フォントの集まり | 呼び手が渡すフォント（`FontSet`）。照合の集まりとは別 |
| AAT | Apple の TrueType の組版の表（`morx`・`kerx`・Apple の `kern`・`trak`）。macOS のシステムのフォントの約 2 割（787 書体のうち 165）はこれだけを持つ |
| 読む・木を作る・計画・描く | `parse`（→ `Document`）・`build`（→ `Tree`）・`plan`（→ `Plan`）・`render`・`render_region` の段 |
| 手間 | 決めた数え方の仕事の量。1 単位 ≈ 1 ns（M4 Max・release）。時刻でなく手間で断り、中止を見る |
| 見積もり | 段に入る前に返す、その段の手間と確保の上限（`Estimate`） |
| 上限 | 呼び手が渡す、断る基準（`Limits`） |
| 報告 | 使ったフォント・見つからなかった文字・使った画像のデコーダ・読まなかった参照・警告（`Report`） |
| 文字・字形 | 文字は符号位置、字形はフォントのグリフ |
| 節 | XML と木の節（node）。文書の区切りは「§」で書く |
| 照合の集まり | 見た目を resvg・Chrome と比べる試験の SVG（`tests/svg/`・`corpus/`） |
| ゴールデン | 入力と画素のダイジェストを版つきで凍結した行 |
| 範囲の描画 | 矩形の範囲だけを描くこと（`render_region`）。全体の切り出しとビット単位で同じ |

## やってはいけないこと

- resvg・usvg・tiny-skia などのコードを写す・読みながら書き写す（照合の相手として動かすのはよい）
- 本番の依存を、承認なしに足す
- 凍結した版のゴールデンの既存の行を書き換える・消す
- 値を変える変更を、版を上げずに入れる
- std の超越関数・`HashMap`・`HashSet`・`sort_unstable`・`mul_add`・`unsafe`・再帰を使う
- 試験の集まりを、ライセンスと出どころを確かめずに取り込む
- Kura のリポジトリ（`/Users/redjet/aruma/repos/zredjet/kura`）のファイルを変える・ブランチを切り替える・コミットする・
  そこで cargo を走らせる（ほかのセッションが作業中。読むだけ）
