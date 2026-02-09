use std::collections::HashMap;
use std::process::Command;

/// Sentinel markers used to separate sections in bash output.
const ENV_MARKER: &str = "__REEF_ENV_MARKER_5f3a__";
const CWD_MARKER: &str = "__REEF_CWD_MARKER_5f3a__";

/// Variables that are internal to bash and should not be synced to fish.
const SKIP_VARS: &[&str] = &[
    "BASH",
    "BASHOPTS",
    "BASHPID",
    "BASH_ALIASES",
    "BASH_ARGC",
    "BASH_ARGV",
    "BASH_CMDS",
    "BASH_COMMAND",
    "BASH_EXECUTION_STRING",
    "BASH_LINENO",
    "BASH_LOADABLES_PATH",
    "BASH_REMATCH",
    "BASH_SOURCE",
    "BASH_SUBSHELL",
    "BASH_VERSINFO",
    "BASH_VERSION",
    "COLUMNS",
    "COMP_WORDBREAKS",
    "DIRSTACK",
    "EUID",
    "FUNCNAME",
    "GROUPS",
    "HISTCMD",
    "HISTFILE",
    "HOSTNAME",
    "HOSTTYPE",
    "IFS",
    "LINES",
    "MACHTYPE",
    "MAILCHECK",
    "OPTERR",
    "OLDPWD",
    "OPTIND",
    "OSTYPE",
    "PIPESTATUS",
    "PPID",
    "PS1",
    "PS2",
    "PS4",
    "PWD",
    "RANDOM",
    "SECONDS",
    "SHELL",
    "SHELLOPTS",
    "SHLVL",
    "UID",
    "_",
];

/// A snapshot of the shell environment at a point in time.
#[derive(Debug)]
pub struct EnvSnapshot {
    pub vars: HashMap<String, String>,
    pub cwd: String,
}

