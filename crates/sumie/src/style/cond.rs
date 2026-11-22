//! 条件を照らす（`@media`・`<style media>`・`@supports`。ADR-66 (2)・ADR-68 (4)）。
//!
//! - `@media`：Media Queries 4 の問いの並び。媒体の種類は `screen`・`all` が真、ほかは偽。条件は 3 値（知らない形・知らない
//!   特徴は「分からない」で、最後に偽）。環境は決まった値（D1）：明暗は呼び手の設定、`prefers-*` は `no-preference`、
//!   色は 8 ビット、解像度は 1dppx（96dpi）、`hover`・`pointer` は `none`、`update` は `none`。幅・高さ・縦横比・向きは文書の
//!   大きさ（CSS の px。width・height・viewBox で決まるとき）で照らし、決まらなければ偽。長さの em・rem は 16px（初期の値）
//! - `@supports`：`(名前: 値)` は sumie が読めるプロパティと形か、`selector()` は sumie が読めるセレクタか。`not`・`and`・`or`
//! - 再帰しない（入れ子のかっこは明示の積み上げ）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Token, ident_is};
use crate::css::select::{NsTable, Tables, parse_selector_list};
use crate::error::Error;
use crate::mem::{MVec, Plain};
use crate::style::ColorScheme;
use crate::style::props::{lookup, prop};
use crate::style::valid::{valid, wide};
use crate::xml::lex::Span;

/// 照らす環境。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Env {
    pub(crate) color_scheme: ColorScheme,
    /// 文書の大きさ（CSS の px）。中身で決まる文書は None
    pub(crate) size: Option<(f64, f64)>,
}

/// 3 値。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tri {
    False,
    True,
    Unknown,
}

impl Plain for Tri {}

impl Tri {
    fn not(self) -> Tri {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }

    fn and(self, o: Tri) -> Tri {
        match (self, o) {
            (Tri::False, _) | (_, Tri::False) => Tri::False,
            (Tri::True, Tri::True) => Tri::True,
            _ => Tri::Unknown,
        }
    }

    fn or(self, o: Tri) -> Tri {
        match (self, o) {
            (Tri::True, _) | (_, Tri::True) => Tri::True,
            (Tri::False, Tri::False) => Tri::False,
            _ => Tri::Unknown,
        }
    }

    fn of(b: bool) -> Tri {
        if b { Tri::True } else { Tri::False }
    }
}

/// 字句を空白を飛ばして読む口（1 つ先を見る）。
struct Cur<'s, 'b> {
    s: &'s [u8],
    lex: Lexer<'s, 'b>,
    peeked: Option<Token>,
}

impl Cur<'_, '_> {
    fn peek(&mut self) -> Result<Token, Error> {
        if let Some(t) = self.peeked {
            return Ok(t);
        }
        loop {
            let t = self.lex.next()?;
            if t.kind != Kind::Whitespace {
                self.peeked = Some(t);
                return Ok(t);
            }
        }
    }

    fn next(&mut self) -> Result<Token, Error> {
        let t = self.peek()?;
        self.peeked = None;
        Ok(t)
    }

    /// 読む範囲の終わり。
    fn lex_end(&self) -> u32 {
        self.s.len() as u32
    }

