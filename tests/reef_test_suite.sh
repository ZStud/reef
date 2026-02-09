#!/usr/bin/env bash
# ============================================================================
# REEF COMPREHENSIVE TEST SUITE
# ============================================================================
# Usage: bash tests/reef_test_suite.sh
#
# Runs each bash command through:
#   1. bash -c  → expected output
#   2. reef translate → fish -c  → actual output (Tier 1)
#   3. reef bash-exec → actual output (Tier 2 fallback)
#
# Results written to: tests/reef_results.txt
# Paste that file to Claude for analysis.
# ============================================================================

set -o pipefail

# --- Colors ---
G=$'\033[32m'    # green
R=$'\033[31m'    # red
Y=$'\033[33m'    # yellow
C=$'\033[36m'    # cyan
D=$'\033[2m'     # dim
B=$'\033[1m'     # bold
N=$'\033[0m'     # reset

# --- Counters ---
PASS=0
FAIL=0
T2_OK=0
T2_DIFF=0
SKIP=0
TOTAL=0

# --- Paths ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RESULTS="$SCRIPT_DIR/reef_results.txt"

# Find reef binary
if command -v reef &>/dev/null; then
    REEF=reef
elif [[ -x "$SCRIPT_DIR/../target/release/reef" ]]; then
    REEF="$SCRIPT_DIR/../target/release/reef"
else
    echo "Error: reef binary not found. Run 'cargo build --release' first."
    exit 1
fi

if ! command -v fish &>/dev/null; then
    echo "Error: fish shell not found."
    exit 1
fi

# --- Results file ---
echo "reef Test Suite Results" > "$RESULTS"
echo "=======================" >> "$RESULTS"
echo "Date: $(date)" >> "$RESULTS"
echo "reef: $($REEF --version 2>/dev/null || echo unknown)" >> "$RESULTS"
echo "fish: $(fish --version 2>/dev/null)" >> "$RESULTS"
echo "bash: $(bash --version | head -1)" >> "$RESULTS"
echo "" >> "$RESULTS"

# --- Test Runner ---
run_test() {
    local num="$1" desc="$2" cmd="$3"
    ((TOTAL++))

    # Expected output from bash
    local bash_out
    bash_out=$(bash -c "$cmd" 2>/dev/null)
    local bash_rc=$?

    # Detect
    local detected="no"
    if $REEF detect --quick -- "$cmd" &>/dev/null; then
        detected="yes"
    fi

    # Translate
    local translated
    translated=$($REEF translate -- "$cmd" 2>/dev/null)
    local t_rc=$?

    if [[ $t_rc -eq 0 && -n "$translated" ]]; then
        # Tier 1: run translated fish and compare
        local fish_out
        fish_out=$(fish -c "$translated" 2>/dev/null)

        if [[ "$bash_out" == "$fish_out" ]]; then
            ((PASS++))
            printf "${G}  PASS${N} #%-5s ${D}%s${N}\n" "$num" "$desc"
            echo "PASS #$num [$desc] → T1:translate" >> "$RESULTS"
        else
            ((FAIL++))
            printf "${R}  FAIL${N} #%-5s ${D}%s${N}\n" "$num" "$desc"
            printf "       ${D}bash:${N} %.100s\n" "$bash_out"
            printf "       ${D}fish:${N} %.100s\n" "$fish_out"
            echo "FAIL #$num [$desc] → T1:translate" >> "$RESULTS"
            echo "  cmd:  $cmd" >> "$RESULTS"
            echo "  xlat: $(echo "$translated" | head -3)" >> "$RESULTS"
            echo "  bash: $bash_out" >> "$RESULTS"
            echo "  fish: $fish_out" >> "$RESULTS"
        fi
    else
        # Tier 2: bash-exec fallback
        # bash-exec redirects command output to stderr, env diff to stdout
        local be_out
        be_out=$($REEF bash-exec -- "$cmd" 2>&1 1>/dev/null)
        local be_rc=$?

        if [[ "$bash_out" == "$be_out" ]]; then
            ((T2_OK++))
            printf "${Y}  T2  ${N} #%-5s ${D}%s${N}  ${D}(detect:$detected)${N}\n" "$num" "$desc"
            echo "T2   #$num [$desc] → bash-exec (detect:$detected)" >> "$RESULTS"
        else
            ((T2_DIFF++))
            printf "${Y}  T2~ ${N} #%-5s ${D}%s${N}  ${D}(detect:$detected, output differs)${N}\n" "$num" "$desc"
            echo "T2~  #$num [$desc] → bash-exec (detect:$detected, output differs)" >> "$RESULTS"
            echo "  cmd:  $cmd" >> "$RESULTS"
            echo "  bash: $bash_out" >> "$RESULTS"
            echo "  bexe: $be_out" >> "$RESULTS"
        fi
    fi
    echo "" >> "$RESULTS"
}

