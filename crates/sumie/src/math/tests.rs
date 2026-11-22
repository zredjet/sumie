//! math の試験。値のダイジェスト（`MATH_DIGEST`）は全ターゲット（CI の 3 つと Rosetta 2 の x86_64）で同じでなければ
//! ならない（D1。libm は純粋な Rust なので、OS の libm に依らない）。値を変えたら版を上げる（determinism.md §8）。
#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::arithmetic_side_effects, clippy::float_cmp)]

use super::*;
use sumie_testkit::chacha::ChaCha;

#[test]
fn max_min_follow_ieee_maximum_number() {
    assert_eq!(max(1.0, 2.0), 2.0);
    assert_eq!(min(1.0, 2.0), 1.0);
    assert!(max(-0.0, 0.0).is_sign_positive());
    assert!(max(0.0, -0.0).is_sign_positive());
    assert!(min(-0.0, 0.0).is_sign_negative());
    assert!(min(0.0, -0.0).is_sign_negative());
    assert!(max(-0.0, -0.0).is_sign_negative());
    assert_eq!(max(f64::NAN, 3.0), 3.0);
    assert_eq!(max(3.0, f64::NAN), 3.0);
    assert_eq!(min(f64::NAN, 3.0), 3.0);
    assert_eq!(min(3.0, f64::NAN), 3.0);
    assert!(max(f64::NAN, f64::NAN).is_nan());
    assert_eq!(max(f64::NEG_INFINITY, f64::INFINITY), f64::INFINITY);
    assert_eq!(min(f64::NEG_INFINITY, f64::INFINITY), f64::NEG_INFINITY);
}

#[test]
fn clamp_never_panics() {
    assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
    assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
    assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
    // 上下の逆は lo
    assert_eq!(clamp(0.5, 1.0, 0.0), 1.0);
    // NaN の x は hi を経て max(lo, hi)
    assert_eq!(clamp(f64::NAN, 0.0, 1.0), 1.0);
    assert_eq!(clamp(0.5, f64::NAN, 1.0), 0.5);
}

#[test]
fn powi_has_a_fixed_order() {
    assert_eq!(powi(2.0, 10), 1024.0);
    assert_eq!(powi(2.0, -2), 0.25);
    assert_eq!(powi(3.0, 0), 1.0);
    assert_eq!(powi(0.0, 0), 1.0);
    assert_eq!(powi(1.5, 3), 1.5 * (1.5 * 1.5));
    // 2 進の冪：x^5 = x · (x²)²
    let x = 1.1f64;
    assert_eq!(powi(x, 5).to_bits(), (x * ((x * x) * (x * x))).to_bits());
    assert_eq!(powi(2.0, i32::MIN), 0.0);
    assert!(powi(2.0, i32::MAX).is_infinite());
}

#[test]
fn degree_constants() {
    assert_eq!(DEG_TO_RAD.to_bits(), 0x3f91_df46_a252_9d39);
    assert_eq!(RAD_TO_DEG.to_bits(), 0x404c_a5dc_1a63_c1f8);
    assert_eq!(180.0 * DEG_TO_RAD, std::f64::consts::PI);
}

/// すべての関数の値のダイジェスト。入力は種 1 の ChaCha8 で、任意のビット（NaN・±∞・非正規化数を含む）と、ふつうの
/// 範囲の数を混ぜる。
fn digest() -> String {
    let mut rng = ChaCha::from_u64(1);
    let mut h = blake3::Hasher::new();
    let mut put = |x: f64| {
        h.update(&x.to_bits().to_le_bytes());
    };
    for i in 0..20_000u32 {
        let (a, b) = if i % 2 == 0 {
            (f64::from_bits(rng.next_u64()), f64::from_bits(rng.next_u64()))
        } else {
            let s = [1.0, 10.0, 1e3, 1e-3, 1e8][(i as usize / 2) % 5];
            ((rng.unit_f64() * 2.0 - 1.0) * s, (rng.unit_f64() * 2.0 - 1.0) * s)
        };
        for v in [
            sin(a),
            cos(a),
            tan(a),
            asin(a),
            acos(a),
            atan(a),
            atan2(a, b),
            exp(a),
            exp2(a),
            ln(a),
            log2(a),
            log10(a),
            pow(a, b),
            cbrt(a),
            hypot(a, b),
            sinh(a),
            cosh(a),
            tanh(a),
            max(a, b),
            min(a, b),
            clamp(a, -1.0, 1.0),
            powi(a, (i % 41) as i32 - 20),
        ] {
            // NaN のビットの形は CPU で違いうるので、NaN は 1 つの印にする（determinism.md §2.5）
            put(if v.is_nan() { f64::NAN } else { v });
        }
    }
    h.finalize().to_hex().to_string()
}

/// 値のダイジェスト。全ターゲットで同じ（CI が照らす）。
const MATH_DIGEST: &str = "b8ba1adf54f7e1020fac7cbe919583ea46dc9af4a4b28ea934f5dc622d02215d";

#[test]
fn values_are_the_same_on_every_target() {
    let d = digest();
    assert_eq!(d, MATH_DIGEST, "math の値が変わった（ターゲットか libm の版か式の違い。determinism.md §2）");
}
