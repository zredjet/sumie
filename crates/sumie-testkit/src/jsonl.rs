//! 平らな JSON の行の読み書き（ゴールデン・照合の集まりの一覧。determinism.md §10）。
//!
//! 1 行は平らなオブジェクト（入れ子なし）で、値は文字列・整数・真偽だけ。書くときは鍵の順を保つ（ゴールデンの行を
//! バイトで比べられるように）。依存を増やさないための小さな自作。

/// 値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i128),
    Bool(bool),
}

/// 1 行（鍵の順を保つ）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub fields: Vec<(String, Value)>,
}

impl Line {
    pub fn new() -> Line {
        Line::default()
    }

    pub fn str(mut self, k: &str, v: impl Into<String>) -> Line {
        self.fields.push((k.to_string(), Value::Str(v.into())));
        self
    }

    pub fn int(mut self, k: &str, v: impl Into<i128>) -> Line {
        self.fields.push((k.to_string(), Value::Int(v.into())));
        self
    }

    pub fn bool(mut self, k: &str, v: bool) -> Line {
        self.fields.push((k.to_string(), Value::Bool(v)));
        self
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        self.fields.iter().find(|(a, _)| a == k).map(|(_, v)| v)
    }

    pub fn get_str(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_int(&self, k: &str) -> Option<i128> {
        match self.get(k) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn get_bool(&self, k: &str) -> Option<bool> {
        match self.get(k) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// 1 行の JSON にする（改行なし）。
    pub fn to_json(&self) -> String {
        let mut s = String::from("{");
        for (i, (k, v)) in self.fields.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            quote(&mut s, k);
            s.push(':');
            match v {
                Value::Str(t) => quote(&mut s, t),
                Value::Int(n) => s.push_str(&n.to_string()),
                Value::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            }
        }
        s.push('}');
        s
    }

    /// 1 行の JSON を読む。
    pub fn parse(text: &str) -> Result<Line, String> {
        let mut p = Parser { b: text.as_bytes(), i: 0 };
        p.ws();
        p.expect(b'{')?;
        let mut line = Line::new();
        p.ws();
        if p.peek() == Some(b'}') {
            p.i += 1;
        } else {
            loop {
                p.ws();
                let k = p.string()?;
                p.ws();
                p.expect(b':')?;
                p.ws();
                let v = p.value()?;
                line.fields.push((k, v));
                p.ws();
                match p.next() {
                    Some(b',') => continue,
                    Some(b'}') => break,
                    other => return Err(format!("{} バイト目：, か }} がない（{other:?}）", p.i)),
                }
            }
        }
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("{} バイト目：行の終わりのあとに字がある", p.i));
        }
        Ok(line)
    }
}

fn quote(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let c = self.peek();
        self.i += 1;
        c
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.next() == Some(c) { Ok(()) } else { Err(format!("{} バイト目：{} がない", self.i, c as char)) }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.i..self.i + 4).ok_or("\\u の 4 桁がない")?;
        let s = std::str::from_utf8(s).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(s, 16).map_err(|e| e.to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next() {
                None => return Err("文字列が閉じない".into()),
                Some(b'"') => break,
                Some(b'\\') => match self.next() {
                    Some(b'"') => out.push(b'"'),
                    Some(b'\\') => out.push(b'\\'),
                    Some(b'/') => out.push(b'/'),
                    Some(b'b') => out.push(8),
                    Some(b'f') => out.push(12),
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'u') => {
                        let mut cp = self.hex4()?;
                        if (0xd800..0xdc00).contains(&cp) {
                            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                                return Err("サロゲートの対がない".into());
                            }
                            let lo = self.hex4()?;
                            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                        }
                        let c = char::from_u32(cp).ok_or("正しくない符号位置")?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    other => return Err(format!("知らないエスケープ {other:?}")),
                },
                Some(c) => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Value::Bool(true))
            }
            Some(b'f') if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Value::Bool(false))
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                self.i += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
                let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?;
                s.parse::<i128>().map(Value::Int).map_err(|e| format!("整数でない {s}：{e}"))
            }
            other => Err(format!("{} バイト目：値でない（{other:?}）", self.i)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let l = Line::new().str("name", "a\"b\\c\nあ\u{1}").int("n", -42).int("big", u64::MAX).bool("ok", true);
        let j = l.to_json();
        assert_eq!(j, "{\"name\":\"a\\\"b\\\\c\\nあ\\u0001\",\"n\":-42,\"big\":18446744073709551615,\"ok\":true}");
        assert_eq!(Line::parse(&j).unwrap(), l);
        assert_eq!(Line::parse(" { } ").unwrap(), Line::new());
        assert_eq!(Line::parse("{\"s\":\"\\ud83d\\ude00\"}").unwrap().get_str("s"), Some("😀"));
    }

    #[test]
    fn rejects_broken() {
        for bad in ["", "{", "{\"a\"}", "{\"a\":}", "{\"a\":1,}", "{\"a\":1} x", "{\"a\":[1]}", "{\"a\":\"\\q\"}"] {
            assert!(Line::parse(bad).is_err(), "{bad}");
        }
    }
}
