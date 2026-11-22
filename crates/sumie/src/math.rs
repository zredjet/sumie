//! 浮動小数の超越関数と、規則を決めた max・min・clamp・powi（determinism.md §2）。
//!
//! 超越関数は `libm = "=0.2.16"`（`default-features = false`）の薄い包みで、どのターゲットでも同じ値になる。std の `f64`
//! の超越関数は OS の libm を呼ぶので clippy で禁じている。`sqrt`・`floor` などの正確に 1 つに決まる口は std のままでよい
//! （§2.1）。

/// 度をラジアンに直す係数（`f64::to_radians` は掛ける定数が std の版で変わった過去があるので使わない。§2.3）。
pub(crate) const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
/// ラジアンを度に直す係数。
pub(crate) const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

pub(crate) fn sin(x: f64) -> f64 {
    libm::sin(x)
}

pub(crate) fn cos(x: f64) -> f64 {
    libm::cos(x)
}

pub(crate) fn tan(x: f64) -> f64 {
    libm::tan(x)
}

pub(crate) fn asin(x: f64) -> f64 {
    libm::asin(x)
}

pub(crate) fn acos(x: f64) -> f64 {
    libm::acos(x)
}

pub(crate) fn atan(x: f64) -> f64 {
    libm::atan(x)
}

/// `atan2(y, x)`（y が先）。
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

pub(crate) fn exp(x: f64) -> f64 {
    libm::exp(x)
}

pub(crate) fn exp2(x: f64) -> f64 {
    libm::exp2(x)
}

/// 自然対数。
pub(crate) fn ln(x: f64) -> f64 {
    libm::log(x)
}

pub(crate) fn log2(x: f64) -> f64 {
    libm::log2(x)
}

pub(crate) fn log10(x: f64) -> f64 {
    libm::log10(x)
}

/// `x` の `y` 乗。
pub(crate) fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

pub(crate) fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}

pub(crate) fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

pub(crate) fn sinh(x: f64) -> f64 {
    libm::sinh(x)
}

pub(crate) fn cosh(x: f64) -> f64 {
    libm::cosh(x)
}

pub(crate) fn tanh(x: f64) -> f64 {
    libm::tanh(x)
}

/// IEEE 754-2019 の maximumNumber（Kura の `math::max` と同じ。ADR-41 (2)）：片方が NaN ならもう片方。-0 < +0 とみなし、
/// `max(-0.0, +0.0)` は `+0.0`。
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return b;
    }
    if b.is_nan() {
        return a;
    }
    if a > b {
        a
    } else if b > a {
        b
    } else if a.is_sign_negative() {
        // 等しい（±0 の組を含む）：a が -0 なら b（+0 か -0）、そうでなければ a
        b
    } else {
        a
    }
}

/// IEEE 754-2019 の minimumNumber：片方が NaN ならもう片方。`min(-0.0, +0.0)` は `-0.0`。
pub(crate) fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return b;
    }
    if b.is_nan() {
        return a;
    }
    if a < b {
        a
    } else if b < a {
        b
    } else if a.is_sign_negative() {
        a
    } else {
        b
    }
}

/// `max(lo, min(x, hi))`。panic しない（`f64::clamp` は NaN と上下の逆で panic する）。`lo > hi` なら `lo`。
/// NaN の `x` は `hi` を経て `max(lo, hi)` になる（NaN は値に入れないので、来たら不具合）。
/// 正の `m` で割った、0 以上 `m` 未満の余り（f64 の `%` は正しく丸める。NaN・∞ は呼び手が入れない）。
#[allow(dead_code, reason = "M1 の values（色の hsl）から使う")]
pub(crate) fn rem_euclid(x: f64, m: f64) -> f64 {
    let r = x % m;
    if r < 0.0 { r + m } else { r }
}

pub(crate) fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    max(lo, min(x, hi))
}

/// `x` の `n` 乗（整数の冪）。掛ける順を決めた 2 進の冪（下の桁から）：`powi` は LLVM の組み込みで、展開のしかたが
/// 版・ターゲットで変わりうる（§2.3）。負の `n` は `1 / x^|n|`。
pub(crate) fn powi(x: f64, n: i32) -> f64 {
    let mut e = n.unsigned_abs();
    let mut base = x;
    let mut acc = 1.0;
    while e != 0 {
        if e & 1 == 1 {
            acc *= base;
        }
        e >>= 1;
        if e != 0 {
            base *= base;
        }
    }
    if n < 0 { 1.0 / acc } else { acc }
}

#[cfg(test)]
mod tests;
