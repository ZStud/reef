mod detect;
mod env_diff;
mod passthrough;
mod translate;

use clap::{Parser, Subcommand};
use std::process;

#[derive(Parser)]
#[command(name = "reef", about = "Bash compatibility layer for fish shell")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Quick-check if input looks like bash syntax
    Detect {
        /// Use fast string-matching only (no parsing)
        #[arg(long)]
        quick: bool,
        /// The command string to check
        #[arg(last = true)]
        input: Vec<String>,
    },
    /// Translate bash syntax to fish
    Translate {
        /// The bash command to translate
        #[arg(last = true)]
        input: Vec<String>,
    },
    /// Execute a command through bash with environment capture
    BashExec {
        /// Only capture and print environment diff (no streaming output)
        #[arg(long)]
        env_diff: bool,
        /// The command to execute
        #[arg(last = true)]
        input: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Detect { quick, input } => {
            let cmd = input.join(" ");
            if quick {
                if detect::looks_like_bash(&cmd) {
                    process::exit(0);
                } else {
                    process::exit(1);
                }
            } else {
                // Full detection (parse attempt) — not yet implemented
                if detect::looks_like_bash(&cmd) {
                    process::exit(0);
                } else {
                    process::exit(1);
                }
            }
        }
        Commands::Translate { input } => {
            let cmd = input.join(" ");
            match translate::translate_bash_to_fish(&cmd) {
                Ok(fish_code) => {
                    print!("{}", fish_code);
                }
                Err(e) => {
                    eprintln!("reef: translation failed: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::BashExec { env_diff, input } => {
            let cmd = input.join(" ");
            let exit_code = if env_diff {
                passthrough::bash_exec_env_diff(&cmd)
            } else {
                passthrough::bash_exec(&cmd)
            };
            process::exit(exit_code);
        }
    }
}
