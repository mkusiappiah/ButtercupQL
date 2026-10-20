//! JSON, as the command writes it and the language server reads it: a small reader and writer, so that `bcql` needs
//! no other crate.

use std::fmt::{self, Write};

/// A JSON value. An object keeps its fields in the order they were written.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

static NULL: Json = Json::Null;

/// How deep arrays and objects may be nested in a text that is read.
const DEEPEST: usize = 256;

impl Json {
    /// Reads a JSON text.
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            text,
            pos: 0,
        };
        reader.space();
        let value = reader.value(0)?;
        reader.space();
        if reader.pos < reader.bytes.len() {
            return Err(reader.fail("more after the value"));
        }
        Ok(value)
    }

    /// An object's field, or null when there is no such field or this is not an object.
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map_or(&NULL, |(_, value)| value),
            _ => &NULL,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    /// A number that is whole and not negative.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(number)
                if *number >= 0.0 && number.fract() == 0.0 && *number < 2f64.powi(64) =>
            {
                Some(*number as u64)
            }
            _ => None,
        }
    }

    /// An array's items; none when this is not an array.
    pub fn items(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
}

/// An object from its fields.
pub fn object<const N: usize>(fields: [(&str, Json); N]) -> Json {
    Json::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

impl From<&str> for Json {
    fn from(text: &str) -> Json {
        Json::String(text.to_string())
    }
}

impl From<String> for Json {
    fn from(text: String) -> Json {
        Json::String(text)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Json {
        Json::Bool(value)
    }
}

impl From<f64> for Json {
    fn from(number: f64) -> Json {
        Json::Number(number)
    }
}

impl From<i64> for Json {
    fn from(number: i64) -> Json {
        Json::Number(number as f64)
    }
}

impl From<u32> for Json {
    fn from(number: u32) -> Json {
        Json::Number(f64::from(number))
    }
}

impl From<usize> for Json {
    fn from(number: usize) -> Json {
        Json::Number(number as f64)
    }
}

impl From<Vec<Json>> for Json {
    fn from(items: Vec<Json>) -> Json {
        Json::Array(items)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Json {
        value.map_or(Json::Null, Into::into)
    }
}

impl fmt::Display for Json {
    /// Writes the value as compact JSON. A number that is not finite is written as null.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(value) => write!(f, "{value}"),
            Json::Number(number) if !number.is_finite() => f.write_str("null"),
            Json::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => {
                write!(f, "{}", *number as i64)
            }
            Json::Number(number) => write!(f, "{number}"),
            Json::String(text) => f.write_str(&quote(text)),
            Json::Array(items) => {
                f.write_char('[')?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_char(']')
            }
            Json::Object(fields) => {
                f.write_char('{')?;
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{}:{value}", quote(name))?;
                }
                f.write_char('}')
            }
        }
    }
}

/// A text as a JSON string, in quotes.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Reader<'t> {
    bytes: &'t [u8],
    text: &'t str,
    pos: usize,
}