skip_test() {
    local num="$1" desc="$2" reason="$3"
    ((TOTAL++))
    ((SKIP++))
    printf "${D}  SKIP${N} #%-5s ${D}%s — %s${N}\n" "$num" "$desc" "$reason"
    echo "SKIP #$num [$desc] → $reason" >> "$RESULTS"
    echo "" >> "$RESULTS"
}

section() {
    echo ""
    printf "${B}${C}  ── %s${N}\n" "$1"
    echo "" >> "$RESULTS"
    echo "── $1" >> "$RESULTS"
}

# ============================================================================
#  TESTS BEGIN
# ============================================================================
echo ""
echo "${B}${C}  reef Comprehensive Test Suite${N}"
echo "${C}  ═══════════════════════════════════════════════════════════${N}"

# ─── CATEGORY 1: Variable Assignment & Export ───
section "Variable Assignment & Export"

run_test "1.1"  "simple export"                'export REEF_T=bar; echo $REEF_T'
run_test "1.2"  "export with spaces"           'export REEF_T="hello world"; echo "$REEF_T"'
run_test "1.3"  "export PATH append"           'export REEF_T="/opt/bin:$PATH"; echo "${REEF_T:0:8}"'
run_test "1.4"  "multiple exports"             'export REEF_A=1 REEF_B=2 REEF_C=3; echo "$REEF_A $REEF_B $REEF_C"'
run_test "1.5"  "export without value"         'REEF_T=hello; export REEF_T; echo $REEF_T'
run_test "1.6"  "unset"                        'export REEF_T=bar; unset REEF_T; echo "gone:${REEF_T:-yes}"'
run_test "1.7"  "declare -x"                   'declare -x REEF_T="test"; echo $REEF_T'
run_test "1.8"  "local in function"            'myfunc() { local x=5; echo $x; }; myfunc'
run_test "1.9"  "inline var before command"    'REEF_T=bar echo "test"'
run_test "1.10" "assignment with cmd subst"    'REEF_T=$(echo hello); echo $REEF_T'
run_test "1.11" "assignment with backticks"    'REEF_T=`echo hello`; echo $REEF_T'
run_test "1.12" "multi inline assignments"     'A=1 B=2 bash -c "echo \$A \$B"'

# ─── CATEGORY 2: Command Substitution ───
section "Command Substitution"

run_test "2.1"  "basic \$()"                   'echo "User: $(whoami)"'
run_test "2.2"  "nested \$()"                  'echo "Dir: $(basename $(pwd))"'
run_test "2.3"  "\$() in assignment"           'REEF_T=$(echo "hello world" | wc -w); echo $REEF_T'
run_test "2.4"  "backtick substitution"        'echo "User: `whoami`"'
run_test "2.5"  "\$() with pipes"              'REEF_T=$(echo "hello world" | tr " " "\n" | wc -l); echo $REEF_T'
run_test "2.6"  "deeply nested (3 levels)"     'echo "$(echo "$(echo "$(whoami)")")"'
run_test "2.7"  "deeply nested (4 levels)"     'echo $(echo $(echo $(echo deep)))'
run_test "2.8"  "cmd subst in argument"        'basename $(echo /home/user/file.txt)'
run_test "2.9"  "cmd subst in double quotes"   'echo "hello $(echo world)"'
run_test "2.10" "multiple cmd substs"          'echo "$(whoami) on $(hostname)"'

# ─── CATEGORY 3: Conditionals ───
section "Conditionals"

run_test "3.1"  "if [ -f ] then fi"            'if [ -f /etc/hostname ]; then echo "exists"; fi'
run_test "3.2"  "if/else"                      'if [ -f /nonexistent ]; then echo "yes"; else echo "no"; fi'
run_test "3.3"  "if/elif/else"                 'x=5; if [ $x -lt 3 ]; then echo "low"; elif [ $x -lt 7 ]; then echo "mid"; else echo "high"; fi'
run_test "3.4"  "[[ -n ]] test"                'if [[ -n "$HOME" ]]; then echo "HOME set"; fi'
run_test "3.5"  "[[ == pattern ]]"             'if [[ "hello" == h* ]]; then echo "match"; fi'
run_test "3.6"  "[[ && ]] combined"            'if [[ -d /tmp && -d /var ]]; then echo "both"; fi'
run_test "3.7"  "[[ || ]] combined"            '[[ -d /tmp || -d /nonexistent ]] && echo "one exists"'
run_test "3.8"  "[[ =~ regex ]]"               'if [[ "hello123" =~ ^[a-z]+[0-9]+$ ]]; then echo "match"; fi'
run_test "3.9"  "test command"                 'if test -d /tmp; then echo "yes"; fi'
run_test "3.10" "ternary-style && ||"          '[ -d /tmp ] && echo "exists" || echo "missing"'
run_test "3.11" "negated test"                 'if ! [ -f /nonexistent ]; then echo "missing"; fi'
run_test "3.12" "[[ ! -f ]]"                   '[[ ! -f /nonexistent ]] && echo "correct"'
run_test "3.13" "[[ -z ]] empty"               '[[ -z "" ]] && echo "empty"'
run_test "3.14" "[[ -n ]] nonempty"            '[[ -n "hello" ]] && echo "nonempty"'
run_test "3.15" "[[ numeric -gt ]]"            '[[ 42 -gt 10 ]] && echo "bigger"'
run_test "3.16" "[[ string == string ]]"       '[[ "abc" == "abc" ]] && echo "match"'
run_test "3.17" "[[ string != string ]]"       '[[ "abc" != "xyz" ]] && echo "different"'
run_test "3.18" "[[ =~ ]] has number"          '[[ "error: line 42" =~ [0-9]+ ]] && echo "has number"'
run_test "3.19" "command -v check"             'if command -v fish >/dev/null 2>&1; then echo "found"; else echo "nope"; fi'
run_test "3.20" "nested if"                    'if [ -d /tmp ]; then if [ -d /var ]; then echo "both"; fi; fi'

