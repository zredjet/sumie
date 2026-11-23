//! パスのデータ（`d`。SVG 2 §9.3.9 の文法）と `points`（§10.6・§10.7）を読む（FR-SH-01・FR-SH-02・ADR-90）。
//!
//! - バイト列のまま走査し、読むバイトを塊ずつ払う（`ScanMeter`）。どの枝も 1 バイト以上進むか終わる
//! - 空白は SVG 2 の wsp（空白・タブ・CR・LF・FF）。引数の間は「空白か、空白に囲まれた `,` 1 つ」で、省いてもよい（数の
//!   符号・`.` で区切れる）。命令の字のすぐあとの `,` は誤り。最後の引数のあとの `,` は受ける（resvg・Chrome と同じ）
//! - 数：`符号? (数字+ ("." 数字+)? | "." 数字+) (("e" | "E") 符号? 数字+)?`。`.` のあとに数字が要る（`10.` は `10` で終わり、
//!   続く `.` は誤り。Chrome と同じ。ADR-90）。指数の字のあとに数字がなければ、数はその前で終わる。値は CSS の字句と同じ
//!   口（`css::lex::number_value`）で正しく丸めて読み、有限でなければ誤り
//! - 弧の旗は 1 字（`0` か `1`）。`1190` は旗 1・1 と 90
//! - 誤り：そこを含む命令の前まで（引数の足りない命令は、最後の正しい区間まで）を描く（SVG 2 §9.5.4）。初めの命令が `M`・
//!   `m` でなければ何も描かない。`Z` のあとの数は誤り
//! - `Z` のあとに `M` 以外の命令が来れば、部分パスの始めの点から新しい部分パスを始める（書き手は MOVE を足す）
//! - `points`：数の組の並び。途中に誤りがあれば何も描かない（Chrome と同じ。ADR-90）。数が奇数なら最後を捨てる
//!   （SVG 2 §10.6。resvg・Chrome と同じ）
//!
//! 置き場に書く量は入力の 1 バイトあたり [`cost::PATH_GEOMETRY_BYTES`] バイト以下（`estimate_build` が使う。最も多いのは
//! `zh1` の繰り返し：3 バイトで閉じ・始まり・線の 35 バイト）。

use crate::budget::Budget;
use crate::css::lex::{NumScan, number_value};
use crate::error::Error;
use crate::geom::{PathWriter, Point};
use crate::mem::MVec;
use crate::xml::lex::ScanMeter;

/// パスのデータを読んだ結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Parsed {
    /// 終わりまで読めた（空を含む）
    Complete,
    /// この位置（値の中のバイト）で誤り（そこまでを描く）
    Error(usize),
}

/// 走査（読むバイトを塊ずつ払う）。
struct Scanner<'s, 'b> {
    s: &'s [u8],
    i: usize,
    meter: ScanMeter,
    budget: &'b Budget<'b>,
}

