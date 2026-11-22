//! 種つきの変異（壊れた入力の試験。CLAUDE.md「新しいパーサーには、壊れた入力のテスト（種つきの変異）」）。
//!
//! 種（`ChaCha`）・元の入力・辞書（字句の断片）・回数を決めれば、作る入力の並びは決まる。パーサーごとの
//! `tests/mutate.rs` が、これで作った入力を通して「panic しない・上限の内で返る」を確かめる。

use crate::chacha::ChaCha;

/// 変異で差し込む、境目になりやすいバイト。
const INTERESTING: &[u8] = &[
    0x00, 0x01, 0x7f, 0x80, 0xff, 0xc3, 0xe3, 0xf0, b'<', b'>', b'&', b';', b'"', b'\'', b'#', b'/', b'=', b'0', b'9',
    b'-', b'+', b'.', b'e', b'E', b' ', b'\n', b'\t', b'\r', b'(', b')', b'{', b'}', b'[', b']', b',', b':', b'%',
];

/// `input` に 1〜4 回の変異を当てる。長さは `max_len` までに切る。
pub fn mutate(rng: &mut ChaCha, input: &[u8], dict: &[&[u8]], max_len: usize) -> Vec<u8> {
    let mut v = input.to_vec();
    let ops = rng.range(1, 4);
    for _ in 0..ops {
        let len = v.len() as u64;
        match rng.below(9) {
            // ビットを裏返す
            0 if len > 0 => {
                let i = rng.below(len) as usize;
                v[i] ^= 1 << rng.below(8);
            }
            // 任意のバイトにする
            1 if len > 0 => {
                let i = rng.below(len) as usize;
                v[i] = rng.next_u32() as u8;
            }
            // 境目のバイトにする
            2 if len > 0 => {
                let i = rng.below(len) as usize;
                v[i] = *rng.pick(INTERESTING).unwrap_or(&0);
            }
            // 任意のバイトを差し込む
            3 => {
                let at = rng.below(len + 1) as usize;
                let n = rng.range(1, 8) as usize;
                let mut b = vec![0u8; n];
                rng.fill(&mut b);
                v.splice(at..at, b);
            }
            // 範囲を消す
            4 if len > 0 => {
                let a = rng.below(len) as usize;
                let b = (a + rng.range(1, 16) as usize).min(v.len());
                v.drain(a..b);
            }
            // 範囲を写して差し込む
            5 if len > 0 => {
                let a = rng.below(len) as usize;
                let b = (a + rng.range(1, 64) as usize).min(v.len());
                let piece = v[a..b].to_vec();
                let at = rng.below(v.len() as u64 + 1) as usize;
                v.splice(at..at, piece);
            }
            // 辞書の断片を差し込む
            6 => {
                if let Some(tok) = rng.pick(dict) {
                    let at = rng.below(len + 1) as usize;
                    v.splice(at..at, tok.iter().copied());
                }
            }
            // 切り詰める
            7 if len > 0 => {
                let n = rng.below(len) as usize;
                v.truncate(n);
            }
            // 範囲を同じ断片の繰り返しにする（入れ子の深さ・長い並びの例）
            _ => {
                if let Some(tok) = rng.pick(dict) {
                    let times = rng.range(2, 64) as usize;
                    let at = rng.below(len + 1) as usize;
                    let rep: Vec<u8> = tok.iter().copied().cycle().take(tok.len() * times).collect();
                    v.splice(at..at, rep);
                }
            }
        }
        if v.len() > max_len {
            v.truncate(max_len);
        }
    }
    v
}

/// `seeds` から `n` 件の変異した入力を作る（各件は元の入力を 1 つ選び、1〜3 回 [`mutate`] を重ねる）。
pub fn cases(seed: u64, n: usize, seeds: &[&[u8]], dict: &[&[u8]], max_len: usize) -> Vec<Vec<u8>> {
    let mut rng = ChaCha::from_u64(seed);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let base: &[u8] = rng.pick(seeds).copied().unwrap_or(&[]);
        let mut v = base.to_vec();
        for _ in 0..rng.range(1, 3) {
            v = mutate(&mut rng, &v, dict, max_len);
        }
        out.push(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_bounded() {
        let seeds: &[&[u8]] = &[b"<svg width=\"10\"/>", b""];
        let dict: &[&[u8]] = &[b"<g>", b"&amp;", b"1e9"];
        let a = cases(1, 200, seeds, dict, 64);
        let b = cases(1, 200, seeds, dict, 64);
        assert_eq!(a, b);
        assert!(a.iter().all(|c| c.len() <= 64));
        assert_ne!(a, cases(2, 200, seeds, dict, 64));
        // 何かは変わる
        assert!(a.iter().any(|c| c.as_slice() != seeds[0] && c.as_slice() != seeds[1]));
    }
}
