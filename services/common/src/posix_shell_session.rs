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
//! starting that command as one. A host process and a host file are exactly the primitives
//! this module has none of, and the crate exposes no seam to replace them with an in-memory
//! volume or an in-process dispatcher — its command resolution and its redirections go
//! straight to the host through the standard library — so there is no configuration of it
//! that could be confined here: taking it would hand a caller's program the developer's
//! machine.
//!
//! What this module does instead is narrower than a shell by construction, and it is not an
//! implementation of the shell LANGUAGE: there is no evaluator for the language's values or
//! its expansions beyond the forms named here, and none is approximated. It PERFORMS a fixed
//! catalog of session-local utilities ([`BUILT_IN_COMMANDS`]) over the real grammar's tree,
//! beside the programs the caller's own environment declares it provides
//! ([`EnvironmentPrograms`]) — and, for a session that works over a tree, the grammar's
//! control flow around those same commands — and every other command, and every command FORM
//! and expansion it does not perform, fails closed with the status a shell answers for a
//! command it cannot run ([`Statuses::could_not_run`]). There is therefore no language subset
//! to keep in step with a specification: a command either names one of those, or it did not
//! run. A command whose NAME nothing resolved is where the shell's own command search ends
//! with nothing, so it is reported the way a shell reports that: the diagnostic naming the
//! command on standard error, beside the status
//! ([`ShellSession::command_not_found`]).
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
//! A caller whose environment HAS storage of its own — a build container's source directory,
//! which the service seeds and then takes the produced files back out of — declares that
//! ([`ShellSession::with_file_utilities`]), and the file utilities ([`VOLUME_COMMANDS`]) are
//! then commands the session performs over that same tree. It is still the one tree above:
//! a path is a key, nothing holds a directory entry, and nothing reaches a host path.
//!
//! # The forms a session that works over a tree performs
//!
//! A program written against storage is written as a SCRIPT, so a session that was given a
//! tree — a machine's ([`ShellSession::in_volume`]) or its own working one — also performs
//! the grammar's own control flow over the commands inside it: `if`/`elif`/`else`, `while`,
//! `until` and `for`, and a brace group — with the standard output of the whole compound
//! command redirected where it carries such a redirection. Each of those runs the same
//! catalog this module performs and needs no primitive beyond it, each command inside spends
//! the same budget ([`Bounds::commands`]), and that budget is what ends a loop with no end of
//! its own. A subshell, a `case` clause, a function definition and a compound command
//! redirecting another descriptor are forms no session performs.
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
//! * the file utilities ([`VOLUME_COMMANDS`]) beside the session-local catalog, which name
//!   what the tree holds as well as change it;
//! * the WORKING DIRECTORY of the session, which `cd` changes and `pwd` writes: a path a
//!   program names without a directory is resolved against it ([`ShellSession::at`]), which is
//!   how a script written for a machine names a file it put somewhere;
//! * the redirections that reach the tree's other descriptors: standard error to a file or a
//!   device, and standard input from one — and the standard output of a whole COMPOUND
//!   command, which is how such a script writes one file out of several lines;
//! * arithmetic expansion ([`super::posix_shell_arithmetic`]), which is how such a program
//!   computes what it writes — performed by every session that works over a tree.
//!
//! None of that reaches a host path, a process or a socket: the tree is in memory, every
//! entry of it was put there by the service that owns the machine, and a path it does not
//! hold resolves to nothing. A session constructed WITHOUT one performs none of those forms,
//! and every other redirection form (another file descriptor, a duplication, a
//! here-document) is one no session performs.
//!
//! # The programs the ENVIRONMENT provides
//!
//! A shell's own utilities are not the only commands a program names: the rest are programs
//! the environment the session runs in has installed, and which ones those are is the
//! caller's fact, not this module's — a build environment installs what its build
//! specification selected, and a session with nothing installed provides nothing beyond the
//! catalog. A caller that has such programs hands this session an
//! [`EnvironmentPrograms`], which is asked for a command the catalog does not name and
//! answers either what the program it provides for it came to — the status it exited with and
//! what it wrote on EACH of its two streams ([`ProgramStreams`]) — or `None` for a name it
//! provides none for, and `None` is the same closed failure as before. Nothing here
//! resolves a command against a host `PATH`, and an [`EnvironmentPrograms`] is handed no way
//! to reach a process, a file or the host: it answers out of what its own caller holds, the
//! FILES of this session's own tree ([`ProgramFiles`]), which is what a program whose work is
//! to carry a file somewhere reads and writes — still the one tree above, where a path is a
//! key and no host path is reachable — and the VARIABLES this session holds, which are the
//! environment the command runs with ([`ProgramInvocation`]), the way a program a shell runs
//! is given the shell's own environment.
//!
//! # The programs the TREE holds
//!
//! The other half of the shell's own COMMAND SEARCH, for a session that works over a tree: a
//! name the catalogs do not hold and the environment provides no program for is looked for IN
//! THAT TREE ([`ShellSession::performed_tree_program`]) — the file a name carrying a slash
//! points at, or the first directory of the session's own `PATH` holding a file of that name —
//! and what is found is performed as a SCRIPT in this session, with the operands as its
//! positional parameters, the way `sh FILE ARG…` is and under the same budget. That is how a
//! program a caller's environment really ships (an executable under a Lambda layer's
//! `/opt/bin`, a script an earlier command installed) is run by its own name, at the top level
//! of a line and inside a nested program alike.
//!
//! The search is over the TREE and over nothing else: there is no host `PATH` here, no host
//! file is opened for it, and a session given no tree resolves no program this way at all. A
//! file whose own interpreter line names another program needs THAT interpreter, which is a
//! program the environment has to provide like any other; where it provides none, the search
//! ends with nothing and nothing runs.
//!
//! # Pipelines, and the filters that read what one wrote
//!
//! A pipeline CONNECTS its commands: "the standard output of the first command shall be
//! connected to the standard input of the second command", so what the last command of a
//! pipeline wrote is what the pipeline wrote, and every stage before it wrote to the stage
//! after it rather than to the session. That is what makes a program's own computation
//! observable: the bytes one command produced are the bytes the next one counts, digests or
//! selects from. A stage that redirects its own standard output sends it to the file instead,
//! as the shell does, and so passes nothing on; what one stage may pass to the next is bounded
//! by [`Bounds::substitution_bytes`], and a stage past that bound ends the pipeline rather than
//! leaving the stage after it to answer for a stream nothing produced.
//!
//! The commands that READ that input are the text filters ([`TEXT_FILTER_COMMANDS`], whose
//! catalog, options and output forms are stated in
//! [`super::posix_shell_text_filters`]) and `cat` with no file operand: each answers out of ONE
//! input — the file it was given, the stage before it, or the file standard input was
//! redirected from — and writes what it computed over it. A caller whose own ENVIRONMENT
//! provides a program of that name keeps it: the environment is asked first, and the filter is
//! only what a session whose environment provides none falls back to. Nothing else here reads
//! standard input: there is no terminal behind it, so a filter with no input at all is a
//! command that could not run.
//!
//! # Command substitution
//!
//! `$(…)` is a command whose OUTPUT is a word of another command, so EVERY session performs
//! it the way the shell specifies — it is the shell's own form rather than a machine's: the
//! substituted text is read by the same grammar and performed in this session, and what it
//! wrote, with trailing newlines removed, is the word. It is performed in a SUBSHELL: the
//! variables it sets and an `exit` it performs do not reach the session that substituted it,
//! rolled back off a log of what it CHANGED rather than off a copy of the store. Performing
//! one spends a command of the same budget before anything else — so how many a line may open
//! is bounded whatever their text performs — the commands inside it spend their own, what it
//! captures is bounded by [`Bounds::substitution_bytes`], how many one session performs by
//! [`Bounds::substitutions`], and how deeply they may nest by
//! [`Bounds::substitution_depth`]: a substituted command is no wider, and no cheaper, than a
//! command written on the line itself, and the catalogs and the environment's programs are
//! all it can name either.
//!
//! # Bounds
//!
//! Every bound is the emulator's, held in code the program cannot reach, and every one of
//! them is a field of [`Bounds`]: how many commands one run performs (latched — past it a
//! line performs nothing, and performing a command substitution spends one too), how long a
//! line may be and how deeply it may nest (checked WITHOUT parsing, which is what keeps the
//! grammar's own recursion in range), how much one word and one line may expand to, how many
//! variables and how many bytes of them the session holds — which holds for the variables it
//! is HANDED exactly as for the ones its program sets — and, for a caller that KEEPS what the
//! program wrote ([`OutputLimits`]), how many lines and how many bytes per line it may keep.
//!
//! # The MEMORY the caller's environment gives the program
//!
//! One limit is NOT one of those: the memory the environment the program runs in has
//! ([`SessionConfig::memory`]). That is the caller's own fact — the memory a container's
//! declaration states for it — and it is a HARD limit the way such a declaration is: a
//! program whose values would take what it holds past that much is KILLED where it stands
//! ([`ShellSession::killed_for_memory`], the status [`KILLED_BY_MEMORY_STATUS`]), so nothing
//! of it after that point is performed and what it had already written is the whole of what
//! it produced. The demand is weighed BEFORE the bytes are held, the way a runtime refuses an
//! allocation past a process's limit rather than making it first. A caller whose environment
//! states no such limit leaves it absent, and the bounds above are then the only ceilings.
//!
//! # The services that drive it
//!
//! * `service-com-amazonaws-codebuild-codebuild-20161006`'s `state::build_shell` performs a
//!   buildspec's command lines for an exit status and keeps no output, which is this module
//!   with [`OutputLimits`] absent, and hands it the programs of the runtimes that build
//!   installed as its [`EnvironmentPrograms`];
//! * `service-com-amazonaws-sagemaker-sagemaker`'s `state::notebook_lifecycle_hook` performs
//!   a notebook instance's lifecycle hook and keeps its output, which it publishes to that
//!   hook's CloudWatch Logs stream.
//! * `service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore`'s
//!   `state::code_interpreter_shell` performs a code interpreter session's `executeCommand`
//!   and `startCommandExecution` programs over that session's own workspace, and reports the
//!   two streams separately ([`ShellSession::output`], [`ShellSession::errors`]) because its
//!   API carries a member for each.
//!
//! # The other session, and what the two SHARE
//!
//! `service-com-amazonaws-ecs-amazonec2containerservicev20141113`'s `state::container_shell`
//! performs a container's program as a PROCESS: it loops, waits, installs signal handlers and
//! delivers each line as it is written, which this session does none of — it is driven
//! SYNCHRONOUSLY by callers that perform a whole program while holding their own store's lock
//! (`…lambdamicrovms…::state::guest_machine::in_volume` hands this session a machine's tree
//! from inside that guard, and `…ec2…::state::guest_console` performs a boot script the same
//! way), so the walk over the grammar's tree cannot be the same function in both until those
//! callers no longer need a synchronous one.
//!
//! What IS the same is everything that needs no await, and it lives in ONE place rather than
//! in two copies:
//!
//! * the FILES both sessions work over and the utilities that work over them — the seam, `cat`,
//!   `ls`, `mkdir`, `touch`, `rm`, `sync`, the `test` questions about a file, and the
//!   redirections of the three standard streams ([`super::posix_shell_files`]);
//! * the TEXT FILTERS ([`super::posix_shell_text_filters`]);
//! * the `test` questions about values alone ([`value_test_answer`]);
//! * the in-memory TREE a machine's files are held in ([`super::posix_shell_volume`]).
//!
//! Nothing new should restate any of those — call these modules. A new session needs only its
//! own walk, and only where the one here cannot serve it.

use std::collections::BTreeMap;

use super::posix_shell_arithmetic::{self, ArithmeticVariables};
use super::posix_shell_files::{
    self, AppliedRedirect, FileRefusal, HereInput, Redirections, ShellFiles,
};
use super::posix_shell_text_filters::{Filter, STANDARD_INPUT_LABEL, TEXT_FILTER_COMMANDS};
use super::posix_shell_volume::{
    MountFacts as ShellMountFacts, MountedTree as ShellMountedTree, ShellVolume,
};

/// The commands a session provides, as their names.
///
/// Each one's whole effect is inside the session, which is why it can be performed in
/// process: `echo` writes a line of the program's output, `cat` writes a file this session
/// holds, `true`, `false` and `:` answer a status, `exit` ends the session,
/// `export`/`unset` change its variables, and `set` changes the OPTIONS the commands after
/// it are performed under ([`ShellOption`]) — which is what a script's own first line does.
/// A command that is not one of these is either a
/// TEXT FILTER ([`TEXT_FILTER_COMMANDS`]) or a file utility ([`VOLUME_COMMANDS`]) this
/// session performs, or it needs a program, which is the environment's to provide
/// ([`EnvironmentPrograms`]) and not the shell's, and is answered as a command that could not
/// be run when the environment provides none of that name.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "echo",
    "cat",
    "true",
    "false",
    ":",
    "exit",
    "export",
    "unset",
    SET_COMMAND,
    BREAK_COMMAND,
    CONTINUE_COMMAND,
    SEQUENCE_COMMAND,
];

/// The built-ins that leave the enclosing LOOP: `break` — "exit from the smallest enclosing
/// for, while, or until loop" — and `continue`, which goes on with "the next iteration of the
/// smallest enclosing" one. Each takes an optional count of enclosing loops to leave, and
/// neither is a command of its own: what it does is change which command runs NEXT
/// ([`ShellSession::run_loop_control`]), which is why a session holds it rather than the
/// command answering out of it.
const BREAK_COMMAND: &str = "break";
const CONTINUE_COMMAND: &str = "continue";

/// The utility that writes a SEQUENCE of numbers — "print numbers from FIRST to LAST, in steps
/// of INCREMENT" — which is what a program written as "try this a few times" iterates over
/// (`for i in $(seq 6)`). Its whole effect is the lines it writes, which is why every session
/// performs it ([`ShellSession::run_sequence`]).
const SEQUENCE_COMMAND: &str = "seq";

/// The built-in utility that changes the OPTIONS the shell performs the rest of the program
/// under ([`ShellOption`]), which is what a script's own first line does.
const SET_COMMAND: &str = "set";

/// What one program of the environment a session runs in answered: the status it exited
/// with and the lines it wrote.
#[derive(Debug, Clone)]
pub struct ProgramOutcome {
    /// The status the program exited with, which is the status the command carrying it
    /// answers.
    pub status: i32,
    /// The lines the program wrote, in the order it wrote them.
    pub output: Vec<String>,
}

/// The programs the ENVIRONMENT a session runs in provides, beyond the shell's own
/// utilities ([`BUILT_IN_COMMANDS`]).
///
/// A session asks this for a command whose name the catalog does not hold. An
/// implementation answers only out of state its own caller holds and out of the FILES of the
/// session's tree it is handed ([`ProgramFiles`]) — there is no process to start, no host
/// `PATH` to resolve against and no file of the host to open here — and answers `None`
/// for every name it provides no program for, which the session then reports as a command
/// it could not run.
pub trait EnvironmentPrograms: ::std::fmt::Debug + Send + Sync {
    /// What this environment's program `name` answers for `arguments`, or `None` when this
    /// environment provides no program called `name` — or performs nothing of that
    /// invocation of it.
    ///
    /// `files` is the tree of the session the command was written in: the files a program
    /// carrying one somewhere reads its source out of and writes its destination into. A
    /// program whose work touches no file leaves it alone.
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome>;

    /// [`Self::run`] for a program that reads STANDARD INPUT: `standard_input` is what the
    /// stage before it in a pipeline wrote, or the file an input redirection named, and
    /// `None` when the command was given neither.
    ///
    /// This is the form the session asks, so a program that reads its input (a login taking
    /// a password on standard input, a filter) overrides it; the default performs the
    /// invocation without reading one, which is what a program that ignores standard input
    /// does. A name this answers `None` for leaves the input in place for the utility the
    /// session falls back to.
    fn run_with_input(
        &self,
        name: &str,
        arguments: &[String],
        standard_input: Option<&str>,
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        let _ = standard_input;
        self.run(name, arguments, files)
    }

    /// [`Self::run_with_input`] for one whole INVOCATION: the form the session ASKS, which
    /// carries the environment the command runs with ([`ProgramInvocation::environment`]) and
    /// is answered on both of the program's streams ([`ProgramStreams`]).
    ///
    /// A program that reads the variables the session holds, or that writes a diagnostic of
    /// its own, overrides this; the default performs the invocation through
    /// [`Self::run_with_input`] and reports what it answered as standard output alone, which
    /// is what a program that reads no environment and writes no diagnostic does.
    fn run_invocation(
        &self,
        invocation: &ProgramInvocation<'_>,
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramStreams> {
        self.run_with_input(
            invocation.name,
            invocation.arguments,
            invocation.standard_input,
            files,
        )
        .map(|outcome| ProgramStreams {
            status: outcome.status,
            output: outcome.output,
            errors: Vec::new(),
        })
    }
}

/// ONE invocation of a program the environment provides, as the session performs it.
///
/// It carries everything of the command that is not the session itself: the name it was
/// written with, the words after it, what it reads on standard input, and the VARIABLES the
/// session holds — which is the environment a program the shell runs is given, so an
/// interpreter or a utility that reads one is answered by what the lines before it set.
#[derive(Debug)]
pub struct ProgramInvocation<'a> {
    /// The name the command was written with ([`EnvironmentPrograms::run`]'s `name`).
    pub name: &'a str,
    /// The words after it.
    pub arguments: &'a [String],
    /// What the stage before it in a pipeline wrote, the text a here-document carries, or the
    /// file standard input was redirected from — `None` when the command was given none.
    pub standard_input: Option<&'a str>,
    /// The DIRECTORY the session is in, which is what a path the command names without one is
    /// resolved against — and what a program whose own default is "the directory I was started
    /// in" is started in.
    pub working_directory: &'a str,
    /// The variables the session holds, which are the environment this invocation runs with.
    pub environment: &'a BTreeMap<String, String>,
}

/// What one program answered on BOTH of its streams
/// ([`EnvironmentPrograms::run_invocation`]).
#[derive(Debug, Clone, Default)]
pub struct ProgramStreams {
    /// The status the program exited with, which is the status the command carrying it
    /// answers.
    pub status: i32,
    /// The lines it wrote to standard output, in the order it wrote them.
    pub output: Vec<String>,
    /// The lines it wrote to standard ERROR, in the order it wrote them — taken by the
    /// session's error stream, apart from the output above, so what a program reported it
    /// could not do is not part of what it wrote.
    pub errors: Vec<String>,
}

