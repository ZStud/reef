use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// A complete command — foreground or background.
#[derive(Debug)]
pub enum Cmd<'a> {
    List(AndOrList<'a>),
    Job(AndOrList<'a>),
}

/// A chain of commands connected by `&&` and `||`.
#[derive(Debug)]
pub struct AndOrList<'a> {
    pub first: Pipeline<'a>,
    pub rest: Vec<AndOr<'a>>,
}

#[derive(Debug)]
pub enum AndOr<'a> {
    And(Pipeline<'a>),
    Or(Pipeline<'a>),
}

/// A pipeline: one or more commands connected by `|`.
#[derive(Debug)]
pub enum Pipeline<'a> {
    Single(Executable<'a>),
    /// `[!] cmd1 | cmd2 | ...` — bool is true if negated.
    Pipe(bool, Vec<Executable<'a>>),
}

#[derive(Debug)]
pub enum Executable<'a> {
    Simple(SimpleCmd<'a>),
    Compound(CompoundCmd<'a>),
    FuncDef(&'a str, CompoundCmd<'a>),
}

// ---------------------------------------------------------------------------
// Simple command
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SimpleCmd<'a> {
    pub prefix: Vec<CmdPrefix<'a>>,
    pub suffix: Vec<CmdSuffix<'a>>,
}

#[derive(Debug)]
pub enum CmdPrefix<'a> {
    Assign(&'a str, Option<Word<'a>>),
    /// `arr=(word ...)` — array assignment.
    ArrayAssign(&'a str, Vec<Word<'a>>),
    /// `arr+=(word ...)` — array append.
    ArrayAppend(&'a str, Vec<Word<'a>>),
    Redirect(Redir<'a>),
}

#[derive(Debug)]
pub enum CmdSuffix<'a> {
    Word(Word<'a>),
    Redirect(Redir<'a>),
}

// ---------------------------------------------------------------------------
// Compound commands
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CompoundCmd<'a> {
    pub kind: CompoundKind<'a>,
    pub redirects: Vec<Redir<'a>>,
}

#[derive(Debug)]
pub enum CompoundKind<'a> {
    For {
        var: &'a str,
        words: Option<Vec<Word<'a>>>,
        body: Vec<Cmd<'a>>,
    },
    While(GuardBody<'a>),
    Until(GuardBody<'a>),
    If {
        conditionals: Vec<GuardBody<'a>>,
        else_branch: Option<Vec<Cmd<'a>>>,
    },
    Case {
        word: Word<'a>,
        arms: Vec<CaseArm<'a>>,
    },
    CFor {
        init: Option<Arith<'a>>,
        cond: Option<Arith<'a>>,
        step: Option<Arith<'a>>,
        body: Vec<Cmd<'a>>,
    },
    Brace(Vec<Cmd<'a>>),
    Subshell(Vec<Cmd<'a>>),
    DoubleBracket(Vec<Cmd<'a>>),
    Arithmetic(Arith<'a>),
}

#[derive(Debug)]
pub struct GuardBody<'a> {
    pub guard: Vec<Cmd<'a>>,
    pub body: Vec<Cmd<'a>>,
}

#[derive(Debug)]
pub struct CaseArm<'a> {
    pub patterns: Vec<Word<'a>>,
    pub body: Vec<Cmd<'a>>,
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Word<'a> {
    Simple(WordPart<'a>),
    Concat(Vec<WordPart<'a>>),
}

#[derive(Debug)]
pub enum WordPart<'a> {
    Bare(Atom<'a>),
    DQuoted(Vec<Atom<'a>>),
    SQuoted(&'a str),
}

#[derive(Debug)]
pub enum Atom<'a> {
    Lit(&'a str),
    Escaped(Cow<'a, str>),
    Param(Param<'a>),
    Subst(Box<Subst<'a>>),
    Star,
    Question,
    SquareOpen,
    SquareClose,
    Tilde,
    ProcSubIn(Vec<Cmd<'a>>),
    /// ANSI-C `$'...'` — raw content between the quotes (escape sequences unresolved).
    AnsiCQuoted(&'a str),
    BraceRange {
        start: &'a str,
        end: &'a str,
        step: Option<&'a str>,
    },
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Param<'a> {
    Var(&'a str),
    Positional(u32),
    At,
    Star,
    Pound,
    Status,
    Pid,
    Bang,
    Dash,
}

// ---------------------------------------------------------------------------
// Substitutions
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Subst<'a> {
    Cmd(Vec<Cmd<'a>>),
    Arith(Option<Arith<'a>>),
    Len(Param<'a>),
    /// `${!var}` — indirect variable expansion.
    Indirect(&'a str),
    /// `${!prefix*}` / `${!prefix@}` — list variables matching prefix.
    PrefixList(&'a str),
    /// `${var@Q}` — parameter transformation (quoting).
    Transform(&'a str, u8),
    /// `${var:-word}` / `${var-word}` — fish `set -q` can't distinguish empty vs unset.
    Default(Param<'a>, Option<Word<'a>>),
    Assign(Param<'a>, Option<Word<'a>>),
    Error(Param<'a>, Option<Word<'a>>),
    Alt(Param<'a>, Option<Word<'a>>),
    TrimSuffixSmall(Param<'a>, Option<Word<'a>>),
    TrimSuffixLarge(Param<'a>, Option<Word<'a>>),
    TrimPrefixSmall(Param<'a>, Option<Word<'a>>),
    TrimPrefixLarge(Param<'a>, Option<Word<'a>>),
    Replace(Param<'a>, Option<Word<'a>>, Option<Word<'a>>),
    ReplaceAll(Param<'a>, Option<Word<'a>>, Option<Word<'a>>),
    ReplacePrefix(Param<'a>, Option<Word<'a>>, Option<Word<'a>>),
    ReplaceSuffix(Param<'a>, Option<Word<'a>>, Option<Word<'a>>),
    Substring(Param<'a>, &'a str, Option<&'a str>),
    Upper(bool, Param<'a>),
    Lower(bool, Param<'a>),
    /// `${arr[index]}` — array element access (index is a Word for $((expr)) support).
    ArrayElement(&'a str, Word<'a>),
    /// `${arr[@]}` or `${arr[*]}` — all array elements.
    ArrayAll(&'a str),
    /// `${#arr[@]}` — array length.
    ArrayLen(&'a str),
    /// `${arr[@]:offset:length}` — array slice.
    ArraySlice(&'a str, &'a str, Option<&'a str>),
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Arith<'a> {
    Var(&'a str),
    Lit(i64),

    Add(Box<Arith<'a>>, Box<Arith<'a>>),
    Sub(Box<Arith<'a>>, Box<Arith<'a>>),
    Mul(Box<Arith<'a>>, Box<Arith<'a>>),
    Div(Box<Arith<'a>>, Box<Arith<'a>>),
    Rem(Box<Arith<'a>>, Box<Arith<'a>>),
    Pow(Box<Arith<'a>>, Box<Arith<'a>>),

    Lt(Box<Arith<'a>>, Box<Arith<'a>>),
    Le(Box<Arith<'a>>, Box<Arith<'a>>),
    Gt(Box<Arith<'a>>, Box<Arith<'a>>),
    Ge(Box<Arith<'a>>, Box<Arith<'a>>),
    Eq(Box<Arith<'a>>, Box<Arith<'a>>),
    Ne(Box<Arith<'a>>, Box<Arith<'a>>),

    BitAnd(Box<Arith<'a>>, Box<Arith<'a>>),
    BitOr(Box<Arith<'a>>, Box<Arith<'a>>),
    BitXor(Box<Arith<'a>>, Box<Arith<'a>>),
    LogAnd(Box<Arith<'a>>, Box<Arith<'a>>),
    LogOr(Box<Arith<'a>>, Box<Arith<'a>>),
    Shl(Box<Arith<'a>>, Box<Arith<'a>>),
    Shr(Box<Arith<'a>>, Box<Arith<'a>>),

    Pos(Box<Arith<'a>>),
    Neg(Box<Arith<'a>>),
    LogNot(Box<Arith<'a>>),
    BitNot(Box<Arith<'a>>),

    PreInc(&'a str),
    PostInc(&'a str),
    PreDec(&'a str),
    PostDec(&'a str),

    Ternary(Box<Arith<'a>>, Box<Arith<'a>>, Box<Arith<'a>>),
    Assign(&'a str, Box<Arith<'a>>),
}

// ---------------------------------------------------------------------------
// Heredoc body
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum HeredocBody<'a> {
    /// Quoted delimiter — no expansion (literal text).
    Literal(&'a str),
    /// Unquoted delimiter — variable and command expansion.
    Interpolated(Vec<Atom<'a>>),
}

// ---------------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Redir<'a> {
    Read(Option<u16>, Word<'a>),
    Write(Option<u16>, Word<'a>),
    Append(Option<u16>, Word<'a>),
    ReadWrite(Option<u16>, Word<'a>),
    Clobber(Option<u16>, Word<'a>),
    DupRead(Option<u16>, Word<'a>),
    DupWrite(Option<u16>, Word<'a>),
    HereString(Word<'a>),
    Heredoc(HeredocBody<'a>),
    WriteAll(Word<'a>),
    AppendAll(Word<'a>),
}