# ─── CATEGORY 4: Loops ───
section "Loops"

run_test "4.1"  "for with \$(seq)"             'for i in $(seq 1 5); do echo $i; done'
run_test "4.2"  "for with word list"           'for color in red green blue; do echo $color; done'
run_test "4.3"  "for with glob"                'for f in /tmp/.X*; do echo "$f"; done 2>/dev/null || echo "no match"'
run_test "4.4"  "for with brace range"         'for i in {1..5}; do echo $i; done'
run_test "4.5"  "for with brace step"          'for i in {0..20..5}; do echo $i; done'
run_test "4.6"  "while read"                   'echo -e "one\ntwo\nthree" | while read line; do echo "Got: $line"; done'
run_test "4.7"  "while with condition"         'i=0; while [ $i -lt 5 ]; do echo $i; i=$((i+1)); done'
run_test "4.8"  "until loop"                   'i=0; until [ $i -ge 3 ]; do echo $i; i=$((i+1)); done'
run_test "4.9"  "nested for loops"             'for i in 1 2; do for j in a b; do echo "$i$j"; done; done'
run_test "4.10" "for with quoted items"        'for word in "hello world" "foo bar"; do echo "$word"; done'
run_test "4.11" "for with cmd subst words"     'for f in $(echo a b c); do echo "item:$f"; done'
run_test "4.12" "c-style for loop"             'for ((i=0; i<5; i++)); do echo $i; done'
run_test "4.13" "c-style for with step"        'for ((i=0; i<=20; i+=5)); do echo $i; done'
run_test "4.14" "c-style for countdown"        'for ((i=5; i>0; i--)); do echo $i; done'
run_test "4.15" "while read -r from pipe"      'printf "1 a\n2 b\n3 c\n" | while read -r num letter; do echo "$num=$letter"; done'

# ─── CATEGORY 5: Arithmetic ───
section "Arithmetic"

run_test "5.1"  "basic addition"               'echo $((2 + 3))'
run_test "5.2"  "multiply"                     'echo $((2 + 3 * 4))'
run_test "5.3"  "grouped"                      'echo $(( (5 + 3) * 2 ))'
run_test "5.4"  "modulo"                       'echo $((17 % 5))'
run_test "5.5"  "with variables"               'x=10; y=3; echo $((x * y))'
run_test "5.6"  "comparison > (1/0)"           'echo $((5 > 3))'
run_test "5.7"  "comparison < (1/0)"           'echo $((3 < 5))'
run_test "5.8"  "comparison == (1/0)"          'echo $((5 == 5))'
run_test "5.9"  "ternary"                      'x=5; echo $((x > 3 ? 100 : 200))'
run_test "5.10" "nested ternary"               'echo $((1 > 2 ? 10 : 3 > 2 ? 20 : 30))'
run_test "5.11" "bitwise AND"                  'echo $((255 & 15))'
run_test "5.12" "bitwise OR"                   'echo $((170 | 85))'
run_test "5.13" "bitwise XOR"                  'echo $((255 ^ 170))'
run_test "5.14" "left shift"                   'echo $((1 << 8))'
run_test "5.15" "right shift"                  'echo $((256 >> 4))'
run_test "5.16" "negation"                     'echo $((-5 + 10))'
run_test "5.17" "arith in condition"           'if [ $((2+2)) -eq 4 ]; then echo "math"; fi'
run_test "5.18" "(( )) as test"                'x=5; if ((x > 3)); then echo "big"; fi'
run_test "5.19" "(( )) increment"              'x=5; ((x++)); echo $x'
run_test "5.20" "let command"                  'let "x = 5 + 3"; echo $x'

# ─── CATEGORY 6: Parameter Expansion ───
section "Parameter Expansion"