/// The programs of TWO environments over one session, asked in order.
///
/// What an environment provides is not always held in one place: a machine has the programs
/// its own owner installed in it, and a caller that reaches into that machine may supply the
/// ones only IT can — a program whose answers come from state the machine's owner cannot read.
/// This layers the two without either having to know the other's names: a command is asked of
/// the FIRST catalog, and of the second only where the first provides no program of that name,
/// so the machine's own program wins a name they both carry.
#[derive(Debug)]
pub struct LayeredPrograms {
    /// Asked first: the environment whose own programs these are.
    own: Box<dyn EnvironmentPrograms>,
    /// Asked for a name the first provides nothing for.
    supplied: Box<dyn EnvironmentPrograms>,
}

/// The two catalogs as one, `own` taking precedence ([`LayeredPrograms`]).
#[must_use]
pub fn layered(
    own: Box<dyn EnvironmentPrograms>,
    supplied: Box<dyn EnvironmentPrograms>,
) -> Box<dyn EnvironmentPrograms> {
    Box::new(LayeredPrograms { own, supplied })
}

impl EnvironmentPrograms for LayeredPrograms {
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        self.own
            .run(name, arguments, files)
            .or_else(|| self.supplied.run(name, arguments, files))
    }

    fn run_with_input(
        &self,
        name: &str,
        arguments: &[String],
        standard_input: Option<&str>,
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        self.own
            .run_with_input(name, arguments, standard_input, files)
            .or_else(|| {
                self.supplied
                    .run_with_input(name, arguments, standard_input, files)
            })
    }

    fn run_invocation(
        &self,
        invocation: &ProgramInvocation<'_>,
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramStreams> {
        self.own
            .run_invocation(invocation, files)
            .or_else(|| self.supplied.run_invocation(invocation, files))
    }
}

/// The value `word` carries for one of the options `names`, in either spelling an option takes
/// it in: joined to the option (`-tnfs4`, `--types=nfs4`) or as the next word.
///
/// The ONE reading of an option word for the PROGRAMS an environment provides
/// ([`EnvironmentPrograms`]), so two of them cannot read the same option grammar two ways:
/// `None` when `word` is not one of those options at all, and `Some(None)` for one written
/// with no value after it — the command's own usage failure, and so an invocation the program
/// performs nothing of.
///
/// `words` is the rest of the command's words, advanced only where the value is the next one.
#[must_use]
pub fn program_option_value<'a>(
    word: &str,
    words: &mut impl Iterator<Item = &'a String>,
    names: &[&str],
) -> Option<Option<String>> {
    for name in names {
        if word == *name {
            return Some(words.next().cloned());
        }
        if let Some(joined) = word.strip_prefix(name) {
            // `--name=value` carries its separator; `-nvalue` does not.
            let joined = joined.strip_prefix('=').unwrap_or(joined);
            if !joined.is_empty() {
                return Some(Some(joined.to_string()));
            }
        }
    }
    None
}

/// The FILES of the tree a session works over, as the ENVIRONMENT's own programs reach them.///
/// It is the same tree the session's redirections write and its `cat` reads — a machine's
/// where it was given one, and the session's own otherwise — so a program hands a file the
/// commands around it wrote on, and leaves one they read back. A path is a KEY of that tree:
/// nothing here opens, creates or removes a path of the host.
pub trait ProgramFiles {
    /// What reading `path` answers in that tree, or `None` when it holds no file there.
    fn read(&self, path: &str) -> Option<String>;

    /// Write `contents` at `path`, replacing what the tree holds there.
    ///
    /// `false` is a write that did not happen — a path the tree does not accept, an entry it
    /// will not let a program write, or a write past one of its bounds — which the program
    /// reports as its own failure.
    fn write(&mut self, path: &str, contents: &str) -> bool;

    /// Remove the file at `path`, answering whether there was one to remove — what a program
    /// that MOVES a file does to the one it carried away.
    fn remove(&mut self, path: &str) -> bool;

    /// The names directly under the DIRECTORY `path` names (the tree's root for the empty
    /// path), each once and in path order — what a program that LISTS a directory answers.
    ///
    /// The default is empty, which is what a tree that holds no directory entry of its own
    /// answers for every path: an environment whose programs list a directory is one whose
    /// session implements this.
    fn entries_in(&self, path: &str) -> Vec<String> {
        let _ = path;
        Vec::new()
    }

    /// Make `path` a directory of that tree, with every directory above it, answering
    /// whether the tree now has it — what a program that INSTALLS something has to do for
    /// the directories what it installs ships, so the commands around it can write files
    /// under them.
    ///
    /// The default is `false`, which is "this tree has no directory to make": an environment
    /// whose programs make one is one whose session implements this.
    fn make_directories(&mut self, path: &str) -> bool {
        let _ = path;
        false
    }

    /// Mount the filesystem `tree` at `path` in that tree, so every path under it is a path
    /// of that filesystem from now on — what a program that MOUNTS one does.
    ///
    /// The filesystem is another component's, held by whoever owns it, and the program that
    /// mounts it is the one that could resolve it ([`super::posix_shell_volume::MountedTree`]).
    /// `false` is a mount that did not happen, which the program reports as its own failure.
    ///
    /// The default is `false`, which is "nothing can be mounted into this tree": a session
    /// whose tree takes a mount is one that implements this.
    fn mount(&mut self, path: &str, tree: super::posix_shell_volume::MountedTree) -> bool {
        let _ = (path, tree);
        false
    }

    /// Take the filesystem mounted at `path` away again, answering whether one was mounted
    /// there. The default is `false`, as [`Self::mount`]'s is.
    fn unmount(&mut self, path: &str) -> bool {
        let _ = path;
        false
    }

    /// What is mounted AT `path` exactly, and `None` for a path that is not a mount point —
    /// what a program LISTING the mounts of the machine answers out of.
    fn mounted_at(&self, path: &str) -> Option<super::posix_shell_volume::MountFacts> {
        let _ = path;
        None
    }

    /// What filesystem `path` lies ON, and `None` for a path on the tree's own storage —
    /// what a program reporting the FILESYSTEM of a path answers out of.
    fn file_system_at(&self, path: &str) -> Option<super::posix_shell_volume::MountFacts> {
        let _ = path;
        None
    }

    /// How many bytes the tree's OWN storage holds now, as one of its files accounts for its
    /// path and its contents — what a program reporting how FULL the filesystem is reports,
    /// and `None` for a tree that keeps no account of them.
    ///
    /// It counts the files a program may write and not the ones the service SEALED into the
    /// tree: a published seed is the service's own, not the machine's storage, which is the
    /// same reading the owner of the tree takes of it when the run is over
    /// ([`ShellSession::tree_files`]).
    fn stored_byte_count(&self) -> Option<usize> {
        None
    }
}

/// The commands a session whose tree is a MACHINE's — or its own WORKING tree
/// ([`ShellSession::with_file_utilities`]) — provides on top of [`BUILT_IN_COMMANDS`], as
/// their names.
///
/// Each one's whole effect is inside the session and the tree it works over: `printf` writes
/// one line of output, `mkdir`, `touch` and `rm` change the tree, `cp` and `mv` duplicate and
/// rename what it holds, `sync` flushes what was written to it (nothing is pending in a tree a
/// write reaches at once, so it writes nothing
/// and succeeds), `tee` writes what it reads
/// into the files it names and passes it on, `ls` names what the tree
/// holds, `test` (and its `[`
/// spelling) answers a question about it, `sh` performs a program in this same session —
/// `sh -c PROGRAM` the text it was given and `sh FILE` the script the TREE holds at that
/// path, which is how a machine's init system runs one line and how a script left in the
/// tree is run — `.` (with its bash `source` spelling) performs the commands of a file the
/// tree holds in this same session, which is how a program reads a file of assignments the
/// environment published to it, and `sleep` states the TIME the program took
/// ([`ShellSession::slept`]). A session given neither tree provides none of them: a
/// directory, a removal, a nested program and a file to read commands from are what an
/// environment with storage of its own has, and such a session's tree is only the files its
/// redirections left.
///
/// Every utility the shared file-utility reader performs over a session's tree
/// ([`posix_shell_files::FILE_COMMANDS`]) is named here or among [`BUILT_IN_COMMANDS`]: a
/// name it performs and no catalog holds would be resolved as a PROGRAM and reported as a
/// command the shell could not find, however completely this module implements it.
pub const VOLUME_COMMANDS: &[&str] = &[
    "printf",
    "mkdir",
    "touch",
    "rm",
    posix_shell_files::COPY_COMMAND,
    posix_shell_files::MOVE_COMMAND,
    SYNC_COMMAND,
    TEE_COMMAND,
    LIST_COMMAND,
    "test",
    "[",
    "sh",
    "bash",
    ".",
    "source",
    "sleep",
];

/// The command of [`VOLUME_COMMANDS`] that FLUSHES what was written to the tree, performed by
/// the shared file-utility reader like the other utilities that work over it.
const SYNC_COMMAND: &str = "sync";

/// The commands only a session working over a MACHINE's tree provides
/// ([`ShellSession::in_volume`]), as their names: `cd` changes the DIRECTORY the paths a
/// program names without one are resolved against — "cd: change the working directory" — and
/// `pwd` writes it.
///
/// They are not among [`VOLUME_COMMANDS`] because a session whose tree is its OWN
/// ([`ShellSession::with_file_utilities`]) has no directory to be in: such a tree keys a file
/// by the path the program named, so neither command is one that session performs, and a name
/// its own environment provides a program for stays that program's.
pub const MACHINE_TREE_COMMANDS: &[&str] = &[CHANGE_DIRECTORY, PRINT_DIRECTORY];

/// The command of [`MACHINE_TREE_COMMANDS`] that changes the working directory, and the one
/// that writes it.
const CHANGE_DIRECTORY: &str = "cd";
const PRINT_DIRECTORY: &str = "pwd";

/// The variable a shell publishes the working directory in: "PWD … shall be set by the cd
/// utility", beside the one it publishes the directory before it in, which is what `cd -`
/// changes back to.
const WORKING_DIRECTORY_VARIABLE: &str = "PWD";
const PREVIOUS_DIRECTORY_VARIABLE: &str = "OLDPWD";

/// The variable `cd` with no operand changes to: "if no directory operand is given and the
/// HOME environment variable is set to a non-empty value, the cd utility shall behave as if
/// the directory named in the HOME variable was specified".
const HOME_VARIABLE: &str = "HOME";

/// The operand `cd` takes for the directory the session was in BEFORE the last one.
const PREVIOUS_DIRECTORY_OPERAND: &str = "-";

/// The command of [`VOLUME_COMMANDS`] that writes what it READS into the files it names: a
/// program's own way of putting a document somewhere, beside a redirection.
const TEE_COMMAND: &str = "tee";

/// The one command of [`VOLUME_COMMANDS`] that only READS the tree, and so the one an
/// environment's own program for that name takes precedence over
/// ([`ShellSession::run_built_in`]): a caller whose storage has its own catalog of names
/// answers `ls` out of that catalog.
const LIST_COMMAND: &str = "ls";

/// How many numbers ONE invocation of [`SEQUENCE_COMMAND`] writes.
///
/// The EMULATOR's number: the operands are a caller's text, so what one of them can make this
/// process build is bounded here rather than by the numbers it was given. Far above the few a
/// retry loop iterates over and far below what would cost this process anything noticeable.
const MAX_SEQUENCE_NUMBERS: usize = 4_096;

/// The commands that only change HOW the command after them is run, and are therefore not
/// commands of their own: `nohup` — "invoke a utility immune to hangups", whose operands are
/// "the utility to be invoked" and its arguments — and `setsid`, which runs one in a session
/// of its own. This session has no signal to block and no session to lead, so the command
/// such a wrapper names IS the command performed, with the wrapper's remaining words as its
/// own ([`ShellSession::run_simple_command`]). A wrapper naming no utility runs nothing.
const COMMAND_WRAPPERS: &[&str] = &["nohup", "setsid"];

/// The command that runs another one AS ANOTHER USER: `sudo` — "execute a command as another
/// user", whose operands after its own options are "the command to run and any arguments".
///
/// It is a wrapper of the same kind ([`COMMAND_WRAPPERS`]) with options of its own, which is
/// why it is read apart from them: the words it consumes are its own and not the command's.
/// The session has one identity and nothing to escalate — an agent delivering a script to a
/// machine runs it as the superuser already — so the command it names IS the command
/// performed, with the words after it as that command's own
/// ([`ShellSession::run_simple_command`]). A `sudo` naming no utility at all runs nothing,
/// which is its own usage failure.
const PRIVILEGE_WRAPPER: &str = "sudo";

/// The options of [`PRIVILEGE_WRAPPER`] whose VALUE is the next word, so both words are the
/// wrapper's own rather than the command's (`sudo(8)`: the user, group, prompt, host, role,
/// type, chroot, directory and timeout a command may be run under, and the user a password is
/// read for). The `--option=value` spelling of each is one word and needs no entry.
const PRIVILEGE_WRAPPER_VALUE_OPTIONS: &[&str] = &[
    "-u", "--user", "-g", "--group", "-p", "--prompt", "-h", "--host", "-r", "--role", "-t",
    "--type", "-R", "--chroot", "-D", "--chdir", "-T", "--command-timeout", "-C", "--close-from",
    "-U", "--other-user",
];

/// The options of [`PRIVILEGE_WRAPPER`] that name the USER the command is to run as, and the
/// one identity a session has: an agent's script runs as the superuser, which is the identity
/// the command the wrapper names is performed with. An invocation naming ANOTHER user asks for
/// an identity this session does not have, so it is one the wrapper performs nothing of rather
/// than one performed as the identity it has.
const PRIVILEGE_WRAPPER_USER_OPTIONS: &[&str] = &["-u", "--user", "-U", "--other-user"];

/// See [`PRIVILEGE_WRAPPER_USER_OPTIONS`].
const SUPERUSER: &str = "root";

/// The command that runs another one FOR A WHILE: `timeout` — "start COMMAND, and kill it if
/// still running after DURATION", whose operands are that duration, "the command to run and
/// any arguments".
///
/// It is a wrapper of the same kind as [`COMMAND_WRAPPERS`] with a duration of its own, so the
/// command it names IS the command performed ([`ShellSession::run_simple_command`]) — nothing
/// this session performs waits on anything outside it, and the only time a command here takes
/// is the time it STATES ([`ShellSession::run_sleep`]). What the duration then decides is
/// whether that command ran out of it: one that did is reported as this wrapper ended it
/// ([`TIMED_OUT_STATUS`]) rather than as the command's own answer. A `timeout` naming no
/// duration, or no utility, runs nothing.
///
/// So the duration is measured against the time the commands under it STATED, which is the
/// only time there is to spend here: a command that states none runs to its own end whatever
/// the duration says, and is held instead by the session's command budget
/// ([`Bounds::commands`]) — as is a command that states more than one session may
/// ([`Bounds::sleep_seconds`]).
const DURATION_WRAPPER: &str = "timeout";

/// The options of [`DURATION_WRAPPER`] whose VALUE is the next word, so both words are the
/// wrapper's own rather than the command's (`timeout(1)`: the signal it sends and the further
/// duration after which it sends `KILL`). The `--option=value` spelling of each is one word and
/// needs no entry.
const DURATION_WRAPPER_VALUE_OPTIONS: &[&str] = &["-s", "--signal", "-k", "--kill-after"];

/// The option of [`DURATION_WRAPPER`] that keeps the COMMAND's own answer for a command the
/// duration ran out on: "--preserve-status: exit with the same status as COMMAND, even when the
/// command times out".
const PRESERVE_STATUS_OPTION: &str = "--preserve-status";

/// What [`DURATION_WRAPPER`] answers for a command it ended: "if the command times out, and
/// --preserve-status is not specified, then exit with status 124" (`timeout(1)`).
const TIMED_OUT_STATUS: i32 = 124;

/// The SECONDS a duration word states — `None` for a word that states no duration at all, or
/// one no finite number of seconds can hold.
///
/// One reading for both utilities that take one, which spell it the same way: `sleep`'s time
/// operand and the DURATION of [`DURATION_WRAPPER`] are "a floating point number with an
/// optional suffix: `s` for seconds (the default), `m` for minutes, `h` for hours or `d` for
/// days". What is checked is the SCALED value, which is the one answered: a word near the top
/// of the range states no duration once its suffix is applied to it.
fn stated_seconds(operand: &str) -> Option<f64> {
    let (value, unit) = match operand.chars().next_back() {
        Some(suffix @ ('s' | 'm' | 'h' | 'd')) => (
            &operand[..operand.len().saturating_sub(suffix.len_utf8())],
            suffix,
        ),
        _ => (operand, 's'),
    };
    let stated = value.parse::<f64>().ok()?;
    let factor = match unit {
        'm' => 60.0,
        'h' => 3_600.0,
        'd' => 86_400.0,
        _ => 1.0,
    };
    let seconds = stated * factor;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
}

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes". For every other command
/// an assignment before it is that command's environment alone.
const PERSISTING_COMMANDS: &[&str] = &[":", ".", "exit", "export", "unset", SET_COMMAND];

/// One OPTION of the shell a program's `set` command turns on or off, out of the options
/// whose effect this session performs.
///
/// Each is an option POSIX gives `set`, and each changes how the commands AFTER it are
/// performed rather than doing anything itself — which is why a session holds them
/// ([`ShellOptions`]) instead of the command answering out of them. An option whose effect
/// this session does not perform is not here, and `set` fails closed for it like every other
/// form no session performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellOption {
    /// `-e`, `-o errexit`: "when this option is on, when any command fails … the shell
    /// immediately shall exit".
    ErrExit,
    /// `-u`, `-o nounset`: "the shell shall write a message to standard error when it tries
    /// to expand a variable that is not set and immediately exit".
    NoUnset,
    /// `-o pipefail`: the status of a pipeline is the last command in it that failed, and
    /// zero for one in which none did.
    PipeFail,
}

/// The letter `set` names an option by, for the options that have one: `-e` and `-u`.
/// `pipefail` is named only by `-o`, which is how the shell spells it.
fn option_lettered(letter: char) -> Option<ShellOption> {
    match letter {
        'e' => Some(ShellOption::ErrExit),
        'u' => Some(ShellOption::NoUnset),
        _ => None,
    }
}

