//! 手間の式（architecture.md §5.1・§5.2）。見積もりと実際の勘定の両方がここの関数を呼ぶ（式が食い違わない）。
//! 見積もりは「これから回る数の上限」、勘定は「実際に回った数」を同じ式に入れる。
//!
//! 係数は M0 の仮の校正（M4 Max・release。`mem::tests::calibrate`。decisions.md ADR-47）。1 単位 ≈ 1 ns を目安にし、
//! M7 で全体を校正し直す。係数を変えても画素（D1）は変わらない。見積もりは変わるが、見積もりの約束（D2）は rev ごと
//! なので版は上げない（determinism.md §1.1・§8.2）。
//!
//! 見積もりは、操作ごとの式（[`append`]・[`insert`]・[`sorted_insert`] など）を操作の数だけ足して求める（実際の勘定も
//! 操作ごとに同じ式で払う。量をまとめてから 1 度に式に入れると、切り上げの分だけ実際より小さくなる）。

/// 0 埋めの 1 単位あたりのバイト数（新しく取った領域に初めて触れる分を含む）。
pub(crate) const FILL_BYTES_PER_UNIT: u64 = 16;
/// 写し（入れ物を伸ばすときの写し・`extend_from_slice`）の 1 単位あたりのバイト数。
pub(crate) const COPY_BYTES_PER_UNIT: u64 = 16;
/// 安定な併合の並べ替えの、1 つの要素を 1 段で比べて写す手間。
pub(crate) const MERGE_STEP: u64 = 4;
/// パスのデータ・`points` の入力 1 バイトあたりに、幾何の置き場（命令 1 バイト・座標 8 バイト）へ書く量の上限（`values::path`
/// の注。最も多いのは `zh1` の繰り返しの 3 バイトで 35 バイト）。
#[allow(dead_code, reason = "木の見積もり（M2 の tree）から使う")]
pub(crate) const PATH_GEOMETRY_BYTES: u64 = 12;
/// 平坦化で点を 1 つ求める手間（曲線の式を解いて点を渡す。architecture.md §5.2 の「計画」。M2 の仮の校正：M2 の集まりで
/// 計画の時間 ÷ 見積もりが 3 ns／単位に届いたので 8 から上げた。ADR-98）。
pub(crate) const FLATTEN_POINT: u64 = 16;
/// 曲線（2 次・3 次・弧）の区間 1 つを平坦化する決まった手間（描く座標系に写して分ける数を求める：`hypot`・平方根、弧は
/// 半径の固有値。M2 の仮の校正。16 × 16 の小さなアイコンで、区間ごとの分が点の手間に比べて大きかった。ADR-98）。
pub(crate) const FLATTEN_CURVE: u64 = 64;
/// 計画が round の継ぎ目の点を抑えるために、区間 1 つの曲がる角を求める手間（向きの角の `atan2` が数個と、弧の接線の
/// `sin`・`cos`。単位 1 の 4 回目のレビュー）。
pub(crate) const TURN_SEGMENT: u64 = 128;
/// 線を作るときに区間 1 つのつなぎ目が滑らかかを決める手間（始めと終わりの接線（弧は `sin`・`cos`）と向きの比べ。ADR-99 (2)）。
pub(crate) const JOIN_TANGENT: u64 = 64;
/// 破線の部分の端の向きを、区間の両端の頂点の向きから内挿して求める手間（向きの正規化が数回。ADR-101）。
pub(crate) const DASH_TANGENT: u64 = 32;
/// 見積もり（`Tree::estimate`）が区間 1 つを数える手間：決まった分（描く座標系の長さ・行の帯と見る箱の切り取り）・線のとき
/// の分（曲がる角の `atan2`・線を作る座標系の長さ）・曲線の分（分ける数・厳密な外接矩形・制御多角形の長さ）。前は区間ごとに
/// 4 単位だけで、時間が手間の 15〜80 倍だった（単位 2 の 4 回目のレビュー。ADR-101）。
pub(crate) const COUNT_SEGMENT: u64 = 32;
pub(crate) const COUNT_STROKE: u64 = 32;
pub(crate) const COUNT_CURVE: u64 = 160;
/// 計画で辺を 1 本作る手間（描く座標系に写した線分を確かめ、箱で切り、固定小数にする。M2 の仮の校正。ADR-98）。
pub(crate) const PLAN_EDGE: u64 = 16;
/// 線の外形・破線の点を 1 つ作る手間。
pub(crate) const OUTLINE_POINT: u64 = 8;
/// ラスタライズ：1 行の決まった手間・行にかかる辺 1 本の手間・セル 1 つの手間・掃く画素 1 つの手間。
pub(crate) const RASTER_ROW: u64 = 8;
pub(crate) const RASTER_EDGE_ROW: u64 = 8;
pub(crate) const RASTER_CELL: u64 = 4;
pub(crate) const RASTER_SWEEP: u64 = 1;
/// 描く段の、辺 1 本を行にかける手間（かかる辺の並びに足す（push：決まった分と 4 バイトの写し）・最後に終わったと見る）と、
/// 辺 1 本の 1 行の手間（セルを積む・終わったかを見る）。範囲の見積もりと描く段の払いの式。
pub(crate) const RASTER_EDGE_START: u64 = RASTER_EDGE_ROW + OP_STEP + OP_STEP + copy(4);
pub(crate) const RASTER_EDGE_ROW_WORK: u64 = RASTER_EDGE_ROW + OP_STEP;

