// SPDX-License-Identifier: CC0-1.0

//! The ONE bounded, session-local POSIX/bash SHELL SESSION every emulated service that has
//! to perform caller-supplied shell text drives.
//!
//! # What this module is
//!
//! Several emulated APIs carry a shell PROGRAM as a caller-supplied member: a build's
//! command lines, a container's `command`, a notebook instance's lifecycle hook. What the
//! client observes is what running that program produced — a status, the lines it wrote, the
//! variables it left behind — so a service that only STORES the text has implemented nothing.
//!
//! The grammar is not read here: a line is tokenized and parsed by `brush-parser`, the
//! POSIX/bash tokenizer and grammar, exactly as this workspace reads caller-supplied SQL
//! with `sqlparser` and caller-supplied YAML with `yaml-rust2`. That parser builds a syntax
//! tree and runs nothing, so the and-or lists, pipelines, assignments and quoting of a
//! program are the shell's own rather than something scanned out of the text. Word expansion
//! is read from the same parser's word pieces.
//!
//! # Why the grammar and not an engine
//!
//! The one maintained shell ENGINE this grammar has — `brush-core`, the executing half of
//! the same project — is deliberately not a dependency, and what follows is not a
//! reimplementation of it. An engine's whole purpose is to run what the shell language can
//! express: it resolves a command name against the host's `PATH` and starts a host PROCESS
//! for it, opens the host files a redirection names, and expands a command substitution by
//! running that command. Those are exactly the primitives this module has none of, and the
//! crate exposes no seam to replace them with an in-memory volume or an in-process
//! dispatcher — its command resolution and its redirections go straight to the host through
//! the standard library — so there is no configuration of it that could be confined here:
//! taking it would hand a caller's program the developer's machine.
//!
//! What this module does instead is narrower than a shell by construction, and it is not an
//! implementation of the shell LANGUAGE: there is no evaluator for the language's constructs
//! here, and none is approximated. It PERFORMS a fixed catalog of session-local utilities
//! ([`BUILT_IN_COMMANDS`]) over the real grammar's tree, and every other command, and every
//! command FORM and expansion it does not perform, fails closed with the status a shell
//! answers for a command it cannot run ([`Statuses::could_not_run`]). There is therefore no
//! language subset to keep in step with a specification: a command either names one of those
//! utilities, or it did not run.
//!
//! # What a session provides
//!
//! It runs IN THIS PROCESS. It has no process to start, no network to reach, no HOST
//! filesystem and no host environment: the utilities it provides are the ones whose whole
//! effect is inside the session itself — its output, its status, its variables and the files
//! it holds itself. The variables it starts with are the ones its caller hands it, because
//! those are the environment the program is given.
//!
//! # The working tree
//!
//! A program's commands write FILES — a build writes the file its artifacts name, a hook
//! writes the configuration it was given — so a session holds a tree of its own: an output
//! redirection (`> path`, `>> path`) collects what the command wrote and keeps it under that
//! path in this session, and `cat` reads a path back out of it. The tree is session-local
//! memory, keyed by the path the program named and bounded like everything else
//! ([`Bounds::files`], [`Bounds::file_bytes`], [`Bounds::tree_bytes`]); no host file is
//! opened, created or read.
//!
//! # A session that runs in a MACHINE's tree
//!
//! One kind of caller-supplied program is the program of a MACHINE the emulator holds — an
//! instance's boot script — and such a program's whole purpose is to read and write that
//! machine: it keeps a counter on a volume, writes a line to a console device and reads an
//! identity the kernel published to it. A caller that holds the machine hands the session
//! that machine's tree ([`ShellSession::in_volume`], [`super::posix_shell_volume`]) — which
//! then replaces the session's own, because the program's files are the machine's, not this
//! session's — and a session that HAS one performs the forms a program inside a machine is
//! written in, over that tree and nothing else:
//!
//! * the file utilities ([`VOLUME_COMMANDS`]) beside the session-local catalog;
//! * the redirections that reach the tree's other descriptors: standard error to a file or a
//!   device, and standard input from one;
//! * command substitution, whose commands run in this same session;
//! * arithmetic expansion ([`super::posix_shell_arithmetic`]), which is how such a program
//!   computes what it writes.
//!
//! None of that reaches a host path, a process or a socket: the tree is in memory, every
//! entry of it was put there by the service that owns the machine, and a path it does not
//! hold resolves to nothing. A session constructed WITHOUT one performs none of those forms,
//! and every other redirection form (another file descriptor, a duplication, a
//! here-document) is one no session performs.
//!
//! # Bounds
//!
//! Every bound is the emulator's, held in code the program cannot reach, and every one of
//! them is a field of [`Bounds`]: how many commands one run performs (latched — past it a
//! line performs nothing), how long a line may be and how deeply it may nest (checked
//! WITHOUT parsing, which is what keeps the grammar's own recursion in range), how much one
//! word and one line may expand to, how many variables and how many bytes of them the
//! session holds, and — for a caller that KEEPS what the program wrote
//! ([`OutputLimits`]) — how many lines and how many bytes per line it may keep.
//!
//! # The services that drive it
//!
//! * `service-com-amazonaws-codebuild-codebuild-20161006`'s `state::build_shell` performs a
//!   buildspec's command lines for an exit status and keeps no output, which is this module
//!   with [`OutputLimits`] absent;
//! * `service-com-amazonaws-sagemaker-sagemaker`'s `state::notebook_lifecycle_hook` performs
//!   a notebook instance's lifecycle hook and keeps its output, which it publishes to that
//!   hook's CloudWatch Logs stream.
//!
//! One service still carries its OWN copy of this session, written before this module
//! existed, and it is to be MIGRATED onto this one rather than copied again — exactly as the
//! claim-rule and log-group-name copies this crate lists are:
//! `service-com-amazonaws-ecs-amazonec2containerservicev20141113`'s `state::container_shell`
//! performs a container's program as a PROCESS — it loops, waits, installs signal handlers
//! and delivers each line as it is written — so moving it here means ADDING those to this
//! session rather than taking them away, and doing that in a change about neither of the two
//! services above would widen what their programs may run.
//!
//! Nothing new should restate the session — call this module.

use std::collections::BTreeMap;

use super::posix_shell_arithmetic::{self, ArithmeticVariables};
use super::posix_shell_volume::ShellVolume;

/// Where one command's standard output, standard error and standard input were REDIRECTED
/// to, for the command being performed.
///
/// Every session performs a redirection of standard output into the tree it writes; the
/// other two are performed only by a session given a MACHINE's tree
/// ([`ShellSession::in_volume`]), whose program is the one that reports on another
/// descriptor and reads a file back.
#[derive(Debug, Clone, Default)]
struct Redirections {
    /// The path standard output goes to, and whether it is APPENDED to.
    out: Option<(String, bool)>,
    /// The path standard error goes to, and whether it is APPENDED to.
    error: Option<(String, bool)>,
    /// The path standard input is read from.
    input: Option<String>,
}

/// The commands a session provides, as their names.
///
/// Each one's whole effect is inside the session, which is why it can be performed in
/// process: `echo` writes a line of the program's output, `cat` writes a file this session
/// holds, `true`, `false` and `:` answer a status, `exit` ends the session, and
/// `export`/`unset` change its variables. A command that is not one of these needs a
/// program, a host tree or a network a session does not have.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "echo", "cat", "true", "false", ":", "exit", "export", "unset",
];

/// The commands a session given a MACHINE's tree provides on top of
/// [`BUILT_IN_COMMANDS`], as their names.
///
/// Each one's whole effect is inside the session and the tree it was handed: `printf` writes
/// one line of output, `mkdir`, `touch` and `rm` change the tree, `test` (and its `[`
/// spelling) answers a question about it, and `sh -c` performs a program in this same
/// session, which is how a machine's init system runs one line. A session that was given no
/// machine's tree provides none of them: a directory, a removal and a nested program are what
/// a machine's own storage has, and its own tree is the files its redirections left.
pub const VOLUME_COMMANDS: &[&str] =
    &["printf", "mkdir", "touch", "rm", "test", "[", "sh", "bash"];

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes". For every other command
/// an assignment before it is that command's environment alone.
const PERSISTING_COMMANDS: &[&str] = &[":", "exit", "export", "unset"];

/// The statuses a session answers with, so a service whose API reports a status reports the
/// one a shell answers for its own condition.
#[derive(Debug, Clone, Copy)]
pub struct Statuses {
    /// The status of a command that ran to success.
    pub success: i32,
    /// The status of a command that ran and answered failure, which is what the `false`
    /// utility is specified to do ("exit with a status code indicating failure").
    pub failure: i32,
    /// The status a shell reports for a line it could not parse — what `sh` is specified to
    /// exit with on a syntax error it detects.
    pub syntax_error: i32,
    /// The status a shell reports when "the utility to be executed could not be found",
    /// which is what a session answers for every command it provides no program for, for a
    /// command form it does not perform, and for a line past its bounds.
    pub could_not_run: i32,
}

impl Default for Statuses {
    /// The statuses POSIX gives those four conditions.
    fn default() -> Self {
        Self {
            success: 0,
            failure: 1,
            syntax_error: 2,
            could_not_run: 127,
        }
    }
}

