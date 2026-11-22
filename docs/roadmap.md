# ロードマップ

マイルストーンは上から順に進めます。各マイルストーンの「依頼文」をそのまま Claude Code に貼れば始められます。
完了条件をすべて満たしたら、このファイルのチェックを付けてコミットします。

共通の進め方：

1. 依頼文を貼る → Claude Code が関連の docs を読み、計画（作るファイル・手順・テスト）を出す
2. 計画を確かめて承認 → 実装
3. 作業の単位ごとに、まっさらなレビュー用のセッションでレビュー → 指摘を直す、を 2 回くり返す（絶対。2 回目で「重大」の
   指摘が出たとき、または直しが作り直しに近い大きさになったときだけ 3 回目を行う。ADR-28）
4. `scripts/verify.sh`（全チェック）がすべて通る → 完了条件を 1 つずつ確かめた結果と、各回のレビューの指摘と直し方を
   報告してもらう
5. 仕様の判断が要る所は推測せずに質問し、決まったら `docs/decisions.md` に追記する
6. コミット（小さく。`feat(path): 弧を 3 次曲線に分ける` の形）

どのマイルストーンでも共通の完了条件：新しいロジックにテストがある／見積もり ≥ 実際（手間・確保）が試験で確かめられて
いる／その段の手間の係数を仮に校正して decisions.md に記録した（「時間 ≤ 手間 × 2 ns」はこの仮の係数で確かめる。M7 で全体を
校正し直す）／新しいパーサーにファズの対象と種つきの変異のテストがある／`cargo tree -e normal,build -p sumie` が `libm` だけ。

---

## M0　土台

- [x] `git init`・workspace（`crates/sumie`・`crates/sumie-testkit`・`crates/sumie-tools`・`fuzz/`。`[workspace.lints]` を受け継ぐのは
  `crates/sumie` だけ）・`rust-toolchain.toml`・`publish = false`・
  ライセンスのファイル・`.gitignore`
- [x] lint：`#![forbid(unsafe_code)]`、`[workspace.lints]`（CLAUDE.md の一覧）、`crates/sumie/clippy.toml`（厳しい規則。道具には
  別のゆるい設定）。禁じる口の一覧は、std の文書の Panics の節と確保する口から機械的に作る。`disallowed-macros` で `assert!`・
  `debug_assert!` を拾えるかを試験で確かめ、拾えなければ `tests/rules.rs` の走査に回す。結合テストの頭で `allow` する口を決める
- [x] `tests/rules.rs`（src の走査：`cfg(target_*)`・`std::arch`・`cfg(debug_assertions)`・`[features]`・`mul_add`・`mem` の外での
  確保・`MVec` に入れる型が `repr(C)` か基本の型か・モジュールの層の向き・自分の名前を呼ぶ関数（再帰））。禁じる口の一覧の
  観点に、LLVM に並べ替えを許す浮動小数の口（`algebraic_*` など。1.99.0 で stable かを確かめる）と `std::process::abort` も入れる
- [x] `crates/sumie-testkit`（自前の ChaCha8・種つきの変異・照合の集まりの読み込み・確保を数える確保器の包み）
- [x] 基礎のモジュールの骨組み：`math`（libm の包み・`max`・`min`・`clamp`・`powi`）・`budget`・`cost`・`mem`（`MVec`・
  `SortedVec`・`SortedMap`・`stable_sort`。捨てるときに `Budget` へ返す）・`Cancel`・`Error`
- [x] 確保を数える確保器（dev 依存のクレートを選ぶ）と、ヒープの増え ≤ `Budget` の数えた量を確かめる試験の仕組み（ADR-31。
  libtest の雑音を避けるため `harness = false` で 1 本ずつ回すか、スレッドごとに数える確保器を選ぶ）