/// 描く段の 1 行の手間（行の決まった分・掃く・セルを 0 に戻す・塗る。項目の矩形の幅 `width`。掃く範囲と戻す範囲は左の外の
/// 1 列と右の 1 列を含みうるので 2 列多く数える）。
pub(crate) const fn raster_row(width: u64) -> u64 {
    RASTER_ROW
        .saturating_add(RASTER_SWEEP.saturating_mul(6))
        .saturating_add(width.saturating_mul(2 * RASTER_SWEEP + PAINT_PIXEL))
}

/// 塗り：画素 1 つを合成する手間。
pub(crate) const PAINT_PIXEL: u64 = 2;
/// 2 分の探索の 1 歩の手間。
pub(crate) const SEARCH_STEP: u64 = 8;
/// id の表の 2 分の探索で、表が大きいとき 1 歩に足す手間（1 歩で表・属性・入力の 3 か所を散らばって読み、表が大きいと
/// キャッシュに乗らず 1 歩が数十 ns になる。id 20 万を散らばった順に引くと 2.2 ns／単位だった。文書の 6 回目のレビュー）。
pub(crate) const ID_PROBE_LARGE: u64 = 64;
/// 足さない表の大きさ（id の数）。
pub(crate) const ID_PROBE_SMALL: u64 = 4096;

/// `n` 個の id の表を引く 1 歩に足す手間。
pub(crate) const fn id_probe(n: u64) -> u64 {
    if n > ID_PROBE_SMALL { ID_PROBE_LARGE } else { 0 }
}

/// 切り上げの割り算（`b` が 0 なら `a`）。
const fn ceil_div(a: u64, b: u64) -> u64 {
    match (a.checked_div(b), a.checked_rem(b)) {
        (Some(q), Some(r)) => q.saturating_add(if r == 0 { 0 } else { 1 }),
        _ => a,
    }
}

/// `bytes` バイトを 0（か同じ値）で埋める手間。
pub(crate) const fn fill(bytes: u64) -> u64 {
    ceil_div(bytes, FILL_BYTES_PER_UNIT)
}

/// `bytes` バイトを写す手間。
pub(crate) const fn copy(bytes: u64) -> u64 {
    ceil_div(bytes, COPY_BYTES_PER_UNIT)
}

/// ⌈log₂ n⌉（n ≤ 1 なら 0）。併合の段の数と 2 分の探索の歩数に使う。
pub(crate) const fn ceil_log2(n: u64) -> u64 {
    if n <= 1 { 0 } else { u64::BITS.saturating_sub(n.saturating_sub(1).leading_zeros()) as u64 }
}

