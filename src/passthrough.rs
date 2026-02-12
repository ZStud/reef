use std::io::{self, Write};
use std::process::{Command, Stdio};

use crate::env_diff::{self, EnvSnapshot};

/// Sentinel markers for separating env data from command output in bash.
const ENV_MARKER: &str = "__REEF_ENV_MARKER_5f3a__";
const CWD_MARKER: &str = "__REEF_CWD_MARKER_5f3a__";

/// Execute a command through bash with streaming output, then print
/// environment changes as fish commands to stdout.
///
/// How it works:
/// 1. Capture a "before" snapshot of the current environment
/// 2. Run the command in bash with stderr inherited (streams directly)
/// 3. Stdout is captured — the command output appears before our markers,
///    and we print it back to the real stdout immediately
/// 4. After the markers, we parse the env dump
/// 5. Diff before/after and print fish set commands
///
/// The caller (fish) is expected to eval the fish commands that come after
/// the real command output. To make this work cleanly, the fish wrapper
/// sources the output, so we separate command output (printed to stderr
/// for the user to see) from fish commands (printed to stdout for eval).
pub fn bash_exec(command: &str) -> i32 {
    let before = EnvSnapshot::capture_current();

    // Run the user's command in bash with output to stderr (so user sees it),
    // then dump env to stdout (for fish to eval).
    let script = build_script(&shell_escape_for_bash(command), " >&2", true);

    let output = match Command::new("bash")
        .args(["-c", &script])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("reef: failed to run bash: {e}");
            return 1;
        }
    };

    let exit_code = output.status.code().unwrap_or(1);
    diff_and_print_env(&before, &output.stdout);
    exit_code
}

/// Execute a command through bash and only print environment diff as
/// fish commands. No command output is shown. Used by `source.fish`
/// to source bash scripts and capture their environment side effects.
pub fn bash_exec_env_diff(command: &str) -> i32 {
    let before = EnvSnapshot::capture_current();

    // Run the command and capture env afterward — all in one bash invocation.
    // Suppress command stdout/stderr since we only want the env diff.
    let script = build_script(&shell_escape_for_bash(command), " >/dev/null 2>&1", false);

    let output = match Command::new("bash").args(["-c", &script]).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("reef: failed to run bash: {e}");
            return 1;
        }
    };

    diff_and_print_env(&before, &output.stdout);

    if output.status.success() {
        0
    } else {
        output.status.code().unwrap_or(1)
    }
}

/// Parse env data from bash stdout (after sentinel markers), diff against
/// the before snapshot, and print fish `set` commands to stdout.
fn diff_and_print_env(before: &EnvSnapshot, raw_stdout: &[u8]) {
    let stdout = String::from_utf8_lossy(raw_stdout);

    let env_start = stdout.find(ENV_MARKER);
    let cwd_start = stdout.find(CWD_MARKER);

    if let (Some(env_pos), Some(cwd_pos)) = (env_start, cwd_start) {
        let env_section = &stdout[env_pos + ENV_MARKER.len()..cwd_pos];
        let cwd_section = stdout[cwd_pos + CWD_MARKER.len()..].trim();

        let after = EnvSnapshot {
            vars: env_diff::parse_null_separated_env(env_section),
            cwd: cwd_section.to_string(),
        };

        let commands = before.diff(&after);
        if commands.is_empty() {
            return;
        }
        // Build single buffer and write once to minimize syscalls
        let total_len: usize = commands.iter().map(|c| c.len() + 1).sum();
        let mut buf = String::with_capacity(total_len);
        for cmd in &commands {
            buf.push_str(cmd);
            buf.push('\n');
        }
        let _ = io::stdout().lock().write_all(buf.as_bytes());
    }
}

/// Build a bash script that evals the command with the given redirect suffix,
/// then dumps env markers + env -0 + cwd for the diff.
fn build_script(escaped_cmd: &str, redirect: &str, track_exit: bool) -> String {
    let mut s = String::with_capacity(escaped_cmd.len() + 100);
    s.push_str("eval ");
    s.push_str(escaped_cmd);
    s.push_str(redirect);
    s.push('\n');
    if track_exit {
        s.push_str("__reef_exit=$?\n");
    }
    s.push_str("echo '");
    s.push_str(ENV_MARKER);
    s.push_str("'\nenv -0\necho '");
    s.push_str(CWD_MARKER);
    s.push_str("'\npwd");
    if track_exit {
        s.push_str("\nexit $__reef_exit");
    }
    s
}

/// Escape a command string for embedding in a bash `eval` statement.
/// We single-quote the entire thing to prevent any interpretation.
fn shell_escape_for_bash(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 2);
    result.push('\'');
    for &b in s.as_bytes() {
        if b == b'\'' {
            result.push_str("'\\''");
        } else {
            result.push(b as char);
        }
    }
    result.push('\'');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_simple() {
        assert_eq!(shell_escape_for_bash("echo hello"), "'echo hello'");
    }

    #[test]
    fn shell_escape_with_quotes() {
        assert_eq!(
            shell_escape_for_bash("echo 'it'\"s\""),
            "'echo '\\''it'\\''\"s\"'"
        );
    }

    #[test]
    fn bash_exec_sets_var() {
        // Run a command that exports a unique variable
        let code = bash_exec("export __REEF_TEST_VAR_xyzzy=hello_reef");
        // The command should succeed
        assert_eq!(code, 0);
    }

    #[test]
    fn bash_exec_env_diff_captures_var() {
        // This test verifies that bash_exec_env_diff runs without error
        let code = bash_exec_env_diff("export __REEF_TEST_ED_VAR=test_val");
        assert_eq!(code, 0);
    }

    #[test]
    fn bash_exec_preserves_exit_code() {
        let code = bash_exec("exit 42");
        assert_eq!(code, 42);
    }

    #[test]
    fn bash_exec_exit_code_zero() {
        let code = bash_exec("true");
        assert_eq!(code, 0);
    }
}