- [x] ゴールデンの仕組み（形式・BLAKE3・`scripts/check-goldens-append-only.sh`）
- [x] ファズの仕組み（`fuzz/` の独立の workspace と最初の対象の雛形）と、種つきの変異のテストの道具（自前の ChaCha8）
- [x] 小さいスタック（128 KiB）のスレッドで試験を回す道具
- [x] `sumie-tools`：`oracle resvg`・`oracle chrome`・`compare`・`bench`・`corpus`・`gen-heavy` の骨組み
- [x] 試験の集まりの取り込み（ライセンスを確かめ、写しを許すものだけを出どころ・版・ライセンスの写しとともに。取り込む前に一覧を見せて承認を得る。ADR-21）
- [x] CI（GitHub Actions：`ubuntu-latest`・`ubuntu-24.04-arm`・`macos-15`（arm64）。非公開のリポジトリで arm の Linux が使えるかを確かめる。ADR-22）と
  手元の全チェック `scripts/verify.sh`。→ リポジトリを公開にして 3 つのターゲットで回した（ADR-54）。M0 の終わりの
  ed8c3d6 で、CI の回 45017566008 の 5 つのジョブ（3 つのターゲットの試験・checks・fuzz）が通り、手元の `scripts/verify.sh`
  の全段（release・native・x86_64・ファズ）も 22b5fbb で通った
- [x] Kura と同じ resvg（`raster-images` なし）で、sbix の絵文字（Apple Color Emoji）がどう描かれるか（代わりのフォントとして
  選ばれるか・仮の輪郭を描くか）を確かめ、絵文字の書体の数え方をユーザーに決めてもらう（ADR-14 (5)・ADR-40）。→ resvg は
  Apple Color Emoji を選んで何も描かない。(a) 字ごとに数える（ADR-56 (2)）
- [x] 非公開のリポジトリで `ubuntu-24.04-arm` が使えるか・含まれる分の時間で足りるかを確かめ、足りないときの手（QEMU の上で
  回す・公開にする・有料の分）を相談する（ADR-22・ADR-35）。→ 公開のリポジトリにした（ADR-54）
- [x] 取り込みの前提を確かめる：同じ package の 2 つの git の rev を別名の依存で入れた小さなクレートが Cargo で組めるか・
  Kura の CI が非公開のリポジトリの git の依存を取れるか（鍵の置き方）（ADR-18・ADR-35）。→ 別名の依存で組めた。公開に
  したので鍵は要らない（ADR-53・ADR-54）
- [x] `gen-heavy` の入力（地図風の線 30 万区間・地図風の形・遅い入力 S1〜S5・S7・互換の例 S6・展開の爆弾）の引数を決めて
  architecture.md §10 に追記し、
  resvg 単体の時間・確保のピークを測る（ADR-19・ADR-35）。→ architecture.md §10（ADR-57 (3)・ADR-58 (2)）
- [x] Kura の側に、照合の集まりを Kura の今の `SvgCodec` に通した「断る／描く」の一覧を作ってもらうよう、ユーザーに頼む
  （働き手を通さず、プロセスの中の `SvgCodec` で。このリポジトリからは Kura で cargo を走らせない。一覧は Kura の rev つきで
  `corpus/` の中に置く。NFR-10。依頼文は `corpus/kura-request.md`。M0 の終わりの時点で一覧は未着。のちに依頼を取り下げ、
  NFR-10 は Kura の設定の resvg と照らす（ADR-63 (1)））。Kura の手元の SVG（Kura の働き手の実測に使った 1,439 件）の置き場所をユーザーに聞き、git の
  外で Kura の `SvgCodec` と sumie の両方に通して「描く／断る」と大きさの一致を照らす仕組みを作る（結果の一覧だけを残す。
  ライセンスでコミットできないため）。→ 仕組みは `kura-compat` で作った。1,439 件は消えていて使えないので、代わりに実例の
  SVG 1,241 件を `corpus/real/` に取り込んだ（ADR-58 (3)・ADR-59）
