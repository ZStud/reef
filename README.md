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
yay -S reef-tools        # modern tool wrappers (grep→rg, find→fd, ls→eza, etc.)
```

The two packages are independent — install either or both.

### From source
```bash
git clone https://github.com/ZStud/reef
cd reef
cargo build --release
fish install.fish              # core only
fish install.fish --tools      # also install tool wrappers
```

The install script places the binary and fish functions in the right locations. No configuration needed.

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

**Tier 2 — AST Translation** (~1ms)
A Rust-powered parser converts bash syntax to fish equivalents before execution.
```
for i in $(seq 5); do echo $i; done  →  for i in (seq 5); echo $i; end
if [[ -n "$HOME" ]]; then echo y; fi →  if test -n "$HOME"; echo y; end
echo $((2 + 2))                      →  echo (math "2 + 2")
${var:-default}                      →  (set -q var; and echo $var; or echo "default")
```

**Tier 3 — Bash Passthrough** (~3ms)
Anything too complex to translate runs through bash directly. Environment changes are captured and applied back to your fish session.
```
source <(kubectl completion bash)    →  runs in bash, env synced to fish
declare -A map=([k]=v)               →  runs in bash, output streamed
```

Every tier falls back to the next. Nothing breaks — the worst case is 3ms of latency, which is still faster than zsh's startup time.

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

248 out of 251 bash constructs pass in the test suite. The 3 remaining are environment-specific (an unimplemented tool wrapper and missing dependencies), not translation failures.

| Category | Examples | Tier |
|---|---|---|
| Variables & export | `export`, `unset`, `declare`, `local` | 1 |
| Command substitution | `$(cmd)`, `` `cmd` ``, nested | 2 |
| Conditionals | `if/then/fi`, `[[ ]]`, `[ ]`, `test` | 2 |
| Loops | `for/do/done`, `while`, `until`, C-style `for` | 2 |
| Arithmetic | `$(( ))`, `(( ))`, `let` | 2 |
| Parameter expansion | `${:-}`, `${%%}`, `${//}`, `${#}`, `${^^}` | 2-3 |
| Case statements | `case/esac` with patterns and fall-through | 2 |
| Functions | `function f()`, local vars, return values | 2 |
| Redirections | `2>&1`, `&>`, `>&`, append, fd manipulation | 2-3 |
| Heredocs & herestrings | `<<EOF`, `<<<`, quoted delimiters | 3 |
| Process substitution | `<()`, `>()` | 3 |
| Arrays | Indexed and associative | 3 |
| Traps & signals | `trap`, `EXIT`, `ERR` | 3 |
| Coprocesses | `coproc`, named coprocesses | 3 |
| Namerefs | `declare -n` | 3 |
| Special variables | `$?`, `$$`, `$!`, `$RANDOM`, `$SECONDS` | 2-3 |
| Real-world patterns | nvm, conda, pyenv, curl\|bash, eval | 1-3 |

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
| Tier 2 detection | <0.5ms | Every Enter keypress |
| Tier 2 AST translation | ~1ms | Bash syntax detected |
| Tier 3 bash passthrough | ~3ms | Complex/untranslatable bash |

The binary is 1.2MB with LTO and strip. Detection runs on every keypress and adds zero perceptible latency. Even the slowest path (3ms passthrough) is faster than zsh's startup time with oh-my-zsh.

---

## FAQ

**Does this slow down fish?**
No. Detection is sub-millisecond string matching — no regex, no parsing. If your command is native fish, reef adds zero overhead. You literally cannot perceive it.

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
bash tests/reef_test_suite.sh
```

If you find a bash construct that doesn't work, open an issue with the command and expected output. That becomes a test case and a fix.

---

## License

MIT
