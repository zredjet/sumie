//! 段 6 の一部：塗り（architecture.md §6.3）。画素は RGBA8 の乗算済み。合成は整数で、255 で割る丸めは [`div255`] の 1 つ。
//! M2 は単色の source-over だけ（グラデーション・pattern・blend は M3）。

/// `x / 255` の四捨五入（`x ≤ 255 · 255`。`(x + 128 + ((x + 128) >> 8)) >> 8` はこの範囲で正しく丸める）。
pub(crate) fn div255(x: u32) -> u32 {
    let y = x.saturating_add(128);
    y.saturating_add(y >> 8) >> 8
}

/// 単色（r・g・b は 0〜255、α は 0〜1。乗算しない）を乗算済みの 8 ビットにする（α は `round(255 α)`、色は
/// `round(c · α)`。どれも 0〜255 に切り詰める。色の成分は α 以下になる）。
pub(crate) fn premultiply(c: [f64; 4]) -> [u8; 4] {
    let clamp = |v: f64, hi: f64| {
        if v.is_nan() || v <= 0.0 {
            0.0
        } else if v >= hi {
            hi
        } else {
            v
        }
    };
    let a = clamp(c[3], 1.0);
    let ch = |v: f64| clamp((clamp(v, 255.0) * a).round(), 255.0) as u8;
    [ch(c[0]), ch(c[1]), ch(c[2]), clamp((a * 255.0).round(), 255.0) as u8]
}

/// 乗算済みの色 `src` を被覆 `cov`（0〜255）で、画素の並び `dst`（RGBA8 乗算済みが `len` 個続くもの）に source-over で重ねる。
pub(crate) fn blend_span(dst: &mut [u8], src: [u8; 4], cov: u8) {
    let c = u32::from(cov);
    let s = src.map(|v| div255(u32::from(v).saturating_mul(c)));
    if s[3] == 0 {
        return;
    }
    let inv = 255u32.saturating_sub(s[3]);
    for (i, d) in dst.iter_mut().enumerate() {
        let sk = s.get(i & 3).copied().unwrap_or(0);
        let v = sk.saturating_add(div255(u32::from(*d).saturating_mul(inv)));
        *d = u8::try_from(v).unwrap_or(u8::MAX);
    }
}

#[cfg(test)]
mod tests;
