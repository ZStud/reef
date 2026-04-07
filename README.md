# reef

**Bash compatibility for fish shell. Just works.**

Reef makes bash syntax work seamlessly inside [fish](https://fishshell.com). No prefix commands, no mode switching, no learning curve. You type bash — fish runs it.

```
> export PATH="/opt/bin:$PATH"                          # just works
> for f in *.log; do wc -l "$f"; done                   # just works
> source ~/.nvm/nvm.sh && nvm use 18                     # just works
> echo "Hello $(whoami), it's $((2+2)) o'clock"         # just works
```

Fish is the fastest, friendliest shell out there. The only reason people don't switch is bash compatibility. Reef fixes that.

---

## Why

Fish has better autosuggestions, syntax highlighting, completions, and startup time than bash or zsh — all out of the box, zero configuration. But paste a command from Stack Overflow and it breaks. `export`, `$()`, `for/do/done`, `[[ ]]` — none of it works in fish natively.

Every other solution requires you to change your behavior:
- **bass** — prefix every command with `bass`
- **zsh** — spend an hour configuring plugins to match fish's defaults
- **just learn fish syntax** — the fish community's advice for 20 years

Reef requires nothing. You install it and forget it exists.

---

## Install

### Arch / CachyOS (AUR)
```
yay -S reef              # bash compatibility layer
yay -S reef-tools        # modern tool wrappers (grep->rg, find->fd, cat->bat, cd->zoxide, etc.)
```

The two packages are independent — install either or both. `reef-tools` installs the wrappers only — the modern tools themselves are optional dependencies. Install whichever ones you want:

```
yay -S ripgrep fd sd dust procs eza bat zoxide   # all of them
yay -S bat zoxide                                 # or just the ones you want
```

Wrappers for tools you haven't installed simply pass through to the original command.

### Homebrew (macOS / Linux)
```
brew tap ZStud/reef
brew install reef
```

homebrew-core requires 225+ stars (or 90+ forks/watchers) for new formulae — once reef hits that, `brew install reef` will work without tapping.

### Fedora / RHEL (Copr)
```
sudo dnf copr enable zstud/reef
sudo dnf install reef                # bash compatibility layer
sudo dnf install reef-tools          # optional: modern tool wrappers
```

### Nix
```
nix profile install github:ZStud/reef
nix profile install github:ZStud/reef#reef-tools   # optional tool wrappers
```

Or as a flake input:
```nix
{ inputs.reef.url = "github:ZStud/reef"; }
```

Also available via nixpkgs (v0.3.0 update pending):
```
nix-env -iA nixpkgs.reef
```

### Cargo (crates.io)
```
cargo install reef-shell
```

### From source
```bash
git clone https://github.com/ZStud/reef
cd reef
cargo build --release
fish fish/install.fish              # core only
fish fish/install.fish --tools      # also install tool wrappers
```

The install script places the binary and fish functions in the right locations. No configuration needed.

> **Why no Debian/Ubuntu package?** Reef uses Rust edition 2024, which requires Rust 1.85+. Ubuntu ships Rust 1.82 (as of Plucky 25.04), so a native PPA build isn't possible yet. Debian/Ubuntu users can install via Homebrew, Nix, `cargo install reef-shell`, or from source.

### Uninstall

**AUR:**
```
yay -R reef reef-tools
```

**Homebrew:**
```
brew uninstall reef && brew untap ZStud/reef
```


**Fedora / RHEL:**
```
sudo dnf remove reef reef-tools
sudo dnf copr remove zstud/reef
```

**Nix:**
```
nix profile remove reef
```

**Cargo:**
```
cargo uninstall reef-shell
```

**From source:**
```bash
fish fish/install.fish --uninstall
```

---

## How It Works

Reef uses three tiers, each faster than the last:

**Tier 1 — Keyword Wrappers** (<0.1ms)
Fish functions that handle common bash builtins natively.
```
export VAR=val     ->  set -gx VAR val
unset VAR          ->  set -e VAR
source script.sh   ->  bash sourcing with env capture
```

**Tier 2 — AST Translation** (~0.4ms)
A custom zero-copy Rust parser converts bash syntax to fish equivalents before execution. Zero dependencies, zero allocations.
```
for i in $(seq 5); do echo $i; done  ->  for i in (seq 5); echo $i; end
if [[ -n "$HOME" ]]; then echo y; fi ->  if test -n "$HOME"; echo y; end
echo $((2 + 2))                      ->  echo (math "2 + 2")
${var:-default}                      ->  (set -q var; and echo $var; or echo "default")
```

**Tier 2.5 — Confirm Mode** (optional)
Preview what reef will do before it executes. Shows the fish translation (T2) or indicates bash passthrough (T3), then waits for `[Y/n]` confirmation.
```
> export FOO=bar
  -> fish: set -gx FOO bar
  Execute? [Y/n] y                    # Y or Enter = execute, n = cancel
```

**Tier 3 — Bash Passthrough** (~1.6ms)
Anything too complex to translate runs through bash directly. Environment changes are captured and applied back to your fish session.
```
declare -A map=([k]=v)               ->  runs in bash, output streamed
```

Every tier falls back to the next. Nothing breaks — the worst case is 1.6ms of latency, which is faster than zsh's startup time.

---

## Commands

```
reef on                    # enable reef (default)
reef off                   # disable — raw fish, no translation
reef status                # show current settings
reef display bash          # show commands as you typed them (default)
reef display fish          # show the fish translation on the commandline
reef history bash          # log your original bash input to history (default)
reef history fish          # log translated fish commands to history
reef history both          # log both
reef confirm on            # preview translations before executing
reef confirm off           # execute immediately (default)
reef persist off           # fresh bash each command (default)
reef persist state         # exported vars persist across commands
reef persist full          # persistent bash coprocess — everything persists
```

Any setting command accepts `--silent` to suppress the confirmation echo:

```
reef on --silent           # enable without printing to terminal
reef persist full --silent # start coprocess quietly
reef confirm on --silent   # enable confirm mode quietly
```

The flag works in any position (`reef --silent off`, `reef persist --silent state`, etc.). Status queries and error messages are never silenced.

---

## Confirm Mode

When enabled (`reef confirm on`), reef shows what it will do before executing. You get a `[Y/n]` prompt for every bash command — translations, passthroughs, daemon routes, and source commands.

```
> export FOO=bar
  -> fish: set -gx FOO bar
  Execute? [Y/n]                      # Enter or y = execute

> some_unsupported_bash
  -> bash passthrough (no fish equivalent)
  Execute? [Y/n] n                    # n = cancel, command stays for editing
```

This is off by default — zero overhead when disabled. Useful for learning what reef does, debugging translations, or when you want explicit control over what runs.

---

## Persistence Modes

By default, each bash passthrough command runs in a fresh bash subprocess. Variables, functions, and fds set in one command are gone by the next. Reef v0.3 adds two opt-in persistence modes that let bash state survive across commands.

### `reef persist off` (default)

Current behavior. T1 -> T2 -> T3 pipeline. Each T3 passthrough spawns fresh bash.

### `reef persist state`

After each bash passthrough, reef saves the exported environment to a state file. The next passthrough restores it before running your command. Exported variables persist across commands. T2 translation still handles what it can.

```
> export MY_VAR=hello        # saved to state file
> echo $MY_VAR               # works — env diff synced it to fish
> bash -c 'echo $MY_VAR'     # works — state file restored it in bash
```

**Overhead:** ~0.4ms to source a typical state file (30 vars). **Persists:** exported variables. **Doesn't persist:** unexported vars, fds, traps, functions.

### `reef persist full`

Spawns a long-lived bash process per fish session. All bash-detected commands route through this single process — T2 translation is skipped to keep bash state consistent. Since it's the same process, everything persists.

```
> MY_VAR=hello               # unexported — would normally be lost
> echo $MY_VAR               # works — same bash process
> myfunc() { echo "works"; } # function persists
> myfunc                     # works
> exec 3>&1                  # fd persists
> echo "hello" >&3           # works — same process
```

**Overhead:** ~1.6ms per command, ~2-4MB for the bash process. **Persists:** everything — exported and unexported vars, functions, aliases, traps, fds, cwd.

The daemon is cleaned up automatically when your fish session exits.

---

## What's Covered

498 tests covering bash constructs across all tiers.

| Category | Examples | Tier |
|---|---|---|
| Variables & export | `export`, `unset`, `declare`, `local`, `readonly` | 1 |
| Command substitution | `$(cmd)`, `` `cmd` ``, nested | 2 |
| Conditionals | `if/then/elif/else/fi`, `[[ ]]`, `[ ]`, `test` | 2 |
| Loops | `for/do/done`, `while`, `until`, C-style `for ((i=0;...))` | 2 |
| Arithmetic | `$(( ))`, `(( ))`, bitwise ops, ternary, pre/post inc/dec | 2 |
| Parameter expansion | `${:-}`, `${%%}`, `${//}`, `${#}`, `${^^}`, `${,,}`, `${:offset:len}`, `${!ref}`, `${@Q}` | 2 |
| String replacement | `${var/pat/rep}`, `${var//pat/rep}`, prefix/suffix anchored | 2 |
| Case statements | `case/esac` with patterns, wildcards, char classes | 2 |
| Functions | `name() {}`, `function name {}`, local vars, return | 2 |
| Redirections | `2>&1`, `&>`, `&>>`, `>|`, `<>`, fd manipulation | 2 |
| Here-strings | `<<<` | 2 |
| Heredocs | `<<'EOF'`, `<<"EOF"` | 2 |
| Process substitution | `<(cmd)` | 2 |
| Arrays | `${arr[@]}`, `${#arr[@]}`, `${arr[i]}`, `arr+=()`, slicing | 2 |
| Brace ranges | `{1..10}`, `{a..z}`, `{1..10..2}` | 2 |
| Traps & signals | `trap 'cmd' EXIT`, `trap '' SIGINT` | 2 |
| Special variables | `$?`, `$$`, `$!`, `$@`, `$#`, `$RANDOM` | 2 |
| Real-world patterns | nvm, conda, pyenv, docker, curl\|bash, eval | 2-3 |
| Associative arrays | `declare -A` | 3 |
| Coprocesses | `coproc` | 3 |
| Namerefs | `declare -n` | 3 |

---

## Library

Reef is both a binary and a Rust library. You can embed it in editors, CI pipelines, or your own tools:

```rust
use reef::detect::looks_like_bash;
use reef::translate::translate_bash_to_fish;

if looks_like_bash("export FOO=bar") {
    let fish = translate_bash_to_fish("export FOO=bar").unwrap();
    assert_eq!(fish, "set -gx FOO bar");
}
```

```toml
[dependencies]
reef-shell = "0.3"
```

The library exposes the full public API: detection, parsing, AST types, translation, passthrough execution, env diffing, and daemon control. All types are `#[non_exhaustive]` for forward compatibility. Zero dependencies.

---

## reef-tools — Drop-In Modern Tool Replacements

`reef-tools` is a separate package that swaps legacy coreutils for faster, modern alternatives — transparently. These aren't simple aliases. Each wrapper fully mimics the original tool's flag interface, translating GNU flags to their modern equivalents so your muscle memory, scripts, and Stack Overflow commands all keep working.

| You type | Runs under the hood | Why |
|---|---|---|
| `grep` | [ripgrep](https://github.com/BurntSushi/ripgrep) | 5-10x faster. Respects `.gitignore`. Recursive by default. Unicode-aware. |
| `find` | [fd](https://github.com/sharkdp/fd) | Simpler syntax, 5x faster, ignores `.git`/`node_modules` by default. Colorized output. |
| `sed` | [sd](https://github.com/chmln/sd) | Sane regex syntax (no escaping groups). 2-10x faster on large files. |
| `ls` | [eza](https://github.com/eza-community/eza) | Git-aware. Colors and icons. Tree view with `-R`. Human sizes by default. |
| `du` | [dust](https://github.com/bootandy/dust) | Visual bar chart of disk usage. Instant overview instead of a wall of numbers. |
| `ps` | [procs](https://github.com/dalance/procs) | Colorized. Searchable. Shows ports, Docker containers, tree view. |
| `cat` | [bat](https://github.com/sharkdp/bat) | Syntax highlighting. Line numbers. Git diff markers. Auto-paging. Pipe-safe. |
| `cd` | [zoxide](https://github.com/ajeetdsouza/zoxide) | Learns your habits. Fuzzy-matches directories. `cd proj` jumps to `/home/you/Projects`. |

```
> grep -ri "TODO" src/           # actually runs: rg -i "TODO" src/
> find . -name "*.rs" -type f    # actually runs: fd -e rs -t f .
> sed -i 's/foo/bar/g' file.txt  # actually runs: sd -i 'foo' 'bar' file.txt
> ls -ltr                        # actually runs: eza -l --sort=modified --reverse
> du -sh /var                    # actually runs: dust -d 1 /var
> ps aux                         # actually runs: procs (shows all by default)
> cat config.yaml                # actually runs: bat (highlighting, line numbers, pager)
> cd proj                        # actually runs: zoxide (fuzzy-matches to ~/Projects)
```

The wrappers handle combined flags (`-sh` -> `-s` + `-h`), flags with arguments (`-A 3`, `--max-depth=2`), `--include`/`--exclude` glob translation, and more. `--version` and `--help` pass through to the original tool so nothing surprises you. If a wrapper hits a flag it doesn't recognize, it falls back to the real GNU tool automatically — nothing breaks.

---

## .bashrc Compatibility

Tools like nvm, conda, pyenv, and rustup write initialization lines to `~/.bashrc`. Reef auto-sources this on shell startup:

```fish
# conf.d/reef.fish (installed automatically)
if test -f ~/.bashrc
    reef bash-exec --env-diff -- "source ~/.bashrc" | source
end
```

Every `export PATH=...` and `eval "$(tool init)"` that any installer ever appended to your `.bashrc` works in fish automatically.

---

## Performance

| Path | Latency | When |
|---|---|---|
| Native fish | 0ms | Valid fish syntax |
| Tier 1 keyword wrapper | <0.1ms | `export`, `unset`, `source` |
| Tier 2 detection | ~0.4ms | Every Enter keypress |
| Tier 2 AST translation | ~0.4ms | Bash syntax detected |
| Tier 3 bash passthrough | ~1.6ms | Complex/untranslatable bash |

The binary is ~490KB with zero dependencies, LTO, and strip. Detection runs on every keypress and adds zero perceptible latency. Even the slowest path (1.6ms passthrough) is faster than zsh's startup time with oh-my-zsh.

---

## v0.3 — What's New

**Persistence modes** — bash state survives across commands:
- `reef persist state` — exported variables persist via state file (~0.4ms overhead)
- `reef persist full` — persistent bash coprocess, everything persists (~1.6ms, ~2-4MB)
- Automatic daemon lifecycle management (start on enable, cleanup on fish exit)

**Confirm mode** — preview before execution:
- `reef confirm on` — shows translation or passthrough type with `[Y/n]` prompt
- Works on all paths: T2 translation, T3 passthrough, daemon, source
- Zero overhead when off (default)

**Library crate** — reef is now a binary + library:
- `use reef::translate::translate_bash_to_fish` — embed in editors, CI, tools
- Full public API: detection, parsing, AST, translation, passthrough, daemon
- `#![deny(missing_docs)]`, `#![forbid(unsafe_code)]`, `#![warn(clippy::all)]`

**Code quality audit:**
- `#[must_use]` on all public functions returning values
- `#[non_exhaustive]` on all public enums
- `Hash` derive on `Param` and `ParseError`
- `# Panics` doc sections on functions with `.expect()`
- `# Examples` doc tests on all public entry points (compile-verified)
- `EnvSnapshot::diff_into()` — zero intermediate allocations
- 498 unit tests + 11 doc tests, zero clippy warnings

## v0.2

Custom zero-copy parser replacing conch-parser, full optimization pass across every subsystem. See the [wiki](https://github.com/ZStud/reef/wiki/v0.2-Custom-Parser-&-Optimization-Pass) for the complete changelog.

**Highlights:**
- **Binary:** 1.16MB -> 468KB (60% smaller)
- **Dependencies:** 23 crates -> 0
- **Tests:** 130 -> 484
- **AST translation:** ~1ms -> ~0.4ms
- **Bash passthrough:** ~3ms -> ~1.6ms
- Custom recursive-descent parser (lexer + AST + parser, 3,300 lines)
- Zero-copy `&'a str` AST — no allocations during parse
- Rust edition 2024, zero clippy warnings
- Fixed detection false positives on single-quoted strings
- Fixed parser infinite loop on empty case arm bodies

---

## FAQ

**Does this slow down fish?**
No. Detection is a sub-millisecond byte scan — no regex, no parsing. If your command is native fish, reef adds zero overhead. You literally cannot perceive it.

**Does this change fish's behavior?**
No. Fish is still fish. Your prompt, completions, autosuggestions, syntax highlighting, and config all work exactly the same. Reef only activates when it detects bash syntax.

**What if reef gets something wrong?**
`reef off` disables it instantly. You're back to raw fish. Report the issue, we'll fix it, `reef on` when the update lands.

**Why not just use zsh?**
Zsh is bash-compatible but requires extensive configuration to match fish's out-of-box experience. Fish + reef gives you both: bash compatibility and the best interactive shell, zero configuration.

**Why not just use bass?**
Bass requires prefixing every command with `bass`. Reef is automatic — you type bash and it works. No prefix, no mode switch, no thought.

**Will the fish team support this?**
Reef is an independent project. It uses fish's public APIs (functions, keybindings, `commandline` builtin) and doesn't modify fish internals. It works with fish, not against it.

---

## Limitations

These are fundamental to how shell bridging works — not just reef, but any tool (bass, foreign-env, etc.) that runs bash from fish.

**Unexported variables don't persist** (unless using `reef persist full`). Reef runs bash commands in a subprocess and captures environment changes via `env`. Only exported variables appear in `env`, so `FOO=bar` (no `export`) set through bash passthrough won't persist in your fish session. `export FOO=bar` works fine. With `reef persist full`, all variables persist in the long-lived bash coprocess.

**File descriptor manipulation is single-command** (unless using `reef persist full`). `exec 3>&1 4>&2` works when it's part of a single line, but fd state can't cross process boundaries. With `reef persist full`, fds persist because it's the same bash process.

**Subshell isolation is real** (unless using `reef persist full`). Commands run through Tier 3 passthrough execute in a bash subprocess. Side effects that aren't environment variables don't carry over. `reef persist full` removes this limitation entirely.

In practice, these rarely matter. The vast majority of copy-pasted bash — `export`, loops, pipes, conditionals, `curl | bash`, nvm/conda/pyenv init — works without hitting any of these limits. And `reef persist full` eliminates them for power users.

---

## Contributing

Reef is a library + binary crate with compile-time enforcement:
- `#![forbid(unsafe_code)]` — no unsafe anywhere
- `#![deny(missing_docs)]` — every public item must be documented
- `#![warn(clippy::all)]` — clippy clean

Run the test suite:
```bash
cargo test          # 498 unit tests + 11 doc tests
cargo clippy        # must be warning-free
```

If you find a bash construct that doesn't work, open an issue with the command and expected output. That becomes a test case and a fix.

---

## License

MIT
