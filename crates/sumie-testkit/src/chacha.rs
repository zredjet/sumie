//! 自前の ChaCha（試験の擬似乱数。determinism.md §5・ADR-39 (5)）。
//!
//! 状態の並びは D. J. Bernstein の元の形（定数 4 語・鍵 8 語・64 ビットの数え 2 語・64 ビットの流れ 2 語。すべて
//! リトルエンディアン）。出力は区切りの 16 語を順に返す。`next_u64` は続く 2 語を (下, 上) として組む。この並びは
//! rand_chacha 0.9 の `ChaCha{8,20}Rng::from_seed` ＋ `set_stream` と同じ（試験で照らす）。
//!
//! 種の決め方（decisions.md ADR-48）：`from_u64(seed)` は鍵の最初の 8 バイトに `seed` をリトルエンディアンで置き、
//! 残りの鍵・流れ・数えは 0。

/// ChaCha の擬似乱数。
#[derive(Debug, Clone)]
pub struct ChaCha {
    state: [u32; 16],
    buf: [u32; 16],
    idx: usize,
    rounds: u32,
}

const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

impl ChaCha {
    /// 鍵・流れ・段の数（8 か 20）から作る。
    pub fn new(key: [u8; 32], stream: u64, rounds: u32) -> ChaCha {
        let mut state = [0u32; 16];
        state[..4].copy_from_slice(&SIGMA);
        for (i, w) in key.as_chunks::<4>().0.iter().enumerate() {
            state[4 + i] = u32::from_le_bytes(*w);
        }
        state[14] = stream as u32;
        state[15] = (stream >> 32) as u32;
        ChaCha { state, buf: [0; 16], idx: 16, rounds }
    }

    /// 8 段の ChaCha（試験の既定）。
    pub fn chacha8(key: [u8; 32]) -> ChaCha {
        ChaCha::new(key, 0, 8)
    }

    /// `u64` の種から作る（鍵の最初の 8 バイトに種）。
    pub fn from_u64(seed: u64) -> ChaCha {
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&seed.to_le_bytes());
        ChaCha::chacha8(key)
    }

    fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(16);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(12);
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(8);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(7);
    }

    fn refill(&mut self) {
        let mut x = self.state;
        for _ in 0..self.rounds / 2 {
            Self::quarter(&mut x, 0, 4, 8, 12);
            Self::quarter(&mut x, 1, 5, 9, 13);
            Self::quarter(&mut x, 2, 6, 10, 14);
            Self::quarter(&mut x, 3, 7, 11, 15);
            Self::quarter(&mut x, 0, 5, 10, 15);
            Self::quarter(&mut x, 1, 6, 11, 12);
            Self::quarter(&mut x, 2, 7, 8, 13);
            Self::quarter(&mut x, 3, 4, 9, 14);
        }
        for (o, (a, b)) in self.buf.iter_mut().zip(x.iter().zip(self.state.iter())) {
            *o = a.wrapping_add(*b);
        }
        // 64 ビットの数え
        let counter = (u64::from(self.state[13]) << 32 | u64::from(self.state[12])).wrapping_add(1);
        self.state[12] = counter as u32;
        self.state[13] = (counter >> 32) as u32;
        self.idx = 0;
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.idx >= 16 {
            self.refill();
        }
        let v = self.buf[self.idx];
        self.idx += 1;
        v
    }

    pub fn next_u64(&mut self) -> u64 {
        let lo = u64::from(self.next_u32());
        let hi = u64::from(self.next_u32());
        hi << 32 | lo
    }

    /// `0..n` の一様な数（n が 0 なら 0）。捨てて引き直す方法なので偏らない。
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let zone = u64::MAX - (u64::MAX % n + 1) % n;
        loop {
            let v = self.next_u64();
            if v <= zone {
                return v % n;
            }
        }
    }

    /// `lo..=hi` の一様な数。
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below(hi - lo + 1)
    }

    /// 確率 `num / den` で真。
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }

    /// バイト列を埋める。
    pub fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.next_u32() as u8;
        }
    }

    /// 並びから 1 つ選ぶ（空なら None）。
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        items.get(self.below(items.len() as u64) as usize)
    }

    /// 0 以上 1 未満の `f64`（53 ビット）。
    pub fn unit_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_chacha::{ChaCha8Rng, ChaCha20Rng};
    use rand_core::{RngCore, SeedableRng};

    fn key(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(i as u8);
        }
        k
    }

    #[test]
    fn matches_rand_chacha_8_and_20() {
        for s in 0..4u8 {
            for stream in [0u64, 1, 0xdead_beef_0000_0001] {
                let mut ours = ChaCha::new(key(s), stream, 8);
                let mut theirs = ChaCha8Rng::from_seed(key(s));
                theirs.set_stream(stream);
                for _ in 0..100 {
                    assert_eq!(ours.next_u32(), theirs.next_u32());
                }
                for _ in 0..100 {
                    assert_eq!(ours.next_u64(), theirs.next_u64());
                }
                let mut ours = ChaCha::new(key(s), stream, 20);
                let mut theirs = ChaCha20Rng::from_seed(key(s));
                theirs.set_stream(stream);
                for _ in 0..100 {
                    assert_eq!(ours.next_u32(), theirs.next_u32());
                }
            }
        }
    }

    /// RFC 8439 §2.3.2 の ChaCha20 の区切り（数え 1・nonce 00:00:00:09:00:00:00:4a:00:00:00:00）。元の形の状態では
    /// 語 12 が数え、語 13〜15 が nonce に当たる。
    #[test]
    fn rfc8439_block() {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut c = ChaCha::new(k, 0, 20);
        c.state[12] = 1;
        c.state[13] = 0x0900_0000;
        c.state[14] = 0x4a00_0000;
        c.state[15] = 0;
        let first: Vec<u32> = (0..4).map(|_| c.next_u32()).collect();
        assert_eq!(first, [0xe4e7_f110, 0x1559_3bd1, 0x1fdd_0f50, 0xc471_20a3]);
    }

    #[test]
    fn below_is_in_range_and_deterministic() {
        let mut a = ChaCha::from_u64(7);
        let mut b = ChaCha::from_u64(7);
        for n in [1u64, 2, 3, 10, 1000, u64::MAX] {
            for _ in 0..50 {
                let x = a.below(n);
                assert!(x < n);
                assert_eq!(x, b.below(n));
            }
        }
        assert_eq!(a.below(0), 0);
        assert!((0..100).all(|_| (5..=9).contains(&a.range(5, 9))));
    }
}
