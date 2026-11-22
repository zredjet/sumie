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
/// 2 分の探索の 1 歩の手間。
pub(crate) const SEARCH_STEP: u64 = 8;

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

/// 木を作る段の、展開したあとの要素 1 つあたりの手間と確保（M1 の仮の値。木を作る段（M2）で式に置き換える）。
pub(crate) const BUILD_PER_INSTANCE: u64 = 256;
pub(crate) const BUILD_BYTES_PER_INSTANCE: u64 = 128;

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
