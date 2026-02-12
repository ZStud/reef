use std::fmt;

/// Byte-oriented scanner for bash input. Operates on `&[u8]` with a position
/// cursor. No token enum — the parser calls methods directly (peek/eat/read).
/// Every read method returns `&'a str` — a zero-copy slice of the input.
pub struct Lexer<'a> {
    src: &'a [u8],
    input: &'a str,
    pos: usize,
}

/// Error produced when the parser encounters invalid or unsupported bash syntax.
#[derive(Debug)]
pub struct ParseError {
    /// Byte offset in the input where the error occurred.
    pub pos: usize,
    /// Human-readable description of the error.
    pub msg: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at byte {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer {
            src: input.as_bytes(),
            input,
            pos: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Position / lookahead
    // -----------------------------------------------------------------------

    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Peek current byte. Returns 0 at EOF — NUL never appears in shell input.
    #[inline]
    pub fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }

    /// Peek at `pos + offset`.
    #[inline]
    pub fn peek_at(&self, offset: usize) -> u8 {
        let i = self.pos + offset;
        if i < self.src.len() { self.src[i] } else { 0 }
    }

    /// Slice of the original input from `start` to current position.
    #[inline]
    pub fn slice(&self, start: usize) -> &'a str {
        &self.input[start..self.pos]
    }

