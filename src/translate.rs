use std::borrow::Cow;
use std::fmt;

use crate::ast::*;
use crate::lexer::ParseError;
use crate::parser::Parser;

/// Translation context threaded through all emitters.
struct Ctx {
    in_subshell: bool,
}

impl Ctx {
    fn new() -> Self {
        Ctx {
            in_subshell: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error produced during bash-to-fish translation.
#[derive(Debug)]
pub enum TranslateError {
    /// The input uses a bash feature that has no fish equivalent.
    Unsupported(&'static str),
    /// The input failed to parse as valid bash.
    Parse(ParseError),
}

impl fmt::Display for TranslateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranslateError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            TranslateError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TranslateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TranslateError::Parse(e) => Some(e),
            TranslateError::Unsupported(_) => None,
        }
    }
}

impl From<ParseError> for TranslateError {
    fn from(e: ParseError) -> Self {
        TranslateError::Parse(e)
    }
}

/// Module-local result alias — reduces `Result<(), TranslateError>` noise.
type Res<T> = Result<T, TranslateError>;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Translate a bash command string to fish shell syntax.
pub fn translate_bash_to_fish(input: &str) -> Result<String, TranslateError> {
    let cmds = Parser::new(input).parse()?;
    let mut ctx = Ctx::new();
    let mut out = String::with_capacity(input.len());
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        emit_cmd(&mut ctx, cmd, &mut out)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Command-level emitters
// ---------------------------------------------------------------------------

fn emit_cmd(ctx: &mut Ctx, cmd: &Cmd<'_>, out: &mut String) -> Res<()> {
    match cmd {
        Cmd::List(list) => emit_and_or(ctx, list, out),
        Cmd::Job(list) => {
            emit_and_or(ctx, list, out)?;
            out.push_str(" &");
            Ok(())
        }
    }
}

fn emit_and_or(ctx: &mut Ctx, list: &AndOrList<'_>, out: &mut String) -> Res<()> {
    emit_pipeline(ctx, &list.first, out)?;
    for and_or in &list.rest {
        match and_or {
            AndOr::And(p) => {
                out.push_str("; and ");
                emit_pipeline(ctx, p, out)?;
            }
            AndOr::Or(p) => {
                out.push_str("; or ");
                emit_pipeline(ctx, p, out)?;
            }
        }
    }
    Ok(())
}

fn emit_pipeline(ctx: &mut Ctx, pipeline: &Pipeline<'_>, out: &mut String) -> Res<()> {
    match pipeline {
        Pipeline::Single(exec) => emit_exec(ctx, exec, out),
        Pipeline::Pipe(negated, cmds) => {
            if *negated {
                out.push_str("not ");
            }
            for (i, c) in cmds.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                emit_exec(ctx, c, out)?;
            }
            Ok(())
        }
    }
}

fn emit_exec(ctx: &mut Ctx, exec: &Executable<'_>, out: &mut String) -> Res<()> {
    match exec {
        Executable::Simple(simple) => emit_simple(ctx, simple, out),
        Executable::Compound(compound) => emit_compound(ctx, compound, out),
        Executable::FuncDef(name, body) => {
            out.push_str("function ");
            out.push_str(name);
            out.push('\n');
            // Unwrap brace group to avoid nested begin/end inside function
            match &body.kind {
                CompoundKind::Brace(cmds) => emit_body(ctx, cmds, out)?,
                other => emit_compound_kind(ctx, other, out)?,
            }
            out.push_str("\nend");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Simple command
// ---------------------------------------------------------------------------

fn emit_simple(ctx: &mut Ctx, cmd: &SimpleCmd<'_>, out: &mut String) -> Res<()> {
    let mut env_vars: Vec<(&str, &Option<Word<'_>>)> = Vec::new();
    let mut array_ops: Vec<&CmdPrefix<'_>> = Vec::new();
    let mut cmd_words: Vec<&Word<'_>> = Vec::new();
    let mut redirects: Vec<&Redir<'_>> = Vec::new();
    let mut herestring: Option<&Word<'_>> = None;
    let mut heredoc: Option<&HeredocBody<'_>> = None;

    for item in &cmd.prefix {
        match item {
            CmdPrefix::Assign(name, val) => env_vars.push((name, val)),
            CmdPrefix::ArrayAssign(..) | CmdPrefix::ArrayAppend(..) => array_ops.push(item),
            CmdPrefix::Redirect(Redir::HereString(w)) => herestring = Some(w),
            CmdPrefix::Redirect(Redir::Heredoc(body)) => heredoc = Some(body),
            CmdPrefix::Redirect(r) => redirects.push(r),
        }
    }
    for item in &cmd.suffix {
        match item {
            CmdSuffix::Word(w) => cmd_words.push(w),
            CmdSuffix::Redirect(Redir::HereString(w)) => herestring = Some(w),
            CmdSuffix::Redirect(Redir::Heredoc(body)) => heredoc = Some(body),
            CmdSuffix::Redirect(r) => redirects.push(r),
        }
    }

    // Standalone assignment (no command words)
    if cmd_words.is_empty() {
        if !array_ops.is_empty() {
            return emit_array_assignments(ctx, &env_vars, &array_ops, out);
        }
        if !env_vars.is_empty() {
            return emit_var_assignments(ctx, &env_vars, out);
        }
    }

    let cmd_name = cmd_words.first().and_then(|w| word_as_str(w));

    // mapfile/readarray needs its own redirects before here-string emission
    if matches!(cmd_name.as_deref(), Some("mapfile" | "readarray")) {
        return emit_mapfile(ctx, &cmd_words, &redirects, herestring, out);
    }

    // Pipe input: here-string or heredoc
    if let Some(hs_word) = herestring {
        out.push_str("echo ");
        emit_word(ctx, hs_word, out)?;
        out.push_str(" | ");
    }
    if let Some(body) = heredoc {
        emit_heredoc_body(ctx, body, out)?;
        out.push_str(" | ");
    }

    // Prefix assignments with a command: VAR=val cmd args
    // Bail — fish list variables (PATH, CDPATH) expand differently under env,
    // and the scoping semantics are subtle. Let bash handle it.
    // Must check before builtin dispatch to avoid silently dropping the prefix.
    if !env_vars.is_empty() && !cmd_words.is_empty() {
        return Err(TranslateError::Unsupported("prefix assignment with command"));
    }

    // Builtin dispatch — returns early if handled
    if let Some(ref name) = cmd_name
        && let Some(result) = dispatch_builtin(ctx, name, &cmd_words, &redirects, out)
    {
        return result;
    }

    // `exit` inside a subshell can't be emulated with fish's begin/end —
    // `return` would exit the whole function, not just the begin block.
    // Bail to T2 bash-exec so it runs correctly in a real subprocess.
    if ctx.in_subshell && cmd_name.as_deref() == Some("exit") {
        return Err(TranslateError::Unsupported("exit in subshell"));
    }

    // Emit command and arguments
    for (i, word) in cmd_words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        emit_word(ctx, word, out)?;
    }

    // Emit redirects
    for redir in &redirects {
        out.push(' ');
        emit_redir(ctx, redir, out)?;
    }

    Ok(())
}

/// Emit standalone array assignments: `arr=(a b c)` → `set arr a b c`
fn emit_array_assignments(ctx: &mut Ctx, 
    env_vars: &[(&str, &Option<Word<'_>>)],
    array_ops: &[&CmdPrefix<'_>],
    out: &mut String,
) -> Res<()> {
    let set_kw = if ctx.in_subshell { "set -l " } else { "set " };
    let mut first = true;
    for (i, (name, value)) in env_vars.iter().enumerate() {
        if !first || i > 0 {
            out.push('\n');
        }
        first = false;
        out.push_str(set_kw);
        out.push_str(name);
        if let Some(val) = value {
            out.push(' ');
            emit_word(ctx, val, out)?;
        }
    }
    for op in array_ops {
        if !first {
            out.push('\n');
        }
        first = false;
        match op {
            CmdPrefix::ArrayAssign(name, words) => {
                out.push_str(set_kw);
                out.push_str(name);
                for w in words {
                    out.push(' ');
                    emit_word(ctx, w, out)?;
                }
            }
            CmdPrefix::ArrayAppend(name, words) => {
                out.push_str(if ctx.in_subshell { "set -la " } else { "set -a " });
                out.push_str(name);
                for w in words {
                    out.push(' ');
                    emit_word(ctx, w, out)?;
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

/// Emit standalone variable assignments: `VAR=val` → `set VAR val`
fn emit_var_assignments(ctx: &mut Ctx, 
    env_vars: &[(&str, &Option<Word<'_>>)],
    out: &mut String,
) -> Res<()> {
    let set_kw = if ctx.in_subshell { "set -l " } else { "set " };
    for (i, (name, value)) in env_vars.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(set_kw);
        out.push_str(name);
        if let Some(val) = value {
            out.push(' ');
            emit_word(ctx, val, out)?;
        }
    }
    Ok(())
}

/// Dispatch to builtin emitters. Returns `Some(result)` if handled, `None` to fall through.
fn dispatch_builtin(ctx: &mut Ctx, 
    name: &str,
    cmd_words: &[&Word<'_>],
    redirects: &[&Redir<'_>],
    out: &mut String,
) -> Option<Res<()>> {
    match name {
        "export" => Some(emit_export(ctx, &cmd_words[1..], out)),
        "unset" => Some(emit_unset(ctx, &cmd_words[1..], out)),
        "local" => Some(emit_local(ctx, &cmd_words[1..], out)),
        "declare" | "typeset" => Some(emit_declare(ctx, &cmd_words[1..], out)),
        "readonly" => Some(emit_readonly(ctx, &cmd_words[1..], out)),
        "[[" => Some(emit_double_bracket(ctx, &cmd_words[1..], redirects, out)),
        "let" => Some(emit_let(ctx, &cmd_words[1..], out)),
        "shopt" => Some(Err(TranslateError::Unsupported("shopt"))),
        "trap" => Some(emit_trap(ctx, &cmd_words[1..], out)),
        "shift" => Some(emit_shift(ctx, &cmd_words[1..], out)),
        "alias" => Some(emit_alias(ctx, &cmd_words[1..], out)),
        "read" => Some(emit_read(ctx, cmd_words, redirects, out)),
        "set" => Some(emit_bash_set(ctx, &cmd_words[1..], out)),
        "select" => Some(Err(TranslateError::Unsupported("select loop"))),
        "getopts" => Some(Err(TranslateError::Unsupported(
            "getopts (use argparse in fish)",
        ))),
        "exec" if cmd_words.len() == 1 && !redirects.is_empty() => {
            Some(Err(TranslateError::Unsupported("exec fd manipulation")))
        }
        "eval" => Some(emit_eval(ctx, &cmd_words[1..], out)),
        "printf" => dispatch_printf(ctx, cmd_words, out),
        _ => None,
    }
}

/// Handle `printf` special cases. Returns `Some` if the call was handled.
fn dispatch_printf(ctx: &mut Ctx, 
    cmd_words: &[&Word<'_>],
    out: &mut String,
) -> Option<Res<()>> {
    // Detect repetition pattern: printf '%0.sCHAR' {1..N} or printf '%.0sCHAR' {1..N}
    if cmd_words.len() >= 3
        && let Some(fmt) = word_as_str(cmd_words[1])
        && let Some(ch) = extract_printf_repeat_char(fmt.as_ref())
        && let Some(count) = extract_brace_range_count(&cmd_words[2..])
    {
        out.push_str("string repeat -n ");
        itoa(out, count);
        out.push_str(" -- '");
        out.push(ch);
        out.push('\'');
        return Some(Ok(()));
    }
    // Reject unsupported %0.s format if not the repeat pattern
    for w in &cmd_words[1..] {
        let text: Cow<'_, str> = if let Some(s) = word_as_str(w) {
            s
        } else {
            let mut buf = String::with_capacity(64);
            let _ = emit_word(ctx, w, &mut buf);
            Cow::Owned(buf)
        };
        if text.contains("%0.s") || text.contains("%.0s") {
            return Some(Err(TranslateError::Unsupported(
                "printf %0.s format (fish printf doesn't support this)",
            )));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Bash builtin translations
// ---------------------------------------------------------------------------

/// `export VAR=val` → `set -gx VAR val`
fn emit_export(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg)
            && s.starts_with('-')
        {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;

        if let Some((var_name, value_parts)) = split_word_at_equals(ctx, arg) {
            out.push_str("set -gx ");
            out.push_str(&var_name);
            if !value_parts.is_empty() {
                out.push(' ');
                // PATH-like variables: split colon-separated values into fish list
                if var_name.ends_with("PATH") && value_parts.contains(':') {
                    out.push_str(&value_parts.replace(':', " "));
                } else {
                    out.push_str(&value_parts);
                }
            }
        } else if let Some(s) = word_as_str(arg) {
            out.push_str("set -gx ");
            out.push_str(&s);
            out.push_str(" $");
            out.push_str(&s);
        } else {
            out.push_str("set -gx ");
            emit_word(ctx, arg, out)?;
        }
    }
    Ok(())
}

/// Split a word at the first `=` sign, returning (`var_name`, `value_as_fish`).
fn split_word_at_equals(ctx: &mut Ctx, word: &Word<'_>) -> Option<(String, String)> {
    let mut full = String::with_capacity(64);
    if emit_word(ctx, word, &mut full).is_err() {
        return None;
    }
    let eq_pos = full.find('=')?;
    let value_part = full.split_off(eq_pos + 1);
    full.pop(); // remove trailing '='
    let var_name = full;

    let value = if value_part.len() >= 2
        && ((value_part.starts_with('"') && value_part.ends_with('"'))
            || (value_part.starts_with('\'') && value_part.ends_with('\'')))
    {
        let mut v = value_part;
        v.pop();
        v.remove(0);
        v
    } else {
        value_part
    };

    Some((var_name, value))
}

/// `unset VAR` → `set -e VAR`
fn emit_unset(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let mut first = true;
    for arg in args {
        let s = word_as_str(arg);
        if matches!(s.as_deref(), Some(f) if f.starts_with('-')) {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;
        // Check for array element pattern: arr[n]
        if let Some(ref s) = s
            && let Some((name, idx_str)) = parse_array_index_str(s)
            && let Ok(idx) = idx_str.parse::<i64>()
        {
            out.push_str("set -e ");
            out.push_str(name);
            out.push('[');
            itoa(out, idx + 1);
            out.push(']');
            continue;
        }
        out.push_str("set -e ");
        emit_word(ctx, arg, out)?;
    }
    Ok(())
}

/// Parse `name[index]` pattern from a string.
fn parse_array_index_str(s: &str) -> Option<(&str, &str)> {
    let bracket = s.find('[')?;
    if !s.ends_with(']') {
        return None;
    }
    let name = &s[..bracket];
    let idx = &s[bracket + 1..s.len() - 1];
    if name.is_empty() || idx.is_empty() {
        return None;
    }
    Some((name, idx))
}

/// `local VAR=val` → `set -l VAR val`
fn emit_local(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let mut first = true;
    for arg in args {
        let s = word_as_str(arg);
        if matches!(s.as_deref(), Some(f) if f.starts_with('-')) {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;

        if let Some(s) = s {
            out.push_str("set -l ");
            if let Some(eq) = s.find('=') {
                out.push_str(&s[..eq]);
                out.push(' ');
                out.push_str(&s[eq + 1..]);
            } else {
                out.push_str(&s);
            }
        } else if let Some((name, val)) = split_word_at_equals(ctx, arg) {
            out.push_str("set -l ");
            out.push_str(&name);
            out.push(' ');
            out.push_str(&val);
        } else {
            out.push_str("set -l ");
            emit_word(ctx, arg, out)?;
        }
    }
    Ok(())
}

/// `declare [-x] [-g] VAR=val` → `set [-gx] VAR val`
/// `declare -p VAR` → `set --show VAR`
fn emit_declare(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let mut scope = "-g";
    let mut print_mode = false;
    let mut remaining = Vec::new();

    for arg in args {
        if let Some(s) = word_as_str(arg) {
            match &*s {
                "-n" => {
                    return Err(TranslateError::Unsupported("declare -n (nameref)"));
                }
                "-A" | "-Ag" | "-gA" => {
                    return Err(TranslateError::Unsupported(
                        "declare -A (associative array)",
                    ));
                }
                "-p" => print_mode = true,
                "-x" => scope = "-gx",
                "-g" => scope = "-g",
                s if s.starts_with('-') => {}
                _ => remaining.push(*arg),
            }
        } else {
            remaining.push(*arg);
        }
    }

    if print_mode {
        if remaining.is_empty() {
            out.push_str("set --show");
        } else {
            for (i, arg) in remaining.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str("set --show ");
                emit_word(ctx, arg, out)?;
            }
        }
        return Ok(());
    }

    let mut first = true;
    for arg in &remaining {
        if !first {
            out.push('\n');
        }
        first = false;

        if let Some((var_name, value_parts)) = split_word_at_equals(ctx, arg) {
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
            emit_word(ctx, arg, out)?;
        }
    }
    Ok(())
}

/// `read` — strip bash-specific flags that don't exist in fish.
/// `trap 'handler' SIG ...` → `function __reef_trap_SIG --on-signal SIG; handler; end`
/// `trap 'handler' EXIT` → `function __reef_trap_EXIT --on-event fish_exit; handler; end`
/// `trap - SIG` → `functions -e __reef_trap_SIG`
fn emit_trap(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    if args.is_empty() {
        return Err(TranslateError::Unsupported("bare trap"));
    }

    let handler_str = word_as_str(args[0]);

    if handler_str.as_deref() == Some("-") {
        for sig_word in &args[1..] {
            let sig = word_as_str(sig_word)
                .ok_or(TranslateError::Unsupported("trap with dynamic signal"))?;
            let name = sig.strip_prefix("SIG").unwrap_or(&sig);
            out.push_str("functions -e __reef_trap_");
            out.push_str(name);
        }
        return Ok(());
    }

    if args.len() < 2 {
        return Err(TranslateError::Unsupported("trap with missing signal"));
    }

    // Get fish body from handler: either translate from string or emit directly
    let fish_body = match &handler_str {
        Some(h) if h.is_empty() => String::new(),
        Some(h) => translate_bash_to_fish(h)?,
        None => {
            // Handler contains variables — emit it as fish command directly
            let mut body = String::with_capacity(128);
            emit_word_unquoted(ctx, args[0], &mut body)?;
            translate_bash_to_fish(&body)?
        }
    };

    for (i, sig_word) in args[1..].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let sig =
            word_as_str(sig_word).ok_or(TranslateError::Unsupported("trap with dynamic signal"))?;
        let name = sig.strip_prefix("SIG").unwrap_or(&sig);

        // ERR trap has no fish equivalent
        if name == "ERR" {
            return Err(TranslateError::Unsupported("trap ERR (no fish equivalent)"));
        }

        out.push_str("function __reef_trap_");
        out.push_str(name);
        if name == "EXIT" || name == "0" {
            out.push_str(" --on-event fish_exit");
        } else {
            out.push_str(" --on-signal ");
            out.push_str(name);
        }
        if fish_body.is_empty() {
            out.push_str("; end");
        } else {
            out.push('\n');
            out.push_str(&fish_body);
            out.push_str("\nend");
        }
    }
    Ok(())
}

fn emit_read(ctx: &mut Ctx, 
    cmd_words: &[&Word<'_>],
    redirects: &[&Redir<'_>],
    out: &mut String,
) -> Res<()> {
    out.push_str("read");
    let mut skip_next = false;
    for word in &cmd_words[1..] {
        if skip_next {
            skip_next = false;
            // Emit the prompt argument for -P
            out.push_str(" -P ");
            emit_word(ctx, word, out)?;
            continue;
        }
        if let Some(s) = word_as_str(word) {
            if s == "-r" || s == "-ra" || s == "-ar" {
                // fish read is raw by default; -a handled below
                if s.contains('a') {
                    out.push_str(" --list");
                }
                continue;
            }
            if s == "-a" {
                // bash read -a → fish read --list
                out.push_str(" --list");
                continue;
            }
            if s == "-p" {
                // bash read -p "prompt" → fish read -P "prompt"
                skip_next = true;
                continue;
            }
            // Handle combined flags like -rp, -rn, etc.
            if s.as_bytes()[0] == b'-' && s.len() > 1 && s.as_bytes()[1] != b'-' {
                let mut wrote_flags = false;
                let mut needs_prompt = false;
                for &b in &s.as_bytes()[1..] {
                    match b {
                        b'r' => {} // skip -r (fish default)
                        b'a' => out.push_str(" --list"),
                        b'p' => needs_prompt = true,
                        _ => {
                            if !wrote_flags {
                                out.push_str(" -");
                                wrote_flags = true;
                            }
                            out.push(b as char);
                        }
                    }
                }
                if needs_prompt {
                    skip_next = true;
                }
                continue;
            }
        }
        out.push(' ');
        emit_word(ctx, word, out)?;
    }
    emit_redirects(ctx, redirects, out)?;
    Ok(())
}

/// `mapfile -t arr <<< "$(cmd)"` → `set arr (cmd)`
/// `readarray -t arr < <(cmd)` → `set arr (cmd)`
fn emit_mapfile(ctx: &mut Ctx, 
    cmd_words: &[&Word<'_>],
    redirects: &[&Redir<'_>],
    herestring: Option<&Word<'_>>,
    out: &mut String,
) -> Res<()> {
    // Parse args: skip flags (-t, -d, etc.), find variable name
    let mut var_name: Cow<'_, str> = Cow::Borrowed("MAPFILE"); // default bash array name
    let mut skip_next = false;
    for word in &cmd_words[1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if let Some(s) = word_as_str(word) {
            match s.as_bytes().first() {
                Some(b'-') => {
                    // -t strips trailing newline (fish does this by default)
                    // -O, -s, -c, -C, -d, -n, -u take an argument
                    match &*s {
                        "-O" | "-s" | "-c" | "-C" | "-d" | "-n" | "-u" => skip_next = true,
                        _ => {}
                    }
                }
                _ => var_name = s,
            }
        }
    }
    // Check herestring first
    if let Some(hs_word) = herestring {
        // mapfile -t arr <<< "$(cmd)" → set arr (string split \n -- "content")
        out.push_str("set ");
        out.push_str(&var_name);
        out.push_str(" (string split -- \\n ");
        emit_word(ctx, hs_word, out)?;
        out.push(')');
        return Ok(());
    }

    // Find the input source from redirects
    let mut has_input_redir = false;
    for redir in redirects {
        match redir {
            Redir::HereString(word) => {
                out.push_str("set ");
                out.push_str(&var_name);
                out.push_str(" (string split -- \\n ");
                emit_word(ctx, word, out)?;
                out.push(')');
                has_input_redir = true;
                break;
            }
            Redir::Read(_, word) => {
                // mapfile -t arr < <(cmd) — extract commands from ProcSubIn
                out.push_str("set ");
                out.push_str(&var_name);
                out.push_str(" (");
                if let Some(cmds) = extract_procsub_cmds(word) {
                    for (i, cmd) in cmds.iter().enumerate() {
                        if i > 0 {
                            out.push_str("; ");
                        }
                        emit_cmd(ctx, cmd, out)?;
                    }
                } else {
                    out.push_str("cat ");
                    emit_word(ctx, word, out)?;
                }
                out.push(')');
                has_input_redir = true;
                break;
            }
            _ => {}
        }
    }
    if !has_input_redir {
        out.push_str("set ");
        out.push_str(&var_name);
        out.push_str(" (cat)");
    }
    Ok(())
}

/// `shift` → `set -e argv[1]`; `shift N` → `set argv $argv[(math "N+1")..]`
/// `eval "$(cmd)"` → `cmd | source`
/// `eval $var` / other forms → unsupported (fall to T2)
fn emit_eval(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    // Extract the command list from eval "$(cmd)" or eval $(cmd)
    let cmds = extract_eval_cmds(args).ok_or(TranslateError::Unsupported("eval"))?;
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        emit_cmd(ctx, cmd, out)?;
    }
    out.push_str(" | source");
    Ok(())
}

fn extract_eval_cmds<'a>(args: &[&'a Word<'a>]) -> Option<&'a [Cmd<'a>]> {
    let [arg] = args else { return None };
    let subst = match arg {
        Word::Simple(WordPart::DQuoted(atoms)) => match atoms.as_slice() {
            [Atom::Subst(s)] => s,
            _ => return None,
        },
        Word::Simple(WordPart::Bare(Atom::Subst(s))) => s,
        _ => return None,
    };
    match subst.as_ref() {
        Subst::Cmd(cmds) => Some(cmds),
        _ => None,
    }
}

/// `set -e`, `set -u`, `set -x`, `set -o pipefail` → no-op comments (fish has no equivalents).
/// `set -- args...` → `set argv args...`
fn emit_bash_set(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    if args.is_empty() {
        out.push_str("set");
        return Ok(());
    }
    if let Some(first) = args.first().and_then(|w| word_as_str(w)) {
        let fb = first.as_bytes();
        if fb == b"--" {
            // set -- val1 val2 → set argv val1 val2
            out.push_str("set argv");
            for arg in &args[1..] {
                out.push(' ');
                emit_word(ctx, arg, out)?;
            }
            return Ok(());
        }
        // set [-+][euxo]... — shell options have no fish equivalent
        if fb.len() >= 2
            && (fb[0] == b'-' || fb[0] == b'+')
            && fb[1..]
                .iter()
                .all(|&b| matches!(b, b'e' | b'u' | b'x' | b'o'))
        {
            out.push_str("# set");
            for arg in args {
                out.push(' ');
                emit_word(ctx, arg, out)?;
            }
            out.push_str(" # no fish equivalent");
            return Ok(());
        }
    }
    // Unknown set usage — pass through
    out.push_str("set");
    for arg in args {
        out.push(' ');
        emit_word(ctx, arg, out)?;
    }
    Ok(())
}

fn emit_shift(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let Some(first) = args.first() else {
        out.push_str("set -e argv[1]");
        return Ok(());
    };
    if let Some(s) = word_as_str(first)
        && let Ok(n) = s.parse::<u32>()
    {
        if n <= 1 {
            out.push_str("set -e argv[1]");
        } else {
            out.push_str("set argv $argv[");
            itoa(out, i64::from(n + 1));
            out.push_str("..]");
        }
        return Ok(());
    }
    // Dynamic shift amount
    out.push_str("set argv $argv[(math \"");
    emit_word(ctx, first, out)?;
    out.push_str(" + 1\")..]");
    Ok(())
}

/// `alias name='cmd args'` → `alias name 'cmd args'`
fn emit_alias(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    out.push_str("alias");
    for arg in args {
        out.push(' ');
        if let Some(s) = word_as_str(arg) {
            // Bash alias format: name='value' or name="value"
            // Fish alias format: alias name 'value' (space-separated)
            if let Some(eq_pos) = s.find('=') {
                let name = &s[..eq_pos];
                let value = &s[eq_pos + 1..];
                // Strip surrounding quotes from value if present
                let unquoted = if (value.starts_with('\'') && value.ends_with('\''))
                    || (value.starts_with('"') && value.ends_with('"'))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                out.push_str(name);
                out.push(' ');
                out.push('\'');
                out.push_str(unquoted);
                out.push('\'');
                continue;
            }
        }
        emit_word(ctx, arg, out)?;
    }
    Ok(())
}

/// `readonly VAR=val` → `set -g VAR val`
fn emit_readonly(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    let mut first = true;
    for arg in args {
        if let Some(s) = word_as_str(arg)
            && s.starts_with('-')
        {
            continue;
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
        } else if let Some((name, val)) = split_word_at_equals(ctx, arg) {
            out.push_str("set -g ");
            out.push_str(&name);
            out.push(' ');
            out.push_str(&val);
        } else {
            out.push_str("set -g ");
            emit_word(ctx, arg, out)?;
        }
    }
    Ok(())
}

/// `let expr` → parse each argument as an arithmetic expression and emit.
fn emit_let(ctx: &mut Ctx, args: &[&Word<'_>], out: &mut String) -> Res<()> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Reconstruct the argument as a string and re-parse as arithmetic
        let mut arg_str = String::with_capacity(32);
        emit_word_unquoted(ctx, arg, &mut arg_str)?;

        // Parse the let argument as an arithmetic expression
        let mut parser = Parser::new(&arg_str);
        match parser.arith(0) {
            Ok(arith) => {
                emit_standalone_arith(ctx, &arith, out)?;
            }
            Err(_) => {
                return Err(TranslateError::Unsupported("'let' with complex expression"));
            }
        }
    }
    Ok(())
}

/// Emit `string match [-rq|-q] 'pattern' -- subject` for [[ ]] operators.
fn emit_string_match(ctx: &mut Ctx, 
    lhs: &[&Word<'_>],
    rhs: &[&Word<'_>],
    regex: bool,
    negated: bool,
    out: &mut String,
) -> Res<()> {
    if regex {
        // For regex matching, capture into __bash_rematch so ${BASH_REMATCH[n]} works.
        // `set __bash_rematch (string match -r ...)` returns 0 on match.
        if negated {
            out.push_str("not ");
        }
        out.push_str("set __bash_rematch (string match -r -- ");
    } else {
        if negated {
            out.push_str("not ");
        }
        out.push_str("string match -q -- ");
    }
    let mut pat_buf = String::with_capacity(32);
    for (i, w) in rhs.iter().enumerate() {
        if i > 0 {
            pat_buf.push(' ');
        }
        emit_word_unquoted(ctx, w, &mut pat_buf)?;
    }
    push_sq_escaped(out, &pat_buf);
    out.push(' ');
    for w in lhs {
        emit_word(ctx, w, out)?;
    }
    if regex {
        out.push(')');
    }
    Ok(())
}

/// `[[ cond ]]` → `test cond` or `string match -q pattern subject`
fn emit_double_bracket(ctx: &mut Ctx, 
    args: &[&Word<'_>],
    redirects: &[&Redir<'_>],
    out: &mut String,
) -> Res<()> {
    // Strip trailing ]]
    let filtered = if args.last().and_then(|a| word_as_str(a)).as_deref() == Some("]]") {
        &args[..args.len() - 1]
    } else {
        args
    };

    // Strip leading `!` negation operator
    let (filtered, bang_negated) =
        if !filtered.is_empty() && word_as_str(filtered[0]).as_deref() == Some("!") {
            (&filtered[1..], true)
        } else {
            (filtered, false)
        };

    // Find =~ operator position for regex matching
    let regex_pos = filtered
        .iter()
        .position(|a| word_as_str(a).as_deref() == Some("=~"));

    // Find == or != operator position for glob pattern matching
    let op_pos = filtered.iter().position(|a| {
        let s = word_as_str(a);
        matches!(s.as_deref(), Some("==" | "!="))
    });

    // [[ -v var ]] → set -q var
    if filtered.len() == 2
        && let Some(flag) = word_as_str(filtered[0])
        && flag.as_ref() == "-v"
    {
        if bang_negated {
            out.push_str("not ");
        }
        out.push_str("set -q ");
        emit_word(ctx, filtered[1], out)?;
        emit_redirects(ctx, redirects, out)?;
        return Ok(());
    }

    if let Some(pos) = regex_pos {
        emit_string_match(ctx, &filtered[..pos], &filtered[pos + 1..], true, bang_negated, out)?;
    } else if let Some(pos) = op_pos {
        let negated = word_as_str(filtered[pos]).as_deref() == Some("!=");
        // XOR: `[[ ! x != y ]]` → double negation cancels out
        emit_string_match(ctx, &filtered[..pos], &filtered[pos + 1..], false, negated ^ bang_negated, out)?;
    } else {
        if bang_negated {
            out.push_str("not ");
        }
        out.push_str("test");
        for arg in filtered {
            out.push(' ');
            emit_word(ctx, arg, out)?;
        }
    }

    emit_redirects(ctx, redirects, out)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Compound commands
// ---------------------------------------------------------------------------

fn emit_compound(ctx: &mut Ctx, cmd: &CompoundCmd<'_>, out: &mut String) -> Res<()> {
    let herestring = cmd.redirects.iter().find_map(|r| match r {
        Redir::HereString(w) => Some(w),
        _ => None,
    });
    let heredoc = cmd.redirects.iter().find_map(|r| match r {
        Redir::Heredoc(body) => Some(body),
        _ => None,
    });
    if let Some(hs_word) = herestring {
        out.push_str("echo ");
        emit_word(ctx, hs_word, out)?;
        out.push_str(" | ");
    }
    if let Some(body) = heredoc {
        emit_heredoc_body(ctx, body, out)?;
        out.push_str(" | ");
    }
    emit_compound_kind(ctx, &cmd.kind, out)?;
    for redir in &cmd.redirects {
        if matches!(redir, Redir::HereString(..) | Redir::Heredoc(..)) {
            continue;
        }
        out.push(' ');
        emit_redir(ctx, redir, out)?;
    }
    Ok(())
}

/// If the word is a bare (unquoted) command substitution like $(cmd),
/// return a reference to the commands inside.
fn get_bare_command_subst<'a>(word: &'a Word<'a>) -> Option<&'a [Cmd<'a>]> {
    match word {
        Word::Simple(WordPart::Bare(Atom::Subst(subst))) => match subst.as_ref() {
            Subst::Cmd(cmds) => Some(cmds),
            _ => None,
        },
        _ => None,
    }
}

/// Check if the word is a bare unquoted `$var` that bash would word-split.
fn is_bare_var_ref(word: &Word<'_>) -> bool {
    matches!(word, Word::Simple(WordPart::Bare(Atom::Param(Param::Var(_)))))
}

/// Emit a command substitution with `| string split -n ' '` inside the parens,
/// replicating bash's IFS word splitting for for-loop word lists.
fn emit_command_subst_with_split(ctx: &mut Ctx, cmds: &[Cmd<'_>], out: &mut String) -> Res<()> {
    out.push('(');
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        emit_cmd(ctx, cmd, out)?;
    }
    out.push_str(" | string split -n ' ')");
    Ok(())
}

fn emit_compound_kind(ctx: &mut Ctx, kind: &CompoundKind<'_>, out: &mut String) -> Res<()> {
    match kind {
        CompoundKind::For { var, words, body } => {
            out.push_str("for ");
            out.push_str(var);
            out.push_str(" in ");
            if let Some(words) = words {
                for (i, w) in words.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    if let Some(cmds) = get_bare_command_subst(w) {
                        emit_command_subst_with_split(ctx, cmds, out)?;
                    } else if is_bare_var_ref(w) {
                        // Bash word-splits unquoted $var; fish doesn't.
                        // Wrap in string split to match bash semantics.
                        out.push_str("(string split -n -- ' ' ");
                        emit_word(ctx, w, out)?;
                        out.push(')');
                    } else {
                        emit_word(ctx, w, out)?;
                    }
                }
            } else {
                out.push_str("$argv");
            }
            out.push('\n');
            emit_body(ctx, body, out)?;
            out.push_str("\nend");
        }

        CompoundKind::While(guard_body) => {
            out.push_str("while ");
            emit_guard(ctx, &guard_body.guard, out)?;
            out.push('\n');
            emit_body(ctx, &guard_body.body, out)?;
            out.push_str("\nend");
        }

        CompoundKind::Until(guard_body) => {
            out.push_str("while not ");
            emit_guard(ctx, &guard_body.guard, out)?;
            out.push('\n');
            emit_body(ctx, &guard_body.body, out)?;
            out.push_str("\nend");
        }

        CompoundKind::If {
            conditionals,
            else_branch,
        } => {
            for (i, guard_body) in conditionals.iter().enumerate() {
                if i == 0 {
                    out.push_str("if ");
                } else {
                    out.push_str("\nelse if ");
                }
                emit_guard(ctx, &guard_body.guard, out)?;
                out.push('\n');
                emit_body(ctx, &guard_body.body, out)?;
            }
            if let Some(else_body) = else_branch {
                out.push_str("\nelse\n");
                emit_body(ctx, else_body, out)?;
            }
            out.push_str("\nend");
        }

        CompoundKind::Case { word, arms } => {
            out.push_str("switch ");
            emit_word(ctx, word, out)?;
            out.push('\n');
            let mut pat_buf = String::with_capacity(32);
            for arm in arms {
                out.push_str("case ");
                for (i, pattern) in arm.patterns.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    pat_buf.clear();
                    emit_word(ctx, pattern, &mut pat_buf)?;

                    if let Some(expanded) = expand_bracket_pattern(&pat_buf) {
                        out.push_str(&expanded);
                    } else if pat_buf.contains('*') || pat_buf.contains('?') {
                        push_sq_escaped(out, &pat_buf);
                    } else {
                        out.push_str(&pat_buf);
                    }
                }
                out.push('\n');
                emit_body(ctx, &arm.body, out)?;
                out.push('\n');
            }
            out.push_str("end");
        }

        CompoundKind::CFor {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(init_expr) = init {
                emit_standalone_arith(ctx, init_expr, out)?;
                out.push('\n');
            }
            out.push_str("while ");
            if let Some(cond_expr) = cond {
                emit_arith_condition(cond_expr, out)?;
            } else {
                out.push_str("true");
            }
            out.push('\n');
            emit_body(ctx, body, out)?;
            if let Some(step_expr) = step {
                out.push('\n');
                emit_standalone_arith(ctx, step_expr, out)?;
            }
            out.push_str("\nend");
        }

        CompoundKind::Brace(cmds) => {
            out.push_str("begin\n");
            emit_body(ctx, cmds, out)?;
            out.push_str("\nend");
        }

        CompoundKind::Subshell(cmds) => {
            if cmds.is_empty() {
                return Err(TranslateError::Unsupported("empty subshell"));
            }
            out.push_str("begin\n");
            out.push_str("set -l __reef_pwd (pwd)\n");
            let prev = ctx.in_subshell;
            ctx.in_subshell = true;
            emit_body(ctx, cmds, out)?;
            ctx.in_subshell = prev;
            out.push_str(
                "\nset -l __reef_rc $status; cd $__reef_pwd 2>/dev/null\nend",
            );
        }

        CompoundKind::DoubleBracket(cmds) => {
            emit_body(ctx, cmds, out)?;
        }

        CompoundKind::Arithmetic(arith) => {
            emit_standalone_arith(ctx, arith, out)?;
        }
    }
    Ok(())
}

/// Expand a pure bracket pattern [chars] to space-separated alternatives.
fn expand_bracket_pattern(pat: &str) -> Option<String> {
    if !pat.starts_with('[') || !pat.ends_with(']') || pat.len() < 3 {
        return None;
    }
    let inner = &pat[1..pat.len() - 1];
    if inner.contains('-') {
        return None;
    }
    let mut result = String::with_capacity(inner.len() * 4);
    for (i, &b) in inner.as_bytes().iter().enumerate() {
        if i > 0 {
            result.push(' ');
        }
        if b == b'\'' {
            result.push_str("'\\'''");
        } else {
            result.push('\'');
            result.push(b as char);
            result.push('\'');
        }
    }
    Some(result)
}

fn emit_guard(ctx: &mut Ctx, guard: &[Cmd<'_>], out: &mut String) -> Res<()> {
    if guard.len() == 1 {
        emit_cmd(ctx, &guard[0], out)?;
    } else {
        out.push_str("begin; ");
        for (i, cmd) in guard.iter().enumerate() {
            if i > 0 {
                out.push_str("; ");
            }
            emit_cmd(ctx, cmd, out)?;
        }
        out.push_str("; end");
    }
    Ok(())
}

fn emit_body(ctx: &mut Ctx, cmds: &[Cmd<'_>], out: &mut String) -> Res<()> {
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        emit_cmd(ctx, cmd, out)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Word-level emitters
// ---------------------------------------------------------------------------

fn emit_word(ctx: &mut Ctx, word: &Word<'_>, out: &mut String) -> Res<()> {
    // Check for nested brace expansion that fish handles in different order
    if word_has_nested_braces(word) {
        return Err(TranslateError::Unsupported(
            "nested brace expansion (fish expands in different order)",
        ));
    }
    match word {
        Word::Simple(p) => emit_word_part(ctx, p, out),
        Word::Concat(parts) => {
            for p in parts {
                emit_word_part(ctx, p, out)?;
            }
            Ok(())
        }
    }
}

/// Emit a word with its outer quoting layer stripped.
fn emit_word_unquoted(ctx: &mut Ctx, word: &Word<'_>, out: &mut String) -> Res<()> {
    match word {
        Word::Simple(WordPart::DQuoted(parts)) => {
            for part in parts {
                emit_atom(ctx, part, out)?;
            }
            Ok(())
        }
        Word::Simple(WordPart::SQuoted(s)) => {
            out.push_str(s);
            Ok(())
        }
        _ => emit_word(ctx, word, out),
    }
}

fn emit_word_part(ctx: &mut Ctx, part: &WordPart<'_>, out: &mut String) -> Res<()> {
    match part {
        WordPart::Bare(atom) => emit_atom(ctx, atom, out),
        WordPart::DQuoted(parts) => {
            let mut in_quotes = true;
            out.push('"');
            for atom in parts {
                if let Atom::Subst(_) = atom {
                    if in_quotes {
                        out.push('"');
                        in_quotes = false;
                    }
                } else if !in_quotes {
                    out.push('"');
                    in_quotes = true;
                }
                emit_atom(ctx, atom, out)?;
            }
            if in_quotes {
                out.push('"');
            }
            Ok(())
        }
        WordPart::SQuoted(s) => {
            out.push('\'');
            out.push_str(s);
            out.push('\'');
            Ok(())
        }
    }
}

fn emit_atom(ctx: &mut Ctx, atom: &Atom<'_>, out: &mut String) -> Res<()> {
    match atom {
        Atom::Lit(s) => {
            out.push_str(s);
            Ok(())
        }
        Atom::Escaped(s) => {
            out.push('\\');
            out.push_str(s);
            Ok(())
        }
        Atom::Param(param) => {
            check_untranslatable_var(param)?;
            emit_param(param, out);
            Ok(())
        }
        Atom::Subst(subst) => emit_subst(ctx, subst, out),
        Atom::Star => {
            out.push('*');
            Ok(())
        }
        Atom::Question => {
            out.push('?');
            Ok(())
        }
        Atom::SquareOpen => {
            out.push('[');
            Ok(())
        }
        Atom::SquareClose => {
            out.push(']');
            Ok(())
        }
        Atom::Tilde => {
            out.push('~');
            Ok(())
        }
        Atom::ProcSubIn(cmds) => {
            out.push('(');
            for (i, cmd) in cmds.iter().enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                emit_cmd(ctx, cmd, out)?;
            }
            out.push_str(" | psub)");
            Ok(())
        }
        Atom::AnsiCQuoted(s) => {
            emit_ansi_c_quoted(s, out);
            Ok(())
        }
        Atom::BraceRange { start, end, step } => {
            emit_brace_range(start, end, *step, out);
            Ok(())
        }
    }
}

/// Detect adjacent brace comma expansions like `{a,b}{1,2}` which fish expands
/// in a different order than bash. Checks the word structure for adjacent
/// `Lit("{")...Lit("...,...")...` groups.
fn word_has_nested_braces(word: &Word<'_>) -> bool {
    // Flatten to a string and check for }{  with commas in both groups
    let mut flat = String::with_capacity(64);
    match word {
        Word::Simple(p) => {
            flat_part(p, &mut flat);
        }
        Word::Concat(parts) => {
            for p in parts {
                flat_part(p, &mut flat);
            }
        }
    }
    has_nested_brace_expansion(&flat)
}

fn flat_part(part: &WordPart<'_>, out: &mut String) {
    match part {
        WordPart::Bare(Atom::Lit(s)) | WordPart::SQuoted(s) => out.push_str(s),
        WordPart::Bare(_) | WordPart::DQuoted(_) => {} // skip non-literal atoms and variables
    }
}

fn has_nested_brace_expansion(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut brace_count = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut has_comma = false;
            i += 1;
            while i < bytes.len() && bytes[i] != b'}' {
                if bytes[i] == b',' {
                    has_comma = true;
                }
                i += 1;
            }
            if i < bytes.len() && has_comma {
                brace_count += 1;
                if brace_count >= 2 {
                    return true;
                }
            } else {
                brace_count = 0;
            }
        } else {
            brace_count = 0;
        }
        i += 1;
    }
    false
}

/// Translate bash `$'...'` ANSI-C quoting to fish.
/// Fish only interprets escape sequences like `\n`, `\t` outside of quotes,
/// so we use double quotes for literal text and break out for escapes.
/// Ensure we're outside double quotes (bare mode for fish escape sequences).
#[inline]
fn ensure_bare(in_dq: &mut bool, out: &mut String) {
    if *in_dq {
        out.push('"');
        *in_dq = false;
    }
}

/// Ensure we're inside double quotes (for literal text).
#[inline]
fn ensure_dquoted(in_dq: &mut bool, out: &mut String) {
    if !*in_dq {
        out.push('"');
        *in_dq = true;
    }
}

fn emit_ansi_c_quoted(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_dq = false;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                // Escapes that fish interprets only bare (outside quotes)
                b'n' | b't' | b'r' | b'a' | b'b' | b'e' | b'f' | b'v' => {
                    ensure_bare(&mut in_dq, out);
                    out.push('\\');
                    out.push(bytes[i + 1] as char);
                    i += 2;
                }
                b'E' => {
                    ensure_bare(&mut in_dq, out);
                    out.push_str("\\e");
                    i += 2;
                }
                b'x' | b'0' => {
                    ensure_bare(&mut in_dq, out);
                    out.push('\\');
                    i += 1;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'x' || bytes[i] == b'0')
                    {
                        out.push(bytes[i] as char);
                        i += 1;
                    }
                }
                b'\'' => {
                    ensure_dquoted(&mut in_dq, out);
                    out.push('\'');
                    i += 2;
                }
                b'\\' => {
                    ensure_dquoted(&mut in_dq, out);
                    out.push_str("\\\\");
                    i += 2;
                }
                b'?' => {
                    ensure_dquoted(&mut in_dq, out);
                    out.push('?');
                    i += 2;
                }
                _ => {
                    ensure_dquoted(&mut in_dq, out);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                }
            }
        } else {
            ensure_dquoted(&mut in_dq, out);
            match bytes[i] {
                b'$' => out.push_str("\\$"),
                b'"' => out.push_str("\\\""),
                _ => out.push(bytes[i] as char),
            }
            i += 1;
        }
    }
    if in_dq {
        out.push('"');
    }
}

fn emit_brace_range(start: &str, end: &str, step: Option<&str>, out: &mut String) {
    // Alpha range: {a..z} → expand inline
    let sc = start.as_bytes().first().copied().unwrap_or(0);
    let ec = end.as_bytes().first().copied().unwrap_or(0);
    if start.len() == 1 && end.len() == 1 && sc.is_ascii_alphabetic() && ec.is_ascii_alphabetic() {
        out.push_str(&expand_alpha_range(sc as char, ec as char));
        return;
    }

    // Numeric range
    if let Some(step) = step {
        out.push_str("(seq ");
        out.push_str(start);
        out.push(' ');
        out.push_str(step);
        out.push(' ');
        out.push_str(end);
        out.push(')');
    } else if let (Ok(s), Ok(e)) = (start.parse::<i64>(), end.parse::<i64>()) {
        out.push_str("(seq ");
        out.push_str(start);
        if s > e {
            out.push_str(" -1 ");
        } else {
            out.push(' ');
        }
        out.push_str(end);
        out.push(')');
    } else {
        out.push_str("(seq ");
        out.push_str(start);
        out.push(' ');
        out.push_str(end);
        out.push(')');
    }
}

fn expand_alpha_range(start: char, end: char) -> String {
    let (lo, hi) = if start <= end {
        (start as u8, end as u8)
    } else {
        (end as u8, start as u8)
    };
    let count = (hi - lo + 1) as usize;
    let mut result = String::with_capacity(count * 2);
    if start <= end {
        for c in lo..=hi {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push(c as char);
        }
    } else {
        for c in (lo..=hi).rev() {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push(c as char);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Parameter and substitution emitters
// ---------------------------------------------------------------------------

/// Reject bash-specific variables that have no fish equivalent.
fn check_untranslatable_var(param: &Param<'_>) -> Res<()> {
    if let Param::Var(name) = param {
        match *name {
            "LINENO" => return Err(TranslateError::Unsupported("$LINENO")),
            "FUNCNAME" => return Err(TranslateError::Unsupported("$FUNCNAME")),
            "SECONDS" => return Err(TranslateError::Unsupported("$SECONDS")),
            "COMP_WORDS" | "COMP_CWORD" | "COMP_LINE" | "COMP_POINT" => {
                return Err(TranslateError::Unsupported("bash completion variable"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn emit_param(param: &Param<'_>, out: &mut String) {
    match param {
        Param::Var("RANDOM") => out.push_str("(random)"),
        Param::Var("HOSTNAME") => out.push_str("$hostname"),
        Param::Var("BASH_SOURCE" | "BASH_SOURCE[@]") => {
            out.push_str("(status filename)");
        }
        Param::Var("PIPESTATUS") => out.push_str("$pipestatus"),
        Param::Var(name) => {
            out.push('$');
            out.push_str(name);
        }
        Param::Positional(n) => {
            if *n == 0 {
                out.push_str("(status filename)");
            } else {
                out.push_str("$argv[");
                itoa(out, i64::from(*n));
                out.push(']');
            }
        }
        Param::At | Param::Star => out.push_str("$argv"),
        Param::Pound => out.push_str("(count $argv)"),
        Param::Status => out.push_str("$status"),
        Param::Pid => out.push_str("$fish_pid"),
        Param::Bang => out.push_str("$last_pid"),
        Param::Dash => out.push_str("\"\""),
    }
}

fn emit_subst(ctx: &mut Ctx, subst: &Subst<'_>, out: &mut String) -> Res<()> {
    match subst {
        Subst::Cmd(cmds) => {
            out.push('(');
            for (i, cmd) in cmds.iter().enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                emit_cmd(ctx, cmd, out)?;
            }
            out.push(')');
            Ok(())
        }

        Subst::Arith(Some(arith)) => {
            if arith_has_unsupported(arith) {
                return Err(TranslateError::Unsupported(
                    "unsupported arithmetic (bitwise, increment, or assignment)",
                ));
            }
            if arith_needs_test(arith) {
                emit_arith_as_command(arith, out)
            } else {
                out.push_str("(math \"");
                emit_arith(arith, out);
                out.push_str("\")");
                Ok(())
            }
        }
        Subst::Arith(None) => {
            out.push_str("(math 0)");
            Ok(())
        }

        Subst::Indirect(name) => {
            // ${!ref} → $$ref in fish
            out.push_str("$$");
            out.push_str(name);
            Ok(())
        }

        Subst::PrefixList(prefix) => {
            // ${!prefix*} → (set -n | string match 'prefix*')
            out.push_str("(set -n | string match '");
            out.push_str(prefix);
            out.push_str("*')");
            Ok(())
        }

        Subst::Transform(name, op) => {
            match op {
                b'Q' => {
                    // ${var@Q} → (string escape -- $var)
                    out.push_str("(string escape -- $");
                    out.push_str(name);
                    out.push(')');
                    Ok(())
                }
                b'U' => {
                    // ${var@U} → (string upper -- $var)
                    out.push_str("(string upper -- $");
                    out.push_str(name);
                    out.push(')');
                    Ok(())
                }
                b'u' => {
                    // ${var@u} → capitalize first char
                    out.push_str("(string sub -l 1 -- $");
                    out.push_str(name);
                    out.push_str(" | string upper)(string sub -s 2 -- $");
                    out.push_str(name);
                    out.push(')');
                    Ok(())
                }
                b'L' => {
                    // ${var@L} → (string lower -- $var)
                    out.push_str("(string lower -- $");
                    out.push_str(name);
                    out.push(')');
                    Ok(())
                }
                b'E' => Err(TranslateError::Unsupported("${var@E} escape expansion")),
                b'P' => Err(TranslateError::Unsupported("${var@P} prompt expansion")),
                b'A' => Err(TranslateError::Unsupported("${var@A} assignment form")),
                b'K' => Err(TranslateError::Unsupported("${var@K} quoted key-value")),
                b'a' => Err(TranslateError::Unsupported("${var@a} attribute flags")),
                _ => Err(TranslateError::Unsupported(
                    "unsupported parameter transformation",
                )),
            }
        }

        Subst::Len(param) => {
            out.push_str("(string length -- \"");
            emit_param(param, out);
            out.push_str("\")");
            Ok(())
        }

        Subst::Default(param, word) => {
            out.push_str("(set -q ");
            emit_param_name(param, out);
            out.push_str("; and echo $");
            emit_param_name(param, out);
            out.push_str("; or echo ");
            if let Some(w) = word {
                emit_word(ctx, w, out)?;
            }
            out.push(')');
            Ok(())
        }

        Subst::Assign(param, word) => {
            out.push_str("(set -q ");
            emit_param_name(param, out);
            out.push_str("; or set ");
            emit_param_name(param, out);
            out.push(' ');
            if let Some(w) = word {
                emit_word(ctx, w, out)?;
            }
            out.push_str("; echo $");
            emit_param_name(param, out);
            out.push(')');
            Ok(())
        }

        Subst::Error(param, word) => {
            out.push_str("(set -q ");
            emit_param_name(param, out);
            out.push_str("; and echo $");
            emit_param_name(param, out);
            out.push_str("; or begin; echo ");
            if let Some(w) = word {
                emit_word(ctx, w, out)?;
            } else {
                out.push_str("'parameter ");
                emit_param_name(param, out);
                out.push_str(" not set'");
            }
            out.push_str(" >&2; return 1; end)");
            Ok(())
        }

        Subst::Alt(param, word) => {
            out.push_str("(set -q ");
            emit_param_name(param, out);
            out.push_str("; and echo ");
            if let Some(w) = word {
                emit_word(ctx, w, out)?;
            }
            out.push(')');
            Ok(())
        }

        Subst::TrimSuffixSmall(param, pattern) => {
            emit_string_op(ctx, param, pattern.as_ref(), "suffix", false, out)
        }
        Subst::TrimSuffixLarge(param, pattern) => {
            emit_string_op(ctx, param, pattern.as_ref(), "suffix", true, out)
        }
        Subst::TrimPrefixSmall(param, pattern) => {
            emit_string_op(ctx, param, pattern.as_ref(), "prefix", false, out)
        }
        Subst::TrimPrefixLarge(param, pattern) => {
            emit_string_op(ctx, param, pattern.as_ref(), "prefix", true, out)
        }

        Subst::Upper(all, param) => {
            if !all {
                // ${var^} capitalize first char: upper first char + rest
                out.push_str("(string sub -l 1 -- $");
                emit_param_name(param, out);
                out.push_str(" | string upper)(string sub -s 2 -- $");
                emit_param_name(param, out);
                out.push(')');
                return Ok(());
            }
            out.push_str("(string upper -- \"");
            emit_param(param, out);
            out.push_str("\")");
            Ok(())
        }
        Subst::Lower(all, param) => {
            if !all {
                out.push_str("(string sub -l 1 -- $");
                emit_param_name(param, out);
                out.push_str(" | string lower)(string sub -s 2 -- $");
                emit_param_name(param, out);
                out.push(')');
                return Ok(());
            }
            out.push_str("(string lower -- \"");
            emit_param(param, out);
            out.push_str("\")");
            Ok(())
        }

        Subst::Replace(param, pattern, replacement) => {
            emit_string_replace(ctx, param, pattern.as_ref(), replacement.as_ref(), false, false, false, out)
        }
        Subst::ReplaceAll(param, pattern, replacement) => {
            emit_string_replace(ctx, param, pattern.as_ref(), replacement.as_ref(), true, false, false, out)
        }
        Subst::ReplacePrefix(param, pattern, replacement) => {
            emit_string_replace(ctx, param, pattern.as_ref(), replacement.as_ref(), false, true, false, out)
        }
        Subst::ReplaceSuffix(param, pattern, replacement) => {
            emit_string_replace(ctx, param, pattern.as_ref(), replacement.as_ref(), false, false, true, out)
        }

        Subst::Substring(param, offset, length) => {
            out.push_str("(string sub -s (math \"");
            out.push_str(offset);
            out.push_str(" + 1\")");
            if let Some(len) = length {
                out.push_str(" -l (math \"");
                out.push_str(len);
                out.push_str("\")");
            }
            out.push_str(" -- \"");
            emit_param(param, out);
            out.push_str("\")");
            Ok(())
        }

        // --- Array operations ---
        Subst::ArrayElement(name, idx) => {
            if *name == "BASH_REMATCH" {
                out.push_str("$__bash_rematch[");
                emit_array_index(ctx, idx, out)?;
                out.push(']');
            } else if *name == "PIPESTATUS" {
                out.push_str("$pipestatus[");
                emit_array_index(ctx, idx, out)?;
                out.push(']');
            } else {
                // ${arr[n]} → $arr[n+1]  (bash 0-indexed → fish 1-indexed)
                out.push('$');
                out.push_str(name);
                out.push('[');
                emit_array_index(ctx, idx, out)?;
                out.push(']');
            }
            Ok(())
        }
        Subst::ArrayAll(name) => {
            // ${arr[@]} → $arr
            if *name == "PIPESTATUS" {
                out.push_str("$pipestatus");
            } else {
                out.push('$');
                out.push_str(name);
            }
            Ok(())
        }
        Subst::ArrayLen(name) => {
            // ${#arr[@]} → (count $arr)
            out.push_str("(count $");
            out.push_str(name);
            out.push(')');
            Ok(())
        }
        Subst::ArraySlice(name, offset, length) => {
            // ${arr[@]:offset:length} → $arr[(math "offset + 1")..(math "offset + length")]
            out.push('$');
            out.push_str(name);
            out.push_str("[(math \"");
            out.push_str(offset);
            out.push_str(" + 1\")..(math \"");
            if let Some(len) = length {
                out.push_str(offset);
                out.push_str(" + ");
                out.push_str(len);
            } else {
                // No length — to end of array
                out.push_str("(count $");
                out.push_str(name);
                out.push(')');
            }
            out.push_str("\")]");
            Ok(())
        }
    }
}

/// Emit a bash array index as a fish 1-based index.
/// Handles: literal numbers (compile-time +1), $var (math "$var + 1"),
/// and $((expr)) (inlines the arithmetic + 1).
fn emit_array_index(ctx: &mut Ctx, idx: &Word<'_>, out: &mut String) -> Res<()> {
    // Case 1: simple literal number — add 1 at compile time
    if let Some(s) = word_as_str(idx)
        && let Ok(n) = s.parse::<i64>()
    {
        itoa(out, n + 1);
        return Ok(());
    }

    // Case 2: $((arith_expr)) — inline the arithmetic expression
    if let Word::Simple(WordPart::Bare(Atom::Subst(subst))) = idx
        && let Subst::Arith(Some(arith)) = subst.as_ref()
    {
        out.push_str("(math \"");
        emit_arith(arith, out);
        out.push_str(" + 1\")");
        return Ok(());
    }

    // Case 3: other expressions ($var, etc.) — wrap in math
    out.push_str("(math \"");
    emit_word(ctx, idx, out)?;
    out.push_str(" + 1\")");
    Ok(())
}

/// Emit ${var%pattern} / ${var#pattern} style operations using fish string replace.
fn emit_string_op(ctx: &mut Ctx, 
    param: &Param<'_>,
    pattern: Option<&Word<'_>>,
    kind: &str,
    greedy: bool,
    out: &mut String,
) -> Res<()> {
    // For non-greedy suffix removal (%), use ^(.*)PATTERN$ → '$1'.
    // The greedy (.*) captures max prefix, leaving the shortest suffix.
    let suffix_small = kind == "suffix" && !greedy;

    out.push_str("(string replace -r -- '");

    if suffix_small {
        out.push_str("^(.*)");
    } else if kind == "prefix" {
        out.push('^');
    }

    if let Some(p) = pattern {
        // For suffix_small, pattern uses greedy * because the prefix
        // capture group handles shortest-suffix semantics.
        let pat_greedy = if suffix_small { true } else { greedy };
        emit_word_as_pattern(ctx, p, out, pat_greedy)?;
    }

    if kind == "suffix" {
        out.push('$');
    }

    if suffix_small {
        out.push_str("' '$1' $");
    } else {
        out.push_str("' '' $");
    }
    emit_param_name(param, out);
    out.push(')');
    Ok(())
}

/// Emit `${var/pat/rep}` family using fish `string replace`.
#[allow(clippy::too_many_arguments)]
fn emit_string_replace(ctx: &mut Ctx,
    param: &Param<'_>,
    pattern: Option<&Word<'_>>,
    replacement: Option<&Word<'_>>,
    all: bool,
    prefix: bool,
    suffix: bool,
    out: &mut String,
) -> Res<()> {
    let needs_regex = prefix || suffix || pattern.is_some_and(word_has_glob);

    out.push_str("(string replace ");
    if needs_regex {
        out.push_str("-r ");
    }
    if all {
        out.push_str("-a ");
    }
    out.push_str("-- '");

    if prefix {
        out.push('^');
    }
    if let Some(p) = pattern {
        if needs_regex {
            emit_word_as_pattern(ctx, p, out, true)?;
        } else {
            emit_word_unquoted(ctx, p, out)?;
        }
    }
    if suffix {
        out.push('$');
    }
    out.push_str("' '");
    if let Some(r) = replacement {
        emit_word_unquoted(ctx, r, out)?;
    }
    out.push_str("' \"$");
    emit_param_name(param, out);
    out.push_str("\")");
    Ok(())
}

/// Emit a word as a regex pattern (basic glob→regex conversion).
/// Uses lookahead to correctly convert non-greedy `*` by examining
/// the character that follows the glob star.
fn emit_word_as_pattern(ctx: &mut Ctx, 
    word: &Word<'_>,
    out: &mut String,
    greedy: bool,
) -> Res<()> {
    // Flatten pattern to a list of "pattern pieces" for lookahead
    let mut pieces: Vec<PatPiece<'_>> = Vec::new();
    match word {
        Word::Simple(p) => collect_pattern_pieces(p, &mut pieces),
        Word::Concat(parts) => {
            for p in parts {
                collect_pattern_pieces(p, &mut pieces);
            }
        }
    }

    // Emit with lookahead
    for piece in &pieces {
        match piece {
            PatPiece::Lit(s) => {
                for &b in s.as_bytes() {
                    match b {
                        b'.' | b'+' | b'(' | b')' | b'{' | b'}' | b'|' | b'\\' | b'^' | b'$' => {
                            out.push('\\');
                            out.push(b as char);
                        }
                        _ => out.push(b as char),
                    }
                }
            }
            PatPiece::Star => {
                if greedy {
                    out.push_str(".*");
                } else {
                    out.push_str(".*?");
                }
            }
            PatPiece::Question => out.push('.'),
            PatPiece::Other(atom) => emit_atom(ctx, atom, out)?,
        }
    }
    Ok(())
}

enum PatPiece<'a> {
    Lit(&'a str),
    Star,
    Question,
    Other(&'a Atom<'a>),
}

fn collect_pattern_pieces<'a>(part: &'a WordPart<'a>, pieces: &mut Vec<PatPiece<'a>>) {
    match part {
        WordPart::Bare(atom) => match atom {
            Atom::Lit(s) => pieces.push(PatPiece::Lit(s)),
            Atom::Star => pieces.push(PatPiece::Star),
            Atom::Question => pieces.push(PatPiece::Question),
            other => pieces.push(PatPiece::Other(other)),
        },
        WordPart::SQuoted(s) => pieces.push(PatPiece::Lit(s)),
        WordPart::DQuoted(atoms) => {
            for atom in atoms {
                match atom {
                    Atom::Lit(s) => pieces.push(PatPiece::Lit(s)),
                    other => pieces.push(PatPiece::Other(other)),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

/// Emit standalone `(( expr ))` as a fish assignment.
fn emit_standalone_arith(ctx: &mut Ctx, arith: &Arith<'_>, out: &mut String) -> Res<()> {
    let set_kw = if ctx.in_subshell { "set -l " } else { "set " };
    match arith {
        Arith::PostInc(var) | Arith::PreInc(var) => {
            out.push_str(set_kw);
            out.push_str(var);
            out.push_str(" (math \"$");
            out.push_str(var);
            out.push_str(" + 1\")");
            Ok(())
        }
        Arith::PostDec(var) | Arith::PreDec(var) => {
            out.push_str(set_kw);
            out.push_str(var);
            out.push_str(" (math \"$");
            out.push_str(var);
            out.push_str(" - 1\")");
            Ok(())
        }
        Arith::Assign(var, expr) => {
            out.push_str(set_kw);
            out.push_str(var);
            out.push_str(" (math \"");
            emit_arith(expr, out);
            out.push_str("\")");
            Ok(())
        }
        Arith::Lt(..)
        | Arith::Le(..)
        | Arith::Gt(..)
        | Arith::Ge(..)
        | Arith::Eq(..)
        | Arith::Ne(..)
        | Arith::LogAnd(..)
        | Arith::LogOr(..)
        | Arith::LogNot(..) => emit_arith_condition(arith, out),

        _ => Err(TranslateError::Unsupported(
            "unsupported standalone arithmetic expression",
        )),
    }
}

fn emit_arith(arith: &Arith<'_>, out: &mut String) {
    match arith {
        Arith::Var(name) => {
            // Positional parameters: $1 → $argv[1], etc.
            if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                out.push_str("$argv[");
                out.push_str(name);
                out.push(']');
            } else {
                out.push('$');
                out.push_str(name);
            }
        }
        Arith::Lit(n) => {
            itoa(out, *n);
        }

        Arith::Add(l, r) => emit_arith_binop(l, " + ", r, out),
        Arith::Sub(l, r) => emit_arith_binop(l, " - ", r, out),
        Arith::Mul(l, r) => emit_arith_binop(l, " * ", r, out),
        Arith::Div(l, r) => {
            // Bash integer division truncates toward zero; fish math returns float.
            // floor() is correct for positive quotients (the common case).
            // Negative quotients differ: floor(-7/2)=-4 vs bash's -3.  Fish has
            // no trunc(), and negative integer division in interactive shells is rare.
            out.push_str("floor(");
            emit_arith(l, out);
            out.push_str(" / ");
            emit_arith(r, out);
            out.push(')');
        }
        Arith::Rem(l, r) => emit_arith_binop(l, " % ", r, out),
        Arith::Pow(l, r) => emit_arith_binop(l, " ^ ", r, out),
        Arith::Lt(l, r) => emit_arith_binop(l, " < ", r, out),
        Arith::Le(l, r) => emit_arith_binop(l, " <= ", r, out),
        Arith::Gt(l, r) => emit_arith_binop(l, " > ", r, out),
        Arith::Ge(l, r) => emit_arith_binop(l, " >= ", r, out),
        Arith::Eq(l, r) => emit_arith_binop(l, " == ", r, out),
        Arith::Ne(l, r) => emit_arith_binop(l, " != ", r, out),
        Arith::BitAnd(l, r) => {
            out.push_str("bitand(");
            emit_arith(l, out);
            out.push_str(", ");
            emit_arith(r, out);
            out.push(')');
        }
        Arith::BitOr(l, r) => {
            out.push_str("bitor(");
            emit_arith(l, out);
            out.push_str(", ");
            emit_arith(r, out);
            out.push(')');
        }
        Arith::BitXor(l, r) => {
            out.push_str("bitxor(");
            emit_arith(l, out);
            out.push_str(", ");
            emit_arith(r, out);
            out.push(')');
        }
        Arith::LogAnd(l, r) => emit_arith_binop(l, " && ", r, out),
        Arith::LogOr(l, r) => emit_arith_binop(l, " || ", r, out),
        Arith::Shl(l, r) => {
            // fish math doesn't have <<, use: a * 2^n
            out.push('(');
            emit_arith(l, out);
            out.push_str(" * 2 ^ ");
            emit_arith(r, out);
            out.push(')');
        }
        Arith::Shr(l, r) => {
            // fish math doesn't have >>, use: floor(a / 2^n)
            out.push_str("floor(");
            emit_arith(l, out);
            out.push_str(" / 2 ^ ");
            emit_arith(r, out);
            out.push(')');
        }

        Arith::Pos(e) => {
            out.push('+');
            emit_arith(e, out);
        }
        Arith::Neg(e) => {
            out.push('-');
            emit_arith(e, out);
        }
        Arith::LogNot(e) => {
            out.push('!');
            emit_arith(e, out);
        }
        Arith::BitNot(e) => {
            // fish math doesn't have ~, use: bitxor(x, -1) which flips all bits
            out.push_str("bitxor(");
            emit_arith(e, out);
            out.push_str(", -1)");
        }

        Arith::PostInc(var) | Arith::PreInc(var) => {
            out.push_str("($");
            out.push_str(var);
            out.push_str(" + 1)");
        }
        Arith::PostDec(var) | Arith::PreDec(var) => {
            out.push_str("($");
            out.push_str(var);
            out.push_str(" - 1)");
        }

        Arith::Ternary(cond, then_val, else_val) => {
            out.push('(');
            emit_arith(cond, out);
            out.push_str(" ? ");
            emit_arith(then_val, out);
            out.push_str(" : ");
            emit_arith(else_val, out);
            out.push(')');
        }

        Arith::Assign(var, expr) => {
            out.push_str(var);
            out.push_str(" = ");
            emit_arith(expr, out);
        }
    }
}

fn emit_arith_binop(l: &Arith<'_>, op: &str, r: &Arith<'_>, out: &mut String) {
    let l_needs_parens = is_arith_binop(l);
    let r_needs_parens = is_arith_binop(r);

    if l_needs_parens {
        out.push('(');
    }
    emit_arith(l, out);
    if l_needs_parens {
        out.push(')');
    }

    out.push_str(op);

    if r_needs_parens {
        out.push('(');
    }
    emit_arith(r, out);
    if r_needs_parens {
        out.push(')');
    }
}

fn is_arith_binop(arith: &Arith<'_>) -> bool {
    matches!(
        arith,
        Arith::Add(..)
            | Arith::Sub(..)
            | Arith::Mul(..)
            | Arith::Div(..)
            | Arith::Rem(..)
            | Arith::Pow(..)
            | Arith::Lt(..)
            | Arith::Le(..)
            | Arith::Gt(..)
            | Arith::Ge(..)
            | Arith::Eq(..)
            | Arith::Ne(..)
            | Arith::BitAnd(..)
            | Arith::BitOr(..)
            | Arith::BitXor(..)
            | Arith::LogAnd(..)
            | Arith::LogOr(..)
            | Arith::Shl(..)
            | Arith::Shr(..)
    )
}

/// Check if an arithmetic expression contains operations that fish math can't handle.
fn arith_has_unsupported(arith: &Arith<'_>) -> bool {
    match arith {
        Arith::PostInc(..)
        | Arith::PreInc(..)
        | Arith::PostDec(..)
        | Arith::PreDec(..)
        | Arith::Assign(..) => true,

        Arith::Add(l, r)
        | Arith::Sub(l, r)
        | Arith::Mul(l, r)
        | Arith::Div(l, r)
        | Arith::Rem(l, r)
        | Arith::Pow(l, r)
        | Arith::Lt(l, r)
        | Arith::Le(l, r)
        | Arith::Gt(l, r)
        | Arith::Ge(l, r)
        | Arith::Eq(l, r)
        | Arith::Ne(l, r)
        | Arith::LogAnd(l, r)
        | Arith::LogOr(l, r)
        | Arith::BitAnd(l, r)
        | Arith::BitOr(l, r)
        | Arith::BitXor(l, r)
        | Arith::Shl(l, r)
        | Arith::Shr(l, r) => arith_has_unsupported(l) || arith_has_unsupported(r),

        Arith::Pos(e) | Arith::Neg(e) | Arith::LogNot(e) | Arith::BitNot(e) => {
            arith_has_unsupported(e)
        }

        Arith::Ternary(c, t, f) => {
            arith_has_unsupported(c) || arith_has_unsupported(t) || arith_has_unsupported(f)
        }

        Arith::Var(_) | Arith::Lit(_) => false,
    }
}

/// Check if an arithmetic expression requires test-based evaluation.
fn arith_needs_test(arith: &Arith<'_>) -> bool {
    matches!(
        arith,
        Arith::Lt(..)
            | Arith::Le(..)
            | Arith::Gt(..)
            | Arith::Ge(..)
            | Arith::Eq(..)
            | Arith::Ne(..)
            | Arith::LogAnd(..)
            | Arith::LogOr(..)
            | Arith::LogNot(..)
            | Arith::Ternary(..)
    )
}

fn emit_arith_as_command(arith: &Arith<'_>, out: &mut String) -> Res<()> {
    if let Arith::Ternary(cond, then_val, else_val) = arith {
        out.push_str("(if ");
        emit_arith_condition(cond, out)?;
        out.push_str("; echo ");
        emit_arith_value(then_val, out)?;
        out.push_str("; else; echo ");
        emit_arith_value(else_val, out)?;
        out.push_str("; end)");
    } else {
        out.push('(');
        emit_arith_condition(arith, out)?;
        out.push_str("; and echo 1; or echo 0)");
    }
    Ok(())
}

fn emit_arith_condition(arith: &Arith<'_>, out: &mut String) -> Res<()> {
    match arith {
        Arith::Lt(l, r) => emit_test_cmp(l, "-lt", r, out),
        Arith::Le(l, r) => emit_test_cmp(l, "-le", r, out),
        Arith::Gt(l, r) => emit_test_cmp(l, "-gt", r, out),
        Arith::Ge(l, r) => emit_test_cmp(l, "-ge", r, out),
        Arith::Eq(l, r) => emit_test_cmp(l, "-eq", r, out),
        Arith::Ne(l, r) => emit_test_cmp(l, "-ne", r, out),
        Arith::LogAnd(l, r) => {
            emit_arith_condition(l, out)?;
            out.push_str("; and ");
            emit_arith_condition(r, out)
        }
        Arith::LogOr(l, r) => {
            emit_arith_condition(l, out)?;
            out.push_str("; or ");
            emit_arith_condition(r, out)
        }
        Arith::LogNot(e) => {
            out.push_str("not ");
            emit_arith_condition(e, out)
        }
        _ => {
            out.push_str("test ");
            emit_arith_value(arith, out)?;
            out.push_str(" -ne 0");
            Ok(())
        }
    }
}

fn emit_test_cmp(
    l: &Arith<'_>,
    op: &str,
    r: &Arith<'_>,
    out: &mut String,
) -> Res<()> {
    out.push_str("test ");
    emit_arith_value(l, out)?;
    out.push(' ');
    out.push_str(op);
    out.push(' ');
    emit_arith_value(r, out)
}

fn emit_arith_value(arith: &Arith<'_>, out: &mut String) -> Res<()> {
    match arith {
        Arith::Var(name) => {
            out.push('$');
            out.push_str(name);
            Ok(())
        }
        Arith::Lit(n) => {
            itoa(out, *n);
            Ok(())
        }
        _ if arith_needs_test(arith) => emit_arith_as_command(arith, out),
        _ => {
            out.push_str("(math \"");
            emit_arith(arith, out);
            out.push_str("\")");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------------

fn emit_redirects(ctx: &mut Ctx, redirects: &[&Redir<'_>], out: &mut String) -> Res<()> {
    for redir in redirects {
        out.push(' ');
        emit_redir(ctx, redir, out)?;
    }
    Ok(())
}

fn emit_redir(ctx: &mut Ctx, redir: &Redir<'_>, out: &mut String) -> Res<()> {
    fn write_fd(fd: Option<u16>, out: &mut String) {
        if let Some(n) = fd {
            itoa(out, i64::from(n));
        }
    }

    match redir {
        Redir::Read(fd, word) => {
            write_fd(*fd, out);
            out.push('<');
            emit_word(ctx, word, out)?;
        }
        Redir::Write(fd, word) => {
            write_fd(*fd, out);
            out.push('>');
            emit_word(ctx, word, out)?;
        }
        Redir::Append(fd, word) => {
            write_fd(*fd, out);
            out.push_str(">>");
            emit_word(ctx, word, out)?;
        }
        Redir::ReadWrite(fd, word) => {
            write_fd(*fd, out);
            out.push_str("<>");
            emit_word(ctx, word, out)?;
        }
        Redir::Clobber(fd, word) => {
            write_fd(*fd, out);
            out.push_str(">|");
            emit_word(ctx, word, out)?;
        }
        Redir::DupRead(fd, word) => {
            write_fd(*fd, out);
            out.push_str("<&");
            emit_word(ctx, word, out)?;
        }
        Redir::DupWrite(fd, word) => {
            write_fd(*fd, out);
            out.push_str(">&");
            emit_word(ctx, word, out)?;
        }
        Redir::HereString(_) | Redir::Heredoc(_) => {
            // Handled at a higher level (emit_simple / emit_compound)
        }
        Redir::WriteAll(word) => {
            out.push('>');
            emit_word(ctx, word, out)?;
            out.push_str(" 2>&1");
        }
        Redir::AppendAll(word) => {
            out.push_str(">>");
            emit_word(ctx, word, out)?;
            out.push_str(" 2>&1");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the repeated character from a printf format like `%0.s-` or `%.0s-`.
/// Returns Some(char) if the format is a repetition pattern.
fn extract_printf_repeat_char(fmt: &str) -> Option<char> {
    // Match patterns: "%0.sCHAR", "%.0sCHAR", "%0.0sCHAR" etc.
    let stripped = fmt.strip_prefix('%')?;
    // Find the 's' after digits and dots
    let s_pos = stripped.find('s')?;
    let before_s = &stripped[..s_pos];
    // Validate it's a zero-width format: "0.", ".0", "0.0", etc.
    if before_s.contains('0') && (before_s.contains('.') || before_s == "0") {
        let after_s = &stripped[s_pos + 1..];
        after_s.as_bytes().first().map(|&b| b as char)
    } else {
        None
    }
}

/// Extract a count from brace range arguments like {1..N}.
/// Checks if the remaining args form a brace range and returns the count.
fn extract_brace_range_count(args: &[&Word<'_>]) -> Option<i64> {
    // Look for BraceRange atom in the word
    for arg in args {
        if let Word::Simple(WordPart::Bare(Atom::BraceRange { start, end, step })) = arg {
            let s: i64 = start.parse().ok()?;
            let e: i64 = end.parse().ok()?;
            let st: i64 = step.and_then(|s| s.parse().ok()).unwrap_or(1);
            if st == 0 {
                return None;
            }
            let count = ((e - s).abs() / st.abs()) + 1;
            return Some(count);
        }
    }
    None
}

/// Extract commands from a `ProcSubIn` atom inside a word.
fn extract_procsub_cmds<'a>(word: &'a Word<'a>) -> Option<&'a Vec<Cmd<'a>>> {
    match word {
        Word::Simple(WordPart::Bare(Atom::ProcSubIn(cmds))) => Some(cmds),
        _ => None,
    }
}

fn word_as_str<'a>(word: &'a Word<'a>) -> Option<Cow<'a, str>> {
    if let Word::Simple(WordPart::Bare(Atom::Lit(s)) | WordPart::SQuoted(s)) = word {
        return Some(Cow::Borrowed(s));
    }
    let mut buf = String::with_capacity(64);
    if word_to_simple_string(word, &mut buf) {
        Some(Cow::Owned(buf))
    } else {
        None
    }
}

#[inline]
fn word_has_glob(word: &Word<'_>) -> bool {
    match word {
        Word::Simple(p) => part_has_glob(p),
        Word::Concat(parts) => parts.iter().any(part_has_glob),
    }
}

#[inline]
fn part_has_glob(part: &WordPart<'_>) -> bool {
    match part {
        WordPart::Bare(atom) => matches!(atom, Atom::Star | Atom::Question),
        WordPart::DQuoted(atoms) => atoms
            .iter()
            .any(|a| matches!(a, Atom::Star | Atom::Question)),
        WordPart::SQuoted(_) => false,
    }
}

fn word_to_simple_string(word: &Word<'_>, out: &mut String) -> bool {
    match word {
        Word::Simple(p) => part_to_string(p, out),
        Word::Concat(parts) => {
            for p in parts {
                if !part_to_string(p, out) {
                    return false;
                }
            }
            true
        }
    }
}

fn part_to_string(part: &WordPart<'_>, out: &mut String) -> bool {
    match part {
        WordPart::Bare(atom) => atom_to_string(atom, out),
        WordPart::SQuoted(s) => {
            out.push_str(s);
            true
        }
        WordPart::DQuoted(atoms) => {
            for atom in atoms {
                if !atom_to_string(atom, out) {
                    return false;
                }
            }
            true
        }
    }
}

fn atom_to_string(atom: &Atom<'_>, out: &mut String) -> bool {
    match atom {
        Atom::Lit(s) => {
            out.push_str(s);
            true
        }
        Atom::Escaped(s) => {
            out.push_str(s);
            true
        }
        Atom::SquareOpen => {
            out.push('[');
            true
        }
        Atom::SquareClose => {
            out.push(']');
            true
        }
        Atom::Tilde => {
            out.push('~');
            true
        }
        Atom::Star => {
            out.push('*');
            true
        }
        Atom::Question => {
            out.push('?');
            true
        }
        _ => false,
    }
}

#[inline]
fn itoa(out: &mut String, n: impl std::fmt::Display) {
    use std::fmt::Write;
    let _ = write!(out, "{n}");
}

/// Push `s` into `out` wrapped in single quotes, escaping internal `'` chars.
/// Writes directly — no intermediate String allocation.
fn push_sq_escaped(out: &mut String, s: &str) {
    out.push('\'');
    for b in s.bytes() {
        if b == b'\'' {
            out.push_str("'\\''");
        } else {
            out.push(b as char);
        }
    }
    out.push('\'');
}

/// Emit a heredoc body. Literal bodies use single quotes, interpolated bodies
/// use double quotes with variable/command expansion.
fn emit_heredoc_body(ctx: &mut Ctx, body: &HeredocBody<'_>, out: &mut String) -> Res<()> {
    match body {
        HeredocBody::Literal(text) => {
            out.push_str("printf '%s\\n' ");
            push_sq_escaped(out, text.strip_suffix('\n').unwrap_or(text));
            Ok(())
        }
        HeredocBody::Interpolated(atoms) => {
            // Build the body content with literal newlines (fish double quotes
            // support embedded newlines), then strip the trailing newline.
            let mut body_str = String::with_capacity(256);
            for atom in atoms {
                match atom {
                    Atom::Lit(s) => {
                        for &b in s.as_bytes() {
                            match b {
                                b'"' => body_str.push_str("\\\""),
                                b'\\' => body_str.push_str("\\\\"),
                                b'$' => body_str.push_str("\\$"),
                                _ => body_str.push(b as char),
                            }
                        }
                    }
                    Atom::Escaped(s) => {
                        // \$ → literal $, \\ → literal \, \` → literal `
                        match s.as_ref() {
                            "$" => body_str.push('$'),
                            "\\" => body_str.push_str("\\\\"),
                            "`" => body_str.push('`'),
                            _ => body_str.push_str(s),
                        }
                    }
                    Atom::Param(param) => emit_param(param, &mut body_str),
                    Atom::Subst(subst) => {
                        body_str.push('"');
                        emit_subst(ctx, subst, &mut body_str)?;
                        body_str.push('"');
                    }
                    _ => emit_atom(ctx, atom, &mut body_str)?,
                }
            }
            // Strip trailing newline (the one before the delimiter line)
            let trimmed = body_str.strip_suffix('\n').unwrap_or(&body_str);
            out.push_str("printf '%s\\n' \"");
            out.push_str(trimmed);
            out.push('"');
            Ok(())
        }
    }
}

fn emit_param_name(param: &Param<'_>, out: &mut String) {
    match param {
        Param::Var("HOSTNAME") => out.push_str("hostname"),
        Param::Var("PIPESTATUS") => out.push_str("pipestatus"),
        Param::Var(name) => out.push_str(name),
        Param::Positional(n) => {
            out.push_str("argv[");
            itoa(out, i64::from(*n));
            out.push(']');
        }
        Param::At | Param::Star => out.push_str("argv"),
        Param::Pound => out.push_str("ARGC"),
        Param::Status => out.push_str("status"),
        Param::Pid => out.push_str("fish_pid"),
        Param::Bang => out.push_str("last_pid"),
        Param::Dash => out.push_str("FISH_FLAGS"),
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

    fn t_unsupported(bash: &str) {
        assert!(matches!(translate_bash_to_fish(bash), Err(TranslateError::Unsupported(_))));
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
        // Prefix assignments with a command bail to bash passthrough
        t_unsupported("FOO=bar command");
    }

    // --- Export ---

    #[test]
    fn export_simple() {
        assert_eq!(t("export EDITOR=vim"), "set -gx EDITOR vim");
    }

    #[test]
    fn export_path_splits_colons() {
        assert_eq!(
            t("export PATH=/usr/bin:$PATH"),
            "set -gx PATH /usr/bin $PATH"
        );
        assert_eq!(
            t("export PATH=$HOME/bin:/usr/local/bin:$PATH"),
            "set -gx PATH $HOME/bin /usr/local/bin $PATH"
        );
        // Non-PATH variable should NOT split on colons
        assert_eq!(
            t("export FOO=a:b:c"),
            "set -gx FOO a:b:c"
        );
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

    #[test]
    fn for_loop_with_glob() {
        let result = t("for f in *.txt; do echo $f; done");
        assert!(result.contains("for f in *.txt"));
        assert!(result.contains("echo $f"));
    }

    #[test]
    fn for_loop_bare_var_gets_split() {
        let result = t(r#"files="a b c"; for f in $files; do echo $f; done"#);
        assert!(result.contains("(string split -n -- ' ' $files)"));
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
        assert!(
            result.contains(r#""result is ""#),
            "outer quotes should close before math, got: {}",
            result
        );
    }

    // --- Nested control structures ---

    #[test]
    fn nested_for_if() {
        let result = t("for f in $(ls); do if test -f $f; then echo $f is a file; fi; done");
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
        // Prefix assignments with a command bail to bash passthrough
        t_unsupported("CC=gcc CXX=g++ make");
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
    fn ansi_c_quoting_simple() {
        assert_eq!(t("echo $'hello'"), "echo \"hello\"");
    }

    #[test]
    fn ansi_c_quoting_newline() {
        // \n must be emitted bare (outside quotes) for fish to interpret it
        assert_eq!(t("echo $'line1\\nline2'"), "echo \"line1\"\\n\"line2\"");
    }

    #[test]
    fn ansi_c_quoting_tab() {
        assert_eq!(t("echo $'a\\tb'"), "echo \"a\"\\t\"b\"");
    }

    #[test]
    fn ansi_c_quoting_escaped_squote() {
        assert_eq!(t("echo $'it\\'s'"), "echo \"it's\"");
    }

    #[test]
    fn ansi_c_quoting_escape_e() {
        assert_eq!(t("echo $'\\E[31m'"), "echo \\e\"[31m\"");
    }

    #[test]
    fn ansi_c_quoting_dollar() {
        assert_eq!(t("echo $'costs $5'"), "echo \"costs \\$5\"");
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
        let result = t("(cd /tmp && ls)");
        assert!(result.contains("begin\n"));
        assert!(result.contains("cd /tmp; and ls"));
        assert!(result.contains("set -l __reef_pwd (pwd)"));
        assert!(result.contains("cd $__reef_pwd 2>/dev/null"));
        assert!(result.contains("\nend"));
    }

    #[test]
    fn subshell_pipeline() {
        let result = t("(echo hello; echo world)");
        assert!(result.contains("begin\n"));
        assert!(result.contains("echo hello\necho world"));
        assert!(result.contains("\nend"));
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
        // `cmd` is an older form of $(cmd) — parser handles both
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
    fn dollar_random() {
        assert_eq!(t("echo $RANDOM"), "echo (random)");
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
        // [[ ]] is bash-specific — translate to test and strip ]]
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
        assert!(result.contains("string match -q -- 'w*'"), "got: {}", result);
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
        assert!(
            result.contains("; and test -r /etc/hostname"),
            "got: {}",
            result
        );
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
        assert!(result.contains("string match -r"), "got: {}", result);
        assert!(result.contains("__bash_rematch"), "got: {}", result);
        assert!(result.contains("'^[a-z]+$' \"$str\""), "got: {}", result);
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
        assert!(
            result.contains("echo \"hello $name\" | grep"),
            "got: {}",
            result
        );
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
        let result = t("for i in 1 2 3; do for j in a b c; do echo $i$j; done; done");
        assert!(result.contains("for i in 1 2 3"));
        assert!(result.contains("for j in a b c"));
        assert!(result.contains("echo $i$j"));
        // Two end keywords for the two loops
        let end_count = result.matches("end").count();
        assert!(
            end_count >= 2,
            "Expected at least 2 'end' keywords, got {}",
            end_count
        );
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

    // --- Standalone (( )) arithmetic ---

    #[test]
    fn standalone_arith_post_increment() {
        let result = t("(( i++ ))");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("+ 1"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_pre_increment() {
        let result = t("(( ++i ))");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("+ 1"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_post_decrement() {
        let result = t("(( i-- ))");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("- 1"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_pre_decrement() {
        let result = t("(( --i ))");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("- 1"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_plus_equals() {
        let result = t("(( count += 5 ))");
        assert!(result.contains("set count"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("+ 5"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_minus_equals() {
        let result = t("(( x -= 3 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("- 3"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_times_equals() {
        let result = t("(( x *= 2 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("* 2"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_div_equals() {
        let result = t("(( x /= 4 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("/ 4"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_mod_equals() {
        let result = t("(( x %= 3 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("% 3"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_simple_assign() {
        let result = t("(( x = 42 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("42"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_assign_expr() {
        let result = t("(( x = y + 1 ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_in_loop() {
        // Common pattern: while loop with counter
        let result = t("while test $i -lt 10; do echo $i; (( i++ )); done");
        assert!(result.contains("while test $i -lt 10"), "got: {}", result);
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("+ 1"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_comparison() {
        assert_eq!(t("(( x > 5 ))"), "test $x -gt 5");
    }

    #[test]
    fn standalone_arith_comparison_eq() {
        assert_eq!(t("(( x == 0 ))"), "test $x -eq 0");
    }

    #[test]
    fn standalone_arith_logical_and() {
        assert_eq!(
            t("(( x > 0 && y < 10 ))"),
            "test $x -gt 0; and test $y -lt 10"
        );
    }

    #[test]
    fn cstyle_for_loop() {
        let result = t("for (( i=0; i<10; i++ )); do echo $i; done");
        assert!(result.contains("set i (math \"0\")"), "got: {}", result);
        assert!(result.contains("while test $i -lt 10"), "got: {}", result);
        assert!(result.contains("echo $i"), "got: {}", result);
        assert!(
            result.contains("set i (math \"$i + 1\")"),
            "got: {}",
            result
        );
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_in_quotes_untouched() {
        // (( )) inside quotes should not be rewritten
        let result = t("echo '(( i++ ))'");
        assert!(result.contains("(( i++ ))"), "got: {}", result);
    }

    // --- Comprehensive arithmetic $((…)) ---

    #[test]
    fn arith_subtraction() {
        let result = t("echo $((10 - 3))");
        assert_eq!(result, r#"echo (math "10 - 3")"#);
    }

    #[test]
    fn arith_division() {
        let result = t("echo $((20 / 4))");
        assert_eq!(result, r#"echo (math "floor(20 / 4)")"#);
    }

    #[test]
    fn arith_power() {
        let result = t("echo $((2 ** 10))");
        assert_eq!(result, r#"echo (math "2 ^ 10")"#);
    }

    #[test]
    fn arith_nested_parens() {
        let result = t("echo $(( (2 + 3) * (4 - 1) ))");
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("(2 + 3) * (4 - 1)"), "got: {}", result);
    }

    #[test]
    fn arith_unary_neg() {
        let result = t("echo $((-x + 5))");
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("-$x"), "got: {}", result);
    }

    #[test]
    fn arith_variables_only() {
        let result = t("echo $((a + b * c))");
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("$a + ($b * $c)"), "got: {}", result);
    }

    #[test]
    fn arith_comparison_eq() {
        let result = t("echo $((x == y))");
        assert!(result.contains("test $x -eq $y"), "got: {}", result);
    }

    #[test]
    fn arith_comparison_ne() {
        let result = t("echo $((x != y))");
        assert!(result.contains("test $x -ne $y"), "got: {}", result);
    }

    #[test]
    fn arith_comparison_le() {
        let result = t("echo $((a <= b))");
        assert!(result.contains("test $a -le $b"), "got: {}", result);
    }

    #[test]
    fn arith_comparison_ge() {
        let result = t("echo $((a >= b))");
        assert!(result.contains("test $a -ge $b"), "got: {}", result);
    }

    #[test]
    fn arith_comparison_lt() {
        let result = t("echo $((a < b))");
        assert!(result.contains("test $a -lt $b"), "got: {}", result);
    }

    #[test]
    fn arith_logic_and() {
        let result = t("echo $((a > 0 && b > 0))");
        assert!(result.contains("test $a -gt 0"), "got: {}", result);
        assert!(result.contains("; and "), "got: {}", result);
        assert!(result.contains("test $b -gt 0"), "got: {}", result);
    }

    #[test]
    fn arith_logic_or() {
        let result = t("echo $((a == 0 || b == 0))");
        assert!(result.contains("test $a -eq 0"), "got: {}", result);
        assert!(result.contains("; or "), "got: {}", result);
    }

    #[test]
    fn arith_logic_not() {
        let result = t("echo $((!x))");
        assert!(result.contains("not "), "got: {}", result);
    }

    #[test]
    fn arith_ternary_with_math() {
        let result = t("echo $((x > 0 ? x * 2 : 0))");
        assert!(result.contains("if test $x -gt 0"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn arith_in_assignment() {
        let result = t("z=$((x + y))");
        assert!(result.contains("set z"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn arith_in_condition() {
        let result = t("if [ $((x % 2)) -eq 0 ]; then echo even; fi");
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("echo even"), "got: {}", result);
    }

    #[test]
    fn arith_multiple_in_line() {
        let result = t("echo $((a + 1)) $((b + 2))");
        assert!(result.contains(r#"(math "$a + 1")"#), "got: {}", result);
        assert!(result.contains(r#"(math "$b + 2")"#), "got: {}", result);
    }

    #[test]
    fn arith_deeply_nested() {
        let result = t("echo $(( ((2 + 3)) * ((4 + 5)) ))");
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn arith_empty() {
        // $(()) is valid bash, evaluates to 0
        let result = t("echo $(())");
        assert!(result.contains("echo"), "got: {}", result);
    }

    #[test]
    fn arith_complex_expression() {
        let result = t("echo $(( (x + y) / 2 - z * 3 ))");
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("/ 2"), "got: {}", result);
    }

    #[test]
    fn arith_in_export() {
        let result = t("export N=$((x + 1))");
        assert!(result.contains("set -gx N"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn arith_in_local() {
        let result = t("local result=$((a * b))");
        assert!(result.contains("set -l result"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    // --- Standalone (( )) with compound expressions ---

    #[test]
    fn standalone_arith_assign_compound() {
        let result = t("(( total = x + y * 2 ))");
        assert!(result.contains("set total"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
        assert!(result.contains("$x + ($y * 2)"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_nested_assign() {
        let result = t("(( x = (a + b) * c ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("math"), "got: {}", result);
    }

    #[test]
    fn standalone_arith_multiple_in_sequence() {
        let result = t("(( x++ )); (( y-- ))");
        assert!(result.contains("set x"), "got: {}", result);
        assert!(result.contains("set y"), "got: {}", result);
        assert!(result.contains("+ 1"), "got: {}", result);
        assert!(result.contains("- 1"), "got: {}", result);
    }

    // --- Case modification ---

    #[test]
    fn upper_all() {
        let result = t("echo ${var^^}");
        assert_eq!(result, "echo (string upper -- \"$var\")");
    }

    #[test]
    fn lower_all() {
        let result = t("echo ${var,,}");
        assert_eq!(result, "echo (string lower -- \"$var\")");
    }

    #[test]
    fn upper_first() {
        let result = t("echo ${var^}");
        assert!(result.contains("string sub -l 1"));
        assert!(result.contains("string upper"));
        assert!(result.contains("string sub -s 2"));
    }

    #[test]
    fn lower_first() {
        let result = t("echo ${var,}");
        assert!(result.contains("string sub -l 1"));
        assert!(result.contains("string lower"));
        assert!(result.contains("string sub -s 2"));
    }

    // --- Pattern replacement ---

    #[test]
    fn replace_first() {
        let result = t("echo ${var/foo/bar}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("'foo'"), "got: {}", result);
        assert!(result.contains("'bar'"), "got: {}", result);
        assert!(result.contains("$var"), "got: {}", result);
    }

    #[test]
    fn replace_all() {
        let result = t("echo ${var//foo/bar}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("-a"), "got: {}", result);
    }

    #[test]
    fn replace_prefix() {
        let result = t("echo ${var/#foo/bar}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("-r"), "got: {}", result);
        assert!(result.contains("'^foo'"), "got: {}", result);
    }

    #[test]
    fn replace_suffix() {
        let result = t("echo ${var/%foo/bar}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("-r"), "got: {}", result);
        assert!(result.contains("'foo$'"), "got: {}", result);
    }

    #[test]
    fn replace_delete() {
        let result = t("echo ${var/foo}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("-- 'foo' '' \"$var\""), "got: {}", result);
    }

    // --- Substring ---

    #[test]
    fn substring_offset_only() {
        let result = t("echo ${var:2}");
        assert!(result.contains("string sub"), "got: {}", result);
        assert!(result.contains("-s (math \"2 + 1\")"), "got: {}", result);
        assert!(result.contains("$var"), "got: {}", result);
    }

    #[test]
    fn substring_offset_and_length() {
        let result = t("echo ${var:2:5}");
        assert!(result.contains("string sub"), "got: {}", result);
        assert!(result.contains("-s (math \"2 + 1\")"), "got: {}", result);
        assert!(result.contains("-l (math \"5\")"), "got: {}", result);
    }

    // --- Process substitution ---

    #[test]
    fn process_substitution_in() {
        let result = t("diff <(sort a) <(sort b)");
        assert!(result.contains("(sort a | psub)"), "got: {}", result);
        assert!(result.contains("(sort b | psub)"), "got: {}", result);
    }

    #[test]
    fn process_substitution_out_unsupported() {
        let result = translate_bash_to_fish("tee >(grep foo)");
        assert!(result.is_err());
    }

    // --- C-style for ---

    #[test]
    fn cstyle_for_no_init() {
        let result = t("for (( ; i<5; i++ )); do echo $i; done");
        assert!(result.contains("while test $i -lt 5"), "got: {}", result);
        assert!(
            result.contains("set i (math \"$i + 1\")"),
            "got: {}",
            result
        );
    }

    #[test]
    fn cstyle_for_no_step() {
        let result = t("for (( i=0; i<5; )); do echo $i; done");
        assert!(result.contains("set i (math \"0\")"), "got: {}", result);
        assert!(result.contains("while test $i -lt 5"), "got: {}", result);
    }

    // --- Heredoc ---

    #[test]
    fn heredoc_quoted() {
        let result = t("cat <<'EOF'\nhello world\nEOF");
        assert!(result.contains("printf"), "got: {}", result);
        assert!(result.contains("hello world"), "got: {}", result);
        assert!(result.contains("| cat"), "got: {}", result);
    }

    #[test]
    fn heredoc_double_quoted() {
        let result = t("cat <<\"EOF\"\nhello world\nEOF");
        assert!(result.contains("printf"), "got: {}", result);
        assert!(result.contains("| cat"), "got: {}", result);
    }

    #[test]
    fn heredoc_unquoted() {
        let result = t("cat <<EOF\nhello $NAME\nEOF");
        assert!(result.contains("printf"), "got: {}", result);
        assert!(result.contains("$NAME"), "got: {}", result);
        assert!(result.contains("| cat"), "got: {}", result);
    }

    // --- Case fallthrough errors ---

    #[test]
    fn case_fallthrough_error() {
        let result = translate_bash_to_fish("case $x in a) echo a;& b) echo b;; esac");
        assert!(result.is_err());
    }

    #[test]
    fn case_continue_error() {
        let result = translate_bash_to_fish("case $x in a) echo a;;& b) echo b;; esac");
        assert!(result.is_err());
    }

    // --- Arrays ---

    #[test]
    fn array_assign() {
        let result = t("arr=(one two three)");
        assert_eq!(result, "set arr one two three");
    }

    #[test]
    fn array_element_access() {
        let result = t("echo ${arr[1]}");
        assert!(result.contains("$arr[2]"), "got: {}", result);
    }

    #[test]
    fn array_all() {
        let result = t("echo ${arr[@]}");
        assert!(result.contains("$arr"), "got: {}", result);
    }

    #[test]
    fn array_length() {
        let result = t("echo ${#arr[@]}");
        assert!(result.contains("(count $arr)"), "got: {}", result);
    }

    #[test]
    fn array_append() {
        let result = t("arr+=(three)");
        assert_eq!(result, "set -a arr three");
    }

    #[test]
    fn array_slice() {
        let result = t("echo ${arr[@]:1:3}");
        assert!(result.contains("$arr["), "got: {}", result);
    }

    // --- Trap ---

    #[test]
    fn trap_exit() {
        assert_eq!(
            t("trap 'echo bye' EXIT"),
            "function __reef_trap_EXIT --on-event fish_exit\necho bye\nend"
        );
    }

    #[test]
    fn trap_signal() {
        assert_eq!(
            t("trap 'cleanup' INT"),
            "function __reef_trap_INT --on-signal INT\ncleanup\nend"
        );
    }

    #[test]
    fn trap_sigprefix() {
        assert_eq!(
            t("trap 'cleanup' SIGTERM"),
            "function __reef_trap_TERM --on-signal TERM\ncleanup\nend"
        );
    }

    #[test]
    fn trap_reset() {
        assert_eq!(t("trap - INT"), "functions -e __reef_trap_INT");
    }

    #[test]
    fn trap_ignore() {
        assert_eq!(
            t("trap '' INT"),
            "function __reef_trap_INT --on-signal INT; end"
        );
    }

    #[test]
    fn trap_multiple_signals() {
        let result = t("trap 'cleanup' INT TERM");
        assert!(result.contains("__reef_trap_INT --on-signal INT"));
        assert!(result.contains("__reef_trap_TERM --on-signal TERM"));
    }

    // --- declare -p ---

    #[test]
    fn declare_print() {
        assert_eq!(t("declare -p FOO"), "set --show FOO");
    }

    #[test]
    fn declare_print_all() {
        assert_eq!(t("declare -p"), "set --show");
    }

    #[test]
    fn declare_print_multiple() {
        let result = t("declare -p FOO BAR");
        assert!(result.contains("set --show FOO"), "got: {}", result);
        assert!(result.contains("set --show BAR"), "got: {}", result);
    }

    // --- ${!prefix*} ---

    #[test]
    fn prefix_list() {
        assert_eq!(
            t("echo ${!BASH_*}"),
            "echo (set -n | string match 'BASH_*')"
        );
    }

    #[test]
    fn prefix_list_at() {
        assert_eq!(t("echo ${!MY@}"), "echo (set -n | string match 'MY*')");
    }

    // --- set -e/u/x ---

    #[test]
    fn bash_set_errexit() {
        let result = t("set -e");
        assert!(result.contains("# set -e"), "got: {}", result);
        assert!(result.contains("no fish equivalent"), "got: {}", result);
    }

    #[test]
    fn bash_set_eux() {
        let result = t("set -eux");
        assert!(result.contains("# set -eux"), "got: {}", result);
    }

    #[test]
    fn bash_set_positional() {
        assert_eq!(t("set -- a b c"), "set argv a b c");
    }

    // --- select / getopts / exec fd / eval ---

    #[test]
    fn select_unsupported() {
        assert!(translate_bash_to_fish("select opt in a b c; do echo $opt; done").is_err());
    }

    #[test]
    fn getopts_unsupported() {
        assert!(translate_bash_to_fish("getopts 'abc' opt").is_err());
    }

    #[test]
    fn exec_fd_unsupported() {
        assert!(translate_bash_to_fish("exec 3>&1").is_err());
    }

    #[test]
    fn eval_cmd_subst() {
        assert_eq!(
            t("eval \"$(pyenv init --path)\""),
            "pyenv init --path | source"
        );
    }

    #[test]
    fn eval_dynamic_unsupported() {
        assert!(translate_bash_to_fish("eval $cmd").is_err());
    }

    // --- Untranslatable variables ---

    #[test]
    fn lineno_unsupported() {
        assert!(translate_bash_to_fish("echo $LINENO").is_err());
    }

    #[test]
    fn funcname_unsupported() {
        assert!(translate_bash_to_fish("echo $FUNCNAME").is_err());
    }

    #[test]
    fn seconds_unsupported() {
        assert!(translate_bash_to_fish("echo $SECONDS").is_err());
    }

    // --- @E/@A transformations unsupported ---

    #[test]
    fn transform_e_unsupported() {
        assert!(translate_bash_to_fish("echo ${var@E}").is_err());
    }

    #[test]
    fn transform_a_unsupported() {
        assert!(translate_bash_to_fish("echo ${var@A}").is_err());
    }

    // --- Regression: previously fixed bugs ---

    #[test]
    fn negation_double_bracket_glob() {
        let result = t(r#"[[ ! "hello" == w* ]]"#);
        assert!(result.contains("not "), "should negate: got: {}", result);
        assert!(!result.contains(r#"\!"#), "should not escape !: got: {}", result);
    }

    #[test]
    fn negation_double_bracket_string() {
        let result = t(r#"[[ ! "$x" == "yes" ]]"#);
        assert!(
            result.contains("not ") || result.contains("!="),
            "should negate: got: {}",
            result
        );
    }

    #[test]
    fn negation_double_bracket_test_flag() {
        let result = t(r#"[[ ! -z "$var" ]]"#);
        assert!(result.contains("not test"), "should negate: got: {}", result);
    }

    #[test]
    fn integer_division_truncates() {
        let result = t("echo $((10 / 3))");
        assert!(result.contains("floor(10 / 3)"), "got: {}", result);
    }

    #[test]
    fn integer_division_exact() {
        let result = t("echo $((20 / 4))");
        assert!(result.contains("floor(20 / 4)"), "got: {}", result);
    }

    #[test]
    fn path_colon_splitting() {
        let result = t("export PATH=/usr/local/bin:/usr/bin:$PATH");
        assert!(
            !result.contains(':'),
            "colons should be split: got: {}",
            result
        );
        assert!(result.contains("/usr/local/bin /usr/bin"), "got: {}", result);
    }

    #[test]
    fn manpath_colon_splitting() {
        let result = t("export MANPATH=/usr/share/man:/usr/local/man");
        assert!(
            result.contains("/usr/share/man /usr/local/man"),
            "got: {}",
            result
        );
    }

    #[test]
    fn non_path_var_keeps_colons() {
        assert_eq!(t("export FOO=a:b:c"), "set -gx FOO a:b:c");
    }

    #[test]
    fn prefix_assignment_bails_to_t2() {
        assert!(translate_bash_to_fish("IFS=: read -ra parts").is_err());
    }

    #[test]
    fn subshell_exit_bails_to_t2() {
        assert!(translate_bash_to_fish("(exit 1)").is_err());
    }

    // --- Complex real-world translations ---

    #[test]
    fn translate_if_dir_exists() {
        let result = t("if [ -d /tmp ]; then echo exists; else echo nope; fi");
        assert!(result.contains("[ -d /tmp ]"), "got: {}", result);
        assert!(result.contains("else"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn translate_for_glob() {
        let result = t("for f in *.txt; do echo $f; done");
        assert!(result.contains("for f in *.txt"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn translate_while_read() {
        let result = t("while read -r line; do echo $line; done < /tmp/input");
        assert!(result.contains("while read"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn translate_command_in_string() {
        let result = t(r#"echo "Hello $USER, you are in $(pwd)""#);
        assert!(result.contains("$USER"), "got: {}", result);
        assert!(result.contains("(pwd)"), "got: {}", result);
    }

    #[test]
    fn translate_test_and_or() {
        let result = t("test -f /etc/passwd && echo found || echo missing");
        assert!(result.contains("test -f /etc/passwd"), "got: {}", result);
        assert!(result.contains("; and echo found"), "got: {}", result);
        assert!(result.contains("; or echo missing"), "got: {}", result);
    }

    #[test]
    fn translate_chained_commands() {
        let result = t("mkdir -p /tmp/test && cd /tmp/test && touch file.txt");
        assert!(result.contains("mkdir -p /tmp/test"), "got: {}", result);
        assert!(result.contains("cd /tmp/test"), "got: {}", result);
    }

    #[test]
    fn translate_pipeline() {
        let result = t("cat file.txt | grep pattern | sort | uniq -c");
        assert!(result.contains("cat file.txt | grep pattern | sort | uniq -c"), "got: {}", result);
    }

    #[test]
    fn translate_home_expansion() {
        let result = t("echo ${HOME}/documents");
        assert!(result.contains("$HOME"), "got: {}", result);
        assert!(result.contains("/documents"), "got: {}", result);
    }

    #[test]
    fn translate_command_v() {
        let result = t("command -v git > /dev/null 2>&1 && echo installed");
        assert!(result.contains("command -v git"), "got: {}", result);
    }

    #[test]
    fn translate_regex_match() {
        let result = t(r#"[[ "$x" =~ ^[0-9]+$ ]]"#);
        assert!(result.contains("string match -r"), "got: {}", result);
        assert!(result.contains("^[0-9]+$"), "got: {}", result);
    }

    // --- C-style for edge cases ---

    #[test]
    fn cstyle_for_decrementing() {
        let result = t("for ((i=10; i>0; i--)); do echo $i; done");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("while test"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn cstyle_for_step_by_two() {
        let result = t("for ((i=0; i<10; i+=2)); do echo $i; done");
        assert!(result.contains("set i"), "got: {}", result);
        assert!(result.contains("$i + 2"), "got: {}", result);
    }

    #[test]
    fn cstyle_for_infinite() {
        let result = t("for ((;;)); do echo loop; break; done");
        assert!(result.contains("while true"), "got: {}", result);
        assert!(result.contains("break"), "got: {}", result);
    }

    // --- Case statement edge cases ---

    #[test]
    fn case_char_classes() {
        let result = t(r#"case "$x" in [0-9]*) echo num;; [a-z]*) echo alpha;; esac"#);
        assert!(result.contains("switch"), "got: {}", result);
        assert!(result.contains("'[0-9]*'"), "got: {}", result);
    }

    #[test]
    fn case_multiple_patterns() {
        let result = t(
            r#"case "$1" in -h|--help) echo help;; -v|--verbose) echo verbose;; esac"#,
        );
        assert!(result.contains("switch"), "got: {}", result);
        assert!(result.contains("--help"), "got: {}", result);
        assert!(result.contains("-h"), "got: {}", result);
    }

    // --- String operation edge cases ---

    #[test]
    fn replace_with_empty_replacement() {
        let result = t("echo ${var/foo}");
        assert!(result.contains("string replace"), "got: {}", result);
        assert!(result.contains("foo"), "got: {}", result);
    }

    #[test]
    fn substring_negative_not_supported() {
        // Ensure negative offset doesn't panic — T2 bail is acceptable
        let _ = translate_bash_to_fish("echo ${var: -3}");
    }

    // --- Heredoc edge cases ---

    #[test]
    fn heredoc_multiline_body() {
        let result = t("cat <<'EOF'\nline1\nline2\nline3\nEOF");
        assert!(result.contains("printf"), "got: {}", result);
        assert!(result.contains("line1"), "got: {}", result);
        assert!(result.contains("line3"), "got: {}", result);
        assert!(result.contains("| cat"), "got: {}", result);
    }

    #[test]
    fn heredoc_with_grep() {
        let result = t("grep pattern <<'END'\nfoo\nbar\nbaz\nEND");
        assert!(result.contains("printf"), "got: {}", result);
        assert!(result.contains("| grep pattern"), "got: {}", result);
    }

    // --- Process substitution ---

    #[test]
    fn process_sub_diff() {
        let result = t("diff <(sort file1) <(sort file2)");
        assert!(result.contains("psub"), "got: {}", result);
        assert!(result.contains("sort file1"), "got: {}", result);
        assert!(result.contains("sort file2"), "got: {}", result);
    }

    // --- Arithmetic edge cases ---

    #[test]
    fn arith_modulo_integer() {
        let result = t("echo $((10 % 3))");
        assert!(result.contains("10 % 3"), "got: {}", result);
    }

    #[test]
    fn arith_nested_operations() {
        let result = t("echo $(( (a + b) * (c - d) ))");
        assert!(result.contains("$a + $b"), "got: {}", result);
        assert!(result.contains("$c - $d"), "got: {}", result);
    }

    #[test]
    fn arith_postincrement_standalone() {
        let result = t("(( i++ ))");
        assert!(result.contains("set i (math"), "got: {}", result);
    }

    #[test]
    fn arith_compound_assign_standalone() {
        let result = t("(( x += 5 ))");
        assert!(result.contains("set x (math"), "got: {}", result);
    }

    // --- Double bracket operators ---

    #[test]
    fn double_bracket_not_equal() {
        let result = t(r#"[[ "$x" != "hello" ]]"#);
        assert!(result.contains("string match") || result.contains("!="), "got: {}", result);
    }

    #[test]
    fn double_bracket_less_than() {
        // `<` inside [[ ]] is tricky — parser may not handle (redirect ambiguity).
        // T2 bail is acceptable; just verify no panic.
        let _ = translate_bash_to_fish(r#"[[ "$a" < "$b" ]]"#);
    }

    #[test]
    fn double_bracket_n_flag() {
        let result = t(r#"[[ -n "$var" ]]"#);
        assert!(result.contains("test -n"), "got: {}", result);
    }

    #[test]
    fn double_bracket_z_flag() {
        let result = t(r#"[[ -z "$var" ]]"#);
        assert!(result.contains("test -z"), "got: {}", result);
    }

    // --- Redirect edge cases ---

    #[test]
    fn redirect_dev_null() {
        let result = t("command > /dev/null 2>&1");
        assert!(result.contains(">/dev/null") || result.contains("> /dev/null"), "got: {}", result);
    }

    #[test]
    fn redirect_stderr_to_file() {
        let result = t("command 2> errors.log");
        assert!(result.contains("errors.log"), "got: {}", result);
    }

    // --- Mixed complex scenarios ---

    #[test]
    fn nested_if_with_arithmetic() {
        let result = t("if [ $((x + 1)) -gt 5 ]; then echo big; fi");
        assert!(result.contains("if "), "got: {}", result);
        assert!(result.contains("-gt 5"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn function_with_local_vars() {
        let result = t("myfunc() { local x=1; echo $x; }");
        assert!(result.contains("function myfunc"), "got: {}", result);
        assert!(result.contains("set -l x 1"), "got: {}", result);
    }

    #[test]
    fn for_loop_with_command_substitution() {
        let result = t("for f in $(ls *.txt); do echo $f; done");
        assert!(result.contains("for f in"), "got: {}", result);
        assert!(result.contains("ls *.txt"), "got: {}", result);
        assert!(result.contains("end"), "got: {}", result);
    }

    #[test]
    fn shopt_bails_to_t2() {
        assert!(translate_bash_to_fish("shopt -s nullglob").is_err());
    }

    #[test]
    fn declare_export_flag() {
        assert!(t("declare -x FOO=bar").contains("set -gx FOO bar"));
    }

    // --- Eval special patterns ---

    #[test]
    fn eval_pyenv_init() {
        let result = t(r#"eval "$(pyenv init -)""#);
        assert!(result.contains("pyenv init -"), "got: {}", result);
        assert!(result.contains("source"), "got: {}", result);
    }

    #[test]
    fn eval_ssh_agent() {
        let result = t(r#"eval "$(ssh-agent -s)""#);
        assert!(result.contains("ssh-agent -s"), "got: {}", result);
        assert!(result.contains("source"), "got: {}", result);
    }

    // --- Herestring edge cases ---

    #[test]
    fn herestring_with_variable() {
        let result = t("read x <<< $HOME");
        assert!(result.contains("echo $HOME"), "got: {}", result);
        assert!(result.contains("| read x"), "got: {}", result);
    }

    #[test]
    fn herestring_with_double_quoted() {
        let result = t(r#"read x <<< "hello world""#);
        assert!(result.contains("hello world"), "got: {}", result);
        assert!(result.contains("| read x"), "got: {}", result);
    }

    // --- Empty/trivial inputs ---

    #[test]
    fn translate_empty_command() {
        assert!(translate_bash_to_fish("").is_ok());
    }

    #[test]
    fn translate_comment_stripped() {
        // Parser strips comments — empty output
        assert!(t("# this is a comment").is_empty());
    }

    #[test]
    fn translate_multiple_semicolons() {
        let result = t("echo a; echo b; echo c");
        assert_eq!(result, "echo a\necho b\necho c");
    }

    // --- Bitwise arithmetic ---

    #[test]
    fn arith_bitand() {
        let result = t("echo $((x & 0xFF))");
        assert!(result.contains("bitand("), "got: {}", result);
    }

    #[test]
    fn arith_bitor() {
        let result = t("echo $((a | b))");
        assert!(result.contains("bitor("), "got: {}", result);
    }

    #[test]
    fn arith_bitxor() {
        let result = t("echo $((a ^ b))");
        assert!(result.contains("bitxor("), "got: {}", result);
    }

    #[test]
    fn arith_bitnot() {
        let result = t("echo $((~x))");
        assert!(result.contains("bitxor("), "got: {}", result);
        assert!(result.contains("-1"), "got: {}", result);
    }

    #[test]
    fn arith_shift_left() {
        let result = t("echo $((1 << 4))");
        assert!(result.contains("* 2 ^"), "got: {}", result);
    }

    #[test]
    fn arith_shift_right() {
        let result = t("echo $((x >> 2))");
        assert!(result.contains("floor("), "got: {}", result);
        assert!(result.contains("/ 2 ^"), "got: {}", result);
    }

    // --- Indirect expansion ---

    #[test]
    fn indirect_expansion() {
        let result = t(r#"echo "${!ref}""#);
        assert!(result.contains("$$ref"), "got: {}", result);
    }

    // --- Parameter transform ---

    #[test]
    fn transform_quote() {
        let result = t(r#"echo "${var@Q}""#);
        assert!(result.contains("string escape -- $var"), "got: {}", result);
    }

    #[test]
    fn transform_upper() {
        let result = t(r#"echo "${var@U}""#);
        assert!(result.contains("string upper -- $var"), "got: {}", result);
    }

    #[test]
    fn transform_lower() {
        let result = t(r#"echo "${var@L}""#);
        assert!(result.contains("string lower -- $var"), "got: {}", result);
    }

    #[test]
    fn transform_capitalize() {
        let result = t(r#"echo "${var@u}""#);
        assert!(result.contains("string sub -l 1"), "got: {}", result);
        assert!(result.contains("string upper"), "got: {}", result);
    }

    #[test]
    fn transform_p_unsupported() {
        assert!(translate_bash_to_fish(r#"echo "${var@P}""#).is_err());
    }

    #[test]
    fn transform_k_unsupported() {
        assert!(translate_bash_to_fish(r#"echo "${var@K}""#).is_err());
    }

    // --- Real-world one-liners ---

    #[test]
    fn pip_install() {
        let result = t("pip install -r requirements.txt");
        assert_eq!(result, "pip install -r requirements.txt");
    }

    #[test]
    fn docker_run() {
        let result = t("docker run -it --rm -v /tmp:/data ubuntu bash");
        assert!(result.contains("docker run"), "got: {}", result);
    }

    #[test]
    fn npm_run_dev() {
        let result = t("npm run dev");
        assert_eq!(result, "npm run dev");
    }

    #[test]
    fn make_j() {
        let result = t("make -j4");
        assert_eq!(result, "make -j4");
    }

    #[test]
    fn cargo_test_filter() {
        let result = t("cargo test -- --test-threads=1");
        assert_eq!(result, "cargo test -- --test-threads=1");
    }

    #[test]
    fn git_log_oneline() {
        let result = t("git log --oneline -10");
        assert_eq!(result, "git log --oneline -10");
    }

    #[test]
    fn tar_extract() {
        let result = t("tar xzf archive.tar.gz -C /tmp");
        assert_eq!(result, "tar xzf archive.tar.gz -C /tmp");
    }

    #[test]
    fn chmod_recursive() {
        let result = t("chmod -R 755 /var/www");
        assert_eq!(result, "chmod -R 755 /var/www");
    }

    #[test]
    fn grep_recursive() {
        let result = t("grep -rn TODO src/");
        assert_eq!(result, "grep -rn TODO src/");
    }

    #[test]
    fn xargs_rm() {
        let result = t("find . -name '*.bak' -print0 | xargs -0 rm -f");
        assert!(result.contains("find ."), "got: {}", result);
        assert!(result.contains("| xargs"), "got: {}", result);
    }

    #[test]
    fn ssh_command() {
        let result = t("ssh user@host 'uptime'");
        assert!(result.contains("ssh user@host"), "got: {}", result);
    }

    #[test]
    fn rsync_command() {
        let result = t("rsync -avz --delete src/ dest/");
        assert_eq!(result, "rsync -avz --delete src/ dest/");
    }

    #[test]
    fn curl_json() {
        let result = t("curl -s -H 'Content-Type: application/json' https://api.example.com/data");
        assert!(result.contains("curl -s"), "got: {}", result);
    }

    #[test]
    fn systemctl_status() {
        let result = t("systemctl status nginx");
        assert_eq!(result, "systemctl status nginx");
    }

    #[test]
    fn kill_process() {
        let result = t("kill -9 1234");
        assert_eq!(result, "kill -9 1234");
    }

    #[test]
    fn ps_grep_pipeline() {
        let result = t("ps aux | grep nginx | grep -v grep");
        assert_eq!(result, "ps aux | grep nginx | grep -v grep");
    }

    #[test]
    fn du_sort() {
        let result = t("du -sh * | sort -hr | head -10");
        assert!(result.contains("du -sh"), "got: {}", result);
        assert!(result.contains("| sort -hr"), "got: {}", result);
    }

    #[test]
    fn source_env_file() {
        // source passes through (fish also has `source`)
        let result = t("source ~/.bashrc");
        assert!(result.contains("source"), "got: {}", result);
    }

    #[test]
    fn dot_source_profile() {
        // . (dot source) passes through
        let result = t(". ~/.profile");
        assert!(result.contains("."), "got: {}", result);
    }

    // --- Nested substitution ---

    #[test]
    fn nested_param_in_cmd_subst() {
        let result = t(r#"echo "$(basename "${file}")""#);
        assert!(result.contains("basename"), "got: {}", result);
    }

    #[test]
    fn cmd_subst_in_assignment() {
        let result = t("result=$(grep -c error log.txt)");
        assert!(result.contains("set result"), "got: {}", result);
        assert!(result.contains("grep -c error"), "got: {}", result);
    }

    #[test]
    fn arith_in_array_index() {
        let result = t("echo ${arr[$((i+1))]}");
        assert!(result.contains("$arr"), "got: {}", result);
    }

    #[test]
    fn nested_cmd_subst_three_deep() {
        let result = t("echo $(dirname $(dirname $(which python)))");
        assert!(result.contains("dirname"), "got: {}", result);
        assert!(result.contains("which python"), "got: {}", result);
    }

    // --- Complex quoting ---

    #[test]
    fn mixed_quotes_in_command() {
        let result = t(r#"echo "It's a test""#);
        assert!(result.contains("It"), "got: {}", result);
    }

    #[test]
    fn double_quotes_preserve_variable() {
        let result = t(r#"echo "Hello $USER, you are in $PWD""#);
        assert!(result.contains("$USER"), "got: {}", result);
        assert!(result.contains("$PWD"), "got: {}", result);
    }

    #[test]
    fn empty_string_arg() {
        let result = t(r#"echo "" foo"#);
        assert!(result.contains(r#""""#), "got: {}", result);
    }

    // --- For loop edge cases ---

    #[test]
    fn for_in_brace_range() {
        let result = t("for i in {1..5}; do echo $i; done");
        assert!(result.contains("for i in (seq 1 5)"), "got: {}", result);
    }

    #[test]
    fn for_in_brace_range_with_step() {
        let result = t("for i in {0..10..2}; do echo $i; done");
        assert!(result.contains("seq 0 2 10"), "got: {}", result);
    }

    #[test]
    fn for_loop_multiple_commands() {
        let result = t("for f in *.txt; do echo $f; wc -l $f; done");
        assert!(result.contains("for f in *.txt"), "got: {}", result);
        assert!(result.contains("echo $f"), "got: {}", result);
        assert!(result.contains("wc -l $f"), "got: {}", result);
    }

    // --- While loop edge cases ---

    #[test]
    fn while_true_loop() {
        let result = t("while true; do echo loop; sleep 1; done");
        assert!(result.contains("while true"), "got: {}", result);
        assert!(result.contains("sleep 1"), "got: {}", result);
    }

    #[test]
    fn while_command_condition() {
        let result = t("while pgrep -x nginx > /dev/null; do sleep 5; done");
        assert!(result.contains("while pgrep"), "got: {}", result);
    }

    // --- If edge cases ---

    #[test]
    fn if_command_condition() {
        let result = t("if grep -q error /var/log/syslog; then echo found; fi");
        assert!(result.contains("if grep -q error"), "got: {}", result);
        assert!(result.contains("echo found"), "got: {}", result);
    }

    #[test]
    fn if_negated_condition() {
        let result = t("if ! command -v git > /dev/null; then echo missing; fi");
        assert!(result.contains("if not"), "got: {}", result);
        assert!(result.contains("command -v git"), "got: {}", result);
    }

    #[test]
    fn if_test_file_ops() {
        let result = t("if [ -f /etc/passwd ] && [ -r /etc/passwd ]; then echo ok; fi");
        assert!(result.contains("-f /etc/passwd"), "got: {}", result);
        assert!(result.contains("-r /etc/passwd"), "got: {}", result);
    }

    #[test]
    fn if_elif_chain() {
        let result = t("if [ $x -eq 1 ]; then echo one; elif [ $x -eq 2 ]; then echo two; elif [ $x -eq 3 ]; then echo three; else echo other; fi");
        assert!(result.contains("else if"), "got: {}", result);
        assert!(result.contains("echo three"), "got: {}", result);
        assert!(result.contains("echo other"), "got: {}", result);
    }

    // --- Case edge cases ---

    #[test]
    fn case_with_default_only() {
        let result = t(r#"case "$x" in *) echo default ;; esac"#);
        assert!(result.contains("switch"), "got: {}", result);
        assert!(result.contains("case '*'"), "got: {}", result);
    }

    #[test]
    fn case_empty_body() {
        // Empty case arm: a) ;; — was causing parser infinite loop
        let result = t(r#"case "$x" in a) ;; b) echo b ;; esac"#);
        assert!(result.contains("switch"), "got: {}", result);
        assert!(result.contains("echo b"), "got: {}", result);
    }

    // --- Function edge cases ---

    #[test]
    fn function_with_return() {
        let result = t("myfunc() { echo hello; return 0; }");
        assert!(result.contains("function myfunc"), "got: {}", result);
        assert!(result.contains("return 0"), "got: {}", result);
    }

    #[test]
    fn function_keyword_syntax() {
        let result = t("function myfunc { echo hello; }");
        assert!(result.contains("function myfunc"), "got: {}", result);
    }

    // --- Export edge cases ---

    #[test]
    fn export_with_special_chars_value() {
        let result = t(r#"export GREETING="Hello World""#);
        assert!(result.contains("set -gx GREETING"), "got: {}", result);
        assert!(result.contains("Hello World"), "got: {}", result);
    }

    #[test]
    fn export_append_path() {
        let result = t(r#"export PATH="$HOME/bin:$PATH""#);
        assert!(result.contains("set -gx PATH"), "got: {}", result);
    }

    // --- Declare edge cases ---

    #[test]
    fn declare_local() {
        let result = t("declare foo=bar");
        assert!(result.contains("set") && result.contains("foo") && result.contains("bar"), "got: {}", result);
    }

    #[test]
    fn declare_integer() {
        let result = translate_bash_to_fish("declare -i num=42");
        // -i might bail or be handled
        let _ = result;
    }

    // --- Read command ---

    #[test]
    fn read_single_var() {
        let result = t("read name");
        assert!(result.contains("read name"), "got: {}", result);
    }

    #[test]
    fn read_prompt() {
        let result = t(r#"read -p "Enter name: " name"#);
        assert!(result.contains("read"), "got: {}", result);
    }

    // --- Test/bracket edge cases ---

    #[test]
    fn test_string_equality() {
        let result = t(r#"[ "$a" = "hello" ]"#);
        assert!(result.contains("test") || result.contains("["), "got: {}", result);
    }

    #[test]
    fn test_numeric_comparison() {
        let result = t("[ $count -gt 10 ]");
        assert!(result.contains("10"), "got: {}", result);
    }

    #[test]
    fn double_bracket_regex_with_capture() {
        let result = t(r#"[[ "$line" =~ ^([0-9]+) ]]"#);
        assert!(result.contains("string match -r"), "got: {}", result);
    }

    #[test]
    fn double_bracket_compound() {
        let result = t(r#"[[ -n "$a" && -z "$b" ]]"#);
        assert!(result.contains("-n"), "got: {}", result);
        assert!(result.contains("-z"), "got: {}", result);
    }

    // --- Redirect edge cases ---

    #[test]
    fn redirect_both_to_file() {
        let result = t("command > out.txt 2>&1");
        assert!(result.contains("out.txt"), "got: {}", result);
    }

    #[test]
    fn redirect_input_and_output() {
        let result = t("sort < input.txt > output.txt");
        assert!(result.contains("sort"), "got: {}", result);
        assert!(result.contains("input.txt"), "got: {}", result);
    }

    #[test]
    fn redirect_append_stderr() {
        let result = t("command >> log.txt 2>&1");
        assert!(result.contains("log.txt"), "got: {}", result);
    }

    // --- Trap edge cases ---

    #[test]
    fn trap_on_err() {
        let result = translate_bash_to_fish("trap 'echo error' ERR");
        // ERR trap may or may not be supported
        let _ = result;
    }

    #[test]
    fn trap_cleanup_function() {
        let result = t("trap cleanup EXIT");
        assert!(result.contains("cleanup"), "got: {}", result);
        assert!(result.contains("fish_exit"), "got: {}", result);
    }

    // --- Arithmetic edge cases ---

    #[test]
    fn arith_comma_operator() {
        // Comma in arithmetic: ((a=1, b=2))
        let result = translate_bash_to_fish("((a=1, b=2))");
        // May or may not be supported
        let _ = result;
    }

    #[test]
    fn arith_pre_decrement_in_subst() {
        // Pre-decrement inside $(()) is unsupported — bails to T2
        assert!(translate_bash_to_fish("echo $((--x))").is_err());
    }

    #[test]
    fn arith_hex_literal() {
        let result = t("echo $((0xFF))");
        assert!(result.contains("math"), "got: {}", result);
    }

    // --- Compound commands ---

    #[test]
    fn brace_group_with_redirect() {
        let result = t("{ echo a; echo b; } > output.txt");
        assert!(result.contains("echo a"), "got: {}", result);
        assert!(result.contains("echo b"), "got: {}", result);
    }

    #[test]
    fn subshell_with_env() {
        // Subshell that modifies env — fish begin/end isn't a true subshell
        let result = translate_bash_to_fish("(cd /tmp && ls)");
        // Should work as begin/end or bail
        let _ = result;
    }

    // --- Complex real-world patterns ---

    #[test]
    fn nvm_init_pattern() {
        let result = translate_bash_to_fish(r#"export NVM_DIR="$HOME/.nvm""#);
        assert!(result.is_ok());
    }

    #[test]
    fn conditional_mkdir() {
        let result = t("[ -d /tmp/mydir ] || mkdir -p /tmp/mydir");
        assert!(result.contains("/tmp/mydir"), "got: {}", result);
        assert!(result.contains("mkdir"), "got: {}", result);
    }

    #[test]
    fn count_files() {
        let result = t("ls -1 | wc -l");
        assert_eq!(result, "ls -1 | wc -l");
    }

    #[test]
    fn check_exit_code() {
        let result = t("if [ $? -ne 0 ]; then echo failed; fi");
        assert!(result.contains("$status"), "got: {}", result);
    }

    #[test]
    fn string_contains_check() {
        let result = t(r#"[[ "$string" == *"substring"* ]]"#);
        assert!(result.contains("string match"), "got: {}", result);
    }

    #[test]
    fn default_value_in_assignment() {
        let result = t(r#"name="${1:-World}""#);
        assert!(result.contains("World"), "got: {}", result);
    }

    #[test]
    fn multiline_if() {
        let result = t("if [ -f ~/.bashrc ]; then\n  echo found\nfi");
        assert!(result.contains("if"), "got: {}", result);
        assert!(result.contains("echo found"), "got: {}", result);
    }

    #[test]
    fn variable_in_path() {
        let result = t(r#"ls "$HOME/Documents""#);
        assert!(result.contains("$HOME"), "got: {}", result);
    }

    #[test]
    fn command_chaining() {
        let result = t("mkdir -p build && cd build && cmake ..");
        assert!(result.contains("mkdir -p build"), "got: {}", result);
        assert!(result.contains("cd build"), "got: {}", result);
    }

    #[test]
    fn process_sub_with_while() {
        let result = t("while read line; do echo $line; done < <(ls -1)");
        assert!(result.contains("psub"), "got: {}", result);
    }

    #[test]
    fn heredoc_cat_pattern() {
        let result = t("cat <<'EOF'\nhello world\nEOF");
        assert!(result.contains("hello world"), "got: {}", result);
    }

    #[test]
    fn heredoc_to_file() {
        let result = t("cat <<'EOF' > /tmp/file\ncontent\nEOF");
        assert!(result.contains("content"), "got: {}", result);
    }

    // --- Param expansion edge cases ---

    #[test]
    fn param_strip_extension() {
        let result = t(r#"echo "${filename%.*}""#);
        assert!(result.contains("string replace -r"), "got: {}", result);
    }

    #[test]
    fn param_strip_path() {
        let result = t(r#"echo "${filepath##*/}""#);
        assert!(result.contains("string replace -r"), "got: {}", result);
    }

    #[test]
    fn param_get_extension() {
        let result = t(r#"echo "${filename##*.}""#);
        assert!(result.contains("string replace -r"), "got: {}", result);
    }

    #[test]
    fn param_get_directory() {
        let result = t(r#"echo "${filepath%/*}""#);
        assert!(result.contains("string replace -r"), "got: {}", result);
    }

    #[test]
    fn param_default_empty_var() {
        let result = t(r#"echo "${unset_var:-default_value}""#);
        assert!(result.contains("default_value"), "got: {}", result);
    }

    #[test]
    fn param_error_with_message() {
        let result = t(r#"echo "${required:?must be set}""#);
        assert!(result.contains("must be set"), "got: {}", result);
    }

    #[test]
    fn substring_from_end() {
        let result = t(r#"echo "${str:0:3}""#);
        assert!(result.contains("string sub"), "got: {}", result);
    }

    // --- Array edge cases ---

    #[test]
    fn array_iteration() {
        let result = t(r#"for item in "${arr[@]}"; do echo "$item"; done"#);
        assert!(result.contains("for item in"), "got: {}", result);
        assert!(result.contains("$arr"), "got: {}", result);
    }

    #[test]
    fn array_length_check() {
        let result = t(r#"echo "${#arr[@]}""#);
        assert!(result.contains("count $arr"), "got: {}", result);
    }

    #[test]
    fn array_with_spaces() {
        let result = t(r#"arr=("hello world" "foo bar")"#);
        assert!(result.contains("set arr"), "got: {}", result);
    }

    // --- Background and job control ---

    #[test]
    fn background_with_redirect() {
        let result = t("long_running_task > /dev/null 2>&1 &");
        assert!(result.contains("&"), "got: {}", result);
    }

    #[test]
    fn sequential_background() {
        let result = t("cmd1 & cmd2 &");
        assert!(result.contains("&"), "got: {}", result);
    }

    // --- Unset edge cases ---

    #[test]
    fn unset_multiple() {
        let result = t("unset FOO BAR BAZ");
        assert!(result.contains("set -e FOO"), "got: {}", result);
        assert!(result.contains("set -e BAR"), "got: {}", result);
        assert!(result.contains("set -e BAZ"), "got: {}", result);
    }

    #[test]
    fn unset_function() {
        // unset -f should bail
        let result = translate_bash_to_fish("unset -f myfunc");
        let _ = result;
    }

    // --- Misc edge cases ---

    #[test]
    fn true_false_commands() {
        assert_eq!(t("true"), "true");
        assert_eq!(t("false"), "false");
    }

    #[test]
    fn colon_noop() {
        let result = t(":");
        assert!(result.contains(":") || result.contains("true") || result.is_empty(), "got: {}", result);
    }

    #[test]
    fn echo_with_flags() {
        let result = t("echo -n hello");
        assert!(result.contains("echo -n hello"), "got: {}", result);
    }

    #[test]
    fn echo_with_escape() {
        let result = t("echo -e 'hello\\nworld'");
        assert!(result.contains("echo"), "got: {}", result);
    }

    #[test]
    fn printf_format() {
        let result = t(r#"printf "%s\n" hello"#);
        assert!(result.contains("printf"), "got: {}", result);
    }

    #[test]
    fn test_with_not() {
        let result = t("[ ! -f /tmp/lock ]");
        assert!(result.contains("!") || result.contains("not"), "got: {}", result);
    }

    #[test]
    fn pipeline_three_stages() {
        let result = t("cat file | sort | uniq -c");
        assert!(result.contains("| sort |"), "got: {}", result);
    }

    #[test]
    fn subshell_captures_output() {
        let result = t("result=$(cd /tmp && pwd)");
        assert!(result.contains("set result"), "got: {}", result);
    }

    #[test]
    fn multiple_var_assignment() {
        let result = t("a=1; b=2; c=3");
        assert!(result.contains("set a 1"), "got: {}", result);
        assert!(result.contains("set b 2"), "got: {}", result);
        assert!(result.contains("set c 3"), "got: {}", result);
    }

    #[test]
    fn replace_all_slashes() {
        let result = t(r#"echo "${path//\//\\.}""#);
        assert!(result.contains("string replace"), "got: {}", result);
    }
}