run_test "6.1"  "\${var:-default}"             'unset REEF_T; echo ${REEF_T:-"fallback"}'
run_test "6.2"  "\${var:=default}"             'unset REEF_T; echo ${REEF_T:="assigned"}; echo $REEF_T'
run_test "6.3"  "\${var:+alternate}"           'REEF_T="hello"; echo ${REEF_T:+"it exists"}'
run_test "6.4"  "\${#var} length"              'REEF_T="hello"; echo ${#REEF_T}'
run_test "6.5"  "\${var%.*} suffix"            'REEF_T="doc.tar.gz"; echo ${REEF_T%.*}'
run_test "6.6"  "\${var%%.*} longest suffix"   'REEF_T="doc.tar.gz"; echo ${REEF_T%%.*}'
run_test "6.7"  "\${var#*/} prefix"            'REEF_T="/home/user/docs"; echo ${REEF_T#*/}'
run_test "6.8"  "\${var##*/} longest prefix"   'REEF_T="/home/user/docs"; echo ${REEF_T##*/}'
run_test "6.9"  "\${var/old/new} replace"      'REEF_T="hello world"; echo ${REEF_T/world/earth}'
run_test "6.10" "\${var//old/new} global repl" 'REEF_T="aabaa"; echo ${REEF_T//a/x}'
run_test "6.11" "\${var} simple braces"        'REEF_T=hello; echo ${REEF_T}'
run_test "6.12" "\${var:0:5} substring"        'REEF_T="hello world"; echo ${REEF_T:0:5}'
run_test "6.13" "\${var:6} offset"             'REEF_T="hello world"; echo ${REEF_T:6}'
run_test "6.14" "\${var^^} uppercase"          'REEF_T="hello"; echo ${REEF_T^^}'
run_test "6.15" "\${var,,} lowercase"          'REEF_T="HELLO"; echo ${REEF_T,,}'
run_test "6.16" "\${var^} capitalize"          'REEF_T="hello"; echo ${REEF_T^}'
run_test "6.17" "\${!var} indirect"            'REEF_T="hello"; ref=REEF_T; echo ${!ref}'

# ─── CATEGORY 7: Here Strings ───
section "Here Strings"

run_test "7.1"  "basic here string"            'cat <<< "hello world"'
run_test "7.2"  "here string with variable"    'REEF_T=test; cat <<< "value: $REEF_T"'
run_test "7.3"  "here string to read"          'read -r a b <<< "hello world"; echo "$a $b"'
run_test "7.4"  "here string grep"             'grep "hello" <<< "hello world"'
run_test "7.5"  "here string single quoted"    "cat <<< 'single quoted'"

# ─── CATEGORY 8: Heredocs ───
section "Heredocs"

run_test "8.1"  "basic heredoc" "$(cat <<'OUTER'
cat <<EOF
Hello World
This is a heredoc
EOF
OUTER
)"

run_test "8.2"  "heredoc with expansion" "$(cat <<'OUTER'
NAME=Xavier; cat <<EOF
Hello $NAME
EOF
OUTER
)"

run_test "8.3"  "heredoc no expansion" "$(cat <<'OUTER'
cat <<'EOF'
$HOME is literal
$(whoami) is literal
EOF
OUTER
)"

run_test "8.4"  "heredoc piped" "$(cat <<'OUTER'
cat <<EOF | wc -l
line1
line2
line3
EOF
OUTER
)"

# ─── CATEGORY 9: Process Substitution ───
section "Process Substitution"

run_test "9.1"  "diff <()"                     'diff <(echo "hello") <(echo "hello"); echo "rc:$?"'
run_test "9.2"  "paste <(seq)"                 'paste <(seq 3) <(seq 4 6)'
run_test "9.3"  "while read < <()"             'while read -r line; do echo ">>$line"; done < <(echo -e "x\ny")'
run_test "9.4"  "cat <(echo)"                  'cat <(echo "from process subst")'

# ─── CATEGORY 10: Arrays ───
section "Arrays (Indexed)"

run_test "10.1" "array access"                 'arr=(one two three); echo ${arr[1]}'
run_test "10.2" "array all elements"           'arr=(one two three); echo ${arr[@]}'
run_test "10.3" "array length"                 'arr=(one two three); echo ${#arr[@]}'
run_test "10.4" "array append"                 'arr=(one two); arr+=(three); echo ${arr[@]}'
run_test "10.5" "array slice"                  'arr=(a b c d e); echo ${arr[@]:1:3}'
run_test "10.6" "array in for loop"            'arr=(red green blue); for c in "${arr[@]}"; do echo $c; done'
run_test "10.7" "array element delete"         'arr=(one two three); unset arr[1]; echo ${arr[@]}'
run_test "10.8" "array with spaces"            'arr=("hello world" "foo bar"); echo "${arr[0]}"'

# ─── CATEGORY 11: Associative Arrays ───
section "Associative Arrays"