    fn ident(&mut self, w: &[u8]) -> Result<bool, Error> {
        let t = self.peek()?;
        if t.kind == Kind::Ident && ident_is(self.s, t.inner, t.escaped, w) {
            self.next()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// かっこの中身を `)` まで読み飛ばし、中身の範囲を返す（`(` は読んだあと）。
    fn skip_parens(&mut self) -> Result<Span, Error> {
        let start = self.peek()?.span.start;
        let mut end = start;
        let mut depth: u32 = 0;
        loop {
            let t = self.next()?;
            match t.kind {
                Kind::Eof => return Ok(Span { start, end }),
                Kind::RParen if depth == 0 => return Ok(Span { start, end }),
                Kind::LParen | Kind::Function | Kind::LBracket | Kind::LBrace => depth = depth.saturating_add(1),
                Kind::RParen | Kind::RBracket | Kind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            end = t.span.end;
        }
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 条件の並べ方：`not x`・`x and y and …`・`x or y or …`。かっこの中は積み上げで読む。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Frame {
    /// これまでの値と、つなぎ方（0 まだ、1 and、2 or）と、頭の not
    acc: Tri,
    op: u8,
    negate: bool,
    started: bool,
    /// この段で使ったつなぎ（0 まだ、1 and、2 or。and と or を同じ段に混ぜるのは誤り）
    chain: u8,
    pad: [u8; 3],
}

impl Plain for Frame {}

/// 条件の 1 つ（かっこの中の特徴・宣言・`selector()`）を照らす。`media` なら媒体の特徴、偽なら `@supports`。
type Leaf<'f> = &'f mut dyn FnMut(&mut Cur<'_, '_>, Span) -> Result<Tri, Error>;

/// `not`・`and`・`or` とかっこの条件を読む（かっこの中身が条件か特徴かは `leaf` が見分ける前に、`(` のあとが `not`・`(` で
/// 始まるかで決める）。
fn condition(
    c: &mut Cur<'_, '_>,
    budget: &Budget<'_>,
    leaf: Leaf<'_>,
    ns: NsTable<'_>,
    stop_at_comma: bool,
) -> Result<Tri, Error> {
    let mut stack: MVec<'_, Frame> = MVec::new(budget_ref(budget));
    stack.push(Frame { acc: Tri::True, op: 0, negate: false, started: false, chain: 0, pad: [0; 3] })?;
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = c.peek()?;
        let Some(mut f) = stack.last().copied() else { return Ok(Tri::Unknown) };
        // 1 つの項
        let term: Option<Tri> = if !f.started || f.op != 0 || f.negate {
            if !f.started && c.ident(b"not")? {
                f.negate = true;
                if let Some(x) = stack.last_mut() {
                    *x = f;
                }
                continue;
            }
            match t.kind {
                Kind::LParen => {
                    c.next()?;
                    // かっこの中が条件（`(`・`not` で始まる）なら入れ子の段、そうでなければ葉
                    let n = c.peek()?;
                    if n.kind == Kind::LParen || (n.kind == Kind::Ident && ident_is(c.s, n.inner, n.escaped, b"not")) {
                        stack.push(Frame {
                            acc: Tri::True,
                            op: 0,
                            negate: false,
                            started: false,
                            chain: 0,
                            pad: [0; 3],
                        })?;
                        continue;
                    }
                    let inner = c.skip_parens()?;
                    Some(leaf(c, inner)?)
                }
                Kind::Function => {
                    // `selector(…)` などの関数（葉に渡す。知らない関数は分からない）
                    c.next()?;
                    let inner = c.skip_parens()?;
                    Some(function_leaf(c.s, t, inner, ns, budget)?)
                }
                _ => None,
            }
        } else {
            None
        };
        if let Some(mut v) = term {
            if f.negate {
                v = v.not();
                f.negate = false;
            }
            f.acc = match (f.started, f.op) {
                (false, _) => v,
                (true, 1) => f.acc.and(v),
                (true, 2) => f.acc.or(v),
                _ => Tri::Unknown,
            };
            f.started = true;
            f.op = 0;
            if let Some(x) = stack.last_mut() {
                *x = f;
            }
            continue;
        }
        // つなぎ・閉じ・終わり
        let t = c.peek()?;
        let end_here = t.kind == Kind::Eof || (stop_at_comma && t.kind == Kind::Comma);
        if t.kind == Kind::RParen && stack.len() > 1 {
            c.next()?;
            stack.pop();
            let v = if f.started && f.op == 0 { f.acc } else { Tri::Unknown };
            if let Some(p) = stack.last_mut() {
                let mut pv = v;
                if p.negate {
                    pv = pv.not();
                    p.negate = false;
                }
                p.acc = match (p.started, p.op) {
                    (false, _) => pv,
                    (true, 1) => p.acc.and(pv),
                    (true, 2) => p.acc.or(pv),
                    _ => Tri::Unknown,
                };
                p.started = true;
                p.op = 0;
            }
            continue;
        }
        if end_here {
            if stack.len() > 1 || !f.started || f.op != 0 {
                return Ok(Tri::Unknown);
            }
            return Ok(f.acc);
        }
        if f.started && f.op == 0 {
            let op = if c.ident(b"and")? {
                1
            } else if c.ident(b"or")? {
                2
            } else {
                return Ok(Tri::Unknown);
            };
            if f.chain != 0 && f.chain != op {
                return Ok(Tri::Unknown);
            }
            if let Some(x) = stack.last_mut() {
                x.op = op;
                x.chain = op;
            }
            continue;
        }
        return Ok(Tri::Unknown);
    }
}

/// `selector(…)` の葉（`@supports`）。ほかの関数は分からない。
fn function_leaf(s: &[u8], f: Token, inner: Span, ns: NsTable<'_>, budget: &Budget<'_>) -> Result<Tri, Error> {
    if !ident_is(s, f.inner, f.escaped, b"selector") {
        return Ok(Tri::Unknown);
    }
    let mut t = Tables::new(budget_ref(budget));
    Ok(Tri::of(parse_selector_list(s, inner, ns, &mut t, budget)?.is_some()))
}

/// `@media` の問いの並び（`range` は字句の範囲）が真か。空の並びは真。
pub(crate) fn media(s: &[u8], range: Span, env: &Env, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur { s, lex: Lexer::over(s, range, budget), peeked: None };
    if c.peek()?.kind == Kind::Eof {
        return Ok(true);
    }
    let mut any = false;
    loop {
        budget.spend(cost::OP_STEP)?;
        let v = media_query(&mut c, env, budget)?;
        if v == Tri::True {
            any = true;
        }
        // 壊れた問いは、次の `,` まで飛ばして偽（`not all`）
        loop {
            let t = c.peek()?;
            match t.kind {
                Kind::Eof => return Ok(any),
                Kind::Comma => {
                    c.next()?;
                    break;
                }
                Kind::LParen | Kind::Function => {
                    c.next()?;
                    c.skip_parens()?;
                }
                _ => {
                    c.next()?;
                }
            }
        }
    }
}

/// 1 つの問い。
fn media_query(c: &mut Cur<'_, '_>, env: &Env, budget: &Budget<'_>) -> Result<Tri, Error> {
    let t = c.peek()?;
    let mut leaf = |cur: &mut Cur<'_, '_>, inner: Span| feature(cur.s, inner, env, budget);
    // 媒体の種類の形か（`not screen` と `not (…)` は `not` のあとの字句で見分ける。2 つ先は字句を読み直して見る）
    let type_form = if t.kind != Kind::Ident {
        false
    } else if ident_is(c.s, t.inner, t.escaped, b"not") {
        let end = c.lex_end();
        let mut probe = Lexer::over(c.s, Span { start: t.span.end, end }, budget_ref(budget));
        let mut n = probe.next()?;
        while n.kind == Kind::Whitespace {
            n = probe.next()?;
        }
        n.kind == Kind::Ident
    } else {
        true
    };
    if type_form {
        // [not | only]? 媒体の種類 [and 条件]?
        let negate = c.ident(b"not")?;
        if !negate {
            c.ident(b"only")?;
        }
        let ty = c.next()?;
        if ty.kind != Kind::Ident {
            return Ok(Tri::Unknown);
        }
        let is = |w: &[u8]| ident_is(c.s, ty.inner, ty.escaped, w);
        // `and`・`or`・`not`・`only` は媒体の種類に書けない
        if is(b"and") || is(b"or") || is(b"not") || is(b"only") {
            return Ok(Tri::Unknown);
        }
        let mut v = Tri::of(is(b"screen") || is(b"all"));
        let n = c.peek()?;
        if n.kind == Kind::Ident && ident_is(c.s, n.inner, n.escaped, b"and") {
            c.next()?;
            let rest = condition(c, budget, &mut leaf, NsTable::EMPTY, true)?;
            v = v.and(rest);
        } else if !matches!(n.kind, Kind::Eof | Kind::Comma) {
            return Ok(Tri::Unknown);
        }
        // `not` は問い全体を逆にする（分からないものは偽のまま）
        return Ok(if negate {
            match v {
                Tri::Unknown => Tri::False,
                x => x.not(),
            }
        } else {
            v
        });
    }
    let v = condition(c, budget, &mut leaf, NsTable::EMPTY, true)?;
    Ok(if v == Tri::Unknown { Tri::False } else { v })
}

/// 媒体の特徴（かっこの中身）。
fn feature(s: &[u8], inner: Span, env: &Env, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur { s, lex: Lexer::over(s, inner, budget), peeked: None };
    let first = c.next()?;
    // 範囲の形（`width >= 10px`・`10px <= width <= 20px`。`<=`・`>=` は間に空白を書けない）
    let cmp = |c: &mut Cur<'_, '_>| -> Result<Option<(u8, bool)>, Error> {
        let t = c.peek()?;
        let op = if t.is_delim(b'<') {
            b'<'
        } else if t.is_delim(b'>') {
            b'>'
        } else if t.is_delim(b'=') {
            b'='
        } else {
            return Ok(None);
        };
        c.next()?;
        let mut eq = op == b'=';
        let n = c.peek()?;
        if op != b'=' && n.is_delim(b'=') && n.span.start == t.span.end {
            c.next()?;
            eq = true;
        }
        Ok(Some((op, eq)))
    };
    if first.kind == Kind::Ident {
        let name_tok = first;
        let t = c.peek()?;
        if t.kind == Kind::Eof {
            // 真か偽の形（値のない特徴）
            return Ok(boolean_feature(s, name_tok, env));
        }
        if t.kind == Kind::Colon {
            c.next()?;
            let v = c.next()?;
            if c.peek()?.kind != Kind::Eof {
                return Ok(Tri::Unknown);
            }
            return Ok(value_feature(s, name_tok, v, env));
        }
        if let Some((op, eq)) = cmp(&mut c)? {
            let v = c.next()?;
            if c.peek()?.kind != Kind::Eof {
                return Ok(Tri::Unknown);
            }
            return Ok(range_feature(s, name_tok, op, eq, v, false, env));
        }
        return Ok(Tri::Unknown);
    }
    // `値 < 名前 [< 値]`
    let Some((op1, eq1)) = cmp(&mut c)? else { return Ok(Tri::Unknown) };
    let name_tok = c.next()?;
    if name_tok.kind != Kind::Ident {
        return Ok(Tri::Unknown);
    }
    let mut r = range_feature(s, name_tok, op1, eq1, first, true, env);
    if let Some((op2, eq2)) = cmp(&mut c)? {
        let v2 = c.next()?;
        r = r.and(range_feature(s, name_tok, op2, eq2, v2, false, env));
    }
    if c.peek()?.kind != Kind::Eof {
        return Ok(Tri::Unknown);
    }
    Ok(r)
}

/// 特徴の今の値（数か、言葉の比べ方）。
enum Val {
    Num(f64),
    Word(&'static [u8]),
    Unknown,
}

fn env_value(s: &[u8], name: Token, env: &Env) -> (Val, bool) {
    let is = |w: &[u8]| ident_is(s, name.inner, name.escaped, w);
    // （値、範囲で比べてよいか）
    let size = env.size;
    if is(b"width") {
        return (size.map_or(Val::Unknown, |(w, _)| Val::Num(w)), true);
    }
    if is(b"height") {
        return (size.map_or(Val::Unknown, |(_, h)| Val::Num(h)), true);
    }
    if is(b"aspect-ratio") {
        return (size.map_or(Val::Unknown, |(w, h)| if h > 0.0 { Val::Num(w / h) } else { Val::Unknown }), true);
    }
    if is(b"orientation") {
        return (size.map_or(Val::Unknown, |(w, h)| Val::Word(if h >= w { b"portrait" } else { b"landscape" })), false);
    }
    if is(b"resolution") {
        return (Val::Num(1.0), true);
    }
    if is(b"color") {
        return (Val::Num(8.0), true);
    }
    if is(b"color-index") || is(b"monochrome") || is(b"grid") {
        return (Val::Num(0.0), true);
    }
    let word: &'static [u8] = if is(b"prefers-color-scheme") {
        match env.color_scheme {
            ColorScheme::Dark => b"dark",
            _ => b"light",
        }
    } else if is(b"prefers-reduced-motion")
        || is(b"prefers-reduced-transparency")
        || is(b"prefers-contrast")
        || is(b"prefers-reduced-data")
    {
        b"no-preference"
    } else if is(b"forced-colors")
        || is(b"inverted-colors")
        || is(b"hover")
        || is(b"any-hover")
        || is(b"pointer")
        || is(b"any-pointer")
        || is(b"update")
        || is(b"scripting")
    {
        b"none"
    } else if is(b"dynamic-range") || is(b"video-dynamic-range") {
        b"standard"
    } else if is(b"scan") {
        b"progressive"
    } else if is(b"display-mode") {
        b"browser"
    } else if is(b"color-gamut") {
        b"srgb"
    } else {
        return (Val::Unknown, false);
    };
    (Val::Word(word), false)
}

fn boolean_feature(s: &[u8], name: Token, env: &Env) -> Tri {
    match env_value(s, name, env).0 {
        Val::Num(n) => Tri::of(n != 0.0),
        Val::Word(w) => Tri::of(w != b"none" && w != b"no-preference"),
        Val::Unknown => Tri::Unknown,
    }
}

/// 特徴の値の字句を数にする（長さは px、解像度は dppx、比は `a/b` の頭だけ）。
fn value_num(s: &[u8], v: Token) -> Option<f64> {
    if !v.finite {
        return None;
    }
    match v.kind {
        Kind::Number => Some(v.value),
        Kind::Dimension => {
            let is = |w: &[u8]| ident_is(s, v.inner, v.escaped, w);
            let k = if is(b"px") {
                1.0
            } else if is(b"em") || is(b"rem") {
                16.0
            } else if is(b"in") {
                96.0
            } else if is(b"cm") {
                96.0 / 2.54
            } else if is(b"mm") {
                96.0 / 25.4
            } else if is(b"q") {
                96.0 / 101.6
            } else if is(b"pt") {
                96.0 / 72.0
            } else if is(b"pc") {
                16.0
            } else if is(b"dppx") || is(b"x") {
                1.0
            } else if is(b"dpi") {
                1.0 / 96.0
            } else if is(b"dpcm") {
                2.54 / 96.0
            } else {
                return None;
            };
            Some(v.value * k)
        }
        _ => None,
    }
}

fn value_feature(s: &[u8], name: Token, v: Token, env: &Env) -> Tri {
    // min-・max- の頭
    let raw = name.inner.of(s);
    let (prefix, base_len) = if raw.len() > 4 && raw.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(b"min-")) {
        (Some(b'>'), 4usize)
    } else if raw.len() > 4 && raw.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(b"max-")) {
        (Some(b'<'), 4usize)
    } else {
        (None, 0)
    };
    let base =
        Token { inner: Span { start: name.inner.start.saturating_add(base_len as u32), end: name.inner.end }, ..name };
    let (cur, rangeable) = env_value(s, base, env);
    match (prefix, cur) {
        (Some(_), _) if !rangeable => Tri::Unknown,
        (Some(op), Val::Num(n)) => match value_num(s, v) {
            Some(x) => Tri::of(if op == b'>' { n >= x } else { n <= x }),
            None => Tri::Unknown,
        },
        (None, Val::Num(n)) => match value_num(s, v) {
            Some(x) => Tri::of(n == x),
            None => Tri::Unknown,
        },
        (None, Val::Word(w)) => {
            if v.kind == Kind::Ident {
                Tri::of(ident_is(s, v.inner, v.escaped, w))
            } else {
                Tri::Unknown
            }
        }
        _ => Tri::Unknown,
    }
}

/// 範囲の形の 1 つの比べ（`flip` なら `値 op 名前`）。
fn range_feature(s: &[u8], name: Token, op: u8, eq: bool, v: Token, flip: bool, env: &Env) -> Tri {
    let (cur, rangeable) = env_value(s, name, env);
    let (Val::Num(n), true) = (cur, rangeable) else { return Tri::Unknown };
    let Some(x) = value_num(s, v) else { return Tri::Unknown };
    // 名前 op 値 の形にそろえる
    let (lhs, rhs) = if flip { (x, n) } else { (n, x) };
    Tri::of(match (op, eq) {
        (b'<', false) => lhs < rhs,
        (b'<', true) => lhs <= rhs,
        (b'>', false) => lhs > rhs,
        (b'>', true) => lhs >= rhs,
        _ => lhs == rhs,
    })
}

/// `@supports` の条件が真か。
pub(crate) fn supports(s: &[u8], range: Span, ns: NsTable<'_>, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur { s, lex: Lexer::over(s, range, budget), peeked: None };
    let mut leaf = |cur: &mut Cur<'_, '_>, inner: Span| declaration_leaf(cur.s, inner, budget);
    Ok(condition(&mut c, budget, &mut leaf, ns, false)? == Tri::True)
}

/// `(名前: 値)` が sumie の読める宣言か。
fn declaration_leaf(s: &[u8], inner: Span, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur { s, lex: Lexer::over(s, inner, budget), peeked: None };
    let name = c.next()?;
    if name.kind != Kind::Ident || c.next()?.kind != Kind::Colon {
        return Ok(Tri::Unknown);
    }
    let vstart = c.peek()?.span.start;
    let value = Span { start: vstart, end: inner.end.max(vstart) };
    let id = if name.escaped { None } else { lookup(name.inner.of(s), true) };
    let Some(p) = id.and_then(prop) else { return Ok(Tri::False) };
    if wide(s, value, budget)?.is_some() {
        return Ok(Tri::True);
    }
    Ok(Tri::of(valid(p.kind, s, value, false, budget)?))
}

#[cfg(test)]
mod tests;