- [x] `oracle resvg` は Kura と同じ usvg の設定（総称の書体 Times・Helvetica・Apple Chancery・Papyrus・Courier、フォントを名前の
  順に登録、languages、`default_size`）で描く。`oracle chrome` の透明の背景（`--default-background-color=00000000` など）と窓の
  大きさを決めて architecture.md §10 に追記する。→ Kura の Cargo.lock に依存の版も揃えた（ADR-52・ADR-57・ADR-60）

**完了条件**：`scripts/verify.sh` が通る／Linux でも `cargo test` が通る／`cargo tree -e normal,build -p sumie` が `libm` だけ／
照合の集まりを resvg と Chrome で描いて、差の一覧の HTML が出る／resvg と Chrome の差の分布を測り、ADR-15 の基準の値を
確定して decisions.md に追記する（ADR-15 の「全体の 0.5%」の分母を、キャンバスの全画素と「どちらかの画像で α > 0 の画素」の
両方で測って決める）／集まりと重い入力での resvg 単体の時間・確保のピークを記録する／確保を数える確保器の試験が
動く／CI の 3 つのターゲット（または相談して決めた代わり）で試験が回る

**依頼文**
> CLAUDE.md と docs/ をすべて読んで、docs/roadmap.md の M0 を実装してください。最初に計画（作るファイルと手順）を
> 出して、私の承認を待ってください。試験の集まりは、取り込む前にライセンスと出どころを一覧にして見せてください。

---

## M1　XML・CSS・文書

- [x] `xml`：字句・節の配列・名前空間・文字の参照・DTD の内部の実体（上限つき）・`xml:space`
- [x] `css`：字句・規則・セレクタ・詳細度・`!important`・照合（行きがけの順に流す部分ごとの旗。手間の勘定。右端の索引）
- [x] `values`：数・長さ・色・paint・変形・角度・並び・`viewBox`・`preserveAspectRatio`（パスのデータは M2）
- [x] `style`：プロパティの表・カスケード・継承・`currentColor`・em・ex
- [x] `doc`：ID の表・参照（保守的な辺。受け継ぐ参照と use の文脈）・輪・展開の数・`switch`・`tref`・大きさ
- [x] `estimate_parse`（読む段と同じ道を走らせて量を測る。ADR-70 で「本文を 1 度だけ字句に分けて数える」から改めた）・`estimate_build`（文字を除く）・`Report` の骨組み
- [x] NFR-10 の照らし（ADR-63 (1)）：照合の集まりで、Kura の設定の resvg が読めるもの（大きさが出るもの）を sumie の `parse` が
  断らないこと・大きさが同じこと（`sumie-tools sizes`）。外れるものは一覧にして承認を得る
- [x] 大きさを中身で決める文書（ADR-58 (1)）：`Document::size`・`pixel_size` の型（「木が要る」の返し方）と、外接矩形の細かい
  規則（線の太さを含まない・片方だけある幅と高さ・%）を resvg と照らして決める（例：resvg の試験の `structure/svg/no-size`
  は 199 × 199・`tests/svg/m0/no-size.svg` は 50 × 50・W3C の `struct-frag-01`・`04`）
- [x] M1 で resvg と照らして決める項目を確かめ、decisions.md に追記する：同じ id の扱い・`requiredFeatures`・輪の参照の扱い・
  大きさの端（0・負・1 未満・%）・CSS のセレクタとアットルールの範囲・XML の整形式の厳しさ（roxmltree が受けるもの）・ATTLIST の
  既定の属性値を当てないこと（XML 1.0 §5.1 と違うが roxmltree と同じ）・`Document::needs` の `<image>` の判定を usvg が実際に解いた `<image>` と照らす（ADR-63 (1)）
- [x] ファズの対象：`xml`・`dtd`・`css`・`values`・`document`
- [x] `sumie-tools` に計算した値の照合（usvg 0.48.1 の木と、節の数・変形・塗り・不透明度などを比べる）
- [x] CSS の `@scope` を当てるマイルストーンを決めて、この表に足す（M1 では飛ばして警告する。2026-11-23、ユーザーの判断
  「後のマイルストーンで」。ADR-78 (3)。M7 に足した。ADR-83 (3)）
