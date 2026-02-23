// filter/parser.rs — Haystack filter parser.
//
// Grammar:
//   condOr   := condAnd ("or" condAnd)*
//   condAnd  := term ("and" term)*
//   term     := "(" condOr ")"
//              | "not" id                   → missing
//              | "^" sym                    → isSymbol
//              | UpperId                    → isSpec
//              | id "::" ...                → isSpec (qualified name)
//              | path op val               → comparison
//              | path                       → has
//   path     := id ("->" id)*
//   op       := "==" | "!=" | "<" | "<=" | ">" | ">="
//   val      := number | string | ref | bool | null | na | date | time | datetime

use crate::error::{FolioError, Result};
use crate::types::{HRef, Val};
use crate::filter::ast::{Filter, FilterPath};
use chrono::{NaiveDate, NaiveTime, TimeZone};

pub fn parse(input: &str) -> Result<Filter> {
    let mut p = Parser::new(input);
    let f = p.cond_or()?;
    if !p.is_eof() {
        return Err(FolioError::Protocol(format!(
            "Unexpected token '{}' at position {} in filter: {}",
            p.remaining(), p.pos, input
        )));
    }
    Ok(f)
}

struct Parser<'a> {
    input: &'a str,
    pos:   usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { input: s, pos: 0 }
    }

    fn remaining(&self) -> &str {
        &self.input[self.pos..]
    }

    fn is_eof(&self) -> bool {
        self.skip_ws();
        self.pos >= self.input.len()
    }

    fn skip_ws(&self) -> () {
        // Note: skip_ws mutates pos indirectly via peek helpers
    }

    fn peek_char(&self) -> Option<char> {
        let s = self.input[self.pos..].trim_start();
        let trimmed_off = self.input[self.pos..].len() - s.len();
        s.chars().next()
    }

    // Skip whitespace and return current position after trim
    fn trim_pos(&self) -> usize {
        let tail = &self.input[self.pos..];
        let trimmed = tail.trim_start_matches(|c: char| c.is_whitespace());
        self.pos + (tail.len() - trimmed.len())
    }

    fn advance_to_trimmed(&mut self) {
        self.pos = self.trim_pos();
    }

    fn cond_or(&mut self) -> Result<Filter> {
        let mut lhs = self.cond_and()?;
        loop {
            if self.peek_keyword("or") {
                self.consume_keyword("or");
                let rhs = self.cond_and()?;
                lhs = Filter::Or(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn cond_and(&mut self) -> Result<Filter> {
        let mut lhs = self.term()?;
        loop {
            if self.peek_keyword("and") {
                self.consume_keyword("and");
                let rhs = self.term()?;
                lhs = Filter::And(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Filter> {
        self.advance_to_trimmed();
        let tail = &self.input[self.pos..];

        // Parenthesized sub-expression
        if tail.starts_with('(') {
            self.pos += 1;
            let f = self.cond_or()?;
            self.advance_to_trimmed();
            if !self.input[self.pos..].starts_with(')') {
                return Err(FolioError::Protocol("Expected ')' in filter".into()));
            }
            self.pos += 1;
            return Ok(f);
        }

        // Symbol: ^sym
        if tail.starts_with('^') {
            self.pos += 1;
            let sym = self.read_id()?;
            return Ok(Filter::IsSymbol(sym));
        }

        // "not" keyword → missing
        if self.peek_keyword("not") {
            self.consume_keyword("not");
            let p = self.read_path()?;
            return Ok(Filter::Missing(p));
        }

        // Identifier-based term
        if self.peek_is_id() {
            let id = self.read_id()?;

            // Check for spec qualified name (id :: ...) or dotted (id . ...)
            self.advance_to_trimmed();
            let after = &self.input[self.pos..];

            if after.starts_with("::") || after.starts_with('.') {
                // isSpec: qualified name — consume rest of spec name
                let spec = self.read_spec_name(&id)?;
                return Ok(Filter::IsSpec(spec));
            }

            // isSpec: starts with uppercase letter
            if id.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                return Ok(Filter::IsSpec(id));
            }

            // path with possible -> continuation
            let path = self.extend_path(id)?;

            // comparison or has
            self.advance_to_trimmed();
            return self.path_filter(path);
        }

        Err(FolioError::Protocol(format!(
            "Unexpected token in filter at pos {}: {:?}",
            self.pos,
            &self.input[self.pos..(self.pos + 20).min(self.input.len())]
        )))
    }

    /// After reading a base path, apply comparison operator or return Has.
    fn path_filter(&mut self, path: FilterPath) -> Result<Filter> {
        let tail = &self.input[self.pos..];

        if tail.starts_with("==") { self.pos += 2; let v = self.read_val()?; return Ok(Filter::Eq(path, v)); }
        if tail.starts_with("!=") { self.pos += 2; let v = self.read_val()?; return Ok(Filter::Ne(path, v)); }
        if tail.starts_with("<=") { self.pos += 2; let v = self.read_val()?; return Ok(Filter::Le(path, v)); }
        if tail.starts_with(">=") { self.pos += 2; let v = self.read_val()?; return Ok(Filter::Ge(path, v)); }
        if tail.starts_with('<')  { self.pos += 1; let v = self.read_val()?; return Ok(Filter::Lt(path, v)); }
        if tail.starts_with('>')  { self.pos += 1; let v = self.read_val()?; return Ok(Filter::Gt(path, v)); }

        Ok(Filter::Has(path))
    }

    fn read_path(&mut self) -> Result<FilterPath> {
        let id = self.read_id()?;
        self.extend_path(id)
    }

    fn extend_path(&mut self, first: String) -> Result<FilterPath> {
        let mut segments = vec![first];
        loop {
            self.advance_to_trimmed();
            if self.input[self.pos..].starts_with("->") {
                self.pos += 2;
                self.advance_to_trimmed();
                segments.push(self.read_id()?);
            } else {
                break;
            }
        }
        if segments.len() == 1 {
            Ok(FilterPath::single(segments.remove(0)))
        } else {
            Ok(FilterPath::multi(segments))
        }
    }

    fn read_spec_name(&mut self, first: &str) -> Result<String> {
        // Already consumed the first identifier. Now consume :: or .
        let mut name = first.to_string();
        loop {
            self.advance_to_trimmed();
            let t = &self.input[self.pos..];
            if t.starts_with("::") {
                self.pos += 2;
                name.push_str("::");
                name.push_str(&self.read_id()?);
            } else if t.starts_with('.') {
                self.pos += 1;
                name.push('.');
                name.push_str(&self.read_id()?);
            } else {
                break;
            }
        }
        Ok(name)
    }

    fn read_id(&mut self) -> Result<String> {
        self.advance_to_trimmed();
        let tail = &self.input[self.pos..];
        let end = tail.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(tail.len());
        if end == 0 {
            return Err(FolioError::Protocol(format!(
                "Expected identifier at pos {} in filter: {}",
                self.pos, self.input
            )));
        }
        let id = tail[..end].to_string();
        self.pos += end;
        Ok(id)
    }

    fn peek_is_id(&self) -> bool {
        let p = self.trim_pos();
        self.input[p..].chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        let p = self.trim_pos();
        let tail = &self.input[p..];
        if !tail.starts_with(kw) { return false; }
        // keyword must be followed by non-alphanumeric
        let after = &tail[kw.len()..];
        after.chars().next().map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(true)
    }

    fn consume_keyword(&mut self, kw: &str) {
        self.advance_to_trimmed();
        self.pos += kw.len();
    }

    // ── Value parsing ────────────────────────────────────────────────────────

    fn read_val(&mut self) -> Result<Val> {
        self.advance_to_trimmed();
        let tail = &self.input[self.pos..];

        // Null
        if tail.starts_with("null") && !tail[4..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            self.pos += 4;
            return Ok(Val::Null);
        }

        // NA
        if tail.starts_with("NA") && !tail[2..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            self.pos += 2;
            return Ok(Val::NA);
        }

        // Bool true / false
        if tail.starts_with("true") && !tail[4..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            self.pos += 4;
            return Ok(Val::Bool(true));
        }
        if tail.starts_with("false") && !tail[5..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            self.pos += 5;
            return Ok(Val::Bool(false));
        }

        // Marker `M`
        if tail.starts_with('M') && !tail[1..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            self.pos += 1;
            return Ok(Val::Marker);
        }

        // Ref: @id or @id "dis"
        if tail.starts_with('@') {
            return self.read_ref();
        }

        // String: "..."
        if tail.starts_with('"') {
            return self.read_str_literal();
        }

        // Uri: `...`
        if tail.starts_with('`') {
            return self.read_uri_literal();
        }

        // DateTime, Date, Time, or Number — all start with a digit or minus
        if tail.starts_with('-') || tail.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return self.read_number_or_temporal();
        }

        // Symbol: ^sym
        if tail.starts_with('^') {
            self.pos += 1;
            let sym = self.read_id()?;
            return Ok(Val::Symbol(sym));
        }

        Err(FolioError::Protocol(format!(
            "Cannot parse filter value at pos {}: {:?}",
            self.pos,
            &tail[..20.min(tail.len())]
        )))
    }

    fn read_ref(&mut self) -> Result<Val> {
        // @id or @id "dis"
        self.pos += 1; // skip @
        let tail = &self.input[self.pos..];
        // Ref id: any chars up to whitespace or operator
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '<' | '>' | '=' | '!' | '"'))
            .unwrap_or(tail.len());
        let id = tail[..end].to_string();
        self.pos += end;

        // Optional display string
        self.advance_to_trimmed();
        let dis = if self.input[self.pos..].starts_with('"') {
            let s = self.read_str_literal()?;
            if let Val::Str(ds) = s { Some(ds) } else { None }
        } else {
            None
        };

        Ok(Val::Ref(HRef { id, dis }))
    }

    fn read_str_literal(&mut self) -> Result<Val> {
        self.pos += 1; // skip opening "
        let mut s = String::new();
        let chars: Vec<char> = self.input[self.pos..].chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '"' => { i += 1; break; }
                '\\' => {
                    i += 1;
                    if i < chars.len() {
                        match chars[i] {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            c => { s.push('\\'); s.push(c); }
                        }
                        i += 1;
                    }
                }
                c => { s.push(c); i += 1; }
            }
        }
        // Advance pos by the number of bytes consumed
        let consumed: usize = chars[..i].iter().collect::<String>().len();
        self.pos += consumed;
        Ok(Val::Str(s))
    }

    fn read_uri_literal(&mut self) -> Result<Val> {
        self.pos += 1; // skip `
        let tail = &self.input[self.pos..];
        let end = tail.find('`').unwrap_or(tail.len());
        let uri = tail[..end].to_string();
        self.pos += end + 1; // skip closing `
        Ok(Val::Uri(uri))
    }

    fn read_number_or_temporal(&mut self) -> Result<Val> {
        // Peek ahead: if looks like YYYY-MM-DD or YYYY-MM-DDThh:mm treat as temporal
        let tail = &self.input[self.pos..];

        // Date pattern: YYYY-MM-DD (no T after)
        if is_date_pattern(tail) {
            return self.read_date();
        }

        // DateTime: YYYY-MM-DDThh:mm...
        if is_datetime_pattern(tail) {
            return self.read_datetime_literal();
        }

        // Time: hh:mm or hh:mm:ss
        if is_time_pattern(tail) {
            return self.read_time_literal();
        }

        // Number (with optional unit)
        self.read_number_literal()
    }

    fn read_number_literal(&mut self) -> Result<Val> {
        let tail = &self.input[self.pos..];
        let mut end = 0;
        let chars: Vec<char> = tail.chars().collect();

        // Optional minus
        if end < chars.len() && chars[end] == '-' { end += 1; }

        // Digits before decimal
        while end < chars.len() && chars[end].is_ascii_digit() { end += 1; }

        // Optional decimal
        if end < chars.len() && chars[end] == '.' {
            end += 1;
            while end < chars.len() && chars[end].is_ascii_digit() { end += 1; }
        }

        // Optional exponent
        if end < chars.len() && (chars[end] == 'e' || chars[end] == 'E') {
            end += 1;
            if end < chars.len() && (chars[end] == '+' || chars[end] == '-') { end += 1; }
            while end < chars.len() && chars[end].is_ascii_digit() { end += 1; }
        }

        let num_str: String = chars[..end].iter().collect();
        let num_bytes = num_str.len();
        let val: f64 = num_str.parse().map_err(|e| {
            FolioError::Protocol(format!("Invalid number in filter '{}': {}", num_str, e))
        })?;
        self.pos += num_bytes;

        // Optional unit (alphanumeric chars after number, no space)
        let tail = &self.input[self.pos..];
        let unit_end = tail.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '<' | '>' | '=' | '!' | '"' | '@')).unwrap_or(tail.len());
        let unit = if unit_end > 0 { Some(tail[..unit_end].to_string()) } else { None };
        if unit_end > 0 { self.pos += unit_end; }

        Ok(Val::Number(val, unit))
    }

    fn read_date(&mut self) -> Result<Val> {
        let tail = &self.input[self.pos..];
        // YYYY-MM-DD
        if tail.len() < 10 {
            return Err(FolioError::Protocol("Short date literal".into()));
        }
        let year:  i32 = tail[0..4].parse().map_err(|_| FolioError::Protocol("Bad year".into()))?;
        let month: u32 = tail[5..7].parse().map_err(|_| FolioError::Protocol("Bad month".into()))?;
        let day:   u32 = tail[8..10].parse().map_err(|_| FolioError::Protocol("Bad day".into()))?;
        self.pos += 10;
        let d = NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| FolioError::Protocol(format!("Invalid date: {}-{}-{}", year, month, day)))?;
        Ok(Val::Date(d))
    }

    fn read_datetime_literal(&mut self) -> Result<Val> {
        // ISO 8601: YYYY-MM-DDThh:mm:ss[.FFF][±hh:mm|Z][ TzName]
        let tail = &self.input[self.pos..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '"'))
            .unwrap_or(tail.len());
        let dt_str = &tail[..end];
        self.pos += end;

        // Parse ISO 8601
        // Try to extract timezone name if present after a space, e.g. "2009-02-03T04:05:06-05:00 New_York"
        // For now, parse as UTC if no timezone specified
        let (dt_part, tz_name) = if let Some(sp) = dt_str.find(' ') {
            (&dt_str[..sp], Some(&dt_str[sp+1..]))
        } else {
            (dt_str, None)
        };

        // Use chrono to parse the date-time part
        use chrono::DateTime as CDateTime;
        let tz: chrono_tz::Tz = tz_name
            .and_then(|s| s.parse().ok())
            .unwrap_or(chrono_tz::UTC);

        // Parse as RFC3339 or manual parse
        if let Ok(dt) = CDateTime::parse_from_rfc3339(dt_part) {
            let dt_tz = dt.with_timezone(&tz);
            return Ok(Val::DateTime(dt_tz));
        }

        // Fallback: try parsing just date+time, assume UTC
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(dt_part, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Val::DateTime(tz.from_utc_datetime(&naive)));
        }

        Err(FolioError::Protocol(format!("Cannot parse datetime: {}", dt_str)))
    }

    fn read_time_literal(&mut self) -> Result<Val> {
        // hh:mm or hh:mm:ss or hh:mm:ss.FFF
        let tail = &self.input[self.pos..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '"'))
            .unwrap_or(tail.len());
        let t_str = &tail[..end];
        self.pos += end;

        let t = NaiveTime::parse_from_str(t_str, "%H:%M:%S")
            .or_else(|_| NaiveTime::parse_from_str(t_str, "%H:%M:%S%.f"))
            .or_else(|_| NaiveTime::parse_from_str(t_str, "%H:%M"))
            .map_err(|e| FolioError::Protocol(format!("Cannot parse time '{}': {}", t_str, e)))?;
        Ok(Val::Time(t))
    }
}