    /// Slice of the original input from `start` to `end`.
    #[inline]
    pub fn slice_range(&self, start: usize, end: usize) -> &'a str {
        debug_assert!(
            start <= end && end <= self.src.len(),
            "slice_range({start}, {end}): len={}",
            self.src.len()
        );
        &self.input[start..end]
    }

    /// Remaining input from current position to end.
    #[inline]
    pub fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    // -----------------------------------------------------------------------
    // Advance
    // -----------------------------------------------------------------------

    /// Set position directly — used for backtracking.
    #[inline]
    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    #[inline]
    pub fn bump(&mut self) {
        self.pos += 1;
    }

    #[inline]
    pub fn bump_n(&mut self, n: usize) {
        self.pos += n;
    }

    /// Advance if current byte matches. Returns true if consumed.
    #[inline]
    pub fn eat(&mut self, b: u8) -> bool {
        if self.peek() == b {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Advance if the upcoming bytes match a string. Returns true if consumed.
    pub fn eat_str(&mut self, s: &[u8]) -> bool {
        if self.pos + s.len() <= self.src.len() && &self.src[self.pos..self.pos + s.len()] == s {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Skip
    // -----------------------------------------------------------------------

    /// Skip spaces and tabs (not newlines).
    pub fn skip_blanks(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b' ' | b'\t' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Skip a `#` comment through end of line.
    pub fn skip_comment(&mut self) {
        if self.peek() == b'#' {
            while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                self.pos += 1;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Read — all return &'a str, zero allocation
    // -----------------------------------------------------------------------

    /// Read a shell variable name: `[a-zA-Z_][a-zA-Z_0-9]*`.
    /// Returns empty string if no valid name at current position.
    pub fn read_name(&mut self) -> &'a str {
        let start = self.pos;
        if self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_alphabetic() || self.src[self.pos] == b'_')
        {
            self.pos += 1;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
        }
        self.slice(start)
    }

    /// Read a digit sequence: `[0-9]+`. Returns empty string if no digits.
    pub fn read_number(&mut self) -> &'a str {
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        self.slice(start)
    }

    // -----------------------------------------------------------------------
    // Balanced extraction
    // -----------------------------------------------------------------------

    /// Read content inside single quotes. Cursor starts after `'`.
    /// No escaping — ends at next `'`. Returns content, cursor after closing `'`.
    pub fn scan_squote(&mut self) -> Result<&'a str, ParseError> {
        let start = self.pos;
        while self.pos < self.src.len() {
            if self.src[self.pos] == b'\'' {
                let content = self.slice(start);
                self.pos += 1;
                return Ok(content);
            }
            self.pos += 1;
        }
        Err(self.err("unterminated single quote"))
    }

    // -----------------------------------------------------------------------
    // Keyword detection — does NOT consume
    // -----------------------------------------------------------------------

    /// Check if the next word matches `kw` and is followed by a word boundary.
    pub fn at_keyword(&self, kw: &[u8]) -> bool {
        let end = self.pos + kw.len();
        if end > self.src.len() {
            return false;
        }
        if &self.src[self.pos..end] != kw {
            return false;
        }
        // Single-byte metacharacters are self-delimiting — no boundary needed
        if kw.len() == 1 && is_meta(kw[0]) {
            return true;
        }
        // Multi-byte keywords need a word boundary after them
        end >= self.src.len() || is_meta(self.src[end])
    }

    /// Check if any of the given keywords match at the current position.
    pub fn at_any_keyword(&self, keywords: &[&[u8]]) -> bool {
        keywords.iter().any(|kw| self.at_keyword(kw))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    pub fn err(&self, msg: &'static str) -> ParseError {
        ParseError { pos: self.pos, msg }
    }
}

/// Shell metacharacters — terminate words and act as delimiters.
#[inline]
pub const fn is_meta(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>' | b'\0'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_and_eof() {
        let lex = Lexer::new("");
        assert!(lex.is_eof());
        assert_eq!(lex.peek(), 0);

        let lex = Lexer::new("a");
        assert!(!lex.is_eof());
        assert_eq!(lex.peek(), b'a');
    }

    #[test]
    fn eat_and_bump() {
        let mut lex = Lexer::new("ab");
        assert!(lex.eat(b'a'));
        assert!(!lex.eat(b'a'));
        assert!(lex.eat(b'b'));
        assert!(lex.is_eof());
    }

    #[test]
    fn eat_str() {
        let mut lex = Lexer::new("then done");
        assert!(lex.eat_str(b"then"));
        assert_eq!(lex.peek(), b' ');
        lex.bump();
        assert!(lex.eat_str(b"done"));
        assert!(lex.is_eof());
    }

    #[test]
    fn skip_blanks_not_newlines() {
        let mut lex = Lexer::new("  \t\nfoo");
        lex.skip_blanks();
        assert_eq!(lex.peek(), b'\n');
    }

    #[test]
    fn read_name() {
        let mut lex = Lexer::new("FOO_bar123 rest");
        assert_eq!(lex.read_name(), "FOO_bar123");
        assert_eq!(lex.peek(), b' ');
    }

    #[test]
    fn read_name_underscore_start() {
        let mut lex = Lexer::new("_private");
        assert_eq!(lex.read_name(), "_private");
    }

    #[test]
    fn read_name_no_match() {
        let mut lex = Lexer::new("123abc");
        assert_eq!(lex.read_name(), "");
        assert_eq!(lex.pos(), 0);
    }

    #[test]
    fn read_number() {
        let mut lex = Lexer::new("42rest");
        assert_eq!(lex.read_number(), "42");
    }

    #[test]
    fn scan_squote() {
        let mut lex = Lexer::new("hello world'rest");
        let content = lex.scan_squote().unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(lex.peek(), b'r');
    }

    #[test]
    fn at_keyword() {
        let lex = Lexer::new("then ");
        assert!(lex.at_keyword(b"then"));
        assert!(!lex.at_keyword(b"the"));
    }

    #[test]
    fn at_keyword_eof() {
        let lex = Lexer::new("fi");
        assert!(lex.at_keyword(b"fi"));
    }

    #[test]
    fn at_keyword_no_boundary() {
        let lex = Lexer::new("done_stuff");
        assert!(!lex.at_keyword(b"done"));
    }

    #[test]
    fn skip_comment() {
        let mut lex = Lexer::new("# this is a comment\nnext");
        lex.skip_comment();
        assert_eq!(lex.peek(), b'\n');
    }
}