- [x] CSS の `calc()`・`var()`（カスタムプロパティ）を読むマイルストーンを決めて、この表に足す（M1 では壊れた値として警告する。
  2026-11-22、ユーザーの判断「後のマイルストーンで」。ADR-74。M7 に足した。ADR-83 (3)）

**完了条件**：集まりのすべてで大きさが resvg と同じ（FR-OUT-02）／計算した値の照合の差の一覧が承認済み／Kura の
`svg.rs` の試験にある攻撃の例（実体の爆弾・深さ 10 万段・CSS の組み合わせの爆発・8 万の宣言・参照の輪・巨大な数・
DTD のまわりの多バイト文字・英数字と記号の 15 字のすべての 3 文字の並び（数の走査が終わること））を通す（断るか、上限の内で読む）／128 KiB のスタックの試験／
見積もり ≥ 実際／ヒープの増え ≤ 数えた量／ファズを対象ごとに 1 時間

**依頼文**
> docs/roadmap.md の M1 を実装してください。最初に計画を出して承認を待ってください。CSS のセレクタと SVG 2 の
> プレゼンテーション属性の最終の範囲は、resvg と照らした結果を見せてから決めてください。

---

## M2　パスの幾何とラスタライズ

- [ ] パスのデータの解析・弧（中心の形への変換）・基本の図形をパスに・変形
- [ ] 木を作る段（`build`）は use の x・y を CSS のプロパティとしても読む（M1 の walk は属性だけを読む。ADR-84 (6)）
- [ ] 平坦化（分ける数を曲線と変形と誤差だけで先に決める。線の太さを入れるかを決める）・決まった箱での切り取り・箱の外の片側の
  曲線を平坦化しない・セルを線分の元の両端から閉じた式で求める・範囲の左の外の分を cover だけ足す（ADR-30・ADR-38）
- [ ] 線の外形（端・継ぎ目・`miterlimit`）・破線・`non-scaling-stroke`
- [ ] ラスタライズ（面積の被覆・非ゼロ・偶奇・`crispEdges`）
- [ ] 単色の塗りと線・source-over の合成・出力のバッファ
- [ ] 計画（`Tree::plan` → `Plan`。平坦化と線の外形は計画で 1 度）・`render`・`estimate`（ここまでの機能。ADR-38）
- [ ] 範囲の描画（`Plan::render_region`・`estimate_region`。範囲と交わらないパスは平坦化の前の外接矩形で落とす）と、全体の
  切り出しとの一致の性質のテスト（ADR-13）。タイルで描いたときの合計の時間を測る
- [ ] ゴールデン（版 0）をターゲットをまたいで照らし始める（版 0 は作り直してよい。ADR-20）
- [ ] ファズの対象：`path`・`render`（図形）

**完了条件**：図形と単色だけの集まりで ADR-15 の基準を満たす／ゴールデンが 3 つのターゲットで同じ／release・debug・
`target-cpu=native` で同じ／範囲の描画の一致（境目をまたぐ曲線・破線・太い線・巨大な図形を含む）／Kura の S1（回転の下の
太い線）・S4（細かい破線の丸い端）を見積もりの内で描くか描く前に断り、時間 ≤ 手間 × 2 ns／地図風の線 30 万区間の時間を
記録する（目標は ADR-19）

**依頼文**
> docs/roadmap.md の M2 を実装してください。最初に計画を出して承認を待ってください。ラスタライズの方式と
> 平坦化の誤差は、resvg との差の例を見せてから確定してください。

---

## M3　塗り・合成・marker