// ── Pattern detection helpers ─────────────────────────────────────────────────

fn is_date_pattern(s: &str) -> bool {
    // YYYY-MM-DD (no T following)
    if s.len() < 10 { return false; }
    let chars: Vec<char> = s.chars().collect();
    chars[0].is_ascii_digit() && chars[1].is_ascii_digit() &&
    chars[2].is_ascii_digit() && chars[3].is_ascii_digit() &&
    chars[4] == '-' && chars[5].is_ascii_digit() && chars[6].is_ascii_digit() &&
    chars[7] == '-' && chars[8].is_ascii_digit() && chars[9].is_ascii_digit() &&
    s.len().saturating_sub(10) > 0 && s.chars().nth(10) != Some('T')
    || (s.len() == 10 &&
        chars[0].is_ascii_digit() && chars[1].is_ascii_digit() &&
        chars[2].is_ascii_digit() && chars[3].is_ascii_digit() &&
        chars[4] == '-')
}

fn is_datetime_pattern(s: &str) -> bool {
    if s.len() < 11 { return false; }
    let chars: Vec<char> = s.chars().collect();
    chars[0].is_ascii_digit() && chars[1].is_ascii_digit() &&
    chars[2].is_ascii_digit() && chars[3].is_ascii_digit() &&
    chars[4] == '-' &&
    chars.get(10) == Some(&'T')
}

fn is_time_pattern(s: &str) -> bool {
    if s.len() < 5 { return false; }
    let chars: Vec<char> = s.chars().collect();
    chars[0].is_ascii_digit() && chars[1].is_ascii_digit() && chars[2] == ':'
}
