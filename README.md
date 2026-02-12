# reef

**Bash compatibility for fish shell. Just works.**

Reef makes bash syntax work seamlessly inside [fish](https://fishshell.com). No prefix commands, no mode switching, no learning curve. You type bash — fish runs it.

```
❯ export PATH="/opt/bin:$PATH"                          # just works
❯ for f in *.log; do wc -l "$f"; done                   # just works
❯ source ~/.nvm/nvm.sh && nvm use 18                     # just works
❯ echo "Hello $(whoami), it's $((2+2)) o'clock"         # just works
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
yay -S reef-tools        # modern tool wrappers (grep→rg, find→fd, cat→bat, cd→zoxide, etc.)
```

The two packages are independent — install either or both. `reef-tools` installs the wrappers only — the modern tools themselves are optional dependencies. Install whichever ones you want:

```
paru -S ripgrep fd sd dust procs eza bat zoxide   # all of them
paru -S bat zoxide                                 # or just the ones you want
```

Wrappers for tools you haven't installed simply pass through to the original command.

### From source
```bash
git clone https://github.com/ZStud/reef
cd reef
cargo build --release
fish fish/install.fish              # core only
fish fish/install.fish --tools      # also install tool wrappers
```

The install script places the binary and fish functions in the right locations. No configuration needed.

### Uninstall

**AUR:**
```
yay -R reef              # removes everything
yay -R reef-tools        # removes tool wrappers
```

**From source:**
```bash
fish fish/install.fish --uninstall
cargo uninstall reef      # if installed via cargo install
```

---

## How It Works

Reef uses three tiers, each faster than the last:

**Tier 1 — Keyword Wrappers** (<0.1ms)
Fish functions that handle common bash builtins natively.
```
export VAR=val     →  set -gx VAR val
unset VAR          →  set -e VAR
source script.sh   →  bash sourcing with env capture
```

**Tier 2 — AST Translation** (~0.4ms)
A custom zero-copy Rust parser converts bash syntax to fish equivalents before execution. Zero dependencies, zero allocations.
```
for i in $(seq 5); do echo $i; done  →  for i in (seq 5); echo $i; end
if [[ -n "$HOME" ]]; then echo y; fi →  if test -n "$HOME"; echo y; end
echo $((2 + 2))                      →  echo (math "2 + 2")
${var:-default}                      →  (set -q var; and echo $var; or echo "default")
```

**Tier 3 — Bash Passthrough** (~1.6ms)
Anything too complex to translate runs through bash directly. Environment changes are captured and applied back to your fish session.
```
declare -A map=([k]=v)               →  runs in bash, output streamed
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
```

---

## What's Covered

484 tests covering bash constructs across all tiers.

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
❯ grep -ri "TODO" src/           # actually runs: rg -i "TODO" src/
❯ find . -name "*.rs" -type f    # actually runs: fd -e rs -t f .
❯ sed -i 's/foo/bar/g' file.txt  # actually runs: sd -i 'foo' 'bar' file.txt
❯ ls -ltr                        # actually runs: eza -l --sort=modified --reverse
❯ du -sh /var                    # actually runs: dust -d 1 /var
❯ ps aux                         # actually runs: procs (shows all by default)
❯ cat config.yaml                # actually runs: bat (highlighting, line numbers, pager)
❯ cd proj                        # actually runs: zoxide (fuzzy-matches to ~/Projects)
```

The wrappers handle combined flags (`-sh` → `-s` + `-h`), flags with arguments (`-A 3`, `--max-depth=2`), `--include`/`--exclude` glob translation, and more. `--version` and `--help` pass through to the original tool so nothing surprises you. If a wrapper hits a flag it doesn't recognize, it falls back to the real GNU tool automatically — nothing breaks.

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

The binary is 468KB with zero dependencies, LTO, and strip. Detection runs on every keypress and adds zero perceptible latency. Even the slowest path (1.6ms passthrough) is faster than zsh's startup time with oh-my-zsh.

---

## v0.2 — What Changed

Custom zero-copy parser replacing conch-parser, full optimization pass across every subsystem. See the [wiki](https://github.com/ZStud/reef/wiki/v0.2-Custom-Parser-&-Optimization-Pass) for the complete changelog.

**Highlights:**
- **Binary:** 1.16MB → 468KB (60% smaller)
- **Dependencies:** 23 crates → 0
- **Tests:** 130 → 484
- **AST translation:** ~1ms → ~0.4ms
- **Bash passthrough:** ~3ms → ~1.6ms
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

## Contributing

Run the test suite:
```bash
cargo test
```

If you find a bash construct that doesn't work, open an issue with the command and expected output. That becomes a test case and a fix.

---

## License

MIT
