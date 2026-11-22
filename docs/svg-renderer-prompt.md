# SVG の描画器（仮称）— 立ち上げの依頼

> 新しい空のリポジトリ（`git init` 済み）で Claude Code を起動し、この文書をまるごと最初の依頼として貼る。
> `<…>` の所は貼る前に埋めるか、そのまま貼って最初の質問で決める。

---

## あなたへの依頼

このリポジトリで、SVG を読んで画素に描く Rust のライブラリを一から作ります。

**最初のこのセッションでは実装をしません。** 次の 3 つを作って、私の承認を待ってください。

1. `CLAUDE.md`：このプロジェクトの原則・規則・コマンド（下の「守ること」を写して整える）
2. `docs/`
   - `requirements.md`：対応する SVG の範囲と対象外
   - `architecture.md`：段の分け方・型・公開の口
   - `determinism.md`：決定性と版の規則
   - `roadmap.md`：マイルストーンと完了条件
   - `decisions.md`：決定の記録（ADR の表。決まったことはここに追記する）
3. 最初に決めてほしいことの質問の一覧（それぞれ推奨つき。下の「最初に決めること」に足りないものがあれば足す）

返答・文書・コミットのメッセージはすべて日本語で書いてください。

---

## 背景

私は Kura という Mac 用のメディアライブラリ（Tauri 2 ＋ Rust）を作っています。Kura の編集のエンジン（`kura-engine`）は、
**同じ文書・同じ元なら、どの環境でも同じ値**（R1）を約束しています。そのために、超越関数は `libm` クレート（`=0.2.16`）
だけ、`HashMap` 禁止、`unsafe` 禁止、などの規則で書いています。

その中で SVG だけは resvg 0.48（usvg・tiny-skia・harfrust・fontdb・roxmltree など 61 クレート）で読んでいて、次の問題を
Kura の側で無理に囲っています。

| 問題 | Kura が今していること |
|---|---|
| 中で std の三角関数（OS の libm）を使うので、値が OS の版や CPU で変わりうる | R1 を約束できず、読むたびに「アンカー」（読んだ値の可逆の写し）を残す |
| 描く途中で止められない（中止の口がない） | アプリ自身を別のプロセスで起こして SVG を読ませ、形 2 秒・値 10 秒で子を殺す |
| 入れ子の深さだけ再帰する（スタックのあふれは受け止められない） | XML を渡す前に生のバイト列で深さを数え、参照の展開を含めて 64 段で断る |
| 手間と確保の見積もりがない | 約 3,000 行の見積もりを外から書いて断る（DTD の実体・`<use>` の展開・marker の複製・文字の組版の 2 乗の手間・textPath の精度・破線の数・pattern の参照…） |

**ゴール**：これらを内側で解決した SVG の描画器を作り、Kura が resvg・別のプロセス・外の見積もり・アンカーをやめられる
ようにすること。

---

## ゴールと対象外

### ゴール

- 静的な SVG（SVG 1.1 と、SVG 2 のうち resvg が対応している範囲）を読み、RGBA8（乗算済み）で描く
- **値の再現**：同じ入力・同じフォントのバイト列・同じ設定なら、aarch64-apple-darwin・x86_64・aarch64 の Linux の
  どれでも、画素がビット単位で同じ
- **安全**：どんな入力でも、panic・スタックのあふれ・終わらないループ・上限を超える確保をしない。途中で止められる。
  手間と確保を前もって見積もれる

### 対象外（v1）

- アニメーション（SMIL）・スクリプト・操作への反応
- 外部の資源（ファイル・http）の読み込み（参照は「読まなかった」と報告するだけ）
- SVG の書き出し・編集の API
- `<image>` のラスター画像の展開（v1 は断る。呼び手がデコードを注入する口を作るかは質問）

---

## 守ること（Kura に組み込むための条件）

### 決定性

- 超越関数（sin・cos・tan・atan2・exp・ln・pow・cbrt など）は `libm = "=0.2.16"`（`default-features = false`）だけで
  求める。std の f32・f64 の超越関数は clippy の `disallowed-methods` で禁止する。`sqrt` は IEEE 754 で正しく丸めると
  決まっているので std のままでよい
- `mul_add` を使わない。浮動小数の演算の順を決めて、並べ替えない
- `HashMap`・`HashSet` を使わない（`BTreeMap` か並べた `Vec`）。`sort_unstable*`・`binary_search*` も使わない
- 浮動小数の max・min は ±0 と NaN の扱いを決めた自前の関数にする
- 乱数は使わない（要るなら種つきの ChaCha8）
- SIMD は値が変わらない形（整数）だけ。迷ったら使わない。`target_feature` で枝を分けない
- 並列：ライブラリ自身はスレッドを作らない。並べて描くなら、結果がスレッドの数・分け方に依存しない形だけにする
  （Kura は自分のスレッドのプールでタイルを並べて描く。「矩形の範囲を描く」口を、全体を描いて切り出したものと
  ビット単位で同じにするかは質問）