- [ ] `linearGradient`・`radialGradient`（焦点・`fr`）・`spreadMethod`・`href` の鎖
- [ ] `pattern`（入れ子・単位・変形・`viewBox`）
- [ ] 層の計画（積み上げの最大）・描く途中で確保しない
- [ ] グループの `opacity`・`clipPath`・`mask`（輝度・アルファ）・`mix-blend-mode`・`isolation`
- [ ] `marker`（`orient`・単位・`viewBox`・`context-fill`・`context-stroke`）
- [ ] 画像のデコーダを注入する口（`ImageDecoder`・`data:` の URL・決定的な拡大縮小。ADR-16）

**完了条件**：ここまでの機能の集まりで基準を満たす／Kura の pattern・mask の爆弾（巨大なタイル・objectBoundingBox の
入れ子 24 段・曲線をタイルいっぱいに 3,000 回描き直す形・0.0001 の破線 1,000 本）を描く前に断るか、上限の内で描く／Kura の
S2（stop の多いグラデーション）・S3（同じ offset の stop）・S5（回転した pattern）を見積もりの内で描くか描く前に断り、
時間 ≤ 手間 × 2 ns／範囲の描画の一致／見積もり ≥ 実際

**依頼文**
> docs/roadmap.md の M3 を実装してください。最初に計画を出して承認を待ってください。グラデーションの補間と
> pattern の標本の取り方は、resvg と Chrome との差を見せてから決めてください。

---

## M4　フィルタ

- [ ] filter の領域・部分領域・`primitiveUnits`・`in`・`result`・`color-interpolation-filters`
- [ ] プリミティブのすべて（FR-FI-01）と CSS の filter 関数
- [ ] 範囲の描画の reach

**完了条件**：フィルタの集まりで基準を満たす／Kura の資源の爆弾（feFlood × 2 万・半径 10⁶ の feMorphology・
numOctaves 4e9 の feTurbulence・1e30 のぼかし・order 200 の feConvolveMatrix）を描く前に断るか、上限の内で描く／Kura の S7（オクターブの多い
feTurbulence）を見積もりの内で描くか描く前に断り、時間 ≤ 手間 × 2 ns／範囲の描画の一致

**依頼文**
> docs/roadmap.md の M4 を実装してください。最初に計画を出して承認を待ってください。

---

## M5　フォントの読み取りと文字の組版

- [ ] 始める前に、resvg 0.48.1 が可変フォントの軸（`wght`・`wdth`）を動かすかを確かめ、decisions.md に追記する（ADR-14 (4)）
- [ ] sfnt・cmap（形式 0・4・6・12・13・14）・hmtx・name・OS/2・post・glyf・loca・CFF
- [ ] OpenType の組版：GDEF・GSUB・GPOS・OpenType の `kern`
- [ ] AAT の組版（ラテン文字・和文の分）：`morx` の非文脈・合字・文脈の副表・`kerx`（`ankr`）・Apple の形の `kern`・`trak`。
  状態機械の歩数の上限（ADR-29）
- [ ] 字形ごとの上限と組版の上限（版に入る定数）。字形ごとの上限を超えた字形・壊れた字形は輪郭を空にして警告、組版の上限を
  超えたらそれ以上当てはめずに続ける（ADR-34）
- [ ] 試験用の AAT のフォントを作る（`gen-fonts`：写しを許すフォントに fontTools で `morx`・`kerx`・Apple の `kern`・`trak` を足す。
  作り方と出どころを `tests/fonts/` の一覧に書く。AAT の方式をすべてのターゲットのゴールデンで照らすため）
- [ ] `FontSet`（`Arc` で共有するバイト列・索引・`FontLimits` と中止・選び方・総称の書体・代わりの並び・輪郭の表のない書体を
  数えない）・`FontKey`・報告
- [ ] 組版（addressable character への位置の割り当て・塊・`text-anchor`・spacing・`textLength`・組版の方式の選び方（OpenType・
  AAT。ADR-36）・当てはめと上限・入れ子の lookup の積み上げ）・組版の手間の見積もり（`FontSet` の索引の組版の係数）