impl Reader<'_> {
    fn fail(&self, what: &str) -> String {
        format!("the JSON is not valid at byte {}: {what}", self.pos)
    }

    fn space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.pos) {
            self.pos += 1;
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.fail("an unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > DEEPEST {
            return Err(self.fail("nested too deeply"));
        }
        match self.bytes.get(self.pos) {
            None => Err(self.fail("the text ends before a value")),
            Some(b'n') => self.word("null", Json::Null),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    self.space();
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(self.fail("expected `,` or `]`")),
                    }
                }
            }
            Some(b'{') => {
                self.pos += 1;
                let mut fields = Vec::new();
                self.space();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.space();
                    if self.bytes.get(self.pos) != Some(&b'"') {
                        return Err(self.fail("expected a field's name"));
                    }
                    let name = self.string()?;
                    self.space();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err(self.fail("expected `:`"));
                    }
                    self.pos += 1;
                    self.space();
                    let value = self.value(depth + 1)?;
                    fields.push((name, value));
                    self.space();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Json::Object(fields));
                        }
                        _ => return Err(self.fail("expected `,` or `}`")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.fail("expected a value")),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        let digits = |reader: &mut Self| {
            let from = reader.pos;
            while reader.bytes.get(reader.pos).is_some_and(u8::is_ascii_digit) {
                reader.pos += 1;
            }
            reader.pos > from
        };
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        if self.bytes.get(self.pos) == Some(&b'0') {
            self.pos += 1;
        } else if !digits(self) {
            return Err(self.fail("a number needs digits"));
        }
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !digits(self) {
                return Err(self.fail("a number needs digits after its point"));
            }
        }
        if let Some(b'e' | b'E') = self.bytes.get(self.pos) {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.bytes.get(self.pos) {
                self.pos += 1;
            }
            if !digits(self) {
                return Err(self.fail("a number needs digits in its exponent"));
            }
        }
        self.text[start..self.pos]
            .parse()
            .map(Json::Number)
            .map_err(|_| self.fail("a number that can't be read"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let run = self.pos;
            while let Some(&byte) = self.bytes.get(self.pos) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(&self.text[run..self.pos]);
            match self.bytes.get(self.pos) {
                None => return Err(self.fail("a string that is not closed")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let Some(&escape) = self.bytes.get(self.pos) else {
                        return Err(self.fail("a string that is not closed"));
                    };
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex()?;
                            let c = if (0xd800..0xdc00).contains(&first)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                let back = self.pos;
                                self.pos += 2;
                                let second = self.hex()?;
                                if (0xdc00..0xe000).contains(&second) {
                                    char::from_u32(
                                        0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00),
                                    )
                                } else {
                                    self.pos = back;
                                    None
                                }
                            } else {
                                char::from_u32(first)
                            };
                            out.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
                        }
                        _ => return Err(self.fail("an unknown escape")),
                    }
                }
                Some(_) => return Err(self.fail("a control character in a string")),
            }
        }
    }

    fn hex(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.pos..self.pos + 4)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.fail("`\\u` needs four hexadecimal digits"))?;
        self.pos += 4;
        Ok(u32::from_str_radix(digits, 16).expect("the digits are hexadecimal"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read() {
        let value = Json::parse(
            r#" {"id": 1, "method": "initialize", "params": {"a": [true, false, null, -2.5e3, "x\ny"]}} "#,
        )
        .unwrap();
        assert_eq!(value.get("id").as_u64(), Some(1));
        assert_eq!(value.get("method").as_str(), Some("initialize"));
        let items = value.get("params").get("a").items();
        assert_eq!(
            items,
            [
                Json::Bool(true),
                Json::Bool(false),
                Json::Null,
                Json::Number(-2500.0),
                Json::from("x\ny")
            ]
        );
        assert!(value.get("missing").get("deeper").is_null());
    }

    #[test]
    fn escapes_are_read_and_written() {
        let value = Json::parse(r#""a\"b\\c\u00e9\ud83c\udf3c\u0001""#).unwrap();
        assert_eq!(value, Json::from("a\"b\\cé🌼\u{1}"));
        assert_eq!(value.to_string(), "\"a\\\"b\\\\cé🌼\\u0001\"");
        // A lone half of a pair is replaced, as UTF-8 can't hold it.
        assert_eq!(
            Json::parse(r#""\ud83c!""#).unwrap(),
            Json::from("\u{fffd}!")
        );
    }

    #[test]
    fn values_are_written_compactly_in_order() {
        let value = object([
            ("jsonrpc", "2.0".into()),
            ("id", 7u32.into()),
            (
                "result",
                Json::Array(vec![1.5.into(), Json::Null, f64::NAN.into()]),
            ),
        ]);
        assert_eq!(
            value.to_string(),
            r#"{"jsonrpc":"2.0","id":7,"result":[1.5,null,null]}"#
        );
        assert_eq!(
            Json::parse(&value.to_string()).unwrap().get("id").as_u64(),
            Some(7)
        );
    }

    #[test]
    fn mistakes_are_refused() {
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "01",
            "1.",
            "\"open",
            "tru",
            "[1] 2",
            "\"\\x\"",
            "\"\t\"",
        ] {
            assert!(Json::parse(text).is_err(), "{text:?} was read");
        }
        let deep = "[".repeat(300) + &"]".repeat(300);
        assert!(Json::parse(&deep).is_err());
    }
}