/// How much one session may do. Every field is the EMULATOR'S number, not a modelled one:
/// the programs are caller-supplied text, so what one of them can cost is bounded here.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// How many commands one session performs before it is given up on. Latched: past it a
    /// line performs nothing at all.
    pub commands: usize,
    /// The longest line a session reads.
    pub command_bytes: usize,
    /// How many grouping indicators or reserved words one line may carry before the session
    /// gives up on reading it — what keeps the grammar's own recursion in range, since a
    /// parser descends once per nested group.
    pub nesting: usize,
    /// How many bytes one word's expansion may produce.
    pub expansion_bytes: usize,
    /// How many bytes ONE line's expansions may materialise at once, which is their SUM.
    pub line_expansion_bytes: usize,
    /// How many bytes of variables one session may hold at once.
    pub session_bytes: usize,
    /// How many variables one session may hold at once, so neither a few long values nor
    /// very many short ones grow the store without a stated ceiling.
    pub session_variables: usize,
    /// How deeply a word's expansion is walked, applied to the pieces a quoted word nests.
    pub word_depth: usize,
    /// How many command substitutions and `sh -c` programs one session performs, which is
    /// what bounds the work a program can reach by nesting one inside another. Each one
    /// also spends a command of [`Bounds::commands`].
    pub substitutions: usize,
    /// How deeply those substitutions and nested `sh -c` programs may nest, which is what
    /// keeps this module's own recursion in range.
    pub substitution_depth: usize,
    /// How many bytes ONE substitution may capture before it has no value at all.
    pub substitution_bytes: usize,
    /// How many files one session's own tree holds at once.
    pub files: usize,
    /// How many bytes ONE of those files may hold, which is also how much of a redirected
    /// command's output is collected for it.
    pub file_bytes: usize,
    /// How many bytes the whole tree may hold at once, so neither a few large files nor very
    /// many small ones grow it without a stated ceiling.
    pub tree_bytes: usize,
}

impl Default for Bounds {
    /// Bounds far above any real program of the kinds these services carry, and far below
    /// what would cost the host anything noticeable.
    fn default() -> Self {
        Self {
            commands: 1_000,
            command_bytes: 16 * 1024,
            nesting: 64,
            expansion_bytes: 64 * 1024,
            line_expansion_bytes: 256 * 1024,
            session_bytes: 1024 * 1024,
            session_variables: 4_096,
            word_depth: 32,
            substitutions: 256,
            substitution_depth: 8,
            substitution_bytes: 64 * 1024,
            files: 256,
            file_bytes: 256 * 1024,
            tree_bytes: 4 * 1024 * 1024,
        }
    }
}

/// How much of what a program WROTE a session keeps, for a caller that reports it.
///
/// A caller that reports no output leaves this absent, and then nothing is kept at all: the
/// bytes a command writes are not delivered anywhere, so a session that is not asked for
/// them does not grow with them.
#[derive(Debug, Clone, Copy)]
pub struct OutputLimits {
    /// How many lines one session keeps. Past it a line is not kept at all.
    pub lines: usize,
    /// How many bytes ONE kept line may carry. A longer line is TRUNCATED on a character
    /// boundary rather than dropped, so what the program wrote is still reported.
    pub line_bytes: usize,
}

/// What a session is constructed with: its statuses, its bounds, and whether it keeps what
/// the program wrote.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionConfig {
    /// See [`Statuses`].
    pub statuses: Statuses,
    /// See [`Bounds`].
    pub bounds: Bounds,
    /// See [`OutputLimits`]; `None` keeps no output.
    pub output: Option<OutputLimits>,
}

/// The characters that open a nested group in a line, counted by [`within_parse_bounds`]: a
/// subshell or command substitution, a brace group or expansion, a bracket expression, and
/// the older command-substitution spelling.
const NESTING_INDICATORS: [char; 4] = ['(', '{', '[', '`'];

/// The reserved WORDS that open or close a nested command in a line, counted by
/// [`within_parse_bounds`] beside [`NESTING_INDICATORS`]: the shell's compound commands nest
/// through these rather than through any character, so counting characters alone would leave
/// the grammar's recursion unbounded for a line of `if … then …`. The words that CLOSE one
/// are counted too, which can only make the count higher — an over-count refuses a line and
/// never admits one.
const NESTING_WORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "while", "until", "for", "select", "case", "esac", "in",
    "do", "done", "function", "time",
];

/// The characters a reserved word may be glued to in a line: the shell's own separators and
/// operators. A word is counted when it stands between any of these (or whitespace), so
/// `:;if` carries an `if` exactly as `: ; if` does.
const WORD_SEPARATORS: [char; 10] = [';', '&', '|', '(', ')', '{', '}', '<', '>', '!'];

/// The path a session's tree keys a file by, for the path a program named — `None` when the
/// program named no path at all.
///
/// A relative path is what a program's commands write, and `./name` and `name` are the same
/// file to them, so the one spelling that differs only in that prefix is folded into it.
/// Nothing else about the path is interpreted: it is a key in this session's own tree, never
/// a path on the host.
fn file_path(named: &str) -> Option<String> {
    let trimmed = named.trim();
    let folded = trimmed.strip_prefix("./").unwrap_or(trimmed);
    (!folded.is_empty()).then(|| folded.to_string())
}

/// Whether `line` is inside the bounds a parse is given: its size, and how many nested
/// commands it opens — through a grouping character or a reserved word.
///
/// Checked WITHOUT parsing, which is the point: the parser is what the nesting bound
/// protects.
fn within_parse_bounds(line: &str, bounds: &Bounds) -> bool {
    if line.len() > bounds.command_bytes {
        return false;
    }
    let grouped = line
        .chars()
        .filter(|character| NESTING_INDICATORS.contains(character))
        .count();
    let worded = line
        .split(|character: char| character.is_whitespace() || WORD_SEPARATORS.contains(&character))
        .filter(|word| NESTING_WORDS.contains(word))
        .count();
    grouped.saturating_add(worded) <= bounds.nesting
}

/// The file descriptor a command's own writes go to, which every session performs a
/// redirection of.
const STANDARD_OUTPUT: i32 = 1;

/// The descriptor a command reports what it could not do on, and the one it reads from: the
/// two a session given a MACHINE's tree performs a redirection of too.
const STANDARD_ERROR: i32 = 2;

/// See [`STANDARD_ERROR`].
const STANDARD_INPUT: i32 = 0;

/// What one command SUBSTITUTION, or one redirected command, has captured so far.
#[derive(Debug, Default)]
struct Capture {
    /// The bytes the commands inside it wrote to standard output.
    text: String,
    /// How many of those bytes it may hold: what a file may hold for a redirected command
    /// ([`Bounds::file_bytes`]), and what a substitution may be worth
    /// ([`Bounds::substitution_bytes`]).
    limit: usize,
    /// Set once it went past that limit, in which case the write did not happen at all and
    /// the substitution has no value rather than a clipped one.
    overflowed: bool,
}

/// The session one caller-supplied program runs in: the variables its commands see and
/// change, what its `echo` commands wrote (when the caller keeps output), the files its
/// redirections left behind, and how much running is left in its budget.
#[derive(Debug)]
pub struct ShellSession {
    config: SessionConfig,
    /// The variables this session holds, which is what a word's parameter expansion resolves
    /// against and what a caller reads an exported variable from.
    variables: BTreeMap<String, String>,
    /// How many bytes those variables' names and values come to, kept as they are written
    /// rather than recomputed, so the session-bytes check costs one addition however many
    /// variables the session holds.
    held_bytes: usize,
    /// How many more bytes the line being run may expand to, reset for each line.
    expansion_left: usize,
    /// How many more commands this session may perform.
    commands_left: usize,
    /// The status `exit` ended this session with, once a command asked for one: every later
    /// line is then not performed at all, the way a shell stops at `exit`.
    exit_status: Option<i32>,
    /// What the program wrote, one entry per performed `echo`, kept only when the
    /// configuration asks for it.
    output: Vec<String>,
    /// The tree this session's commands read and write when it runs in a MACHINE's one
    /// ([`Self::in_volume`]) — `None` for a session whose tree is its own
    /// ([`Self::files`]), which performs neither the file utilities nor the forms that reach
    /// a machine's other descriptors.
    volume: Option<ShellVolume>,
    /// The command substitutions being captured, innermost last: while one is open, what a
    /// command writes to standard output is ITS value rather than the session's output.
    captures: Vec<Capture>,
    /// How many more substitutions and `sh -c` programs this session performs.
    substitutions_left: usize,
    /// How many `sh -c` programs are running inside one another right now, counted beside
    /// the captures above as the DEPTH this module's own recursion is bounded by.
    nested_programs: usize,
    /// The files this session's own tree holds, keyed by the path a redirection named — the
    /// tree of a session that was given no machine's.
    files: BTreeMap<String, String>,
    /// How many bytes that tree comes to, kept as it is written.
    tree_bytes: usize,
    /// Set when a write to the file a redirection named did not happen, which is what makes
    /// the command carrying it answer as one that could not be run.
    redirect_failed: bool,
}

impl ShellSession {
    /// A session holding `variables`, with the whole command budget `config` allows left.
    #[must_use]
    pub fn new(variables: BTreeMap<String, String>, config: SessionConfig) -> Self {
        let held_bytes = variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum();
        Self {
            variables,
            held_bytes,
            expansion_left: config.bounds.line_expansion_bytes,
            commands_left: config.bounds.commands,
            exit_status: None,
            output: Vec::new(),
            volume: None,
            captures: Vec::new(),
            substitutions_left: config.bounds.substitutions,
            nested_programs: 0,
            files: BTreeMap::new(),
            tree_bytes: 0,
            redirect_failed: false,
            config,
        }
    }

    /// A session holding `variables` that runs IN `volume`: a MACHINE's tree, which replaces
    /// the session's own as what its commands read and write, and the whole of what they can
    /// reach. See the module docs for the forms a session with one performs.
    #[must_use]
    pub fn in_volume(
        variables: BTreeMap<String, String>,
        config: SessionConfig,
        volume: ShellVolume,
    ) -> Self {
        Self {
            volume: Some(volume),
            ..Self::new(variables, config)
        }
    }

    /// The tree this session ran in, with everything its program wrote to it — `None` for a
    /// session that was given none. The caller keeps what it holds beside the machine, so
    /// the next run of a program on that machine reads what this one wrote.
    #[must_use]
    pub fn into_volume(self) -> Option<ShellVolume> {
        self.volume
    }

