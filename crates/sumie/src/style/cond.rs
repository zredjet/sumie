//! 条件を照らす（`@media`・`<style media>`・`@supports`。ADR-66 (2)・ADR-68 (4)）。
//!
//! - `@media`：Media Queries 4 の問いの並び。媒体の種類は `screen`・`all` が真、ほかは偽。条件は 3 値（知らない形・知らない
//!   特徴は「分からない」で、最後に偽）。環境は決まった値（D1）：明暗は呼び手の設定、`prefers-*` は `no-preference`、
//!   色は 8 ビット、解像度は 1dppx（96dpi）、`hover`・`pointer` は `none`、`update` は `none`。幅・高さ・縦横比・向きは文書の
//!   大きさ（CSS の px。width・height・viewBox で決まるとき）で照らし、決まらなければ偽。長さの em・rem は 16px（初期の値）
//! - `@supports`：`(名前: 値 [!important])` は sumie が読めるプロパティと形か、`selector()` は sumie が読めるセレクタか。
//!   `not`・`and`・`or`。知らない形（general-enclosed）は偽（CSS Conditional 3 §6.1）。sumie が形を確かめずに受ける値
//!   （`Raw` のプロパティなど）の宣言は分からない（3 値のまま返す。分からない条件の規則が要素に効けば、木を作る段が
//!   NotYet で断る。単位 3 の 8 回目のレビュー）
//! - 書き方は仕様のとおりに確かめる：`not` の段には `and`・`or` を書けない、`and` と `or` を同じ段に混ぜない、媒体の種類の
//!   あとには `or` を書けない。特徴の値は特徴ごとの形（長さ・解像度・比・整数）だけ。範囲の形の 2 つの比べは同じ向き
//! - 再帰しない（入れ子のかっこは明示の積み上げ）

use crate::budget::Budget;
use crate::cost;
use crate::css::decl::{Item, parse_declarations};
use crate::css::lex::{Kind, Lexer, Nest, Token, ident_is};
use crate::css::select::{NsTable, Tables, parse_selector_list};
use crate::error::Error;
use crate::mem::{MVec, Plain};
use crate::style::ColorScheme;
use crate::style::cascade::decl_prop;
use crate::style::props::prop;
use crate::style::valid::{Check, Ctx, check, unread, wide};
use crate::xml::lex::Span;

/// 照らす環境。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Env {
    pub(crate) color_scheme: ColorScheme,
    /// 文書の大きさ（CSS の px）。中身で決まる文書は None
    pub(crate) size: Option<(f64, f64)>,
    /// 初期の文字の大きさ（px。em・ex などの基準。`StyleDefaults.font_size`。Media Queries 4 §1.3）
    pub(crate) font_size: f64,
}

/// 3 値。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tri {
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
    /// 2 つ先の字句（`not` のあとを見るとき）
    peeked2: Option<Token>,
}

impl<'s, 'b> Cur<'s, 'b> {
    fn new(s: &'s [u8], range: Span, budget: &'b Budget<'b>) -> Cur<'s, 'b> {
        Cur { s, lex: Lexer::over(s, range, budget), peeked: None, peeked2: None }
    }

    /// 空白でない次の字句を字句の口から読む。
    fn raw_next(&mut self) -> Result<Token, Error> {
        loop {
            let t = self.lex.next()?;
            if t.kind != Kind::Whitespace {
                return Ok(t);
            }
        }
    }

    fn peek(&mut self) -> Result<Token, Error> {
        if let Some(t) = self.peeked {
            return Ok(t);
        }
        let t = self.raw_next()?;
        self.peeked = Some(t);
        Ok(t)
    }

    /// 2 つ先の空白でない字句を見る（同じ字句の口で読む。読み直さない）。
    fn peek2(&mut self) -> Result<Token, Error> {
        self.peek()?;
        if let Some(t) = self.peeked2 {
            return Ok(t);
        }
        let t = self.raw_next()?;
        self.peeked2 = Some(t);
        Ok(t)
    }

    fn next(&mut self) -> Result<Token, Error> {
        let t = self.peek()?;
        self.peeked = self.peeked2.take();
        Ok(t)
    }

