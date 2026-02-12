mod ast;
mod detect;
mod env_diff;
mod lexer;
mod parser;
mod passthrough;
mod translate;

use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("usage: reef <detect|translate|bash-exec> [flags] -- <command>");
        process::exit(2);
    }

    match args[1].as_str() {
        "detect" => {
            // --quick and full detection use the same heuristic for now;
            // full AST-based detection may be added later.
            let cmd = collect_after_dashdash(&args[2..]);
            if detect::looks_like_bash(&cmd) {
                process::exit(0);
            } else {
                process::exit(1);
            }
        }
        "translate" => {
            let cmd = collect_after_dashdash(&args[2..]);
            match translate::translate_bash_to_fish(&cmd) {
                Ok(fish_code) => print!("{fish_code}"),
                Err(e) => {
                    eprintln!("reef: translation failed: {e}");
                    process::exit(1);
                }
            }
        }
        "bash-exec" => {
            let env_diff = args[2..].iter().any(|a| a == "--env-diff");
            let cmd = collect_after_dashdash(&args[2..]);
            let exit_code = if env_diff {
                passthrough::bash_exec_env_diff(&cmd)
            } else {
                passthrough::bash_exec(&cmd)
            };
            process::exit(exit_code);
        }
        other => {
            eprintln!("reef: unknown command '{other}'");
            process::exit(2);
        }
    }
}

/// Collect all arguments after `--` into a single string.
fn collect_after_dashdash(args: &[String]) -> String {
    if let Some(pos) = args.iter().position(|a| a == "--") {
        args[pos + 1..].join(" ")
    } else {
        args.join(" ")
    }
}
