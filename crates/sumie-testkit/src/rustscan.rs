//! Rust のソースの字句（`tests/rules.rs` の走査と、`sumie-tools gen-disallowed` が std の文書を読むのに使う）。
//!
//! 完全な Rust の字句解析ではないが、注釈・文字列・文字の字句・寿命を見分け、識別子と記号を行の番号つきで返す。
//! 文書の注釈（`///`・`//!`・`/** */`・`/*! */`）は [`Kind::Doc`] として中身を返す。

/// 字句の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 識別子とキーワード（`r#` の生の識別子は `r#` を除いた名前）
    Ident,
    /// 寿命（`'a`。中身は `'` を除いた名前）
    Lifetime,
    /// 文字列（中身は引用符を除いた生の字句。エスケープは解かない）
    Str,
    /// そのほかの字句（数・文字・バイト）
    Literal,
    /// 記号 1 文字
    Punct,
    /// 文書の注釈（`///`・`//!`・`/**`・`/*!`）。中身は印を除いた 1 行または 1 塊
    Doc,
}

/// 字句。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub text: String,
    /// 1 から数える行の番号
    pub line: u32,
}

impl Token {
    /// 識別子 `name` か。
    pub fn is_ident(&self, name: &str) -> bool {
        self.kind == Kind::Ident && self.text == name
    }

    /// 記号 `c` か。
    pub fn is_punct(&self, c: char) -> bool {
        self.kind == Kind::Punct && self.text.len() == c.len_utf8() && self.text.starts_with(c)
    }
}

/// ソースを字句に分ける。壊れた字句（閉じない文字列など）は、残りを 1 つの字句にして終える。
pub fn lex(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    while i < b.len() {
        let c = b[i];
        // 空白
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // 注釈
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = find_byte(b, i, b'\n');
            let body = &src[i..end];
            let doc = (body.starts_with("///") && !body.starts_with("////")) || body.starts_with("//!");
            if doc {
                out.push(Token { kind: Kind::Doc, text: body[3..].to_string(), line });
            }
            i = end;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let start_line = line;
            let mut depth = 0u32;
            let mut j = i;
            while j < b.len() {
                if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if b[j] == b'\n' {
                        line += 1;
                    }
                    j += 1;
                }
            }
            let body = src.get(i..j.min(b.len())).unwrap_or("");
            let doc =
                (body.starts_with("/**") && !body.starts_with("/***") && body != "/**/") || body.starts_with("/*!");
            // 閉じない注釈（`/** あ`）でも文字の境目で切る
            let inner = body.strip_prefix("/**").or_else(|| body.strip_prefix("/*!")).unwrap_or("");
            let inner = inner.strip_suffix("*/").unwrap_or(inner);
            if doc {
                out.push(Token { kind: Kind::Doc, text: inner.to_string(), line: start_line });
            }
            i = j;
            continue;
        }
        // 生の文字列・バイト文字列・C の文字列（r"…"・r#"…"#・br"…"・b"…"・c"…"・cr"…"）
        if let Some((prefix_len, raw)) = string_prefix(b, i) {
            let start_line = line;
            let (end, content) = if raw {
                scan_raw_string(src, i + prefix_len, &mut line)
            } else {
                scan_string(src, i + prefix_len, &mut line)
            };
            out.push(Token { kind: Kind::Str, text: content, line: start_line });
            i = end;
            continue;
        }
        if c == b'"' {
            let start_line = line;
            let (end, content) = scan_string(src, i, &mut line);
            out.push(Token { kind: Kind::Str, text: content, line: start_line });
            i = end;
            continue;
        }
        // バイトの字句 b'x'
        if c == b'b' && b.get(i + 1) == Some(&b'\'') {
            let end = scan_char(b, i + 1);
            out.push(Token { kind: Kind::Literal, text: src[i..end].to_string(), line });
            i = end;
            continue;
        }
        // 文字の字句か寿命
        if c == b'\'' {
            if is_char_literal(src, i) {
                let end = scan_char(b, i);
                out.push(Token { kind: Kind::Literal, text: src[i..end].to_string(), line });
                i = end;
            } else {
                let mut j = i + 1;
                while j < b.len() && is_ident_byte(b[j]) {
                    j += 1;
                }
                if j == i + 1 {
                    out.push(Token { kind: Kind::Punct, text: "'".to_string(), line });
                    i += 1;
                } else {
                    out.push(Token { kind: Kind::Lifetime, text: src[i + 1..j].to_string(), line });
                    i = j;
                }
            }
            continue;
        }
        // 識別子（生の識別子 r#name を含む）
        if is_ident_start(c) {
            let mut start = i;
            if c == b'r' && b.get(i + 1) == Some(&b'#') && b.get(i + 2).is_some_and(|&x| is_ident_start(x)) {
                start = i + 2;
            }
            let mut j = start;
            while j < b.len() && is_ident_byte(b[j]) {
                j += 1;
            }
            out.push(Token { kind: Kind::Ident, text: src[start..j].to_string(), line });
            i = j;
            continue;
        }
        // 数
        if c.is_ascii_digit() {
            let mut j = i + 1;
            let hex = c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X'));
            // 接尾辞（u8・usize・f64 など）が始まったら、指数の符号と小数点を取らない（`5usize-1` は 3 つの字句）
            let mut suffix = false;
            while j < b.len() {
                let d = b[j];
                if d.is_ascii_alphanumeric() || d == b'_' {
                    let exp = !hex && !suffix && (d == b'e' || d == b'E');
                    if d.is_ascii_alphabetic() && !exp && !(hex && d.is_ascii_hexdigit()) && !(hex && j == i + 1) {
                        suffix = true;
                    }
                    j += 1;
                    if exp && matches!(b.get(j), Some(b'+' | b'-')) && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                        j += 1;
                    }
                } else if d == b'.' && !suffix && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                } else {
                    break;
                }
            }
            out.push(Token { kind: Kind::Literal, text: src[i..j].to_string(), line });
            i = j;
            continue;
        }
        // 記号（UTF-8 の 1 文字）
        let len = utf8_len(c);
        let end = (i + len).min(b.len());
        out.push(Token { kind: Kind::Punct, text: src[i..end].to_string(), line });
        i = end;
    }
    out
}