/// The option `set -o NAME` names, for the options this session performs.
fn option_named(name: &str) -> Option<ShellOption> {
    match name {
        "errexit" => Some(ShellOption::ErrExit),
        "nounset" => Some(ShellOption::NoUnset),
        "pipefail" => Some(ShellOption::PipeFail),
        _ => None,
    }
}

/// The letter that names an option by its long NAME instead ([`option_named`]): the word
/// after the one carrying it, wherever in a group of letters it stands — `set -o pipefail`
/// and `set -uo pipefail` are the same option.
const LONG_OPTION_LETTER: char = 'o';

/// Which of those options are ON in a session right now — all of them off for a session
/// whose program has not turned one on, which is how a shell starts.
#[derive(Debug, Clone, Copy, Default)]
struct ShellOptions {
    /// See [`ShellOption::ErrExit`].
    errexit: bool,
    /// See [`ShellOption::NoUnset`].
    nounset: bool,
    /// See [`ShellOption::PipeFail`].
    pipefail: bool,
}

/// What the shell writes for the expansion of a variable it holds none of while
/// [`ShellOption::NoUnset`] is on, beside ending the program on a non-zero status:
/// `<name>: parameter not set`, under the name the shell writes its diagnostics with
/// ([`SHELL_NAME`]).
const PARAMETER_NOT_SET: &str = "parameter not set";

/// What a shell writes about a command NAME its search resolved nothing for, and the name it
/// writes its own diagnostics under ([`ShellSession::command_not_found`]): a shell prefixes
/// one with the name it was invoked as, and this session is the `sh` a program's command
/// lines are performed by.
const COMMAND_NOT_FOUND: &str = "command not found";
/// See [`COMMAND_NOT_FOUND`].
const SHELL_NAME: &str = "sh";

/// The variable a shell searches a command NAME in the directories of: "PATH … shall be used
/// … to effect command interpretation". Searched over the TREE a session works over and over
/// nothing else ([`ShellSession::performed_tree_program`]).
const SEARCH_PATH_VARIABLE: &str = "PATH";

/// The names a NESTED shell of this session is invoked under, which is also the whole of what
/// a resolved program's interpreter line may name: a program of this environment is performed
/// by this session, so a file asking for another interpreter asks for a program the
/// environment has to provide.
///
/// Shared with the callers that resolve a program before handing a line to a session, for the
/// reason [`command_base_name`] is.
pub const NESTED_SHELL_NAMES: &[&str] = &["sh", "bash"];

/// The last component of `path`, which is the name a program is known by.
///
/// Shared with the callers that resolve a program themselves before handing a line to a
/// session (`…lambda…::state::execution::guest_child_process`), so one reading of a program's
/// name serves both.
#[must_use]
pub fn command_base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The interpreter a script's own first line names (`#!/bin/sh`, `#!/usr/bin/env bash`), and
/// `None` for a file that names none — which is a file a shell runs itself.
///
/// The program `env` RUNS the one named after it, so the name read through it is that one.
/// Shared for the reason [`command_base_name`] is: a caller that decides whether it can run a
/// program at all reads its interpreter line the same way the session does.
#[must_use]
pub fn script_interpreter(text: &str) -> Option<String> {
    let line = text.lines().next()?.strip_prefix("#!")?.trim();
    let mut words = line.split_whitespace();
    let first = words.next()?;
    if command_base_name(first) == "env" {
        // `env NAME=VALUE … PROGRAM`: the program is the first word that is neither an
        // assignment nor an option of `env` itself.
        return words
            .find(|word| !word.contains('=') && !word.starts_with('-'))
            .map(str::to_string);
    }
    Some(first.to_string())
}

/// One stream of a finished session as the BYTES it wrote: the session keeps what a command
/// wrote as the LINES of it ([`ShellSession::output`], [`ShellSession::errors`]), so the stream
/// is those lines, each ended the way a line is ended. A session that wrote nothing wrote no
/// bytes at all.
///
/// The ONE reading of that for every caller whose API reports a stream's bytes rather than its
/// lines, so two of them cannot differ about it.
#[must_use]
pub fn stream_text(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut written = lines.join("\n");
    written.push('\n');
    written
}

/// What a shell adds to a signal number to report the status of a command a signal ended:
/// "the exit status of a command that terminated because it received a signal shall be
/// reported as greater than 128" — 128 plus the signal's number.
pub const EXIT_SIGNAL_BASE: i32 = 128;

/// The number of the signal a runtime KILLS a process with, which no handler can intercept:
/// `SIGKILL` is signal 9.
pub const SIGKILL_NUMBER: i32 = 9;

/// The status a program ends with when the environment it runs in KILLED it for using more
/// memory than that environment gives it ([`SessionConfig::memory`]): the kill is a
/// `SIGKILL`, so the status is [`EXIT_SIGNAL_BASE`] plus [`SIGKILL_NUMBER`].
pub const KILLED_BY_MEMORY_STATUS: i32 = EXIT_SIGNAL_BASE + SIGKILL_NUMBER;

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
    /// How many bytes ONE substitution may capture before it has no value at all, which is
    /// also what ONE stage of a pipeline may pass to the stage after it.
    pub substitution_bytes: usize,
    /// How many files one session's own tree holds at once.
    pub files: usize,
    /// How many bytes ONE of those files may hold, which is also how much of a redirected
    /// command's output is collected for it.
    pub file_bytes: usize,
    /// How many bytes the whole tree may hold at once, so neither a few large files nor very
    /// many small ones grow it without a stated ceiling.
    pub tree_bytes: usize,
    /// How many seconds of `sleep` one session may state in total ([`ShellSession::slept`]),
    /// so a caller that honours that time is never asked for an unbounded wait.
    pub sleep_seconds: u64,
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
            sleep_seconds: 300,
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

/// What a session is constructed with: its statuses, its bounds, whether it keeps what
/// the program wrote, and how much MEMORY the environment it runs in gives it.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionConfig {
    /// See [`Statuses`].
    pub statuses: Statuses,
    /// See [`Bounds`].
    pub bounds: Bounds,
    /// See [`OutputLimits`]; `None` keeps no output.
    pub output: Option<OutputLimits>,
    /// How many BYTES of memory the environment this program runs in gives it.
    ///
    /// The CALLER's own fact rather than a bound of this module's — the memory a container's
    /// declaration states for it — and a HARD one: a program whose values would take what it
    /// holds past this much is KILLED where it stands
    /// ([`ShellSession::killed_for_memory`]), because that is what a runtime holding such a
    /// limit does to a process that goes past it. `None` for a caller whose environment
    /// states no such limit, which is held to [`Bounds`] alone.
    pub memory: Option<usize>,
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
/// protects. The count can only be higher than the depth a parse descends to, never lower,
/// so it refuses a program and never admits one; a caller that hands this session a whole
/// SCRIPT therefore states the bound for a script's worth of groups, and sizes the stack its
/// run is performed on for that bound (`…ec2…::state::guest_commands`).
///
/// It is deliberately a COUNT and not a measured depth: a depth would have to be measured
/// through the program's quoting and its here-documents to stay an upper bound on what the
/// parse descends to, and a measure that is not an upper bound is no bound at all — a closing
/// character of a quoted word would leave a group it never entered, and a program could then
/// hold the measure near zero while nesting without end.
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
    /// What the program wrote, one entry per performed command that wrote a line, kept only
    /// when the configuration asks for it.
    output: Vec<String>,
    /// What the program's commands reported they could not do, one entry per line, kept
    /// under the same limits as [`Self::output`] and for the same callers: a service whose
    /// API reports the two streams SEPARATELY ([`Self::errors`]) reads this one, and nothing
    /// is kept at all for a session whose configuration asks for no output.
    errors: Vec<String>,
    /// The programs the environment this session runs in provides, asked for a command
    /// neither catalog names; `None` for a session whose environment provides none.
    programs: Option<Box<dyn EnvironmentPrograms>>,
    /// The tree this session's commands read and write when it runs in a MACHINE's one
    /// ([`Self::in_volume`]) — `None` for a session whose tree is its own
    /// ([`Self::files`]), which performs neither the file utilities nor the forms that reach
    /// a machine's other descriptors.
    volume: Option<ShellVolume>,
    /// The command substitutions being captured, innermost last: while one is open, what a
    /// command writes to standard output is ITS value rather than the session's output.
    captures: Vec<Capture>,
    /// For each command substitution being performed, innermost last, what the session's
    /// variable store has to be put back to when it ends: the value it held for each name the
    /// substituted commands touched, before they did. A substitution is a SUBSHELL, and this
    /// is what rolls one back — which costs what it changed rather than a copy of the store.
    substitution_undo: Vec<BTreeMap<String, Option<String>>>,
    /// The status of the last command substitution performed for the simple command being
    /// run, which is the status of a command that has no command name at all.
    last_substitution_status: Option<i32>,
    /// How many more substitutions and `sh -c` programs this session performs.
    substitutions_left: usize,
    /// How many `sh -c` programs are running inside one another right now, counted beside
    /// the captures above as the DEPTH this module's own recursion is bounded by.
    nested_programs: usize,
    /// The files this session's own tree holds, keyed by the path a redirection named — the
    /// tree of a session that was given no machine's.
    files: BTreeMap<String, String>,
    /// Set for a session whose own tree is the WORKING tree of the environment its program
    /// runs in ([`Self::with_file_utilities`]), which is what makes the file utilities
    /// ([`VOLUME_COMMANDS`]) commands this session performs over that tree.
    working_tree: bool,
    /// How many bytes that tree comes to, kept as it is written.
    tree_bytes: usize,
    /// Set when a write to the file a redirection named did not happen, which is what makes
    /// the command carrying it answer as one that could not be run.
    redirect_failed: bool,
    /// What the stage BEFORE the command being performed wrote, which is that command's
    /// standard input — `None` for a command that is not reading one stage of a pipeline.
    /// Held by [`ShellSession::run_pipeline`] for the one command it belongs to, and taken by
    /// the first command that reads it, the way a stream is consumed by whoever reads it.
    piped_input: Option<String>,
    /// How long this session's `sleep` commands have stated the program took.
    slept: ::std::time::Duration,
    /// The directory the paths a program names WITHOUT one are resolved against, which `cd`
    /// is what changes ([`Self::run_change_directory`]). The root of the tree for a session
    /// nothing has changed it in, and never read for a session whose tree is its own: such a
    /// tree keys a file by the path a program named.
    working_directory: String,
    /// The shell OPTIONS this session performs the program under, which its own `set`
    /// commands turn on and off ([`Self::run_set`]).
    options: ShellOptions,
    /// How many enclosing commands are TESTING the status of what they run right now — the
    /// condition of an `if`, a `while` or an `until` — which is where [`ShellOption::ErrExit`]
    /// does not end the program: "the shell shall not exit if the command that fails is part
    /// of the … condition of a while, until or if statement".
    tested_status: usize,
    /// Whether the pipeline whose status the and-or list being performed answers with was
    /// written with `!`, which is the other place that option does not end the program: the
    /// status of such a pipeline is one the program itself is reading.
    negated_pipeline: bool,
    /// Set for a caller that reports standard output and standard ERROR as members of its own,
    /// which keeps a diagnostic off the console this session keeps
    /// ([`Self::reporting_both_streams`]).
    separate_errors: bool,
    /// How many COMPOUND commands are being performed inside one another right now, which is
    /// what bounds this module's own recursion over the grammar's control flow
    /// ([`Self::run_compound`]).
    compound_depth: usize,
    /// How many LOOPS are being performed inside one another right now, which is what decides
    /// whether `break` and `continue` have a loop to leave at all.
    loop_depth: usize,
    /// The loop the program asked to leave, and how many enclosing loops it is leaving —
    /// `None` while no `break` or `continue` is in flight. Set by those built-ins and taken by
    /// the loop that honours it ([`Self::run_loop_control`]).
    loop_control: Option<LoopControl>,
    /// The status of the most recent PIPELINE this session performed, which is what `$?`
    /// expands to ([`Self::expand_parameter`]). Zero before the first one, which is the status
    /// a shell reads before it has run anything.
    last_status: i32,
    /// The POSITIONAL PARAMETERS the program was given ([`Self::with_arguments`]): the name it
    /// was invoked under (`$0`) and the operands after it (`$1`, `$2`, …), which is what a
    /// caller that runs a SCRIPT with arguments hands it. Empty for a program invoked with
    /// none, where every one of them expands to nothing.
    positional: Vec<String>,
    /// How many ARITHMETIC expansions are being resolved inside one another right now, which is
    /// what bounds that path's own recursion ([`Self::expand_arithmetic`]).
    ///
    /// It is counted here rather than taken from the word's remaining depth because resolving
    /// one starts a new WORD — the expression's own text, whose expansions are resolved before
    /// the arithmetic is parsed — and a new word is given the whole word budget again. Without
    /// this, an expression nesting `$((…))` inside itself would restart the only counter that
    /// bounds the walk, so the recursion a single program text could drive would be bounded by
    /// the length of that text alone.
    arithmetic_depth: usize,
    /// How many commands of this program named something NOTHING resolved — see
    /// [`Self::unresolved_commands`].
    unresolved_commands: usize,
    /// How many bytes of memory the environment this session runs in gives the program
    /// ([`SessionConfig::memory`]), once the environment the session STARTS with is in place:
    /// those variables are the environment the program is given rather than something it
    /// allocated, so the limit is armed after they are held and applies to what the program
    /// itself would hold on top of them.
    memory_limit: Option<usize>,
    /// Set when the environment KILLED the program for going past that limit, after which no
    /// command of it is performed — see [`Self::killed_for_memory`].
    memory_killed: bool,
}

/// What a `break` or a `continue` asked for: which of the two it was, and how many enclosing
/// loops it leaves.
#[derive(Debug, Clone, Copy)]
struct LoopControl {
    /// Whether the loops being left go on to their next iteration (`continue`) or end
    /// (`break`).
    goes_on: bool,
    /// How many enclosing loops this leaves — "the n-th enclosing loop", at least one.
    levels: usize,
}

/// The FILES of the tree this session works over, handed to the ENVIRONMENT's programs
/// while one of them runs ([`ShellSession::run_environment_program`]).
///
/// It reaches the one tree the session's own commands read and write ([`ShellSession::read_path`],
/// [`ShellSession::write_path`]) and nothing else of the session.
struct SessionFiles<'a> {
    session: &'a mut ShellSession,
}

impl ProgramFiles for SessionFiles<'_> {
    fn read(&self, path: &str) -> Option<String> {
        self.session.read_path(path)
    }

    fn write(&mut self, path: &str, contents: &str) -> bool {
        self.session.write_path(path, contents, false)
    }

    fn remove(&mut self, path: &str) -> bool {
        self.session.tree_remove_file(path)
    }

    fn entries_in(&self, path: &str) -> Vec<String> {
        self.session.tree_entry_names(path)
    }

    fn make_directories(&mut self, path: &str) -> bool {
        self.session.tree_make_directories(path)
    }

    fn mount(&mut self, path: &str, tree: ShellMountedTree) -> bool {
        self.session.tree_mount(path, tree)
    }

    fn unmount(&mut self, path: &str) -> bool {
        self.session.tree_unmount(path)
    }

    fn mounted_at(&self, path: &str) -> Option<ShellMountFacts> {
        self.session.tree_mounted_at(path)
    }

    fn file_system_at(&self, path: &str) -> Option<ShellMountFacts> {
        self.session.tree_file_system_at(path)
    }

    fn stored_byte_count(&self) -> Option<usize> {
        Some(self.session.tree_stored_bytes())
    }
}

/// The same files, as the SEAM the shared file utilities reach them through
/// ([`posix_shell_files::ShellFiles`]) — so what `cat`, `ls`, `mkdir`, `touch` and `rm` do and
/// SAY is written once for every session in this workspace, and what differs is only where
/// the files are.
///
/// A refusal is reported as the error the tree's answer stands for: this tree answers a write
/// or a removal with whether it happened, so a path it will not take one for is the
/// `No such file or directory` a session has always reported for it — except a path a program
/// may only READ, which is the read-only filesystem it is
/// ([`super::posix_shell_volume::ShellVolume::is_read_only`]). It keeps no mode, owner
/// or modification time, so it reports no [`posix_shell_files::FileEntry`] — which is what
/// makes a long listing a form these sessions do not perform.
impl ShellFiles for SessionFiles<'_> {
    fn read(&self, path: &str) -> Result<String, FileRefusal> {
        self.session.read_path(path).ok_or(FileRefusal::NotFound)
    }

    fn write(&mut self, path: &str, text: &str, append: bool) -> Result<(), FileRefusal> {
        if self.session.write_path(path, text, append) {
            Ok(())
        } else if self.session.tree_is_read_only(path) {
            Err(FileRefusal::ReadOnly)
        } else {
            Err(FileRefusal::NotFound)
        }
    }

    fn make_directories(&mut self, path: &str) -> Result<(), FileRefusal> {
        if self.session.tree_make_directories(path) {
            Ok(())
        } else if self.session.tree_is_read_only(path) {
            Err(FileRefusal::ReadOnly)
        } else {
            Err(FileRefusal::NotFound)
        }
    }

    fn remove_file(&mut self, path: &str) -> Result<(), FileRefusal> {
        if self.session.tree_holds_directory(path) {
            return Err(FileRefusal::IsADirectory);
        }
        if self.session.tree_is_read_only(path) {
            return Err(FileRefusal::ReadOnly);
        }
        if self.session.tree_remove_file(path) {
            Ok(())
        } else {
            Err(FileRefusal::NotFound)
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.session.tree_holds(path)
    }

    fn is_file(&self, path: &str) -> bool {
        self.session.tree_holds_file(path)
    }

    fn is_directory(&self, path: &str) -> bool {
        self.session.tree_holds_directory(path)
    }

    fn writable(&self, path: &str) -> bool {
        // What `-w` asks is whether a write would be performed, and the write paths above
        // refuse a read-only entry: so a path of a read-only filesystem is held and not
        // writable, and every other path this tree accepts is both.
        self.session.tree_holds(path) && !self.session.tree_is_read_only(path)
    }

    fn entry_names(&self, path: &str) -> Vec<String> {
        self.session.tree_entry_names(path)
    }

    fn keeps_directories(&self) -> bool {
        // A session's OWN tree keys a file by its whole path and holds no directory entry, so
        // every directory above a file exists already; a MACHINE's tree holds them.
        self.session.volume.is_some()
    }
}