run_test "11.1" "declare -A and use"           'declare -A m; m[red]="#FF0000"; m[blue]="#0000FF"; echo ${m[red]}'
run_test "11.2" "compound assignment"          'declare -A u=([name]="Xavier" [city]="Montana"); echo ${u[name]}'
run_test "11.3" "all keys"                     'declare -A m=([a]=1 [b]=2); echo ${!m[@]} | tr " " "\n" | sort | tr "\n" " "'
run_test "11.4" "iterate assoc array"          'declare -A m=([x]=10 [y]=20); for k in $(echo ${!m[@]} | tr " " "\n" | sort); do echo "$k=${m[$k]}"; done'

# ─── CATEGORY 12: Case Statements ───
section "Case Statements"

run_test "12.1" "simple case"                  'x="hello"; case $x in hello) echo "hi";; bye) echo "cya";; esac'
run_test "12.2" "case with wildcard"           'x="foobar"; case $x in foo*) echo "starts with foo";; *) echo "other";; esac'
run_test "12.3" "case multi-pattern"           'x="yes"; case $x in y|yes|Y|YES) echo "affirm";; *) echo "no";; esac'
run_test "12.4" "case bracket pattern"         'case "Y" in [yY]) echo "y";; [nN]) echo "n";; *) echo "other";; esac'
run_test "12.5" "case ;& fallthrough"          'x="a"; case $x in a) echo "a";& b) echo "b";; esac'
run_test "12.6" "nested case in if"            'x=foo; if true; then case "$x" in foo) echo "matched";; esac; fi'

# ─── CATEGORY 13: Functions ───
section "Functions"

run_test "13.1" "function keyword"             'function greet { echo "Hello $1"; }; greet "World"'
run_test "13.2" "parens syntax"                'greet() { echo "Hello $1"; }; greet "World"'
run_test "13.3" "function with local"          'myfunc() { local x=5; local y=10; echo $((x + y)); }; myfunc'
run_test "13.4" "function with return"         'is_even() { return $(( $1 % 2 )); }; is_even 4 && echo "even" || echo "odd"'
run_test "13.5" "recursive function"           'factorial() { if [ $1 -le 1 ]; then echo 1; else echo $(( $1 * $(factorial $(($1 - 1))) )); fi; }; factorial 5'

# ─── CATEGORY 14: Redirections ───
section "Redirections"

run_test "14.1" "stderr to stdout"             'command cat /nonexistent_reef_path 2>&1 | head -1'
run_test "14.2" "discard stderr"               'ls /nonexistent_reef_path 2>/dev/null; echo "ok"'
run_test "14.3" "discard both &>"              'ls /tmp /nonexistent_reef_path &>/dev/null; echo "ok"'
run_test "14.4" "append >>"                    'echo "line1" > /tmp/reef_t.txt; echo "line2" >> /tmp/reef_t.txt; cat /tmp/reef_t.txt; rm /tmp/reef_t.txt'
run_test "14.5" "redirect stderr only"         'echo "visible" 2>/dev/null'

# ─── CATEGORY 15: Subshells & Grouping ───
section "Subshells & Grouping"

run_test "15.1" "subshell basic"               '(echo "from subshell")'
run_test "15.2" "subshell isolation"           'x=outer; (x=inner; echo $x); echo $x'
run_test "15.3" "subshell cd"                  '(cd /tmp && echo "In: $(pwd)"); echo "Back: $(pwd)"'
run_test "15.4" "brace group piped"            '{ echo "one"; echo "two"; echo "three"; } | wc -l'
run_test "15.5" "exit from subshell"           '(exit 0); echo "rc:$?"'
run_test "15.6" "background and wait"          'sleep 0.1 & wait; echo "done"'

# ─── CATEGORY 16: Brace Expansion ───
section "Brace Expansion"

run_test "16.1" "brace list"                   'echo {apple,banana,cherry}'
run_test "16.2" "brace with prefix"            'echo file_{a,b,c}.txt'
run_test "16.3" "numeric range"                'echo {1..5}'
run_test "16.4" "range with step"              'echo {0..20..5}'
run_test "16.5" "alpha range"                  'echo {a..e}'
run_test "16.6" "reverse range"                'echo {5..1}'
run_test "16.7" "nested braces"                'echo {a,b}{1,2}'
run_test "16.8" "brace expansion in echo"      'echo {1..3}'

# ─── CATEGORY 17: Pipelines & Chains ───
section "Pipelines & Chains"