- 時刻・環境変数・ロケールを読まない
- 出力に効くものには版を持つ（描画器の識別子 `<名前>-render/1` など）。値を変える変更は版を上げる。
  ゴールデン（入力と画素のダイジェスト）は追記だけで、既存の行を書き換えない

### 安全

- `unsafe` は `forbid`
- 本番のコードで `unwrap`・`expect`・`panic!`・落ちうる添字を書かない（clippy で強制）
- 再帰しない：木のたどり・CSS のセレクタの照合・参照の展開・フィルタの連鎖は、明示の積み上げで書く
- 確保は `try_reserve_exact` で行い、上限（`Limits`：入力のバイト数・要素の数・展開の数・キャンバスの画素・
  一時の画像のバイト数・手間）を先に見積もって断る。見積もりは実際に確保する大きさ・回る数に合わせる
  （キャンバスとの比でなく絶対の予算）
- 中止：呼び手が渡す中止の印を、決まった手間ごとに確かめる。止めたら `Cancelled` を返す
- 手間の数え方を決め、木を作る前と描く前に見積もりを返す口を持つ（Kura はそれで予算を確保する）
- DTD は内部の実体だけを、展開の大きさに上限を付けて扱う。外部の実体・パラメタ実体は断る
- 外から来たバイト列を `&str` の位置で切らない（多バイト文字の途中で panic する）。走査のループは、どの枝でも必ず
  1 バイト以上進める
- パーサーごとに、壊れた入力のテスト（種つきの変異）と cargo-fuzz の対象を足す

### 依存

- 本番の依存は `libm` だけを目標にする（XML・CSS・パスの解析・ラスタライズ・フォントの読み取り・文字の組版まで自作）。
  ほかを足したいときは、理由を書いて私に聞く
- **resvg・usvg・tiny-skia などのコードは写さない（完全な自作）**。W3C の SVG 1.1 / SVG 2・CSS・OpenType・
  Unicode（UAX #9 など）の仕様から書く。resvg は dev 依存の照合の相手として使ってよい（版を固定する）

### フォント

- ライブラリはファイルを開かず、フォントを探さない。呼び手がフォントのバイト列の集まりを渡す（Kura は macOS の
  システムのフォントを mmap して渡す）
- 描いた結果と一緒に、使ったフォント（バイト列のダイジェスト）と見つからなかった字を報告する（Kura は文字のある
  SVG の値の再現を、フォントのダイジェストで約束する）
- 和文の横書きは必須。そのほかの組版（縦書き・アラビア文字・インド系の文字・カラーの絵文字）を v1 にどこまで入れるかは
  質問

### 環境

- Rust の edition 2024・stable
- OS に依存しない（Linux でも `cargo test` が通る）

---

## 対応の範囲の目安（requirements.md で確定する）

Kura が今 resvg で読めているものを、v1 で落とさないことが条件です。

- **構造**：svg（入れ子・viewBox・preserveAspectRatio）・g・defs・use・symbol・switch、style 要素と style 属性
  （CSS のセレクタの範囲は決める）、プレゼンテーション属性、継承、currentColor
- **図形**：path・rect（角丸）・circle・ellipse・line・polyline・polygon
- **塗りと線**：単色・linearGradient・radialGradient（焦点つき）・pattern、fill-rule、線の幅・端・継ぎ目・miterlimit・
  破線、opacity・fill-opacity・stroke-opacity、marker
- **合成**：clipPath・mask（輝度・アルファ）・グループの opacity・mix-blend-mode・isolation
- **フィルタ**：feGaussianBlur・feOffset・feFlood・feBlend・feComposite・feMerge・feColorMatrix・feComponentTransfer・
  feMorphology・feConvolveMatrix・feTurbulence・feDisplacementMap・feTile・feImage（要素への参照）・feDropShadow・
  照明（feDiffuseLighting・feSpecularLighting）。filter の領域、color-interpolation-filters（linearRGB）
- **文字**：text・tspan・textPath、x・y・dx・dy・rotate、text-anchor、font-family・size・weight・style・stretch、
  letter-spacing・word-spacing、textLength・lengthAdjust、文字の装飾、総称の書体の割り当て、代わりのフォント
- **出力**：幅・高さ（ユーザー単位、96 dpi）を切り上げた画素。倍率の指定

---

## 公開の口の形（たたき台。architecture.md で確定する）