impl ShellSession {
    /// A session holding `variables`, with the whole command budget `config` allows left,
    /// whose environment provides no program beyond the catalog.
    ///
    /// The store's ceilings ([`Bounds::session_variables`], [`Bounds::session_bytes`]) hold
    /// for the variables a session STARTS with exactly as they do for the ones its program
    /// sets, so a caller handing it more than a session may hold hands it what fits: the rest
    /// are not in the environment the program is given.
    #[must_use]
    pub fn new(variables: BTreeMap<String, String>, config: SessionConfig) -> Self {
        let mut session = Self {
            variables: BTreeMap::new(),
            held_bytes: 0,
            expansion_left: config.bounds.line_expansion_bytes,
            commands_left: config.bounds.commands,
            exit_status: None,
            output: Vec::new(),
            errors: Vec::new(),
            programs: None,
            volume: None,
            captures: Vec::new(),
            substitution_undo: Vec::new(),
            last_substitution_status: None,
            substitutions_left: config.bounds.substitutions,
            nested_programs: 0,
            files: BTreeMap::new(),
            working_tree: false,
            tree_bytes: 0,
            redirect_failed: false,
            piped_input: None,
            slept: ::std::time::Duration::ZERO,
            working_directory: super::posix_shell_volume::ROOT.to_string(),
            options: ShellOptions::default(),
            tested_status: 0,
            negated_pipeline: false,
            separate_errors: false,
            compound_depth: 0,
            loop_depth: 0,
            loop_control: None,
            last_status: 0,
            positional: Vec::new(),
            arithmetic_depth: 0,
            unresolved_commands: 0,
            memory_limit: None,
            memory_killed: false,
            config,
        };
        for (name, value) in variables {
            if !session.set_variable(name, value) {
                break;
            }
        }
        // The memory the environment gives the program is a limit on what the PROGRAM holds,
        // so it is armed once the environment the session starts with is in place (see
        // [`Self::memory_limit`]). What those variables hold is accounted for all the same:
        // the program's own values are weighed on top of them.
        session.memory_limit = config.memory;
        session
    }

    /// [`Self::new`] for a session whose environment provides `programs` as well: a command
    /// the catalog does not name is asked of them before it is reported as one this session
    /// could not run.
    #[must_use]
    pub fn with_programs(
        variables: BTreeMap<String, String>,
        config: SessionConfig,
        programs: Box<dyn EnvironmentPrograms>,
    ) -> Self {
        Self {
            programs: Some(programs),
            ..Self::new(variables, config)
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

    /// This session with its OWN tree declared the WORKING tree of the environment the
    /// program runs in, so the file utilities ([`VOLUME_COMMANDS`]) are commands it performs
    /// over that tree.
    ///
    /// What a caller declares with this is that its environment HAS storage of its own — a
    /// build container's source directory, say — rather than only the files a redirection
    /// leaves behind. That tree keys a file by the whole path the program named and holds no
    /// directory entry of its own, so a directory of it exists exactly as long as a file
    /// under it does: `mkdir` has nothing to make and answers success (it fails only where a
    /// FILE already holds that path), `test -d` answers for a path some file lies under, and
    /// a write never needs the directory above it to have been made first. The forms that
    /// reach a MACHINE's other descriptors are not these: they stay a session given a
    /// machine's tree ([`Self::in_volume`]).
    #[must_use]
    pub fn with_file_utilities(mut self) -> Self {
        self.working_tree = true;
        self
    }

    /// This session with `programs` as the programs the ENVIRONMENT it runs in provides, for
    /// a caller that constructed it some other way than [`Self::with_programs`] — a session
    /// running in a MACHINE's tree ([`Self::in_volume`]) whose machine also has programs
    /// installed in it.
    ///
    /// The same seam [`Self::with_programs`] fills: a command neither catalog names is asked
    /// of `programs` before it is reported as one this session could not run.
    #[must_use]
    pub fn with_environment_programs(mut self, programs: Box<dyn EnvironmentPrograms>) -> Self {
        self.programs = Some(programs);
        self
    }

    /// The POSITIONAL PARAMETERS the program is given: `arguments[0]` is the name it was
    /// invoked under, which `$0` expands to, and the operands after it are `$1`, `$2` and so
    /// on — what a caller that runs a SCRIPT with operands hands it.
    ///
    /// A session given none holds none, and then every positional parameter expands to
    /// nothing, which is what an unset parameter expands to. Field splitting is a form no
    /// session performs, so `$@` and `$*` are both the operands joined by one space
    /// ([`Self::expand_parameter`]).
    #[must_use]
    pub fn with_arguments(mut self, arguments: Vec<String>) -> Self {
        self.positional = arguments;
        self
    }

    /// This session with `text` as the STANDARD INPUT the program was given: the bytes the
    /// caller handed it, which the first command that reads standard input reads.
    ///
    /// It is a stream, so it is consumed by whoever reads it: the first command of a pipeline
    /// reads it ([`Self::run_pipeline`]), a command that reads none leaves it for whatever
    /// reads next, and a command whose own input is redirected reads that file instead.
    #[must_use]
    pub fn with_standard_input(mut self, text: String) -> Self {
        self.piped_input = Some(text);
        self
    }

    /// This session with `path` as the DIRECTORY its program starts in, which is what the
    /// paths it names without one are resolved against ([`Self::at`]) — the working directory
    /// the caller started the program in.
    ///
    /// Only a session that works over a tree resolves a path against it; for one whose tree is
    /// its own a path is a key, so there is nothing for this to change.
    #[must_use]
    pub fn in_directory(mut self, path: &str) -> Self {
        if let Some(resolved) = super::posix_shell_volume::resolved_path(path) {
            self.working_directory = resolved;
        }
        self
    }

    /// This session for a caller whose API reports the two streams SEPARATELY — one member
    /// for what the program wrote and one for what it reported it could not do.
    ///
    /// A session given a MACHINE's tree keeps a diagnostic on the console as well as in
    /// [`Self::errors`], because a console is where a machine's own diagnostics are read and
    /// the one stream a console carries is both of them. A caller that reports the two as
    /// members of its own reads them apart ([`Self::output`], [`Self::errors`]), so for it a
    /// diagnostic is NOT part of what the program wrote: this keeps it off the console, the
    /// way the two descriptors of a program are separate streams.
    #[must_use]
    pub fn reporting_both_streams(mut self) -> Self {
        self.separate_errors = true;
        self
    }

    /// Put `contents` at `path` in the tree this session reads and writes, the way the
    /// environment seeds a file its program is to find there — a source archive the build
    /// unpacked, a cached path it restored.
    ///
    /// `false` is a seed that did not happen, under [`Self::write_path`]'s rule: a path the
    /// tree does not accept, or a write past one of the tree's bounds.
    pub fn put_file(&mut self, path: &str, contents: &str) -> bool {
        self.write_path(path, contents, false)
    }

    /// The files the tree this session works over holds, as `(path, contents)` in path
    /// order — what the caller takes back out of a finished run: the files a build's
    /// commands produced, which are what its artifacts and its cache are selected from.
    #[must_use]
    pub fn tree_files(&self) -> Vec<(&str, &str)> {
        match &self.volume {
            Some(volume) => volume.stored_files().collect(),
            None => self
                .files
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.as_str()))
                .collect(),
        }
    }

    /// How many bytes the files above come to, as one of them accounts for its path and its
    /// contents — what a program of the tree's owner reports as the storage IN USE
    /// ([`ProgramFiles::stored_byte_count`]).
    #[must_use]
    pub fn tree_stored_bytes(&self) -> usize {
        self.tree_files()
            .iter()
            .map(|(path, contents)| path.len().saturating_add(contents.len()))
            .sum()
    }