run_test "17.1" "simple pipeline"              'echo "hello world" | tr " " "\n" | sort'
run_test "17.2" "3-stage pipeline"             'echo "3 1 2" | tr " " "\n" | sort'
run_test "17.3" "AND chain"                    'true && echo "a" && echo "b"'
run_test "17.4" "OR chain"                     'false || echo "fallback"'
run_test "17.5" "mixed AND/OR (true)"          'true && echo "yes" || echo "no"'
run_test "17.6" "mixed AND/OR (false)"         'false && echo "yes" || echo "no"'
run_test "17.7" "semicolon chain"              'x=1; y=2; z=$((x+y)); echo "sum:$z"'
run_test "17.8" "pipe to while"               'echo -e "a\nb\nc" | while read -r line; do echo "got:$line"; done'
run_test "17.9" "pipeline wc"                  'echo "one two three" | wc -w'
run_test "17.10" "pipeline tr"                 'echo "HELLO" | tr A-Z a-z'
run_test "17.11" "pipeline cut"                'echo "a:b:c" | cut -d: -f2'

# ─── CATEGORY 18: Special Variables ───
section "Special Variables"

run_test "18.1" "\$? success"                  'true; echo $?'
run_test "18.2" "\$? failure"                  'false; echo $?'
run_test "18.3" "colon noop"                   ': ; echo "after noop"'
run_test "18.4" "colon as true"                ': && echo "null succeeded"'

# ─── CATEGORY 19: Quoting Edge Cases ───
section "Quoting Edge Cases"

run_test "19.1" "single in double"             "echo \"it's a test\""
run_test "19.2" "double in single"             "echo 'he said \"hello\"'"
run_test "19.3" "escaped double quotes"        'echo "she said \"wow\""'
run_test "19.4" "dollar in single (literal)"   "echo '\$HOME is not expanded'"
run_test "19.5" "backslash in double"          'echo "path: C:\\Users\\test"'
run_test "19.6" "empty string args"            'echo "" "hello" ""'
run_test "19.7" "adjacent quoting"             'echo "hello"" ""world"'

# ─── CATEGORY 20: Traps & Signals ───
section "Traps & Signals"

run_test "20.1" "trap EXIT"                    'trap "echo cleanup" EXIT; echo "running"'
run_test "20.2" "trap ERR"                     'trap "echo error" ERR; false; true'
run_test "20.3" "trap cleanup file"            'TMP=$(mktemp); trap "rm -f $TMP" EXIT; echo "test" > $TMP; cat $TMP'

# ─── CATEGORY 21: Namerefs & Indirect ───
section "Namerefs & Advanced"

run_test "21.1" "nameref basic"                'x=hello; declare -n ref=x; echo $ref'
run_test "21.2" "nameref assignment"           'declare -n ref=target; ref="world"; echo $target'
run_test "21.3" "nameref in function"          'set_result() { declare -n out=$1; out="done"; }; set_result myvar; echo $myvar'
run_test "21.4" "eval simple"                  'cmd="echo hello"; eval $cmd'
run_test "21.5" "eval indirect"                'name=HOME; eval echo \$$name'

# ─── CATEGORY 22: mapfile / readarray ───
section "mapfile & readarray"

run_test "22.1" "mapfile from cmd"             'mapfile -t lines <<< "$(echo -e "a\nb\nc")"; echo "${lines[1]}"'
run_test "22.2" "readarray"                    'readarray -t nums < <(seq 1 5); echo "${nums[2]}"'

# ─── CATEGORY 23: Coprocesses ───
section "Coprocesses"

run_test "23.1" "basic coproc"                 'coproc { echo "hello from coproc"; }; read -r output <&"${COPROC[0]}"; echo "$output"'

# ─── CATEGORY 24: Real-World One-Liners ───
section "Real-World One-Liners"

run_test "24.1" "basename in subst"            'echo $(basename /home/user/file.txt)'
run_test "24.2" "dirname in subst"             'echo $(dirname /home/user/file.txt)'
run_test "24.3" "uname system info"            'echo "OS: $(uname -s)"'
run_test "24.4" "nproc cores"                  'echo "CPU: $(nproc) cores"'
run_test "24.5" "awk field extract"            'echo "hello:world:foo" | awk -F: '"'"'{print $2}'"'"''
run_test "24.6" "sed substitution"             'echo "hello world" | sed "s/world/earth/"'
run_test "24.7" "xargs with placeholder"       'echo -e "a\nb\nc" | xargs -I{} echo "item: {}"'
run_test "24.8" "check cmd exists"             'if command -v fish >/dev/null 2>&1; then echo "found"; else echo "nope"; fi'
run_test "24.9" "test -f file"                 'if test -f /etc/hostname; then echo "exists"; fi'
run_test "24.10" "export + run"                'export REEF_T=hello && echo "set: $REEF_T"'

# ─── CATEGORY 25: Obscure & Exotic ───
section "Obscure & Exotic"