    /// The tree this session runs in, for a caller reading what the program has written to
    /// it so far.
    #[must_use]
    pub fn volume(&self) -> Option<&ShellVolume> {
        self.volume.as_ref()
    }

    /// The value this session holds for `name`, or `None` when it holds none.
    #[must_use]
    pub fn variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    /// The status `exit` ended this session with, or `None` while nothing has.
    #[must_use]
    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// The lines this session's `echo` commands wrote, in the order they wrote them. Empty
    /// for a session whose configuration keeps no output.
    #[must_use]
    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// [`Self::output`], taking the lines rather than borrowing them.
    #[must_use]
    pub fn into_output(self) -> Vec<String> {
        self.output
    }


    /// Perform one LINE of the program and answer the status the shell answered for it.
    ///
    /// A line that does not parse is the shell's own syntax error
    /// ([`Statuses::syntax_error`]); a line outside the bounds this parse is given
    /// ([`within_parse_bounds`]), or one reached after the command budget is spent, is a
    /// line this session did not perform at all, which is reported as a command it could
    /// not run ([`Statuses::could_not_run`]) rather than as one that succeeded.
    pub fn run(&mut self, line: &str) -> i32 {
        // Each line gets the whole expansion budget: the words of one line are live at the
        // same instant, and the words of the line before it are not.
        self.expansion_left = self.config.bounds.line_expansion_bytes;
        self.run_program(line)
    }

    /// [`Self::run`] without resetting the line's expansion budget: what a command
    /// substitution and an `sh -c` program are performed through, so the whole of one line
    /// — the commands nested inside it included — shares the one budget.
    fn run_program(&mut self, line: &str) -> i32 {
        if !within_parse_bounds(line, &self.config.bounds) || self.commands_left == 0 {
            return self.config.statuses.could_not_run;
        }
        let Ok(tokens) = ::brush_parser::tokenize_str(line) else {
            return self.config.statuses.syntax_error;
        };
        let options = ::brush_parser::ParserOptions::default();
        let Ok(program) = ::brush_parser::parse_tokens(&tokens, &options) else {
            return self.config.statuses.syntax_error;
        };
        let mut status = self.config.statuses.success;
        for complete_command in &program.complete_commands {
            for item in &complete_command.0 {
                status = self.run_and_or_list(&item.0);
                if self.exit_status.is_some() {
                    return status;
                }
            }
        }
        status
    }

    /// Perform an and-or list: its first pipeline, then each later pipeline only when the
    /// operator before it says so — `&&` runs the next on a zero status, `||` on a non-zero
    /// one — and answer the status of the last pipeline that ran.
    fn run_and_or_list(&mut self, list: &::brush_parser::ast::AndOrList) -> i32 {
        let mut status = self.run_pipeline(&list.first);
        for next in &list.additional {
            if self.exit_status.is_some() {
                break;
            }
            match next {
                ::brush_parser::ast::AndOr::And(pipeline) => {
                    if status == self.config.statuses.success {
                        status = self.run_pipeline(pipeline);
                    }
                }
                ::brush_parser::ast::AndOr::Or(pipeline) => {
                    if status != self.config.statuses.success {
                        status = self.run_pipeline(pipeline);
                    }
                }
            }
        }
        status
    }

    /// Perform a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!`.
    ///
    /// Every command of the pipeline is performed, in order, because each answers its own
    /// status; this session passes no bytes from one command to the next, so a pipeline's
    /// commands each read nothing.
    fn run_pipeline(&mut self, pipeline: &::brush_parser::ast::Pipeline) -> i32 {
        let mut status = self.config.statuses.success;
        for command in &pipeline.seq {
            status = self.run_command(command);
            if self.exit_status.is_some() {
                break;
            }
        }
        if pipeline.bang {
            status = if status == self.config.statuses.success {
                self.config.statuses.failure
            } else {
                self.config.statuses.success
            };
        }
        status
    }