impl EnvSnapshot {
    /// Capture just the current environment (no user command).
    pub fn capture_current() -> Result<Self, String> {
        let script = format!(
            "echo '{}'\nenv -0\necho '{}'\npwd",
            ENV_MARKER, CWD_MARKER,
        );

        let output = Command::new("bash")
            .args(["-c", &script])
            .output()
            .map_err(|e| format!("failed to run bash: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        let env_start = stdout.find(ENV_MARKER);
        let cwd_start = stdout.find(CWD_MARKER);

        let (vars, cwd) = match (env_start, cwd_start) {
            (Some(env_pos), Some(cwd_pos)) => {
                let env_section = &stdout[env_pos + ENV_MARKER.len()..cwd_pos];
                let cwd_section = stdout[cwd_pos + CWD_MARKER.len()..].trim();
                (parse_null_separated_env(env_section), cwd_section.to_string())
            }
            _ => (HashMap::new(), String::new()),
        };

        Ok(EnvSnapshot { vars, cwd })
    }

    /// Diff two snapshots, returning fish commands to apply the changes.
    ///
    /// Returns commands like:
    ///   set -gx VAR value
    ///   set -e VAR
    ///   cd /new/path
    pub fn diff(&self, after: &EnvSnapshot) -> Vec<String> {
        let mut commands = Vec::new();

        // New or changed variables
        for (key, new_val) in &after.vars {
            if should_skip_var(key) {
                continue;
            }

            let changed = match self.vars.get(key) {
                Some(old_val) => old_val != new_val,
                None => true,
            };

            if changed {
                // PATH-like variables: split on : for fish list semantics
                if key.ends_with("PATH") && new_val.contains(':') {
                    let parts: Vec<&str> = new_val.split(':').collect();
                    let fish_val = parts.join(" ");
                    commands.push(format!("set -gx {} {}", key, fish_val));
                } else {
                    commands.push(format!("set -gx {} {}", key, shell_escape(new_val)));
                }
            }
        }

        // Removed variables
        for key in self.vars.keys() {
            if should_skip_var(key) {
                continue;
            }
            if !after.vars.contains_key(key) {
                commands.push(format!("set -e {}", key));
            }
        }

        // Changed directory
        if !after.cwd.is_empty() && self.cwd != after.cwd {
            commands.push(format!("cd {}", shell_escape(&after.cwd)));
        }

        commands
    }
}

/// Parse null-separated environment output (from `env -0`).
pub fn parse_null_separated_env(data: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();

    // env -0 outputs VAR=value\0VAR=value\0...
    for entry in data.split('\0') {
        let entry = entry.trim_start_matches('\n');
        if entry.is_empty() {
            continue;
        }
        if let Some(eq_pos) = entry.find('=') {
            let key = &entry[..eq_pos];
            let value = &entry[eq_pos + 1..];
            // Skip entries that don't look like valid variable names
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                vars.insert(key.to_string(), value.to_string());
            }
        }
    }

    vars
}

/// Check if a variable should be skipped during env sync.
fn should_skip_var(name: &str) -> bool {
    SKIP_VARS.contains(&name)
}

/// Escape a string for safe use in fish shell commands.
fn shell_escape(s: &str) -> String {
    // If it's simple (alphanumeric, slashes, dots, hyphens), no quoting needed
    if s.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ':' | '~' | '+' | ','))
    {
        return s.to_string();
    }
    // Otherwise, single-quote it (escaping any internal single quotes)
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_null_env() {
        let data = "FOO=bar\0BAZ=qux\0MULTI=hello world\0";
        let vars = parse_null_separated_env(data);
        assert_eq!(vars.get("FOO").unwrap(), "bar");
        assert_eq!(vars.get("BAZ").unwrap(), "qux");
        assert_eq!(vars.get("MULTI").unwrap(), "hello world");
    }

    #[test]
    fn diff_new_var() {
        let before = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/home".to_string(),
        };
        let mut after_vars = HashMap::new();
        after_vars.insert("NEW_VAR".to_string(), "hello".to_string());
        let after = EnvSnapshot {
            vars: after_vars,
            cwd: "/home".to_string(),
        };

        let cmds = before.diff(&after);
        assert!(cmds.iter().any(|c| c.contains("set -gx NEW_VAR")));
    }

    #[test]
    fn diff_removed_var() {
        let mut before_vars = HashMap::new();
        before_vars.insert("OLD_VAR".to_string(), "gone".to_string());
        let before = EnvSnapshot {
            vars: before_vars,
            cwd: "/home".to_string(),
        };
        let after = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/home".to_string(),
        };

        let cmds = before.diff(&after);
        assert!(cmds.iter().any(|c| c == "set -e OLD_VAR"));
    }

    #[test]
    fn diff_changed_cwd() {
        let before = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/home".to_string(),
        };
        let after = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/tmp".to_string(),
        };

        let cmds = before.diff(&after);
        assert!(cmds.iter().any(|c| c.contains("cd /tmp")));
    }

    #[test]
    fn diff_path_split() {
        let before = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/home".to_string(),
        };
        let mut after_vars = HashMap::new();
        after_vars.insert("PATH".to_string(), "/usr/bin:/usr/local/bin".to_string());
        let after = EnvSnapshot {
            vars: after_vars,
            cwd: "/home".to_string(),
        };

        let cmds = before.diff(&after);
        let path_cmd = cmds.iter().find(|c| c.contains("PATH")).unwrap();
        assert!(path_cmd.contains("/usr/bin /usr/local/bin"));
    }

    #[test]
    fn skip_bash_internal_vars() {
        let before = EnvSnapshot {
            vars: HashMap::new(),
            cwd: "/home".to_string(),
        };
        let mut after_vars = HashMap::new();
        after_vars.insert("BASH_VERSION".to_string(), "5.2.0".to_string());
        after_vars.insert("REAL_VAR".to_string(), "keep".to_string());
        let after = EnvSnapshot {
            vars: after_vars,
            cwd: "/home".to_string(),
        };

        let cmds = before.diff(&after);
        assert!(!cmds.iter().any(|c| c.contains("BASH_VERSION")));
        assert!(cmds.iter().any(|c| c.contains("REAL_VAR")));
    }

    #[test]
    fn shell_escape_simple() {
        assert_eq!(shell_escape("/usr/bin"), "/usr/bin");
        assert_eq!(shell_escape("hello"), "hello");
    }

    #[test]
    fn shell_escape_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn shell_escape_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn capture_current_env() {
        // This test actually runs bash — integration test
        let snap = EnvSnapshot::capture_current().unwrap();
        assert!(!snap.vars.is_empty());
        assert!(!snap.cwd.is_empty());
        assert!(snap.vars.contains_key("HOME"));
    }
}