run_test "25.1"  "ANSI-C quoting"              "echo \$'hello\\tworld'"
run_test "25.2"  "brace alpha range"           'echo {a..z}'
run_test "25.3"  "brace seq step"              'echo {0..20..5}'
run_test "25.4"  "printf repetition"           'printf "%0.s-" {1..10}; echo'
run_test "25.5"  "tilde expansion"             'echo ~'
run_test "25.6"  "empty if body (: noop)"      'if true; then :; fi; echo "ok"'
run_test "25.7"  "IFS manipulation"            'IFS=:; read -ra parts <<< "a:b:c:d"; echo "${parts[2]}"'
run_test "25.8"  "regex capture groups"        '[[ "2024-01-15" =~ ^([0-9]{4})-([0-9]{2})-([0-9]{2})$ ]] && echo "${BASH_REMATCH[1]} ${BASH_REMATCH[2]} ${BASH_REMATCH[3]}"'
run_test "25.9"  "wait for bg jobs"            'sleep 0.05 & p1=$!; sleep 0.05 & p2=$!; wait $p1 $p2; echo "both done"'
run_test "25.10" "nested brace+arith"          'for i in {1..3}; do for j in {1..3}; do echo -n "$((i*j)) "; done; echo; done'
run_test "25.11" "set -eo pipefail"            'set -eo pipefail; echo "start"; true; echo "end"'
run_test "25.12" "multiple redirection"        '{ echo out; echo err >&2; } > /tmp/reef_t1.txt 2> /tmp/reef_t2.txt; cat /tmp/reef_t1.txt; cat /tmp/reef_t2.txt; rm /tmp/reef_t1.txt /tmp/reef_t2.txt'
run_test "25.13" "arith in array index"        'arr=(a b c d e); i=2; echo ${arr[$((i+1))]}'
run_test "25.14" "extglob"                     'shopt -s extglob; x="aabcc"; echo ${x/+(a)/X}'
run_test "25.15" "read from brace group"       'echo "hello world" | { read -r a b; echo "$a|$b"; }'
run_test "25.16" "nested case in loop"         'for x in a b c; do case $x in a) echo "first";; *) echo "other: $x";; esac; done'
run_test "25.17" "function + export"           'myfn() { export REEF_FN=inside; echo $REEF_FN; }; myfn'
run_test "25.18" "multiline semicolons"        'a=1; b=2; c=3; d=$((a+b+c)); echo "total:$d"'
run_test "25.19" "deeply chained pipes"        'seq 10 | head -5 | tail -3 | sort -r | tr "\n" " "; echo'

# ─── CATEGORY 26: Stress / Breakage Attempts ───
section "Stress & Breakage Attempts"

run_test "26.1"  "100 iteration loop"          'for i in $(seq 1 100); do echo -n "$i "; done; echo'
run_test "26.2"  "long pipeline"               'seq 1000 | sort -n | tail -1'
run_test "26.3"  "var with special chars"      'REEF_T="hello\$world"; echo "$REEF_T"'
run_test "26.4"  "semicolons everywhere"       'echo a; echo b; echo c; echo d; echo e'
run_test "26.5"  "nested subshells"            '( ( ( echo "deep" ) ) )'
run_test "26.6"  "empty subshell"              '(); echo "after"'
run_test "26.7"  "if with complex condition"   'if [ -d /tmp ] && [ -d /var ] || [ -d /home ]; then echo "yes"; fi'
run_test "26.8"  "arithmetic overflow"         'echo $((2**31))'
run_test "26.9"  "whitespace in var"           'REEF_T="  hello  world  "; echo "|${REEF_T}|"'
run_test "26.10" "newline in var"              'REEF_T=$'"'"'first\nsecond'"'"'; echo "$REEF_T"'
run_test "26.11" "tab in var"                  'REEF_T=$'"'"'col1\tcol2'"'"'; echo "$REEF_T"'
run_test "26.12" "empty case match"            'x=""; case "$x" in "") echo "empty";; *) echo "other";; esac'
run_test "26.13" "case with spaces"            'x="hello world"; case "$x" in "hello world") echo "matched";; esac'
run_test "26.14" "for loop no body"            'for i in 1 2 3; do :; done; echo "ok"'
run_test "26.15" "while false"                 'while false; do echo "never"; done; echo "ok"'
run_test "26.16" "export existing PATH"        'export PATH="$PATH"; echo "ok"'
run_test "26.17" "nested arithmetic"           'echo $(( ((2+3)) * ((4+5)) ))'
run_test "26.18" "command with equals"         'echo "key=value"'
run_test "26.19" "multiple here strings"       'read -r a <<< "first"; read -r b <<< "second"; echo "$a $b"'
run_test "26.20" "chained tests"               '[[ -d /tmp ]] && [[ -d /var ]] && [[ -d /home ]] && echo "all exist"'

# ─── CATEGORY 27: Real-World Tool Patterns (graceful without deps) ───
section "Real-World Tool Patterns (may not be installed)"