/// 併合の 1 段で 1 つの要素を比べて写す手間（比べる関数は O(1) で軽いとする。要素の大きさの写しを足す）。
pub(crate) const fn merge_step(size: u64) -> u64 {
    MERGE_STEP.saturating_add(copy(size))
}

/// `n` 個（1 つ `size` バイト）の安定な併合の並べ替えの手間の上限（作業の領域の確保と写し・段ごとの併合・戻しの写し）。
/// 2 個未満は何もしない。作業の領域の確保の増えは `n × size`（[`stable_sort_bytes`]）。比べる関数は O(1) で軽いこと
/// （重い比べ方をするときは、呼び手がその分を別に払う）。
pub(crate) const fn stable_sort(n: u64, size: u64) -> u64 {
    if n < 2 {
        return 0;
    }
    let bytes = n.saturating_mul(size);
    let merge = n.saturating_mul(ceil_log2(n)).saturating_mul(merge_step(size));
    // 作業の領域の確保（1 回）・作業の領域への写し（append 1 回）・段ごとの併合・戻しの写し
    ALLOC_STEP.saturating_add(OP_STEP).saturating_add(merge).saturating_add(copy(bytes)).saturating_add(copy(bytes))
}

/// 安定な併合の並べ替えの作業の領域（確保の増え）。
pub(crate) const fn stable_sort_bytes(n: u64, size: u64) -> u64 {
    if n < 2 { 0 } else { n.saturating_mul(size) }
}

/// `n` 個の並びの 2 分の探索の手間（歩数 ⌈log₂ (n + 1)⌉）。
pub(crate) const fn search(n: u64) -> u64 {
    ceil_log2(n.saturating_add(1)).saturating_mul(SEARCH_STEP).saturating_add(1)
}

/// 差し込むときに `moved` 個（1 つ `size` バイト）を後ろへずらす手間（写しと同じ式に、差し込みの 1 を足す）。
pub(crate) const fn shift(moved: u64, size: u64) -> u64 {
    copy(moved.saturating_mul(size)).saturating_add(1)
}

/// CSS の名前の escape を解きながら読む手間（`chars` 字。1 字は生のバイトで 9 まで：`\\`・16 進 6 桁・空白（`\\r\\n`）。
/// 生のバイトの 1 バイト 1 単位の上限で払う。CSS の 3 回目のレビューで、escape の多い名前の比べの払い不足を指摘された）。
pub(crate) const fn decode(chars: u64) -> u64 {
    chars.saturating_mul(9)
}

/// 入力のバイトを読む手間（XML の字句・展開。1 バイト 1 単位。architecture.md §5.2）。
pub(crate) const fn scan(bytes: u64) -> u64 {
    bytes
}

/// 数の字句を f64 に読む手間（`str::parse::<f64>`。字句のバイト数に比例する。読む分の `scan` とは別に払う）。
pub(crate) const fn number(bytes: u64) -> u64 {
    bytes.saturating_mul(4).saturating_add(OP_STEP)
}

/// 1 回の確保の決まった手間（確保器を呼ぶ分。取った領域に触れる分は fill・copy で数える）。
pub(crate) const ALLOC_STEP: u64 = 64;

/// 入れ物の操作 1 回の決まった手間（容量の確かめ・勘定・std の口を呼ぶ分。push を 1 単位 ≈ 1 ns に合わせる）。
pub(crate) const OP_STEP: u64 = 4;

/// CSS の字句を 1 つ作る手間（字句の種類を決めて範囲を作る分。読むバイトの `scan` とは別に払う。短い字句の並びで 1 単位
/// ≈ 1 ns になるように。M7 で校正する）。
pub(crate) const TOKEN: u64 = 16;