    /// Perform one command of a pipeline.
    ///
    /// A SIMPLE command is the form this session performs. Every other form a shell grammar
    /// admits — a compound command, a function definition, an extended test — is one this
    /// session does not perform, and a command that did not run answers as one that could
    /// not be run rather than as one that succeeded.
    fn run_command(&mut self, command: &::brush_parser::ast::Command) -> i32 {
        match command {
            ::brush_parser::ast::Command::Simple(simple) => self.run_simple_command(simple),
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform a simple command: its variable assignments and its redirections, then the
    /// command its first word names with the words after it as its arguments, with what it
    /// writes going to the file a redirection names.
    ///
    /// A command with no word at all is the assignment form — "the variable assignments
    /// shall affect the current execution environment" — which is how a program sets a
    /// variable a caller later reads. An assignment written BEFORE a command word affects
    /// that command's environment alone, and persists in the session only for the special
    /// built-in utilities POSIX keeps it for ([`PERSISTING_COMMANDS`]); since every word is
    /// expanded before the assignments are applied, a command that does not keep them sees
    /// the value either way.
    ///
    /// An output redirection writes what the command wrote into the tree this session reads
    /// and writes ([`Self::apply_redirect`]); the other descriptors a machine's program
    /// redirects are performed only by a session that was given a machine's tree. Any other
    /// prefix or suffix item is a redirection form or a substitution this session does not
    /// perform, and the command is then not run.
    ///
    /// The file a redirection names is written whatever the command answered, the way a
    /// shell opens it before it resolves the command at all: a command this session has no
    /// utility for leaves the file it redirected to behind, empty.
    fn run_simple_command(&mut self, simple: &::brush_parser::ast::SimpleCommand) -> i32 {
        // The command budget is LATCHED, and it is spent per COMMAND rather than per line:
        // once it is gone no further command of any line is performed, however many a single
        // line carries.
        if self.commands_left == 0 {
            return self.config.statuses.could_not_run;
        }
        self.commands_left = self.commands_left.saturating_sub(1);
        let mut assignments: Vec<(String, String)> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let mut redirections = Redirections::default();
        let prefix = simple.prefix.as_ref().map(|prefix| &prefix.0);
        let suffix = simple.suffix.as_ref().map(|suffix| &suffix.0);
        for item in prefix.into_iter().chain(suffix).flatten() {
            match item {
                ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word) => {
                    let Some(expanded) = self.expand_word(&word.value) else {
                        return self.config.statuses.could_not_run;
                    };
                    words.push(expanded);
                }
                ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
                    let Some(pair) = self.assignment_of(assignment) else {
                        return self.config.statuses.could_not_run;
                    };
                    assignments.push(pair);
                }
                ::brush_parser::ast::CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                    if !self.apply_redirect(redirect, &mut redirections) {
                        return self.config.statuses.could_not_run;
                    }
                }
                _ => return self.config.statuses.could_not_run,
            }
        }
        let name = match simple.word_or_name.as_ref() {
            Some(word) => match self.expand_word(&word.value) {
                Some(expanded) => Some(expanded),
                None => return self.config.statuses.could_not_run,
            },
            None => None,
        };
        let persists = name
            .as_deref()
            .is_none_or(|name| PERSISTING_COMMANDS.contains(&name));
        if persists {
            for (name, value) in assignments {
                if !self.set_variable(name, value) {
                    return self.config.statuses.could_not_run;
                }
            }
        }
        // What a redirected command writes is collected for the file and written when the
        // command has finished, the way a shell's own redirection is one write of what the
        // command produced: a write the tree will not take then leaves the tree as it was.
        //
        // The refused-write flag is this command's own: a command nested inside it (a
        // substitution, an `sh -c` program) answers for its own refused write, and the one
        // held for a command still running is restored after it rather than consumed by the
        // nested one.
        let outer_refused = ::std::mem::replace(&mut self.redirect_failed, false);
        let redirected = redirections.out.take();
        if redirected.is_some() {
            self.captures.push(Capture {
                limit: self.config.bounds.file_bytes,
                ..Capture::default()
            });
        }
        let status = match name {
            Some(name) => self.run_built_in(&name, &words, &redirections),
            // Nothing but assignments, or a redirection alone: they have been applied, which
            // is the whole command.
            None => self.config.statuses.success,
        };
        if let Some((path, append)) = redirected {
            // The file a redirection names is written whatever the command answered, the way
            // a shell opens it before it resolves the command at all: a command this session
            // has no utility for leaves that file behind, empty.
            let captured = self.captures.pop().unwrap_or_default();
            if captured.overflowed || !self.write_path(&path, &captured.text, append) {
                self.redirect_failed = true;
            }
        }
        if ::std::mem::replace(&mut self.redirect_failed, outer_refused) {
            // A write the tree would not take did not happen, so the command did not do what
            // it was asked: reported as one that could not be run rather than as one that
            // wrote the file.
            return self.config.statuses.could_not_run;
        }
        status
    }

    /// One redirection of a simple command, recorded in `redirections` — `false` for a form
    /// this session does not perform.
    ///
    /// Every session performs the output redirections of standard output: `> path` writes the
    /// file from the start and `>> path` adds to what it holds ("the file shall be opened for
    /// appending"), and `>|` is `>` with the no-clobber option off. A session given a
    /// MACHINE's tree performs the other descriptors a machine's program redirects too —
    /// standard error to a file or a device (`2>`, `2>>`) and standard input from one (`<`).
    /// A duplication (`>&`), a read-write open, a here-document, a here-string and a
    /// descriptor other than 0, 1 and 2 are forms no session performs.
    fn apply_redirect(
        &mut self,
        redirect: &::brush_parser::ast::IoRedirect,
        redirections: &mut Redirections,
    ) -> bool {
        use ::brush_parser::ast::IoFileRedirectKind as Kind;
        let machine = self.volume.is_some();
        let ::brush_parser::ast::IoRedirect::File(fd, kind, target) = redirect else {
            return false;
        };
        let ::brush_parser::ast::IoFileRedirectTarget::Filename(word) = target else {
            return false;
        };
        let Some(path) = self.expand_word(&word.value) else {
            return false;
        };
        match kind {
            Kind::Read => {
                if !machine || fd.unwrap_or(0) != STANDARD_INPUT {
                    return false;
                }
                redirections.input = Some(path);
            }
            Kind::Write | Kind::Clobber | Kind::Append => {
                let append = matches!(kind, Kind::Append);
                match fd.unwrap_or(STANDARD_OUTPUT) {
                    STANDARD_OUTPUT => redirections.out = Some((path, append)),
                    STANDARD_ERROR if machine => redirections.error = Some((path, append)),
                    _ => return false,
                }
            }
            Kind::ReadAndWrite | Kind::DuplicateInput | Kind::DuplicateOutput => return false,
        }
        true
    }

    /// What reading `path` answers in the tree this session reads: a machine's tree where it
    /// was given one, and its own otherwise.
    fn read_path(&self, path: &str) -> Option<String> {
        match &self.volume {
            Some(volume) => volume.read(path),
            None => self.files.get(&file_path(path)?).cloned(),
        }
    }

    /// Write `text` at `path` in the tree this session writes, replacing what is there or
    /// ADDING to it.
    ///
    /// `false` is a write that did not happen — a path the tree does not accept, an entry it
    /// will not let a program write, or a write past one of its bounds — which the command
    /// performing it reports.
    fn write_path(&mut self, path: &str, text: &str, append: bool) -> bool {
        if let Some(volume) = self.volume.as_mut() {
            return volume.write(path, text, append);
        }
        let Some(path) = file_path(path) else {
            return false;
        };
        let mut whole = if append {
            self.files.get(&path).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        whole.push_str(text);
        if whole.len() > self.config.bounds.file_bytes {
            return false;
        }
        let displaced = self
            .files
            .get(&path)
            .map(|held| path.len().saturating_add(held.len()));
        if displaced.is_none() && self.files.len() >= self.config.bounds.files {
            return false;
        }
        let would_hold = self
            .tree_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(path.len())
            .saturating_add(whole.len());
        if would_hold > self.config.bounds.tree_bytes {
            return false;
        }
        self.tree_bytes = would_hold;
        self.files.insert(path, whole);
        true
    }

    /// One assignment of a simple command as the name and value it sets, or `None` when it
    /// is a form this session does not perform — an array assignment, an appending
    /// assignment, or a value whose expansion it cannot resolve.
    fn assignment_of(
        &mut self,
        assignment: &::brush_parser::ast::Assignment,
    ) -> Option<(String, String)> {
        if assignment.append {
            return None;
        }
        let ::brush_parser::ast::AssignmentName::VariableName(name) = &assignment.name else {
            return None;
        };
        let ::brush_parser::ast::AssignmentValue::Scalar(value) = &assignment.value else {
            return None;
        };
        Some((name.clone(), self.expand_word(&value.value)?))
    }

    /// Set `name` to `value` in this session, unless it is already holding
    /// [`Bounds::session_bytes`] or [`Bounds::session_variables`] — `false` is then a
    /// command this session did not perform, which is what its caller reports.
    fn set_variable(&mut self, name: String, value: String) -> bool {
        let displaced = self
            .variables
            .get(&name)
            .map(|held| name.len().saturating_add(held.len()));
        if displaced.is_none() && self.variables.len() >= self.config.bounds.session_variables {
            return false;
        }
        let would_hold = self
            .held_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(name.len())
            .saturating_add(value.len());
        if would_hold > self.config.bounds.session_bytes {
            return false;
        }
        self.held_bytes = would_hold;
        self.variables.insert(name, value);
        true
    }

    /// Remove `name` from this session, with the bytes it held.
    fn unset_variable(&mut self, name: &str) {
        if let Some(value) = self.variables.remove(name) {
            self.held_bytes = self
                .held_bytes
                .saturating_sub(name.len().saturating_add(value.len()));
        }
    }

    /// Write `text` to the program's standard OUTPUT, as one write of a command: to the
    /// path a redirection names, to the command substitution being captured, or — when
    /// neither — to what the session keeps for its caller.
    ///
    /// `false` is a write that did not happen, which is the failure the command performing
    /// it reports: a redirection the tree refused, or a capture past
    /// [`Bounds::substitution_bytes`].
    fn write_stdout(&mut self, text: &str, redirections: &Redirections) -> bool {
        self.write_stream(redirections.out.clone(), text)
    }

    /// Write `text` to the program's standard ERROR, which is where a command reports what
    /// it could not do.
    ///
    /// With a redirection of its own it goes to the file or device that names, and NOT to
    /// where standard output is going: standard error is not what a command substitution
    /// captures, and not what a redirection of standard output takes. With none, it reaches
    /// what the session keeps only for a session given a MACHINE's tree, whose console is
    /// where a guest's diagnostics are read; for every other session a diagnostic is not part
    /// of what the program wrote, so nothing keeps it.
    fn write_stderr(&mut self, text: &str, redirections: &Redirections) -> bool {
        if let Some((path, append)) = redirections.error.clone() {
            if !self.write_path(&path, text, append) {
                self.redirect_failed = true;
                return false;
            }
            return true;
        }
        if self.volume.is_some() {
            self.keep_output(text);
        }
        true
    }

    /// [`Self::write_stdout`]'s and [`Self::write_stderr`]'s one write.
    fn write_stream(&mut self, target: Option<(String, bool)>, text: &str) -> bool {
        if let Some((path, append)) = target {
            if !self.write_path(&path, text, append) {
                self.redirect_failed = true;
                return false;
            }
            return true;
        }
        if let Some(capture) = self.captures.last_mut() {
            if capture.text.len().saturating_add(text.len()) > capture.limit {
                capture.overflowed = true;
                return false;
            }
            capture.text.push_str(text);
            return true;
        }
        self.keep_output(text);
        true
    }

    /// Keep `text` for the caller, one entry per LINE of it: a console and a log are read
    /// by the line, so a command that wrote several lines is kept as several.
    ///
    /// Nothing is kept for a session whose configuration asks for no output. A line past
    /// [`OutputLimits::line_bytes`] is truncated on a character boundary, and a line past
    /// [`OutputLimits::lines`] is not kept at all.
    fn keep_output(&mut self, text: &str) {
        let Some(limits) = self.config.output else {
            return;
        };
        if text.is_empty() {
            // Nothing was written, so there is no line to keep: a command that wrote no
            // bytes at all is not one that wrote an empty line.
            return;
        }
        for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
            if self.output.len() >= limits.lines {
                return;
            }
            let mut line = line.to_string();
            if line.len() > limits.line_bytes {
                let mut cut = limits.line_bytes;
                while cut > 0 && !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.truncate(cut);
            }
            self.output.push(line);
        }
    }

    /// Perform the command `name` with `arguments`, when it is one this session provides —
    /// [`BUILT_IN_COMMANDS`], and [`VOLUME_COMMANDS`] too for a session given a MACHINE's
    /// tree.
    ///
    /// `echo` writes a line of the program's output, without its newline when it is given
    /// `-n`. `cat` writes each file the tree holds for the paths it was given. `true` and
    /// `:` answer success and `false` answers failure, which is the whole of what those
    /// utilities do. `exit` ends the session with the status its argument names, defaulting
    /// to success, and a status outside 0..=255 is taken modulo 256 the way a shell takes an
    /// exit status. `export` sets the variables its `NAME=VALUE` arguments name and `unset`
    /// removes them. The machine commands are [`Self::run_volume_command`]'s.
    fn run_built_in(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        // The catalog is what this session provides: a command outside it needs a program, a
        // host tree or a network this session does not have.
        if !BUILT_IN_COMMANDS.contains(&name) {
            if self.volume.is_some() && VOLUME_COMMANDS.contains(&name) {
                return self.run_volume_command(name, arguments, redirections);
            }
            return self.config.statuses.could_not_run;
        }
        match name {
            "echo" => {
                let (words, newline) = match arguments.split_first() {
                    Some((first, rest)) if first == "-n" => (rest, false),
                    _ => (arguments, true),
                };
                let text = if newline {
                    format!("{}\n", words.join(" "))
                } else {
                    words.join(" ")
                };
                if self.write_stdout(&text, redirections) {
                    self.config.statuses.success
                } else {
                    self.config.statuses.failure
                }
            }
            "cat" => self.run_cat(arguments, redirections),
            "true" | ":" => self.config.statuses.success,
            "false" => self.config.statuses.failure,
            "exit" => {
                let status = match arguments.first() {
                    Some(argument) => match argument.parse::<i64>() {
                        Ok(status) => {
                            i32::try_from(status.rem_euclid(256)).unwrap_or(self.config.statuses.failure)
                        }
                        // A status a shell cannot read is its own usage failure, and the
                        // session still ends.
                        Err(_) => self.config.statuses.failure,
                    },
                    None => self.config.statuses.success,
                };
                self.exit_status = Some(status);
                status
            }
            "export" => {
                for argument in arguments {
                    // `export NAME=VALUE` sets it; `export NAME` marks a variable this
                    // session already holds, and marking one it does not hold sets nothing.
                    if let Some((name, value)) = argument.split_once('=') {
                        if !self.set_variable(name.to_string(), value.to_string()) {
                            return self.config.statuses.could_not_run;
                        }
                    }
                }
                self.config.statuses.success
            }
            "unset" => {
                for argument in arguments {
                    self.unset_variable(argument);
                }
                self.config.statuses.success
            }
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform one of the commands a session given a MACHINE's tree provides
    /// ([`VOLUME_COMMANDS`]), over that tree and nothing else.
    ///
    /// `printf` writes its format with its arguments substituted. `mkdir` makes a
    /// directory (`-p` makes the ones above it too), `touch` makes an empty file where the
    /// tree holds none, and `rm` removes a file (`-f` passes over one that is not there).
    /// `test`, and its `[` spelling, answers a question about the tree or about two values.
    /// `sh -c` performs a program in THIS session, which is how a machine's init system
    /// runs one line of a script.
    fn run_volume_command(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        match name {
            "cat" => self.run_cat(arguments, redirections),
            "printf" => self.run_printf(arguments, redirections),
            "mkdir" => self.run_mkdir(arguments, redirections),
            "touch" => self.run_touch(arguments, redirections),
            "rm" => self.run_remove(arguments, redirections),
            "test" | "[" => self.run_test(name, arguments),
            "sh" | "bash" => self.run_nested_program(arguments),
            _ => self.config.statuses.could_not_run,
        }
    }

    /// `cat`: the contents of each file named, in order, on standard output.
    ///
    /// Read out of the tree this session reads ([`Self::read_path`]), so a session given a
    /// machine's tree reads that machine's files and every other session reads its own. "cat
    /// … shall exit with a value >0 if an error occurred", and a path the tree holds no file
    /// for is one this session could not read: it is reported on standard error in the
    /// utility's own wording and the command answers failure, without stopping the files
    /// after it.
    ///
    /// With no file named it reads what standard input was redirected from; a session with
    /// no such redirection has no terminal to read, so that form is one it does not perform.
    fn run_cat(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let mut paths: Vec<String> = arguments
            .iter()
            .filter(|argument| !argument.starts_with('-'))
            .cloned()
            .collect();
        if paths.is_empty() {
            paths.extend(redirections.input.clone());
        }
        if paths.is_empty() {
            return self.config.statuses.could_not_run;
        }
        let mut status = self.config.statuses.success;
        for path in paths {
            // The tree answers a read only for what can be read: a file of its own, or a
            // device that discards what is written to it (and so has nothing to give back).
            match self.read_path(&path) {
                Some(contents) => {
                    if !self.write_stdout(&contents, redirections) {
                        status = self.config.statuses.failure;
                    }
                }
                None => {
                    let _ = self.write_stderr(
                        &format!("cat: {path}: No such file or directory\n"),
                        redirections,
                    );
                    status = self.config.statuses.failure;
                }
            }
        }
        status
    }

    /// `printf`: the format with its arguments substituted, on standard output.
    ///
    /// The conversions performed are the ones a boot script writes with — `%s`, `%d`/`%i`
    /// and `%%` — and the escapes are `\n`, `\t`, `\r` and `\\`. The format is applied
    /// ONCE, however many arguments are left over, and a conversion outside that set is one
    /// this command does not perform: nothing is written and it answers failure.
    fn run_printf(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some((format, rest)) = arguments.split_first() else {
            let _ = self.write_stderr("printf: usage: printf format [arguments]\n", redirections);
            return self.config.statuses.failure;
        };
        let Some(text) = formatted(format, rest) else {
            let _ = self.write_stderr(
                &format!("printf: {format}: invalid format\n"),
                redirections,
            );
            return self.config.statuses.failure;
        };
        if self.write_stdout(&text, redirections) {
            self.config.statuses.success
        } else {
            self.config.statuses.failure
        }
    }

    /// `mkdir`: the directories named, with the ones above them when `-p` asks for it.
    fn run_mkdir(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let parents = arguments.iter().any(|argument| argument == "-p");
        let mut status = self.config.statuses.success;
        for path in arguments.iter().filter(|argument| !argument.starts_with('-')) {
            let Some(volume) = self.volume.as_mut() else {
                return self.config.statuses.could_not_run;
            };
            if volume.is_directory(path) {
                if !parents {
                    let _ = self.write_stderr(
                        &format!("mkdir: cannot create directory '{path}': File exists\n"),
                        redirections,
                    );
                    status = self.config.statuses.failure;
                }
                continue;
            }
            // Without `-p` a machine makes ONE directory, so the one above it has to be
            // there already.
            let has_parent = path
                .rsplit_once('/')
                .is_none_or(|(parent, _)| volume.is_directory(if parent.is_empty() { "/" } else { parent }));
            if (!parents && !has_parent) || !volume.make_directories(path) {
                let _ = self.write_stderr(
                    &format!("mkdir: cannot create directory '{path}': No such file or directory\n"),
                    redirections,
                );
                status = self.config.statuses.failure;
            }
        }
        status
    }

    /// `touch`: an empty file where the tree holds none, and nothing at all where it already
    /// holds one — this session keeps no modification time to move.
    fn run_touch(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let mut status = self.config.statuses.success;
        for path in arguments.iter().filter(|argument| !argument.starts_with('-')) {
            let Some(volume) = self.volume.as_mut() else {
                return self.config.statuses.could_not_run;
            };
            if volume.exists(path) {
                continue;
            }
            if !volume.write(path, "", false) {
                let _ = self.write_stderr(
                    &format!("touch: cannot touch '{path}': No such file or directory\n"),
                    redirections,
                );
                status = self.config.statuses.failure;
            }
        }
        status
    }

    /// `rm`: the files named removed from the tree. A file that is not there is the
    /// utility's own failure unless `-f` was given, and a directory is not removed by this
    /// command at all.
    fn run_remove(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let forced = arguments
            .iter()
            .any(|argument| argument.starts_with('-') && argument.contains('f'));
        let mut status = self.config.statuses.success;
        for path in arguments.iter().filter(|argument| !argument.starts_with('-')) {
            let Some(volume) = self.volume.as_mut() else {
                return self.config.statuses.could_not_run;
            };
            if volume.is_directory(path) {
                let _ = self.write_stderr(
                    &format!("rm: cannot remove '{path}': Is a directory\n"),
                    redirections,
                );
                status = self.config.statuses.failure;
                continue;
            }
            if !volume.remove_file(path) && !forced {
                let _ = self.write_stderr(
                    &format!("rm: cannot remove '{path}': No such file or directory\n"),
                    redirections,
                );
                status = self.config.statuses.failure;
            }
        }
        status
    }

    /// `test` / `[`: the question its arguments ask, answered as a status — success for
    /// true, failure for false.
    ///
    /// The questions answered are the ones a boot script asks: whether the tree holds a
    /// path (`-e`), a file (`-f`), a directory (`-d`) or a file with something in it
    /// (`-s`), whether a string is empty (`-z`) or not (`-n`), whether two strings are
    /// equal (`=`, `!=`), and how two integers compare (`-eq`, `-ne`, `-lt`, `-le`, `-gt`,
    /// `-ge`). A single argument is true when it is not empty. A form outside that set is
    /// one this command does not answer, which is reported as a command that could not run
    /// rather than as a false answer.
    fn run_test(&mut self, name: &str, arguments: &[String]) -> i32 {
        let mut arguments = arguments.to_vec();
        if name == "[" {
            // The `[` spelling is the same utility, and its last argument is the closing
            // bracket rather than part of the question.
            if arguments.last().map(String::as_str) != Some("]") {
                return self.config.statuses.could_not_run;
            }
            arguments.pop();
        }
        let negated = arguments.first().map(String::as_str) == Some("!");
        if negated {
            arguments.remove(0);
        }
        let Some(answer) = self.test_answer(&arguments) else {
            return self.config.statuses.could_not_run;
        };
        if answer != negated {
            self.config.statuses.success
        } else {
            self.config.statuses.failure
        }
    }

    /// [`Self::run_test`]'s reading of the question, or `None` for a form it does not
    /// answer.
    fn test_answer(&self, arguments: &[String]) -> Option<bool> {
        let volume = self.volume.as_ref()?;
        match arguments {
            [] => Some(false),
            [value] => Some(!value.is_empty()),
            [operator, value] => match operator.as_str() {
                "-e" => Some(volume.exists(value)),
                "-f" => Some(volume.is_file(value)),
                "-d" => Some(volume.is_directory(value)),
                "-s" => Some(volume.read(value).is_some_and(|held| !held.is_empty())),
                "-r" | "-w" => Some(volume.exists(value)),
                "-z" => Some(value.is_empty()),
                "-n" => Some(!value.is_empty()),
                _ => None,
            },
            [left, operator, right] => match operator.as_str() {
                "=" | "==" => Some(left == right),
                "!=" => Some(left != right),
                "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
                    let left = left.trim().parse::<i64>().ok()?;
                    let right = right.trim().parse::<i64>().ok()?;
                    Some(match operator.as_str() {
                        "-eq" => left == right,
                        "-ne" => left != right,
                        "-lt" => left < right,
                        "-le" => left <= right,
                        "-gt" => left > right,
                        _ => left >= right,
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `sh -c PROGRAM` (and `bash -c`): the program performed in THIS session, under the
    /// same bounds and against the same tree and variables.
    ///
    /// It counts against the same budget and the same DEPTH a command substitution does
    /// ([`Bounds::substitutions`], [`Bounds::substitution_depth`]): a nested shell is the
    /// other way one program can run another, so what bounds this module's own recursion has
    /// to count both.
    ///
    /// Any other invocation — a script named by path, an interactive shell, a login shell —
    /// needs something this session does not have, so it is a command that could not run.
    fn run_nested_program(&mut self, arguments: &[String]) -> i32 {
        let Some((flag, rest)) = arguments.split_first() else {
            return self.config.statuses.could_not_run;
        };
        if flag != "-c" {
            return self.config.statuses.could_not_run;
        }
        let Some(program) = rest.first() else {
            return self.config.statuses.could_not_run;
        };
        if self.substitutions_left == 0 || self.nesting_depth() >= self.config.bounds.substitution_depth
        {
            return self.config.statuses.could_not_run;
        }
        self.substitutions_left = self.substitutions_left.saturating_sub(1);
        self.nested_programs = self.nested_programs.saturating_add(1);
        let status = self.run_program(program);
        self.nested_programs = self.nested_programs.saturating_sub(1);
        status
    }

    /// How deeply one program is running inside another right now: the substitutions being
    /// captured and the nested shells around them, which are the two ways a program here
    /// runs another.
    fn nesting_depth(&self) -> usize {
        self.captures.len().saturating_add(self.nested_programs)
    }

    /// `word` with its expansions resolved, or `None` when it carries an expansion this
    /// session cannot resolve.
    ///
    /// The pieces come from `brush-parser`'s word parser, so what is quoted, what is a
    /// parameter expansion and what is literal text is the shell's own reading of the word.
    /// A parameter this session holds nothing for expands to nothing, which is what an unset
    /// parameter expands to. A command substitution and an arithmetic expansion are resolved
    /// only by a session that runs in a volume ([`Self::expand_substitution`],
    /// [`Self::expand_arithmetic`]); for a session with no tree, and for a tilde prefix
    /// whichever session carries it — there is no home directory here — the word has no
    /// expansion and the command carrying it is not run. Nor has a word that expands past
    /// [`Bounds::expansion_bytes`], or one whose expansion would take the line past
    /// [`Bounds::line_expansion_bytes`].
    fn expand_word(&mut self, word: &str) -> Option<String> {
        let options = ::brush_parser::ParserOptions::default();
        let pieces = ::brush_parser::word::parse(word, &options).ok()?;
        let expanded = self.expand_pieces(&pieces, self.config.bounds.word_depth)?;
        self.expansion_left = self.expansion_left.checked_sub(expanded.len())?;
        Some(expanded)
    }

    /// [`Self::expand_word`]'s walk over the pieces a word parsed into, with `depth` levels
    /// of quoting left: a word nested past [`Bounds::word_depth`] has no expansion here, so
    /// this walk's own recursion stays in range whatever the word nests.
    fn expand_pieces(
        &mut self,
        pieces: &[::brush_parser::word::WordPieceWithSource],
        depth: usize,
    ) -> Option<String> {
        let depth = depth.checked_sub(1)?;
        let mut expanded = String::new();
        for piece in pieces {
            let addition = match &piece.piece {
                ::brush_parser::word::WordPiece::Text(text)
                | ::brush_parser::word::WordPiece::SingleQuotedText(text) => text.clone(),
                ::brush_parser::word::WordPiece::DoubleQuotedSequence(inner) => {
                    let inner = inner.clone();
                    self.expand_pieces(&inner, depth)?
                }
                ::brush_parser::word::WordPiece::ParameterExpansion(parameter) => {
                    self.expand_parameter(parameter)?
                }
                ::brush_parser::word::WordPiece::CommandSubstitution(program)
                | ::brush_parser::word::WordPiece::BackquotedCommandSubstitution(program) => {
                    let program = program.clone();
                    self.expand_substitution(&program)?
                }
                ::brush_parser::word::WordPiece::ArithmeticExpression(expression) => {
                    let text = expression.value.clone();
                    self.expand_arithmetic(&text)?
                }
                ::brush_parser::word::WordPiece::AnsiCQuotedText(_)
                | ::brush_parser::word::WordPiece::GettextDoubleQuotedSequence(_)
                | ::brush_parser::word::WordPiece::EscapeSequence(_)
                | ::brush_parser::word::WordPiece::TildeExpansion(_) => return None,
            };
            if expanded.len().saturating_add(addition.len()) > self.config.bounds.expansion_bytes {
                return None;
            }
            expanded.push_str(&addition);
        }
        Some(expanded)
    }

    /// The value of a command SUBSTITUTION: what the commands inside it wrote to standard
    /// output, "with one or more trailing newline characters removed", as the shell
    /// specifies.
    ///
    /// The commands run in THIS session — the same variables, the same tree, the same
    /// budget — so a substitution costs commands of [`Bounds::commands`] like any other
    /// command and cannot reach anything its session cannot. An `exit` inside one ends the
    /// substitution rather than the session, the way it ends a subshell.
    ///
    /// `None` is a substitution with no value: a session that was given no machine's tree
    /// does not perform the form at all, one past [`Bounds::substitutions`] or
    /// [`Bounds::substitution_depth`] does not run it, and one whose output went past
    /// [`Bounds::substitution_bytes`] has no value rather than a clipped one.
    fn expand_substitution(&mut self, program: &str) -> Option<String> {
        self.volume.as_ref()?;
        if self.substitutions_left == 0 || self.nesting_depth() >= self.config.bounds.substitution_depth
        {
            return None;
        }
        self.substitutions_left = self.substitutions_left.saturating_sub(1);
        self.captures.push(Capture {
            limit: self.config.bounds.substitution_bytes,
            ..Capture::default()
        });
        let held_exit = self.exit_status.take();
        let _ = self.run_program(program);
        self.exit_status = held_exit;
        let capture = self.captures.pop()?;
        if capture.overflowed {
            return None;
        }
        Some(capture.text.trim_end_matches('\n').to_string())
    }

    /// The value of an ARITHMETIC expansion (`$((…))`): the number its expression comes to
    /// in this session.
    ///
    /// The text's own parameter expansions are resolved first — `$((count + 1))` and
    /// `$(($count + 1))` are both written — and then the expression is parsed by
    /// `brush-parser`'s arithmetic grammar and evaluated over this session's variables
    /// ([`super::posix_shell_arithmetic`]).
    ///
    /// `None` is an expansion with no value: a session that was given no machine's tree does
    /// not perform the form, and neither does an expression past the bound a parse is given
    /// ([`within_parse_bounds`], applied to the text the expansion materialised rather than
    /// only to the line it came from), one that does not parse, one this module does not
    /// evaluate, or one whose arithmetic has no answer (a division by zero, an overflow).
    fn expand_arithmetic(&mut self, text: &str) -> Option<String> {
        self.volume.as_ref()?;
        let resolved = self.expand_word(text).unwrap_or_else(|| text.to_string());
        // The expansion above can have MATERIALISED text far longer and far more deeply
        // nested than the line itself — a variable's value expands inside the expression —
        // so the same bound a line is read under is applied to it before it reaches a
        // grammar: a parser costs a level of recursion per nested group, and that recursion
        // is what this keeps in range.
        if !within_parse_bounds(&resolved, &self.config.bounds) {
            return None;
        }
        let expression = ::brush_parser::arithmetic::parse(&resolved).ok()?;
        let depth = self.config.bounds.word_depth;
        let value = posix_shell_arithmetic::evaluate(&expression, depth, self)?;
        Some(value.to_string())
    }

    /// The value a parameter expansion resolves to in this session.
    ///
    /// The plain form (`$NAME`, `${NAME}`) is the variable's value, empty when the session
    /// holds none, and the default-value form (`${NAME:-WORD}`) is that value or the default
    /// the expansion names. Every other form — a substring, a replacement, a length, an
    /// indirection, a positional or special parameter — is one this session does not
    /// resolve.
    fn expand_parameter(&self, parameter: &::brush_parser::word::ParameterExpr) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => Some(self.variable(name).unwrap_or_default().to_string()),
            ::brush_parser::word::ParameterExpr::UseDefaultValues {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
                default_value,
                ..
            } => {
                let held = self.variable(name).unwrap_or_default();
                if held.is_empty() {
                    Some(default_value.clone().unwrap_or_default())
                } else {
                    Some(held.to_string())
                }
            }
            _ => None,
        }
    }
}

/// The variables an arithmetic expansion of this session reads and assigns: the session's
/// own, under the session's own bounds — an assignment the session will not take (it is at
/// [`Bounds::session_bytes`] or [`Bounds::session_variables`]) is one the expansion could
/// not perform.
impl ArithmeticVariables for ShellSession {
    fn arithmetic_value(&self, name: &str) -> Option<String> {
        self.variable(name).map(str::to_string)
    }

    fn set_arithmetic_value(&mut self, name: &str, value: i64) -> bool {
        self.set_variable(name.to_string(), value.to_string())
    }
}

/// `format` with `arguments` substituted, as `printf` writes it, or `None` for a format
/// this session does not perform.
///
/// The conversions are `%s` (the argument as it is), `%d` and `%i` (the argument read as an
/// integer, and a value that is not one is worth zero, as the utility reads it) and `%%`
/// (one per cent sign). The escapes are `\n`, `\t`, `\r` and `\\`. An argument the format
/// has no conversion for is left out, and a conversion with no argument left takes the
/// empty string, which is what the utility writes for one.
fn formatted(format: &str, arguments: &[String]) -> Option<String> {
    let mut written = String::new();
    let mut taken = arguments.iter();
    let mut characters = format.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('n') => written.push('\n'),
                Some('t') => written.push('\t'),
                Some('r') => written.push('\r'),
                Some('\\') => written.push('\\'),
                // Every other escape is one this command does not perform.
                _ => return None,
            },
            '%' => match characters.next() {
                Some('%') => written.push('%'),
                Some('s') => written.push_str(taken.next().map_or("", String::as_str)),
                Some('d' | 'i') => {
                    let argument = taken.next().map_or("", String::as_str);
                    let value = argument.trim().parse::<i64>().unwrap_or(0);
                    written.push_str(&value.to_string());
                }
                // Every other conversion is one this command does not perform.
                _ => return None,
            },
            plain => written.push(plain),
        }
    }
    Some(written)
}

#[cfg(test)]
mod tests {
    use super::{
        Bounds, OutputLimits, SessionConfig, ShellSession, ShellVolume, Statuses,
        BUILT_IN_COMMANDS,
    };
    use std::collections::BTreeMap;

    /// A session that keeps its output, with the default statuses and bounds.
    fn session() -> ShellSession {
        ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        )
    }

    /// Perform every line of `program` in one session and answer it.
    fn run(program: &str) -> ShellSession {
        let mut held = session();
        for line in program.lines() {
            held.run(line);
            if held.exit_status().is_some() {
                break;
            }
        }
        held
    }

    /// RULE: only the catalog's utilities are performed; every other command and every form
    /// this session does not perform answers "could not run" and writes nothing.
    #[test]
    fn only_the_catalog_is_performed() {
        let statuses = Statuses::default();
        // (case, the line, the status it answers, the lines it wrote)
        let rows: [(&str, &str, i32, &[&str]); 9] = [
            ("echo", "echo one two", statuses.success, &["one two"]),
            ("true", "true", statuses.success, &[]),
            (":", ":", statuses.success, &[]),
            ("false", "false", statuses.failure, &[]),
            (
                "a host program",
                "curl https://example.invalid",
                statuses.could_not_run,
                &[],
            ),
            (
                "an output redirection writes the session's own file",
                "echo out > kept.txt",
                statuses.success,
                &[],
            ),
            (
                "an input redirection",
                "cat < kept.txt",
                statuses.could_not_run,
                &[],
            ),
            (
                "a command substitution",
                "echo $(whoami)",
                statuses.could_not_run,
                &[],
            ),
            (
                "a compound command",
                "if true; then echo inside; fi",
                statuses.could_not_run,
                &[],
            ),
        ];
        for (case, line, expected_status, expected_output) in rows {
            let mut held = session();
            let status = held.run(line);
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected status {expected_status}, got {status}"
            );
            assert_eq!(
                held.output(),
                expected_output,
                "case {case:?}: expected {expected_output:?}, got {:?}",
                held.output()
            );
        }
        assert_eq!(
            BUILT_IN_COMMANDS.len(),
            8,
            "case the catalog: expected the eight session-local utilities, got {BUILT_IN_COMMANDS:?}"
        );
    }

    /// RULE: a program's commands write FILES in the session's own tree — an output
    /// redirection keeps what the command wrote under the path it named (`>` from the start,
    /// `>>` added to what the file holds), `cat` reads one back, and a path the tree holds no
    /// file for is one `cat` could not read.
    ///
    /// The rows differ along what the rule generalises over: which redirection operator the
    /// program used, how many commands wrote the file, the path's spelling, whether the file
    /// was ever written, and which command the redirection applies to.
    #[test]
    fn a_program_s_commands_write_files_in_the_session_s_tree() {
        let statuses = Statuses::default();
        // (case, the program, the status its last line answered, the lines it wrote)
        let rows: [(&str, &str, i32, &[&str]); 8] = [
            (
                "a redirected write read back",
                "echo first > out.txt\ncat out.txt",
                statuses.success,
                &["first"],
            ),
            (
                "a second write from the start replaces the file",
                "echo first > out.txt\necho second > out.txt\ncat out.txt",
                statuses.success,
                &["second"],
            ),
            (
                "an appending write adds to it",
                "echo first > out.txt\necho second >> out.txt\ncat out.txt",
                statuses.success,
                &["first", "second"],
            ),
            (
                "an appending write to a file nothing wrote yet",
                "echo only >> out.txt\ncat out.txt",
                statuses.success,
                &["only"],
            ),
            (
                "the same file named with and without a leading ./",
                "echo first > ./out.txt\ncat out.txt",
                statuses.success,
                &["first"],
            ),
            (
                "a path nothing wrote",
                "cat missing.txt",
                statuses.failure,
                &[],
            ),
            (
                "an expansion names the path and the content",
                "NAME=out.txt\nWHAT=written\necho $WHAT > $NAME\ncat out.txt",
                statuses.success,
                &["written"],
            ),
            (
                "a redirected command this session has no utility for leaves the file empty",
                "mvn package > out.txt\ncat out.txt",
                statuses.success,
                &[],
            ),
        ];
        for (case, program, expected_status, expected_output) in rows {
            let mut held = session();
            let mut status = statuses.success;
            for line in program.lines() {
                status = held.run(line);
            }
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected status {expected_status}, got {status}"
            );
            assert_eq!(
                held.output(),
                expected_output,
                "case {case:?}: expected {expected_output:?}, got {:?}",
                held.output()
            );
        }
    }

    /// RULE: the tree is bounded like everything else — a write past the per-file ceiling,
    /// one past the whole tree's, and a file past the file count are each a write this
    /// session did not perform, and the file it would have written is not there.
    #[test]
    fn the_tree_s_bounds_stop_the_write() {
        let statuses = Statuses::default();

        // One file may hold no more than `file_bytes`.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    file_bytes: 8,
                    ..Bounds::default()
                },
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        );
        let status = held.run("echo 0123456789 > out.txt");
        assert_eq!(
            status, statuses.could_not_run,
            "case the per-file ceiling: expected {}, got {status}",
            statuses.could_not_run
        );
        assert_eq!(
            held.run("cat out.txt"),
            statuses.failure,
            "case the per-file ceiling: the file must not be there, got {:?}",
            held.output()
        );

        // The tree holds no more than `files` files.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    files: 1,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("echo one > first.txt"),
            statuses.success,
            "case the file count: the first file is written"
        );
        assert_eq!(
            held.run("echo two > second.txt"),
            statuses.could_not_run,
            "case the file count: a file past the count is not written"
        );
        assert_eq!(
            held.run("echo again > first.txt"),
            statuses.success,
            "case the file count: a file the tree already holds is still written"
        );

        // The whole tree holds no more than `tree_bytes`.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    tree_bytes: 24,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("echo onetwothree > first.txt"),
            statuses.success,
            "case the tree ceiling: the first file fits"
        );
        assert_eq!(
            held.run("echo onetwothree > second.txt"),
            statuses.could_not_run,
            "case the tree ceiling: the write past it is not performed"
        );
    }

    /// RULE: a variable the program sets is what its parameter expansions resolve against,
    /// an unset parameter expands to nothing, and `unset` removes one.
    #[test]
    fn variables_are_the_session_s_own() {
        // (case, the program, the line it wrote)
        let rows: [(&str, &str, &str); 5] = [
            ("an assignment and an expansion", "A=one\necho $A", "one"),
            ("export sets a variable", "export B=two\necho ${B}", "two"),
            ("an unset parameter", "echo [${ABSENT}]", "[]"),
            (
                "a default value",
                "echo ${ABSENT:-fallback}",
                "fallback",
            ),
            ("unset removes it", "C=three\nunset C\necho [$C]", "[]"),
        ];
        for (case, program, expected) in rows {
            let held = run(program);
            let written = held.output().last().cloned().unwrap_or_default();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// RULE: `exit` ends the session with the status it names, and nothing after it is
    /// performed.
    #[test]
    fn exit_ends_the_session() {
        for (case, program, expected_status, expected_lines) in [
            ("exit with no status", "echo one\nexit\necho two", 0, 1),
            ("exit 3", "echo one\nexit 3\necho two", 3, 1),
            ("exit past 255 wraps", "exit 300", 44, 0),
        ] {
            let held = run(program);
            assert_eq!(
                held.exit_status(),
                Some(expected_status),
                "case {case:?}: expected the session to end with {expected_status}, got {:?}",
                held.exit_status()
            );
            assert_eq!(
                held.output().len(),
                expected_lines,
                "case {case:?}: expected {expected_lines} line(s), got {:?}",
                held.output()
            );
        }
    }

    /// RULE: every bound is the session's own — the command budget, the line size, the
    /// nesting count, the variable count and the output ceilings each stop the work.
    #[test]
    fn every_bound_stops_the_work() {
        let statuses = Statuses::default();

        // The command budget is latched: past it a line performs nothing.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    commands: 2,
                    ..Bounds::default()
                },
                output: Some(OutputLimits {
                    lines: 8,
                    line_bytes: 64,
                }),
                ..SessionConfig::default()
            },
        );
        for _ in 0..4 {
            held.run("echo line");
        }
        assert_eq!(
            held.output().len(),
            2,
            "case the command budget: expected 2 performed commands, got {:?}",
            held.output()
        );

        // A line past the size bound, and one past the nesting bound, are not read at all.
        let mut held = session();
        let long = format!("echo {}", "x".repeat(Bounds::default().command_bytes));
        assert_eq!(
            held.run(&long),
            statuses.could_not_run,
            "case an oversized line: expected it to perform nothing"
        );
        let nested = format!("echo {}", "(".repeat(Bounds::default().nesting + 1));
        assert_eq!(
            held.run(&nested),
            statuses.could_not_run,
            "case a deeply nested line: expected it to perform nothing"
        );
        assert!(
            held.output().is_empty(),
            "case the parse bounds: expected no output, got {:?}",
            held.output()
        );

        // The variable count is bounded: a further assignment performs nothing.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    session_variables: 1,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        assert_eq!(
            held.run("A=one"),
            statuses.success,
            "case the first variable: expected it to be set"
        );
        assert_eq!(
            held.run("B=two"),
            statuses.could_not_run,
            "case the variable ceiling: expected the assignment to perform nothing"
        );

        // The output ceilings: at most `lines` entries, each at most `line_bytes` bytes.
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                output: Some(OutputLimits {
                    lines: 2,
                    line_bytes: 4,
                }),
                ..SessionConfig::default()
            },
        );
        for _ in 0..5 {
            held.run("echo abcdefgh");
        }
        assert_eq!(
            held.output(),
            ["abcd", "abcd"],
            "case the output ceilings: expected two truncated lines, got {:?}",
            held.output()
        );

        // A session that keeps no output keeps none, whatever the program writes.
        let mut held = ShellSession::new(BTreeMap::new(), SessionConfig::default());
        held.run("echo nothing-is-kept");
        assert!(
            held.output().is_empty(),
            "case no output limits: expected nothing kept, got {:?}",
            held.output()
        );
    }

    /// A session that runs in a tree holding a counter, a sealed identity, a console device
    /// and a null device — the shape of a machine a boot script runs on.
    fn in_volume() -> ShellSession {
        let mut volume = ShellVolume::default();
        volume.put_file("/var/lib/counter", "1\n");
        volume.seal_file(
            "/proc/sys/kernel/random/boot_id",
            "6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30\n",
        );
        volume.add_keeping_device("/dev/console");
        volume.add_discarding_device("/dev/null");
        ShellSession::in_volume(
            BTreeMap::new(),
            SessionConfig {
                output: Some(OutputLimits {
                    lines: 16,
                    line_bytes: 256,
                }),
                ..SessionConfig::default()
            },
            volume,
        )
    }

    /// RULE: a session that runs in a volume performs the forms a program of a machine is
    /// written in — the file utilities, redirections, command substitution and arithmetic
    /// expansion — over that tree and nothing else.
    #[test]
    fn a_session_in_a_volume_performs_a_machine_s_program() {
        let statuses = Statuses::default();
        // (case, the program, the status, the lines it wrote to the console device)
        let rows: [(&str, &str, i32, &[&str]); 9] = [
            (
                "a redirection to a device",
                "echo marker > /dev/console",
                statuses.success,
                &["marker"],
            ),
            (
                "a command substitution reading a sealed file",
                "echo id $(cat /proc/sys/kernel/random/boot_id) > /dev/console",
                statuses.success,
                &["id 6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30"],
            ),
            (
                "an arithmetic expansion over a file's contents",
                "C=$(cat /var/lib/counter)\necho next $((C + 1)) > /dev/console",
                statuses.success,
                &["next 2"],
            ),
            (
                "a file written and read back",
                "echo kept > /var/lib/other\ncat /var/lib/other > /dev/console",
                statuses.success,
                &["kept"],
            ),
            (
                "an appending redirection",
                "echo one >> /dev/console\necho two >> /dev/console",
                statuses.success,
                &["one", "two"],
            ),
            (
                "a failure reported on standard error",
                "cat /var/lib/absent 2>/dev/console",
                statuses.failure,
                &["cat: /var/lib/absent: No such file or directory"],
            ),
            (
                "a discarded failure",
                "cat /var/lib/absent 2>/dev/null",
                statuses.failure,
                &[],
            ),
            (
                "a question about the tree",
                "test -f /var/lib/counter && echo there > /dev/console",
                statuses.success,
                &["there"],
            ),
            (
                "a program run through sh -c",
                "sh -c 'printf \"%s=%d\\n\" count 7 > /dev/console'",
                statuses.success,
                &["count=7"],
            ),
        ];
        for (case, program, expected_status, expected_console) in rows {
            let mut held = in_volume();
            let mut status = statuses.success;
            for line in program.lines() {
                status = held.run(line);
            }
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected status {expected_status}, got {status}"
            );
            let volume = held.into_volume().expect("the session ran in a volume");
            assert_eq!(
                volume.device_lines("/dev/console"),
                expected_console,
                "case {case:?}: expected the console to carry {expected_console:?}, got {:?}",
                volume.device_lines("/dev/console")
            );
        }
    }

    /// RULE: what a program wrote to the tree is what the caller takes back out, so the next
    /// run of a program on that machine reads what this one wrote — which is how a boot
    /// script keeps a count across boots.
    #[test]
    fn what_a_program_wrote_to_the_tree_comes_back_out() {
        let program = [
            "COUNT=$(cat /var/lib/counter)",
            "COUNT=$((COUNT + 1))",
            "echo $COUNT > /var/lib/counter",
            "echo boot $COUNT > /dev/console",
        ];
        let mut carried: Option<ShellVolume> = None;
        // (the boot, the count the console should carry)
        for (boot, expected) in [(1_u32, "boot 2"), (2, "boot 3")] {
            let mut held = match carried.take() {
                Some(volume) => ShellSession::in_volume(
                    BTreeMap::new(),
                    SessionConfig {
                        output: Some(OutputLimits {
                            lines: 16,
                            line_bytes: 256,
                        }),
                        ..SessionConfig::default()
                    },
                    volume,
                ),
                None => in_volume(),
            };
            for line in program {
                held.run(line);
            }
            let volume = held.into_volume().expect("the session ran in a volume");
            let console = volume.device_lines("/dev/console").to_vec();
            assert_eq!(
                console,
                [expected],
                "case run {boot}: expected the console to carry {expected:?}, got {console:?}"
            );
            // The next run starts from the tree this one left, with its console cleared the
            // way a device of a new boot is.
            let mut next = ShellVolume::default();
            for (path, contents) in volume.stored_files() {
                next.put_file(path, contents);
            }
            next.seal_file(
                "/proc/sys/kernel/random/boot_id",
                "6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30\n",
            );
            next.add_keeping_device("/dev/console");
            next.add_discarding_device("/dev/null");
            carried = Some(next);
        }
    }

    /// RULE: every bound a session in a volume adds is the emulator's own — how many
    /// substitutions one session performs, how deeply they nest, how many bytes one of them
    /// may capture, and the bound an expansion's own TEXT is read under before it reaches a
    /// grammar — and a program past one of them performs nothing rather than more.
    #[test]
    fn every_volume_bound_stops_the_work() {
        let statuses = Statuses::default();
        let config = |bounds: Bounds| SessionConfig {
            bounds,
            output: Some(OutputLimits {
                lines: 8,
                line_bytes: 64,
            }),
            ..SessionConfig::default()
        };
        let volume = || {
            let mut volume = ShellVolume::default();
            volume.put_file("/var/lib/counter", "1\n");
            volume.add_keeping_device("/dev/console");
            volume
        };

        // The substitution budget: past it a word carrying one has no expansion, so the
        // command is not performed.
        let mut held = ShellSession::in_volume(
            BTreeMap::new(),
            config(Bounds {
                substitutions: 1,
                ..Bounds::default()
            }),
            volume(),
        );
        assert_eq!(
            held.run("echo $(cat /var/lib/counter) > /dev/console"),
            statuses.success,
            "case the first substitution: expected it to be performed"
        );
        assert_eq!(
            held.run("echo $(cat /var/lib/counter) > /dev/console"),
            statuses.could_not_run,
            "case the substitution budget: expected the command to perform nothing"
        );

        // The capture ceiling: a substitution whose output goes past it has no value.
        let mut held = ShellSession::in_volume(
            BTreeMap::new(),
            config(Bounds {
                substitution_bytes: 4,
                ..Bounds::default()
            }),
            volume(),
        );
        assert_eq!(
            held.run("echo $(echo abcdefghijkl) > /dev/console"),
            statuses.could_not_run,
            "case the capture ceiling: expected the command to perform nothing"
        );

        // The nesting ceiling: a substitution inside a substitution is performed while the
        // nesting is inside the bound, and past it the inner one has no value at all, so
        // the word carrying it expands without what that command would have written.
        // (case, the depth allowed, what the console then carries)
        for (case, depth, expected) in [("inside the depth", 2, "1"), ("past the depth", 1, "")] {
            let mut held = ShellSession::in_volume(
                BTreeMap::new(),
                config(Bounds {
                    substitution_depth: depth,
                    ..Bounds::default()
                }),
                volume(),
            );
            held.run("echo $(echo $(cat /var/lib/counter)) > /dev/console");
            let written = held.into_volume().expect("a volume").device_lines("/dev/console").to_vec();
            assert_eq!(
                written,
                [expected],
                "case {case:?}: expected the console to carry {expected:?}, got {written:?}"
            );
        }

        // The bound an ARITHMETIC expansion's own text is read under: the expansion of a
        // variable can materialise text far more deeply nested than the line itself, and a
        // parser costs a level of recursion per nested group, so the expansion has no value
        // past the bound rather than driving that recursion.
        let mut held = ShellSession::in_volume(
            BTreeMap::from([("NESTED".to_string(), "(".repeat(64))]),
            config(Bounds {
                nesting: 8,
                ..Bounds::default()
            }),
            volume(),
        );
        assert_eq!(
            held.run("echo $(( $NESTED 1 )) > /dev/console"),
            statuses.could_not_run,
            "case the expansion's own nesting: expected the command to perform nothing"
        );
        assert_eq!(
            held.run("echo $(( 1 + 1 )) > /dev/console"),
            statuses.success,
            "case an expansion inside the bound: expected it to be performed"
        );
        let written = held
            .into_volume()
            .expect("a volume")
            .device_lines("/dev/console")
            .to_vec();
        assert_eq!(
            written,
            ["2"],
            "case the expansion's own nesting: expected only the bounded expansion's value \
             on the console, got {written:?}"
        );
    }

    /// RULE: a session that was given no MACHINE's tree performs none of the forms a machine's
    /// program is written in — there is no device to write, no descriptor to report on and no
    /// storage of the machine to read — so what such a session performs is its own catalog
    /// over its own tree.
    #[test]
    fn a_session_with_no_volume_performs_none_of_them() {
        let statuses = Statuses::default();
        // (case, the line a program of a machine is written with)
        let rows = [
            ("a command substitution", "echo $(cat out.txt)"),
            ("an arithmetic expansion", "echo $((1 + 1))"),
            ("a redirection reading a file", "cat < out.txt"),
            ("a redirection of standard error", "cat missing 2> err.txt"),
            ("a question about a tree", "test -f out.txt"),
            ("a nested program", "sh -c 'echo inside'"),
            ("making a directory", "mkdir -p /var/lib/sim"),
            ("removing a file", "rm -f out.txt"),
            ("writing a formatted line", "printf '%s\\n' value"),
        ];
        for (case, line) in rows {
            let mut held = session();
            let status = held.run(line);
            assert_eq!(
                status, statuses.could_not_run,
                "case {case:?}: expected the status a shell answers for a command it cannot \
                 run, got {status}"
            );
            assert!(
                held.output().is_empty(),
                "case {case:?}: expected nothing written, got {:?}",
                held.output()
            );
        }
    }
}