run_test "27.1"  "nvm setup"                   'export NVM_DIR="$HOME/.nvm"; [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh" || echo "nvm not found"'
run_test "27.2"  "conda path"                  'export PATH="/opt/conda/bin:$PATH"; command -v conda >/dev/null 2>&1 && echo "conda found" || echo "conda not found"'
run_test "27.3"  "pyenv init"                  'export PYENV_ROOT="$HOME/.pyenv"; export PATH="$PYENV_ROOT/bin:$PATH"; command -v pyenv >/dev/null 2>&1 && eval "$(pyenv init --path)" && echo "pyenv ok" || echo "pyenv not found"'
run_test "27.4"  "cargo/rust path"             'export PATH="$HOME/.cargo/bin:$PATH"; command -v cargo >/dev/null 2>&1 && echo "cargo found" || echo "cargo not found"'
run_test "27.5"  "docker compose"              'export COMPOSE_PROJECT_NAME=myapp; command -v docker >/dev/null 2>&1 && docker compose version 2>/dev/null || echo "docker not found"'
run_test "27.6"  "aws cli pattern"             'INSTANCE_ID=$(aws ec2 describe-instances --query "Reservations[0].Instances[0].InstanceId" --output text 2>/dev/null || echo "none"); echo "Instance: $INSTANCE_ID"'
run_test "27.7"  "kubectl pattern"             'NS=$(kubectl get namespaces -o name 2>/dev/null || echo "none"); echo "Namespaces: $NS"'
run_test "27.8"  "terraform pattern"           'TF_VER=$(command -v terraform >/dev/null 2>&1 && terraform version -json 2>/dev/null | head -1 || echo "not installed"); echo "TF: $TF_VER"'
run_test "27.9"  "npm/node pattern"            'NODE_VER=$(node --version 2>/dev/null || echo "none"); echo "Node: $NODE_VER"'
run_test "27.10" "go version pattern"          'GO_VER=$(go version 2>/dev/null || echo "none"); echo "Go: $GO_VER"'
run_test "27.11" "java version pattern"        'JAVA_VER=$(command -v java >/dev/null 2>&1 && java -version 2>&1 | head -1 || echo "not installed"); echo "Java: $JAVA_VER"'
run_test "27.12" "ruby version pattern"        'RUBY_VER=$(ruby --version 2>/dev/null || echo "none"); echo "Ruby: $RUBY_VER"'
run_test "27.13" "ssh remote cmd"              'ssh -o ConnectTimeout=1 -o BatchMode=yes nonexistent_host "echo hi" 2>/dev/null || echo "ssh failed gracefully"'
run_test "27.14" "curl installer pattern"      'curl -fsSL https://127.0.0.1:1/nonexistent 2>/dev/null || echo "curl failed gracefully"'
run_test "27.15" "git hook one-liner"          'changed=$(git diff --cached --name-only 2>/dev/null); if [ -n "$changed" ]; then for f in $changed; do echo "checking $f"; done; else echo "no staged files"; fi'
run_test "27.16" "systemd status check"        'systemctl is-active sshd 2>/dev/null || echo "service unknown"'
run_test "27.17" "pip install pattern"         'pip install --dry-run nonexistent_pkg_reef_test 2>/dev/null || echo "pip failed gracefully"'
run_test "27.18" "conda activate pattern"      'command -v conda >/dev/null 2>&1 && conda activate base 2>/dev/null || echo "conda not available"'
run_test "27.19" "process CSV files"           'for f in *.csv; do [ -f "$f" ] && wc -l "$f" || echo "no csv files"; break; done'
run_test "27.20" "system info one-liner"       'echo "OS: $(uname -s) $(uname -r)"; echo "CPU: $(nproc) cores"; echo "Mem: $(free -h 2>/dev/null | awk "/^Mem:/{print \$2}" || echo "unknown")"'


# ============================================================================
#  SUMMARY
# ============================================================================
echo ""
echo "${C}  ═══════════════════════════════════════════════════════════${N}"
TOTAL_PASS=$((PASS + T2_OK))
echo "  ${G}T1 PASS:${N}      $PASS"
echo "  ${Y}T2 OK:${N}        $T2_OK"
echo "  ${Y}T2 diff:${N}      $T2_DIFF"
echo "  ${R}FAIL:${N}         $FAIL"
echo "  ${D}SKIP:${N}         $SKIP"
echo "  ${B}Total:${N}        $TOTAL"
echo "${C}  ═══════════════════════════════════════════════════════════${N}"
echo ""
echo "  Results written to: $RESULTS"
echo ""

# Write summary to results file
echo "═══════════════════════════════════════════" >> "$RESULTS"
echo "SUMMARY" >> "$RESULTS"
echo "T1 PASS:    $PASS" >> "$RESULTS"
echo "T2 OK:      $T2_OK" >> "$RESULTS"
echo "T2 diff:    $T2_DIFF" >> "$RESULTS"
echo "FAIL:       $FAIL" >> "$RESULTS"
echo "SKIP:       $SKIP" >> "$RESULTS"
echo "Total:      $TOTAL" >> "$RESULTS"
echo "═══════════════════════════════════════════" >> "$RESULTS"