    /// Forget that a program ended this session with `exit`, so the next line runs.    ///
    /// For a caller that hands this session SEVERAL programs over one tree — a phase's
    /// command list and then the block that runs after it — where an `exit` ends the program
    /// that performed it and not the ones the caller runs next.
    pub fn clear_exit(&mut self) {
        self.exit_status = None;
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

    /// Whether the environment this session runs in KILLED the program for using more memory
    /// than that environment gives it ([`SessionConfig::memory`]).
    ///
    /// For a caller whose API reports WHY a program ended: a killed program did not end on its
    /// own, so what it was doing when the kill landed was never finished — only what it had
    /// already written is what it produced. The status such a program ends with is
    /// [`KILLED_BY_MEMORY_STATUS`].
    #[must_use]
    pub fn killed_for_memory(&self) -> bool {
        self.memory_killed
    }

    /// KILL the program: it asked for more memory than the environment it runs in gives it, so
    /// the environment ends it where it stands — nothing of it is performed from here on, and
    /// the status it ends with is the one a killed process ends with
    /// ([`KILLED_BY_MEMORY_STATUS`]).
    ///
    /// A kill is not a command's failure and no handler of the program sees it: the latch is
    /// the one `exit` sets, which is what makes every later line of the program unperformed.
    fn kill_for_memory(&mut self) {
        self.memory_killed = true;
        self.exit_status = Some(KILLED_BY_MEMORY_STATUS);
    }

    /// How many commands of this program named something NOTHING resolved: no utility of this
    /// session's own catalogs, no program of the environment it runs in and no file of the
    /// tree it works over, so the command did not run at all
    /// ([`Self::command_not_found`] is where each is counted).
    ///
    /// For a caller that has to know whether what its program DID is the whole of what it
    /// asked for: a count of nothing means every command of it was performed here, and any
    /// count above that means the program asked for something this environment provides
    /// nothing of — whose effect, whatever the program wanted of it, is held nowhere. A
    /// caller reporting state the program may have changed reads this to tell what it holds
    /// from what it merely did not see.
    #[must_use]
    pub fn unresolved_commands(&self) -> usize {
        self.unresolved_commands
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

    /// The lines this session's commands wrote to standard ERROR, in the order they wrote
    /// them — what a command reported it could not do.
    ///
    /// For a caller whose API reports the two streams as members of its own: a session given
    /// a MACHINE's tree keeps a diagnostic on its console ([`Self::output`]) as well, because
    /// that is where a guest's diagnostics are read, and for every other session this is the
    /// only place one is kept. Empty for a session whose configuration keeps no output, and
    /// for one whose commands reported nothing.
    #[must_use]
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// How long this session's `sleep` commands stated the program took
    /// ([`Self::run_sleep`]).
    ///
    /// No thread waited for it: a caller whose API reports how long a command took, or
    /// converges a submitted task on it, is the one that honours this.
    #[must_use]
    pub fn slept(&self) -> ::std::time::Duration {
        self.slept
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
    ///
    /// The status of the WHOLE list is what [`ShellOption::ErrExit`] reads: a pipeline the
    /// operator after it tests is one "part of an AND-OR list other than the last", which that
    /// option passes over, so the check belongs here rather than at each pipeline.
    fn run_and_or_list(&mut self, list: &::brush_parser::ast::AndOrList) -> i32 {
        let mut status = self.run_pipeline(&list.first);
        // Whether the status is the one the LAST pipeline of the list answered. A pipeline the
        // operator after it short-circuited is a status the list itself tested — "part of an
        // AND-OR list other than the last" — which [`ShellOption::ErrExit`] passes over.
        let mut from_last = list.additional.is_empty();
        let count = list.additional.len();
        for (index, next) in list.additional.iter().enumerate() {
            if self.exit_status.is_some() {
                break;
            }
            let (pipeline, runs) = match next {
                ::brush_parser::ast::AndOr::And(pipeline) => {
                    (pipeline, status == self.config.statuses.success)
                }
                ::brush_parser::ast::AndOr::Or(pipeline) => {
                    (pipeline, status != self.config.statuses.success)
                }
            };
            if runs {
                status = self.run_pipeline(pipeline);
            }
            from_last = runs && index.saturating_add(1) == count;
        }
        if from_last {
            self.exit_on_failure(status);
        }
        status
    }

    /// End the program on `status` when [`ShellOption::ErrExit`] is on and this list's status
    /// is not one the program itself is reading: "when any command fails … the shell
    /// immediately shall exit", other than inside the condition of an `if`, a `while` or an
    /// `until` ([`Self::tested_status`]) and other than a pipeline written with `!`
    /// ([`Self::negated_pipeline`]).
    ///
    /// A program that has already ended keeps the status it ended with.
    fn exit_on_failure(&mut self, status: i32) {
        if !self.options.errexit
            || self.tested_status > 0
            || self.negated_pipeline
            || self.exit_status.is_some()
            || status == self.config.statuses.success
        {
            return;
        }
        self.exit_status = Some(status);
    }

    /// Perform `list` as a status this program is READING rather than acting on — the
    /// condition of an `if`, a `while` or an `until` — so a command that fails inside it does
    /// not end the program under [`ShellOption::ErrExit`].
    fn run_tested_list(&mut self, list: &::brush_parser::ast::CompoundList) -> i32 {
        self.tested_status = self.tested_status.saturating_add(1);
        let status = self.run_compound_list(list);
        self.tested_status = self.tested_status.saturating_sub(1);
        status
    }

    /// Perform a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!`.
    ///
    /// Every command of the pipeline is performed, in order, because each answers its own
    /// status, and each is CONNECTED to the one after it: what a stage writes to standard
    /// output is the standard input of the next stage ([`Self::standard_input`]) rather than
    /// what the session keeps, so only the LAST stage's output is the pipeline's. A stage
    /// whose own standard output is redirected writes to that file and passes nothing on, the
    /// way the shell's redirection takes precedence over the pipe.
    ///
    /// A stage that wrote more than [`Bounds::substitution_bytes`] ends the pipeline: the bytes
    /// the pipe could not carry are bytes the stage after it would have read, so the pipeline
    /// answers the failure of the write that did not happen rather than running the rest of it
    /// over an empty stream — a stage past the bound must not be able to make a later stage
    /// answer for input nothing produced.
    ///
    /// Under [`ShellOption::PipeFail`] the status is "the value of the last (rightmost)
    /// command to exit with a non-zero status, or zero if all commands exit successfully"
    /// instead — read before a `!` negates it, the way the shell reads it.
    fn run_pipeline(&mut self, pipeline: &::brush_parser::ast::Pipeline) -> i32 {
        let status = self.performed_pipeline(pipeline);
        // "?: Expands to the decimal exit status of the most recent pipeline" — which is this
        // one, from here until the next pipeline runs.
        self.last_status = status;
        status
    }

    /// [`Self::run_pipeline`] itself, around which that status is recorded.
    fn performed_pipeline(&mut self, pipeline: &::brush_parser::ast::Pipeline) -> i32 {
        let mut status = self.config.statuses.success;
        // The rightmost stage that failed, which is the pipeline's status under
        // [`ShellOption::PipeFail`].
        let mut failed: Option<i32> = None;
        let last = pipeline.seq.len().saturating_sub(1);
        // What the stage before the one being performed wrote.
        let mut upstream: Option<String> = None;
        for (index, command) in pipeline.seq.iter().enumerate() {
            // Every stage but the last writes to the stage after it, which is what collects
            // its output here instead of the session.
            let connected = index < last;
            if connected {
                self.captures.push(Capture {
                    limit: self.config.bounds.substitution_bytes,
                    ..Capture::default()
                });
            }
            // The input belongs to this one command: whatever the command around this
            // pipeline was reading is put back after it. The FIRST stage is the one exception,
            // because its standard input IS the session's own — what the caller handed the
            // program ([`Self::with_standard_input`]), or what the command around this
            // pipeline was reading — so that stream is left exactly where it is: a stage that
            // reads it takes it, and one that reads none leaves it for whatever reads next.
            if index == 0 {
                status = self.run_command(command);
            } else {
                let held_input = ::std::mem::replace(&mut self.piped_input, upstream.take());
                status = self.run_command(command);
                self.piped_input = held_input;
            }
            if status != self.config.statuses.success {
                failed = Some(status);
            }
            if connected {
                let captured = self.captures.pop().unwrap_or_default();
                if captured.overflowed {
                    // Not a pipeline that ran, so not one a `!` negates either.
                    self.negated_pipeline = false;
                    return self.config.statuses.failure;
                }
                upstream = Some(captured.text);
            }
            if self.exit_status.is_some() {
                break;
            }
        }
        if self.options.pipefail {
            status = failed.unwrap_or(self.config.statuses.success);
        }
        if pipeline.bang {
            status = if status == self.config.statuses.success {
                self.config.statuses.failure
            } else {
                self.config.statuses.success
            };
        }
        // Which the and-or list around it reads, because the status of a pipeline the program
        // itself negated is not one [`ShellOption::ErrExit`] ends the program for.
        self.negated_pipeline = pipeline.bang;
        status
    }

    /// Perform one command of a pipeline.
    ///
    /// A SIMPLE command is the form every session performs. The grammar's COMPOUND
    /// commands — the control flow a program is written with — are performed over this
    /// same catalog by a session that works over a tree ([`Self::run_compound`]), because
    /// they need no primitive beyond the commands inside them, and so is the output
    /// REDIRECTION of a whole compound command ([`Self::run_redirected_compound`]), which
    /// is how a script writes one file from several commands. Every other form a shell
    /// grammar admits — a function definition, an extended test, a compound command
    /// redirecting another descriptor — is one this session does not perform, and a
    /// command that did not run answers as one that could not be run rather than as one
    /// that succeeded.
    ///
    /// EVERY command spends one of [`Bounds::commands`] here, whatever form it takes and
    /// whether or not this session performs it: that budget is latched, so it is what ends
    /// a loop with no end of its own, and a form that answered without running would
    /// otherwise let a loop iterate for nothing.
    fn run_command(&mut self, command: &::brush_parser::ast::Command) -> i32 {
        if self.commands_left == 0 {
            return self.config.statuses.could_not_run;
        }
        self.commands_left = self.commands_left.saturating_sub(1);
        match command {
            ::brush_parser::ast::Command::Simple(simple) => self.run_simple_command(simple),
            ::brush_parser::ast::Command::Compound(compound, redirections)
                if self.works_over_a_tree() =>
            {
                match redirections {
                    None => self.run_compound(compound),
                    Some(list) => self.run_redirected_compound(compound, &list.0),
                }
            }
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform a compound command whose STANDARD OUTPUT is redirected: `{ …; } > PATH`,
    /// and the `>> PATH` form that adds to what the file holds.
    ///
    /// "The redirection applies to the compound command as a whole", so what every command
    /// inside it wrote is ONE write of that file — which is how a script that builds a file
    /// out of several lines writes it, rather than appending to it once per line. The write
    /// happens when the compound command has finished, the way a redirected simple command's
    /// does ([`Self::run_simple_command`]), and a write the tree will not take leaves the
    /// tree as it was and answers as a command that could not be run.
    ///
    /// Only standard output is redirected here: a compound command redirecting another
    /// descriptor is a form this session does not perform, since the commands INSIDE it are
    /// what redirect their own input and diagnostics.
    fn run_redirected_compound(
        &mut self,
        compound: &::brush_parser::ast::CompoundCommand,
        redirects: &[::brush_parser::ast::IoRedirect],
    ) -> i32 {
        let mut redirections = Redirections::default();
        for redirect in redirects {
            if !self.apply_redirect(redirect, &mut redirections) {
                return self.config.statuses.could_not_run;
            }
        }
        let Some((path, append)) = redirections.out.take() else {
            return self.config.statuses.could_not_run;
        };
        if redirections != Redirections::default() {
            // Another descriptor was redirected beside standard output, which is not a
            // form performed here.
            return self.config.statuses.could_not_run;
        }
        // The refused-write flag is this command's own, exactly as it is for a redirected
        // simple command: one held for a command still running is restored after it.
        let outer_refused = ::std::mem::replace(&mut self.redirect_failed, false);
        self.captures.push(Capture {
            limit: self.config.bounds.file_bytes,
            ..Capture::default()
        });
        let status = self.run_compound(compound);
        let captured = self.captures.pop().unwrap_or_default();
        if captured.overflowed || !self.write_path(&path, &captured.text, append) {
            self.redirect_failed = true;
        }
        if ::std::mem::replace(&mut self.redirect_failed, outer_refused) {
            return self.config.statuses.could_not_run;
        }
        status
    }

    /// Whether this session works over a TREE — a machine's ([`Self::in_volume`]) or its
    /// own working one ([`Self::with_file_utilities`]) — which is what makes its caller's
    /// program a script of an environment with storage rather than one command line.
    fn works_over_a_tree(&self) -> bool {
        self.volume.is_some() || self.working_tree
    }

    /// Perform a COMPOUND command: the control flow the grammar reads, over the commands
    /// inside it.
    ///
    /// `if`/`elif`/`else` runs the branch whose condition answered success, `while` and
    /// `until` repeat their body while their condition answers success (or failure), and a
    /// brace group runs its list in this session. Every command inside — this compound
    /// command itself included ([`Self::run_command`]) — spends the session's own command
    /// budget, which is what bounds a loop: each iteration performs at least its condition,
    /// so once the budget is gone the loop answers as a command that could not run rather
    /// than iterating again.
    ///
    /// Every other compound form is one this session does not perform: a subshell and a
    /// `case` clause need an environment of their own or a pattern evaluation this session
    /// has none of, so they answer as a command that could not be run.
    ///
    /// How deeply they may NEST is bounded here, by the same bound a parse of the program is
    /// given ([`Bounds::nesting`]): performing one costs this module a chain of calls per
    /// level — the compound command, its list, its and-or lists, its pipelines and the
    /// commands inside them — so a program nesting one inside another without end would spend
    /// the stack of the thread performing it rather than a budget. A compound command past
    /// that depth is one this session does not perform, which is the same closed failure
    /// every other form it cannot perform answers with.
    ///
    /// It is the BELT under the parse bound rather than the first thing a program meets: that
    /// bound counts every grouping character and reserved word of the program
    /// ([`within_parse_bounds`]), and a compound command carries several of each per level, so
    /// a program that would nest this deep is refused before it is read at all. What this
    /// bound answers for is a session whose caller states the parse bound for a whole SCRIPT's
    /// worth of groups, where the count a flat script spends says nothing about the depth any
    /// one line of it reaches.
    fn run_compound(&mut self, compound: &::brush_parser::ast::CompoundCommand) -> i32 {
        use ::brush_parser::ast::CompoundCommand as Compound;
        if self.compound_depth >= self.config.bounds.nesting {
            return self.config.statuses.could_not_run;
        }
        self.compound_depth = self.compound_depth.saturating_add(1);
        let status = match compound {
            Compound::BraceGroup(group) => self.run_compound_list(&group.list),
            Compound::IfClause(clause) => self.run_if_clause(clause),
            Compound::WhileClause(clause) => self.run_while_clause(clause, true),
            Compound::UntilClause(clause) => self.run_while_clause(clause, false),
            Compound::ForClause(clause) => self.run_for_clause(clause),
            _ => self.config.statuses.could_not_run,
        };
        self.compound_depth = self.compound_depth.saturating_sub(1);
        status
    }

    /// Perform a list of and-or lists — the body of a compound command — and answer the
    /// status of the last one that ran.
    ///
    /// A `break` or `continue` the body performed ends the list where it stands: what runs next
    /// is for the enclosing LOOP to decide ([`Self::run_loop_control`]), so the lists between
    /// that command and the loop — the branches of an `if`, a brace group — stop too, exactly
    /// as the program's own `exit` stops them.
    fn run_compound_list(&mut self, list: &::brush_parser::ast::CompoundList) -> i32 {
        let mut status = self.config.statuses.success;
        for item in &list.0 {
            status = self.run_and_or_list(&item.0);
            if self.exit_status.is_some() || self.loop_control.is_some() {
                return status;
            }
        }
        status
    }

    /// `if LIST; then LIST; elif LIST; then LIST; else LIST; fi`: the branch whose
    /// condition answered success, and success for an `if` no branch of which ran, which is
    /// the status POSIX gives it ("exit status of zero if no condition is true").
    fn run_if_clause(&mut self, clause: &::brush_parser::ast::IfClauseCommand) -> i32 {
        let condition = self.run_tested_list(&clause.condition);
        if self.exit_status.is_some() {
            return condition;
        }
        if condition == self.config.statuses.success {
            return self.run_compound_list(&clause.then);
        }
        for otherwise in clause.elses.iter().flatten() {
            match otherwise.condition.as_ref() {
                // An `elif`, which is its own condition and its own branch.
                Some(condition) => {
                    let answered = self.run_tested_list(condition);
                    if self.exit_status.is_some() {
                        return answered;
                    }
                    if answered == self.config.statuses.success {
                        return self.run_compound_list(&otherwise.body);
                    }
                }
                // The `else` branch, which runs when no condition before it did.
                None => return self.run_compound_list(&otherwise.body),
            }
        }
        self.config.statuses.success
    }

    /// `while LIST; do LIST; done`, and `until`, which is the same with the condition read
    /// the other way round. The status is the last body that ran, and success for a loop
    /// whose body never did.
    fn run_while_clause(
        &mut self,
        clause: &::brush_parser::ast::WhileOrUntilClauseCommand,
        while_success: bool,
    ) -> i32 {
        let mut status = self.config.statuses.success;
        self.loop_depth = self.loop_depth.saturating_add(1);
        loop {
            // The budget is what ends a loop that has no end of its own, and a loop cut
            // off by it did not run to its own end: it answers as a command that could
            // not run rather than as the body that last succeeded.
            if self.commands_left == 0 {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return self.config.statuses.could_not_run;
            }
            let condition = self.run_tested_list(&clause.0);
            if self.exit_status.is_some() {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return condition;
            }
            if (condition == self.config.statuses.success) != while_success {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return status;
            }
            status = self.run_compound_list(&clause.1.list);
            if self.exit_status.is_some() {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return status;
            }
            match self.taken_loop_control() {
                Some(true) => continue,
                Some(false) => break,
                None => {}
            }
        }
        self.loop_depth = self.loop_depth.saturating_sub(1);
        status
    }

    /// What the `break` or `continue` the body just performed means FOR THIS LOOP:
    /// `Some(true)` to go on with its next iteration, `Some(false)` to end it, and `None` when
    /// the body performed neither — or when the one it performed is leaving an OUTER loop too,
    /// which is left in flight for that loop to take and makes this one end where it stands.
    fn taken_loop_control(&mut self) -> Option<bool> {
        let control = self.loop_control?;
        if control.levels > 1 {
            // "the n-th enclosing loop": this loop is one of the ones being left, and the
            // rest of them is the enclosing loop's to honour.
            self.loop_control = Some(LoopControl {
                levels: control.levels - 1,
                ..control
            });
            return Some(false);
        }
        self.loop_control = None;
        Some(control.goes_on)
    }

    /// Perform `break` or `continue`: leave the enclosing loops the argument names — one by
    /// default — so what runs next is that loop's next iteration or the command after it.
    ///
    /// "If n is greater than the number of enclosing loops, the outermost enclosing loop shall
    /// be used", and one performed where there is no loop at all changes nothing and answers
    /// success. A count that is not a positive integer is the command's own usage failure.
    fn run_loop_control(&mut self, name: &str, arguments: &[String]) -> i32 {
        let levels = match arguments.first() {
            Some(written) => match written.parse::<usize>() {
                Ok(levels) if levels >= 1 => levels,
                _ => return self.config.statuses.failure,
            },
            None => 1,
        };
        if self.loop_depth == 0 {
            return self.config.statuses.success;
        }
        self.loop_control = Some(LoopControl {
            goes_on: name == CONTINUE_COMMAND,
            levels: levels.min(self.loop_depth),
        });
        self.config.statuses.success
    }

    /// `for NAME in WORD…; do LIST; done`: the body once per word, with the loop's variable
    /// holding that word. A `for` with no word list at all iterates over nothing here —
    /// there are no positional parameters for it to take.
    fn run_for_clause(&mut self, clause: &::brush_parser::ast::ForClauseCommand) -> i32 {
        let mut status = self.config.statuses.success;
        let Some(values) = clause.values.as_ref() else {
            return status;
        };
        self.loop_depth = self.loop_depth.saturating_add(1);
        for word in values {
            let Some(value) = self.expand_word(&word.value) else {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return self.config.statuses.could_not_run;
            };
            if !self.set_variable(clause.variable_name.clone(), value) {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return self.config.statuses.could_not_run;
            }
            if self.commands_left == 0 {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return self.config.statuses.could_not_run;
            }
            status = self.run_compound_list(&clause.body.list);
            if self.exit_status.is_some() {
                self.loop_depth = self.loop_depth.saturating_sub(1);
                return status;
            }
            match self.taken_loop_control() {
                Some(true) => continue,
                Some(false) => break,
                None => {}
            }
        }
        self.loop_depth = self.loop_depth.saturating_sub(1);
        status
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
        // The command budget was spent for this command by [`Self::run_command`], which
        // spends it for every form.
        // The status of a command with no command name is the status of the last command
        // substitution ITS OWN words performed, so what an earlier command's words
        // substituted is not read here.
        self.last_substitution_status = None;
        let mut assignments: Vec<(String, String)> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let mut redirections = Redirections::default();
        let prefix = simple.prefix.as_ref().map(|prefix| &prefix.0);
        let suffix = simple.suffix.as_ref().map(|suffix| &suffix.0);
        // The PREFIX: the words before the command name, where an assignment is an assignment
        // — "the variable assignments shall affect the current execution environment".
        for item in prefix.into_iter().flatten() {
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
        // The SUFFIX: the words AFTER the command name, where a word of the `NAME=VALUE` form
        // is an ARGUMENT and nothing else — "assignment to a variable shall occur only in the
        // command prefix". The grammar still reports such a word as an assignment word,
        // because that is the form the utilities that take one read their operands in
        // (`export NAME=VALUE`), so what it reaches the command as is the word itself: an
        // option value carrying an `=` (`-o key=value`) is one of the command's own words, not
        // a variable this session sets.
        for item in suffix.into_iter().flatten() {
            match item {
                ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word)
                | ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(_, word) => {
                    let Some(expanded) = self.expand_word(&word.value) else {
                        return self.config.statuses.could_not_run;
                    };
                    words.push(expanded);
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
        // A command invoked through a WRAPPER that only changes how it is run
        // ([`COMMAND_WRAPPERS`]) is that command: the wrapper's first remaining word is the
        // command name and the rest are its words. One naming no utility at all runs
        // nothing, which is the wrapper's own failure.
        let mut name = name;
        // The duration a [`DURATION_WRAPPER`] bounds the command by, and whether it was asked
        // to keep that command's own answer.
        let mut bounded: Option<::std::time::Duration> = None;
        let mut preserve_status = false;
        loop {
            match name.as_deref() {
                Some(wrapper) if COMMAND_WRAPPERS.contains(&wrapper) => {}
                // The duration wrapper carries OPTIONS and then the DURATION it bounds the
                // command by ([`DURATION_WRAPPER`]), so those words are its own.
                Some(DURATION_WRAPPER) => {
                    while let Some(first) = words.first().cloned() {
                        if !first.starts_with('-') {
                            break;
                        }
                        words.remove(0);
                        if first == PRESERVE_STATUS_OPTION {
                            preserve_status = true;
                        }
                        // `--` ends the wrapper's own options: the word after it is the
                        // duration, whatever it starts with.
                        if first == "--" {
                            break;
                        }
                        if DURATION_WRAPPER_VALUE_OPTIONS.contains(&first.as_str())
                            && !words.is_empty()
                        {
                            words.remove(0);
                        }
                    }
                    if words.is_empty() {
                        return self.config.statuses.could_not_run;
                    }
                    let stated = words.remove(0);
                    // A duration this wrapper cannot read, or one no span can hold, is its own
                    // usage failure: nothing of the command it was to bound is performed.
                    let Some(limit) = stated_seconds(&stated)
                        .and_then(|seconds| ::std::time::Duration::try_from_secs_f64(seconds).ok())
                    else {
                        return self.config.statuses.could_not_run;
                    };
                    // "A duration of 0 disables the associated timeout", so such a wrapper
                    // bounds the command by nothing at all.
                    if !limit.is_zero() {
                        bounded = Some(bounded.map_or(limit, |held| held.min(limit)));
                    }
                }
                // The privilege wrapper carries OPTIONS of its own before the command it
                // names ([`PRIVILEGE_WRAPPER`]), so those words are dropped here rather than
                // taken for the command's name or its arguments.
                Some(PRIVILEGE_WRAPPER) => {
                    while let Some(first) = words.first().cloned() {
                        if !first.starts_with('-') {
                            break;
                        }
                        words.remove(0);
                        // `--` ends the wrapper's own options: the word after it is the
                        // command, whatever it starts with.
                        if first == "--" {
                            break;
                        }
                        if PRIVILEGE_WRAPPER_VALUE_OPTIONS.contains(&first.as_str())
                            && !words.is_empty()
                        {
                            let value = words.remove(0);
                            // An invocation asking for an identity this session does not have
                            // performs nothing ([`PRIVILEGE_WRAPPER_USER_OPTIONS`]).
                            if PRIVILEGE_WRAPPER_USER_OPTIONS.contains(&first.as_str())
                                && value != SUPERUSER
                            {
                                return self.config.statuses.could_not_run;
                            }
                        }
                    }
                }
                _ => break,
            }
            if words.is_empty() {
                return self.config.statuses.could_not_run;
            }
            name = Some(words.remove(0));
        }
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
        // What the command took before it ran, so a wrapper that bounds it by a duration can
        // tell whether it ran out of that duration ([`DURATION_WRAPPER`]).
        let taken_before = self.slept;
        let status = match name {
            Some(name) => self.run_built_in(&name, &words, &redirections),
            // Nothing but assignments, or a redirection alone: they have been applied, which
            // is the whole command. "If there is no command name, but the command contained a
            // command substitution, the command shall complete with the exit status of the
            // last command substitution performed."
            None => self
                .last_substitution_status
                .take()
                .unwrap_or(self.config.statuses.success),
        };
        // A command the wrapper's duration ran out on is one the wrapper ENDED, so it answers
        // the status that says so rather than what the command itself came to.
        let status = match bounded {
            Some(limit) if !preserve_status && self.slept.saturating_sub(taken_before) > limit => {
                TIMED_OUT_STATUS
            }
            _ => status,
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
    /// WHICH forms are read is the shared reading of a redirection node
    /// ([`AppliedRedirect::of`]); which of them this SESSION performs is its own: every
    /// session performs the output redirections of standard output and the duplication of one
    /// standard stream onto the other, while standard error to a file and standard input from
    /// one are a MACHINE program's forms, so only a session given a machine's tree performs
    /// them.
    ///
    /// A session given a machine's tree also performs the HERE-DOCUMENT and HERE-STRING
    /// forms, whose text is written inside the command itself: `<<WORD` — with `<<-WORD`
    /// stripping the leading tabs off each of its lines — and `<<<WORD` are the command's
    /// standard input, which is how a machine's program writes a document it was given into a
    /// file. A document whose delimiter was QUOTED (`<<'WORD'`) is taken literally, and an
    /// unquoted one is expanded, as the shell specifies.
    fn apply_redirect(
        &mut self,
        redirect: &::brush_parser::ast::IoRedirect,
        redirections: &mut Redirections,
    ) -> bool {
        let machine = self.volume.is_some();
        if let Some(here) = HereInput::of(redirect) {
            // A here-document reaches the descriptor a `<` does, so it is a form a session
            // that works over a tree performs.
            if !machine {
                return false;
            }
            // WHICH forms carry text, and what the shell does to each, is the shared reading
            // ([`HereInput`]); what is this session's own is the EXPANSION, and whether the
            // text is expanded at all is the delimiter's own question, which that reading
            // answers.
            let text = if here.requires_expansion {
                match self.expand_word(here.written) {
                    Some(expanded) => expanded,
                    None => return false,
                }
            } else {
                here.written.to_string()
            };
            here.record(&text, redirections);
            return true;
        }
        let ::brush_parser::ast::IoRedirect::File(descriptor, kind, target) = redirect else {
            return false;
        };
        let expanded = match target {
            ::brush_parser::ast::IoFileRedirectTarget::Duplicate(word)
            | ::brush_parser::ast::IoFileRedirectTarget::Filename(word) => {
                match self.expand_word(&word.value) {
                    Some(expanded) => expanded,
                    None => return false,
                }
            }
            _ => return false,
        };
        let Some(applied) = AppliedRedirect::of(*descriptor, kind, &expanded) else {
            return false;
        };
        if !machine
            && matches!(
                applied,
                AppliedRedirect::Input(_) | AppliedRedirect::Error(..)
            )
        {
            return false;
        }
        applied.record(redirections);
        true
    }

    /// `path` as the tree this session works over keys it: a path a program named WITHOUT a
    /// directory is resolved against this session's working directory, which is what `cd`
    /// changes ([`Self::run_change_directory`]), and `./name` and `name` are the same file.
    ///
    /// A session whose tree is its OWN keys a file by the path the program named and has no
    /// working directory to resolve one against, so the path is left exactly as written.
    fn at(&self, path: &str) -> String {
        if self.volume.is_none() {
            return path.to_string();
        }
        let named = path.trim();
        let folded = named.strip_prefix("./").unwrap_or(named);
        if folded.starts_with('/') {
            return folded.to_string();
        }
        if folded.is_empty() || folded == "." {
            return self.working_directory.clone();
        }
        format!(
            "{}/{folded}",
            self.working_directory
                .trim_end_matches('/')
        )
    }

    /// What reading `path` answers in the tree this session reads: a machine's tree where it
    /// was given one, and its own otherwise.
    fn read_path(&self, path: &str) -> Option<String> {
        match &self.volume {
            Some(volume) => volume.read(&self.at(path)),
            None => self.files.get(&file_path(path)?).cloned(),
        }
    }

    /// What the command being performed reads from STANDARD INPUT, and `None` when there is
    /// none to read.
    ///
    /// The file an input redirection names, when the command carries one (a session given a
    /// MACHINE's tree is the one that performs that form), and otherwise what the stage before
    /// it in a pipeline wrote. It is TAKEN: a stream is consumed by whoever reads it, so the
    /// command after it in the same pipeline reads nothing rather than the same bytes twice.
    /// There is no terminal behind this: a command with neither reads nothing at all.
    fn standard_input(&mut self, redirections: &Redirections) -> Option<String> {
        // The text a here-document or here-string carries IS the input, and it is the
        // command's own: nothing of the tree is read for it.
        if let Some(text) = redirections.input_text.clone() {
            return Some(text);
        }
        if let Some(path) = redirections.input.clone() {
            return self.read_path(&path);
        }
        self.piped_input.take()
    }

    /// [`Self::standard_input`] for the utilities that READ it, with the status to answer when
    /// there is nothing to read: the utility's own failure for a file standard input was
    /// redirected from that the tree holds nothing at — reported on standard error in the
    /// utility's own wording, as a named file is — and a command that could not run when there
    /// was no input at all, since there is no terminal behind it here.
    fn read_standard_input(
        &mut self,
        name: &str,
        redirections: &Redirections,
    ) -> Result<String, i32> {
        let redirected = redirections.input.clone();
        match self.standard_input(redirections) {
            Some(input) => Ok(input),
            None => match redirected {
                Some(path) => {
                    let _ = self.write_stderr(
                        &format!("{name}: {path}: No such file or directory\n"),
                        redirections,
                    );
                    Err(self.config.statuses.failure)
                }
                None => Err(self.config.statuses.could_not_run),
            },
        }
    }

    /// Write `text` at `path` in the tree this session writes, replacing what is there or
    /// ADDING to it.
    ///
    /// `false` is a write that did not happen — a path the tree does not accept, an entry it
    /// will not let a program write, or a write past one of its bounds — which the command
    /// performing it reports.
    fn write_path(&mut self, path: &str, text: &str, append: bool) -> bool {
        let resolved = self.at(path);
        if let Some(volume) = self.volume.as_mut() {
            return volume.write(&resolved, text, append);
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

    /// Whether `path` is one the tree this session works over will let a program READ but not
    /// WRITE: a sealed file or a path under a read-only filesystem
    /// ([`super::posix_shell_volume::ShellVolume::is_read_only`]).
    ///
    /// What the utility performing a write reports the refusal as, so a write under a
    /// read-only mount is reported as the filesystem it is rather than as a path that is not
    /// there. A session whose tree is its own has no read-only path: a path is a key of it.
    fn tree_is_read_only(&self, path: &str) -> bool {
        let resolved = self.at(path);
        self.volume
            .as_ref()
            .is_some_and(|volume| volume.is_read_only(&resolved))
    }

    /// Whether the tree this session works over holds anything at `path` — a file, a
    /// directory, or (in a machine's tree) a device.
    fn tree_holds(&self, path: &str) -> bool {
        match &self.volume {
            Some(volume) => volume.exists(&self.at(path)),
            None => self.tree_holds_file(path) || self.tree_holds_directory(path),
        }
    }

    /// Whether `path` is a FILE of the tree this session works over.
    fn tree_holds_file(&self, path: &str) -> bool {
        match &self.volume {
            Some(volume) => volume.is_file(&self.at(path)),
            None => file_path(path).is_some_and(|path| self.files.contains_key(&path)),
        }
    }

    /// Whether `path` is a DIRECTORY of the tree this session works over.
    ///
    /// A session's own tree keys a file by its whole path and holds no directory entry, so a
    /// directory of it is a path some file lies under — which is also as long as it exists
    /// for: removing the last file under it leaves no directory behind.
    fn tree_holds_directory(&self, path: &str) -> bool {
        match &self.volume {
            Some(volume) => volume.is_directory(&self.at(path)),
            None => file_path(path).is_some_and(|path| {
                let under = format!("{path}/");
                !self.files.contains_key(&path)
                    && self.files.keys().any(|held| held.starts_with(&under))
            }),
        }
    }

    /// Make `path` a directory of the tree this session works over, with every directory
    /// above it, answering whether the tree now has it.
    ///
    /// In a session's own tree there is nothing to make: a path is a key, so a file may be
    /// written under `path` whether or not anything made it first, and the only answer that
    /// is not success is a FILE already holding that path.
    fn tree_make_directories(&mut self, path: &str) -> bool {
        let resolved = self.at(path);
        match self.volume.as_mut() {
            Some(volume) => volume.make_directories(&resolved),
            None => !self.tree_holds_file(path),
        }
    }

    /// Mount the filesystem `tree` at `path` in the tree this session works over, so every
    /// path under it is a path of that filesystem from now on.
    ///
    /// Only a session given a MACHINE's tree takes one: a session's own tree keys a file by
    /// the path a program named and has no directory to mount a filesystem over.
    fn tree_mount(&mut self, path: &str, tree: ShellMountedTree) -> bool {
        let resolved = self.at(path);
        self.volume
            .as_mut()
            .is_some_and(|volume| volume.mount(&resolved, tree))
    }

    /// Take the filesystem mounted at `path` away again, answering whether one was mounted
    /// there.
    fn tree_unmount(&mut self, path: &str) -> bool {
        let resolved = self.at(path);
        self.volume
            .as_mut()
            .is_some_and(|volume| volume.unmount(&resolved))
    }

    /// What is mounted AT `path` exactly in that tree, and `None` for a path that is not a
    /// mount point of it.
    fn tree_mounted_at(&self, path: &str) -> Option<ShellMountFacts> {
        let resolved = self.at(path);
        self.volume.as_ref()?.mount_facts(&resolved).cloned()
    }

    /// What filesystem `path` lies ON in that tree, and `None` for a path on the machine's
    /// own storage.
    fn tree_file_system_at(&self, path: &str) -> Option<ShellMountFacts> {
        let resolved = self.at(path);
        self.volume.as_ref()?.file_system_facts(&resolved).cloned()
    }

    /// Remove the file at `path` from the tree this session works over, answering whether
    /// there was one to remove.
    fn tree_remove_file(&mut self, path: &str) -> bool {
        let resolved = self.at(path);
        if let Some(volume) = self.volume.as_mut() {
            return volume.remove_file(&resolved);
        }
        let Some(path) = file_path(path) else {
            return false;
        };
        match self.files.remove(&path) {
            Some(contents) => {
                self.tree_bytes = self
                    .tree_bytes
                    .saturating_sub(contents.len().saturating_add(path.len()));
                true
            }
            None => false,
        }
    }

    /// The names directly under the DIRECTORY `directory` of the tree this session works
    /// over (its root for the empty path), each once and in path order.
    ///
    /// The tree keys a file by its whole path and holds no entry of its own for a directory,
    /// so a name here is either a file directly under that path or the first segment of a
    /// deeper file's path, which is what makes it a directory.
    fn tree_entry_names(&self, directory: &str) -> Vec<String> {
        let resolved = self.at(directory);
        let held = resolved.trim();
        let held = held.strip_prefix("./").unwrap_or(held);
        let prefix = match held {
            "" | "." | "/" => String::new(),
            named => format!("{}/", named.trim_end_matches('/')),
        };
        let mut names: Vec<String> = Vec::new();
        for (path, _) in self.tree_files() {
            let Some(relative) = path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let relative = relative.strip_prefix('/').unwrap_or(relative);
            let name = relative.split('/').next().unwrap_or(relative).to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    /// Perform the program the ENVIRONMENT this session runs in provides for `name`, when it
    /// provides one ([`EnvironmentPrograms`]).
    ///
    /// What that program answered is what the command answers, and each line it wrote is
    /// written the way a command writes one — so a redirection takes it, a command
    /// substitution captures it, and nothing else keeps it. A name the environment provides
    /// no program for is where the shell's own COMMAND SEARCH ends with nothing: the command
    /// did not run, and that is reported the way a shell reports it
    /// ([`Self::command_not_found`]).
    ///
    /// The program is handed this session's own FILES ([`ProgramFiles`]), so a program whose
    /// work is to carry a file somewhere reads the one the commands around it wrote and
    /// leaves the one they read back. The programs are taken OFF the session while it runs
    /// and put back after, which is what lets it be handed the session's tree at the same
    /// time; the VARIABLES the session holds travel the same way, because they are the
    /// environment the program runs with ([`ProgramInvocation::environment`]) and the tree
    /// is reached through the session itself. Nothing else of the session is reachable from
    /// it.
    ///
    /// What the program wrote on EACH of its two streams is written where that stream goes
    /// ([`ProgramStreams`]): its output to standard output, and a diagnostic of its own to
    /// standard error, so what a caller reads as the program's output is what the program
    /// wrote there and nothing else.
    fn run_environment_program(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        if let Some(status) = self.performed_environment_program(name, arguments, redirections) {
            return status;
        }
        // The other half of the shell's own command search: a PROGRAM the tree this session
        // works over holds at that name ([`Self::performed_tree_program`]).
        if let Some(status) = self.performed_tree_program(name, arguments, redirections) {
            return status;
        }
        self.command_not_found(name, redirections)
    }

    /// Perform the PROGRAM the tree this session works over holds for the command `name`, as
    /// the shell's own COMMAND SEARCH resolves one — `None` when nothing resolved.
    ///
    /// "If the command name contains at least one slash, the shell shall execute the named
    /// utility", resolved against the directory the session is in; a name with no slash is
    /// searched for "using the PATH environment variable", in the order it names its
    /// directories, and the first directory holding a FILE of that name is the one that
    /// provides it. The search is over the TREE this session works over and over nothing else:
    /// there is no host `PATH` here, and a session given no tree resolves no program this way
    /// at all.
    ///
    /// What is found is performed as a SCRIPT in this session, with the operands as its
    /// positional parameters — exactly as `sh FILE ARG…` is ([`Self::run_nested_program`]),
    /// under the same budget and the same depth — because a program is run BY a shell here and
    /// there is no process to execute one in. That is also what a shell does with a file it
    /// could not execute itself: "the shell shall execute [it] as a shell script".
    ///
    /// A file whose own interpreter line names another program is a program of THAT
    /// interpreter, and the interpreter is a program this environment has to provide like any
    /// other: when it provides none, the search ends with nothing and that is reported for the
    /// interpreter the file names ([`Self::command_not_found`]), because nothing ran.
    fn performed_tree_program(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> Option<i32> {
        if !self.works_over_a_tree() {
            return None;
        }
        let path = if name.contains('/') {
            name.to_string()
        } else {
            self.searched_program_path(name)?
        };
        let text = self.read_path(&path)?;
        if let Some(interpreter) = script_interpreter(&text) {
            if !NESTED_SHELL_NAMES.contains(&command_base_name(&interpreter)) {
                let named = interpreter.clone();
                return Some(self.command_not_found(&named, redirections));
            }
        }
        let mut named = vec![path];
        named.extend(arguments.iter().cloned());
        Some(self.run_script_text(&text, named))
    }

    /// The path of the first directory of this session's `PATH` that holds a FILE called
    /// `name`, and `None` when none of them does — the shell's own path search.
    fn searched_program_path(&self, name: &str) -> Option<String> {
        let published = self.variable(SEARCH_PATH_VARIABLE)?.to_string();
        published
            .split(':')
            .filter(|directory| !directory.is_empty())
            .map(|directory| format!("{}/{name}", directory.trim_end_matches('/')))
            .find(|path| self.tree_holds_file(path))
    }

    /// Perform `text` as a program of this session, with `arguments` as its positional
    /// parameters — what a nested shell ([`Self::run_nested_program`]) and a resolved program
    /// ([`Self::performed_tree_program`]) are both performed through, and the ONE place either
    /// of them spends what running a program costs.
    ///
    /// The budget and the depth are read here and CHARGED here, around the run
    /// ([`Bounds::substitutions`], [`Bounds::substitution_depth`]): every level of a program
    /// that runs a program spends one of each, so a program that invokes itself — a script
    /// running `sh` on its own path, a program of the tree whose name resolves back to itself —
    /// is ended by that bound, with the status a command that could not run answers, rather
    /// than by this module's own recursion.
    fn run_script_text(&mut self, text: &str, arguments: Vec<String>) -> i32 {
        if self.substitutions_left == 0
            || self.nesting_depth() >= self.config.bounds.substitution_depth
        {
            return self.config.statuses.could_not_run;
        }
        self.substitutions_left = self.substitutions_left.saturating_sub(1);
        self.nested_programs = self.nested_programs.saturating_add(1);
        // The nested program reads ITS own operands, and the program around it reads the ones
        // it was given once the nested one has ended.
        let held = ::std::mem::replace(&mut self.positional, arguments);
        let status = self.run_program(text);
        self.positional = held;
        self.nested_programs = self.nested_programs.saturating_sub(1);
        status
    }

    /// What this session answers for a command NAME nothing resolved: no utility of its own
    /// catalogs and no program of its environment.
    ///
    /// The shell's command search found nothing to run, and a shell says so: "if the search
    /// is unsuccessful, the shell shall write an error message and the exit status shall be
    /// 127". The diagnostic names the command and goes to standard ERROR — where a
    /// redirection of that stream takes it, and where the session keeps it for a caller that
    /// reports the two streams — beside that status ([`Statuses::could_not_run`]). A bare
    /// status with nothing written for it would leave a caller unable to tell a command this
    /// environment does not have from one that ran and wrote nothing.
    fn command_not_found(&mut self, name: &str, redirections: &Redirections) -> i32 {
        self.unresolved_commands = self.unresolved_commands.saturating_add(1);
        let _ = self.write_stderr(
            &format!("{SHELL_NAME}: {name}: {COMMAND_NOT_FOUND}\n"),
            redirections,
        );
        self.config.statuses.could_not_run
    }

    /// [`Self::run_environment_program`], answering `None` when this environment provides no
    /// program of that name at all — which is what lets a caller fall back to a utility this
    /// session performs itself, rather than shadowing the environment's own program with it.
    fn performed_environment_program(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> Option<i32> {
        let programs = self.programs.take()?;
        // What the command would READ on standard input, left in place while the program is
        // asked: a name this environment provides no program for falls back to a utility of
        // this session, which reads that same input.
        let input = self.peeked_standard_input(redirections);
        // The variables are the ENVIRONMENT this invocation runs with, taken off the session
        // while it runs and put back after for the same reason the programs are: the session
        // itself is handed over as the program's tree, and nothing of it is reachable from
        // the program besides those files. A program changes none of them — a variable one
        // sets is its own process's — so what goes back is what came off.
        let environment = ::std::mem::take(&mut self.variables);
        // The directory the session is IN, which a program resolves its own relative paths
        // against: a copy, because the session itself is handed over as the program's tree.
        let working_directory = self.working_directory.clone();
        let answered = {
            let invocation = ProgramInvocation {
                name,
                arguments,
                standard_input: input.as_deref(),
                working_directory: &working_directory,
                environment: &environment,
            };
            let mut files = SessionFiles { session: self };
            programs.run_invocation(&invocation, &mut files)
        };
        self.variables = environment;
        self.programs = Some(programs);
        let outcome = answered?;
        // The program performed the invocation, so the stream it was handed is consumed: a
        // stream is read by whoever reads it, and nothing after this command reads it twice.
        let _ = self.standard_input(redirections);
        for line in outcome.output {
            if !self.write_stdout(&format!("{line}\n"), redirections) {
                return Some(self.config.statuses.failure);
            }
        }
        // What the program reported it could not do goes where a command's diagnostics go,
        // which is not where its output goes.
        for line in outcome.errors {
            if !self.write_stderr(&format!("{line}\n"), redirections) {
                return Some(self.config.statuses.failure);
            }
        }
        Some(outcome.status)
    }

    /// What the command being performed would read from STANDARD INPUT, WITHOUT consuming it
    /// ([`Self::standard_input`] is the reading that does).
    fn peeked_standard_input(&self, redirections: &Redirections) -> Option<String> {
        if let Some(text) = redirections.input_text.clone() {
            return Some(text);
        }
        match redirections.input.as_deref() {
            Some(path) => self.read_path(path),
            None => self.piped_input.clone(),
        }
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
    ///
    /// A value that would take what this session holds past the MEMORY its environment gives
    /// the program ([`SessionConfig::memory`]) is a different answer: the program asked its
    /// environment for memory that environment does not have, so the program is KILLED rather
    /// than left to carry on without the value ([`Self::kill_for_memory`]). The demand is
    /// weighed BEFORE the bytes are held, the way a runtime refuses an allocation past a
    /// process's limit rather than making it first.
    ///
    /// A change made while a command substitution is open is recorded on that substitution's
    /// undo log first ([`OpenSubstitution`]), so it can be rolled back when the subshell
    /// ends.
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
        if self.memory_limit.is_some_and(|limit| would_hold > limit) {
            self.kill_for_memory();
            return false;
        }
        if would_hold > self.config.bounds.session_bytes {
            return false;
        }
        self.record_for_rollback(&name);
        self.held_bytes = would_hold;
        self.variables.insert(name, value);
        true
    }

    /// Remove `name` from this session, with the bytes it held.
    fn unset_variable(&mut self, name: &str) {
        self.record_for_rollback(name);
        if let Some(value) = self.variables.remove(name) {
            self.held_bytes = self
                .held_bytes
                .saturating_sub(name.len().saturating_add(value.len()));
        }
    }

    /// Record what the innermost open command substitution has to put `name` back to, when
    /// one is open and has not recorded it already — the FIRST touch is the one that names
    /// the value to go back to.
    fn record_for_rollback(&mut self, name: &str) {
        let held = self.variables.get(name).cloned();
        if let Some(undo) = self.substitution_undo.last_mut() {
            undo.entry(name.to_string()).or_insert(held);
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
        if redirections.out_to_error {
            // `1>&2`: this command's output goes where its diagnostics go.
            return self.write_error_stream(text, redirections);
        }
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
        if redirections.error_to_out {
            // `2>&1`: this command's diagnostics go where its output goes, so they are what
            // a redirection takes, what a substitution captures and what the caller reads as
            // standard output.
            return self.write_stream(redirections.out.clone(), text);
        }
        self.write_error_stream(text, redirections)
    }

    /// [`Self::write_stderr`]'s write to the ERROR stream itself, with no duplication
    /// applied: the file or device a redirection of standard error names, and otherwise what
    /// the session keeps.
    fn write_error_stream(&mut self, text: &str, redirections: &Redirections) -> bool {
        if let Some((path, append)) = redirections.error.clone() {
            if !self.write_path(&path, text, append) {
                self.redirect_failed = true;
                return false;
            }
            return true;
        }
        // Kept for the caller that reports the two streams separately ([`Self::errors`]),
        // and ALSO on the console of a session given a machine's tree, which is where a
        // guest's diagnostics are read — unless the caller reports the two as members of its
        // own ([`Self::reporting_both_streams`]), for which they are separate streams.
        self.keep_error(text);
        if self.volume.is_some() && !self.separate_errors {
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

    /// [`Self::keep_output`] for standard ERROR, into the lines [`Self::errors`] answers
    /// with and under the same limits.
    fn keep_error(&mut self, text: &str) {
        let Some(limits) = self.config.output else {
            return;
        };
        if text.is_empty() {
            return;
        }
        for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
            if self.errors.len() >= limits.lines {
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
            self.errors.push(line);
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
    /// removes them. The machine commands are [`Self::run_volume_command`]'s, and a command
    /// neither catalog names is the ENVIRONMENT's to provide
    /// ([`Self::run_environment_program`]).
    fn run_built_in(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        // The catalogs are what this session provides: a command outside them needs a
        // program, which is the environment's to provide and not the shell's.
        if !BUILT_IN_COMMANDS.contains(&name) {
            // The text filters answer out of ONE input — a file, or what the stage before them
            // wrote — so they need no tree of the environment's and every session performs
            // them. The ENVIRONMENT's own program for that name comes FIRST: a caller whose
            // environment provides `wc` has its own catalog for it, and this must not shadow
            // it; the filter is what a session whose environment provides none falls back to.
            if TEXT_FILTER_COMMANDS.contains(&name) {
                if let Some(status) =
                    self.performed_environment_program(name, arguments, redirections)
                {
                    return status;
                }
                return self.run_text_filter(name, arguments, redirections);
            }
            // The commands about the DIRECTORY the session is in, which only a session
            // working over a machine's tree provides ([`MACHINE_TREE_COMMANDS`]).
            if self.volume.is_some() && MACHINE_TREE_COMMANDS.contains(&name) {
                return match name {
                    CHANGE_DIRECTORY => self.run_change_directory(arguments, redirections),
                    _ => self.run_print_directory(redirections),
                };
            }
            if (self.volume.is_some() || self.working_tree) && VOLUME_COMMANDS.contains(&name) {
                // `ls` NAMES what the tree holds rather than changing it, and an environment
                // whose storage has its own catalog provides its own program for that name —
                // which this must not shadow, the same precedence the text filters above
                // take. The file utility is what a session whose environment provides none
                // falls back to.
                if name == LIST_COMMAND {
                    if let Some(status) =
                        self.performed_environment_program(name, arguments, redirections)
                    {
                        return status;
                    }
                }
                return self.run_volume_command(name, arguments, redirections);
            }
            return self.run_environment_program(name, arguments, redirections);
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
            "cat" => self.run_files_command(name, arguments, redirections),
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
            SET_COMMAND => self.run_set(arguments, redirections),
            BREAK_COMMAND | CONTINUE_COMMAND => self.run_loop_control(name, arguments),
            SEQUENCE_COMMAND => self.run_sequence(arguments, redirections),
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform `set`: turn the shell OPTIONS its words name on (`-`) or off (`+`), for the
    /// options whose effect this session performs ([`ShellOption`]).
    ///
    /// `set` with no word at all writes "the names and values of all shell variables", which
    /// is what the utility does when it is given none. Every other FORM is one no session
    /// performs — an option whose effect it does not perform, a positional-parameter
    /// assignment, `set --` — and fails closed as a command that could not be run, with no
    /// option changed: the words are read first and applied only once every one of them is an
    /// option this session performs, so a program is never left half-way between two sets of
    /// options.
    fn run_set(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        if arguments.is_empty() {
            let written: String = self
                .variables
                .iter()
                .map(|(name, value)| format!("{name}={value}\n"))
                .collect();
            return if self.write_stdout(&written, redirections) {
                self.config.statuses.success
            } else {
                self.config.statuses.failure
            };
        }
        let mut changes: Vec<(ShellOption, bool)> = Vec::new();
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            // `-` turns the options its letters name on and `+` turns them off, which is the
            // way round the utility is specified.
            let (on, letters) = match (word.strip_prefix('-'), word.strip_prefix('+')) {
                (Some(letters), _) => (true, letters),
                (_, Some(letters)) => (false, letters),
                _ => return self.config.statuses.could_not_run,
            };
            if letters.is_empty() {
                return self.config.statuses.could_not_run;
            }
            for letter in letters.chars() {
                // `o` names the option by its long NAME, which is the word after this one —
                // wherever in the group the letter stands, so `-uo pipefail` sets both the
                // option its letter names and the one its name does. It is the only spelling
                // some options have.
                if letter == LONG_OPTION_LETTER {
                    let Some(option) = words.next().and_then(|named| option_named(named)) else {
                        return self.config.statuses.could_not_run;
                    };
                    changes.push((option, on));
                    continue;
                }
                let Some(option) = option_lettered(letter) else {
                    return self.config.statuses.could_not_run;
                };
                changes.push((option, on));
            }
        }
        for (option, on) in changes {
            match option {
                ShellOption::ErrExit => self.options.errexit = on,
                ShellOption::NoUnset => self.options.nounset = on,
                ShellOption::PipeFail => self.options.pipefail = on,
            }
        }
        self.config.statuses.success
    }

    /// Perform one of the commands a session working over a TREE provides
    /// ([`VOLUME_COMMANDS`]) — a machine's ([`Self::in_volume`]) or its own working one
    /// ([`Self::with_file_utilities`]) — over that tree and nothing else.
    ///
    /// `printf` writes its format with its arguments substituted. `mkdir` makes a
    /// directory (`-p` makes the ones above it too), `touch` makes an empty file where the
    /// tree holds none, and `rm` removes a file (`-f` passes over one that is not there).
    /// `sync` flushes the tree, which has nothing pending in it.
    /// `ls` writes the names the tree holds at the paths it was given ([`Self::run_list`]).
    /// `tee` writes what it READS into each file it names and passes it on
    /// ([`Self::run_tee`]).
    /// `test`, and its `[` spelling, answers a question about the tree or about two values.
    /// `sh -c` performs a program in THIS session, which is how a machine's init system
    /// runs one line of a script, `sh FILE` performs the script the tree holds at that path
    /// in the same way, `.` performs the commands of a file the tree holds in it
    /// ([`Self::run_dot`]), and `sleep` states the TIME the program took
    /// ([`Self::slept`]).
    fn run_volume_command(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        match name {
            // The utilities that work over the session's FILES are the shared reading of them
            // ([`super::posix_shell_files`]), performed over this session's own tree.
            name if posix_shell_files::FILE_COMMANDS.contains(&name) => {
                self.run_files_command(name, arguments, redirections)
            }
            "printf" => self.run_printf(arguments, redirections),
            TEE_COMMAND => self.run_tee(arguments, redirections),
            "test" | "[" => self.run_test(name, arguments),
            "sh" | "bash" => self.run_nested_program(arguments, redirections),
            "." | "source" => self.run_dot(arguments, redirections),
            "sleep" => self.run_sleep(arguments, redirections),
            _ => self.config.statuses.could_not_run,
        }
    }

    /// Perform `cd`: change the directory the paths a program names without one are resolved
    /// against ([`Self::at`]).
    ///
    /// The operand is resolved against the directory the session is in now, so a relative
    /// operand walks from there, and the change happens only for a directory the tree HOLDS:
    /// a path it holds a file at is not a directory and a path it holds nothing at is not
    /// there, each reported in the wording the utility reports it in and leaving the session
    /// where it was. An invocation with no operand changes to the directory `HOME` names, and
    /// changes nothing at all for a session whose environment publishes none — "the default
    /// behavior is implementation-defined and no further action shall be taken"; `cd -`
    /// changes back to the directory before this one and writes it, as a shell does.
    ///
    /// A session whose tree is its own performs none of this: such a tree keys a file by the
    /// path a program named and holds no directory to be in.
    fn run_change_directory(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        if self.volume.is_none() {
            return self.config.statuses.could_not_run;
        }
        let operand = arguments
            .iter()
            .map(String::as_str)
            .find(|word| *word == PREVIOUS_DIRECTORY_OPERAND || !word.starts_with('-'));
        let previous = operand == Some(PREVIOUS_DIRECTORY_OPERAND);
        let named = match operand {
            Some(PREVIOUS_DIRECTORY_OPERAND) => match self
                .variable(PREVIOUS_DIRECTORY_VARIABLE)
                .map(str::to_string)
            {
                Some(held) => held,
                None => {
                    let _ = self.write_stderr(
                        &format!("{CHANGE_DIRECTORY}: {PREVIOUS_DIRECTORY_VARIABLE} not set\n"),
                        redirections,
                    );
                    return self.config.statuses.failure;
                }
            },
            Some(named) => named.to_string(),
            None => match self
                .variable(HOME_VARIABLE)
                .filter(|held| !held.is_empty())
                .map(str::to_string)
            {
                Some(home) => home,
                None => return self.config.statuses.success,
            },
        };
        let target = self.at(&named);
        if self.tree_holds_file(&target) {
            let _ = self.write_stderr(
                &format!("{CHANGE_DIRECTORY}: {named}: Not a directory\n"),
                redirections,
            );
            return self.config.statuses.failure;
        }
        if !self.tree_holds_directory(&target) {
            let _ = self.write_stderr(
                &format!("{CHANGE_DIRECTORY}: {named}: No such file or directory\n"),
                redirections,
            );
            return self.config.statuses.failure;
        }
        let left = ::std::mem::replace(&mut self.working_directory, target.clone());
        let _ = self.set_variable(PREVIOUS_DIRECTORY_VARIABLE.to_string(), left);
        let _ = self.set_variable(WORKING_DIRECTORY_VARIABLE.to_string(), target.clone());
        if previous && !self.write_stdout(&format!("{target}\n"), redirections) {
            return self.config.statuses.failure;
        }
        self.config.statuses.success
    }

    /// Perform `pwd`: write the directory this session is in — "write the absolute pathname of
    /// the current working directory".
    fn run_print_directory(&mut self, redirections: &Redirections) -> i32 {
        if self.volume.is_none() {
            return self.config.statuses.could_not_run;
        }
        let written = format!("{}\n", self.working_directory);
        if self.write_stdout(&written, redirections) {
            self.config.statuses.success
        } else {
            self.config.statuses.failure
        }
    }

    /// Perform one of the FILE utilities ([`posix_shell_files::FILE_COMMANDS`]) over this
    /// session's tree, and write what it produced through this session's own streams.
    ///
    /// WHAT each utility does and SAYS is the shared reading of it; what the session adds is
    /// where its files are ([`SessionFiles`]) and where its two streams go. `cat` with no
    /// operand reads STANDARD INPUT, which only the session can resolve — the file one was
    /// redirected from, or what the stage before it in a pipeline wrote.
    fn run_files_command(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        let reads_input = name == "cat" && !posix_shell_files::names_an_operand(arguments);
        let input = if reads_input {
            match self.read_standard_input(name, redirections) {
                Ok(input) => Some(input),
                Err(status) => return status,
            }
        } else {
            None
        };
        let performed = {
            let mut files = SessionFiles { session: self };
            posix_shell_files::perform_file_command(
                name,
                arguments,
                &mut files,
                input.as_deref(),
            )
        };
        let Some(outcome) = performed else {
            return self.config.statuses.could_not_run;
        };
        if !outcome.errors.is_empty() {
            let _ = self.write_stderr(&outcome.errors, redirections);
        }
        if !outcome.output.is_empty() && !self.write_stdout(&outcome.output, redirections) {
            return self.config.statuses.failure;
        }
        if outcome.succeeded {
            self.config.statuses.success
        } else {
            self.config.statuses.failure
        }
    }

    /// Perform one of the TEXT FILTERS ([`TEXT_FILTER_COMMANDS`]) over its one input: the file
    /// it names, or standard input.
    ///
    /// What it writes is what [`super::posix_shell_text_filters`] computed over those bytes —
    /// a digest, a count, a selection — and nothing of the invocation is interpreted here. An
    /// option or operand form that module does not perform is a command this session could not
    /// run, a file the tree holds none of is reported the way the file utilities report one,
    /// and a filter with no input at all could not run either: there is no terminal behind
    /// standard input here.
    fn run_text_filter(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        let Some(filter) = Filter::read(name, arguments) else {
            return self.config.statuses.could_not_run;
        };
        let (label, input) = match filter.file() {
            Some(path) => match self.read_path(path) {
                Some(held) => (path.to_string(), held),
                None => {
                    let _ = self.write_stderr(
                        &format!("{name}: {path}: No such file or directory\n"),
                        redirections,
                    );
                    return self.config.statuses.failure;
                }
            },
            None => match self.read_standard_input(name, redirections) {
                Ok(input) => (STANDARD_INPUT_LABEL.to_string(), input),
                Err(status) => return status,
            },
        };
        let written = match filter.perform(&label, &input) {
            Ok(performed) => performed,
            // An input the utility cannot compute over: what it writes about it goes to
            // standard error, and nothing is written to standard output.
            Err(refusal) => {
                let _ = self.write_stderr(&format!("{refusal}\n"), redirections);
                return self.config.statuses.failure;
            }
        };
        if !self.write_stdout(&written.written, redirections) {
            return self.config.statuses.failure;
        }
        // The status of a utility that reports a SELECTION is that selection
        // ([`super::posix_shell_text_filters::Selection`]), which is what an `if` around such
        // a command branches on; every other one answers success for an input it computed
        // over.
        if written.selected == Some(false) {
            return self.config.statuses.failure;
        }
        self.config.statuses.success
    }

    /// `sleep`: the time the program asked to take, added to what this session has slept
    /// ([`Self::slept`]).
    ///
    /// No thread waits for it. This session performs a program's commands in process and
    /// has no clock of its own to spend, so what a `sleep` states is the TIME THE PROGRAM
    /// TOOK: the caller whose API reports a duration, or converges a submitted task on one,
    /// honours it, and a caller that reports no duration performs the sleep in no time at
    /// all. What one program may ask for in total is bounded ([`Bounds::sleep_seconds`]).
    ///
    /// "The sleep utility shall suspend execution for at least the integral number of
    /// seconds specified by the time operand": a time this command cannot read is the
    /// utility's own usage failure, and so is a missing operand.
    fn run_sleep(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some(operand) = arguments.iter().find(|argument| !argument.starts_with('-')) else {
            let _ = self.write_stderr("sleep: missing operand\n", redirections);
            return self.config.statuses.failure;
        };
        let Some(seconds) = stated_seconds(operand) else {
            let _ = self.write_stderr(
                &format!("sleep: invalid time interval '{operand}'\n"),
                redirections,
            );
            return self.config.statuses.failure;
        };
        let asked = ::std::time::Duration::try_from_secs_f64(seconds)
            .unwrap_or(::std::time::Duration::ZERO);
        let ceiling = ::std::time::Duration::from_secs(self.config.bounds.sleep_seconds);
        self.slept = self.slept.saturating_add(asked).min(ceiling);
        self.config.statuses.success
    }

    /// `tee`: "copy standard input to each OUTPUT FILE and also to standard output".
    ///
    /// What it reads is its standard input ([`Self::read_standard_input`]) — a here-document,
    /// a file an input redirection named, or what the stage before it in a pipeline wrote —
    /// and it is written into every file operand, from the start or APPENDED for `-a`
    /// ("append the output to the files"), and then passed on. A write the tree will not take
    /// is reported in the utility's own wording and the command answers failure, without
    /// stopping the files after it.
    fn run_tee(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let append = arguments
            .iter()
            .any(|argument| argument == "-a" || argument == "--append");
        let paths: Vec<String> = arguments
            .iter()
            .filter(|argument| !argument.starts_with('-'))
            .cloned()
            .collect();
        let input = match self.read_standard_input(TEE_COMMAND, redirections) {
            Ok(input) => input,
            Err(status) => return status,
        };
        let mut status = self.config.statuses.success;
        for path in paths {
            if !self.write_path(&path, &input, append) {
                let _ = self.write_stderr(
                    &format!("{TEE_COMMAND}: {path}: Cannot write to file\n"),
                    redirections,
                );
                status = self.config.statuses.failure;
            }
        }
        if !self.write_stdout(&input, redirections) {
            status = self.config.statuses.failure;
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
        let Some(text) = printf_formatted(format, rest) else {
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

    /// `seq`: the numbers from the first operand to the last, in steps of the increment
    /// between them, one per line — "seq LAST", "seq FIRST LAST" and "seq FIRST INCREMENT
    /// LAST", which is the form a program written as "try this a few times" iterates over.
    ///
    /// `-s` writes a separator of its own between the numbers instead of a line ending, and
    /// `-w` pads them with leading zeros to a common width, as the utility does. A sequence
    /// that does not reach its last number writes nothing and answers success, which is what
    /// `seq 5 1` does.
    ///
    /// How many numbers one invocation writes is bounded by [`MAX_SEQUENCE_NUMBERS`], the
    /// emulator's own number: the operands are a caller's text, so a sequence past it is one
    /// this command does not perform rather than output this process holds. A non-integer
    /// operand, and an option this command does not read, are the same closed failure: nothing
    /// is written.
    fn run_sequence(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let mut separator: Option<String> = None;
        let mut equal_width = false;
        let mut operands: Vec<i64> = Vec::new();
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            if word == "--" {
                for operand in words.by_ref() {
                    match operand.parse::<i64>() {
                        Ok(number) => operands.push(number),
                        Err(_) => return self.config.statuses.could_not_run,
                    }
                }
                break;
            }
            // A negative number is an operand, not an option, which is what makes the
            // leading-dash test a test of the WHOLE word.
            if word.parse::<i64>().is_ok() {
                operands.push(word.parse::<i64>().unwrap_or_default());
                continue;
            }
            if word == "-s" || word == "--separator" {
                match words.next() {
                    Some(given) => separator = Some(given.clone()),
                    None => return self.config.statuses.could_not_run,
                }
                continue;
            }
            if let Some(given) = word
                .strip_prefix("--separator=")
                .or_else(|| word.strip_prefix("-s"))
            {
                separator = Some(given.to_string());
                continue;
            }
            if word == "-w" || word == "--equal-width" {
                equal_width = true;
                continue;
            }
            // Anything else — an option this command does not read, and an operand that is no
            // integer — is an invocation it performs nothing of.
            return self.config.statuses.could_not_run;
        }
        let (first, increment, last) = match operands.as_slice() {
            [last] => (1, 1, *last),
            [first, last] => (*first, 1, *last),
            [first, increment, last] => (*first, *increment, *last),
            _ => return self.config.statuses.could_not_run,
        };
        if increment == 0 {
            return self.config.statuses.could_not_run;
        }
        let mut numbers: Vec<i64> = Vec::new();
        let mut at = first;
        while (increment > 0 && at <= last) || (increment < 0 && at >= last) {
            if numbers.len() >= MAX_SEQUENCE_NUMBERS {
                return self.config.statuses.could_not_run;
            }
            numbers.push(at);
            match at.checked_add(increment) {
                Some(next) => at = next,
                None => break,
            }
        }
        let width = if equal_width {
            numbers
                .iter()
                .map(|number| number.to_string().len())
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        let written: Vec<String> = numbers
            .iter()
            .map(|number| format!("{:0>width$}", number.to_string()))
            .collect();
        // The sequence ends with a line ending whichever separator the invocation named, which
        // is what the utility writes.
        let text = match written.is_empty() {
            true => String::new(),
            false => format!(
                "{}\n",
                written.join(separator.as_deref().unwrap_or("\n"))
            ),
        };
        if self.write_stdout(&text, redirections) {
            self.config.statuses.success
        } else {
            self.config.statuses.failure
        }
    }

    /// `test` / `[`: the question its arguments ask, answered as a status — success for
    /// true, failure for false.    ///
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
    ///
    /// The questions about a FILE are the shared reading of them
    /// ([`posix_shell_files::file_test_answer`]) over this session's own tree, and the
    /// questions about VALUES are the same wherever the utility is performed
    /// ([`value_test_answer`]).
    fn test_answer(&mut self, arguments: &[String]) -> Option<bool> {
        let answered = {
            let files = SessionFiles { session: self };
            posix_shell_files::file_test_answer(arguments, &files)
        };
        answered.or_else(|| value_test_answer(arguments))
    }

    /// `sh -c PROGRAM` (and `bash -c`), and `sh FILE`: the program performed in THIS
    /// session, under the same bounds and against the same tree and variables.
    ///
    /// `sh FILE` is the other way a program of an environment with storage is written — a
    /// script the commands before it left in the tree — so the text performed is read out of
    /// the SAME tree ([`Self::read_path`]) and nothing else: no host path is opened for it,
    /// and a path the tree holds no file for is reported the way a shell reports a script it
    /// cannot open, with the status it exits with.
    ///
    /// It counts against the same budget and the same DEPTH a command substitution does
    /// ([`Bounds::substitutions`], [`Bounds::substitution_depth`]): a nested shell is the
    /// other way one program can run another, so what bounds this module's own recursion has
    /// to count both. That accounting is [`Self::run_script_text`]'s, which is the ONE place a
    /// program of this session runs another — it checks the budget and the depth and CHARGES
    /// them around the run, so a program that invokes itself (a script running `sh` on its own
    /// path) ends on that bound rather than on this module's own recursion.
    ///
    /// Any other invocation — a login shell, an interactive one, an option this session has
    /// nothing to apply — needs something this session does not have, so it is a command that
    /// could not run.
    ///
    /// One invocation is not such a case: a shell invoked with NO operand at all. "If there
    /// are no operands and the -c option is not specified, the -s option shall be assumed" —
    /// the shell reads its commands from STANDARD INPUT — so the text it is given there is the
    /// program it performs: the here-document, the file an input redirection names, or what the
    /// stage before it in a pipeline wrote ([`Self::standard_input`]). A session handed none of
    /// those has an input already at end of file, so no command of it runs and the status is
    /// the one a shell that executed none exits with: "the exit status shall be that of the
    /// last command executed", or success where no command was. It is the program a minimal
    /// image whose own command is a shell declares, so a container of one ends exactly as it
    /// does on a node with no terminal attached.
    fn run_nested_program(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some((first, rest)) = arguments.split_first() else {
            // The program is this shell's own standard input, which the budget and the
            // depth are charged around exactly as they are for an `sh -c` program.
            return match self.standard_input(redirections) {
                Some(program) => self.run_script_text(&program, vec![SHELL_NAME.to_string()]),
                None => self.config.statuses.success,
            };
        };
        // The POSITIONAL PARAMETERS the nested program is given, which is what the operands
        // after the program are: `sh -c PROGRAM NAME ARG…` names the program text, then the
        // NAME that program is invoked under and the arguments after it, and `sh FILE ARG…`
        // gives the script its own path as the name it was invoked under.
        let (program, nested_arguments) = if first == "-c" {
            match rest.split_first() {
                Some((program, operands)) => {
                    // The FIRST operand after the program text is the NAME the program was
                    // invoked under — `$0` — and the ones after it are its `$1`, `$2`, …; an
                    // invocation that names none leaves the shell's own name as `$0`.
                    let mut named = Vec::with_capacity(operands.len().max(1));
                    if operands.is_empty() {
                        named.push(SHELL_NAME.to_string());
                    } else {
                        named.extend(operands.iter().cloned());
                    }
                    (program.clone(), named)
                }
                None => return self.config.statuses.could_not_run,
            }
        } else if first.starts_with('-') {
            return self.config.statuses.could_not_run;
        } else {
            match self.read_path(first) {
                Some(text) => {
                    let mut named = vec![first.clone()];
                    named.extend(rest.iter().cloned());
                    (text, named)
                }
                None => {
                    let _ = self.write_stderr(
                        &format!("sh: {first}: No such file or directory\n"),
                        redirections,
                    );
                    return self.config.statuses.could_not_run;
                }
            }
        };
        // The budget and the depth are checked and CHARGED where the nested program is run
        // ([`Self::run_script_text`]), so there is no second reading of them here to fall out
        // of step with it.
        self.run_script_text(&program, nested_arguments)
    }

    /// `. FILE` (and its bash `source` spelling): the commands of a file the tree holds,
    /// performed in THIS session.
    ///
    /// The dot utility is specified to "execute commands from the file in the current
    /// environment", so the variables the file's own assignments set are the session's once
    /// it has run — which is how a program reads a file of assignments the environment
    /// published to it, a distribution's release file or a tool's profile script. The file
    /// is read out of the tree this session works over and out of nothing else: there is no
    /// search path here, and no host path, so a path the tree holds no file for is one this
    /// utility could not read — reported on standard error, with the status the utility
    /// answers for a file it could not read rather than with the file's own.
    ///
    /// It runs another program in this session, so it spends the same budget and counts
    /// against the same DEPTH an `sh -c` program does ([`Self::run_nested_program`]): a file
    /// that sources itself is bounded by them.
    fn run_dot(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some(path) = arguments.first() else {
            let _ = self.write_stderr(".: filename argument required\n", redirections);
            return self.config.statuses.failure;
        };
        let Some(contents) = self.read_path(path) else {
            let _ = self.write_stderr(
                &format!(".: {path}: No such file or directory\n"),
                redirections,
            );
            return self.config.statuses.failure;
        };
        if self.substitutions_left == 0
            || self.nesting_depth() >= self.config.bounds.substitution_depth
        {
            return self.config.statuses.could_not_run;
        }
        self.substitutions_left = self.substitutions_left.saturating_sub(1);
        self.nested_programs = self.nested_programs.saturating_add(1);
        let status = self.run_program(&contents);
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
    /// parameter expands to. A command substitution is performed by every session
    /// ([`Self::expand_substitution`]); an arithmetic expansion is resolved only by a session
    /// that runs in a volume ([`Self::expand_arithmetic`]), so for a session with no tree —
    /// and for a tilde prefix whichever session carries it, there is no home directory here —
    /// the word has no expansion and the command carrying it is not run. Nor has a word that
    /// expands past [`Bounds::expansion_bytes`], or one whose expansion would take the line
    /// past [`Bounds::line_expansion_bytes`].
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
    /// budget, the same catalogs and the same environment programs — so a substitution
    /// cannot reach anything its session cannot. It is a SUBSHELL: an `exit` inside one ends
    /// the substitution rather than the session, and the variables it set or removed are put
    /// back off the undo log it recorded ([`Self::record_for_rollback`]), which costs what it
    /// changed rather than a copy of the store.
    ///
    /// Its own cost comes out of the same budgets: performing one spends a command of
    /// [`Bounds::commands`] before anything else — so how many a line may open is bounded
    /// whatever the text they carry performs — and the commands inside it spend their own.
    ///
    /// `None` is a substitution with no value: one past [`Bounds::substitutions`],
    /// [`Bounds::substitution_depth`] or the command budget does not run, and one whose
    /// output went past [`Bounds::substitution_bytes`] has no value rather than a clipped
    /// one.
    fn expand_substitution(&mut self, program: &str) -> Option<String> {
        if self.substitutions_left == 0
            || self.commands_left == 0
            || self.nesting_depth() >= self.config.bounds.substitution_depth
        {
            return None;
        }
        self.substitutions_left = self.substitutions_left.saturating_sub(1);
        self.commands_left = self.commands_left.saturating_sub(1);
        self.captures.push(Capture {
            limit: self.config.bounds.substitution_bytes,
            ..Capture::default()
        });
        self.substitution_undo.push(BTreeMap::new());
        let held_exit = self.exit_status.take();
        let held_negated = self.negated_pipeline;
        let held_bytes = self.held_bytes;
        let status = self.run_program(program);
        self.exit_status = held_exit;
        // What the commands INSIDE the substitution were written with is no part of how the
        // command around it is read: the status this substitution answers is the word's.
        self.negated_pipeline = held_negated;
        for (name, held) in self.substitution_undo.pop().unwrap_or_default() {
            match held {
                Some(value) => self.variables.insert(name, value),
                None => self.variables.remove(&name),
            };
        }
        // The store is back to what it was, so what it holds is back to what it held.
        self.held_bytes = held_bytes;
        // The LAST substitution performed is the one a command with no command name answers
        // with, so this replaces what a substitution performed earlier in the same command
        // left.
        self.last_substitution_status = Some(status);
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
    /// ([`within_parse_bounds`], applied both to the text as written and to the text the
    /// expansion materialised), one nested past [`Bounds::word_depth`] inside another
    /// arithmetic expansion ([`Self::arithmetic_depth`]), one that does not parse, one this
    /// module does not evaluate, or one whose arithmetic has no answer (a division by zero, an
    /// overflow).
    fn expand_arithmetic(&mut self, text: &str) -> Option<String> {
        if !self.works_over_a_tree() {
            return None;
        }
        // The text AS WRITTEN is read under the bound a parse is given before anything is
        // expanded, so an expression that nests past it costs no recursion at all.
        if !within_parse_bounds(text, &self.config.bounds)
            || self.arithmetic_depth >= self.config.bounds.word_depth
        {
            return None;
        }
        // Resolving one is a level of NESTING of its own: the expression's own text is
        // expanded as a word, and an `$((…))` inside it is resolved here again — which is the
        // recursion this count bounds, since each of those words is given the word budget
        // afresh.
        self.arithmetic_depth = self.arithmetic_depth.saturating_add(1);
        let value = self.arithmetic_value(text);
        self.arithmetic_depth = self.arithmetic_depth.saturating_sub(1);
        value
    }

    /// [`Self::expand_arithmetic`] once its bounds are spent: the expression's own expansions
    /// resolved, then parsed and evaluated.
    fn arithmetic_value(&mut self, text: &str) -> Option<String> {
        let resolved = self.expand_word(text).unwrap_or_else(|| text.to_string());
        // The expansion above can have MATERIALISED text far longer and far more deeply
        // nested than the line itself — a variable's value expands inside the expression —
        // so the same bound a line is read under is applied to it as well: a parser costs a
        // level of recursion per nested group, and that recursion is what this keeps in range.
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
    /// the expansion names. A POSITIONAL parameter (`$1`, `${11}`) is the operand the program
    /// was given at that position ([`Self::with_arguments`]), and the special parameters a
    /// program reads its own operands through are the three that are about them: `$#` is how
    /// many it was given, and `$@` and `$*` are those operands joined by one space, because
    /// field splitting is a form no session performs. `$?` is resolved beside them — "the
    /// decimal exit status of the most recent pipeline", which is how a script reads whether
    /// the command before it succeeded. Every other form — a substring, a replacement, a
    /// length, an indirection, another special parameter — is one this session does not
    /// resolve.
    ///
    /// While [`ShellOption::NoUnset`] is on, the plain form of a variable this session holds
    /// NONE of is an error instead: "the shell shall write a message to standard error …
    /// and … exit", which is the diagnostic ([`PARAMETER_NOT_SET`]) and the non-zero status
    /// the program then ends on. The default-value form names what to use for an unset
    /// variable, so it is not one.
    fn expand_parameter(
        &mut self,
        parameter: &::brush_parser::word::ParameterExpr,
    ) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => match self.variable(name) {
                Some(value) => Some(value.to_string()),
                None if self.options.nounset => {
                    let name = name.clone();
                    self.refuse_unset_parameter(&name);
                    None
                }
                None => Some(String::new()),
            },
            // `$0` is the name the program was invoked under and `$1` onwards are its
            // operands, which is how a script reads what it was called with.
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Positional(position),
                indirect: false,
            } => Some(self.positional_parameter(*position)),
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Special(special),
                indirect: false,
            } => self.special_parameter(special),
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

    /// The operand at `position` of what this program was invoked with, empty for a position
    /// it was given nothing at — which is what an unset parameter expands to.
    fn positional_parameter(&self, position: u32) -> String {
        self.positional
            .get(position as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// The value of one of the SPECIAL parameters that are about the program's own operands or
    /// about what this session has done, and `None` for every other one — a parameter this
    /// session does not resolve, so the command carrying it is not run.
    ///
    /// `$#` is "the number of positional parameters", which is the operands and not the name
    /// the program was invoked under; `$@` and `$*` are those operands, joined by one space,
    /// because this session performs no field splitting; and `$?` is "the decimal exit status
    /// of the most recent pipeline" this session performed.
    fn special_parameter(
        &self,
        special: &::brush_parser::word::SpecialParameter,
    ) -> Option<String> {
        match special {
            ::brush_parser::word::SpecialParameter::PositionalParameterCount => {
                Some(self.positional.len().saturating_sub(1).to_string())
            }
            ::brush_parser::word::SpecialParameter::AllPositionalParameters { .. } => {
                Some(self.positional.iter().skip(1).cloned().collect::<Vec<_>>().join(" "))
            }
            ::brush_parser::word::SpecialParameter::ShellName => {
                Some(self.positional.first().cloned().unwrap_or_default())
            }
            ::brush_parser::word::SpecialParameter::LastExitStatus => {
                Some(self.last_status.to_string())
            }
            _ => None,
        }
    }

    /// Report the expansion of the unset variable `name` and END the program, which is what a
    /// non-interactive shell does for one while [`ShellOption::NoUnset`] is on.
    ///
    /// The diagnostic goes to standard error — where this session keeps it for the caller
    /// that reports that stream — and the status the program ends on is the failure a shell
    /// exits with for it.
    fn refuse_unset_parameter(&mut self, name: &str) {
        let _ = self.write_stderr(
            &format!("{SHELL_NAME}: {name}: {PARAMETER_NOT_SET}\n"),
            &Redirections::default(),
        );
        if self.exit_status.is_none() {
            self.exit_status = Some(self.config.statuses.failure);
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

/// What the `test` utility (and its `[ ... ]` spelling) answers for the questions that are
/// about VALUES alone — the whole of the utility that needs nothing but its arguments — or
/// `None` for a form this does not answer, which includes every question about a file.
///
/// POSIX's `test` "evaluate[s] expression" and answers it as a status, and these are the
/// primaries whose answer is a property of the words themselves: an expression of no
/// arguments is false, one of a single argument is true when that argument is not the empty
/// string, `-z`/`-n` ask whether a string is empty or not, `=`/`!=` compare two strings, and
/// `-eq`/`-ne`/`-lt`/`-le`/`-gt`/`-ge` compare two integers (`==` is the equality spelling
/// bash also takes). A primary about a FILE (`-e`, `-f`, `-d`, `-s`, `-r`, `-w`) is a
/// question about a tree, so it is the caller's to answer where it holds one and unanswerable
/// — `None` — where it holds none.
///
/// Shared because the reading is the same wherever the utility is performed: a session with
/// a tree of its own answers the file primaries itself and falls back to this
/// ([`ShellSession::test_answer`]), and a session with no tree at all has only these.
pub fn value_test_answer(arguments: &[String]) -> Option<bool> {
    match arguments {
        [] => Some(false),
        [value] => Some(!value.is_empty()),
        [operator, value] => match operator.as_str() {
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

/// `format` with `arguments` substituted, as `printf` writes it, or `None` for a format
/// this command does not perform.
///
/// The ONE reading of a `printf` format in this workspace: every session that provides the
/// utility writes it from here ([`ShellSession::run_printf`], and the container session of
/// `…ecs…::state::container_shell`, whose walk is async and whose catalog is its own), so two
/// of them cannot write one format differently.
///
/// The conversions are `%s` (the argument as it is), `%d` and `%i` (the argument read as an
/// integer, and a value that is not one is worth zero, as the utility reads it) and `%%`
/// (one per cent sign). The escapes are `\n`, `\t`, `\r` and `\\`. An argument the format
/// has no conversion for is left out, and a conversion with no argument left takes the
/// empty string, which is what the utility writes for one.
#[must_use]
pub fn printf_formatted(format: &str, arguments: &[String]) -> Option<String> {
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
mod tests;