- [ ] 和文の横書きとラテン系（FR-TX-09・10）
- [ ] Unicode の表の生成（`gen-unicode`）
- [ ] ファズの対象：フォントの各表・CFF・`morx`・`kerx`・組版

**完了条件**：文字の集まりで基準を満たす（試験用のフォント（AAT のものを含む）と、手元の macOS のシステムのフォントの両方。
AAT だけの Times・Helvetica・Apple Chancery のカーニング、Courier などの `morx` の合字、SF・New York の `trak` を含む）／Kura の文字の
攻撃の例（6,000 字の塊・100 字の text × 400・1,000 字の text を use で 400 回・1 字の tspan × 1,000・1,000 字の text を
pattern・clipPath・mask の中身にして 3,000 回参照）を上限の内で描くか断る／壊れたフォントで panic しない／フォントのファズを各 1 時間

**依頼文**
> docs/roadmap.md の M5 を実装してください。最初に計画を出して承認を待ってください。見つからない字の描き方と
> 代わりのフォントの順は、resvg と Chrome の例を見せてから決めてください。

---

## M6　文字の残り

- [ ] `textPath`（`startOffset`・弧長の精度は倍率に依らない量で決める：根のユーザー単位に、参照先の path 自身と祖先の変形の
  拡大を掛けた座標系での誤差。木を作る段は倍率を知らないため。ADR-43）
- [ ] `text-decoration`・baseline の類
- [ ] 双方向（UAX #9）・ヘブライ文字・アラビア文字の接続（ADR-14 (2)）。OpenType の書体と、AAT の書体（GeezaPro・Raanana
  など。`morx` の並べ替え・挿入の副表を足す。ADR-29）の両方
- [ ] 縦書き（重ければ v1.x へ回すことを相談する。ADR-14 (3)）
- [ ] 可変フォント（M5 の前の確かめで入れると決めたときだけ。ADR-14 (4)）

**完了条件**：文字の集まりの残りで基準を満たす（GeezaPro のアラビア文字の接続を含む）／Kura の textPath の攻撃の例（拡大 10¹⁸・
区間 2 万に 5,000 字・相対の命令の積み重ね）を見積もりの内で描くか断る（時間 ≤ 手間 × 2 ns）／Kura の互換の例 S6（区切りの
ない数の transform を持つ path へのふつうの textPath）をかならず描く

**依頼文**
> docs/roadmap.md の M6 を実装してください。最初に計画を出して承認を待ってください。

---

## M7　Kura に入れる仕上げ

- [ ] 手間の係数の校正のし直し（1 単位 ≈ 1 ns。各マイルストーンの仮の校正をまとめ直す）と上限の既定値の校正
- [ ] NFR-10・A-02 を、Kura の設定の resvg と照らす（照合の集まりと実例の SVG。Kura の側の一覧は取り下げた。ADR-58 (3)・ADR-59・
  ADR-63 (1)）
- [ ] 版の凍結（`sumie-render/1`）とゴールデンの凍結（以後は追記だけ）
- [ ] 性能の計測と目標の確認（ADR-19）
- [ ] resvg との差の記録（`docs/differences.md`）
- [ ] CSS の `calc()`・`var()`（カスタムプロパティ）と `@scope` を読む・当てる（M1 では壊れた値・飛ばすアットルールとして警告
  する。2026-11-23、ユーザーの判断。ADR-83 (3)）
- [ ] ファズ：全部の対象の時間の合計で 24 時間以上（文書の全体を描く対象は、そのうち 8 時間以上）
- [ ] Kura に渡す口の確定と、Kura の側の変更の案（`docs/kura-integration.md`：Kura の今の SVG の呼び出しの一覧と、そのすべての
  置き換え先の口。Kura のリポジトリには書かない）

**完了条件**：requirements.md の A-01〜A-09

**依頼文**
> docs/roadmap.md の M7 を実装してください。最初に計画を出して承認を待ってください。版を凍結する前に、resvg との
> 差の一覧を見せてください。