/// SVG 2 の wsp。
fn is_wsp(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

impl<'s, 'b> Scanner<'s, 'b> {
    fn new(s: &'s [u8], budget: &'b Budget<'b>) -> Scanner<'s, 'b> {
        Scanner { s, i: 0, meter: ScanMeter::new(0, s.len()), budget }
    }

    fn at(&mut self, j: usize) -> Result<Option<u8>, Error> {
        self.meter.pay(self.budget, j)?;
        Ok(self.s.get(j).copied())
    }

    fn digit_at(&mut self, j: usize) -> Result<Option<u8>, Error> {
        Ok(self.at(j)?.filter(u8::is_ascii_digit))
    }

    fn skip_wsp(&mut self) -> Result<(), Error> {
        while self.at(self.i)?.is_some_and(is_wsp) {
            self.i = self.i.saturating_add(1);
        }
        Ok(())
    }

    /// 引数の間の区切り（`wsp* ","? wsp*`）を飛ばす。`,` を飛ばしたら真。
    fn skip_comma_wsp(&mut self) -> Result<bool, Error> {
        self.skip_wsp()?;
        let comma = self.at(self.i)? == Some(b',');
        if comma {
            self.i = self.i.saturating_add(1);
            self.skip_wsp()?;
        }
        Ok(comma)
    }

    /// 数が始まるか（符号・数字・`.` と数字）。
    fn starts_number(&mut self) -> Result<bool, Error> {
        let mut j = self.i;
        if matches!(self.at(j)?, Some(b'+' | b'-')) {
            j = j.saturating_add(1);
        }
        Ok(match self.at(j)? {
            Some(b) if b.is_ascii_digit() => true,
            Some(b'.') => self.digit_at(j.saturating_add(1))?.is_some(),
            _ => false,
        })
    }

    /// 数を読む（数でないか有限でなければ None で、進まない）。
    fn number(&mut self) -> Result<Option<f64>, Error> {
        let i0 = self.i;
        let mut j = i0;
        let mut d = NumScan::default();
        let neg = self.at(j)? == Some(b'-');
        if matches!(self.at(j)?, Some(b'+' | b'-')) {
            j = j.saturating_add(1);
        }
        let mut any = false;
        while let Some(b) = self.digit_at(j)? {
            d.digit(b, j, false);
            j = j.saturating_add(1);
            any = true;
        }
        if self.at(j)? == Some(b'.') && self.digit_at(j.saturating_add(1))?.is_some() {
            j = j.saturating_add(1);
            while let Some(b) = self.digit_at(j)? {
                d.digit(b, j, true);
                j = j.saturating_add(1);
            }
            any = true;
        }
        if !any {
            return Ok(None);
        }
        if matches!(self.at(j)?, Some(b'e' | b'E')) {
            let k = j.saturating_add(1);
            let sign = self.at(k)?;
            let digits = if matches!(sign, Some(b'+' | b'-')) { k.saturating_add(1) } else { k };
            if self.digit_at(digits)?.is_some() {
                d.exp_neg = sign == Some(b'-');
                j = digits;
                while let Some(b) = self.digit_at(j)? {
                    d.exp_digit(b);
                    j = j.saturating_add(1);
                }
            }
        }
        let v = number_value(self.s, i0, j, &d, neg, self.budget)?;
        if !v.is_finite() {
            return Ok(None);
        }
        self.i = j;
        Ok(Some(v))
    }

    /// 弧の旗（1 字の `0`・`1`）。
    fn flag(&mut self) -> Result<Option<bool>, Error> {
        let f = match self.at(self.i)? {
            Some(b'0') => false,
            Some(b'1') => true,
            _ => return Ok(None),
        };
        self.i = self.i.saturating_add(1);
        Ok(Some(f))
    }
}

/// ASCII の英字の大文字（ほかはそのまま）。
trait Upper {
    fn upper(self) -> u8;
}

impl Upper for u8 {
    fn upper(self) -> u8 {
        if self.is_ascii_lowercase() { self ^ 0x20 } else { self }
    }
}

/// 命令の引数の数（数と旗）。
fn args_of(cmd: u8) -> usize {
    match cmd.upper() {
        b'M' | b'L' | b'T' => 2,
        b'H' | b'V' => 1,
        b'C' => 6,
        b'S' | b'Q' => 4,
        b'A' => 7,
        _ => 0,
    }
}

/// 命令の字か。
fn is_command(b: u8) -> bool {
    matches!(b.upper(), b'M' | b'L' | b'H' | b'V' | b'C' | b'S' | b'Q' | b'T' | b'A' | b'Z')
}

/// パスのデータ `s` を読んで `w` に書く（[`Parsed`] を返す。誤りの前までは書く）。
pub(crate) fn path_data(s: &[u8], w: &mut PathWriter<'_, '_>, budget: &Budget<'_>) -> Result<Parsed, Error> {
    let budget = budget_ref(budget);
    let mut sc = Scanner::new(s, budget);
    sc.skip_wsp()?;
    match sc.at(sc.i)? {
        None => return Ok(Parsed::Complete),
        Some(b'M' | b'm') => {}
        Some(_) => return Ok(Parsed::Error(sc.i)),
    }
    // 前の命令（暗黙の繰り返しと、S・T の制御点の映し）
    let mut prev: u8 = 0;
    // 前の区間の制御点（S の映しには 3 次の 2 つ目、T の映しには 2 次の制御点）
    let mut last_ctrl = Point::new(0.0, 0.0);
    // Z のあと、M でない命令は部分パスの始めの点から始める
    let mut after_close = false;
    loop {
        sc.skip_wsp()?;
        let Some(c) = sc.at(sc.i)? else { return Ok(Parsed::Complete) };
        let cmd = if is_command(c) {
            sc.i = sc.i.saturating_add(1);
            sc.skip_wsp()?;
            c
        } else if prev != 0 && !matches!(prev, b'Z' | b'z') && sc.starts_number()? {
            // 同じ命令の繰り返し（M のあとは L）
            match prev {
                b'M' => b'L',
                b'm' => b'l',
                p => p,
            }
        } else {
            return Ok(Parsed::Error(sc.i));
        };
        let n = args_of(cmd);
        let mut a = [0.0f64; 7];
        for k in 0..n {
            if k > 0 {
                sc.skip_comma_wsp()?;
            }
            let v = if cmd.upper() == b'A' && (k == 3 || k == 4) {
                sc.flag()?.map(|f| if f { 1.0 } else { 0.0 })
            } else {
                sc.number()?
            };
            let Some(v) = v else { return Ok(Parsed::Error(sc.i)) };
            if let Some(x) = a.get_mut(k) {
                *x = v;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let cur = w.current();
        let pt = |x: f64, y: f64| if rel { Point::new(cur.x + x, cur.y + y) } else { Point::new(x, y) };
        let upper = cmd.upper();
        if after_close && !matches!(upper, b'M' | b'Z') {
            w.move_to(w.subpath_start())?;
        }
        after_close = false;
        let [a0, a1, a2, a3, a4, a5, a6] = a;
        match upper {
            b'M' => w.move_to(pt(a0, a1))?,
            b'L' => w.line_to(pt(a0, a1))?,
            b'H' => w.line_to(Point::new(if rel { cur.x + a0 } else { a0 }, cur.y))?,
            b'V' => w.line_to(Point::new(cur.x, if rel { cur.y + a0 } else { a0 }))?,
            b'C' => {
                let (c2, p) = (pt(a2, a3), pt(a4, a5));
                w.cubic_to(pt(a0, a1), c2, p)?;
                last_ctrl = c2;
            }
            b'S' => {
                let c1 = if matches!(prev.upper(), b'C' | b'S') { reflect(last_ctrl, cur) } else { cur };
                let (c2, p) = (pt(a0, a1), pt(a2, a3));
                w.cubic_to(c1, c2, p)?;
                last_ctrl = c2;
            }
            b'Q' => {
                let c1 = pt(a0, a1);
                w.quad_to(c1, pt(a2, a3))?;
                last_ctrl = c1;
            }
            b'T' => {
                let c1 = if matches!(prev.upper(), b'Q' | b'T') { reflect(last_ctrl, cur) } else { cur };
                w.quad_to(c1, pt(a0, a1))?;
                last_ctrl = c1;
            }
            b'A' => w.arc_to(a0, a1, a2, a3 != 0.0, a4 != 0.0, pt(a5, a6))?,
            _ => {
                w.close()?;
                after_close = true;
            }
        }
        prev = cmd;
        // 引数のあとの区切り（最後の引数のあとの `,` も受ける）
        if n > 0 {
            sc.skip_comma_wsp()?;
        }
    }
}

/// `c` を `about` について映した点。
fn reflect(c: Point, about: Point) -> Point {
    Point::new(2.0 * about.x - c.x, 2.0 * about.y - c.y)
}

/// `points` を読んで点の並びを `out` に足す。途中に誤りがあれば偽を返す（呼び手は何も描かない）。数が奇数なら最後を
/// 捨てる。
pub(crate) fn points(s: &[u8], out: &mut MVec<'_, Point>, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    let mut sc = Scanner::new(s, budget);
    sc.skip_wsp()?;
    let mut first = true;
    let mut pending: Option<f64> = None;
    loop {
        if !first {
            let comma = sc.skip_comma_wsp()?;
            if sc.at(sc.i)?.is_none() {
                // 終わりの `,` は誤り（数のあとの区切りだけ）
                return Ok(!comma);
            }
        }
        if sc.at(sc.i)?.is_none() {
            return Ok(true);
        }
        let Some(v) = sc.number()? else { return Ok(false) };
        first = false;
        match pending.take() {
            Some(x) => out.push(Point::new(x, v))?,
            None => pending = Some(v),
        }
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