```text
parse(svg バイト列, 読みの設定（上限・フォントの集まり）, 中止の印) -> Result<Tree, Error>
Tree::size()                     -> 描く大きさ（倍率 1 の画素）
Tree::estimate(倍率)             -> 描く前の見積もり（キャンバス・一時の画像のバイト数・手間）
Tree::report()                   -> 使ったフォントのダイジェスト・読まなかった参照・警告
render(&Tree, 描きの設定（倍率・上限）, &mut 出力の画素, 中止の印) -> Result<(), Error>
render_region(&Tree, 矩形, …)     -> 範囲を描く（入れるかは質問）
Error = Limit | Cancelled | Unsupported | Invalid
```

---

## 照合と試験

- 照合の相手
  - resvg（dev 依存。版を固定）
  - Chrome のヘッドレスのスクリーンショット（`/Applications/Google Chrome.app` が入っている）
  - SVG の試験の集まり（resvg のリポジトリの試験・W3C の SVG 1.1 の試験など。**取り込む前にライセンスを確かめ、
    出どころを書く**）
- 一致の基準（反エイリアスの違いをどこまで許すか）は質問で決める
- ゴールデン：入力ごとに、画素のダイジェストを版つきで凍結する（追記だけ）。どのターゲットでも同じ行を照らす
- 性能の目安：resvg と同じ入力で比べる（目標の倍率は質問）。Kura の重い例は「地図風の線 30 万区間」で、resvg は約 4.5 秒

---

## 進め方

- Kura と同じく、docs が正。マイルストーンごとに「計画 → 私の承認 → 実装 → 全チェック → 報告」
- 仕様の判断は推測せず質問する。決まったら `docs/decisions.md` に追記する
- 新しいロジックには必ずテストを書く
- コミットは小さく、`feat(path): 弧を 3 次曲線に分ける` の形で書く
- マイルストーンの目安（roadmap.md で確定する）
  - M0 土台：workspace・clippy の規則・ゴールデンとファズの仕組み・照合の道具（resvg と Chrome で描いて差を測る）
  - M1 XML（DTD の内部の実体まで）・CSS・プレゼンテーション属性の解析と、参照を解決した木
  - M2 パスの幾何（解析・変形・弧・平坦化・線の外形・破線）とラスタライズ（面積の被覆の反エイリアス、非ゼロ・偶奇）
  - M3 塗り（単色・グラデーション・pattern）と合成（opacity・clip・mask・blend）、marker
  - M4 フィルタ
  - M5 フォントの読み取り（glyf・CFF・cmap・hmtx・kern と GPOS・GSUB の一部）と文字の組版・代わりのフォント
  - M6 文字の残り（textPath・装飾・入れるなら縦書き・双方向）
  - M7 Kura に入れる仕上げ：版の凍結・ゴールデン・性能の計測・resvg との差の記録・Kura に渡す口の確定

---

## 最初に決めること（質問の一覧に入れる。推奨つきで）

1. クレートの名前とライセンス（Kura は MIT OR Apache-2.0）
2. 範囲の描画（タイル）を v1 に入れるか
3. 文字の組版の範囲（和文の横書き・ラテン文字のカーニングと合字まで？ 縦書き・アラビア文字・インド系の文字・
   カラーの絵文字（sbix・COLR）は？）
4. 反エイリアスの方式と、resvg・Chrome との一致の基準
5. `<image>` の扱い（v1 は断る／呼び手がデコードを注入する口を作る）
6. 色の扱い（sRGB に固定・フィルタの linearRGB・ICC は無視、でよいか）
7. Kura がどう取り込むか（git の rev を固定した依存／パスの依存／Kura のリポジトリに写す）
8. 性能の目標（resvg の何倍以内か）

---

## Kura の側で参考になるもの（読むだけ。書き込まない）

Kura のリポジトリ：`/Users/redjet/aruma/repos/zredjet/kura`（ほかのセッションが作業中なので、ファイルを変えない・
ブランチを切り替えない・コミットしない）

- `crates/kura-engine/src/codec/svg.rs`・`svg_text.rs`・`fonts.rs`：今の resvg の使い方と、外から足している見積もり
  （何を断る必要があったか。そのまま要件の一覧になる）
- `crates/kura-engine/clippy.toml`・`.claude/rules/engine.md`：決定性の規則
- `docs/editing.md` の §8（決定性と版）・§9.4（アンカー）・§9.5（画像のコーデックの約束。SVG の項と、その下の
  「文字のレイアウトの上限」「別のプロセスで読む」）
- `docs/decisions.md` の ADR-136・ADR-164（別のプロセスの働き手）
- `crates/kura-core/src/codec_ext/svg_worker/`：別のプロセスで読む仕組み（v1 でやめられるようにしたいもの）
- `crates/kura-devtools/src/bench_svg.rs`：SVG の計測
- `fuzz/fuzz_targets/decode_svg.rs`・`decode_svg_text.rs` と `fuzz/corpus/decode_svg/`（約 8,000 件の種。照合と
  ファズの種に使ってよい）