/// 木を作る段（M2 の仮の係数。ADR-93）：展開した要素 1 つの決まった手間（たどる段の積み上げ・計算した値の始まり・効果の
/// 確かめ・節を作る分）。
pub(crate) const BUILD_NODE: u64 = 256;
/// 指定の値 1 つの決まった手間（選ぶ・全体の言葉を見る・字句の始まりと終わり）と、値・属性の 1 バイトの手間（1 バイトが
/// 1 字句でも足りる：字句 16・数 8・読み 1）。
pub(crate) const BUILD_SPEC: u64 = 64;
pub(crate) const BUILD_BYTE: u64 = 24;
/// 図形の塗り 1 つを解く決まった手間（url の中身の写し・空白の除き・id の表を引く始まり・確かめ）と、文書に url の塗りが
/// なければ（色か none だけ）の手間。
pub(crate) const BUILD_PAINT: u64 = 4 * ALLOC_STEP;
pub(crate) const BUILD_PAINT_COLOR: u64 = 2 * OP_STEP;
/// 図形（rect・circle・ellipse・line）の節ごとに書く命令と座標の上限（角の丸い rect：移動・線 4・弧 4・閉じ）。
pub(crate) const SHAPE_VERBS: u64 = 10;
pub(crate) const SHAPE_COORDS: u64 = 50;
/// 図形を置き場に書く手間の上限（`append` の式で、角の丸い rect：移動 10・線 4 × 10・弧 4 × 14・閉じ 9 ＝ 115。命令 1 つは
/// 決まった分 2 つと 1 バイトと座標の写し）。
pub(crate) const SHAPE_WRITE: u64 = 120;
/// 外接矩形の区間 1 つの手間（写して、曲線は導関数の根を求める）。
pub(crate) const BOUNDS_SEGMENT: u64 = 16;
/// パスのデータ・`points`・破線の宣言の 1 バイトを読む手間の上限（字句・数・置き場に書く分。パスのデータは敵対的な形で
/// 1 バイトあたり 14.4 単位だった：`values::path` の試験 `work_and_verbs_per_byte`）。
pub(crate) const DECL_BYTE: u64 = 32;
/// パスのデータの 1 バイトを読む手間の上限（敵対的な形で 14.4。`values::path` の試験）。
pub(crate) const PATH_BYTE: u64 = 16;

/// 木を作る段の、展開した要素 1 つの手間（要素の形だけで決まる分。文脈に依る塗りと外接矩形は呼び手が足す）。
/// `children` は子の節の数（文字も）、`probes` は名前で属性を引くときに見る属性の数（引く回数 × 属性の数）、`attr_bytes` は
/// 引いて読む属性の値のバイト数、`specs`・`spec_bytes` は指定の値の数とバイト数、`edges` は参照の辺の数。
pub(crate) const fn build_node(
    children: u64,
    probes: u64,
    attr_bytes: u64,
    specs: u64,
    spec_bytes: u64,
    edges: u64,
) -> u64 {
    BUILD_NODE
        .saturating_add(OP_STEP.saturating_mul(children.saturating_add(2)))
        .saturating_add(specs.saturating_mul(BUILD_SPEC.saturating_add(2 * OP_STEP)))
        .saturating_add(spec_bytes.saturating_mul(BUILD_BYTE))
        .saturating_add(probes.saturating_mul(OP_STEP))
        .saturating_add(attr_bytes.saturating_mul(BUILD_BYTE))
        .saturating_add(edges.saturating_mul(4 * OP_STEP))
}

/// 図形の塗り 1 つを解く手間の上限（`href` は url の値のバイト数の上限、`escaped` は escape があるか、`ids` は id の表の
/// 大きさ）。1 バイトの手間は、escape を解くなら 16（解く 9・写し・空白の除き）、ほかは 4（写し・空白の除き・比べ）。
/// 解いた url は 1.5 倍まで長くなる。
pub(crate) const fn build_paint(href: u64, escaped: bool, ids: u64) -> u64 {
    let per = if escaped { 16 } else { 4 };
    let decoded = href.saturating_add(href / 2);
    let steps = ceil_log2(ids.saturating_add(1)).saturating_add(1);
    let step = SEARCH_STEP.saturating_add(id_probe(ids)).saturating_add(2 * OP_STEP).saturating_add(copy(decoded));
    BUILD_PAINT.saturating_add(href.saturating_mul(per)).saturating_add(steps.saturating_mul(step))
}

