use std::fmt;

use conch_parser::ast::*;
use conch_parser::lexer::Lexer;
use conch_parser::parse::DefaultParser;

// ---------------------------------------------------------------------------
// Type aliases for the deeply-nested conch-parser generics
// ---------------------------------------------------------------------------
type TLCmd = TopLevelCommand<String>;
type TLWord = TopLevelWord<String>;
type Redir = Redirect<TLWord>;
type AOList = AndOrList<ListableCommand<DefaultPipeableCommand>>;
type ListCmd = ListableCommand<DefaultPipeableCommand>;
type PipeCmd = DefaultPipeableCommand;
type SimpleCmd = SimpleCommand<String, TLWord, Redir>;
type CompCmd = CompoundCommand<CompoundCommandKind<String, TLWord, TLCmd>, Redir>;
type CmdKind = CompoundCommandKind<String, TLWord, TLCmd>;
type Wd = Word<String, Sw>;
type Sw = SimpleWord<String, Parameter<String>, Box<ParamSubst>>;
type ParamSubst = ParameterSubstitution<Parameter<String>, TLWord, TLCmd, Arithmetic<String>>;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------
#[derive(Debug)]
pub enum TranslateError {
    ParseError(String),
    Unsupported(String),
}

impl fmt::Display for TranslateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranslateError::ParseError(msg) => write!(f, "parse error: {}", msg),
            TranslateError::Unsupported(msg) => write!(f, "unsupported: {}", msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Translate a bash command string to fish shell syntax.
pub fn translate_bash_to_fish(input: &str) -> Result<String, TranslateError> {
    // Bail early on constructs we can't translate correctly
    pre_check_bail(input)?;

    // Pre-process: rewrite bash-isms that conch-parser can't handle
    let input = preprocess(input);

    let lex = Lexer::new(input.chars());
    let parser = DefaultParser::new(lex);
    let mut out = String::new();

    for (i, result) in parser.into_iter().enumerate() {
        let cmd: TLCmd =
            result.map_err(|e| TranslateError::ParseError(format!("{:?}", e)))?;
        if i > 0 {
            out.push('\n');
        }
        emit_top_level(&cmd, &mut out)?;
    }

    Ok(out)
}

/// Bail early on constructs that can't be translated to fish.
/// Returns Err(Unsupported) to trigger bash-exec fallback.
fn pre_check_bail(input: &str) -> Result<(), TranslateError> {
    let chars: Vec<char> = input.chars().collect();
    let mut in_sq = false;
    let mut in_dq = false;
    let mut i = 0;

    while i < chars.len() {
        // Track quoting — but $' triggers bail before toggling
        if chars[i] == '\'' && !in_dq {
            in_sq = !in_sq;
            i += 1;
            continue;
        }
        if chars[i] == '"' && !in_sq {
            in_dq = !in_dq;
            i += 1;
            continue;
        }
        if in_sq || in_dq {
            i += 1;
            continue;
        }

        // ANSI-C quoting $'...' — fish doesn't support this
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '\'' {
            return Err(TranslateError::Unsupported(
                "ANSI-C quoting $'...'".into(),
            ));
        }

        // Standalone (( )) arithmetic — conch-parser can't handle these
        // (misparses as nested subshells). Only bail if not $(( )).
        if chars[i] == '(' && i + 1 < chars.len() && chars[i + 1] == '(' {
            if i == 0 || chars[i - 1] != '$' {
                return Err(TranslateError::Unsupported(
                    "standalone (( )) arithmetic".into(),
                ));
            }
        }

        // Adjacent brace expansions like {a,b}{1,2} — fish expands in different order
        if chars[i] == '}' && i + 1 < chars.len() && chars[i + 1] == '{' {
            return Err(TranslateError::Unsupported(
                "adjacent brace expansions (fish expansion order differs)".into(),
            ));
        }

        i += 1;
    }
    Ok(())
}

/// Pre-process bash input to rewrite constructs that conch-parser cannot parse.
///
/// Currently handles:
///   `cmd <<< "string"` → `echo "string" | cmd`
///   `cmd <<< $var`     → `echo $var | cmd`
///   `cmd <<< word`     → `echo word | cmd`
///   `{1..5}` → `(seq 1 5)`, `{1..10..2}` → `(seq 1 2 10)`
///   `&>file` → `>file 2>&1`
fn preprocess(input: &str) -> String {
    let mut result = input.to_string();

    // Rewrite here-strings: `cmd args <<< value` → `echo value | cmd args`
    // Handle quoted values, variables, and bare words
    while let Some(pos) = result.find("<<<") {
        // Find the command that owns the <<<, not previous statements.
        // Look for the last unquoted ; or && or || before <<<.
        let before = &result[..pos];
        let sep_end = find_last_separator(before);

        let prefix = &result[..sep_end];
        let cmd_part = result[sep_end..pos].trim().to_string();

        // Find the value part (everything after <<<, up to ; or && or || or end)
        let after = &result[pos + 3..];
        let after_trimmed = after.trim_start();

        // Extract the here-string value — could be quoted or unquoted
        let (value, rest) = extract_herestring_value(after_trimmed);

        if prefix.is_empty() {
            result = format!("echo {} | {}{}", value, cmd_part, rest);
        } else {
            result = format!("{} echo {} | {}{}", prefix, value, cmd_part, rest);
        }
    }

    // Rewrite brace range expansion: {start..end} → $(seq start end)
    // Also handles step: {start..end..step} → $(seq start step end)
    // Uses $(seq) so conch-parser sees a valid command substitution
    result = rewrite_brace_ranges(&result);

    // Rewrite [[ cond1 && cond2 ]] → [[ cond1 ]] && [[ cond2 ]]
    // conch-parser doesn't understand && inside [[ ]], so we split into
    // separate [[ ]] commands connected by shell && / ||
    result = rewrite_double_bracket_logic(&result);

    // Rewrite &>file → >file 2>&1 (conch-parser treats & as background)
    result = rewrite_ampersand_redirect(&result);

    result
}

/// Rewrite `&>file` → `>file 2>&1` and `&>>file` → `>>file 2>&1`.
/// conch-parser treats `&` as the background operator, so `cmd &>file` gets
/// misparsed as `cmd &` + `>file`.
fn rewrite_ampersand_redirect(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_sq = false;
    let mut in_dq = false;

    while i < chars.len() {
        if chars[i] == '\'' && !in_dq {
            in_sq = !in_sq;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if chars[i] == '"' && !in_sq {
            in_dq = !in_dq;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if in_sq || in_dq {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Match &>> or &> — only when preceded by whitespace/SOL (not a digit,
        // which would make it part of fd>&N syntax)
        if chars[i] == '&'
            && i + 1 < chars.len()
            && chars[i + 1] == '>'
            && (i == 0 || !chars[i - 1].is_ascii_digit())
        {
            let is_append = i + 2 < chars.len() && chars[i + 2] == '>';
            let skip = if is_append { 3 } else { 2 };

            // Find the redirect target (skip whitespace)
            let mut j = i + skip;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let target_start = j;
            while j < chars.len()
                && !chars[j].is_whitespace()
                && chars[j] != ';'
                && chars[j] != '&'
                && chars[j] != '|'
            {
                j += 1;
            }
            let target: String = chars[target_start..j].iter().collect();

            if is_append {
                result.push_str(&format!(">>{} 2>&1", target));
            } else {
                result.push_str(&format!(">{} 2>&1", target));
            }
            i = j;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Rewrite bash brace range expansions like {1..5} to fish (seq 1 5).
fn rewrite_brace_ranges(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < chars.len() {
        // Track quoting state
        if chars[i] == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if chars[i] == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '{' {
            // Try to match {start..end} or {start..end..step}
            if let Some((replacement, consumed)) = try_parse_brace_range(&chars[i..]) {
                result.push_str(&replacement);
                i += consumed;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Try to parse a brace range at the current position.
/// Returns (replacement_string, chars_consumed) or None.
fn try_parse_brace_range(chars: &[char]) -> Option<(String, usize)> {
    if chars.is_empty() || chars[0] != '{' {
        return None;
    }

    // Find the closing brace
    let close = chars.iter().position(|&c| c == '}')?;
    let inner: String = chars[1..close].iter().collect();

    // Split on ".." — could be "start..end" or "start..end..step"
    let parts: Vec<&str> = inner.split("..").collect();
    match parts.len() {
        2 => {
            let start = parts[0];
            let end = parts[1];
            if !is_brace_range_value(start) || !is_brace_range_value(end) {
                return None;
            }

            // Alpha range: {a..e} → expand inline to "a b c d e"
            let sc = start.chars().next()?;
            let ec = end.chars().next()?;
            if sc.is_ascii_alphabetic() && ec.is_ascii_alphabetic() {
                return Some((expand_alpha_range(sc, ec), close + 1));
            }

            // Numeric range: detect reverse and use seq with step -1
            if let (Ok(s), Ok(e)) = (start.parse::<i64>(), end.parse::<i64>()) {
                if s > e {
                    Some((format!("$(seq {} -1 {})", s, e), close + 1))
                } else {
                    Some((format!("$(seq {} {})", s, e), close + 1))
                }
            } else {
                Some((format!("$(seq {} {})", start, end), close + 1))
            }
        }
        3 => {
            let start = parts[0];
            let end = parts[1];
            let step = parts[2];
            if is_brace_range_value(start) && is_brace_range_value(end)
                && step.parse::<i64>().is_ok()
            {
                Some((format!("$(seq {} {} {})", start, step, end), close + 1))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_brace_range_value(s: &str) -> bool {
    // Integer (possibly negative)
    if s.parse::<i64>().is_ok() {
        return true;
    }
    // Single ASCII alphabetic character (for alpha ranges like {a..z})
    s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
}

/// Expand an alphabetic range like {a..e} → "a b c d e".
fn expand_alpha_range(start: char, end: char) -> String {
    let mut chars = Vec::new();
    if start <= end {
        for c in start as u8..=end as u8 {
            chars.push((c as char).to_string());
        }
    } else {
        for c in (end as u8..=start as u8).rev() {
            chars.push((c as char).to_string());
        }
    }
    chars.join(" ")
}

/// Rewrite `[[ cond1 && cond2 ]]` → `[[ cond1 ]] && [[ cond2 ]]`
/// because conch-parser treats `&&`/`||` as shell operators, not [[ ]] keywords.
fn rewrite_double_bracket_logic(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_sq = false;
    let mut in_dq = false;

    while i < chars.len() {
        // Track quoting
        if chars[i] == '\'' && !in_dq {
            in_sq = !in_sq;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if chars[i] == '"' && !in_sq {
            in_dq = !in_dq;
            result.push(chars[i]);
            i += 1;
            continue;
        }
        if in_sq || in_dq {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Look for [[ at a word boundary
        if i + 1 < chars.len() && chars[i] == '[' && chars[i + 1] == '[' {
            // Find the matching ]]
            let mut j = i + 2;
            let mut q_sq = false;
            let mut q_dq = false;
            let mut found_close = None;

            while j + 1 < chars.len() {
                if chars[j] == '\'' && !q_dq {
                    q_sq = !q_sq;
                } else if chars[j] == '"' && !q_sq {
                    q_dq = !q_dq;
                } else if !q_sq && !q_dq && chars[j] == ']' && chars[j + 1] == ']' {
                    found_close = Some(j);
                    break;
                }
                j += 1;
            }

            if let Some(close_pos) = found_close {
                let inner: String = chars[i + 2..close_pos].iter().collect();

                // Check if inner contains unquoted && or ||
                if has_unquoted_logic_op(&inner) {
                    // Split and rewrite
                    let parts = split_bracket_logic(&inner);
                    for (k, (part, op)) in parts.iter().enumerate() {
                        if k > 0 {
                            result.push(' ');
                        }
                        result.push_str("[[ ");
                        result.push_str(part.trim());
                        result.push_str(" ]]");
                        if let Some(operator) = op {
                            result.push(' ');
                            result.push_str(operator);
                        }
                    }
                    i = close_pos + 2; // skip past ]]
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn has_unquoted_logic_op(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut in_sq = false;
    let mut in_dq = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' && !in_dq { in_sq = !in_sq; }
        else if chars[i] == '"' && !in_sq { in_dq = !in_dq; }
        else if !in_sq && !in_dq && i + 1 < chars.len() {
            if (chars[i] == '&' && chars[i + 1] == '&')
                || (chars[i] == '|' && chars[i + 1] == '|')
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn split_bracket_logic(s: &str) -> Vec<(String, Option<String>)> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut in_sq = false;
    let mut in_dq = false;

    while i < chars.len() {
        if chars[i] == '\'' && !in_dq {
            in_sq = !in_sq;
            current.push(chars[i]);
        } else if chars[i] == '"' && !in_sq {
            in_dq = !in_dq;
            current.push(chars[i]);
        } else if !in_sq && !in_dq {
            if i + 1 < chars.len() && chars[i] == '&' && chars[i + 1] == '&' {
                parts.push((current.clone(), Some("&&".to_string())));
                current.clear();
                i += 2;
                continue;
            } else if i + 1 < chars.len() && chars[i] == '|' && chars[i + 1] == '|' {
                parts.push((current.clone(), Some("||".to_string())));
                current.clear();
                i += 2;
                continue;
            } else {
                current.push(chars[i]);
            }
        } else {
            current.push(chars[i]);
        }
        i += 1;
    }
    parts.push((current, None));
    parts
}

/// Find the position just past the last unquoted top-level statement separator
/// (; or && or ||) before a given position. Returns 0 if no separator found.
/// Skips separators followed by shell keywords (do, done, then, fi, else, etc.)
/// since those are part of compound command structure, not separate statements.
fn find_last_separator(s: &str) -> usize {
    const KEYWORDS: &[&str] = &[
        "do", "done", "then", "fi", "else", "elif", "esac", "in",
    ];

    let mut last_sep_end = 0;
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' && !in_double {
            in_single = !in_single;
        } else if chars[i] == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            let sep_end = if chars[i] == ';' {
                Some(i + 1)
            } else if chars[i] == '&' && i + 1 < chars.len() && chars[i + 1] == '&' {
                Some(i + 2)
            } else if chars[i] == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
                Some(i + 2)
            } else {
                None
            };

            if let Some(end) = sep_end {
                // Check if the next word after the separator is a shell keyword
                let rest: String = chars[end..].iter().collect();
                let next_word = rest.trim_start().split_whitespace().next().unwrap_or("");
                if !KEYWORDS.contains(&next_word) {
                    last_sep_end = end;
                }
                if end > i + 1 {
                    i = end - 1; // skip the second char of && or ||
                }
            }
        }
        i += 1;
    }
    last_sep_end
}

/// Extract the value portion of a here-string and any remaining command text.
fn extract_herestring_value(s: &str) -> (&str, &str) {
    if s.is_empty() {
        return ("\"\"", "");
    }

    // Quoted string
    if s.starts_with('"') {
        if let Some(end) = s[1..].find('"') {
            let value = &s[..end + 2]; // include both quotes
            let rest = &s[end + 2..];
            return (value, rest);
        }
    }
    if s.starts_with('\'') {
        if let Some(end) = s[1..].find('\'') {
            let value = &s[..end + 2];
            let rest = &s[end + 2..];
            return (value, rest);
        }
    }

    // Unquoted — read until whitespace, ;, &&, ||, or end
    let end = s
        .find(|c: char| c.is_whitespace() || c == ';')
        .unwrap_or(s.len());

    (&s[..end], &s[end..])
}

// ---------------------------------------------------------------------------
// Command-level emitters
// ---------------------------------------------------------------------------

fn emit_top_level(cmd: &TLCmd, out: &mut String) -> Result<(), TranslateError> {
    match &cmd.0 {
        Command::List(list) => emit_and_or_list(list, out),
        Command::Job(list) => {
            emit_and_or_list(list, out)?;
            out.push_str(" &");
            Ok(())
        }
    }
}

fn emit_and_or_list(list: &AOList, out: &mut String) -> Result<(), TranslateError> {
    emit_listable(&list.first, out)?;
    for and_or in &list.rest {
        match and_or {
            AndOr::And(cmd) => {
                out.push_str("; and ");
                emit_listable(cmd, out)?;
            }
            AndOr::Or(cmd) => {
                out.push_str("; or ");
                emit_listable(cmd, out)?;
            }
        }
    }
    Ok(())
}

fn emit_listable(cmd: &ListCmd, out: &mut String) -> Result<(), TranslateError> {
    match cmd {
        ListableCommand::Single(pipeable) => emit_pipeable(pipeable, out),
        ListableCommand::Pipe(negated, cmds) => {
            if *negated {
                out.push_str("not ");
            }
            for (i, c) in cmds.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                emit_pipeable(c, out)?;
            }
            Ok(())
        }
    }
}

fn emit_pipeable(cmd: &PipeCmd, out: &mut String) -> Result<(), TranslateError> {
    match cmd {
        PipeableCommand::Simple(simple) => emit_simple(simple, out),
        PipeableCommand::Compound(compound) => emit_compound(compound, out),
        PipeableCommand::FunctionDef(name, body) => {
            out.push_str("function ");
            out.push_str(name);
            out.push('\n');
            emit_compound_kind(&body.kind, out)?;
            out.push_str("\nend");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Simple command
// ---------------------------------------------------------------------------

fn emit_simple(cmd: &SimpleCmd, out: &mut String) -> Result<(), TranslateError> {
    let mut env_vars: Vec<(&String, &Option<TLWord>)> = Vec::new();
    let mut cmd_words: Vec<&TLWord> = Vec::new();
    let mut redirects: Vec<&Redir> = Vec::new();

    for item in &cmd.redirects_or_env_vars {
        match item {
            RedirectOrEnvVar::EnvVar(name, value) => env_vars.push((name, value)),
            RedirectOrEnvVar::Redirect(r) => redirects.push(r),
        }
    }
    for item in &cmd.redirects_or_cmd_words {
        match item {
            RedirectOrCmdWord::CmdWord(w) => cmd_words.push(w),
            RedirectOrCmdWord::Redirect(r) => redirects.push(r),
        }
    }

    // Standalone variable assignment: VAR=val → set VAR val
    if cmd_words.is_empty() && !env_vars.is_empty() {
        for (i, (name, value)) in env_vars.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str("set ");
            out.push_str(name);
            if let Some(val) = value {
                out.push(' ');
                emit_word(val, out)?;
            }
        }
        return Ok(());
    }

    // Detect special bash builtins by command name
    let cmd_name = cmd_words.first().and_then(|w| word_as_str(w));

    match cmd_name.as_deref() {
        Some("export") => return emit_export(&cmd_words[1..], out),
        Some("unset") => return emit_unset(&cmd_words[1..], out),
        Some("local") => return emit_local(&cmd_words[1..], out),
        Some("declare") | Some("typeset") => return emit_declare(&cmd_words[1..], out),
        Some("readonly") => return emit_readonly(&cmd_words[1..], out),
        Some("[[") => return emit_double_bracket(&cmd_words[1..], &redirects, out),
        Some("let") => {
            return Err(TranslateError::Unsupported("'let' command".into()));
        }
        Some("trap") => {
            return Err(TranslateError::Unsupported("'trap' command".into()));
        }
        Some("read") => {
            return emit_read(&cmd_words, &redirects, out);
        }
        Some("printf") => {
            // Bail on printf formats that fish doesn't support (e.g. %0.s or %.0s)
            for w in &cmd_words[1..] {
                let mut buf = String::new();
                let text = word_as_str(w).unwrap_or_else(|| {
                    let _ = emit_word(w, &mut buf);
                    buf
                });
                if text.contains("%0.s") || text.contains("%.0s") {
                    return Err(TranslateError::Unsupported(
                        "printf %0.s format (fish printf doesn't support this)".into(),
                    ));
                }
            }
        }
        _ => {}
    }

    // Env-var prefix: VAR=val command args → env VAR=val command args
    if !env_vars.is_empty() {
        out.push_str("env ");
        for (name, value) in &env_vars {
            out.push_str(name);
            out.push('=');
            if let Some(val) = value {
                emit_word(val, out)?;
            }
            out.push(' ');
        }
    }

    // Emit command and arguments
    for (i, word) in cmd_words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        emit_word(word, out)?;
    }

    // Emit redirects
    for redir in &redirects {
        out.push(' ');
        emit_redirect(redir, out)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bash builtin translations
// ---------------------------------------------------------------------------

/// `export VAR=val` → `set -gx VAR val`
fn emit_export(args: &[&TLWord], out: &mut String) -> Result<(), TranslateError> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg) {
            if s.starts_with('-') {
                continue;
            }
        }
        if !first {
            out.push('\n');
        }
        first = false;

        // Try to split the word at the `=` sign — handles both simple literals
        // and complex words like PATH="/usr/local/bin:$PATH"
        if let Some((var_name, value_parts)) = split_word_at_equals(arg) {
            out.push_str("set -gx ");
            out.push_str(&var_name);
            if !value_parts.is_empty() {
                out.push(' ');
                out.push_str(&value_parts);
            }
        } else if let Some(s) = word_as_str(arg) {
            // No `=` found — export VAR (mark as exported)
            out.push_str("set -gx ");
            out.push_str(&s);
            out.push_str(" $");
            out.push_str(&s);
        } else {
            out.push_str("set -gx ");
            emit_word(arg, out)?;
        }
    }
    Ok(())
}

/// Split a word at the first `=` sign, returning (var_name, value_as_fish).
/// Handles complex words like `PATH="/usr/local/bin:$PATH"` where the `=`
/// is inside a literal and the value may contain substitutions.
fn split_word_at_equals(word: &TLWord) -> Option<(String, String)> {
    // Render the whole word to a string for the fish output
    let mut full = String::new();
    if emit_word(word, &mut full).is_err() {
        return None;
    }

    // Find the `=` in the rendered output
    let eq_pos = full.find('=')?;
    let var_name = full[..eq_pos].to_string();
    let value = full[eq_pos + 1..].to_string();

    // Strip surrounding quotes from value if present
    let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else {
        value
    };

    Some((var_name, value))
}

/// `unset VAR` → `set -e VAR`
fn emit_unset(args: &[&TLWord], out: &mut String) -> Result<(), TranslateError> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg) {
            if s.starts_with('-') {
                continue;
            }
        }
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str("set -e ");
        emit_word(arg, out)?;
    }
    Ok(())
}

/// `local VAR=val` → `set -l VAR val`
fn emit_local(args: &[&TLWord], out: &mut String) -> Result<(), TranslateError> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg) {
            if s.starts_with('-') {
                continue;
            }
        }
        if !first {
            out.push('\n');
        }
        first = false;

        if let Some(s) = word_as_str(arg) {
            if let Some(eq) = s.find('=') {
                out.push_str("set -l ");
                out.push_str(&s[..eq]);
                out.push(' ');
                out.push_str(&s[eq + 1..]);
            } else {
                out.push_str("set -l ");
                out.push_str(&s);
            }
        } else {
            out.push_str("set -l ");
            emit_word(arg, out)?;
        }
    }
    Ok(())
}

/// `declare [-x] [-g] VAR=val` → `set [-gx] VAR val`
fn emit_declare(args: &[&TLWord], out: &mut String) -> Result<(), TranslateError> {
    let mut scope = "-g";
    let mut remaining = Vec::new();

    for arg in args {
        if let Some(s) = word_as_str(arg) {
            match s.as_str() {
                "-n" => {
                    return Err(TranslateError::Unsupported(
                        "declare -n (nameref)".into(),
                    ));
                }
                "-x" => scope = "-gx",
                "-g" => scope = "-g",
                s if s.starts_with('-') => {}
                _ => remaining.push(*arg),
            }
        } else {
            remaining.push(*arg);
        }
    }

    let mut first = true;
    for arg in &remaining {
        if !first {
            out.push('\n');
        }
        first = false;

        // Use split_word_at_equals for complex words like VAR="val"
        if let Some((var_name, value_parts)) = split_word_at_equals(arg) {
            out.push_str("set ");
            out.push_str(scope);
            out.push(' ');
            out.push_str(&var_name);
            if !value_parts.is_empty() {
                out.push(' ');
                out.push_str(&value_parts);
            }
        } else if let Some(s) = word_as_str(arg) {
            out.push_str("set ");
            out.push_str(scope);
            out.push(' ');
            out.push_str(&s);
        } else {
            out.push_str("set ");
            out.push_str(scope);
            out.push(' ');
            emit_word(arg, out)?;
        }
    }
    Ok(())
}

/// `readonly VAR=val` → `set -g VAR val`
/// `read` — strip bash-specific flags that don't exist in fish.
/// Bash `-r` (raw mode) has no fish equivalent because fish's read never interprets backslashes.
fn emit_read(cmd_words: &[&TLWord], redirects: &[&Redir], out: &mut String) -> Result<(), TranslateError> {
    out.push_str("read");
    for word in &cmd_words[1..] {
        if let Some(s) = word_as_str(word) {
            if s == "-r" {
                continue; // fish read doesn't need -r (raw is default)
            }
        }
        out.push(' ');
        emit_word(word, out)?;
    }
    for redir in redirects {
        out.push(' ');
        emit_redirect(redir, out)?;
    }
    Ok(())
}

fn emit_readonly(args: &[&TLWord], out: &mut String) -> Result<(), TranslateError> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg) {
            if s.starts_with('-') {
                continue;
            }
        }
        if !first {
            out.push('\n');
        }
        first = false;

        if let Some(s) = word_as_str(arg) {
            if let Some(eq) = s.find('=') {
                out.push_str("set -g ");
                out.push_str(&s[..eq]);
                out.push(' ');
                out.push_str(&s[eq + 1..]);
            } else {
                out.push_str("set -g ");
                out.push_str(&s);
                out.push_str(" $");
                out.push_str(&s);
            }
        } else {
            out.push_str("set -g ");
            emit_word(arg, out)?;
        }
    }
    Ok(())
}

/// `[[ cond ]]` → `test cond` or `string match -q pattern subject`
/// When `==` or `!=` is used, emits `string match` for proper pattern matching
/// (bash `[[ x == pattern ]]` does glob-style pattern matching, not string equality).
/// Falls back to `test` for other operators like `-n`, `-f`, `-eq`, etc.
fn emit_double_bracket(
    args: &[&TLWord],
    redirects: &[&Redir],
    out: &mut String,
) -> Result<(), TranslateError> {
    // Collect args (excluding ]])
    // Note: && and || inside [[ ]] are handled by the preprocessor, which splits
    // them into separate [[ ]] commands connected by shell && / ||.
    let filtered: Vec<&TLWord> = args
        .iter()
        .filter(|a| word_as_str(a).as_deref() != Some("]]"))
        .copied()
        .collect();

    // Find =~ operator position for regex matching
    let regex_pos = filtered.iter().position(|a| {
        word_as_str(a).as_deref() == Some("=~")
    });

    // Find == or != operator position for glob pattern matching
    let op_pos = filtered.iter().position(|a| {
        let s = word_as_str(a);
        matches!(s.as_deref(), Some("==") | Some("!="))
    });

    if let Some(pos) = regex_pos {
        // [[ str =~ regex ]] → string match -rq 'regex' -- str
        let lhs = &filtered[..pos];
        let rhs = &filtered[pos + 1..];

        out.push_str("string match -rq ");
        out.push('\'');
        let mut pat_buf = String::new();
        for (i, w) in rhs.iter().enumerate() {
            if i > 0 {
                pat_buf.push(' ');
            }
            emit_word_unquoted(w, &mut pat_buf)?;
        }
        out.push_str(&pat_buf.replace('\'', "'\\''"));
        out.push('\'');
        out.push_str(" -- ");
        for w in lhs {
            emit_word(w, out)?;
        }
    } else if let Some(pos) = op_pos {
        let is_negated = word_as_str(filtered[pos]).as_deref() == Some("!=");
        let lhs = &filtered[..pos];
        let rhs = &filtered[pos + 1..];

        if is_negated {
            out.push_str("not string match -q ");
        } else {
            out.push_str("string match -q ");
        }
        out.push('\'');
        let mut pat_buf = String::new();
        for (i, w) in rhs.iter().enumerate() {
            if i > 0 {
                pat_buf.push(' ');
            }
            // Strip the outer quoting layer: in bash [[ "abc" == "abc" ]],
            // the quotes are parsed as quoting, not literal characters.
            emit_word_unquoted(w, &mut pat_buf)?;
        }
        out.push_str(&pat_buf.replace('\'', "'\\''"));
        out.push('\'');
        out.push_str(" -- ");
        for w in lhs {
            emit_word(w, out)?;
        }
    } else {
        // No == or != or =~ — use test (for -n, -f, -eq, etc.)
        out.push_str("test");
        for arg in &filtered {
            out.push(' ');
            emit_word(arg, out)?;
        }
    }

    for redir in redirects {
        out.push(' ');
        emit_redirect(redir, out)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Compound commands (for/if/while/case/brace/subshell)
// ---------------------------------------------------------------------------

fn emit_compound(cmd: &CompCmd, out: &mut String) -> Result<(), TranslateError> {
    emit_compound_kind(&cmd.kind, out)?;
    for redir in &cmd.io {
        out.push(' ');
        emit_redirect(redir, out)?;
    }
    Ok(())
}

/// If the word is a bare (unquoted) command substitution like $(cmd),
/// return a reference to the commands inside. Returns None for quoted
/// or non-command-substitution words.
fn get_bare_command_subst(word: &TLWord) -> Option<&Vec<TLCmd>> {
    match &word.0 {
        ComplexWord::Single(Word::Simple(SimpleWord::Subst(subst))) => match subst.as_ref() {
            ParameterSubstitution::Command(cmds) => Some(cmds),
            _ => None,
        },
        _ => None,
    }
}

/// Emit a command substitution with `| string split -n ' '` inside the parens,
/// replicating bash's IFS word splitting for for-loop word lists.
fn emit_command_subst_with_split(cmds: &[TLCmd], out: &mut String) -> Result<(), TranslateError> {
    out.push('(');
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        emit_top_level(cmd, out)?;
    }
    out.push_str(" | string split -n ' ')");
    Ok(())
}

fn emit_compound_kind(kind: &CmdKind, out: &mut String) -> Result<(), TranslateError> {
    match kind {
        CompoundCommandKind::For { var, words, body } => {
            // Bail on for-loops with glob patterns — fish handles no-match
            // (empty iteration) and sort order differently than bash
            if let Some(words) = words {
                if words.iter().any(|w| word_has_glob(w)) {
                    return Err(TranslateError::Unsupported(
                        "for loop with glob pattern (fish glob behavior differs)".into(),
                    ));
                }
            }

            out.push_str("for ");
            out.push_str(var);
            out.push_str(" in ");
            if let Some(words) = words {
                for (i, w) in words.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    // Bare $(cmd) in for-loop word lists need word splitting
                    // to match bash behavior (bash splits on IFS whitespace)
                    if let Some(cmds) = get_bare_command_subst(w) {
                        emit_command_subst_with_split(cmds, out)?;
                    } else {
                        emit_word(w, out)?;
                    }
                }
            } else {
                out.push_str("$argv");
            }
            out.push('\n');
            emit_body(body, out)?;
            out.push_str("\nend");
        }

        CompoundCommandKind::While(guard_body) => {
            out.push_str("while ");
            emit_guard(&guard_body.guard, out)?;
            out.push('\n');
            emit_body(&guard_body.body, out)?;
            out.push_str("\nend");
        }

        CompoundCommandKind::Until(guard_body) => {
            out.push_str("while not ");
            emit_guard(&guard_body.guard, out)?;
            out.push('\n');
            emit_body(&guard_body.body, out)?;
            out.push_str("\nend");
        }

        CompoundCommandKind::If {
            conditionals,
            else_branch,
        } => {
            for (i, guard_body) in conditionals.iter().enumerate() {
                if i == 0 {
                    out.push_str("if ");
                } else {
                    out.push_str("\nelse if ");
                }
                emit_guard(&guard_body.guard, out)?;
                out.push('\n');
                emit_body(&guard_body.body, out)?;
            }
            if let Some(else_body) = else_branch {
                out.push_str("\nelse\n");
                emit_body(else_body, out)?;
            }
            out.push_str("\nend");
        }

        CompoundCommandKind::Case { word, arms } => {
            out.push_str("switch ");
            emit_word(word, out)?;
            out.push('\n');
            for arm in arms {
                out.push_str("case ");
                for (i, pattern) in arm.patterns.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    let mut pat_buf = String::new();
                    emit_word(pattern, &mut pat_buf)?;

                    // Fish case patterns: * is a wildcard even when quoted,
                    // but [...] character classes are NOT supported.
                    // - Expand [chars] to space-separated alternatives: [yY] → y Y
                    // - Quote patterns with * or ? to prevent file globbing
                    //   (fish case still treats quoted * as a wildcard)
                    if let Some(expanded) = expand_bracket_pattern(&pat_buf) {
                        out.push_str(&expanded);
                    } else if pat_buf.contains('*') || pat_buf.contains('?') {
                        out.push('\'');
                        out.push_str(&pat_buf.replace('\'', "'\\''"));
                        out.push('\'');
                    } else {
                        out.push_str(&pat_buf);
                    }
                }
                out.push('\n');
                emit_body(&arm.body, out)?;
                out.push('\n');
            }
            out.push_str("end");
        }

        CompoundCommandKind::Brace(cmds) => {
            out.push_str("begin\n");
            emit_body(cmds, out)?;
            out.push_str("\nend");
        }

        CompoundCommandKind::Subshell(_) => {
            return Err(TranslateError::Unsupported(
                "subshell (fish begin/end doesn't provide process isolation)".into(),
            ));
        }
    }
    Ok(())
}

/// Expand a pure bracket pattern [chars] to space-separated alternatives.
/// `[yY]` → `"y" "Y"`, `[abc]` → `"a" "b" "c"`.
/// Returns None if the pattern is not a pure bracket expression or contains ranges.
fn expand_bracket_pattern(pat: &str) -> Option<String> {
    if !pat.starts_with('[') || !pat.ends_with(']') || pat.len() < 3 {
        return None;
    }
    let inner = &pat[1..pat.len() - 1];
    // Don't expand ranges like [a-z] — too many alternatives
    if inner.contains('-') {
        return None;
    }
    // Expand each character as a separate quoted alternative
    let alternatives: Vec<String> = inner
        .chars()
        .map(|c| {
            if c == '\'' {
                "'\\'''".to_string()
            } else {
                format!("'{}'", c)
            }
        })
        .collect();
    Some(alternatives.join(" "))
}

/// Emit a guard (condition) for if/while.
fn emit_guard(guard: &[TLCmd], out: &mut String) -> Result<(), TranslateError> {
    if guard.len() == 1 {
        emit_top_level(&guard[0], out)?;
    } else {
        out.push_str("begin; ");
        for (i, cmd) in guard.iter().enumerate() {
            if i > 0 {
                out.push_str("; ");
            }
            emit_top_level(cmd, out)?;
        }
        out.push_str("; end");
    }
    Ok(())
}

/// Emit a sequence of commands (body of loop/if/function).
fn emit_body(cmds: &[TLCmd], out: &mut String) -> Result<(), TranslateError> {
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        emit_top_level(cmd, out)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Word-level emitters — the heart of bash→fish translation
// ---------------------------------------------------------------------------

fn emit_word(word: &TLWord, out: &mut String) -> Result<(), TranslateError> {
    emit_complex_word(&word.0, out)
}

/// Emit a word with its outer quoting layer stripped.
/// In bash, `[[ "abc" == "abc" ]]` — the quotes are shell quoting, not literal chars.
/// `emit_word` would produce `"abc"` (with quotes), but for patterns we need just `abc`.
fn emit_word_unquoted(word: &TLWord, out: &mut String) -> Result<(), TranslateError> {
    match &word.0 {
        ComplexWord::Single(Word::DoubleQuoted(parts)) => {
            for part in parts {
                emit_simple_word(part, out)?;
            }
            Ok(())
        }
        ComplexWord::Single(Word::SingleQuoted(s)) => {
            out.push_str(s);
            Ok(())
        }
        _ => emit_word(word, out),
    }
}

fn emit_complex_word(word: &ComplexWord<Wd>, out: &mut String) -> Result<(), TranslateError> {
    match word {
        ComplexWord::Single(w) => emit_single_word(w, out),
        ComplexWord::Concat(words) => {
            for w in words {
                emit_single_word(w, out)?;
            }
            Ok(())
        }
    }
}

fn emit_single_word(word: &Wd, out: &mut String) -> Result<(), TranslateError> {
    match word {
        Word::Simple(sw) => emit_simple_word(sw, out),
        Word::DoubleQuoted(parts) => {
            // When a double-quoted string contains command substitutions like
            // $((...)) or $(...), the inner quotes from the translation would
            // close the outer quotes. Fix: close outer quotes before any Subst,
            // emit it standalone, then reopen outer quotes for remaining text.
            let mut in_quotes = true;
            out.push('"');
            for part in parts {
                match part {
                    SimpleWord::Subst(_) => {
                        if in_quotes {
                            out.push('"');
                            in_quotes = false;
                        }
                        emit_simple_word(part, out)?;
                    }
                    _ => {
                        if !in_quotes {
                            out.push('"');
                            in_quotes = true;
                        }
                        emit_simple_word(part, out)?;
                    }
                }
            }
            if in_quotes {
                out.push('"');
            }
            Ok(())
        }
        Word::SingleQuoted(s) => {
            out.push('\'');
            out.push_str(s);
            out.push('\'');
            Ok(())
        }
    }
}

fn emit_simple_word(sw: &Sw, out: &mut String) -> Result<(), TranslateError> {
    match sw {
        SimpleWord::Literal(s) => {
            out.push_str(s);
            Ok(())
        }
        SimpleWord::Escaped(s) => {
            out.push('\\');
            out.push_str(s);
            Ok(())
        }
        SimpleWord::Param(param) => {
            emit_param(param, out);
            Ok(())
        }
        SimpleWord::Subst(subst) => emit_param_subst(subst, out),
        SimpleWord::Star => {
            out.push('*');
            Ok(())
        }
        SimpleWord::Question => {
            out.push('?');
            Ok(())
        }
        SimpleWord::SquareOpen => {
            out.push('[');
            Ok(())
        }
        SimpleWord::SquareClose => {
            out.push(']');
            Ok(())
        }
        SimpleWord::Tilde => {
            out.push('~');
            Ok(())
        }
        SimpleWord::Colon => {
            out.push(':');
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter and substitution emitters
// ---------------------------------------------------------------------------

fn emit_param(param: &Parameter<String>, out: &mut String) {
    match param {
        Parameter::Var(name) => {
            out.push('$');
            out.push_str(name);
        }
        Parameter::Positional(n) => {
            if *n == 0 {
                out.push_str("(status filename)");
            } else {
                out.push_str(&format!("$argv[{}]", n));
            }
        }
        Parameter::At => out.push_str("$argv"),
        Parameter::Star => out.push_str("$argv"),
        Parameter::Pound => out.push_str("(count $argv)"),
        Parameter::Question => out.push_str("$status"),
        Parameter::Dollar => out.push_str("$fish_pid"),
        Parameter::Bang => out.push_str("$last_pid"),
        Parameter::Dash => out.push_str("\"\""),
    }
}

fn emit_param_subst(subst: &ParamSubst, out: &mut String) -> Result<(), TranslateError> {
    match subst {
        // $(command) → (command)
        ParameterSubstitution::Command(cmds) => {
            out.push('(');
            for (i, cmd) in cmds.iter().enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                emit_top_level(cmd, out)?;
            }
            out.push(')');
            Ok(())
        }

        // $(( expr )) → (math "expr") or test-based for comparisons/ternary
        ParameterSubstitution::Arith(Some(arith)) => {
            // Bail on operations fish math doesn't support
            if arith_has_unsupported(arith) {
                return Err(TranslateError::Unsupported(
                    "unsupported arithmetic (bitwise, increment, or assignment)".into(),
                ));
            }
            if arith_needs_test(arith) {
                emit_arith_as_command(arith, out)
            } else {
                out.push_str("(math \"");
                emit_arithmetic(arith, out);
                out.push_str("\")");
                Ok(())
            }
        }
        ParameterSubstitution::Arith(None) => {
            out.push_str("(math 0)");
            Ok(())
        }

        // ${#var} → (string length -- "$var")
        ParameterSubstitution::Len(param) => {
            out.push_str("(string length -- \"");
            emit_param(param, out);
            out.push_str("\")");
            Ok(())
        }

        // ${var:-word} → (set -q var; and echo $var; or echo word)
        ParameterSubstitution::Default(_, param, word) => {
            let var = param_name(param);
            out.push_str("(set -q ");
            out.push_str(&var);
            out.push_str("; and echo $");
            out.push_str(&var);
            out.push_str("; or echo ");
            if let Some(w) = word {
                emit_word(w, out)?;
            }
            out.push(')');
            Ok(())
        }

        // ${var:=word} → (set -q var; or set var word; echo $var)
        ParameterSubstitution::Assign(_, param, word) => {
            let var = param_name(param);
            out.push_str("(set -q ");
            out.push_str(&var);
            out.push_str("; or set ");
            out.push_str(&var);
            out.push(' ');
            if let Some(w) = word {
                emit_word(w, out)?;
            }
            out.push_str("; echo $");
            out.push_str(&var);
            out.push(')');
            Ok(())
        }

        // ${var:?word} → error if unset
        ParameterSubstitution::Error(_, param, word) => {
            let var = param_name(param);
            out.push_str("(set -q ");
            out.push_str(&var);
            out.push_str("; and echo $");
            out.push_str(&var);
            out.push_str("; or begin; echo ");
            if let Some(w) = word {
                emit_word(w, out)?;
            } else {
                out.push_str(&format!("'parameter {} not set'", var));
            }
            out.push_str(" >&2; return 1; end)");
            Ok(())
        }

        // ${var:+word} → word if var is set
        ParameterSubstitution::Alternative(_, param, word) => {
            let var = param_name(param);
            out.push_str("(set -q ");
            out.push_str(&var);
            out.push_str("; and echo ");
            if let Some(w) = word {
                emit_word(w, out)?;
            }
            out.push(')');
            Ok(())
        }

        // ${var%pattern} → (string replace -r 'pattern$' '' -- $var)
        ParameterSubstitution::RemoveSmallestSuffix(param, pattern) => {
            emit_string_op(param, pattern, "suffix", false, out)
        }
        ParameterSubstitution::RemoveLargestSuffix(param, pattern) => {
            emit_string_op(param, pattern, "suffix", true, out)
        }
        ParameterSubstitution::RemoveSmallestPrefix(param, pattern) => {
            emit_string_op(param, pattern, "prefix", false, out)
        }
        ParameterSubstitution::RemoveLargestPrefix(param, pattern) => {
            emit_string_op(param, pattern, "prefix", true, out)
        }
    }
}

/// Emit ${var%pattern} / ${var#pattern} style operations using fish string replace.
fn emit_string_op(
    param: &Parameter<String>,
    pattern: &Option<TLWord>,
    kind: &str,
    greedy: bool,
    out: &mut String,
) -> Result<(), TranslateError> {
    let var = param_name(param);
    out.push_str("(string replace -r '");

    if kind == "prefix" {
        out.push('^');
    }

    if let Some(p) = pattern {
        emit_word_as_pattern(p, out, greedy)?;
    }

    if kind == "suffix" {
        out.push('$');
    }

    out.push_str("' '' -- $");
    out.push_str(&var);
    out.push(')');
    Ok(())
}

/// Emit a word as a regex pattern (basic glob→regex conversion).
fn emit_word_as_pattern(
    word: &TLWord,
    out: &mut String,
    greedy: bool,
) -> Result<(), TranslateError> {
    match &word.0 {
        ComplexWord::Single(w) => emit_pattern_word(w, out, greedy),
        ComplexWord::Concat(words) => {
            for w in words {
                emit_pattern_word(w, out, greedy)?;
            }
            Ok(())
        }
    }
}

fn emit_pattern_word(word: &Wd, out: &mut String, greedy: bool) -> Result<(), TranslateError> {
    match word {
        Word::Simple(sw) => match sw {
            SimpleWord::Literal(s) => {
                for c in s.chars() {
                    match c {
                        '.' | '+' | '(' | ')' | '{' | '}' | '|' | '\\' | '^' | '$' => {
                            out.push('\\');
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                Ok(())
            }
            SimpleWord::Star => {
                if greedy {
                    out.push_str(".*");
                } else {
                    // For non-greedy (shortest match), use a negated character class
                    // based on the preceding literal character. This is needed because
                    // `.*?$` is equivalent to `.*$` — the `$` anchor forces the match
                    // to extend to the end regardless of greediness.
                    // Example: `${var%.*}` → `\.[^.]*$` (matches last dot to end)
                    let prev = out.chars().last();
                    match prev {
                        Some(c) if c.is_alphanumeric() || "._-/".contains(c) => {
                            let escaped = match c {
                                ']' | '\\' | '^' | '-' => format!("\\{}", c),
                                _ => c.to_string(),
                            };
                            out.push_str(&format!("[^{}]*", escaped));
                        }
                        _ => {
                            out.push_str(".*?");
                        }
                    }
                }
                Ok(())
            }
            SimpleWord::Question => {
                out.push('.');
                Ok(())
            }
            _ => emit_simple_word(sw, out),
        },
        _ => emit_single_word(word, out),
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

fn emit_arithmetic(arith: &Arithmetic<String>, out: &mut String) {
    match arith {
        Arithmetic::Var(name) => {
            out.push('$');
            out.push_str(name);
        }
        Arithmetic::Literal(n) => {
            out.push_str(&n.to_string());
        }

        Arithmetic::Add(l, r) => emit_binop(l, " + ", r, out),
        Arithmetic::Sub(l, r) => emit_binop(l, " - ", r, out),
        Arithmetic::Mult(l, r) => emit_binop(l, " * ", r, out),
        Arithmetic::Div(l, r) => emit_binop(l, " / ", r, out),
        Arithmetic::Modulo(l, r) => emit_binop(l, " % ", r, out),
        Arithmetic::Pow(l, r) => emit_binop(l, " ^ ", r, out),
        Arithmetic::Less(l, r) => emit_binop(l, " < ", r, out),
        Arithmetic::LessEq(l, r) => emit_binop(l, " <= ", r, out),
        Arithmetic::Great(l, r) => emit_binop(l, " > ", r, out),
        Arithmetic::GreatEq(l, r) => emit_binop(l, " >= ", r, out),
        Arithmetic::Eq(l, r) => emit_binop(l, " == ", r, out),
        Arithmetic::NotEq(l, r) => emit_binop(l, " != ", r, out),
        Arithmetic::BitwiseAnd(l, r) => emit_binop(l, " & ", r, out),
        Arithmetic::BitwiseOr(l, r) => emit_binop(l, " | ", r, out),
        Arithmetic::BitwiseXor(l, r) => emit_binop(l, " ^ ", r, out),
        Arithmetic::LogicalAnd(l, r) => emit_binop(l, " && ", r, out),
        Arithmetic::LogicalOr(l, r) => emit_binop(l, " || ", r, out),
        Arithmetic::ShiftLeft(l, r) => emit_binop(l, " << ", r, out),
        Arithmetic::ShiftRight(l, r) => emit_binop(l, " >> ", r, out),

        Arithmetic::UnaryPlus(e) => {
            out.push('+');
            emit_arithmetic(e, out);
        }
        Arithmetic::UnaryMinus(e) => {
            out.push('-');
            emit_arithmetic(e, out);
        }
        Arithmetic::LogicalNot(e) => {
            out.push('!');
            emit_arithmetic(e, out);
        }
        Arithmetic::BitwiseNot(e) => {
            out.push('~');
            emit_arithmetic(e, out);
        }

        Arithmetic::PostIncr(var) | Arithmetic::PreIncr(var) => {
            out.push_str("($");
            out.push_str(var);
            out.push_str(" + 1)");
        }
        Arithmetic::PostDecr(var) | Arithmetic::PreDecr(var) => {
            out.push_str("($");
            out.push_str(var);
            out.push_str(" - 1)");
        }

        Arithmetic::Ternary(cond, then_val, else_val) => {
            // This is a fallback for ternary nested inside math — top-level ternary
            // goes through emit_arith_as_command instead
            out.push('(');
            emit_arithmetic(cond, out);
            out.push_str(" ? ");
            emit_arithmetic(then_val, out);
            out.push_str(" : ");
            emit_arithmetic(else_val, out);
            out.push(')');
        }

        Arithmetic::Assign(var, expr) => {
            out.push_str(var);
            out.push_str(" = ");
            emit_arithmetic(expr, out);
        }

        Arithmetic::Sequence(exprs) => {
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_arithmetic(e, out);
            }
        }
    }
}

fn emit_binop(l: &Arithmetic<String>, op: &str, r: &Arithmetic<String>, out: &mut String) {
    // Wrap binary sub-expressions in parens to preserve AST precedence grouping.
    // Without this, `(5 + 3) * 2` would emit as `5 + 3 * 2` (wrong precedence).
    let l_needs_parens = is_arith_binop(l);
    let r_needs_parens = is_arith_binop(r);

    if l_needs_parens {
        out.push('(');
    }
    emit_arithmetic(l, out);
    if l_needs_parens {
        out.push(')');
    }

    out.push_str(op);

    if r_needs_parens {
        out.push('(');
    }
    emit_arithmetic(r, out);
    if r_needs_parens {
        out.push(')');
    }
}

fn is_arith_binop(arith: &Arithmetic<String>) -> bool {
    matches!(
        arith,
        Arithmetic::Add(..)
            | Arithmetic::Sub(..)
            | Arithmetic::Mult(..)
            | Arithmetic::Div(..)
            | Arithmetic::Modulo(..)
            | Arithmetic::Pow(..)
            | Arithmetic::Less(..)
            | Arithmetic::LessEq(..)
            | Arithmetic::Great(..)
            | Arithmetic::GreatEq(..)
            | Arithmetic::Eq(..)
            | Arithmetic::NotEq(..)
            | Arithmetic::BitwiseAnd(..)
            | Arithmetic::BitwiseOr(..)
            | Arithmetic::BitwiseXor(..)
            | Arithmetic::LogicalAnd(..)
            | Arithmetic::LogicalOr(..)
            | Arithmetic::ShiftLeft(..)
            | Arithmetic::ShiftRight(..)
    )
}

/// Check if an arithmetic expression contains operations that fish math can't handle:
/// bitwise ops (&, |, ^, <<, >>), increment/decrement, assignment.
fn arith_has_unsupported(arith: &Arithmetic<String>) -> bool {
    match arith {
        Arithmetic::BitwiseAnd(..)
        | Arithmetic::BitwiseOr(..)
        | Arithmetic::BitwiseXor(..)
        | Arithmetic::ShiftLeft(..)
        | Arithmetic::ShiftRight(..)
        | Arithmetic::BitwiseNot(..)
        | Arithmetic::PostIncr(..)
        | Arithmetic::PreIncr(..)
        | Arithmetic::PostDecr(..)
        | Arithmetic::PreDecr(..)
        | Arithmetic::Assign(..) => true,

        // Recurse into sub-expressions
        Arithmetic::Add(l, r)
        | Arithmetic::Sub(l, r)
        | Arithmetic::Mult(l, r)
        | Arithmetic::Div(l, r)
        | Arithmetic::Modulo(l, r)
        | Arithmetic::Pow(l, r)
        | Arithmetic::Less(l, r)
        | Arithmetic::LessEq(l, r)
        | Arithmetic::Great(l, r)
        | Arithmetic::GreatEq(l, r)
        | Arithmetic::Eq(l, r)
        | Arithmetic::NotEq(l, r)
        | Arithmetic::LogicalAnd(l, r)
        | Arithmetic::LogicalOr(l, r) => {
            arith_has_unsupported(l) || arith_has_unsupported(r)
        }

        Arithmetic::UnaryPlus(e) | Arithmetic::UnaryMinus(e) | Arithmetic::LogicalNot(e) => {
            arith_has_unsupported(e)
        }

        Arithmetic::Ternary(c, t, f) => {
            arith_has_unsupported(c) || arith_has_unsupported(t) || arith_has_unsupported(f)
        }

        Arithmetic::Sequence(exprs) => exprs.iter().any(arith_has_unsupported),

        Arithmetic::Var(_) | Arithmetic::Literal(_) => false,
    }
}

/// Check if an arithmetic expression requires test-based evaluation
/// (comparisons, logical ops, ternary — things fish math can't handle).
fn arith_needs_test(arith: &Arithmetic<String>) -> bool {
    matches!(
        arith,
        Arithmetic::Less(..)
            | Arithmetic::LessEq(..)
            | Arithmetic::Great(..)
            | Arithmetic::GreatEq(..)
            | Arithmetic::Eq(..)
            | Arithmetic::NotEq(..)
            | Arithmetic::LogicalAnd(..)
            | Arithmetic::LogicalOr(..)
            | Arithmetic::LogicalNot(..)
            | Arithmetic::Ternary(..)
    )
}

/// Emit an arithmetic expression that needs test-based evaluation as a fish command.
/// Returns a command substitution like `(if test $x -gt 5; echo 1; else; echo 0; end)`.
fn emit_arith_as_command(
    arith: &Arithmetic<String>,
    out: &mut String,
) -> Result<(), TranslateError> {
    match arith {
        Arithmetic::Ternary(cond, then_val, else_val) => {
            out.push_str("(if ");
            emit_arith_condition(cond, out)?;
            out.push_str("; echo ");
            emit_arith_value(then_val, out)?;
            out.push_str("; else; echo ");
            emit_arith_value(else_val, out)?;
            out.push_str("; end)");
            Ok(())
        }
        // Standalone comparisons return 0 or 1 in bash
        _ => {
            out.push_str("(");
            emit_arith_condition(arith, out)?;
            out.push_str("; and echo 1; or echo 0)");
            Ok(())
        }
    }
}

/// Emit an arithmetic expression as a fish test condition.
fn emit_arith_condition(
    arith: &Arithmetic<String>,
    out: &mut String,
) -> Result<(), TranslateError> {
    match arith {
        Arithmetic::Less(l, r) => emit_test_cmp(l, "-lt", r, out),
        Arithmetic::LessEq(l, r) => emit_test_cmp(l, "-le", r, out),
        Arithmetic::Great(l, r) => emit_test_cmp(l, "-gt", r, out),
        Arithmetic::GreatEq(l, r) => emit_test_cmp(l, "-ge", r, out),
        Arithmetic::Eq(l, r) => emit_test_cmp(l, "-eq", r, out),
        Arithmetic::NotEq(l, r) => emit_test_cmp(l, "-ne", r, out),
        Arithmetic::LogicalAnd(l, r) => {
            emit_arith_condition(l, out)?;
            out.push_str("; and ");
            emit_arith_condition(r, out)
        }
        Arithmetic::LogicalOr(l, r) => {
            emit_arith_condition(l, out)?;
            out.push_str("; or ");
            emit_arith_condition(r, out)
        }
        Arithmetic::LogicalNot(e) => {
            out.push_str("not ");
            emit_arith_condition(e, out)
        }
        // Non-comparison: treat as truthy (non-zero = true in bash arithmetic)
        _ => {
            out.push_str("test ");
            emit_arith_value(arith, out)?;
            out.push_str(" -ne 0");
            Ok(())
        }
    }
}

/// Emit a test comparison: `test <lhs> <op> <rhs>`
fn emit_test_cmp(
    l: &Arithmetic<String>,
    op: &str,
    r: &Arithmetic<String>,
    out: &mut String,
) -> Result<(), TranslateError> {
    out.push_str("test ");
    emit_arith_value(l, out)?;
    out.push(' ');
    out.push_str(op);
    out.push(' ');
    emit_arith_value(r, out)
}

/// Emit an arithmetic expression as a value (for use in test operands or echo).
/// Simple vars/literals are emitted directly, complex expressions are wrapped in (math "...").
fn emit_arith_value(
    arith: &Arithmetic<String>,
    out: &mut String,
) -> Result<(), TranslateError> {
    match arith {
        Arithmetic::Var(name) => {
            out.push('$');
            out.push_str(name);
            Ok(())
        }
        Arithmetic::Literal(n) => {
            out.push_str(&n.to_string());
            Ok(())
        }
        _ if arith_needs_test(arith) => {
            // Nested comparison/ternary as a value — emit as command substitution
            emit_arith_as_command(arith, out)
        }
        _ => {
            out.push_str("(math \"");
            emit_arithmetic(arith, out);
            out.push_str("\")");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------------

fn emit_redirect(redir: &Redir, out: &mut String) -> Result<(), TranslateError> {
    match redir {
        Redirect::Read(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push('<');
            emit_word(word, out)?;
        }
        Redirect::Write(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push('>');
            emit_word(word, out)?;
        }
        Redirect::Append(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push_str(">>");
            emit_word(word, out)?;
        }
        Redirect::ReadWrite(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push_str("<>");
            emit_word(word, out)?;
        }
        Redirect::Clobber(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push_str(">|");
            emit_word(word, out)?;
        }
        Redirect::Heredoc(_fd, _word) => {
            return Err(TranslateError::Unsupported("heredoc".into()));
        }
        Redirect::DupRead(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push_str("<&");
            emit_word(word, out)?;
        }
        Redirect::DupWrite(fd, word) => {
            if let Some(fd) = fd {
                out.push_str(&fd.to_string());
            }
            out.push_str(">&");
            emit_word(word, out)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to extract a simple string from a TopLevelWord.
fn word_as_str(word: &TLWord) -> Option<String> {
    match &word.0 {
        ComplexWord::Single(Word::Simple(SimpleWord::Literal(s))) => Some(s.clone()),
        ComplexWord::Single(Word::SingleQuoted(s)) => Some(s.clone()),
        _ => {
            // Try rendering the word to a string for simple cases
            // (e.g., [[ which is SquareOpen+SquareOpen)
            let mut buf = String::new();
            if word_to_simple_string(&word.0, &mut buf) {
                Some(buf)
            } else {
                None
            }
        }
    }
}

/// Check if a word contains glob characters (* or ?).
/// Used to detect for-loops iterating over file globs.
fn word_has_glob(word: &TLWord) -> bool {
    fn check_complex(cw: &ComplexWord<Wd>) -> bool {
        match cw {
            ComplexWord::Single(w) => check_word(w),
            ComplexWord::Concat(words) => words.iter().any(|w| check_word(w)),
        }
    }
    fn check_word(w: &Wd) -> bool {
        match w {
            Word::Simple(sw) => check_sw(sw),
            Word::DoubleQuoted(parts) => parts.iter().any(|sw| check_sw(sw)),
            Word::SingleQuoted(_) => false,
        }
    }
    fn check_sw(sw: &Sw) -> bool {
        matches!(sw, SimpleWord::Star | SimpleWord::Question)
    }
    check_complex(&word.0)
}

/// Try to render a ComplexWord to a simple string (no substitutions).
/// Returns false if the word contains anything dynamic.
fn word_to_simple_string(word: &ComplexWord<Wd>, out: &mut String) -> bool {
    match word {
        ComplexWord::Single(w) => word_part_to_string(w, out),
        ComplexWord::Concat(words) => {
            for w in words {
                if !word_part_to_string(w, out) {
                    return false;
                }
            }
            true
        }
    }
}

fn word_part_to_string(word: &Wd, out: &mut String) -> bool {
    match word {
        Word::Simple(sw) => simple_word_to_string(sw, out),
        Word::SingleQuoted(s) => {
            out.push_str(s);
            true
        }
        _ => false,
    }
}

fn simple_word_to_string(sw: &Sw, out: &mut String) -> bool {
    match sw {
        SimpleWord::Literal(s) => {
            out.push_str(s);
            true
        }
        SimpleWord::Escaped(s) => {
            out.push_str(s);
            true
        }
        SimpleWord::SquareOpen => {
            out.push('[');
            true
        }
        SimpleWord::SquareClose => {
            out.push(']');
            true
        }
        SimpleWord::Tilde => {
            out.push('~');
            true
        }
        SimpleWord::Colon => {
            out.push(':');
            true
        }
        SimpleWord::Star => {
            out.push('*');
            true
        }
        SimpleWord::Question => {
            out.push('?');
            true
        }
        _ => false, // Param, Subst — not a simple string
    }
}

/// Extract a variable name from a Parameter.
fn param_name(param: &Parameter<String>) -> String {
    match param {
        Parameter::Var(name) => name.clone(),
        Parameter::Positional(n) => format!("argv[{}]", n),
        Parameter::At | Parameter::Star => "argv".to_string(),
        Parameter::Pound => "ARGC".to_string(),
        Parameter::Question => "status".to_string(),
        Parameter::Dollar => "fish_pid".to_string(),
        Parameter::Bang => "last_pid".to_string(),
        Parameter::Dash => "FISH_FLAGS".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn t(bash: &str) -> String {
        translate_bash_to_fish(bash).unwrap()
    }

    // --- Simple commands ---

    #[test]
    fn simple_echo() {
        assert_eq!(t("echo hello world"), "echo hello world");
    }

    #[test]
    fn simple_pipeline() {
        assert_eq!(
            t("cat file | grep foo | wc -l"),
            "cat file | grep foo | wc -l"
        );
    }

    #[test]
    fn and_or_chain() {
        assert_eq!(
            t("mkdir -p foo && cd foo || echo fail"),
            "mkdir -p foo; and cd foo; or echo fail"
        );
    }

    // --- Variable assignment ---

    #[test]
    fn standalone_assignment() {
        assert_eq!(t("FOO=bar"), "set FOO bar");
    }

    #[test]
    fn env_prefix_command() {
        assert_eq!(t("FOO=bar command"), "env FOO=bar command");
    }

    // --- Export ---

    #[test]
    fn export_simple() {
        assert_eq!(t("export EDITOR=vim"), "set -gx EDITOR vim");
    }

    // --- For loop ---

    #[test]
    fn for_loop_with_seq() {
        let result = t("for i in $(seq 5); do echo $i; done");
        assert!(result.contains("for i in (seq 5 | string split -n ' ')"));
        assert!(result.contains("echo $i"));
        assert!(result.contains("end"));
    }

    #[test]
    fn for_loop_word_split_echo() {
        // Bare $(echo a b c) in for-loop should get string split
        let result = t("for f in $(echo a b c); do echo $f; done");
        assert!(result.contains("for f in (echo a b c | string split -n ' ')"));
    }

    #[test]
    fn for_loop_literal_words_no_split() {
        // Literal words in for-loop should NOT get string split
        let result = t("for f in a b c; do echo $f; done");
        assert!(result.contains("for f in a b c"));
        assert!(!result.contains("string split"));
    }

    #[test]
    fn for_loop_quoted_subst_no_split() {
        // Quoted "$(cmd)" should NOT get string split (quotes suppress it in bash)
        let result = t("for f in \"$(echo a b c)\"; do echo $f; done");
        assert!(!result.contains("string split"));
    }

    // --- If ---

    #[test]
    fn if_then_fi() {
        let result = t("if test -f foo; then echo exists; fi");
        assert!(result.contains("if test -f foo"));
        assert!(result.contains("echo exists"));
        assert!(result.contains("end"));
    }

    #[test]
    fn if_else() {
        let result = t("if test -f foo; then echo yes; else echo no; fi");
        assert!(result.contains("if test -f foo"));
        assert!(result.contains("echo yes"));
        assert!(result.contains("else"));
        assert!(result.contains("echo no"));
        assert!(result.contains("end"));
    }

    // --- While ---

    #[test]
    fn while_loop() {
        let result = t("while true; do echo loop; done");
        assert!(result.contains("while true"));
        assert!(result.contains("echo loop"));
        assert!(result.contains("end"));
    }

    // --- Command substitution ---

    #[test]
    fn command_substitution() {
        assert_eq!(t("echo $(whoami)"), "echo (whoami)");
    }

    // --- Arithmetic ---

    #[test]
    fn arithmetic_substitution() {
        let result = t("echo $((2 + 2))");
        assert!(result.contains("math"));
        assert!(result.contains("2 + 2"));
    }

    // --- Parameters ---

    #[test]
    fn special_params() {
        assert_eq!(t("echo $?"), "echo $status");
    }

    #[test]
    fn positional_params() {
        assert_eq!(t("echo $1"), "echo $argv[1]");
    }

    #[test]
    fn all_args() {
        assert_eq!(t("echo $@"), "echo $argv");
    }

    // --- Unset ---

    #[test]
    fn unset_var() {
        assert_eq!(t("unset FOO"), "set -e FOO");
    }

    // --- Local ---

    #[test]
    fn local_var() {
        assert_eq!(t("local FOO=bar"), "set -l FOO bar");
    }

    // --- Background job ---

    #[test]
    fn background_job() {
        assert_eq!(t("sleep 10 &"), "sleep 10 &");
    }

    // --- Negated pipeline ---

    #[test]
    fn negated_pipeline() {
        assert_eq!(t("! grep -q pattern file"), "not grep -q pattern file");
    }

    // --- Case ---

    #[test]
    fn case_statement() {
        let result = t("case $1 in foo) echo foo;; bar) echo bar;; esac");
        assert!(result.contains("switch"));
        assert!(result.contains("case foo"));
        assert!(result.contains("echo foo"));
        assert!(result.contains("case bar"));
        assert!(result.contains("echo bar"));
        assert!(result.contains("end"));
    }

    // --- Redirects ---

    #[test]
    fn stderr_redirect() {
        assert_eq!(t("cmd 2>/dev/null"), "cmd 2>/dev/null");
    }

    #[test]
    fn stderr_to_stdout() {
        assert_eq!(t("cmd 2>&1"), "cmd 2>&1");
    }

    // --- Brace group ---

    #[test]
    fn brace_group() {
        let result = t("{ echo a; echo b; }");
        assert!(result.contains("begin"));
        assert!(result.contains("echo a"));
        assert!(result.contains("echo b"));
        assert!(result.contains("end"));
    }

    // =======================================================================
    // EXOTIC / OBSCURE / HARD PATTERNS
    // =======================================================================

    // --- Nested command substitution ---

    #[test]
    fn nested_command_substitution() {
        let result = t("echo $(basename $(pwd))");
        assert!(result.contains("(basename (pwd))"));
    }

    #[test]
    fn command_subst_in_args() {
        // Common Stack Overflow pattern
        let result = t("$(which python3) --version");
        assert!(result.contains("(which python3)"));
    }

    // --- Multiple statements ---

    #[test]
    fn semicolon_separated() {
        let result = t("echo a; echo b; echo c");
        assert!(result.contains("echo a"));
        assert!(result.contains("echo b"));
        assert!(result.contains("echo c"));
    }

    // --- Parameter expansion varieties ---

    #[test]
    fn param_default_value() {
        // ${var:-default} — the bread and butter of bash scripts
        let result = t("echo ${HOME:-/tmp}");
        assert!(result.contains("set -q HOME"));
        assert!(result.contains("echo $HOME"));
        assert!(result.contains("/tmp"));
    }

    #[test]
    fn param_assign_default() {
        // ${var:=default} — assign if unset
        let result = t("echo ${FOO:=hello}");
        assert!(result.contains("set -q FOO"));
        assert!(result.contains("set FOO"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn param_error_if_unset() {
        // ${var:?message} — error if unset
        let result = t("echo ${REQUIRED:?must be set}");
        assert!(result.contains("set -q REQUIRED"));
        assert!(result.contains("return 1"));
    }

    #[test]
    fn param_alternative_value() {
        // ${var:+word} — use word if var IS set
        let result = t("echo ${DEBUG:+--verbose}");
        assert!(result.contains("set -q DEBUG"));
        assert!(result.contains("--verbose"));
    }

    #[test]
    fn param_length() {
        // ${#var} — string length
        let result = t("echo ${#PATH}");
        assert!(result.contains("string length"));
        assert!(result.contains("$PATH"));
    }

    #[test]
    fn param_strip_suffix() {
        // ${file%.txt} — remove suffix
        let result = t("echo ${file%.*}");
        assert!(result.contains("string replace"));
        assert!(result.contains("$file"));
    }

    #[test]
    fn param_strip_prefix() {
        // ${path#*/} — remove prefix
        let result = t("echo ${path#*/}");
        assert!(result.contains("string replace"));
        assert!(result.contains("$path"));
    }

    // --- Complex arithmetic ---

    #[test]
    fn arithmetic_multiplication() {
        let result = t("echo $((3 * 4 + 1))");
        assert!(result.contains("math"));
    }

    #[test]
    fn arithmetic_modulo() {
        let result = t("echo $((x % 2))");
        assert!(result.contains("math"));
        assert!(result.contains("%"));
    }

    #[test]
    fn arithmetic_comparison() {
        // $((a > b)) returns 0 or 1 in bash — translated to test-based evaluation
        let result = t("echo $((a > b))");
        assert!(result.contains("test"), "got: {}", result);
        assert!(result.contains("-gt"), "got: {}", result);
    }

    #[test]
    fn arithmetic_in_double_quotes() {
        // $((x * 2)) inside a double-quoted string: the math subst is pulled outside
        // the double quotes to avoid inner " conflicts.
        // "result is $((x * 2))" → "result is "(math "$x * 2")
        let result = t(r#"echo "result is $((x * 2))""#);
        assert!(result.contains("math"), "got: {}", result);
        // The outer string should close before math
        assert!(result.contains(r#""result is ""#), "outer quotes should close before math, got: {}", result);
    }

    // --- Nested control structures ---

    #[test]
    fn nested_for_if() {
        let result =
            t("for f in $(ls); do if test -f $f; then echo $f is a file; fi; done");
        assert!(result.contains("for f in (ls | string split -n ' ')"));
        assert!(result.contains("if test -f $f"));
        assert!(result.contains("end\nend"));
    }

    #[test]
    fn if_elif_else() {
        let result = t(
            "if test $x -eq 1; then echo one; elif test $x -eq 2; then echo two; else echo other; fi",
        );
        assert!(result.contains("if test $x -eq 1"));
        assert!(result.contains("else if test $x -eq 2"));
        assert!(result.contains("echo one"));
        assert!(result.contains("echo two"));
        assert!(result.contains("else\necho other"));
        assert!(result.contains("end"));
    }

    // --- Until loop (not in fish) ---

    #[test]
    fn until_loop() {
        let result = t("until test -f /tmp/ready; do sleep 1; done");
        assert!(result.contains("while not"));
        assert!(result.contains("test -f /tmp/ready"));
        assert!(result.contains("sleep 1"));
        assert!(result.contains("end"));
    }

    // --- Multiple env var prefix ---

    #[test]
    fn multi_env_prefix() {
        let result = t("CC=gcc CXX=g++ make");
        assert!(result.contains("env"));
        assert!(result.contains("CC=gcc"));
        assert!(result.contains("CXX=g++"));
        assert!(result.contains("make"));
    }

    // --- Multiple assignments ---

    #[test]
    fn multi_assignment() {
        let result = t("A=1; B=2; C=3");
        assert!(result.contains("set A 1"));
        assert!(result.contains("set B 2"));
        assert!(result.contains("set C 3"));
    }

    // --- Quoting ---

    #[test]
    fn single_quoted_string() {
        assert_eq!(t("echo 'hello world'"), "echo 'hello world'");
    }

    #[test]
    fn double_quoted_with_var() {
        let result = t("echo \"hello $USER\"");
        assert!(result.contains("\"hello $USER\""));
    }

    #[test]
    fn double_quoted_with_subst() {
        // Command substitutions inside double quotes get split out to avoid
        // inner quote conflicts: "today is $(date)" → "today is "(date)
        let result = t("echo \"today is $(date)\"");
        assert!(result.contains("\"today is \""), "got: {}", result);
        assert!(result.contains("(date)"), "got: {}", result);
    }

    // --- Complex real-world one-liners from Stack Overflow ---

    #[test]
    fn find_and_exec() {
        // Common pattern people paste
        let result = t("find . -name '*.py' -exec grep -l TODO {} +");
        assert!(result.contains("find"));
        assert!(result.contains("'*.py'"));
    }

    #[test]
    fn while_read_loop() {
        // Very common: while read line; do ... done < file
        let result = t("while read line; do echo $line; done");
        assert!(result.contains("while read line"));
        assert!(result.contains("echo $line"));
        assert!(result.contains("end"));
    }

    #[test]
    fn chained_and_or_complex() {
        let result = t("test -d /opt && echo exists || mkdir -p /opt && echo created");
        assert!(result.contains("test -d /opt"));
        assert!(result.contains("; and echo exists"));
        assert!(result.contains("; or mkdir -p /opt"));
        assert!(result.contains("; and echo created"));
    }

    // --- Subshell ---

    #[test]
    fn subshell() {
        // Subshells now bail to T2 because fish begin/end doesn't isolate
        let result = translate_bash_to_fish("(cd /tmp && ls)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("subshell"));
    }

    // --- Function definition ---

    #[test]
    fn function_def() {
        let result = t("greet() { echo hello $1; }");
        assert!(result.contains("function greet"));
        assert!(result.contains("echo hello $argv[1]"));
        assert!(result.contains("end"));
    }

    // --- Complex pipeline with redirects ---

    #[test]
    fn pipeline_with_redirect() {
        let result = t("cat file 2>/dev/null | sort | uniq -c | sort -rn");
        assert!(result.contains("cat file 2>/dev/null"));
        assert!(result.contains("| sort |"));
        assert!(result.contains("| uniq -c |"));
        assert!(result.contains("sort -rn"));
    }

    // --- Backtick command substitution ---

    #[test]
    fn backtick_substitution() {
        // `cmd` is an older form of $(cmd) — conch-parser parses both
        let result = t("echo `whoami`");
        assert!(result.contains("(whoami)"));
    }

    // --- Export multiple vars ---

    #[test]
    fn export_multiple() {
        let result = t("export A=1; export B=2");
        assert!(result.contains("set -gx A 1"));
        assert!(result.contains("set -gx B 2"));
    }

    // --- Declare with flags ---

    #[test]
    fn declare_export() {
        assert_eq!(t("declare -x FOO=bar"), "set -gx FOO bar");
    }

    // --- Case with wildcards ---

    #[test]
    fn case_with_wildcards() {
        let result = t("case $1 in *.txt) echo text;; *.py) echo python;; *) echo unknown;; esac");
        assert!(result.contains("switch"));
        // Patterns with * are quoted to prevent file globbing (fish case still treats quoted * as wildcard)
        assert!(result.contains("case '*.txt'"));
        assert!(result.contains("case '*.py'"));
        assert!(result.contains("case '*'"));
    }

    // --- Multi-line for with complex body ---

    #[test]
    fn for_with_pipeline_body() {
        let result = t("for f in $(find . -name '*.log'); do cat $f | wc -l; done");
        assert!(result.contains("for f in"));
        assert!(result.contains("cat $f | wc -l"));
        assert!(result.contains("end"));
    }

    // --- Redirect append ---

    #[test]
    fn append_redirect() {
        assert_eq!(t("echo hello >>log.txt"), "echo hello >>log.txt");
    }

    // --- Input redirect ---

    #[test]
    fn input_redirect() {
        assert_eq!(t("sort <input.txt"), "sort <input.txt");
    }

    // --- Variable in double-quoted redirect target ---

    #[test]
    fn redirect_with_var() {
        let result = t("echo hello >$LOGFILE");
        assert!(result.contains("echo hello >$LOGFILE"));
    }

    // --- Empty command (just comments or blank) ---

    #[test]
    fn comment_only() {
        // Comments are stripped by the parser — empty output
        let result = t("# this is a comment");
        assert_eq!(result, "");
    }

    // --- Special variables ---

    #[test]
    fn dollar_dollar() {
        assert_eq!(t("echo $$"), "echo $fish_pid");
    }

    #[test]
    fn dollar_bang() {
        let result = t("echo $!");
        assert!(result.contains("$last_pid"));
    }

    #[test]
    fn dollar_pound() {
        let result = t("echo $#");
        assert!(result.contains("count $argv"));
    }

    // --- Tilde expansion ---

    #[test]
    fn tilde_expansion() {
        let result = t("cd ~/projects");
        assert!(result.contains("~"));
        assert!(result.contains("projects"));
    }

    // --- Escaped characters ---

    #[test]
    fn escaped_dollar() {
        let result = t("echo \\$HOME");
        assert!(result.contains("\\$"));
    }

    // --- Multiple pipelines joined with && ---

    #[test]
    fn complex_and_or_pipeline() {
        let result = t("cat file | grep foo && echo found || echo not found");
        assert!(result.contains("cat file | grep foo"));
        assert!(result.contains("; and echo found"));
        assert!(result.contains("; or echo not found"));
    }

    // --- For loop without explicit word list ---

    #[test]
    fn for_without_in() {
        // for var; do ... done iterates over positional params
        let result = t("for arg; do echo $arg; done");
        assert!(result.contains("for arg in $argv"));
        assert!(result.contains("echo $arg"));
        assert!(result.contains("end"));
    }

    // --- Nested arithmetic ---

    #[test]
    fn nested_arithmetic() {
        let result = t("echo $((2 * (3 + 4)))");
        assert!(result.contains("math"));
    }

    // --- Parse error falls through ---

    #[test]
    fn double_bracket_test() {
        // [[ ]] is bash-specific — conch-parser parses it as a command,
        // we translate [[ to test and strip ]]
        let result = t("[[ -n $HOME ]]");
        assert!(result.contains("test -n $HOME"), "got: {}", result);
        assert!(!result.contains("[["));
        assert!(!result.contains("]]"));
    }

    #[test]
    fn double_bracket_equality() {
        // [[ $a == $b ]] → string match for pattern matching
        let result = t("[[ $a == $b ]]");
        assert!(result.contains("string match -q"), "got: {}", result);
    }

    #[test]
    fn double_bracket_wildcard_pattern() {
        // [[ "world" == w* ]] → string match -q 'w*' "world"
        let result = t(r#"if [[ "world" == w* ]]; then echo yes; fi"#);
        assert!(result.contains("string match -q 'w*'"), "got: {}", result);
        assert!(result.contains("echo yes"), "got: {}", result);
    }

    #[test]
    fn double_bracket_negated_pattern() {
        // [[ $x != *.txt ]] → not string match -q '*.txt' $x
        let result = t("[[ $x != *.txt ]]");
        assert!(result.contains("not string match -q"), "got: {}", result);
    }

    #[test]
    fn double_bracket_and() {
        // [[ -f x && -r x ]] → test -f x; and test -r x
        let result = t("if [[ -f /etc/hostname && -r /etc/hostname ]]; then echo ok; fi");
        assert!(result.contains("test -f /etc/hostname"), "got: {}", result);
        assert!(result.contains("; and test -r /etc/hostname"), "got: {}", result);
    }

    #[test]
    fn double_bracket_or() {
        let result = t("[[ -z \"$x\" || -z \"$y\" ]]");
        assert!(result.contains("test -z \"$x\""), "got: {}", result);
        assert!(result.contains("; or test -z \"$y\""), "got: {}", result);
    }

    #[test]
    fn double_bracket_regex() {
        let result = t(r#"[[ "$str" =~ ^[a-z]+$ ]]"#);
        assert!(result.contains("string match -rq"), "got: {}", result);
        assert!(result.contains("-- \"$str\""), "got: {}", result);
    }

    #[test]
    fn brace_range_simple() {
        let result = t("echo {1..5}");
        assert!(result.contains("echo (seq 1 5)"), "got: {}", result);
    }

    #[test]
    fn brace_range_with_step() {
        let result = t("for i in {1..10..2}; do echo $i; done");
        assert!(result.contains("seq 1 2 10"), "got: {}", result);
    }

    #[test]
    fn ternary_arithmetic() {
        let result = t("echo $((x > 5 ? 1 : 0))");
        assert!(result.contains("if test $x -gt 5"), "got: {}", result);
        assert!(result.contains("echo 1"), "got: {}", result);
        assert!(result.contains("echo 0"), "got: {}", result);
    }

    #[test]
    fn herestring_with_preceding_statement() {
        let result = t(r#"name="world"; grep -o "world" <<< "hello $name""#);
        assert!(result.contains("set name \"world\""), "got: {}", result);
        assert!(result.contains("echo \"hello $name\" | grep"), "got: {}", result);
    }

    // --- Real-world: install script patterns ---

    #[test]
    fn curl_pipe_bash() {
        // People paste this ALL the time
        let result = t("curl -fsSL https://example.com/install.sh | bash");
        assert!(result.contains("curl"));
        assert!(result.contains("| bash"));
    }

    #[test]
    fn git_clone_and_cd() {
        let result = t("git clone https://github.com/user/repo.git && cd repo");
        assert!(result.contains("git clone"));
        assert!(result.contains("; and cd repo"));
    }

    // --- Deeply nested ---

    #[test]
    fn deeply_nested_loops() {
        let result = t(
            "for i in 1 2 3; do for j in a b c; do echo $i$j; done; done",
        );
        assert!(result.contains("for i in 1 2 3"));
        assert!(result.contains("for j in a b c"));
        assert!(result.contains("echo $i$j"));
        // Two end keywords for the two loops
        let end_count = result.matches("end").count();
        assert!(end_count >= 2, "Expected at least 2 'end' keywords, got {}", end_count);
    }

    // --- If with && in condition ---

    #[test]
    fn if_with_and_condition() {
        let result = t("if test -f foo && test -r foo; then cat foo; fi");
        assert!(result.contains("if"));
        assert!(result.contains("test -f foo"));
        assert!(result.contains("test -r foo"));
        assert!(result.contains("cat foo"));
        assert!(result.contains("end"));
    }

    // --- Command with single-quoted args containing special chars ---

    #[test]
    fn single_quoted_special_chars() {
        let result = t("grep -E '^[0-9]+$' file.txt");
        assert!(result.contains("grep"));
        assert!(result.contains("'^[0-9]+$'"));
    }

    // --- Multiple exports on one line ---

    #[test]
    fn export_no_value() {
        // export VAR (no =) — just marks as exported
        let result = t("export HOME");
        assert!(result.contains("set -gx HOME $HOME"));
    }

    // --- Here-string (<<<) ---

    #[test]
    fn herestring_quoted() {
        let result = t(r#"while read line; do echo ">> $line"; done <<< "hello world""#);
        assert!(result.contains("echo \"hello world\" |"), "got: {}", result);
        assert!(result.contains("while read line"));
    }

    #[test]
    fn herestring_bare() {
        let result = t("cat <<< hello");
        assert!(result.contains("echo hello | cat"), "got: {}", result);
    }

    #[test]
    fn herestring_variable() {
        let result = t("grep foo <<< $input");
        assert!(result.contains("echo $input | grep foo"), "got: {}", result);
    }
}
