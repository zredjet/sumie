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

/// 属性の値（`="…"` の中）の範囲を並べる。
fn value_spans(v: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < v.len() {
        if v[i] == b'=' && v[i + 1] == b'"' {
            let start = i + 2;
            let mut j = start;
            while j < v.len() && v[j] != b'"' {
                j += 1;
            }
            if j < v.len() {
                out.push((start, j));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// XML の構造を保つ変異：属性の値の中だけを変えるか、値の閉じの引用符のあとに属性を差し込む。辞書の断片のうち空白で
/// 始まるもの（` stroke-width="2"` など）は属性として差し込み、ほかは値の中に差し込むか値の一部と置き換える。値の中の
/// 数字は別の数字にもする。値のない入力は [`mutate`] に回す。
pub fn mutate_values(rng: &mut ChaCha, input: &[u8], dict: &[&[u8]], max_len: usize) -> Vec<u8> {
    let mut v = input.to_vec();
    let attrs: Vec<&[u8]> = dict.iter().copied().filter(|t| t.first() == Some(&b' ')).collect();
    let words: Vec<&[u8]> = dict.iter().copied().filter(|t| t.first() != Some(&b' ') && !t.contains(&b'"')).collect();
    for _ in 0..rng.range(1, 4) {
        let spans = value_spans(&v);
        let Some(&(a, b)) = rng.pick(&spans) else { return mutate(rng, input, dict, max_len) };
        match rng.below(5) {
            // 値の中に断片を差し込む
            0 => {
                if let Some(tok) = rng.pick(&words) {
                    let at = a + rng.below((b - a + 1) as u64) as usize;
                    v.splice(at..at, tok.iter().copied());
                }
            }
            // 値の一部を断片に置き換える
            1 => {
                if let Some(tok) = rng.pick(&words) {
                    let x = a + rng.below((b - a + 1) as u64) as usize;
                    let y = (x + rng.range(0, 8) as usize).min(b);
                    v.splice(x..y, tok.iter().copied());
                }
            }
            // 値の一部を消す
            2 if b > a => {
                let x = a + rng.below((b - a) as u64) as usize;
                let y = (x + rng.range(1, 6) as usize).min(b);
                v.drain(x..y);
            }
            // 数字を別の数字にする
            3 => {
                let digits: Vec<usize> = (a..b).filter(|&i| v[i].is_ascii_digit()).collect();
                if let Some(&i) = rng.pick(&digits) {
                    v[i] = b'0' + rng.below(10) as u8;
                }
            }
            // 値の閉じの引用符のあとに属性を差し込む
            _ => {
                if let Some(tok) = rng.pick(&attrs) {
                    v.splice(b + 1..b + 1, tok.iter().copied());
                }
            }
        }
        if v.len() > max_len {
            v.truncate(max_len);
        }
    }
    v
}

/// `seeds` から `n` 件の、構造を保つ変異（[`mutate_values`]）をした入力を作る。
pub fn value_cases(seed: u64, n: usize, seeds: &[&[u8]], dict: &[&[u8]], max_len: usize) -> Vec<Vec<u8>> {
    let mut rng = ChaCha::from_u64(seed);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let base: &[u8] = rng.pick(seeds).copied().unwrap_or(&[]);
        out.push(mutate_values(&mut rng, base, dict, max_len));
    }
    out
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

    /// 構造を保つ変異は、要素の名前・括弧・引用符の数を変えない（値の中の断片は引用符を持たない）。
    #[test]
    fn value_mutations_keep_structure() {
        let seeds: &[&[u8]] = &[b"<svg width=\"10\"><path d=\"M0 0L5 5\" stroke=\"red\"/></svg>"];
        let dict: &[&[u8]] = &[b"1e30", b" stroke-width=\"2\"", b"M", b"%"];
        let a = value_cases(3, 300, seeds, dict, 256);
        assert_eq!(a, value_cases(3, 300, seeds, dict, 256));
        let count = |v: &[u8], c: u8| v.iter().filter(|&&b| b == c).count();
        let base = seeds[0];
        for c in &a {
            assert!(c.starts_with(b"<svg width=\""), "{}", String::from_utf8_lossy(c));
            assert!(c.ends_with(b"/></svg>"), "{}", String::from_utf8_lossy(c));
            assert_eq!(count(c, b'<'), count(base, b'<'));
            assert_eq!(count(c, b'>'), count(base, b'>'));
            assert_eq!(count(c, b'"') % 2, 0);
        }
        assert!(a.iter().filter(|c| c.as_slice() != base).count() > 250);
    }
}
