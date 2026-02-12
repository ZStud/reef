use std::borrow::Cow;

use crate::ast::*;
use crate::lexer::{Lexer, ParseError, is_meta};

/// Recursive-descent parser for bash syntax. Produces an AST of [`Cmd`] nodes.
pub struct Parser<'a> {
    lex: Lexer<'a>,
    heredoc_resume: Option<usize>,
}

impl<'a> Parser<'a> {
    /// Create a parser for the given bash input.
    pub fn new(input: &'a str) -> Self {
        Parser {
            lex: Lexer::new(input),
            heredoc_resume: None,
        }
    }

    /// Parse the input into a list of commands.
    pub fn parse(mut self) -> Result<Vec<Cmd<'a>>, ParseError> {
        self.cmd_list(&[])
    }

    /// Parse a heredoc body with variable/command expansion (unquoted delimiter).
    /// Similar to double-quoted parsing but stops at EOF.
    pub fn parse_heredoc_body(mut self) -> Result<Vec<Atom<'a>>, ParseError> {
        let mut atoms = Vec::new();
        let mut lit_start = self.lex.pos();

        while !self.lex.is_eof() {
            match self.lex.peek() {
                b'$' => {
                    if self.lex.pos() > lit_start {
                        atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                    }
                    atoms.push(self.dollar()?);
                    lit_start = self.lex.pos();
                }
                b'\\' => {
                    // In heredocs, only \$, \\, \`, and \newline are special
                    let next = self.lex.peek_at(1);
                    if matches!(next, b'$' | b'\\' | b'`' | b'\n') {
                        if self.lex.pos() > lit_start {
                            atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                        }
                        self.lex.bump(); // skip backslash
                        if self.lex.peek() == b'\n' {
                            // line continuation — skip newline
                            self.lex.bump();
                        } else {
                            let esc_start = self.lex.pos();
                            self.lex.bump();
                            atoms.push(Atom::Escaped(Cow::Borrowed(self.lex.slice(esc_start))));
                        }
                        lit_start = self.lex.pos();
                    } else {
                        // Not a special escape — keep the backslash as literal
                        self.lex.bump();
                    }
                }
                b'`' => {
                    if self.lex.pos() > lit_start {
                        atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                    }
                    atoms.push(self.backtick()?);
                    lit_start = self.lex.pos();
                }
                _ => {
                    self.lex.bump();
                }
            }
        }

        if self.lex.pos() > lit_start {
            atoms.push(Atom::Lit(self.lex.slice(lit_start)));
        }
        Ok(atoms)
    }

    // -----------------------------------------------------------------------
    // Command lists
    // -----------------------------------------------------------------------

    /// Parse a sequence of commands separated by `;`, `\n`, or `&`.
    /// Stops when a keyword in `terminators` is found or at EOF.
    fn cmd_list(&mut self, terminators: &[&[u8]]) -> Result<Vec<Cmd<'a>>, ParseError> {
        let mut cmds = Vec::new();
        loop {
            self.skip_separators();
            if self.lex.is_eof() {
                break;
            }
            if !terminators.is_empty() && self.lex.at_any_keyword(terminators) {
                break;
            }
            if self.lex.peek() == b'#' {
                self.lex.skip_comment();
                continue;
            }
            let before = self.lex.pos();
            cmds.push(self.cmd()?);
            if self.lex.pos() == before {
                return Err(self.lex.err("unexpected token"));
            }
        }
        Ok(cmds)
    }

    /// Parse a single complete command (and-or list, possibly backgrounded).
    fn cmd(&mut self) -> Result<Cmd<'a>, ParseError> {
        let list = self.and_or()?;
        self.lex.skip_blanks();
        // If a heredoc was encountered, jump past its body
        if let Some(pos) = self.heredoc_resume.take() {
            self.lex.set_pos(pos);
        }
        if self.lex.eat(b'&') && self.lex.peek() != b'&' && self.lex.peek() != b'>' {
            Ok(Cmd::Job(list))
        } else {
            Ok(Cmd::List(list))
        }
    }

    /// Parse an and-or list: `pipeline ( && pipeline | || pipeline )*`
    fn and_or(&mut self) -> Result<AndOrList<'a>, ParseError> {
        let first = self.pipeline()?;
        let mut rest = Vec::new();
        loop {
            self.lex.skip_blanks();
            if self.lex.peek() == b'&' && self.lex.peek_at(1) == b'&' {
                self.lex.bump_n(2);
                self.skip_separators();
                rest.push(AndOr::And(self.pipeline()?));
            } else if self.lex.peek() == b'|' && self.lex.peek_at(1) == b'|' {
                self.lex.bump_n(2);
                self.skip_separators();
                rest.push(AndOr::Or(self.pipeline()?));
            } else {
                break;
            }
        }
        Ok(AndOrList { first, rest })
    }

    /// Parse a pipeline: `[!] executable ( | executable )*`
    fn pipeline(&mut self) -> Result<Pipeline<'a>, ParseError> {
        self.lex.skip_blanks();
        let negated = self.lex.peek() == b'!' && is_meta(self.lex.peek_at(1));
        if negated {
            self.lex.bump();
            self.lex.skip_blanks();
        }
        let first = self.executable()?;
        self.lex.skip_blanks();
        if self.lex.peek() != b'|' || self.lex.peek_at(1) == b'|' {
            return if negated {
                Ok(Pipeline::Pipe(true, vec![first]))
            } else {
                Ok(Pipeline::Single(first))
            };
        }
        let mut cmds = vec![first];
        while self.lex.peek() == b'|' && self.lex.peek_at(1) != b'|' {
            // Check for |& (pipe stderr too) — treat as 2>&1 |
            let pipe_stderr = self.lex.peek_at(1) == b'&';
            self.lex.bump(); // skip |
            if pipe_stderr {
                self.lex.bump(); // skip &
                // Add 2>&1 redirect to previous command
                let redir_2to1 =
                    Redir::DupWrite(Some(2), Word::Simple(WordPart::Bare(Atom::Lit("1"))));
                Self::add_redirect_to_exec(cmds.last_mut().unwrap(), redir_2to1);
            }
            self.skip_separators();
            cmds.push(self.executable()?);
            self.lex.skip_blanks();
        }
        Ok(Pipeline::Pipe(negated, cmds))
    }

    fn add_redirect_to_exec(exec: &mut Executable<'a>, redir: Redir<'a>) {
        match exec {
            Executable::Simple(cmd) => {
                cmd.suffix.push(CmdSuffix::Redirect(redir));
            }
            Executable::Compound(cmd) | Executable::FuncDef(_, cmd) => {
                cmd.redirects.push(redir);
            }
        }
    }

    /// Parse a single executable: compound command, function def, or simple command.
    fn executable(&mut self) -> Result<Executable<'a>, ParseError> {
        self.lex.skip_blanks();

        // Standalone (( )) arithmetic
        if self.lex.peek() == b'(' && self.lex.peek_at(1) == b'(' {
            let kind = self.standalone_arith()?;
            return self.wrap_compound(kind);
        }

        // Compound commands by keyword / delimiter
        let b = self.lex.peek();
        if b == b'{' && is_meta(self.lex.peek_at(1)) {
            let kind = self.brace_group()?;
            return self.wrap_compound(kind);
        }
        if b == b'(' && self.lex.peek_at(1) != b'(' {
            let kind = self.subshell()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"for") {
            let kind = self.for_cmd()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"while") {
            let kind = self.while_cmd()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"until") {
            let kind = self.until_cmd()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"if") {
            let kind = self.if_cmd()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"case") {
            let kind = self.case_cmd()?;
            return self.wrap_compound(kind);
        }
        if self.lex.at_keyword(b"select") {
            return Err(self.lex.err("unsupported: select loop"));
        }
        if self.lex.at_keyword(b"[[") {
            let kind = self.double_bracket()?;
            return self.wrap_compound(kind);
        }

        // Check for function definition: name()
        if self.at_func_def() {
            return self.func_def();
        }

        // Simple command
        Ok(Executable::Simple(self.simple_cmd()?))
    }

    /// Wrap a compound kind with trailing redirects into an Executable.
    fn wrap_compound(&mut self, kind: CompoundKind<'a>) -> Result<Executable<'a>, ParseError> {
        Ok(Executable::Compound(CompoundCmd {
            kind,
            redirects: self.collect_redirects()?,
        }))
    }

    // -----------------------------------------------------------------------
    // Compound commands
    // -----------------------------------------------------------------------

    fn for_cmd(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"for");
        self.lex.skip_blanks();

        // C-style for (( init; cond; step ))
        if self.lex.peek() == b'(' && self.lex.peek_at(1) == b'(' {
            return self.c_style_for();
        }

        let var = self.lex.read_name();
        if var.is_empty() {
            return Err(self.lex.err("expected variable name after 'for'"));
        }
        self.lex.skip_blanks();

        let words = if self.lex.at_keyword(b"in") {
            self.lex.eat_str(b"in");
            self.lex.skip_blanks();
            let mut words = Vec::new();
            while !self.at_terminator() && !self.lex.at_keyword(b"do") {
                words.push(self.word()?);
                self.lex.skip_blanks();
            }
            Some(words)
        } else {
            None
        };

        self.eat_separator();
        self.expect(b"do", "expected 'do' after for loop header")?;
        let body = self.cmd_list(&[b"done"])?;
        self.expect(b"done", "expected 'done' to close for loop")?;

        Ok(CompoundKind::For { var, words, body })
    }

    fn c_style_for(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.bump_n(2); // skip ((
        self.lex.skip_blanks();

        let init = if self.lex.peek() == b';' {
            None
        } else {
            Some(self.arith(0)?)
        };
        self.lex.skip_blanks();
        if !self.lex.eat(b';') {
            return Err(self.lex.err("expected ';' in C-style for"));
        }
        self.lex.skip_blanks();

        let cond = if self.lex.peek() == b';' {
            None
        } else {
            Some(self.arith(0)?)
        };
        self.lex.skip_blanks();
        if !self.lex.eat(b';') {
            return Err(self.lex.err("expected ';' in C-style for"));
        }
        self.lex.skip_blanks();

        let step = if self.lex.peek() == b')' && self.lex.peek_at(1) == b')' {
            None
        } else {
            Some(self.arith(0)?)
        };
        self.lex.skip_blanks();

        if !(self.lex.peek() == b')' && self.lex.peek_at(1) == b')') {
            return Err(self.lex.err("expected '))' in C-style for"));
        }
        self.lex.bump_n(2);

        self.eat_separator();
        self.expect(b"do", "expected 'do' after for((...)) header")?;
        let body = self.cmd_list(&[b"done"])?;
        self.expect(b"done", "expected 'done' to close for loop")?;

        Ok(CompoundKind::CFor {
            init,
            cond,
            step,
            body,
        })
    }

    fn while_cmd(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"while");
        self.skip_separators();
        let guard = self.cmd_list(&[b"do"])?;
        self.expect(b"do", "expected 'do' after while condition")?;
        let body = self.cmd_list(&[b"done"])?;
        self.expect(b"done", "expected 'done' to close while loop")?;
        Ok(CompoundKind::While(GuardBody { guard, body }))
    }

    fn until_cmd(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"until");
        self.skip_separators();
        let guard = self.cmd_list(&[b"do"])?;
        self.expect(b"do", "expected 'do' after until condition")?;
        let body = self.cmd_list(&[b"done"])?;
        self.expect(b"done", "expected 'done' to close until loop")?;
        Ok(CompoundKind::Until(GuardBody { guard, body }))
    }

    fn if_cmd(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"if");
        self.skip_separators();

        let mut conditionals = Vec::new();
        let guard = self.cmd_list(&[b"then"])?;
        self.expect(b"then", "expected 'then' after if condition")?;
        let body = self.cmd_list(&[b"elif", b"else", b"fi"])?;
        conditionals.push(GuardBody { guard, body });

        while self.lex.at_keyword(b"elif") {
            self.lex.eat_str(b"elif");
            self.skip_separators();
            let guard = self.cmd_list(&[b"then"])?;
            self.expect(b"then", "expected 'then' after elif condition")?;
            let body = self.cmd_list(&[b"elif", b"else", b"fi"])?;
            conditionals.push(GuardBody { guard, body });
        }

        let else_branch = if self.lex.at_keyword(b"else") {
            self.lex.eat_str(b"else");
            self.skip_separators();
            Some(self.cmd_list(&[b"fi"])?)
        } else {
            None
        };

        self.expect(b"fi", "expected 'fi' to close if statement")?;
        Ok(CompoundKind::If {
            conditionals,
            else_branch,
        })
    }

    fn case_cmd(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"case");
        self.lex.skip_blanks();
        let word = self.word()?;
        self.lex.skip_blanks();
        self.expect(b"in", "expected 'in' after case word")?;
        self.skip_separators();

        let mut arms = Vec::new();
        while !self.lex.at_keyword(b"esac") && !self.lex.is_eof() {
            // Optional ( before patterns
            self.lex.skip_blanks();
            self.lex.eat(b'(');
            self.lex.skip_blanks();

            let mut patterns = Vec::new();
            patterns.push(self.word()?);
            self.lex.skip_blanks();
            while self.lex.eat(b'|') {
                self.lex.skip_blanks();
                patterns.push(self.word()?);
                self.lex.skip_blanks();
            }

            self.lex.skip_blanks();
            self.lex.eat(b')');
            // Only skip whitespace/newlines here — NOT semicolons.
            // A bare ;; right after ) means an empty body; skip_separators
            // would eat the ;; and break the terminator check.
            self.lex.skip_blanks();
            while self.lex.peek() == b'\n' {
                self.lex.bump();
                self.lex.skip_blanks();
            }

            let body = self.case_body()?;

            // Eat ;; if present, error on ;& and ;;&
            self.lex.skip_blanks();
            if self.lex.peek() == b';' && self.lex.peek_at(1) == b';' {
                if self.lex.peek_at(2) == b'&' {
                    return Err(self.lex.err("unsupported: case ;;&"));
                }
                self.lex.bump_n(2);
            } else if self.lex.peek() == b';' && self.lex.peek_at(1) == b'&' {
                return Err(self.lex.err("unsupported: case fallthrough ;&"));
            }
            self.skip_separators();

            arms.push(CaseArm { patterns, body });
        }

        self.expect(b"esac", "expected 'esac' to close case statement")?;
        Ok(CompoundKind::Case { word, arms })
    }

    /// Parse case arm body — like `cmd_list` but stops at `;;`, `;&`, `;;&` and `esac`.
    fn case_body(&mut self) -> Result<Vec<Cmd<'a>>, ParseError> {
        let mut cmds = Vec::new();
        loop {
            // Skip separators but preserve ;; and ;&
            loop {
                self.lex.skip_blanks();
                if self.lex.peek() == b';' && matches!(self.lex.peek_at(1), b';' | b'&') {
                    break;
                }
                match self.lex.peek() {
                    b';' | b'\n' => self.lex.bump(),
                    b'#' => self.lex.skip_comment(),
                    _ => break,
                }
            }
            if self.lex.is_eof() || self.lex.at_keyword(b"esac") {
                break;
            }
            if self.lex.peek() == b';' && matches!(self.lex.peek_at(1), b';' | b'&') {
                break;
            }
            cmds.push(self.cmd()?);
        }
        Ok(cmds)
    }

    fn brace_group(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat(b'{');
        self.skip_separators();
        let body = self.cmd_list(&[b"}"])?;
        self.lex.skip_blanks();
        if !self.lex.eat(b'}') {
            return Err(self.lex.err("expected '}'"));
        }
        Ok(CompoundKind::Brace(body))
    }

    fn subshell(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat(b'(');
        self.skip_separators();
        let body = self.cmd_list(&[b")"])?;
        self.lex.skip_blanks();
        if !self.lex.eat(b')') {
            return Err(self.lex.err("expected ')'"));
        }
        Ok(CompoundKind::Subshell(body))
    }

    /// Parse `[[ ... ]]` — split internal `&&`/`||` into an and-or list.
    fn double_bracket(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.eat_str(b"[[");
        self.lex.skip_blanks();

        // Collect all tokens inside [[ ]] as a command list.
        // We need to handle && and || inside [[ ]] as splitting points.
        let mut segments: Vec<(Vec<Word<'a>>, Option<&'a str>)> = Vec::new();
        let mut current_words = Vec::new();

        loop {
            self.lex.skip_blanks();
            if self.lex.is_eof() {
                return Err(self.lex.err("unterminated [["));
            }
            // Check for ]]
            if self.lex.peek() == b']' && self.lex.peek_at(1) == b']' {
                self.lex.bump_n(2);
                segments.push((current_words, None));
                break;
            }
            // Check for && or || inside [[ ]]
            if self.lex.peek() == b'&' && self.lex.peek_at(1) == b'&' {
                segments.push((current_words, Some("&&")));
                current_words = Vec::new();
                self.lex.bump_n(2);
                continue;
            }
            if self.lex.peek() == b'|' && self.lex.peek_at(1) == b'|' {
                segments.push((current_words, Some("||")));
                current_words = Vec::new();
                self.lex.bump_n(2);
                continue;
            }
            current_words.push(self.word_bracket()?);
        }

        // Build command list from segments:
        // [[ a ]] && [[ b ]] || [[ c ]]
        // Each segment becomes a simple command with [[ as the command name
        // and ]] appended, connected by and-or.
        fn build_test_cmd(words: Vec<Word<'_>>) -> Cmd<'_> {
            let mut suffix = Vec::new();
            suffix.push(CmdSuffix::Word(Word::Simple(WordPart::Bare(Atom::Lit(
                "[[",
            )))));
            for w in words {
                suffix.push(CmdSuffix::Word(w));
            }
            suffix.push(CmdSuffix::Word(Word::Simple(WordPart::Bare(Atom::Lit(
                "]]",
            )))));
            Cmd::List(AndOrList {
                first: Pipeline::Single(Executable::Simple(SimpleCmd {
                    prefix: Vec::new(),
                    suffix,
                })),
                rest: Vec::new(),
            })
        }

        if segments.len() == 1 {
            let (words, _) = segments.into_iter().next().unwrap();
            return Ok(CompoundKind::DoubleBracket(vec![build_test_cmd(words)]));
        }

        // Multiple segments — build an and-or list
        let mut iter = segments.into_iter();
        let (first_words, first_op) = iter.next().unwrap();
        let first_cmd = build_test_cmd(first_words);

        let first_pipeline = Pipeline::Single(Executable::Compound(CompoundCmd {
            kind: CompoundKind::DoubleBracket(vec![first_cmd]),
            redirects: Vec::new(),
        }));

        let mut rest = Vec::new();
        let mut pending_op = first_op;

        for (words, op) in iter {
            let test_cmd = build_test_cmd(words);
            let pipe = Pipeline::Single(Executable::Compound(CompoundCmd {
                kind: CompoundKind::DoubleBracket(vec![test_cmd]),
                redirects: Vec::new(),
            }));
            match pending_op {
                Some("||") => rest.push(AndOr::Or(pipe)),
                _ => rest.push(AndOr::And(pipe)),
            }
            pending_op = op;
        }

        // Wrap the whole and-or list as a single command
        let combined = Cmd::List(AndOrList {
            first: first_pipeline,
            rest,
        });

        Ok(CompoundKind::DoubleBracket(vec![combined]))
    }

    /// Parse `(( expr ))` at command position.
    fn standalone_arith(&mut self) -> Result<CompoundKind<'a>, ParseError> {
        self.lex.bump_n(2); // skip ((
        self.lex.skip_blanks();

        let arith = self.arith(0)?;

        self.lex.skip_blanks();
        if self.lex.peek() == b')' && self.lex.peek_at(1) == b')' {
            self.lex.bump_n(2);
            Ok(CompoundKind::Arithmetic(arith))
        } else {
            Err(self.lex.err("expected '))'"))
        }
    }

    fn func_def(&mut self) -> Result<Executable<'a>, ParseError> {
        // Optional 'function' keyword
        if self.lex.at_keyword(b"function") {
            self.lex.eat_str(b"function");
            self.lex.skip_blanks();
        }
        let name = self.lex.read_name();
        if name.is_empty() {
            return Err(self.lex.err("expected function name"));
        }
        self.lex.skip_blanks();
        // Eat ()
        if self.lex.eat(b'(') {
            self.lex.skip_blanks();
            if !self.lex.eat(b')') {
                return Err(self.lex.err("expected ')' in function definition"));
            }
        }
        self.skip_separators();

        // Body must be a compound command (usually { ... })
        let kind = if self.lex.peek() == b'{' && is_meta(self.lex.peek_at(1)) {
            self.brace_group()?
        } else if self.lex.eat(b'(') {
            self.subshell()?
        } else {
            return Err(self.lex.err("expected '{' or '(' after function name"));
        };

        Ok(Executable::FuncDef(
            name,
            CompoundCmd {
                kind,
                redirects: self.collect_redirects()?,
            },
        ))
    }

    // -----------------------------------------------------------------------
    // Simple command
    // -----------------------------------------------------------------------

    fn simple_cmd(&mut self) -> Result<SimpleCmd<'a>, ParseError> {
        let mut prefix = Vec::new();
        let mut suffix = Vec::new();
        let mut saw_word = false;

        loop {
            self.lex.skip_blanks();
            if self.at_terminator() {
                break;
            }

            // Try redirect
            if let Some(redir) = self.try_redirect()? {
                if saw_word {
                    suffix.push(CmdSuffix::Redirect(redir));
                } else {
                    prefix.push(CmdPrefix::Redirect(redir));
                }
                continue;
            }

            // Before the command name: assignments are possible
            if !saw_word && let Some(assign) = self.try_assignment()? {
                prefix.push(assign);
                continue;
            }

            // Regular word
            suffix.push(CmdSuffix::Word(self.word()?));
            saw_word = true;
        }

        Ok(SimpleCmd { prefix, suffix })
    }

    /// Try to parse an assignment: `NAME=value`, `NAME=(word ...)`, or `NAME+=(word ...)`.
    /// Returns None if not at an assignment (doesn't consume anything).
    fn try_assignment(&mut self) -> Result<Option<CmdPrefix<'a>>, ParseError> {
        let start = self.lex.pos();
        let name = self.lex.read_name();
        if name.is_empty() {
            self.rewind(start);
            return Ok(None);
        }

        // Check for += (array append)
        let is_append = self.lex.peek() == b'+' && self.lex.peek_at(1) == b'=';
        if is_append {
            self.lex.bump_n(2); // skip +=
        } else if self.lex.peek() == b'=' {
            self.lex.bump(); // skip =
        } else {
            self.rewind(start);
            return Ok(None);
        }

        // Array assignment: NAME=(word ...) or NAME+=(word ...)
        if self.lex.peek() == b'(' {
            self.lex.bump(); // skip (
            let words = self.array_elements()?;
            if is_append {
                return Ok(Some(CmdPrefix::ArrayAppend(name, words)));
            }
            return Ok(Some(CmdPrefix::ArrayAssign(name, words)));
        }

        // NAME=value or NAME+=value — parse the value if present
        let value = if self.lex.peek() == 0 || is_meta(self.lex.peek()) {
            None
        } else {
            Some(self.word()?)
        };
        Ok(Some(CmdPrefix::Assign(name, value)))
    }

    /// Parse array elements inside `(...)`.
    fn array_elements(&mut self) -> Result<Vec<Word<'a>>, ParseError> {
        let mut words = Vec::new();
        loop {
            self.lex.skip_blanks();
            if self.lex.peek() == b')' {
                self.lex.bump();
                break;
            }
            if self.lex.is_eof() {
                return Err(self.lex.err("unterminated array"));
            }
            words.push(self.word()?);
        }
        Ok(words)
    }

    // -----------------------------------------------------------------------
    // Words
    // -----------------------------------------------------------------------

    /// Parse a complete word (may be a concatenation of multiple parts).
    /// Parse a word inside `[[ ]]` — `(` and `)` are not metacharacters here.
    fn word_bracket(&mut self) -> Result<Word<'a>, ParseError> {
        let mut parts = Vec::new();
        loop {
            if self.lex.is_eof() {
                break;
            }
            let b = self.lex.peek();
            // Inside [[ ]], ( and ) are literal (used in regex patterns)
            if b == b'(' || b == b')' {
                let start = self.lex.pos();
                self.lex.bump();
                parts.push(WordPart::Bare(Atom::Lit(self.lex.slice(start))));
                continue;
            }
            if is_meta(b) {
                break;
            }
            parts.push(self.word_part()?);
        }
        if parts.is_empty() {
            return Err(self.lex.err("expected word"));
        }
        if parts.len() == 1 {
            Ok(Word::Simple(parts.into_iter().next().unwrap()))
        } else {
            Ok(Word::Concat(parts))
        }
    }

    fn word(&mut self) -> Result<Word<'a>, ParseError> {
        let mut parts = Vec::new();
        loop {
            if self.lex.is_eof() {
                break;
            }
            let b = self.lex.peek();
            // Process substitution <( or >( — parse even though < and > are meta
            if b == b'<' && self.lex.peek_at(1) == b'(' {
                self.lex.bump_n(2);
                let cmds = self.cmd_list(&[b")"])?;
                self.lex.skip_blanks();
                if !self.lex.eat(b')') {
                    return Err(self.lex.err("expected ')' for process substitution"));
                }
                parts.push(WordPart::Bare(Atom::ProcSubIn(cmds)));
                continue;
            }
            if b == b'>' && self.lex.peek_at(1) == b'(' {
                return Err(self
                    .lex
                    .err("unsupported: output process substitution >(...)"));
            }
            if is_meta(b) {
                break;
            }
            parts.push(self.word_part()?);
        }
        if parts.is_empty() {
            return Err(self.lex.err("expected word"));
        }
        if parts.len() == 1 {
            Ok(Word::Simple(parts.into_iter().next().unwrap()))
        } else {
            Ok(Word::Concat(parts))
        }
    }

    /// Parse a single word part: bare atoms, double-quoted, or single-quoted.
    fn word_part(&mut self) -> Result<WordPart<'a>, ParseError> {
        match self.lex.peek() {
            b'"' => {
                self.lex.bump();
                let atoms = self.dquoted()?;
                Ok(WordPart::DQuoted(atoms))
            }
            b'\'' => {
                self.lex.bump();
                let content = self.lex.scan_squote()?;
                Ok(WordPart::SQuoted(content))
            }
            _ => {
                let atom = self.atom()?;
                Ok(WordPart::Bare(atom))
            }
        }
    }

    /// Parse atoms inside double quotes until closing `"`.
    fn dquoted(&mut self) -> Result<Vec<Atom<'a>>, ParseError> {
        let mut atoms = Vec::new();
        let mut lit_start = self.lex.pos();

        while !self.lex.is_eof() && self.lex.peek() != b'"' {
            match self.lex.peek() {
                b'$' => {
                    // Flush accumulated literal
                    if self.lex.pos() > lit_start {
                        atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                    }
                    atoms.push(self.dollar()?);
                    lit_start = self.lex.pos();
                }
                b'\\' => {
                    // Flush accumulated literal
                    if self.lex.pos() > lit_start {
                        atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                    }
                    self.lex.bump(); // skip backslash
                    if self.lex.is_eof() {
                        break;
                    }
                    let escaped_start = self.lex.pos();
                    self.lex.bump();
                    atoms.push(Atom::Escaped(Cow::Borrowed(self.lex.slice(escaped_start))));
                    lit_start = self.lex.pos();
                }
                b'`' => {
                    if self.lex.pos() > lit_start {
                        atoms.push(Atom::Lit(self.lex.slice(lit_start)));
                    }
                    atoms.push(self.backtick()?);
                    lit_start = self.lex.pos();
                }
                _ => {
                    self.lex.bump();
                }
            }
        }

        // Flush trailing literal
        if self.lex.pos() > lit_start {
            atoms.push(Atom::Lit(self.lex.slice(lit_start)));
        }

        if !self.lex.eat(b'"') {
            return Err(self.lex.err("unterminated double quote"));
        }
        Ok(atoms)
    }

    /// Parse a single atom in an unquoted context.
    fn atom(&mut self) -> Result<Atom<'a>, ParseError> {
        match self.lex.peek() {
            b'$' => self.dollar(),
            b'\\' => {
                self.lex.bump();
                if self.lex.is_eof() {
                    Ok(Atom::Lit(""))
                } else {
                    let start = self.lex.pos();
                    self.lex.bump();
                    Ok(Atom::Escaped(Cow::Borrowed(self.lex.slice(start))))
                }
            }
            b'*' => {
                self.lex.bump();
                Ok(Atom::Star)
            }
            b'?' => {
                self.lex.bump();
                Ok(Atom::Question)
            }
            b'[' => {
                self.lex.bump();
                Ok(Atom::SquareOpen)
            }
            b']' => {
                self.lex.bump();
                Ok(Atom::SquareClose)
            }
            b'~' => {
                self.lex.bump();
                Ok(Atom::Tilde)
            }
            b'{' => {
                // Try brace range {1..5}
                if let Some(br) = self.try_brace_range() {
                    Ok(br)
                } else {
                    // Adjacent brace expansion check: }{
                    let start = self.lex.pos();
                    self.lex.bump();
                    Ok(Atom::Lit(self.lex.slice(start)))
                }
            }
            b'`' => self.backtick(),
            _ => {
                // Read a run of literal characters
                let start = self.lex.pos();
                while !self.lex.is_eof() {
                    let b = self.lex.peek();
                    if is_meta(b)
                        || matches!(
                            b,
                            b'"' | b'\''
                                | b'$'
                                | b'\\'
                                | b'*'
                                | b'?'
                                | b'['
                                | b']'
                                | b'~'
                                | b'{'
                                | b'`'
                        )
                    {
                        break;
                    }
                    self.lex.bump();
                }
                let s = self.lex.slice(start);
                if s.is_empty() {
                    return Err(self.lex.err("unexpected character"));
                }
                Ok(Atom::Lit(s))
            }
        }
    }

    /// Parse `$...` expansion: `$var`, `${...}`, `$(...)`, `$((...))`, or special param.
    fn dollar(&mut self) -> Result<Atom<'a>, ParseError> {
        self.lex.bump(); // skip $

        match self.lex.peek() {
            b'{' => {
                self.lex.bump(); // skip {
                // ${!var} — indirect expansion, ${!var[@]} — array keys, ${!prefix*} — prefix list
                if self.lex.peek() == b'!' {
                    self.lex.bump();
                    let name = self.lex.read_name();
                    if !name.is_empty() && self.lex.peek() == b'[' {
                        self.lex.bump(); // skip [
                        let idx_byte = self.lex.peek();
                        if (idx_byte == b'@' || idx_byte == b'*') && self.lex.peek_at(1) == b']' {
                            self.lex.bump_n(2); // skip @] or *]
                            if !self.lex.eat(b'}') {
                                return Err(self.lex.err("expected '}'"));
                            }
                            return Err(self.lex.err("unsupported: ${!arr[@]} indirect/keys"));
                        }
                    }
                    // ${!prefix*} or ${!prefix@} — list variable names matching prefix
                    if !name.is_empty() && matches!(self.lex.peek(), b'*' | b'@') {
                        self.lex.bump(); // skip * or @
                        if !self.lex.eat(b'}') {
                            return Err(self.lex.err("expected '}'"));
                        }
                        return Ok(Atom::Subst(Box::new(Subst::PrefixList(name))));
                    }
                    if name.is_empty() {
                        return Err(self.lex.err("expected variable name after ${!"));
                    }
                    if !self.lex.eat(b'}') {
                        return Err(self.lex.err("expected '}'"));
                    }
                    return Ok(Atom::Subst(Box::new(Subst::Indirect(name))));
                }
                // ${#param} or ${#arr[@]} — length
                if self.lex.peek() == b'#' && self.lex.peek_at(1) != b'}' {
                    self.lex.bump();
                    let param = self.read_param()?;
                    // Check for ${#arr[@]} — array length
                    if let Param::Var(name) = param
                        && self.lex.peek() == b'['
                    {
                        self.lex.bump(); // skip [
                        let idx_byte = self.lex.peek();
                        if (idx_byte == b'@' || idx_byte == b'*') && self.lex.peek_at(1) == b']' {
                            self.lex.bump_n(2); // skip @] or *]
                            if !self.lex.eat(b'}') {
                                return Err(self.lex.err("expected '}'"));
                            }
                            return Ok(Atom::Subst(Box::new(Subst::ArrayLen(name))));
                        }
                        return Err(self.lex.err("expected '@]' or '*]' after '#arr['"));
                    }
                    if !self.lex.eat(b'}') {
                        return Err(self.lex.err("expected '}'"));
                    }
                    return Ok(Atom::Subst(Box::new(Subst::Len(param))));
                }
                let param = self.read_param()?;
                // Check for array indexing: ${arr[...]}
                if let Param::Var(name) = param
                    && self.lex.peek() == b'['
                {
                    return self.brace_array_op(name);
                }
                if self.lex.peek() == b'}' {
                    // Bare ${var} — same as $var
                    self.lex.bump();
                    return Ok(Atom::Param(param));
                }
                let subst = self.brace_param_op(param)?;
                Ok(Atom::Subst(Box::new(subst)))
            }
            b'(' => {
                if self.lex.peek_at(1) == b'(' {
                    // $(( arithmetic ))
                    self.lex.bump_n(2); // skip ((
                    let subst = self.arith_subst()?;
                    Ok(Atom::Subst(Box::new(subst)))
                } else {
                    // $( command )
                    self.lex.bump(); // skip (
                    let subst = self.cmd_subst()?;
                    Ok(Atom::Subst(Box::new(subst)))
                }
            }
            b'\'' => {
                // $'...' ANSI-C quoting — scan to closing ', handling \'
                self.lex.bump(); // skip opening '
                let start = self.lex.pos();
                loop {
                    if self.lex.is_eof() {
                        return Err(self.lex.err("unterminated ANSI-C quote"));
                    }
                    if self.lex.peek() == b'\\' {
                        self.lex.bump();
                        if !self.lex.is_eof() {
                            self.lex.bump();
                        }
                        continue;
                    }
                    if self.lex.peek() == b'\'' {
                        let content = self.lex.slice(start);
                        self.lex.bump();
                        return Ok(Atom::AnsiCQuoted(content));
                    }
                    self.lex.bump();
                }
            }
            b'@' => {
                self.lex.bump();
                Ok(Atom::Param(Param::At))
            }
            b'*' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Star))
            }
            b'#' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Pound))
            }
            b'?' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Status))
            }
            b'$' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Pid))
            }
            b'!' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Bang))
            }
            b'-' => {
                self.lex.bump();
                Ok(Atom::Param(Param::Dash))
            }
            b'0'..=b'9' => {
                let start = self.lex.pos();
                self.lex.bump();
                // Multi-digit only for ${N} syntax, bare $N is single digit
                let s = self.lex.slice(start);
                let n: u32 = s.parse().unwrap_or(0);
                Ok(Atom::Param(Param::Positional(n)))
            }
            _ => {
                // $NAME
                let name = self.lex.read_name();
                if name.is_empty() {
                    // Bare $ — emit as literal
                    Ok(Atom::Lit("$"))
                } else {
                    Ok(Atom::Param(Param::Var(name)))
                }
            }
        }
    }

    /// Parse the operator part of `${param OP word}`. The param has already been
    /// read; cursor is on the operator byte.
    fn brace_param_op(&mut self, param: Param<'a>) -> Result<Subst<'a>, ParseError> {
        // Colon prefix: ${var:-word}, ${var:=word}, etc. or substring ${var:offset:length}
        if self.lex.peek() == b':' {
            self.lex.bump();
            match self.lex.peek() {
                b'-' | b'=' | b'?' | b'+' => {}
                _ => {
                    // Substring: ${var:offset} or ${var:offset:length}
                    let offset_start = self.lex.pos();
                    self.scan_substring_part();
                    let offset = self.lex.slice(offset_start);
                    let length = if self.lex.eat(b':') {
                        let len_start = self.lex.pos();
                        self.scan_substring_part();
                        Some(self.lex.slice(len_start))
                    } else {
                        None
                    };
                    if !self.lex.eat(b'}') {
                        return Err(self.lex.err("expected '}'"));
                    }
                    return Ok(Subst::Substring(param, offset, length));
                }
            }
        }

        match self.lex.peek() {
            // Default/assign/error/alt — shared logic for colon and non-colon forms
            b'-' | b'=' | b'?' | b'+' => {
                let op = self.lex.peek();
                self.lex.bump();
                let word = self.brace_param_word()?;
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                match op {
                    b'-' => Ok(Subst::Default(param, word)),
                    b'=' => Ok(Subst::Assign(param, word)),
                    b'?' => Ok(Subst::Error(param, word)),
                    b'+' => Ok(Subst::Alt(param, word)),
                    _ => unreachable!(),
                }
            }
            b'%' => {
                self.lex.bump();
                let large = self.lex.eat(b'%');
                let word = self.brace_param_word()?;
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                if large {
                    Ok(Subst::TrimSuffixLarge(param, word))
                } else {
                    Ok(Subst::TrimSuffixSmall(param, word))
                }
            }
            b'#' => {
                self.lex.bump();
                let large = self.lex.eat(b'#');
                let word = self.brace_param_word()?;
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                if large {
                    Ok(Subst::TrimPrefixLarge(param, word))
                } else {
                    Ok(Subst::TrimPrefixSmall(param, word))
                }
            }
            b'^' => {
                self.lex.bump();
                let all = self.lex.eat(b'^');
                if !self.lex.eat(b'}') {
                    return Err(self
                        .lex
                        .err("expected '}' (patterned case modification unsupported)"));
                }
                Ok(Subst::Upper(all, param))
            }
            b',' => {
                self.lex.bump();
                let all = self.lex.eat(b',');
                if !self.lex.eat(b'}') {
                    return Err(self
                        .lex
                        .err("expected '}' (patterned case modification unsupported)"));
                }
                Ok(Subst::Lower(all, param))
            }
            b'/' => {
                self.lex.bump();
                let (all, prefix, suffix) = match self.lex.peek() {
                    b'/' => {
                        self.lex.bump();
                        (true, false, false)
                    }
                    b'#' => {
                        self.lex.bump();
                        (false, true, false)
                    }
                    b'%' => {
                        self.lex.bump();
                        (false, false, true)
                    }
                    _ => (false, false, false),
                };
                let pattern = self.brace_param_word_until_slash()?;
                let replacement = if self.lex.eat(b'/') {
                    self.brace_param_word()?
                } else {
                    None
                };
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                if prefix {
                    Ok(Subst::ReplacePrefix(param, pattern, replacement))
                } else if suffix {
                    Ok(Subst::ReplaceSuffix(param, pattern, replacement))
                } else if all {
                    Ok(Subst::ReplaceAll(param, pattern, replacement))
                } else {
                    Ok(Subst::Replace(param, pattern, replacement))
                }
            }
            b'@' => {
                self.lex.bump();
                let op = self.lex.peek();
                if !matches!(
                    op,
                    b'Q' | b'E' | b'P' | b'A' | b'K' | b'a' | b'u' | b'U' | b'L'
                ) {
                    return Err(self
                        .lex
                        .err("unsupported parameter transformation operator"));
                }
                self.lex.bump();
                let Param::Var(name) = param else {
                    return Err(self
                        .lex
                        .err("parameter transformation requires a named variable"));
                };
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                Ok(Subst::Transform(name, op))
            }
            _ => Err(self.lex.err("unsupported parameter expansion")),
        }
    }

    /// Parse array indexing after `${name[`.
    /// Handles `${arr[n]}`, `${arr[@]}`, `${arr[*]}`, `${arr[@]:offset:len}`.
    fn brace_array_op(&mut self, name: &'a str) -> Result<Atom<'a>, ParseError> {
        self.lex.bump(); // skip [

        let idx_byte = self.lex.peek();
        if idx_byte == b'@' || idx_byte == b'*' {
            self.lex.bump();
            if !self.lex.eat(b']') {
                return Err(self.lex.err("expected ']'"));
            }
            // Check for slice: ${arr[@]:offset:length}
            if self.lex.peek() == b':' {
                self.lex.bump(); // skip :
                let offset = self.read_brace_number()?;
                let length = if self.lex.eat(b':') {
                    Some(self.read_brace_number()?)
                } else {
                    None
                };
                if !self.lex.eat(b'}') {
                    return Err(self.lex.err("expected '}'"));
                }
                return Ok(Atom::Subst(Box::new(Subst::ArraySlice(
                    name, offset, length,
                ))));
            }
            if !self.lex.eat(b'}') {
                return Err(self.lex.err("expected '}'"));
            }
            return Ok(Atom::Subst(Box::new(Subst::ArrayAll(name))));
        }

        // Numeric or expression index: ${arr[n]} or ${arr[$((expr))]}
        // Read index as a word (supports $var, $((expr)), etc.)
        let idx_word = self.array_index_word()?;
        if !self.lex.eat(b']') {
            return Err(self.lex.err("expected ']'"));
        }
        if !self.lex.eat(b'}') {
            return Err(self.lex.err("expected '}'"));
        }
        Ok(Atom::Subst(Box::new(Subst::ArrayElement(name, idx_word))))
    }

    /// Read a number in `${arr[@]:offset:length}` context.
    fn read_brace_number(&mut self) -> Result<&'a str, ParseError> {
        let start = self.lex.pos();
        // Allow optional leading minus
        if self.lex.peek() == b'-' {
            self.lex.bump();
        }
        while self.lex.peek().is_ascii_digit() {
            self.lex.bump();
        }
        let s = self.lex.slice(start);
        if s.is_empty() || s == "-" {
            return Err(self.lex.err("expected number in array slice"));
        }
        Ok(s)
    }

    /// Parse an array index word (inside `[...]`), stopping at `]`.
    fn array_index_word(&mut self) -> Result<Word<'a>, ParseError> {
        let mut parts = Vec::new();
        loop {
            let b = self.lex.peek();
            if self.lex.is_eof() || b == b']' {
                break;
            }
            match b {
                b'$' => {
                    parts.push(WordPart::Bare(self.dollar()?));
                }
                b'"' => {
                    self.lex.bump();
                    parts.push(WordPart::DQuoted(self.dquoted()?));
                }
                _ => {
                    let start = self.lex.pos();
                    while !self.lex.is_eof()
                        && self.lex.peek() != b']'
                        && self.lex.peek() != b'$'
                        && self.lex.peek() != b'"'
                    {
                        self.lex.bump();
                    }
                    let s = self.lex.slice(start);
                    if !s.is_empty() {
                        parts.push(WordPart::Bare(Atom::Lit(s)));
                    }
                }
            }
        }
        if parts.is_empty() {
            return Err(self.lex.err("empty array index"));
        }
        if parts.len() == 1 {
            Ok(Word::Simple(parts.into_iter().next().unwrap()))
        } else {
            Ok(Word::Concat(parts))
        }
    }

    /// Read a Param from the current position (for ${...} parsing).
    fn read_param(&mut self) -> Result<Param<'a>, ParseError> {
        match self.lex.peek() {
            b'@' => {
                self.lex.bump();
                Ok(Param::At)
            }
            b'*' => {
                self.lex.bump();
                Ok(Param::Star)
            }
            b'#' => {
                self.lex.bump();
                Ok(Param::Pound)
            }
            b'?' => {
                self.lex.bump();
                Ok(Param::Status)
            }
            b'$' => {
                self.lex.bump();
                Ok(Param::Pid)
            }
            b'!' => {
                self.lex.bump();
                Ok(Param::Bang)
            }
            b'-' => {
                self.lex.bump();
                Ok(Param::Dash)
            }
            b'0'..=b'9' => {
                let num = self.lex.read_number();
                let n: u32 = num.parse().unwrap_or(0);
                Ok(Param::Positional(n))
            }
            _ => {
                let name = self.lex.read_name();
                if name.is_empty() {
                    Err(self.lex.err("expected parameter name"))
                } else {
                    Ok(Param::Var(name))
                }
            }
        }
    }

    /// Parse a word inside `${...}` — stops at unquoted `}`.
    #[inline]
    fn brace_param_word(&mut self) -> Result<Option<Word<'a>>, ParseError> {
        self.brace_param_word_until(b'\0') // NUL never appears — no extra stop
    }

    /// Like `brace_param_word` but also stops at unquoted `/`.
    #[inline]
    fn brace_param_word_until_slash(&mut self) -> Result<Option<Word<'a>>, ParseError> {
        self.brace_param_word_until(b'/')
    }

    /// Core: parse a word inside `${...}`, stopping at `}` or `extra_stop`.
    fn brace_param_word_until(&mut self, extra: u8) -> Result<Option<Word<'a>>, ParseError> {
        if self.lex.peek() == b'}' || (extra != 0 && self.lex.peek() == extra) {
            return Ok(None);
        }
        let mut parts = Vec::new();
        loop {
            let b = self.lex.peek();
            if self.lex.is_eof() || b == b'}' || (extra != 0 && b == extra) {
                break;
            }
            match b {
                b'"' => {
                    self.lex.bump();
                    parts.push(WordPart::DQuoted(self.dquoted()?));
                }
                b'\'' => {
                    self.lex.bump();
                    parts.push(WordPart::SQuoted(self.lex.scan_squote()?));
                }
                b'$' => {
                    parts.push(WordPart::Bare(self.dollar()?));
                }
                b'\\' => {
                    self.lex.bump();
                    if self.lex.is_eof() {
                        break;
                    }
                    let start = self.lex.pos();
                    self.lex.bump();
                    parts.push(WordPart::Bare(Atom::Escaped(Cow::Borrowed(
                        self.lex.slice(start),
                    ))));
                }
                b'*' => {
                    self.lex.bump();
                    parts.push(WordPart::Bare(Atom::Star));
                }
                b'?' => {
                    self.lex.bump();
                    parts.push(WordPart::Bare(Atom::Question));
                }
                _ => {
                    let start = self.lex.pos();
                    while !self.lex.is_eof() {
                        let c = self.lex.peek();
                        if c == b'}'
                            || c == b'"'
                            || c == b'\''
                            || c == b'$'
                            || c == b'\\'
                            || c == b'*'
                            || c == b'?'
                            || (extra != 0 && c == extra)
                        {
                            break;
                        }
                        self.lex.bump();
                    }
                    if self.lex.pos() > start {
                        parts.push(WordPart::Bare(Atom::Lit(self.lex.slice(start))));
                    }
                }
            }
        }
        match parts.len() {
            0 => Ok(None),
            1 => Ok(Some(Word::Simple(parts.into_iter().next().unwrap()))),
            _ => Ok(Some(Word::Concat(parts))),
        }
    }

    /// Scan substring offset/length — stops at unquoted `:` or `}`, tracking nesting.
    fn scan_substring_part(&mut self) {
        let mut depth: i32 = 0;
        while !self.lex.is_eof() {
            let b = self.lex.peek();
            if depth == 0 && (b == b':' || b == b'}') {
                break;
            }
            match b {
                b'(' | b'{' => depth += 1,
                b')' | b'}' => depth -= 1,
                _ => {}
            }
            self.lex.bump();
        }
    }

    /// Parse `$(command)` — cursor is after `$(`.
    fn cmd_subst(&mut self) -> Result<Subst<'a>, ParseError> {
        let cmds = self.cmd_list(&[b")"])?;
        self.lex.skip_blanks();
        if !self.lex.eat(b')') {
            return Err(self.lex.err("expected ')' for command substitution"));
        }
        Ok(Subst::Cmd(cmds))
    }

    /// Parse `$((expr))` — cursor is after `$((`.
    fn arith_subst(&mut self) -> Result<Subst<'a>, ParseError> {
        self.lex.skip_blanks();
        if self.lex.peek() == b')' && self.lex.peek_at(1) == b')' {
            self.lex.bump_n(2);
            return Ok(Subst::Arith(None));
        }
        let expr = self.arith(0)?;
        self.lex.skip_blanks();
        if self.lex.peek() == b')' && self.lex.peek_at(1) == b')' {
            self.lex.bump_n(2);
            Ok(Subst::Arith(Some(expr)))
        } else {
            Err(self.lex.err("expected '))' for arithmetic"))
        }
    }

    /// Parse backtick command substitution: `` `...` ``
    fn backtick(&mut self) -> Result<Atom<'a>, ParseError> {
        self.lex.bump(); // skip opening `
        let start = self.lex.pos();
        while !self.lex.is_eof() && self.lex.peek() != b'`' {
            if self.lex.peek() == b'\\' {
                self.lex.bump(); // skip escaped char
            }
            self.lex.bump();
        }
        let content = self.lex.slice(start);
        if !self.lex.eat(b'`') {
            return Err(self.lex.err("unterminated backtick"));
        }
        // Re-parse the content as a command
        let sub_parser = Parser::new(content);
        let cmds = sub_parser.parse()?;
        Ok(Atom::Subst(Box::new(Subst::Cmd(cmds))))
    }

    /// Try to parse `{start..end[..step]}` brace range.
    /// Returns None if not a brace range (doesn't consume).
    fn try_brace_range(&mut self) -> Option<Atom<'a>> {
        let start_pos = self.lex.pos();
        if self.lex.peek() != b'{' {
            return None;
        }

        // Scan ahead to find } and check for ..
        let src = &self.lex.remaining().as_bytes()[1..]; // after {
        let close = src.iter().position(|&b| b == b'}')?;
        let inner_start = start_pos + 1;
        let inner_end = inner_start + close;
        let inner = self.lex.slice_range(inner_start, inner_end);

        // Must contain ..
        let dot_pos = inner.find("..")?;
        if dot_pos == 0 {
            return None;
        }
        let first = &inner[..dot_pos];
        let rest = &inner[dot_pos + 2..];
        if rest.is_empty() {
            return None;
        }

        // Check for optional step: first..end..step
        let (end_val, step_val) = if let Some(dot2) = rest.find("..") {
            if dot2 == 0 || dot2 + 2 >= rest.len() {
                return None;
            }
            (&rest[..dot2], Some(&rest[dot2 + 2..]))
        } else {
            (rest, None)
        };

        // Validate: start and end must be integers or single alpha chars
        fn valid_range_val(s: &str) -> bool {
            s.parse::<i64>().is_ok() || (s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic())
        }
        if !valid_range_val(first) || !valid_range_val(end_val) {
            return None;
        }
        if let Some(step) = step_val
            && step.parse::<i64>().is_err()
        {
            return None;
        }

        // We need to return &'a str slices into the original input.
        // The inner string is a slice of input, so first/end_val/step_val are too.
        // But they were created from `inner` which is a slice. We need to compute
        // the actual input offsets.
        let first_start = inner_start;
        let first_end = inner_start + dot_pos;
        let end_start = inner_start + dot_pos + 2;
        let end_end = if step_val.is_some() {
            end_start + rest.find("..").unwrap()
        } else {
            inner_end
        };
        let step_range = step_val.map(|_| {
            let s = end_end + 2;
            (s, inner_end)
        });

        // Advance past the }
        self.lex.bump_n(inner_end + 1 - start_pos);

        // Check for adjacent brace expansion }{
        if self.lex.peek() == b'{' {
            self.rewind(start_pos);
            return None; // will be caught as unsupported
        }

        let start_slice = self.lex.slice_range(first_start, first_end);
        let end_slice = self.lex.slice_range(end_start, end_end);
        let step_slice = step_range.map(|(s, e)| self.lex.slice_range(s, e));

        Some(Atom::BraceRange {
            start: start_slice,
            end: end_slice,
            step: step_slice,
        })
    }

    // -----------------------------------------------------------------------
    // Redirects
    // -----------------------------------------------------------------------

    /// Try to parse a redirect at the current position.
    /// Returns None if not at a redirect operator.
    fn try_redirect(&mut self) -> Result<Option<Redir<'a>>, ParseError> {
        self.lex.skip_blanks();

        // Read optional fd number
        let start = self.lex.pos();
        let fd_str = self.lex.read_number();
        let fd: Option<u16> = if fd_str.is_empty() {
            None
        } else {
            fd_str.parse().ok()
        };

        let b = self.lex.peek();
        let b2 = self.lex.peek_at(1);

        match (b, b2) {
            (b'<', b'<') if self.lex.peek_at(2) == b'<' => {
                // <<<  here-string
                self.lex.bump_n(3);
                self.lex.skip_blanks();
                let word = self.word()?;
                let _ = fd; // fd always 0 for here-strings
                Ok(Some(Redir::HereString(word)))
            }
            (b'<', b'<') => {
                // << or <<- heredoc
                self.lex.bump_n(2);
                let strip_tabs = self.lex.eat(b'-');
                self.lex.skip_blanks();

                // Parse delimiter — check if quoted
                let (tag, quoted) = self.read_heredoc_delimiter()?;

                // Save position, scan ahead to find body
                let save_pos = self.lex.pos();
                // Skip to end of current line
                while !self.lex.is_eof() && self.lex.peek() != b'\n' {
                    self.lex.bump();
                }
                if self.lex.peek() == b'\n' {
                    self.lex.bump();
                }
                // Read lines until delimiter
                let body_start = self.lex.pos();
                let mut body_end = body_start;
                let mut found = false;
                while !self.lex.is_eof() {
                    let line_start = self.lex.pos();
                    while !self.lex.is_eof() && self.lex.peek() != b'\n' {
                        self.lex.bump();
                    }
                    let line = self.lex.slice(line_start);
                    let trimmed = if strip_tabs {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if trimmed == tag {
                        body_end = line_start;
                        if self.lex.peek() == b'\n' {
                            self.lex.bump();
                        }
                        found = true;
                        break;
                    }
                    if self.lex.peek() == b'\n' {
                        self.lex.bump();
                    }
                }
                if !found {
                    return Err(self.lex.err("unterminated heredoc"));
                }
                let body = self.lex.slice_range(body_start, body_end);
                let after_heredoc = self.lex.pos();
                self.lex.set_pos(save_pos);
                self.heredoc_resume = Some(after_heredoc);

                let heredoc_body = if quoted {
                    HeredocBody::Literal(body)
                } else {
                    // Parse body for variable/command expansions
                    let atoms = Parser::new(body).parse_heredoc_body()?;
                    HeredocBody::Interpolated(atoms)
                };
                let _ = (fd, strip_tabs); // fd always 0, tab-stripping not translated
                Ok(Some(Redir::Heredoc(heredoc_body)))
            }
            (b'<', b'>') => {
                // <>
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::ReadWrite(fd, word)))
            }
            (b'<', b'&') => {
                // <&
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::DupRead(fd, word)))
            }
            (b'<', b'(') if fd.is_none() => {
                // <( — process substitution, not a redirect
                self.rewind(start);
                Ok(None)
            }
            (b'<', _) => {
                // <
                self.lex.bump();
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::Read(fd, word)))
            }
            (b'>', b'>') if self.lex.peek_at(2) == b'|' => {
                // Unusual, treat as >>|
                self.rewind(start);
                Ok(None)
            }
            (b'>', b'>') => {
                // >>
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::Append(fd, word)))
            }
            (b'>', b'|') => {
                // >|
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::Clobber(fd, word)))
            }
            (b'>', b'&') => {
                // >&
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::DupWrite(fd, word)))
            }
            (b'>', b'(') if fd.is_none() => {
                // >( — process substitution, not a redirect
                self.rewind(start);
                Ok(None)
            }
            (b'>', _) => {
                // >
                self.lex.bump();
                self.lex.skip_blanks();
                let word = self.word()?;
                Ok(Some(Redir::Write(fd, word)))
            }
            (b'&', b'>') if fd.is_none() => {
                // &> or &>>
                self.lex.bump_n(2);
                if self.lex.eat(b'>') {
                    // &>>
                    self.lex.skip_blanks();
                    let word = self.word()?;
                    Ok(Some(Redir::AppendAll(word)))
                } else {
                    // &>
                    self.lex.skip_blanks();
                    let word = self.word()?;
                    Ok(Some(Redir::WriteAll(word)))
                }
            }
            _ => {
                // Not a redirect — rewind any consumed fd digits
                self.rewind(start);
                Ok(None)
            }
        }
    }

    /// Collect any trailing redirects after a compound command.
    fn collect_redirects(&mut self) -> Result<Vec<Redir<'a>>, ParseError> {
        let mut redirects = Vec::new();
        loop {
            self.lex.skip_blanks();
            let Some(redir) = self.try_redirect()? else {
                break;
            };
            redirects.push(redir);
        }
        Ok(redirects)
    }

    // -----------------------------------------------------------------------
    // Arithmetic (Pratt parser)
    // -----------------------------------------------------------------------

    /// Parse an arithmetic expression with minimum precedence `min_prec`.
    pub fn arith(&mut self, min_prec: u8) -> Result<Arith<'a>, ParseError> {
        let mut left = self.arith_atom()?;

        loop {
            self.lex.skip_blanks();
            if let Some((prec, op_len, constructor)) = self.arith_infix_op() {
                if prec < min_prec {
                    break;
                }
                self.lex.bump_n(op_len);
                let op_end = self.lex.pos();
                self.lex.skip_blanks();

                // Ternary special case
                if op_len == 1 && self.lex.slice_range(op_end - 1, op_end) == "?" {
                    let then_val = self.arith(0)?;
                    self.lex.skip_blanks();
                    if !self.lex.eat(b':') {
                        return Err(self.lex.err("expected ':' in ternary"));
                    }
                    self.lex.skip_blanks();
                    let else_val = self.arith(0)?;
                    left = Arith::Ternary(Box::new(left), Box::new(then_val), Box::new(else_val));
                    continue;
                }

                // Assignment special case
                if op_len == 1 && self.lex.slice_range(op_end - 1, op_end) == "=" {
                    if let Arith::Var(name) = left {
                        let right = self.arith(prec)?;
                        left = Arith::Assign(name, Box::new(right));
                        continue;
                    }
                    return Err(self.lex.err("expected variable for assignment"));
                }

                let right = self.arith(prec + 1)?;
                left = constructor(Box::new(left), Box::new(right));
            } else {
                // Compound assignment: +=, -=, *=, /=, %=
                let b1 = self.lex.peek();
                let b2 = self.lex.peek_at(1);
                if b2 == b'='
                    && matches!(b1, b'+' | b'-' | b'*' | b'/' | b'%')
                    && let Arith::Var(name) = &left
                {
                    let name = *name;
                    let make_op: fn(Box<Arith<'a>>, Box<Arith<'a>>) -> Arith<'a> = match b1 {
                        b'+' => Arith::Add,
                        b'-' => Arith::Sub,
                        b'*' => Arith::Mul,
                        b'/' => Arith::Div,
                        _ => Arith::Rem,
                    };
                    self.lex.bump_n(2);
                    self.lex.skip_blanks();
                    let right = self.arith(0)?;
                    left = Arith::Assign(
                        name,
                        Box::new(make_op(Box::new(Arith::Var(name)), Box::new(right))),
                    );
                    continue;
                }
                break;
            }
        }

        Ok(left)
    }

    /// Parse an arithmetic atom (number, variable, prefix op, grouping).
    fn arith_atom(&mut self) -> Result<Arith<'a>, ParseError> {
        self.lex.skip_blanks();

        match self.lex.peek() {
            b'(' => {
                self.lex.bump();
                let expr = self.arith(0)?;
                self.lex.skip_blanks();
                if !self.lex.eat(b')') {
                    return Err(self.lex.err("expected ')' in arithmetic"));
                }
                Ok(expr)
            }
            b'+' if self.lex.peek_at(1) == b'+' => {
                // ++var
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let name = self.lex.read_name();
                if name.is_empty() {
                    return Err(self.lex.err("expected variable after '++'"));
                }
                Ok(Arith::PreInc(name))
            }
            b'-' if self.lex.peek_at(1) == b'-' => {
                // --var
                self.lex.bump_n(2);
                self.lex.skip_blanks();
                let name = self.lex.read_name();
                if name.is_empty() {
                    return Err(self.lex.err("expected variable after '--'"));
                }
                Ok(Arith::PreDec(name))
            }
            b'+' => {
                self.lex.bump();
                let e = self.arith_atom()?;
                Ok(Arith::Pos(Box::new(e)))
            }
            b'-' => {
                self.lex.bump();
                let e = self.arith_atom()?;
                Ok(Arith::Neg(Box::new(e)))
            }
            b'!' => {
                self.lex.bump();
                let e = self.arith_atom()?;
                Ok(Arith::LogNot(Box::new(e)))
            }
            b'~' => {
                self.lex.bump();
                let e = self.arith_atom()?;
                Ok(Arith::BitNot(Box::new(e)))
            }
            b'$' => {
                self.lex.bump();
                // Handle $((expr)) as nested arithmetic substitution
                if self.lex.peek() == b'(' && self.lex.peek_at(1) == b'(' {
                    self.lex.bump_n(2);
                    let expr = self.arith(0)?;
                    self.lex.skip_blanks();
                    if !self.lex.eat(b')') || !self.lex.eat(b')') {
                        return Err(self.lex.err("expected '))' in nested arithmetic"));
                    }
                    return Ok(expr);
                }
                // Handle $(cmd) as command substitution in arithmetic — unsupported
                if self.lex.peek() == b'(' {
                    return Err(self
                        .lex
                        .err("unsupported: command substitution in arithmetic"));
                }
                let name = self.lex.read_name();
                if !name.is_empty() {
                    Ok(self.check_postfix(name))
                } else if self.lex.peek().is_ascii_digit() {
                    // Positional parameters: $1, $2, etc.
                    let start = self.lex.pos();
                    while self.lex.peek().is_ascii_digit() {
                        self.lex.bump();
                    }
                    Ok(Arith::Var(self.lex.slice(start)))
                } else {
                    Err(self.lex.err("expected variable after '$' in arithmetic"))
                }
            }
            b'0'..=b'9' => {
                let start = self.lex.pos();
                // Handle hex (0x/0X), octal (0), binary (0b/0B) prefixes
                if self.lex.peek() == b'0' {
                    self.lex.bump();
                    match self.lex.peek() {
                        b'x' | b'X' => {
                            self.lex.bump();
                            while self.lex.peek().is_ascii_hexdigit() {
                                self.lex.bump();
                            }
                            let s = self.lex.slice(start);
                            let n = i64::from_str_radix(&s[2..], 16).unwrap_or(0);
                            return Ok(Arith::Lit(n));
                        }
                        b'b' | b'B' => {
                            self.lex.bump();
                            while matches!(self.lex.peek(), b'0' | b'1') {
                                self.lex.bump();
                            }
                            let s = self.lex.slice(start);
                            let n = i64::from_str_radix(&s[2..], 2).unwrap_or(0);
                            return Ok(Arith::Lit(n));
                        }
                        _ => {} // fall through to read remaining digits (octal or decimal 0)
                    }
                }
                while self.lex.peek().is_ascii_digit() {
                    self.lex.bump();
                }
                let num_str = self.lex.slice(start);
                let n: i64 = if num_str.starts_with('0') && num_str.len() > 1 {
                    i64::from_str_radix(num_str, 8).unwrap_or(0) // octal
                } else {
                    num_str.parse().unwrap_or(0)
                };
                Ok(Arith::Lit(n))
            }
            _ => {
                let name = self.lex.read_name();
                if name.is_empty() {
                    Err(self.lex.err("expected arithmetic expression"))
                } else {
                    Ok(self.check_postfix(name))
                }
            }
        }
    }

    /// Check for postfix ++ or -- after a variable name.
    #[inline]
    fn check_postfix(&mut self, name: &'a str) -> Arith<'a> {
        if self.lex.peek() == b'+' && self.lex.peek_at(1) == b'+' {
            self.lex.bump_n(2);
            Arith::PostInc(name)
        } else if self.lex.peek() == b'-' && self.lex.peek_at(1) == b'-' {
            self.lex.bump_n(2);
            Arith::PostDec(name)
        } else {
            Arith::Var(name)
        }
    }

    /// Return the precedence, operator length, and constructor for a binary
    /// arithmetic infix operator at the current position.
    #[allow(clippy::type_complexity)]
    fn arith_infix_op(
        &self,
    ) -> Option<(u8, usize, fn(Box<Arith<'a>>, Box<Arith<'a>>) -> Arith<'a>)> {
        let b1 = self.lex.peek();
        let b2 = self.lex.peek_at(1);
        let b3 = self.lex.peek_at(2);

        // 3-char ops
        match (b1, b2, b3) {
            (b'<', b'<', b'=') | (b'>', b'>', b'=') => return None, // compound assignment, bail
            _ => {}
        }

        // 2-char ops (check before 1-char)
        match (b1, b2) {
            (b'|', b'|') => return Some((1, 2, |l, r| Arith::LogOr(l, r))),
            (b'&', b'&') => return Some((2, 2, |l, r| Arith::LogAnd(l, r))),
            (b'=', b'=') => return Some((7, 2, |l, r| Arith::Eq(l, r))),
            (b'!', b'=') => return Some((7, 2, |l, r| Arith::Ne(l, r))),
            (b'<', b'=') => return Some((8, 2, |l, r| Arith::Le(l, r))),
            (b'>', b'=') => return Some((8, 2, |l, r| Arith::Ge(l, r))),
            (b'<', b'<') => return Some((9, 2, |l, r| Arith::Shl(l, r))),
            (b'>', b'>') => return Some((9, 2, |l, r| Arith::Shr(l, r))),
            (b'*', b'*') => return Some((13, 2, |l, r| Arith::Pow(l, r))),
            (b'+' | b'-' | b'*' | b'/' | b'%', b'=') => {
                return None; // compound assignment, bail
            }
            _ => {}
        }

        // 1-char ops
        match b1 {
            b'|' => Some((3, 1, |l, r| Arith::BitOr(l, r))),
            b'^' => Some((4, 1, |l, r| Arith::BitXor(l, r))),
            b'&' => Some((5, 1, |l, r| Arith::BitAnd(l, r))),
            b'<' => Some((8, 1, |l, r| Arith::Lt(l, r))),
            b'>' => Some((8, 1, |l, r| Arith::Gt(l, r))),
            b'+' if b2 != b'+' => Some((10, 1, |l, r| Arith::Add(l, r))),
            b'-' if b2 != b'-' => Some((10, 1, |l, r| Arith::Sub(l, r))),
            b'*' if b2 != b'*' => Some((11, 1, |l, r| Arith::Mul(l, r))),
            b'/' => Some((11, 1, |l, r| Arith::Div(l, r))),
            b'%' => Some((11, 1, |l, r| Arith::Rem(l, r))),
            b'?' => Some((0, 1, |l, _| *l)), // placeholder — ternary handled in arith()
            b'=' if b2 != b'=' => Some((0, 1, |l, _| *l)), // placeholder — assignment handled in arith()
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    #[inline]
    fn rewind(&mut self, pos: usize) {
        self.lex.set_pos(pos);
    }

    fn expect(&mut self, kw: &[u8], msg: &'static str) -> Result<(), ParseError> {
        self.lex.skip_blanks();
        if self.lex.eat_str(kw) {
            Ok(())
        } else {
            Err(self.lex.err(msg))
        }
    }

    #[inline]
    fn eat_separator(&mut self) {
        self.lex.skip_blanks();
        if self.lex.peek() == b';' || self.lex.peek() == b'\n' {
            self.lex.bump();
        }
    }

    fn skip_separators(&mut self) {
        loop {
            self.lex.skip_blanks();
            match self.lex.peek() {
                b';' | b'\n' => self.lex.bump(),
                b'#' => self.lex.skip_comment(),
                _ => break,
            }
        }
    }

    /// Read a heredoc delimiter. Returns `(tag, quoted)`.
    /// Quoted delimiters (`'EOF'`, `"EOF"`) suppress variable expansion.
    fn read_heredoc_delimiter(&mut self) -> Result<(&'a str, bool), ParseError> {
        match self.lex.peek() {
            b'\'' => {
                self.lex.bump();
                let tag = self.lex.scan_squote()?;
                Ok((tag, true))
            }
            b'"' => {
                self.lex.bump();
                let start = self.lex.pos();
                while !self.lex.is_eof() && self.lex.peek() != b'"' {
                    if self.lex.peek() == b'\\' {
                        self.lex.bump();
                    }
                    self.lex.bump();
                }
                let tag = self.lex.slice(start);
                if !self.lex.eat(b'"') {
                    return Err(self.lex.err("unterminated heredoc delimiter"));
                }
                Ok((tag, true))
            }
            _ => {
                let start = self.lex.pos();
                while !self.lex.is_eof() && !is_meta(self.lex.peek()) {
                    self.lex.bump();
                }
                let tag = self.lex.slice(start);
                if tag.is_empty() {
                    return Err(self.lex.err("expected heredoc delimiter"));
                }
                Ok((tag, false))
            }
        }
    }

    #[inline]
    fn at_terminator(&self) -> bool {
        let b = self.lex.peek();
        b == 0
            || b == b'\n'
            || b == b';'
            || b == b')'
            || b == b'}'
            || b == b'|'
            || (b == b'&' && self.lex.peek_at(1) != b'>')
    }

    /// Check if we're at a function definition: `NAME ()` or `function NAME`.
    fn at_func_def(&self) -> bool {
        if self.lex.at_keyword(b"function") {
            return true;
        }
        // Check for NAME() pattern
        let src = self.lex.remaining().as_bytes();
        if src.is_empty() || !(src[0].is_ascii_alphabetic() || src[0] == b'_') {
            return false;
        }
        let mut j = 1;
        while j < src.len() && (src[j].is_ascii_alphanumeric() || src[j] == b'_') {
            j += 1;
        }
        while j < src.len() && (src[j] == b' ' || src[j] == b'\t') {
            j += 1;
        }
        j < src.len() && src[j] == b'('
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<Cmd<'_>> {
        Parser::new(input).parse().unwrap()
    }

    fn parse_err(input: &str) -> ParseError {
        Parser::new(input).parse().unwrap_err()
    }

    #[test]
    fn simple_command() {
        let cmds = parse("echo hello world");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn pipeline() {
        let cmds = parse("cat file | grep foo | wc -l");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn and_or_chain() {
        let cmds = parse("cmd1 && cmd2 || cmd3");
        assert_eq!(cmds.len(), 1);
        if let Cmd::List(ref list) = cmds[0] {
            assert_eq!(list.rest.len(), 2);
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn background_job() {
        let cmds = parse("sleep 10 &");
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Cmd::Job(_)));
    }

    #[test]
    fn semicolon_separated() {
        let cmds = parse("echo a; echo b; echo c");
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn assignment() {
        let cmds = parse("FOO=bar");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn export_assignment() {
        let cmds = parse("export PATH=/usr/bin:$PATH");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn for_loop() {
        let cmds = parse("for i in a b c; do echo $i; done");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn while_loop() {
        let cmds = parse("while true; do echo loop; done");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn if_then_fi() {
        let cmds = parse("if test -f foo; then echo yes; fi");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn if_else() {
        let cmds = parse("if test -f foo; then echo yes; else echo no; fi");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn case_statement() {
        let cmds = parse("case $1 in foo) echo foo;; bar) echo bar;; esac");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn brace_group() {
        let cmds = parse("{ echo a; echo b; }");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn command_substitution() {
        let cmds = parse("echo $(whoami)");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn arithmetic_substitution() {
        let cmds = parse("echo $((2 + 3))");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn parameter_expansion_default() {
        let cmds = parse("echo ${HOME:-/tmp}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn single_quoted_word() {
        let cmds = parse("echo 'hello world'");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn double_quoted_word() {
        let cmds = parse("echo \"hello $USER\"");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn redirect_output() {
        let cmds = parse("echo hello >file.txt");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn redirect_stderr() {
        let cmds = parse("cmd 2>/dev/null");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn redirect_dup_write() {
        let cmds = parse("cmd 2>&1");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn here_string() {
        let cmds = parse("cat <<< 'hello'");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn write_all_redirect() {
        let cmds = parse("cmd &>file");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn negated_pipeline() {
        let cmds = parse("! grep -q pattern file");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn special_params() {
        let cmds = parse("echo $? $@ $# $$ $!");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn positional_param() {
        let cmds = parse("echo $1 $2");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn brace_range() {
        let cmds = parse("echo {1..5}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn brace_range_alpha() {
        let cmds = parse("echo {a..z}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn brace_range_with_step() {
        let cmds = parse("echo {1..10..2}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn double_bracket() {
        let cmds = parse("[[ -f /etc/hosts ]]");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn double_bracket_with_and() {
        let cmds = parse("[[ -f a && -f b ]]");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn standalone_arith() {
        let cmds = parse("(( i++ ))");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn standalone_arith_assign() {
        let cmds = parse("(( x = 5 + 3 ))");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn ansi_c_quoting() {
        let cmds = parse("echo $'hello\\nworld'");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn ansi_c_quoting_escaped_squote() {
        let cmds = parse("echo $'it\\'s'");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn env_prefix_command() {
        let cmds = parse("FOO=bar command");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn nested_command_substitution() {
        let cmds = parse("echo $(basename $(pwd))");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn glob_characters() {
        let cmds = parse("ls *.txt");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn escaped_character() {
        let cmds = parse("echo hello\\ world");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn param_expansion_trim_suffix() {
        let cmds = parse("echo ${file%.*}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn param_expansion_trim_prefix() {
        let cmds = parse("echo ${file##*/}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn comment() {
        let cmds = parse("echo hello # this is a comment\necho world");
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn function_def() {
        let cmds = parse("foo() { echo hello; }");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn arithmetic_complex() {
        let cmds = parse("echo $((5 * (3 + 2)))");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn param_expansion_len() {
        let cmds = parse("echo ${#HOME}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn append_redirect() {
        let cmds = parse("echo hello >>file.txt");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn case_modification_upper() {
        let cmds = parse("echo ${var^^}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn case_modification_lower() {
        let cmds = parse("echo ${var,,}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn replace_first() {
        let cmds = parse("echo ${var/foo/bar}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn replace_all() {
        let cmds = parse("echo ${var//foo/bar}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn replace_prefix() {
        let cmds = parse("echo ${var/#foo/bar}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn replace_suffix() {
        let cmds = parse("echo ${var/%foo/bar}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn replace_delete() {
        let cmds = parse("echo ${var/foo}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn substring_offset() {
        let cmds = parse("echo ${var:2}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn substring_offset_length() {
        let cmds = parse("echo ${var:2:5}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn process_substitution() {
        let cmds = parse("diff <(sort a) <(sort b)");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn process_substitution_out_error() {
        let err = parse_err("tee >(grep foo)");
        assert!(err.msg.contains("output process substitution"));
    }

    #[test]
    fn c_style_for() {
        let cmds = parse("for (( i=0; i<10; i++ )); do echo $i; done");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn heredoc_quoted() {
        let cmds = parse("cat <<'EOF'\nhello world\nEOF");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn heredoc_double_quoted() {
        let cmds = parse("cat <<\"EOF\"\nhello world\nEOF");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn heredoc_unquoted() {
        let cmds = parse("cat <<EOF\nhello $NAME\nEOF");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn case_fallthrough_error() {
        let err = parse_err("case $x in a) echo a;& b) echo b;; esac");
        assert!(err.msg.contains("fallthrough"));
    }

    #[test]
    fn case_continue_error() {
        let err = parse_err("case $x in a) echo a;;& b) echo b;; esac");
        assert!(err.msg.contains(";;&"));
    }

    #[test]
    fn prefix_list_star() {
        let cmds = parse("echo ${!BASH_*}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn prefix_list_at() {
        let cmds = parse("echo ${!MY@}");
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn select_error() {
        let err = parse_err("select opt in a b c; do echo $opt; done");
        assert!(err.msg.contains("select"));
    }
}