/// プロパティの表を名前で引く手間（名前を小文字にして写し、表を二分探索する。表は 100 の内なので 7 回ほど比べる）。
pub(crate) const LOOKUP: u64 = 32;

/// 伸ばしたあとの容量（`cap` で `need` 個が入らないとき。倍々・最小 4）。入れ物と見積もりの両方がこれを使う。
pub(crate) const fn grow_cap(cap: u64, need: u64) -> u64 {
    let doubled = cap.saturating_mul(2);
    let c = if need > doubled { need } else { doubled };
    if c < 4 { 4 } else { c }
}

/// 長さ `len`・容量 `cap` の入れ物に `n` 個（1 つ `size` バイト）を足す手間と、そのときの確保の増え（伸ばすなら、
/// 新しい領域の分。古い領域は写したあとに返す）。`MVec::append_slice`・`push`（n = 1）の式。
pub(crate) const fn append(len: u64, cap: u64, n: u64, size: u64) -> (u64, u64) {
    let need = len.saturating_add(n);
    let (grow_work, grow_bytes) = if need > cap {
        let c = grow_cap(cap, need);
        (ALLOC_STEP.saturating_add(copy(len.saturating_mul(size))), c.saturating_mul(size))
    } else {
        (0, 0)
    };
    (OP_STEP.saturating_add(grow_work).saturating_add(copy(n.saturating_mul(size))), grow_bytes)
}

/// 長さ `len`・容量 `cap` の入れ物の `at` の位置に 1 つ差し込む手間と確保の増え（`MVec::insert` の式）。
/// 後ろに足す分（[`append`] の n = 1）と、ずらす分。
pub(crate) const fn insert(len: u64, cap: u64, at: u64, size: u64) -> (u64, u64) {
    let (w, b) = append(len, cap, 1, size);
    let at = if at > len { len } else { at };
    (w.saturating_add(shift(len.saturating_sub(at), size)), b)
}

/// 長さ `len`・容量 `cap` の並べた入れ物に 1 つ入れる手間の上限と確保の増え（`SortedVec::insert` の式。探す分と、
/// いちばん前に差し込む最悪の分）。
pub(crate) const fn sorted_insert(len: u64, cap: u64, size: u64) -> (u64, u64) {
    let (w, b) = insert(len, cap, 0, size);
    (w.saturating_add(search(len)), b)
}

/// 鍵と値の表（`SortedMap::insert_first`・`insert_or_replace`）に 1 つ入れる手間の上限と確保の増え。
pub(crate) const fn map_insert(len: u64, cap: u64, key_size: u64, value_size: u64) -> (u64, u64) {
    let (wk, bk) = insert(len, cap, 0, key_size);
    let (wv, bv) = insert(len, cap, 0, value_size);
    (search(len).saturating_add(wk).saturating_add(wv), bk.saturating_add(bv))
}

/// 長さ `len`・容量 `cap` の入れ物を、新しい長さ `n` まで同じ値で埋めて伸ばす手間と確保の増え（`MVec::resize_to` の式。
/// `n ≤ len` なら切り詰めるだけで手間なし）。
pub(crate) const fn resize(len: u64, cap: u64, n: u64, size: u64) -> (u64, u64) {
    if n <= len {
        return (0, 0);
    }
    let (grow_work, grow_bytes) = if n > cap {
        let c = grow_cap(cap, n);
        (ALLOC_STEP.saturating_add(copy(len.saturating_mul(size))), c.saturating_mul(size))
    } else {
        (0, 0)
    };
    (OP_STEP.saturating_add(grow_work).saturating_add(fill(n.saturating_sub(len).saturating_mul(size))), grow_bytes)
}

#[cfg(test)]
mod tests;