fn find_byte(b: &[u8], from: usize, x: u8) -> usize {
    let mut j = from;
    while j < b.len() && b[j] != x {
        j += 1;
    }
    j
}

fn utf8_len(c: u8) -> usize {
    match c {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// `i` が文字列の接頭辞（r・br・b・c・cr と、生なら #）で始まるなら、引用符または # までの長さと、生かどうか。
fn string_prefix(b: &[u8], i: usize) -> Option<(usize, bool)> {
    // 直前が識別子の文字なら接頭辞ではない（`for"` などは書かれないが、念のため）
    if i > 0 && is_ident_byte(b[i - 1]) {
        return None;
    }
    let rest = &b[i..];
    for (p, raw) in [("br", true), ("cr", true), ("r", true), ("b", false), ("c", false)] {
        let p = p.as_bytes();
        if rest.starts_with(p) {
            let after = rest.get(p.len()).copied();
            if raw && matches!(after, Some(b'"' | b'#')) {
                // r#ident（生の識別子）は文字列でない
                if after == Some(b'#') {
                    let mut k = p.len();
                    while rest.get(k) == Some(&b'#') {
                        k += 1;
                    }
                    if rest.get(k) != Some(&b'"') {
                        return None;
                    }
                }
                return Some((p.len(), true));
            }
            if !raw && after == Some(b'"') {
                return Some((p.len(), false));
            }
        }
    }
    None
}

/// `"…"` を読む（`i` は開きの引用符）。終わりの位置と中身。
fn scan_string(src: &str, i: usize, line: &mut u32) -> (usize, String) {
    let b = src.as_bytes();
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => {
                return (j + 1, src[i + 1..j].to_string());
            }
            b'\n' => {
                *line += 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    (b.len(), src[(i + 1).min(b.len())..].to_string())
}

/// `#…#"…"#…#` を読む（`i` は最初の # か引用符）。
fn scan_raw_string(src: &str, i: usize, line: &mut u32) -> (usize, String) {
    let b = src.as_bytes();
    let mut hashes = 0usize;
    let mut j = i;
    while b.get(j) == Some(&b'#') {
        hashes += 1;
        j += 1;
    }
    // 開きの引用符
    j += 1;
    let start = j;
    while j < b.len() {
        if b[j] == b'"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&b'#')) {
            return (j + 1 + hashes, src[start..j].to_string());
        }
        if b[j] == b'\n' {
            *line += 1;
        }
        j += 1;
    }
    (b.len(), src[start.min(b.len())..].to_string())
}

/// `'` で始まる文字の字句か（寿命でないか）。`'x'`・`'\n'`・`'\u{…}'`・`'あ'` を文字とみなす。
fn is_char_literal(src: &str, i: usize) -> bool {
    let b = src.as_bytes();
    match b.get(i + 1) {
        Some(b'\\') => true,
        Some(&c) => {
            let len = utf8_len(c);
            b.get(i + 1 + len) == Some(&b'\'')
        }
        None => false,
    }
}

/// 文字の字句の終わり（`i` は開きの `'`）。
fn scan_char(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    if b.get(j) == Some(&b'\\') {
        j += 2;
        while j < b.len() && b[j] != b'\'' {
            j += 1;
        }
        return (j + 1).min(b.len());
    }
    if let Some(&c) = b.get(j) {
        j += utf8_len(c);
    }
    (j + 1).min(b.len())
}

/// `tokens[open]` の開きかっこ（`(`・`[`・`{`・`<` は扱わない）に対応する閉じかっこの位置。
pub fn matching(tokens: &[Token], open: usize) -> Option<usize> {
    let (o, c) = match tokens.get(open)?.text.as_str() {
        "(" => ('(', ')'),
        "[" => ('[', ']'),
        "{" => ('{', '}'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (k, t) in tokens.iter().enumerate().skip(open) {
        if t.is_punct(o) {
            depth += 1;
        } else if t.is_punct(c) {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, String)> {
        lex(src).into_iter().map(|t| (t.kind, t.text)).collect()
    }

    #[test]
    fn comments_strings_and_lifetimes() {
        let src =
            "// vec![1]\n/* format!(\"x\") /* nested */ */\nfn f<'a>(x: &'a str) -> char { let _ = \"vec![\"; 'x' }";
        let k = kinds(src);
        assert!(!k.iter().any(|(_, t)| t == "vec" || t == "format" || t == "nested"));
        assert!(k.contains(&(Kind::Lifetime, "a".to_string())));
        assert!(k.contains(&(Kind::Str, "vec![".to_string())));
        assert!(k.contains(&(Kind::Literal, "'x'".to_string())));
        assert!(k.contains(&(Kind::Ident, "fn".to_string())));
    }

    #[test]
    fn raw_strings_and_raw_idents() {
        let k = kinds("let s = r#\"a \" b\"#; let r#type = 1; let b = br\"x\"; let c = b'\\''; let d = '\\u{3042}';");
        assert!(k.contains(&(Kind::Str, "a \" b".to_string())));
        assert!(k.contains(&(Kind::Ident, "type".to_string())));
        assert!(k.contains(&(Kind::Str, "x".to_string())));
        assert!(k.contains(&(Kind::Literal, "b'\\''".to_string())));
        assert!(k.contains(&(Kind::Literal, "'\\u{3042}'".to_string())));
    }

    #[test]
    fn docs_and_lines() {
        let t = lex("/// # Panics\n//! top\n//// not doc\nfn x() {}\n/** block */");
        assert_eq!(t[0].kind, Kind::Doc);
        assert_eq!(t[0].text, " # Panics");
        assert_eq!(t[1].text, " top");
        assert_eq!(t[2].text, "fn");
        assert_eq!(t[2].line, 4);
        assert_eq!(t.last().map(|t| t.text.as_str()), Some(" block "));
    }

    #[test]
    fn numbers_and_ranges() {
        let k = kinds("0..10 1.5e-3 0xFFu8 2.0f64");
        assert_eq!(k[0], (Kind::Literal, "0".to_string()));
        assert_eq!(k[1], (Kind::Punct, ".".to_string()));
        assert!(k.contains(&(Kind::Literal, "1.5e-3".to_string())));
        assert!(k.contains(&(Kind::Literal, "0xFFu8".to_string())));
        assert!(k.contains(&(Kind::Literal, "2.0f64".to_string())));
    }

    #[test]
    fn suffixes_and_unclosed_comments() {
        let k = kinds("5usize-1 1e-3 2e5f64 0xFFu8");
        assert_eq!(k[0], (Kind::Literal, "5usize".to_string()));
        assert_eq!(k[1], (Kind::Punct, "-".to_string()));
        assert!(k.contains(&(Kind::Literal, "1e-3".to_string())));
        assert!(k.contains(&(Kind::Literal, "2e5f64".to_string())));
        assert!(k.contains(&(Kind::Literal, "0xFFu8".to_string())));
        // 閉じない文書の注釈・文字列・文字で panic しない
        assert_eq!(lex("/** あ")[0].kind, Kind::Doc);
        let _ = lex("/*! い");
        let _ = lex("\"あ");
        let _ = lex("'あ");
        let _ = lex("r#\"う");
    }

    #[test]
    fn matching_braces() {
        let t = lex("fn f() { if x { y } }");
        let open = t.iter().position(|t| t.is_punct('{')).unwrap_or(0);
        assert_eq!(matching(&t, open), Some(t.len() - 1));
    }
}