    fn ident(&mut self, w: &[u8]) -> Result<bool, Error> {
        let t = self.peek()?;
        if t.kind == Kind::Ident && ident_is(self.s, t.inner, t.escaped, w) {
            self.next()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// かっこの中身を組の `)` まで読み飛ばし、中身の範囲を返す（`(` は読んだあと。かっこは種類ごとに組で）。
    fn skip_parens(&mut self) -> Result<Span, Error> {
        let start = self.peek()?.span.start;
        let mut end = start;
        let mut nest = Nest::new(self.lex.budget());
        loop {
            let t = self.next()?;
            match t.kind {
                Kind::Eof => return Ok(Span { start, end }),
                Kind::RParen if nest.top() => return Ok(Span { start, end }),
                _ => {}
            }
            nest.feed(&t)?;
            end = t.span.end;
        }
    }

    /// 読んだ開きの字句 `open` の閉じまで読み飛ばす（閉じないまま終われば終わりまで）。
    fn skip_to_close(&mut self, open: &Token) -> Result<(), Error> {
        let mut nest = Nest::new(self.lex.budget());
        nest.feed(open)?;
        loop {
            let t = self.next()?;
            if t.kind == Kind::Eof || nest.feed(&t)? {
                return Ok(());
            }
        }
    }

    /// 開いたままの `depth` 個のかっこを閉じるまで読み飛ばす（壊れた条件の残り。中の `,` で次の問いを始めない）。
    fn close_open(&mut self, depth: usize) -> Result<(), Error> {
        let mut need = depth;
        let mut nest = Nest::new(self.lex.budget());
        while need > 0 {
            let t = self.next()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::RParen if nest.top() => need = need.saturating_sub(1),
                _ => {
                    nest.feed(&t)?;
                }
            }
        }
        Ok(())
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
    /// この段は `not` で始まった（`and`・`or` を書けない）
    negated: bool,
    pad: [u8; 2],
}

impl Plain for Frame {}

/// 条件の 1 つ（かっこの中の特徴・宣言・`selector()`）を照らす。`media` なら媒体の特徴、偽なら `@supports`。
type Leaf<'f> = &'f mut dyn FnMut(&mut Cur<'_, '_>, Span) -> Result<Tri, Error>;

/// `not`・`and`・`or` とかっこの条件を読む（かっこの中身が条件か特徴かは `leaf` が見分ける前に、`(` のあとが `not`・`(` で
/// 始まるかで決める）。
/// 条件の読み方。
#[derive(Clone, Copy)]
struct Mode<'a> {
    ns: NsTable<'a>,
    /// 問いの並びの `,` で止まる（`@media`）
    stop_at_comma: bool,
    /// いちばん外の段に `or` を書けない（媒体の種類のあとの条件）
    without_or: bool,
    /// 知らない形（general-enclosed）の値（`@media` は分からない、`@supports` は偽）
    general: Tri,
}

fn condition(c: &mut Cur<'_, '_>, budget: &Budget<'_>, leaf: Leaf<'_>, mode: Mode<'_>) -> Result<Tri, Error> {
    let stop_at_comma = mode.stop_at_comma;
    let mut stack: MVec<'_, Frame> = MVec::new(budget_ref(budget));
    stack.push(Frame {
        acc: Tri::True,
        op: 0,
        negate: false,
        started: false,
        chain: 0,
        negated: false,
        pad: [0; 2],
    })?;
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = c.peek()?;
        let Some(mut f) = stack.last().copied() else { return Ok(Tri::Unknown) };
        // 1 つの項
        let term: Option<Tri> = if !f.started || f.op != 0 || f.negate {
            if !f.started && !f.negate && c.ident(b"not")? {
                f.negate = true;
                f.negated = true;
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
                    // `(selector(…))` のように、かっこの中の関数も条件
                    if n.kind == Kind::LParen
                        || n.kind == Kind::Function
                        || (n.kind == Kind::Ident && ident_is(c.s, n.inner, n.escaped, b"not"))
                    {
                        stack.push(Frame {
                            acc: Tri::True,
                            op: 0,
                            negate: false,
                            started: false,
                            chain: 0,
                            negated: false,
                            pad: [0; 2],
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
                    Some(function_leaf(c.s, t, inner, mode, budget)?)
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
        // 入れ子のかっこが条件として読めなければ、そのかっこは知らない形（general-enclosed）として外の段に渡す
        let fallback = |c: &mut Cur<'_, '_>, stack: &mut MVec<'_, Frame>| -> Result<bool, Error> {
            if stack.len() < 2 {
                return Ok(false);
            }
            c.close_open(1)?;
            stack.pop();
            if let Some(p) = stack.last_mut() {
                apply(p, mode.general);
            }
            Ok(true)
        };
        // 入れ子のかっこの中の `,` は問いの区切りでない：そのかっこは知らない形（単位 3 の 10 回目のレビュー）
        if stop_at_comma && t.kind == Kind::Comma && stack.len() > 1 && fallback(c, &mut stack)? {
            continue;
        }
        let end_here = t.kind == Kind::Eof || (stop_at_comma && t.kind == Kind::Comma);
        if t.kind == Kind::RParen && stack.len() > 1 {
            c.next()?;
            stack.pop();
            // 閉じたかっこの中が条件として読めなければ、知らない形
            let v = if f.started && f.op == 0 && !f.negate { f.acc } else { mode.general };
            if let Some(p) = stack.last_mut() {
                apply(p, v);
            }
            continue;
        }
        let depth = stack.len().saturating_sub(1);
        if end_here {
            // 閉じないまま終わったかっこは閉じたものとする（CSS Syntax。単位 3 の 9 回目のレビュー）
            while stack.len() > 1 {
                budget.spend(cost::OP_STEP)?;
                let Some(g) = stack.pop() else { break };
                let v = if g.started && g.op == 0 && !g.negate { g.acc } else { mode.general };
                if let Some(p) = stack.last_mut() {
                    apply(p, v);
                }
            }
            let Some(f) = stack.last().copied() else { return Ok(Tri::Unknown) };
            if !f.started || f.op != 0 {
                return Ok(Tri::Unknown);
            }
            return Ok(f.acc);
        }
        if f.started && f.op == 0 && !f.negated {
            let op = if c.ident(b"and")? {
                1
            } else if c.ident(b"or")? {
                2
            } else {
                if fallback(c, &mut stack)? {
                    continue;
                }
                c.close_open(depth)?;
                return Ok(Tri::Unknown);
            };
            if (f.chain != 0 && f.chain != op) || (op == 2 && mode.without_or && stack.len() == 1) {
                if fallback(c, &mut stack)? {
                    continue;
                }
                c.close_open(depth)?;
                return Ok(Tri::Unknown);
            }
            if let Some(x) = stack.last_mut() {
                x.op = op;
                x.chain = op;
            }
            continue;
        }
        if fallback(c, &mut stack)? {
            continue;
        }
        c.close_open(depth)?;
        return Ok(Tri::Unknown);
    }
}

/// 段 `p` に項の値 `v` を足す（頭の `not` を当て、つなぎで合わせる）。
fn apply(p: &mut Frame, v: Tri) {
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

/// `selector(…)` の葉（`@supports`）。`font-tech(…)`・`font-format(…)` は分からない（sumie はフォントの技術を照らさない）。
/// ほかの関数は知らない形。
fn function_leaf(s: &[u8], f: Token, inner: Span, mode: Mode<'_>, budget: &Budget<'_>) -> Result<Tri, Error> {
    let is = |w: &[u8]| ident_is(s, f.inner, f.escaped, w);
    if mode.general == Tri::False && (is(b"font-tech") || is(b"font-format")) {
        return Ok(Tri::Unknown);
    }
    if !is(b"selector") {
        return Ok(mode.general);
    }
    // 1 つの複雑なセレクタ（CSS Conditional 4）。当てないセレクタ（`:has()` など）は読めないので分からない（Chrome は
    // 読む。単位 3 の 9 回目のレビュー）
    let mut t = Tables::new(budget_ref(budget));
    Ok(match parse_selector_list(s, inner, mode.ns, &mut t, budget)? {
        // 寛容な並びで捨てた選択子があれば支えない（Chrome と同じ。単位 3 の 10 回目のレビュー）
        Some(p) if p.dropped => Tri::False,
        Some(p) if p.unsupported => Tri::Unknown,
        Some(p) => Tri::of(t.selectors.get(p.first as usize).is_some_and(|x| x.next == crate::css::select::NONE)),
        None => Tri::False,
    })
}

/// `@media` の条件の読み方。
const fn media_mode(without_or: bool) -> Mode<'static> {
    Mode { ns: NsTable::EMPTY, stop_at_comma: true, without_or, general: Tri::Unknown }
}

/// `@media` の問いの並び（`range` は字句の範囲）が真か。空の並びは真。
pub(crate) fn media(s: &[u8], range: Span, env: &Env, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur::new(s, range, budget);
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
                // `[…]`・`{…}` の中の `,` でも次の問いを始めない（Chrome と同じ）
                Kind::LBracket | Kind::LBrace => {
                    let open = c.next()?;
                    c.skip_to_close(&open)?;
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
        c.peek2()?.kind == Kind::Ident
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
            let rest = condition(c, budget, &mut leaf, media_mode(true))?;
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
    let v = condition(c, budget, &mut leaf, media_mode(false))?;
    Ok(if v == Tri::Unknown { Tri::False } else { v })
}

/// 媒体の特徴（かっこの中身）。
fn feature(s: &[u8], inner: Span, env: &Env, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur::new(s, inner, budget);
    let first = c.next()?;
    if first.kind == Kind::Ident {
        let name_tok = first;
        let t = c.peek()?;
        if t.kind == Kind::Eof {
            // 真か偽の形（値のない特徴）
            return Ok(boolean_feature(s, name_tok, env));
        }
        if t.kind == Kind::Colon {
            c.next()?;
            return value_feature(&mut c, name_tok, env);
        }
        if let Some((op, eq)) = cmp(&mut c)? {
            let Some((cur, ty)) = env_value(s, name_tok, env) else { return Ok(Tri::Unknown) };
            let Some(x) = read_value(&mut c, ty, None, env)? else { return Ok(Tri::Unknown) };
            if c.peek()?.kind != Kind::Eof {
                return Ok(Tri::Unknown);
            }
            return Ok(compare(cur, op, eq, x, false));
        }
        return Ok(Tri::Unknown);
    }
    // `値 op 名前 [op 値]`（2 つの比べは同じ向き。`=` は 1 つの比べだけ）
    let lead = first;
    let ratio_rest = if c.peek()?.is_delim(b'/') {
        c.next()?;
        Some(c.next()?)
    } else {
        None
    };
    let Some((op1, eq1)) = cmp(&mut c)? else { return Ok(Tri::Unknown) };
    let name_tok = c.next()?;
    if name_tok.kind != Kind::Ident {
        return Ok(Tri::Unknown);
    }
    let Some((cur, ty)) = env_value(s, name_tok, env) else { return Ok(Tri::Unknown) };
    let Some(x1) = value_of(s, ty, lead, ratio_rest, env) else { return Ok(Tri::Unknown) };
    let mut r = compare(cur, op1, eq1, x1, true);
    if let Some((op2, eq2)) = cmp(&mut c)? {
        if op1 == b'=' || op2 != op1 {
            return Ok(Tri::Unknown);
        }
        let Some(x2) = read_value(&mut c, ty, None, env)? else { return Ok(Tri::Unknown) };
        r = r.and(compare(cur, op2, eq2, x2, false));
    }
    if c.peek()?.kind != Kind::Eof {
        return Ok(Tri::Unknown);
    }
    Ok(r)
}

/// 範囲の形の比べ（`<`・`>`・`=`。`<=`・`>=` は間に空白を書けない）。
fn cmp(c: &mut Cur<'_, '_>) -> Result<Option<(u8, bool)>, Error> {
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
}

/// 特徴の値の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Length,
    Resolution,
    Ratio,
    Integer,
    /// 数（`-webkit-device-pixel-ratio`）
    Number,
    Word,
}

/// 特徴の今の値（数か、言葉の比べ方）。言葉の特徴は書ける言葉の並びも持つ（知らない言葉は分からない）。
#[derive(Clone, Copy)]
enum Val {
    Num(f64),
    Word(&'static [u8], &'static [&'static [u8]]),
    Unknown,
}

const NO_PREF: &[&[u8]] = &[b"no-preference", b"reduce"];

/// 特徴の今の値と値の形（知らない特徴は None）。
fn env_value(s: &[u8], name: Token, env: &Env) -> Option<(Val, Ty)> {
    let is = |w: &[u8]| ident_is(s, name.inner, name.escaped, w);
    let size = env.size;
    if is(b"width") {
        return Some((size.map_or(Val::Unknown, |(w, _)| Val::Num(w)), Ty::Length));
    }
    if is(b"height") {
        return Some((size.map_or(Val::Unknown, |(_, h)| Val::Num(h)), Ty::Length));
    }
    if is(b"aspect-ratio") {
        return Some((
            size.map_or(Val::Unknown, |(w, h)| if h > 0.0 { Val::Num(w / h) } else { Val::Unknown }),
            Ty::Ratio,
        ));
    }
    if is(b"orientation") {
        const W: &[&[u8]] = &[b"portrait", b"landscape"];
        return Some((
            size.map_or(Val::Unknown, |(w, h)| Val::Word(if h >= w { b"portrait" } else { b"landscape" }, W)),
            Ty::Word,
        ));
    }
    if is(b"resolution") {
        return Some((Val::Num(1.0), Ty::Resolution));
    }
    if is(b"-webkit-device-pixel-ratio") {
        return Some((Val::Num(1.0), Ty::Number));
    }
    if is(b"color") {
        return Some((Val::Num(8.0), Ty::Integer));
    }
    if is(b"color-index") || is(b"monochrome") || is(b"grid") {
        return Some((Val::Num(0.0), Ty::Integer));
    }
    let (word, set): (&'static [u8], &'static [&'static [u8]]) = if is(b"prefers-color-scheme") {
        let w: &'static [u8] = match env.color_scheme {
            ColorScheme::Dark => b"dark",
            _ => b"light",
        };
        (w, &[b"light", b"dark"])
    } else if is(b"prefers-reduced-motion") || is(b"prefers-reduced-transparency") || is(b"prefers-reduced-data") {
        (b"no-preference", NO_PREF)
    } else if is(b"prefers-contrast") {
        (b"no-preference", &[b"no-preference", b"more", b"less", b"custom"])
    } else if is(b"forced-colors") {
        (b"none", &[b"none", b"active"])
    } else if is(b"hover") || is(b"any-hover") {
        (b"none", &[b"none", b"hover"])
    } else if is(b"pointer") || is(b"any-pointer") {
        (b"none", &[b"none", b"coarse", b"fine"])
    } else if is(b"update") {
        (b"none", &[b"none", b"slow", b"fast"])
    } else if is(b"scripting") {
        (b"none", &[b"none", b"initial-only", b"enabled"])
    } else if is(b"dynamic-range") || is(b"video-dynamic-range") {
        (b"standard", &[b"standard", b"high"])
    } else if is(b"scan") {
        (b"progressive", &[b"interlace", b"progressive"])
    } else if is(b"display-mode") {
        (b"browser", &[b"fullscreen", b"standalone", b"minimal-ui", b"browser", b"picture-in-picture"])
    } else if is(b"color-gamut") {
        (b"srgb", &[b"srgb", b"p3", b"rec2020"])
    } else if is(b"overflow-block") {
        (b"scroll", &[b"none", b"scroll", b"paged"])
    } else if is(b"overflow-inline") {
        (b"scroll", &[b"none", b"scroll"])
    } else {
        return None;
    };
    Some((Val::Word(word, set), Ty::Word))
}

fn boolean_feature(s: &[u8], name: Token, env: &Env) -> Tri {
    match env_value(s, name, env).map(|x| x.0) {
        Some(Val::Num(n)) => Tri::of(n != 0.0),
        Some(Val::Word(w, _)) => Tri::of(w != b"none" && w != b"no-preference"),
        _ => Tri::Unknown,
    }
}

/// 値の字句を特徴の形で数にする（長さは px（em・rem は 16px）、解像度は dppx、比は `a / b`）。`ratio_rest` は比の `/` の
/// あとの字句。
fn value_of(s: &[u8], ty: Ty, v: Token, ratio_rest: Option<Token>, env: &Env) -> Option<f64> {
    if !v.finite || (ratio_rest.is_some() && ty != Ty::Ratio) {
        return None;
    }
    let is = |w: &[u8]| ident_is(s, v.inner, v.escaped, w);
    match ty {
        Ty::Length => match v.kind {
            // 単位のない数は 0 だけ
            Kind::Number if v.value == 0.0 => Some(0.0),
            Kind::Dimension => {
                let fs = env.font_size;
                let (vw, vh) = env.size.unwrap_or((f64::NAN, f64::NAN));
                let k = if is(b"px") {
                    1.0
                } else if is(b"em") || is(b"rem") || is(b"ic") {
                    fs
                } else if is(b"ex") || is(b"ch") {
                    fs * 0.5
                } else if is(b"cap") {
                    fs * 0.7
                } else if is(b"vw") || is(b"vi") {
                    vw / 100.0
                } else if is(b"vh") || is(b"vb") {
                    vh / 100.0
                } else if is(b"vmin") {
                    math_min(vw, vh) / 100.0
                } else if is(b"vmax") {
                    math_max(vw, vh) / 100.0
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
                } else {
                    return None;
                };
                Some(v.value * k).filter(|x| x.is_finite())
            }
            _ => None,
        },
        Ty::Resolution => {
            if v.kind != Kind::Dimension {
                return None;
            }
            let k = if is(b"dppx") || is(b"x") {
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
        Ty::Integer => (v.kind == Kind::Number && v.flag).then_some(v.value),
        Ty::Number => (v.kind == Kind::Number).then_some(v.value),
        Ty::Ratio => {
            if v.kind != Kind::Number || v.value < 0.0 {
                return None;
            }
            match ratio_rest {
                None => Some(v.value),
                Some(b) if b.kind == Kind::Number && b.finite && b.value > 0.0 => {
                    Some(v.value / b.value).filter(|x| x.is_finite())
                }
                _ => None,
            }
        }
        Ty::Word => None,
    }
}

/// 浮動小数の小さい方・大きい方（NaN は表示の領域の大きさがないとき。どちらかが NaN なら NaN）。
fn math_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { crate::math::min(a, b) }
}

fn math_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { crate::math::max(a, b) }
}

/// 値を読む（比は `a / b` の 3 つの字句）。
fn read_value(c: &mut Cur<'_, '_>, ty: Ty, first: Option<Token>, env: &Env) -> Result<Option<f64>, Error> {
    let v = match first {
        Some(t) => t,
        None => c.next()?,
    };
    let rest = if ty == Ty::Ratio && c.peek()?.is_delim(b'/') {
        c.next()?;
        Some(c.next()?)
    } else {
        None
    };
    Ok(value_of(c.s, ty, v, rest, env))
}

/// `名前: 値`（`min-`・`max-` の頭は範囲の比べ）。
fn value_feature(c: &mut Cur<'_, '_>, name: Token, env: &Env) -> Result<Tri, Error> {
    let s = c.s;
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
    // `-webkit-min-device-pixel-ratio`・`-webkit-max-device-pixel-ratio`（Chrome が読む。頭の部分の中に min・max がある）
    let webkit = |w: &[u8]| ident_is(s, name.inner, name.escaped, w);
    let (prefix, found) = if webkit(b"-webkit-min-device-pixel-ratio") {
        (Some(b'>'), Some((Val::Num(1.0), Ty::Number)))
    } else if webkit(b"-webkit-max-device-pixel-ratio") {
        (Some(b'<'), Some((Val::Num(1.0), Ty::Number)))
    } else {
        (prefix, env_value(s, base, env))
    };
    let Some((cur, ty)) = found else { return Ok(Tri::Unknown) };
    if ty == Ty::Word {
        if prefix.is_some() {
            return Ok(Tri::Unknown);
        }
        let v = c.next()?;
        if c.peek()?.kind != Kind::Eof || v.kind != Kind::Ident {
            return Ok(Tri::Unknown);
        }
        return Ok(match cur {
            // 書ける言葉でなければ分からない
            Val::Word(w, set) if set.iter().any(|x| ident_is(s, v.inner, v.escaped, x)) => {
                Tri::of(ident_is(s, v.inner, v.escaped, w))
            }
            _ => Tri::Unknown,
        });
    }
    let Some(x) = read_value(c, ty, None, env)? else { return Ok(Tri::Unknown) };
    if c.peek()?.kind != Kind::Eof {
        return Ok(Tri::Unknown);
    }
    Ok(match prefix {
        Some(op) => compare(cur, op, true, x, false),
        None => compare(cur, b'=', true, x, false),
    })
}

/// 今の値 `cur` と `x` を比べる（`flip` なら `x op 今の値`）。今の値が分からなければ分からない。
fn compare(cur: Val, op: u8, eq: bool, x: f64, flip: bool) -> Tri {
    let Val::Num(n) = cur else { return Tri::Unknown };
    let (lhs, rhs) = if flip { (x, n) } else { (n, x) };
    Tri::of(match (op, eq) {
        (b'<', false) => lhs < rhs,
        (b'<', true) => lhs <= rhs,
        (b'>', false) => lhs > rhs,
        (b'>', true) => lhs >= rhs,
        _ => lhs == rhs,
    })
}

/// `@supports` の条件の値（sumie が形を確かめずに受ける値の宣言は分からない：[`Check::Unsure`]。単位 3 の 8 回目の
/// レビュー）。
pub(crate) fn supports(s: &[u8], range: Span, ns: NsTable<'_>, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur::new(s, range, budget);
    let mut leaf = |cur: &mut Cur<'_, '_>, inner: Span| declaration_leaf(cur.s, inner, budget);
    let mode = Mode { ns, stop_at_comma: false, without_or: false, general: Tri::False };
    condition(&mut c, budget, &mut leaf, mode)
}

/// `@import` の `supports(…)` の中身（`<supports-condition>` か 1 つの宣言）の値（CSS Cascade 5 §2.1）。名前と `:` で始まれば
/// 宣言、そうでなければ条件で、条件の形でなければ偽（書き方の誤りを分からないにしない。単位 3 の 9 回目のレビュー）。
pub(crate) fn supports_import(s: &[u8], range: Span, ns: NsTable<'_>, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut c = Cur::new(s, range, budget);
    if c.peek()?.kind == Kind::Ident && c.peek2()?.kind == Kind::Colon {
        return declaration_leaf(s, range, budget);
    }
    if !crate::css::sheet::supports_shape(s, range, budget)? {
        return Ok(Tri::False);
    }
    supports(s, range, ns, budget)
}

/// `(名前: 値 [!important])` が sumie の読める宣言か（名前の escape を解く。宣言でない形は偽）。
fn declaration_leaf(s: &[u8], inner: Span, budget: &Budget<'_>) -> Result<Tri, Error> {
    let budget = budget_ref(budget);
    let mut lex = Lexer::over(s, inner, budget);
    let mut first = None;
    let mut count = 0u32;
    parse_declarations(&mut lex, s, false, budget, &mut |it| {
        if let Item::Decl(d) = it {
            count = count.saturating_add(1);
            if first.is_none() {
                first = Some(d);
            }
        }
        Ok(())
    })?;
    let (Some(d), 1) = (first, count) else { return Ok(Tri::False) };
    // 宣言は 1 つだけで、深さ 0 の `;` は書けない（`(fill: red;)` は偽。Chrome と同じ）
    let mut lex = Lexer::over(s, inner, budget);
    let mut nest = Nest::new(budget);
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Eof => break,
            Kind::Semicolon if nest.top() => return Ok(Tri::False),
            _ => {}
        }
        nest.feed(&t)?;
    }
    // カスタムプロパティ（`--` で始まる名前）はどの値も受ける
    if d.name.of(s).starts_with(b"--") {
        return Ok(Tri::True);
    }
    // sumie の知らないプロパティは偽（Chrome の知る名前でも。名前の表を持たないので、分からないにすると `(foo: bar)` でも
    // 断る。ADR-107 の既知の差）
    let Some(p) = decl_prop(s, &d, budget)?.and_then(prop) else { return Ok(Tri::False) };
    if wide(s, d.value, budget)?.is_some() {
        return Ok(Tri::True);
    }
    // 形だけを見る（表示の領域の単位も受ける）。確かめずに受ける値と、sumie が読まないので壊れた値にした形（関数・読まない
    // 単位）は分からない
    let cx = Ctx { attr: false, unitless: p.unitless, viewport: true };
    Ok(match check(p.kind, s, d.value, cx, budget)? {
        Check::Bad if unread(s, d.value, budget)? => Tri::Unknown,
        Check::Bad => Tri::False,
        Check::Sure => Tri::True,
        Check::Unsure => Tri::Unknown,
    })
}

#[cfg(test)]
mod tests;
