/// Quick check: does this string contain bash-specific syntax?
/// This must be FAST — it runs on every Enter keypress.
/// No regex, no parsing — just byte-level pattern scanning.
pub fn looks_like_bash(input: &str) -> bool {
    let bytes = input.as_bytes();
    let len = bytes.len();

    // Single pass: check 2-byte trigger patterns and set flags for slower checks.
    // Most fish commands bail here immediately (no trigger bytes at all).
    let mut has_keyword_char = false;
    let mut has_brace = false;
    let mut has_eq = false;
    let mut has_paren = false;
    let mut in_dquote = false;
    let mut i = 0;
    while i < len {
        let b = bytes[i];
        let next = if i + 1 < len { bytes[i + 1] } else { 0 };
        match b {
            // Track double-quote state so we don't treat ' inside "..." as
            // a single-quote delimiter (e.g. "it's $((2+2)) o'clock").
            b'\\' if in_dquote => { i += 2; continue; }
            b'"' if !in_dquote => { in_dquote = true; i += 1; continue; }
            b'"' if in_dquote => { in_dquote = false; i += 1; continue; }
            // Skip single-quoted sections — everything is literal inside.
            // Prevents false positives like awk '{print $1}'.
            // But inside double quotes, ' is just a literal character.
            b'\'' if !in_dquote => {
                // $'...' (ANSI-C quoting) IS bash-specific.
                if i > 0 && bytes[i - 1] == b'$' {
                    return true;
                }
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            b'`' => return true,
            // $( alone is valid fish 3.4+ command substitution — don't trigger.
            // $(( is bash arithmetic expansion — not valid fish.
            b'$' => match next {
                b'{' | b'$' | b'#' | b'?' | b'!' | b'0'..=b'9' | b'@' | b'*' => return true,
                b'(' if i + 2 < len && bytes[i + 2] == b'(' => return true,
                _ => {}
            },
            b'<' if matches!(next, b'<' | b'(') => return true,
            b'>' if next == b'(' => return true,
            b'[' if next == b'[' => return true,
            b'(' if next == b'(' && (i == 0 || bytes[i - 1] != b'$') => return true,
            b'(' => has_paren = true,
            b'=' => has_eq = true,
            b'{' => has_brace = true,
            b' ' | b';' | b'\t' | b'\n' => has_keyword_char = true,
            _ => {}
        }
        i += 1;
    }

    // Bash-specific syntax at command position: NAME=, NAME+=, NAME[..]=,
    // NAME(), ( subshell, or { brace group.
    if (has_eq || has_paren || has_brace) && has_bash_cmd_start(bytes) {
        return true;
    }

    // Bash-only variable names: $RANDOM, $SECONDS, etc.
    // Fish doesn't have these as built-in variables.
    if has_bash_var(bytes) {
        return true;
    }

    // Keyword-based checks — only if separator chars were seen.
    if has_keyword_char {
        // Substring indicators with enough built-in context to avoid false positives.
        const INDICATORS: &[&str] = &[
            "export ",
            "unset ",
            "declare ",
            "typeset ",
            "readonly ",
            "local ",
            " do ",
            ";do ",
            "do\n",
            "do;",
            "shopt ",
            "read -p",
            "read -r",
            "for ((",
            "trap ",
            "eval ",
            "select ",
            "getopts ",
        ];
        // Control-flow keywords checked with word boundaries to avoid
        // false positives (e.g. " fi" inside "file", " done" in "done!").
        const BOUNDARY_KEYWORDS: &[&[u8]] = &[
            b"fi", b"esac", b"let",
        ];
        for kw in INDICATORS {
            if input.contains(kw) {
                return true;
            }
        }
        for kw in BOUNDARY_KEYWORDS {
            if has_word(bytes, kw) {
                return true;
            }
        }
    }

    // Brace range expansion: {1..5}, {a..z}, {1..10..2} — needs quote-aware scan
    if has_brace && has_brace_range(bytes) {
        return true;
    }

    false
}

/// Check for bash-only variable references like `$RANDOM`, `$SECONDS`, etc.
/// Requires a word boundary after the name to avoid matching `$RANDOM_SEED`.
fn has_bash_var(bytes: &[u8]) -> bool {
    const BASH_VARS: &[&[u8]] = &[
        b"BASH_VERSION", b"BASH_REMATCH", b"BASH_SOURCE",
        b"RANDOM", b"SECONDS", b"LINENO", b"FUNCNAME",
        b"SHELLOPTS", b"BASHOPTS", b"PIPESTATUS",
    ];
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Skip single-quoted sections
        if bytes[i] == b'\'' {
            i += 1;
            while i < len && bytes[i] != b'\'' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'$' {
            let start = i + 1;
            for var in BASH_VARS {
                let end = start + var.len();
                if end <= len
                    && bytes[start..end] == **var
                    && (end == len
                        || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_')
                {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Check for bash brace range expansion like {1..5} or {a..z}.
/// Skips single- and double-quoted sections to avoid false positives.
fn has_brace_range(bytes: &[u8]) -> bool {
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' => {
                let start = i + 1;
                i = start;
                while i < len && bytes[i] != b'}' {
                    i += 1;
                }
                if i < len {
                    let inner = &bytes[start..i];
                    if let Some(dot_pos) = inner.windows(2).position(|w| w == b"..")
                        && dot_pos > 0
                        && dot_pos + 2 < inner.len()
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Check if `kw` appears as a standalone word: preceded by a separator
/// (or start of input) and followed by a separator (or end of input).
fn has_word(bytes: &[u8], kw: &[u8]) -> bool {
    let len = bytes.len();
    let kw_len = kw.len();
    let mut i = 0;
    while i + kw_len <= len {
        if bytes[i..i + kw_len] == *kw {
            let pre = i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b';' | b'\n' | b'|' | b'&');
            let post = i + kw_len == len
                || matches!(bytes[i + kw_len], b' ' | b'\t' | b';' | b'\n' | b'|' | b'&' | b')');
            if pre && post {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Given `NAME=` at `eq_pos`, skip past the value and check whether a command
/// follows. Returns `Some(pos)` if a token follows (prefix assignment — valid
/// fish 3.1+), or `None` if bare (bash-only).
fn skip_prefix_value(bytes: &[u8], eq_pos: usize) -> Option<usize> {
    let len = bytes.len();
    let mut j = eq_pos + 1;
    // Skip value (handles mixed quoting)
    while j < len && !matches!(bytes[j], b' ' | b'\t' | b'\n' | b';' | b'|' | b'&') {
        match bytes[j] {
            // Skip quoted values; if unterminated, j stays at len and
            // the outer while condition (j < len) exits gracefully.
            b'\'' => {
                j += 1;
                while j < len && bytes[j] != b'\'' { j += 1; }
                if j < len { j += 1; }
            }
            b'"' => {
                j += 1;
                while j < len && bytes[j] != b'"' {
                    if bytes[j] == b'\\' { j += 1; }
                    j += 1;
                }
                if j < len { j += 1; }
            }
            _ => j += 1,
        }
    }
    // Skip whitespace after value
    while j < len && matches!(bytes[j], b' ' | b'\t') { j += 1; }
    // Bare if nothing or a separator follows; otherwise a command is next
    if j >= len || matches!(bytes[j], b'\n' | b';' | b'|' | b'&') {
        None
    } else {
        Some(j)
    }
}

/// Check for bash-specific syntax at command position:
/// `NAME=` (bare), `NAME+=`, `NAME[..]=`, `NAME()`, `(` subshell, or `{` brace group.
/// `NAME=value cmd` (prefix assignment) is valid fish 3.1+ and is NOT flagged.
/// Fish only allows `(cmd)` in argument position. Skips quoted sections.
fn has_bash_cmd_start(bytes: &[u8]) -> bool {
    let len = bytes.len();
    let mut i = 0;
    // 0 = expecting first word (skip whitespace), 1 = inside first word, 2 = past it
    let mut state: u8 = 0;
    while i < len {
        match bytes[i] {
            b'\'' => {
                state = 2;
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            b'"' => {
                state = 2;
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b';' | b'\n' | b'|' | b'&' => state = 0,
            b' ' | b'\t' if state == 0 => {}
            b' ' | b'\t' => state = 2,
            b'(' if state == 0 => return true, // subshell at command start
            // { at command start with whitespace after = bash brace group
            // (fish brace expansion {a,b,c} has no space after {)
            b'{' if state == 0
                && i + 1 < len
                && matches!(bytes[i + 1], b' ' | b'\t' | b'\n') =>
            {
                return true;
            }
            b'(' if state == 1 => return true, // NAME() — bash function def
            b'=' if state == 1 => match skip_prefix_value(bytes, i) {
                None => return true, // bare NAME=val — bash-only
                Some(next) => { i = next; state = 0; continue; }
            }
            _ if state == 0 => {
                if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                    state = 1;
                } else {
                    state = 2;
                }
            }
            _ if state == 1 => {
                if bytes[i] == b'+' && i + 1 < len && bytes[i + 1] == b'=' {
                    return true; // NAME+=
                }
                // NAME[...]= or NAME[...]+=  (array element assignment)
                if bytes[i] == b'[' {
                    let mut j = i + 1;
                    while j < len && bytes[j] != b']' {
                        j += 1;
                    }
                    if j + 1 < len && bytes[j + 1] == b'=' {
                        return true;
                    }
                    if j + 2 < len && bytes[j + 1] == b'+' && bytes[j + 2] == b'=' {
                        return true;
                    }
                }
                if !bytes[i].is_ascii_alphanumeric() && bytes[i] != b'_' {
                    state = 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_export() {
        assert!(looks_like_bash("export PATH=/usr/bin:$PATH"));
        assert!(looks_like_bash("export EDITOR=vim"));
    }

    #[test]
    fn detects_for_loop() {
        assert!(looks_like_bash("for i in $(seq 5); do echo $i; done"));
    }

    #[test]
    fn detects_if_then() {
        assert!(looks_like_bash("if [ -f foo ]; then echo yes; fi"));
    }

    #[test]
    fn dollar_paren_is_valid_fish() {
        // $() is valid fish 3.4+ command substitution — not bash-specific
        assert!(!looks_like_bash("echo $(whoami)"));
        assert!(!looks_like_bash("set myvar $(string upper hello)"));
        assert!(!looks_like_bash("echo $(date)"));
        // But $(( )) is bash arithmetic — still detected
        assert!(looks_like_bash("echo $((2 + 2))"));
        assert!(looks_like_bash("echo $((1+2))"));
        // $(( )) inside double quotes with apostrophes must still be detected
        assert!(looks_like_bash(r#"echo "Hello $(whoami), it's $((2+2)) o'clock""#));
    }

    #[test]
    fn detects_double_brackets() {
        assert!(looks_like_bash("[[ -n \"$HOME\" ]] && echo yes"));
    }

    #[test]
    fn detects_parameter_expansion() {
        assert!(looks_like_bash("echo ${HOME:-/tmp}"));
    }

    #[test]
    fn detects_standalone_double_paren() {
        assert!(looks_like_bash("(( i++ ))"));
        assert!(looks_like_bash("(( x += 5 ))"));
        assert!(looks_like_bash("(( count = 0 ))"));
        assert!(looks_like_bash("echo $((2 + 2))"));
    }

    #[test]
    fn ignores_plain_fish() {
        assert!(!looks_like_bash("echo hello"));
        assert!(!looks_like_bash("set -gx PATH /usr/bin $PATH"));
        assert!(!looks_like_bash("for i in (seq 5); echo $i; end"));
    }

    #[test]
    fn brace_range_unquoted() {
        assert!(has_brace_range(b"{1..5}"));
        assert!(has_brace_range(b"echo {a..z}"));
        assert!(has_brace_range(b"{1..10..2}"));
        assert!(!has_brace_range(b"{..5}"));
        assert!(!has_brace_range(b"{1..}"));
    }

    #[test]
    fn brace_range_skips_quotes() {
        assert!(!has_brace_range(b"echo '{1..5}'"));
        assert!(!has_brace_range(br#"echo "{1..5}""#));
        assert!(has_brace_range(b"echo '{skip}' {1..5}"));
    }

    #[test]
    fn ignores_fish_and_or_operators() {
        // && and || are valid fish 3.0+ syntax — not bash-specific
        assert!(!looks_like_bash("echo foo && echo bar"));
        assert!(!looks_like_bash("echo foo || echo bar"));
        assert!(!looks_like_bash("true && false || echo fallback"));
    }

    #[test]
    fn detects_bare_assignment() {
        assert!(looks_like_bash("FOO=hello"));
        assert!(looks_like_bash("FOO=hello && echo $FOO"));
        assert!(looks_like_bash("x=1"));
        assert!(looks_like_bash("_VAR=value"));
        assert!(looks_like_bash("echo ok; FOO=bar"));
    }

    #[test]
    fn detects_subshell() {
        assert!(looks_like_bash("(cd /tmp && pwd)"));
        assert!(looks_like_bash("(echo a; echo b) | sort"));
        assert!(looks_like_bash("echo ok; (cd /tmp)"));
    }

    #[test]
    fn subshell_skips_fish_cmd_substitution() {
        // fish (cmd) in argument position — not a subshell
        assert!(!looks_like_bash("for i in (seq 5); echo $i; end"));
        assert!(!looks_like_bash("echo (date)"));
        assert!(!looks_like_bash("set x (pwd)"));
    }

    #[test]
    fn bare_assignment_skips_false_positives() {
        // fish set command — not a bash assignment
        assert!(!looks_like_bash("set -gx PATH /usr/bin"));
        // = inside quotes
        assert!(!looks_like_bash("echo 'FOO=bar'"));
        assert!(!looks_like_bash(r#"echo "FOO=bar""#));
        // Not at token boundary (part of a larger word)
        assert!(!looks_like_bash("echo FOO=bar"));
    }

    #[test]
    fn detects_assignment_after_operators() {
        // Bare assignments after operators — bash-only
        assert!(looks_like_bash("echo ok && FOO=bar"));
        assert!(looks_like_bash("echo ok || FOO=bar"));
        assert!(looks_like_bash("echo ok & FOO=bar"));
        // Prefix assignment before command — valid fish 3.1+
        assert!(!looks_like_bash("echo ok | FOO=bar cat"));
    }

    #[test]
    fn prefix_assignment_is_valid_fish() {
        // NAME=value command is valid fish 3.1+ — not bash-specific
        assert!(!looks_like_bash("FOO=bar echo hello"));
        assert!(!looks_like_bash("GIT_DIR=. git status"));
        assert!(!looks_like_bash("FOO=bar BAZ=qux echo hello"));
        assert!(!looks_like_bash("FOO='hello world' echo test"));
        assert!(!looks_like_bash("FOO= echo hello"));
        // But bare assignments (no command after) ARE bash-only
        assert!(looks_like_bash("FOO=bar"));
        assert!(looks_like_bash("FOO=bar BAZ=qux"));
        assert!(looks_like_bash("A=1 B=2"));
    }

    #[test]
    fn detects_function_definition() {
        assert!(looks_like_bash("greet() { echo hello; }"));
        assert!(looks_like_bash("greet() { echo \"Hello, $1!\"; }; greet \"World\""));
        assert!(looks_like_bash("_my_func() { pwd; }"));
    }

    #[test]
    fn detects_special_variables() {
        assert!(looks_like_bash("echo $#"));
        assert!(looks_like_bash("echo \"args: $#\""));
        assert!(looks_like_bash("echo $?"));
        assert!(looks_like_bash("echo $!"));
        assert!(looks_like_bash("echo $$"));
        assert!(looks_like_bash("echo $0"));
        assert!(looks_like_bash("echo $1"));
        assert!(looks_like_bash("echo $@"));
        assert!(looks_like_bash("echo $*"));
    }

    #[test]
    fn detects_backtick_substitution() {
        assert!(looks_like_bash("echo `hostname`"));
        assert!(looks_like_bash("`whoami`"));
    }

    #[test]
    fn detects_compound_assignment() {
        assert!(looks_like_bash("arr+=(4 5)"));
        assert!(looks_like_bash("str+=hello"));
        assert!(looks_like_bash("echo ok; x+=1"));
    }

    #[test]
    fn detects_array_element_assignment() {
        assert!(looks_like_bash("arr[0]=hello"));
        assert!(looks_like_bash("arr[1]+=more"));
        assert!(looks_like_bash("echo ok; arr[2]=val"));
    }

    #[test]
    fn detects_brace_group() {
        assert!(looks_like_bash("{ echo a; echo b; }"));
        assert!(looks_like_bash("{ echo a; } > /tmp/out"));
        assert!(looks_like_bash("echo ok; { echo a; }"));
    }

    #[test]
    fn brace_group_skips_fish_brace_expansion() {
        // fish brace expansion — no space after {
        assert!(!looks_like_bash("echo {a,b,c}"));
        assert!(!looks_like_bash("mkdir -p /tmp/{x,y,z}"));
    }

    #[test]
    fn detects_ansi_c_quoting() {
        assert!(looks_like_bash("echo $'hello\\nworld'"));
        assert!(looks_like_bash("echo $'\\t'"));
    }

    #[test]
    fn keyword_boundary_avoids_false_positives() {
        // "fi" inside words like "file", "find", "diff"
        assert!(!looks_like_bash("cat file.txt"));
        assert!(!looks_like_bash("diff file1 file2"));
        assert!(!looks_like_bash("find . -name '*.py'"));
        // "then" in normal text (no longer a boundary keyword)
        assert!(!looks_like_bash("echo \"and then\""));
        assert!(!looks_like_bash("echo then we go home"));
        // "done" inside normal text
        assert!(!looks_like_bash("echo \"I am done\""));
        // "let" inside normal text (quoted avoids boundary match)
        assert!(!looks_like_bash("echo \"let me think\""));
        // But real bash keywords still detected
        assert!(looks_like_bash("if true; then echo yes; fi"));
        assert!(looks_like_bash("for i in 1 2; do echo $i; done"));
        assert!(looks_like_bash("let x=5"));
    }

    #[test]
    fn skips_dollar_in_single_quotes() {
        // awk/sed with $1, $2 etc. inside single quotes — NOT bash
        assert!(!looks_like_bash("awk '{print $1}' file"));
        assert!(!looks_like_bash("awk '{print $1, $2}' file.txt"));
        assert!(!looks_like_bash("sed 's/$HOME/foo/'"));
        // But $1 outside quotes IS bash
        assert!(looks_like_bash("echo $1"));
        // $'...' (ANSI-C quoting) should still be detected
        assert!(looks_like_bash("echo $'hello\\nworld'"));
    }

    #[test]
    fn skips_bash_vars_in_single_quotes() {
        assert!(!looks_like_bash("echo '$RANDOM'"));
        assert!(!looks_like_bash("awk '{print $RANDOM}'"));
        // But outside quotes, still detected
        assert!(looks_like_bash("echo $RANDOM"));
    }

    #[test]
    fn skips_commands_with_quoted_dollar() {
        // Common tools with $ inside single quotes — NOT bash
        assert!(!looks_like_bash("sed 's/foo/bar/g' file"));
        assert!(!looks_like_bash("sed -i 's/old/new/g' file.txt"));
        assert!(!looks_like_bash("grep -E 'pattern' file"));
        assert!(!looks_like_bash("grep -r 'TODO' ."));
        assert!(!looks_like_bash("find . -name '*.txt'"));
    }

    #[test]
    fn ignores_fish_builtins() {
        assert!(!looks_like_bash("set -l myvar hello"));
        assert!(!looks_like_bash("set -gx PATH /usr/bin $PATH"));
        assert!(!looks_like_bash("string match -r 'pattern' input"));
        assert!(!looks_like_bash("string replace -a old new $var"));
        assert!(!looks_like_bash("math '2 + 2'"));
    }

    #[test]
    fn ignores_simple_commands() {
        assert!(!looks_like_bash("echo hello world"));
        assert!(!looks_like_bash("ls -la /tmp"));
        assert!(!looks_like_bash("cd /tmp && ls"));
        assert!(!looks_like_bash("mkdir -p /tmp/test"));
    }

    #[test]
    fn detects_heredoc() {
        assert!(looks_like_bash("cat <<'EOF'\nhello\nEOF"));
        assert!(looks_like_bash("cat <<EOF\nhello\nEOF"));
        assert!(looks_like_bash("cat <<-'EOF'\nhello\nEOF"));
    }

    #[test]
    fn detects_bash_only_variables() {
        assert!(looks_like_bash("echo $RANDOM"));
        assert!(looks_like_bash("echo $SECONDS"));
        assert!(looks_like_bash("echo $BASH_VERSION"));
        assert!(looks_like_bash("echo $LINENO"));
        assert!(looks_like_bash("echo $FUNCNAME"));
        assert!(looks_like_bash("echo $PIPESTATUS"));
        // Should not match variable names that start with a bash var name
        assert!(!looks_like_bash("echo $RANDOM_SEED"));
        assert!(!looks_like_bash("echo $SECONDS_ELAPSED"));
    }
}
