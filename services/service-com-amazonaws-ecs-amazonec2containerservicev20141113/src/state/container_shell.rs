// SPDX-License-Identifier: CC0-1.0

//! What a container's COMMAND does: the session one container's program runs in.
//!
//! # The rule this module implements
//!
//! A container definition's `command` is "the command that's passed to the container", and
//! `entryPoint` the "entry point that's passed to the container"; together they are the
//! argument vector the container's image runs. What the container WRITES on its standard
//! output and the STATUS it exits with are therefore produced by running that vector —
//! `Container$exitCode` is "the exit code returned from the container" — and so is the
//! container's reaction to the signal `StopTask` sends it ("the equivalent of docker stop
//! is issued ... This results in a stop signal value ... will default to SIGTERM. If the
//! container handles the SIGTERM value gracefully and exits within 30 seconds from
//! receiving it, no SIGKILL value is sent").
//!
//! The grammar is not read here: a shell program is tokenized and parsed by `brush-parser`,
//! the POSIX/bash tokenizer and grammar, exactly as this workspace reads caller-supplied
//! SQL with `sqlparser` and caller-supplied YAML with `yaml-rust2`. That parser builds a
//! syntax tree and runs nothing, so the and-or lists, pipelines, loops, conditionals,
//! assignments and quoting of a program are the shell's own rather than something scanned
//! out of the text. Word expansion is read from the same parser's word pieces.
//!
//! # Why the grammar and not an engine
//!
//! The one maintained shell ENGINE this grammar has — `brush-core`, the executing half of
//! the same project — is deliberately not a dependency of this crate, and what follows is
//! not a reimplementation of it. An engine's whole purpose is to run what the shell
//! language can express: it resolves a command name against the host's `PATH` and starts a
//! host PROCESS for it, opens the host files a redirection names, and expands a command
//! substitution by running that command. Those are exactly the primitives this module has
//! none of, and the crate exposes no seam to replace them with an in-memory volume or an
//! in-process dispatcher, so there is no configuration of it that could be confined here:
//! taking it would hand a caller's task definition the developer's machine.
//!
//! What this module does instead is narrower than a shell by construction, and it is not an
//! implementation of the shell LANGUAGE: it PERFORMS a fixed catalog of utilities
//! ([`BUILT_IN_COMMANDS`], [`FILE_COMMANDS`] and the text filters of
//! [`::service_common::posix_shell_text_filters`]) over the real grammar's tree, and every
//! other command, and every command FORM and expansion it does not perform, fails closed with
//! the status a shell answers for a command it cannot run ([`EXIT_COMMAND_NOT_FOUND`]). There
//! is therefore no language subset to keep in step with a specification: a command either
//! names one of those utilities, or it did not run.
//!
//! What decides whether a NAME belongs in that catalog is where the command's effect lands: a
//! command whose whole effect is inside this session — its output, its status, its variables,
//! its signal handlers, its own filesystem — or inside a CONTROL STRUCTURE this walk already
//! performs. That is why the catalog holds the two special built-ins that leave a LOOP
//! ([`BREAK_COMMAND`], [`CONTINUE_COMMAND`]): the loops are performed here
//! ([`ContainerShell::run_loop`], [`ContainerShell::run_for_clause`]), and a loop that cannot
//! be left is no loop a real program is written around — the shape every one-shot probe takes
//! is "try the work; on success report it and leave the loop; otherwise try again".
//!
//! The programs of the container's ENVIRONMENT this session provides beside those utilities
//! are the AWS CLI ([`super::container_aws_cli`]) — a container's image is routinely one whose
//! program calls the account's services, and those calls are made as the task role — and the
//! HTTP CLIENT a request to a server of the container's own task is made with
//! ([`super::container_http_client`]), which is what every documented container health check is
//! written with.
//!
//! A run that reached such a command CLAIMS NOTHING about the container
//! ([`ContainerOutcome::Abandoned`]): the status above is what the declining command answered
//! INSIDE the session — which is what the `&&`, `||` and loop conditions around it read — and
//! it is not reported as the container's own `exitCode`. The image a container runs holds the
//! program this session has none of, so a run that needed it produced no exit status at all,
//! exactly as a container whose launch names no argument vector produces none; reporting one
//! would be reporting an outcome for work nothing ran — and that outcome would stop the task
//! the essential container it was claimed for belongs to.
//!
//! It is the WHOLE RUN that claims nothing, not the one command: a program that recovers from
//! a command this session declined (`not-a-utility || echo fallback`) still ran none of the
//! program the image holds for that command, so the status it ends with is not an exit status
//! of the container's either. What it WROTE is delivered all the same — the lines the session
//! performed are lines the container really wrote — and only the status is withheld.
//!
//! # What this module shares, and what is its own
//!
//! The one bounded shell session this workspace drives for every service with a
//! caller-supplied script is [`::service_common::posix_shell_session`], and this module is NOT
//! a second copy of what it holds. Everything the two sessions do the same way is called here
//! rather than restated:
//!
//! * the FILES and the utilities over them — the seam a session reaches its files through,
//!   `cat`, `ls`, `mkdir`, `touch`, `rm`, `sync`, the `test` questions about a file, the
//!   reading of a redirection node and the reading of the text a here-document or here-string
//!   carries ([`::service_common::posix_shell_files`]);
//! * the TEXT FILTERS ([`::service_common::posix_shell_text_filters`]);
//! * the `test` questions about values alone
//!   ([`::service_common::posix_shell_session::value_test_answer`]);
//! * the in-memory TREE the files are held in ([`::service_common::posix_shell_volume`]),
//!   which is what a container's own layer and each mounted volume is
//!   ([`super::container_filesystem`]).
//!
//! What is this module's own is the WALK over the grammar's tree, because a container's
//! program is a PROCESS: it runs without a caller waiting on it, delivers each line as it
//! writes it, waits, loops and answers the stop signal through the handler the program
//! installed — so every step of it is `async`. The shared session is SYNCHRONOUS by
//! construction for its callers, several of which perform a whole program while holding their
//! own store's lock (`…lambdamicrovms…::state::guest_machine::in_volume`,
//! `…ec2…::state::guest_console`), and nothing may await under one of those guards. The two
//! walks are therefore one rule in two drivers, and the parts that need no await are written
//! once.
//!
//! `service-com-amazonaws-codebuild-codebuild-20161006`'s `state::build_shell` reads a
//! BUILDSPEC's command lines through that same shared session, and giving a buildspec's line
//! the container's loops, waits and signal handling would widen what a build may run — a
//! decision for a change about that service, not this one.
//!
//! # What this container provides
//!
//! It runs IN THIS PROCESS. It has no process to start, no network to reach and no host
//! environment: the utilities it provides are the ones whose whole effect is inside the
//! container's own session — its output, its status, its variables and its signal handlers —
//! and the ones whose effect is on the container's own FILESYSTEM
//! ([`super::container_filesystem`]), which is the container's writable layer and the
//! task-definition volumes mounted into it. The variables it starts with are the container
//! definition's `environment` (and a `RunTask` override of it), because those are the
//! environment the container's process is given.
//!
//! No host path is reachable from any of it: the filesystem is in memory, every entry of it
//! was put there by this emulator or by a program it performed, and a path outside the
//! container's own layer and its mounts resolves to nothing.
//!
//! # The forms that reach that filesystem
//!
//! A program written against storage writes through the shell's own forms rather than through
//! a utility, so the session performs them over that filesystem:
//!
//! * the REDIRECTIONS of the three standard descriptors — `> path`, `>> path` and `2> path`,
//!   `< path`, and the duplications `2>&1` and `1>&2` — each OPENED where the shell opens it,
//!   before the command runs, so a file the filesystem refuses (a read-only mount, a missing
//!   directory) is reported in the shell's own wording and the command does not run at all;
//! * the forms whose text is written INSIDE the command — a HERE-DOCUMENT (`<<WORD`, and
//!   `<<-WORD` which strips the leading tabs off each of its lines) and a HERE-STRING
//!   (`<<<WORD`) — which ARE the command's standard input, expanded where the delimiter
//!   asked for it and taken literally where it was quoted, so a program that composes a
//!   document and hands it to a command needs no file to put it in;
//! * a PIPELINE connects its stages: "the standard output of the first command shall be
//!   connected to the standard input of the second command", so what one stage wrote is what
//!   the next one reads;
//! * a COMMAND SUBSTITUTION (`$(…)`, `` `…` ``) is a command whose output is a word of
//!   another: the text is read by the same grammar and performed in this session as a
//!   subshell — what it set is put back, an `exit` inside it ends the substitution and not the
//!   session — and what it wrote, with trailing newlines removed, is the word.
//!
//! Standard ERROR is delivered beside standard output: the `awslogs` driver "sends container
//! logs to Amazon CloudWatch Logs", which is both of the streams a container writes, so a
//! diagnostic a command reports reaches the container's log stream exactly as its output does
//! — while a command SUBSTITUTION captures standard output alone, as the shell specifies.
//!
//! An asynchronous list (`command &`) is run in the FOREGROUND: this session has no job
//! control, so there is no second thread of execution for a job to run on, and `wait`
//! answers success because no job is outstanding.
//!
//! # What the run CONSUMES
//!
//! Performing the program IS the container's work, so this session accounts for it as it
//! goes: at each point its time divides — the step it is about to take, the wait it is about
//! to enter, and the end of the run — it records the span its program spent EXECUTING and how
//! many bytes it is holding ([`super::container_consumption`]). That accounting is what every
//! utilization series about the container's task, service or cluster is read off, and it is
//! taken here because this is the only place that knows which spans were the program RUNNING
//! rather than waiting.
//!
//! # Bounds
//!
//! Every bound is this emulator's, held in code the program cannot reach: how many STEPS
//! one run may take ([`MAX_COMMANDS_PER_RUN`], one per command the walk visits — performed
//! or declined — and one per loop iteration, latched: once spent the run is ABANDONED and no
//! status is claimed for it), how long it may run for (the deadline its session is
//! constructed with, which also clamps every wait), how deeply a line may nest
//! ([`MAX_COMMAND_NESTING`], and [`MAX_COMPOUND_DEPTH`] over the walk that performs it), how
//! much one word may expand to, how many command substitutions
//! it may perform ([`MAX_SUBSTITUTIONS`]) and how deeply they may nest
//! ([`MAX_SUBSTITUTION_DEPTH`]), how much one substitution or one stage of a pipeline may
//! carry ([`MAX_CAPTURED_BYTES`]), and how much output it may deliver
//! ([`super::MAX_CONTAINER_LOG_LINES`]). The filesystem holds its own
//! ([`super::container_filesystem`]).
//!
//! The bounds on the TEXT of ONE LINE — the program that carries it, the word that expands to
//! it, the capture that collects it, the line being assembled, and the steps a program spends
//! building it — are each stated ABOVE the longest message one log event carries
//! ([`MAX_LINE_TEXT_BYTES`]), because a line longer than that is one the `awslogs` driver
//! SPLITS into consecutive events rather than one the container may not write: a bound below
//! it would give up on a program a client legitimately runs.
//!
//! # A wait the program names is time its container is UP
//!
//! The seconds in a `sleep` are seconds the program is still there: "the `sleep` utility shall
//! suspend execution for at least the integral number of seconds specified by the `time`
//! operand", so the program has NOT ended while one of its waits is outstanding, and neither
//! has the container. This session therefore SPENDS every wait its program names, wherever
//! that wait stands in the program and whichever spelling the program is written in — the
//! argument vector the runtime exec's (`["sleep", "3600"]`), the shell program that is nothing
//! but the wait (`["sh", "-c", "sleep 3600"]`), the wait among steps of its own
//! (`sh -c 'echo ready; sleep 3600'`), the one inside a loop, and the one with no end of its
//! own (`sleep infinity`, a foreground server) alike. That is what `Task$lastStatus` reports of
//! a task while its containers are up, and it is why `Container$exitCode` — "the exit code
//! returned from the container" — is absent until the program has really ended.
//!
//! Collapsing such a wait would report the container as having EXITED while its program is
//! still waiting, which is the one thing a status, an exit code and a stop reason may not say:
//! a task whose essential container is waiting out a long-lived command would be stopped as one
//! whose essential container exited, so no client could observe the running workload it placed,
//! and a service's scheduler would spend its life replacing tasks behind a `runningCount` the
//! service never reaches.
//!
//! What a wait does NOT decide is how long this emulator takes to answer whatever is waiting to
//! see that program END — the standalone task whose terminal status a client polls for, the AWS
//! Batch job whose verdict and whose dependents wait on the task its container runs as, and the
//! next job that job's queue admits once it releases the capacity or the consumable quantity it
//! holds. The seconds are the caller's own words, so the wall clock one wait may spend on them
//! is bounded HERE, by [`wait_ceiling`], exactly as the steps a run may take and the lifetime it
//! may have are — and past that ceiling the program goes on from the wait, to its next step or
//! to its own end. The ceiling is ORDERED by the seconds the wait named rather than flat, so a
//! program written to be there for longer is there for longer, and CAPPED, so a client that
//! serialises several such containers one after another — one per unit of a scarce resource, one
//! per turn of a queue that admits one at a time — reaches every one of their verdicts inside a
//! budget of its own. A wait that names no END of its own names no length for a ceiling to
//! bound: nothing in the program follows it, so the container waits for whatever will end it
//! from outside — a stop its task is sent, a file another container writes — up to the run's own
//! deadline ([`ContainerShell::wait_for`] clamps every wait to it, and a run that reaches it is
//! ABANDONED and claims nothing about its container).
//!
//! A container whose task a SERVICE's or a DAEMON's scheduler maintains has nothing waiting for
//! that end at all ([`ContainerShell::maintained_by_a_service`]), so no ceiling is put on its
//! waits: a service "runs and maintains your desired number of tasks from a specified task
//! definition" (`CreateService`), so the end of such a container is not a verdict a caller polls
//! for — it is the task leaving the set the service holds, which the scheduler replaces.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::service_common::posix_shell_files::{
    AppliedRedirect, FileRefusal, HereInput, Redirections, ShellFiles, FILE_COMMANDS,
};

use super::*;

/// The exit status of a command that ran to success.
const EXIT_SUCCESS: i32 = 0;

/// The longest wall clock ONE wait of `stated` — a wait a container's program NAMES A LENGTH
/// for — may spend.
///
/// EMULATOR-CHOSEN — the seconds are the caller's own and AWS spends every one of them — and
/// what it is chosen against is the pair of things such a container is read by. A client that
/// polls the task it belongs to must OBSERVE it running: a task whose container is waiting is
/// `RUNNING` while it waits, and a window narrower than the interval such a client looks again
/// at would let it miss that status altogether. And whatever is waiting to see that program END
/// — the standalone task's terminal status a caller polls for, the AWS Batch job whose verdict
/// and whose dependents wait on the task its container runs as, the next job that job's queue
/// admits once it releases the capacity or the consumable quantity it holds — must reach its
/// verdict without the request's own words deciding when: the bound on that is this emulator's,
/// like every other bound in this module.
///
/// Both readings are served by a ceiling that is ORDERED by the seconds the wait named rather
/// than flat, and CAPPED:
///
/// * one span of this emulator's clock for every [`WAIT_DIVISOR`] the wait named, so a program
///   written to be there for longer is there for longer — which is what a client that places
///   such a container to hold a slot, a licence seat or a queue's turn depends on;
/// * never less than the span this emulator gives a standalone task's work
///   ([`CONTAINER_WORK_SPAN`]) — so such a task is observed `RUNNING` exactly as every other
///   one is — and never more than the wait itself, because a wait of a second costs a second;
/// * never more than [`WAIT_CEILING_WORK_SPANS`] of that span, so a wait of an hour cannot put
///   a number in a request in charge of how long every client of this emulator waits. A client
///   whose own work is SERIALISED behind a handful of such containers — one per unit of a
///   scarce resource — reaches all of their verdicts inside one budget of its own, which a
///   ceiling of tens of seconds apiece would spend on the first one or two.
///
/// It bounds the wall clock ONE wait spends, not what the program then does: past the ceiling
/// the program carries on from the wait, to its next step or to its own end, so what the
/// container reports is still its own program's outcome ([`ContainerShell::run_sleep`]). A wait
/// that names no end of its own, and every wait of a container whose task a SERVICE maintains,
/// take no ceiling at all: see the module docs for why neither has a verdict behind it.
#[must_use]
pub fn wait_ceiling(stated: Duration) -> Duration {
    let cap = CONTAINER_WORK_SPAN.saturating_mul(WAIT_CEILING_WORK_SPANS);
    stated.min((stated / WAIT_DIVISOR).clamp(CONTAINER_WORK_SPAN, cap))
}

/// How many seconds a wait must name for this emulator to spend one of its own on it
/// ([`wait_ceiling`]).
const WAIT_DIVISOR: u32 = 3;

/// How many spans of a standalone task's work the longest wait may spend ([`wait_ceiling`]).
const WAIT_CEILING_WORK_SPANS: u32 = 8;

/// The exit status of a command that ran and answered failure, which is what the `false`
/// utility is specified to do ("exit with a status code indicating failure").
const EXIT_FAILURE: i32 = 1;

/// The exit status a shell reports for a program it could not parse — the status `sh` is
/// specified to exit with on a syntax error it detects.
const EXIT_SYNTAX_ERROR: i32 = 2;

/// The exit status a shell reports when "the utility to be executed could not be found",
/// which is what this container answers for every command it provides no program for, and
/// for a command form it does not perform.
const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// What a shell adds to a signal number to report the status of a command a signal ended:
/// "the exit status shall be 128 plus the signal number".
const EXIT_SIGNAL_BASE: i32 = 128;

/// The number of the termination signal — `SIGTERM`, the signal `StopTask` has the agent
/// send a container ("will default to SIGTERM") — so a session with no handler for it ends
/// with [`EXIT_SIGNAL_BASE`] plus this.
const SIGTERM_NUMBER: i32 = 15;

/// How `trap` names the termination signal, and the spellings a program may write it as:
/// the bare name, the `SIG`-prefixed name and the number.
const SIGTERM_NAMES: [&str; 3] = ["TERM", "SIGTERM", "15"];

/// The special built-ins that leave the enclosing LOOP: `break` — "exit from within a `for`,
/// `while` or `until` loop" — and `continue`, which goes on with "the next iteration of the
/// smallest enclosing `for`, `while` or `until` loop". Each takes an optional count of the
/// enclosing loops it leaves ("the n-th enclosing loop").
///
/// They are commands of the LOOPS this session performs ([`ContainerShell::run_loop`],
/// [`ContainerShell::run_for_clause`]), which is why they are performed by the walk that holds
/// those loops rather than by a utility: a program written as a retry loop leaves it with one
/// of these, so a session that performs the loop and not the command performs no such program
/// at all. The shared session's own walk performs them under the same rule
/// ([`::service_common::posix_shell_session`]), as it does the loops, the conditionals and
/// `set -e` around them.
const BREAK_COMMAND: &str = "break";
const CONTINUE_COMMAND: &str = "continue";

/// The utility that WRITES a format with its arguments substituted: "the `printf` utility shall
/// write formatted operands to the standard output". Its whole effect is the container's own
/// output, exactly as `echo`'s is, and what one format WRITES is this workspace's one reading of
/// a `printf` format ([`::service_common::posix_shell_session::printf_formatted`]) rather than a
/// second one — a program that reports a value writes it with either utility.
const PRINTF_COMMAND: &str = "printf";

/// The utility that WAITS: "the `sleep` utility shall suspend execution for at least the
/// integral number of seconds specified by the `time` operand" — the seconds a container's
/// program is still there for ([`ContainerShell::run_sleep`]). Named here so the dispatch and
/// the catalog name the one utility.
const SLEEP_COMMAND: &str = "sleep";

/// The utility that REPLACES the session with the command it names: "the `exec` utility shall
/// open, close, and/or copy file descriptors as specified by any redirections as part of the
/// command. If `exec` is specified with command, it shall replace the shell with command without
/// creating a new process."
///
/// So a command written with it runs, and what the program wrote after it is not run at all: the
/// session was that command. A container's own command uses it exactly for that — handing the
/// container's one process to the server its image ships.
const EXEC_COMMAND: &str = "exec";

/// The commands this container provides, as their names.
///
/// Each one's whole effect is inside the container's own session, which is why it can be
/// performed in process: `echo` and `printf` write to the container's output, `true`, `false`
/// and `:` answer a status, `exit` ends the session, `export`/`unset` change its variables,
/// `trap`
/// records what a signal does to it, `sleep` waits, `wait` answers for the jobs it has none
/// of, `exec` runs the command it names IN PLACE of the session ("the shell shall replace
/// itself with the command"), which is how a container's command hands itself to the server its
/// image ships, `hostname` writes the name the runtime gave this container, and `test` — with
/// its `[ ... ]` spelling — answers a question about the words it
/// was given or about the container's own filesystem ([`ContainerShell::run_test`]). `pwd`,
/// `id`, `ulimit` and `df` REPORT the process this session is — where it runs, who it runs
/// as, the limits it runs under and the space on the filesystem it writes on — off the
/// settings its launch resolved ([`PROCESS_REPORT_COMMANDS`]). `date` writes the instant it ran
/// at, in the form the invocation asked for, off this emulator's own clock
/// ([`CLOCK_COMMAND`]), which is how a program whose contract is a measured span states the
/// window its work happened in. `break` and `continue` leave the
/// LOOP they stand in ([`BREAK_COMMAND`], [`CONTINUE_COMMAND`]), which is how a program written
/// as a retry loop ends it.
/// The commands that work over that filesystem are [`FILE_COMMANDS`]; a command outside either
/// catalog and outside the text filters and the SERVERS the container's image ships
/// ([`super::container_servers`]) needs a program or a network this container does not
/// have.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "echo", PRINTF_COMMAND, "true", "false", ":", "exit", "export", "unset", "trap",
    SLEEP_COMMAND,
    "wait", "test", "[", "set", "exec", "hostname", "pwd", "id", "ulimit", "df", CLOCK_COMMAND,
    BREAK_COMMAND,
    CONTINUE_COMMAND, "stty", "tput",
];

/// The command of [`BUILT_IN_COMMANDS`] that reads the CLOCK: "write the date and time".
///
/// Its whole effect is inside this session — one line on standard output and a status — so it is
/// performed in process, off the same clock every other instant of this emulator is read off. WHAT
/// an invocation asks for and what it writes is this workspace's one reading of that program
/// ([`::service_common::posix_shell_clock`]), shared with every other session that performs
/// caller-supplied shell text, so a line a task definition wrote for it reads the same instant in
/// the same form wherever it runs.
const CLOCK_COMMAND: &str = ::service_common::posix_shell_clock::CLOCK_PROGRAM;

/// The commands of [`BUILT_IN_COMMANDS`] that report the TERMINAL this session is attached to
/// — the one an interactive session of an execute-command call is carried over, whose size the
/// client states on the channel ([`ContainerShell::resize_terminal`]).
///
/// `stty` reports "the terminal line settings" of the session's own terminal and `tput`
/// answers "the value of the terminal-dependent capability" asked for, so what each of them
/// writes is a fact about THIS session; a session attached to no terminal — a container's own
/// program, the agent's health-check probe — performs neither.
const TERMINAL_REPORT_COMMANDS: [&str; 2] = ["stty", "tput"];

/// The commands of [`BUILT_IN_COMMANDS`] that REPORT a fact about the process this session
/// is, rather than doing anything to it: where it runs (`pwd`), who it runs as (`id`), the
/// resource limits it runs under (`ulimit`) and the space on the filesystem it writes on
/// (`df`). Each is answered off the settings the container's launch resolved
/// ([`super::container_process`]), and a question about a setting this emulator holds no
/// value for is a command that did not run.
const PROCESS_REPORT_COMMANDS: [&str; 4] = ["pwd", "id", "ulimit", "df"];

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes".
const PERSISTING_COMMANDS: &[&str] = &[
    ":",
    "exit",
    "export",
    "unset",
    "trap",
    BREAK_COMMAND,
    CONTINUE_COMMAND,
];

/// The shells this session reads a `-c` PROGRAM from, by the name they are invoked under.
///
/// A container's argument vector is exec'd, not interpreted, so a vector whose first word
/// is one of these with a `-c` carries a shell program in the word after it — the form a
/// task definition writes a script in — while every other vector names a utility and its
/// arguments.
const SHELL_COMMAND_NAMES: [&str; 4] = ["sh", "bash", "dash", "ash"];

/// How many STEPS one container run may take — one per command the walk visits and one per
/// loop iteration. THIS NUMBER IS THE EMULATOR'S, not a quota of the service: the run
/// happens in this process, so without a ceiling a program written as an endless loop of
/// instant commands would hold a thread of this emulator for as long as the process lives.
/// Once it is spent the run is ABANDONED — no status is claimed for the container — rather
/// than reported as having ended.
///
/// Its size is a number of COMMANDS in its own right, and it is what a program that
/// assembles one line of output a character at a time costs: such a program spends a handful
/// of commands per character — the write, the counter, the condition and the iteration — and
/// the longest line the `awslogs` driver delivers in ONE event is 262118 characters, so a
/// budget of a couple of million steps is what lets a program writing a line that driver has
/// to SPLIT run to its end instead of being given up on part of the way through it.
pub const MAX_COMMANDS_PER_RUN: usize = 2_100_000;

/// How many steps this session takes between handing the runtime back. THIS NUMBER IS THE
/// EMULATOR'S: a program whose commands never wait would otherwise hold the executor thread
/// it runs on for its whole budget.
const COMMANDS_PER_YIELD: usize = 64;

/// How many bytes of TEXT one line of a container's output may take up anywhere in this
/// session. THIS NUMBER IS THE EMULATOR'S, and it is stated as a multiple of the longest
/// message one log event carries ([`super::MAX_LOG_EVENT_MESSAGE_BYTES`]): the `awslogs`
/// driver SPLITS a line longer than one event into consecutive events, so a line past that
/// length is one a container legitimately writes, and every bound below — the program's text,
/// one word's expansion, a capture, the line being assembled — is set above it so such a
/// program RUNS instead of being given up on. FOUR times it, so a program has room to BUILD
/// such a line (a value doubled until it is long enough ends up twice the length it passed)
/// rather than only to write one.
const MAX_LINE_TEXT_BYTES: usize = 4 * super::MAX_LOG_EVENT_MESSAGE_BYTES;

/// The longest program text this session reads. THIS NUMBER IS THE EMULATOR'S, for the same
/// reason as [`MAX_COMMANDS_PER_RUN`]: it bounds what one parse can cost. Its size is
/// [`MAX_LINE_TEXT_BYTES`]: a program that writes a line the log driver has to split can
/// carry that line as a word of its own, so a smaller bound would refuse the program for the
/// length of what it writes.
const MAX_PROGRAM_BYTES: usize = MAX_LINE_TEXT_BYTES;

/// How many grouping indicators one program may carry before this session gives up on
/// reading it. THIS NUMBER IS THE EMULATOR'S, and it is what keeps the grammar's own
/// recursion in range: a parser descends once per nested group, so a program nested
/// arbitrarily deep would exhaust the stack of the thread running it. It is checked WITHOUT
/// parsing.
const MAX_COMMAND_NESTING: usize = 64;

/// How deeply this session's own walk over the grammar's COMPOUND forms may nest before it
/// gives up on the form. THIS NUMBER IS THE EMULATOR'S, under [`MAX_COMMAND_NESTING`]'s rule
/// applied to the walk rather than to the text: the walk is mutually recursive — a subshell, a
/// brace group or a loop body re-enters it — so every level costs stack frames of the thread
/// running it, and a program nested without end would exhaust that stack. It is checked at the
/// ONE place the walk re-enters a compound form ([`ContainerShell::run_compound_command`]), so
/// it holds however that form was reached — including a nested program a command substitution
/// materialised, whose own text [`within_parse_bounds`] counts separately.
///
/// Set just ABOVE [`MAX_COMMAND_NESTING`]: the text bound already refuses a program carrying
/// more grouping indicators than that, and a program may nest no deeper than it carries
/// indicators, so no program this session admits can reach this limit — nothing a container may
/// legitimately run is refused by it, and a walk that reaches it however it was entered ends as
/// a form this container does not perform rather than past the thread's stack.
const MAX_COMPOUND_DEPTH: usize = MAX_COMMAND_NESTING + 1;

/// The characters that open a nested group in a program, counted by
/// [`within_parse_bounds`]: a subshell or command substitution, a brace group or expansion,
/// a bracket expression, and the older command-substitution spelling.
const NESTING_INDICATORS: [char; 4] = ['(', '{', '[', '`'];

/// The reserved WORDS that open or close a nested command, counted by
/// [`within_parse_bounds`] beside [`NESTING_INDICATORS`]: the shell's compound commands
/// nest through these rather than through any character. The words that CLOSE one are
/// counted too, which can only make the count higher — an over-count refuses a program and
/// never admits one.
const NESTING_WORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "while", "until", "for", "select", "case", "esac", "in",
    "do", "done", "function", "time",
];

/// The characters a reserved word may be glued to: the shell's own separators and
/// operators. A word is counted by [`within_parse_bounds`] when it stands between any of
/// these (or whitespace), so `:;if` carries an `if` exactly as `: ; if` does.
const WORD_SEPARATORS: [char; 10] = [';', '&', '|', '(', ')', '{', '}', '<', '>', '!'];

/// How many bytes one expansion may produce before this session gives up on the word. THIS
/// NUMBER IS THE EMULATOR'S: a parameter expansion copies a value this session holds, so a
/// word that expands several of them is how one command could grow what the next one copies
/// again. Its size is [`MAX_LINE_TEXT_BYTES`]: a program that writes a line the log driver
/// has to split holds that line in a variable and expands it to write it.
const MAX_EXPANSION_BYTES: usize = MAX_LINE_TEXT_BYTES;

/// How many bytes of variables one run's session may hold at once. THIS NUMBER IS THE
/// EMULATOR'S, under [`MAX_EXPANSION_BYTES`]' rule applied to the session rather than to one
/// word: past it a command that would set another variable is not performed.
const MAX_SESSION_BYTES: usize = 2 * MAX_LINE_TEXT_BYTES;

/// How many variables one run's session may hold at once. THIS NUMBER IS THE EMULATOR'S:
/// [`MAX_SESSION_BYTES`] bounds the store's SIZE and this bounds its cardinality.
const MAX_SESSION_VARIABLES: usize = 4096;

/// How many bytes ONE command's expansions may materialise at once. THIS NUMBER IS THE
/// EMULATOR'S: [`MAX_EXPANSION_BYTES`] bounds one word and this bounds their sum.
const MAX_LINE_EXPANSION_BYTES: usize = 2 * MAX_EXPANSION_BYTES;

/// How deeply a word's expansion is walked before this session gives up on it. THIS NUMBER
/// IS THE EMULATOR'S, under [`MAX_COMMAND_NESTING`]'s rule, applied to the pieces a quoted
/// word nests.
const MAX_WORD_DEPTH: usize = 32;

/// How many signal handlers one session may hold. THIS NUMBER IS THE EMULATOR'S: a `trap`
/// records text, so without a ceiling a program could grow this session through it. Far
/// above the number of signals there are.
const MAX_TRAPS: usize = 64;

/// How many bytes of output this session holds before the line it is assembling is
/// delivered unterminated. THIS NUMBER IS THE EMULATOR'S: a program that writes without
/// ever ending a line would otherwise grow this buffer without end. Its size is
/// [`MAX_LINE_TEXT_BYTES`], so a line the program really ended is delivered AS one line and
/// the SPLIT into events is the log driver's own
/// ([`super::MAX_LOG_EVENT_MESSAGE_BYTES`]) rather than this buffer's.
const MAX_PENDING_LINE_BYTES: usize = MAX_LINE_TEXT_BYTES;

/// A future this session's own recursion goes through, so the mutually recursive walk over
/// the grammar's tree has a size the compiler can work out.
type ShellFuture<'a, T> = Pin<Box<dyn ::std::future::Future<Output = T> + Send + 'a>>;

/// What a `break` or a `continue` asked for: which of the two it was, and how many enclosing
/// loops it leaves ([`BREAK_COMMAND`], [`CONTINUE_COMMAND`]).
#[derive(Debug, Clone, Copy)]
struct LoopControl {
    /// Whether the loop being left goes on to its next iteration (`continue`) or ends
    /// (`break`).
    goes_on: bool,
    /// How many enclosing loops this leaves — "the n-th enclosing loop", at least one.
    levels: usize,
}

/// How many bytes ONE command substitution, or ONE stage of a pipeline, may carry. THIS
/// NUMBER IS THE EMULATOR'S, under [`MAX_EXPANSION_BYTES`]' rule applied to what a nested
/// command produced: past it the substitution has no value at all rather than a clipped one,
/// and the pipeline carrying it ends rather than leaving the stage after it to answer for a
/// stream nothing produced.
const MAX_CAPTURED_BYTES: usize = MAX_LINE_TEXT_BYTES;

/// How many bytes one REDIRECTED command's output is collected for the file it names. THIS
/// NUMBER IS THE EMULATOR'S, under [`MAX_CAPTURED_BYTES`]' rule applied to a redirection;
/// what the filesystem itself will hold is its own bound
/// ([`super::container_filesystem::CONTAINER_LAYER_LIMITS`]).
const MAX_REDIRECTED_BYTES: usize = MAX_LINE_TEXT_BYTES;

/// How many command substitutions one run performs. THIS NUMBER IS THE EMULATOR'S: each one
/// performs a program of its own, so this is what bounds the work a program can reach by
/// writing one inside another. Each also spends a step of [`MAX_COMMANDS_PER_RUN`].
pub const MAX_SUBSTITUTIONS: usize = 4096;

/// How deeply command substitutions may nest inside one another. THIS NUMBER IS THE
/// EMULATOR'S, under [`MAX_COMMAND_NESTING`]'s rule: this walk recurses once per open
/// substitution, and that recursion is what this keeps in range.
pub const MAX_SUBSTITUTION_DEPTH: usize = 8;

/// The name a shell reports its own diagnostics under — a redirection it could not open
/// among them — which is the name the container's program was invoked as.
const SHELL_NAME: &str = "sh";

/// What one command SUBSTITUTION, one stage of a pipeline, or one redirected command has
/// captured so far.
#[derive(Debug, Default)]
struct Capture {
    /// The bytes the commands inside it wrote to standard output.
    text: String,
    /// How many of those bytes it may hold ([`MAX_CAPTURED_BYTES`],
    /// [`MAX_REDIRECTED_BYTES`]).
    limit: usize,
    /// Set once it went past that limit, in which case the write did not happen at all and
    /// the capture has no value rather than a clipped one.
    overflowed: bool,
}

/// What one container is given to run: the argument vector its image was handed, read as
/// the program it is.
#[derive(Debug, Clone)]
pub enum ContainerProgram {
    /// A shell program: the vector named a shell with `-c`, and this is the text after it,
    /// beside the POSITIONAL PARAMETERS the words after that text give it.
    Script {
        /// The program text the `-c` names.
        text: String,
        /// The positional parameters the program runs with: `arguments[0]` is the name it
        /// was invoked under (`$0`) and the words after it are `$1`, `$2` and so on
        /// ([`script_positional_parameters`]).
        arguments: Vec<String>,
    },
    /// A utility and its arguments: the vector is exec'd as written, with no shell to
    /// expand or split anything.
    Argv(Vec<String>),
}

/// The POSITIONAL PARAMETERS a `sh -c PROGRAM [NAME [ARGUMENT…]]` invocation gives its
/// program: `NAME` is the name the program is invoked under — what `$0` expands to — and the
/// arguments after it are `$1`, `$2` and so on.
///
/// POSIX, `sh`: with `-c`, "if there are no operands after command_string, the value of the
/// special parameter 0 shall be set to … the name that the shell was invoked by", and
/// otherwise "command_name … shall be assigned to special parameter 0" with the remaining
/// operands "assigned to the positional parameters" from `1` on. So an invocation naming no
/// operands leaves the shell's own name as `$0` ([`SHELL_NAME`]).
fn script_positional_parameters(operands: &[String]) -> Vec<String> {
    if operands.is_empty() {
        return vec![SHELL_NAME.to_string()];
    }
    operands.to_vec()
}

impl ContainerProgram {
    /// The shell program `text`, invoked with no operands: `$0` is the shell's own name and
    /// the program has no positional arguments — what a caller that names a program and
    /// nothing else hands it ([`script_positional_parameters`]).
    #[must_use]
    pub fn shell_script(text: String) -> Self {
        Self::Script {
            text,
            arguments: script_positional_parameters(&[]),
        }
    }

    /// The program the argument vector `entry_point` followed by `command` runs, or `None`
    /// when the launch named no vector at all — a container whose image's own entry point
    /// is what would run, which this emulator does not hold and therefore does not invent.
    /// The command an IMAGE of this emulator's own catalog declares is resolved by the caller
    /// and handed in as `command`
    /// (`super::container_servers::image_declared_command`), so what arrives here is always a
    /// vector something named.
    ///
    /// `entryPoint` comes first because that is the order the container runtime exec's
    /// them: "the entry point ... command is passed as the arguments to the entry point".
    ///
    /// A `-c` is the shell's OPTION only while the shell is still reading options: a vector
    /// is scanned from the shell's name while its words are options, so a `-c` standing
    /// after an operand is an argument of whatever that vector runs rather than this
    /// session's program selector — the same place the shell itself stops reading options.
    pub fn resolve(entry_point: Option<&[String]>, command: Option<&[String]>) -> Option<Self> {
        let mut argv: Vec<String> = Vec::new();
        argv.extend(entry_point.unwrap_or_default().iter().cloned());
        argv.extend(command.unwrap_or_default().iter().cloned());
        let first = argv.first()?;
        let utility = first.rsplit('/').next().unwrap_or(first);
        if SHELL_COMMAND_NAMES.contains(&utility) {
            // `sh [option...] -c <program> [name [argument...]]`: the word after `-c` is
            // the program, and the words after it are its POSITIONAL PARAMETERS — the name
            // it runs under and the arguments after that name
            // ([`script_positional_parameters`]).
            for (index, word) in argv.iter().enumerate().skip(1) {
                if !word.starts_with('-') || word == "--" {
                    break;
                }
                if word == "-c" {
                    if let Some(text) = argv.get(index + 1) {
                        let operands = argv.get(index.saturating_add(2)..).unwrap_or_default();
                        return Some(Self::Script {
                            text: text.clone(),
                            arguments: script_positional_parameters(operands),
                        });
                    }
                    // A `-c` with nothing after it names no program: the vector is left as
                    // the one the image exec's, which this container has no program for,
                    // rather than read as a launch that named no command at all.
                    break;
                }
            }
        }
        Some(Self::Argv(argv))
    }
}

/// The utility whose FOLLOW option asks for a program that does not end
/// ([`follow_free_arguments`]).
const TAIL_COMMAND: &str = "tail";

/// How `tail`'s follow option is written: the flag, the long name, and the long name carrying
/// the descriptor form it may be given (`--follow=name`, `--follow=descriptor`).
const FOLLOW_FLAG: &str = "-f";
const FOLLOW_LONG: &str = "--follow";
const FOLLOW_LONG_VALUE_PREFIX: &str = "--follow=";

/// `arguments` with `tail`'s FOLLOW option taken out, for an invocation that carries one, and
/// `None` for every other invocation — including one of another utility, since no other
/// utility these containers perform reads `-f` as a follow
/// ([`::service_common::posix_shell_text_filters::TEXT_FILTER_COMMANDS`]: `cut -f` names its
/// field list and `grep -f` is not performed at all).
///
/// What is left is the invocation the text filters compute over, so a follow writes exactly what
/// the same invocation without it writes — the last lines of its input, as many as its own count
/// asks for — and the FOLLOW itself is the one thing this session performs rather than that
/// module: the utility "shall not terminate after the last line of the input file has been
/// copied" (POSIX, `tail`), which is a program that goes on rather than text to compute.
fn follow_free_arguments(name: &str, arguments: &[String]) -> Option<Vec<String>> {
    if name != TAIL_COMMAND {
        return None;
    }
    let follows = |argument: &String| {
        argument == FOLLOW_FLAG
            || argument == FOLLOW_LONG
            || argument.starts_with(FOLLOW_LONG_VALUE_PREFIX)
    };
    if !arguments.iter().any(follows) {
        return None;
    }
    Some(
        arguments
            .iter()
            .filter(|argument| !follows(argument))
            .cloned()
            .collect(),
    )
}

/// What a container's run ended as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerOutcome {
    /// The program ended on its own, with this status — the container's `exitCode`.
    Exited(i32),
    /// The program did not end with a status of its own: a bound of this emulator was spent
    /// (its command budget or its deadline), or the program needed a command, command form
    /// or expansion this session does not perform ([`ContainerShell::declined`]). NOTHING is
    /// claimed about the container: it reports no exit code, because none was produced.
    Abandoned,
}

/// How many bytes of the command name a detail NAMES, past which the name is clipped.
///
/// The name is a word of the program a CALLER declared, so its length is a caller's; the
/// detail it goes into is a member with a documented ceiling of its own — `Container$reason`
/// is "a short (1024 max characters) human-readable string", and the `reason` AWS Batch
/// reports for the attempt of a job running as the task is "a short (255 max characters)"
/// one — so the sentence stays well inside both however long a word the request carried.
const MAX_DETAIL_PROGRAM_NAME_BYTES: usize = 64;

/// The detail a container reports for a run that needed a program named `name` this emulator
/// has none of: the program was never started, so the container produced no outcome at all.
///
/// One variable part, the command name the run reached, taken from the program the container
/// declared and clipped to [`MAX_DETAIL_PROGRAM_NAME_BYTES`] at a character boundary. Fixed
/// text otherwise: no status, exit code or duration is claimed with it.
#[must_use]
pub fn program_not_performed_detail(name: &str) -> String {
    let mut clipped = MAX_DETAIL_PROGRAM_NAME_BYTES.min(name.len());
    while clipped > 0 && !name.is_char_boundary(clipped) {
        clipped -= 1;
    }
    let name = &name[..clipped];
    format!(
        "This emulator performs a container's declared program itself and provides no program \
         named \"{name}\", so the program was never started and the container produced no \
         outcome."
    )
}

/// The detail a container reports for a run that needed a command FORM or an expansion this
/// emulator performs none of, where there is no one command name to name.
pub const PROGRAM_FORM_NOT_PERFORMED_DETAIL: &str =
    "This emulator performs a container's declared program itself and performed none of the \
     program this container declared, so the container produced no outcome.";

/// The detail a container whose launch named NO argument vector reports: the program that
/// would run is the one inside the image, which this emulator starts none of.
pub const NO_DECLARED_PROGRAM_DETAIL: &str =
    "This emulator starts no container image, so the program the image declares as its own \
     entry point was never started and the container produced no outcome.";

/// The detail a container reports for a run a bound of this emulator ended before the program
/// finished ([`MAX_COMMANDS_PER_RUN`], and the lifetime one run is given).
pub const RUN_ENDED_BY_A_BOUND_DETAIL: &str =
    "The run of this container's program was ended by a bound of this emulator before the \
     program finished, so the container produced no outcome.";

/// Whether `text` is inside the bounds the parse below is given: its size, and how many
/// nested commands it opens — through a grouping character or a reserved word.
fn within_parse_bounds(text: &str) -> bool {
    if text.len() > MAX_PROGRAM_BYTES {
        return false;
    }
    let grouped = text
        .chars()
        .filter(|character| NESTING_INDICATORS.contains(character))
        .count();
    let worded = text
        .split(|character: char| {
            character.is_whitespace() || WORD_SEPARATORS.contains(&character)
        })
        .filter(|word| NESTING_WORDS.contains(word))
        .count();
    grouped.saturating_add(worded) <= MAX_COMMAND_NESTING
}

/// The size of the TERMINAL a session is attached to, in the order `stty size` reports the
/// two: an interactive session of an execute-command call runs in a pseudo-terminal, and its
/// client states that terminal's size on the channel.
#[derive(Debug, Clone, Copy)]
struct TerminalSize {
    /// How many rows it has.
    rows: u32,
    /// How many columns it has.
    columns: u32,
}

/// The variable a shell holds its terminal's WIDTH in, which a program that formats its
/// output reads.
const TERMINAL_COLUMNS_VARIABLE: &str = "COLUMNS";

/// The variable it holds its terminal's HEIGHT in.
const TERMINAL_LINES_VARIABLE: &str = "LINES";

/// The session ONE container's program runs in: the variables it sees and changes, the
/// signal handlers it has installed, where its output goes, and how much running it has
/// left.
#[derive(Debug)]
pub struct ContainerShell {
    /// The variables this session holds — the container's environment — which is what a
    /// word's parameter expansion resolves against.
    variables: BTreeMap<String, String>,
    /// The POSITIONAL PARAMETERS the program this session performs was given: index 0 is the
    /// name it was invoked under (`$0`) and the ones after it are `$1`, `$2` and so on
    /// ([`script_positional_parameters`]). Empty for a session performing no program of its
    /// own yet, and then `$0` is the shell's own name and every other one expands to nothing,
    /// the way an unset parameter does.
    positional: Vec<String>,
    /// How many bytes those variables' names and values come to, kept as they are written
    /// rather than recomputed, so the [`MAX_SESSION_BYTES`] check costs one addition.
    held_bytes: usize,
    /// What a signal does to this session: the `trap` action recorded for each signal, by
    /// the name [`signal_key`] normalises it to. An empty action is a signal the program
    /// asked to IGNORE.
    traps: BTreeMap<String, String>,
    /// How many more bytes the command being run may expand to
    /// ([`MAX_LINE_EXPANSION_BYTES`]), reset for each command.
    expansion_left: usize,
    /// How many more commands this run may perform ([`MAX_COMMANDS_PER_RUN`]).
    commands_left: usize,
    /// How many commands have been performed since the runtime was last handed back
    /// ([`COMMANDS_PER_YIELD`]).
    commands_since_yield: usize,
    /// The status `exit` ended this session with, once a command asked for one — or the
    /// status a signal ended it with: every later command is then not run at all, the way a
    /// shell stops at `exit`.
    exit_status: Option<i32>,
    /// Whether a bound of this emulator ended the run before the program did, which is what
    /// makes the outcome [`ContainerOutcome::Abandoned`].
    abandoned: bool,
    /// Whether this run reached a command, command FORM or expansion this session does not
    /// perform ([`Self::declined`]) — which makes the outcome
    /// [`ContainerOutcome::Abandoned`] for the reason the module docs give.
    unperformed: bool,
    /// The NAME of the first command this session had no program for, when the thing it did
    /// not perform was a command name rather than a form — what the detail a container
    /// reports for such a run names ([`Self::nothing_performed_detail`]).
    declined_command: Option<String>,
    /// Whether the termination signal has arrived and its handler has not run yet.
    pending_term: bool,
    /// Whether the termination handler is running, so the checkpoint inside it does not
    /// deliver the same signal to itself again.
    delivering_term: bool,
    /// The output the container writes to, and the `awslogs` delivery behind it.
    output: Arc<ContainerOutput>,
    /// The line being assembled: output is delivered one LINE at a time, so a write that
    /// does not end in a newline waits here for the one that does
    /// ([`MAX_PENDING_LINE_BYTES`]).
    pending_line: String,
    /// The signals this container can be sent — the flag `StopTask` raises.
    signals: Arc<ContainerSignals>,
    /// When this emulator gives up on the run.
    deadline: Instant,
    /// The FILESYSTEM this container's program reads and writes: its own writable layer and
    /// the volumes the task definition mounts into it
    /// ([`super::container_filesystem`]).
    filesystem: ContainerFilesystem,
    /// The captures that are open, innermost last: while one is, what a command writes to
    /// standard output is ITS bytes rather than the container's output — a command
    /// substitution's value, one stage of a pipeline, or the file a redirection names.
    captures: Vec<Capture>,
    /// For each command SUBSTITUTION being performed, innermost last, what this session's
    /// variables have to be put back to when it ends: the value each name the substituted
    /// commands touched held before they did. A substitution is a subshell, and this is what
    /// rolls one back — which costs what it changed rather than a copy of the store.
    substitution_undo: Vec<BTreeMap<String, Option<String>>>,
    /// The status of the last command substitution performed for the simple command being
    /// run, which is the status of a command that has no command name at all.
    last_substitution_status: Option<i32>,
    /// How many more command substitutions this run performs ([`MAX_SUBSTITUTIONS`]).
    substitutions_left: usize,
    /// What the stage BEFORE the command being performed wrote, which is that command's
    /// standard input — `None` for a command that is not reading one stage of a pipeline.
    piped_input: Option<String>,
    /// Set when a write to the file a redirection named did not happen, which is what makes
    /// the command carrying it answer as one that failed rather than as one that wrote.
    redirect_failed: bool,
    /// Whether the program asked to be ENDED by the first command that fails (`set -e`).
    errexit: bool,
    /// How many CONDITIONS are being evaluated right now — an `if`'s, a loop's — inside which
    /// a command that fails is the condition's answer rather than a failure that ends the
    /// program under `set -e`.
    condition_depth: usize,
    /// How deeply this session's walk over the grammar's COMPOUND forms is nested right now,
    /// which is what [`MAX_COMPOUND_DEPTH`] bounds.
    compound_depth: usize,
    /// How many LOOPS this walk is performing inside one another right now, which is what
    /// decides whether a `break` or a `continue` has a loop to leave at all
    /// ([`Self::run_loop_control`]).
    loop_depth: usize,
    /// The loop the program asked to leave and how many enclosing loops it is leaving —
    /// `None` while no `break` or `continue` is in flight. Set by those commands and taken by
    /// the loop that honours it ([`Self::taken_loop_control`]).
    loop_control: Option<LoopControl>,
    /// Whether the pipeline just performed was written with `!`, which is the other form
    /// `set -e` does not end the program for.
    negated_pipeline: bool,
    /// WHERE a server this container's program starts is reached
    /// ([`super::container_servers`]) — the address of the interface the task was allocated,
    /// and the records the server is published in — and `None` for a container whose task this
    /// emulator holds no address for, whose server nothing could reach.
    serving: Option<ContainerServing>,
    /// What keeps the servers this container's program started LISTENING: dropping a handle is
    /// that server stopping, so a session that ends — or a run this emulator gives up on and
    /// drops — leaves nothing listening at the task's address.
    listening: Vec<::service_common::served_http_addresses::ServedAddressHandle>,
    /// Whether the program asked to be ENDED by an expansion of a parameter this session
    /// holds nothing for (`set -u`).
    nounset: bool,
    /// The parameter an expansion found unset while [`Self::nounset`] was on, which is what
    /// ends the session rather than leaving the command one it could not run
    /// ([`Self::word_failure`]).
    unset_parameter: Option<String>,
    /// Whether a pipeline's status is its LAST FAILING stage's rather than its last stage's
    /// (`set -o pipefail`).
    pipefail: bool,
    /// The status of the last pipeline this session performed, which is what the special
    /// parameter `$?` expands to.
    last_status: i32,
    /// The AWS CLI this container's environment provides, and `None` for a session given
    /// none — one whose commands reach no account service at all, so an `aws` command is one
    /// it could not run ([`super::container_aws_cli`]).
    aws: Option<Arc<ContainerAwsCli>>,
    /// The HTTP CLIENT this container's environment provides, and `None` for a session given
    /// none — one whose task this emulator holds no address for, which reaches no server of
    /// its own, so such a request is a command it could not run
    /// ([`super::container_http_client`]).
    http: Option<Arc<ContainerHttpClient>>,
    /// What the PROCESS this session is runs as — the user and the resource limits its
    /// container definition gives it, and the ephemeral storage its task was given — which
    /// is what the utilities that report a process's own session answer
    /// ([`super::container_process`]).
    process: ContainerProcess,
    /// Where what this session CONSUMES is accounted for: how long its program has spent
    /// executing, and how many bytes it is holding right now
    /// ([`super::container_consumption`]). A session built without one accounts into a
    /// reading of its own, which nothing samples.
    consumption: Arc<ContainerConsumption>,
    /// The instant that accounting was last brought up to date — what the next span this
    /// session spends executing is measured from.
    executing_since: Instant,
    /// The TERMINAL this session is attached to, and `None` for one attached to none: a
    /// container's own program and the agent's probes run with no terminal, while an
    /// interactive session of an execute-command call runs in the pseudo-terminal its client
    /// states the size of ([`ContainerShell::resize_terminal`]).
    terminal: Option<TerminalSize>,
    /// Whether the container whose program this session performs belongs to a task a SERVICE's
    /// or a DAEMON's scheduler maintains, so nothing is waiting for that program to END and no
    /// ceiling is put on the waits it names ([`Self::wait_ceiling_for`]).
    maintained_by_a_service: bool,
}

impl ContainerShell {
    /// A session holding `variables` — the container's environment — writing to `output`,
    /// answering the signals raised on `signals`, given until `deadline`, reading and
    /// writing `filesystem` — the container's own layer and the volumes mounted into it — and
    /// publishing the servers its program starts through `serving`
    /// ([`super::container_servers`]).
    pub fn new(
        variables: BTreeMap<String, String>,
        output: Arc<ContainerOutput>,
        signals: Arc<ContainerSignals>,
        deadline: Instant,
        filesystem: ContainerFilesystem,
        serving: Option<ContainerServing>,
    ) -> Self {
        let held_bytes = variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum();
        Self {
            variables,
            positional: Vec::new(),
            held_bytes,
            traps: BTreeMap::new(),
            expansion_left: MAX_LINE_EXPANSION_BYTES,
            commands_left: MAX_COMMANDS_PER_RUN,
            commands_since_yield: 0,
            exit_status: None,
            abandoned: false,
            unperformed: false,
            declined_command: None,
            pending_term: false,
            delivering_term: false,
            output,
            pending_line: String::new(),
            signals,
            deadline,
            filesystem,
            captures: Vec::new(),
            substitution_undo: Vec::new(),
            last_substitution_status: None,
            substitutions_left: MAX_SUBSTITUTIONS,
            piped_input: None,
            redirect_failed: false,
            errexit: false,
            condition_depth: 0,
            compound_depth: 0,
            loop_depth: 0,
            loop_control: None,
            negated_pipeline: false,
            serving,
            listening: Vec::new(),
            nounset: false,
            unset_parameter: None,
            pipefail: false,
            last_status: EXIT_SUCCESS,
            aws: None,
            http: None,
            process: ContainerProcess::default(),
            consumption: Arc::new(ContainerConsumption::new()),
            executing_since: Instant::now(),
            terminal: None,
            maintained_by_a_service: false,
        }
    }

    /// This session performing the program of a container whose task a SERVICE's or a DAEMON's
    /// scheduler maintains: nothing is waiting for that program to end, so the waits it names
    /// take no ceiling of this emulator's and the container is up for every second of them (see
    /// the module docs).
    #[must_use]
    pub fn maintained_by_a_service(mut self) -> Self {
        self.maintained_by_a_service = true;
        self
    }

    /// This session as the PROCESS its container's launch resolved: the user it runs as, the
    /// resource limits it runs under and the ephemeral storage it writes on
    /// ([`super::container_process`]). A session built without it holds what a definition
    /// stating none of them holds, so nothing is reported that a launch did not resolve.
    #[must_use]
    pub fn with_process(mut self, process: ContainerProcess) -> Self {
        self.process = process;
        self
    }

    /// This session with the AWS CLI its container's environment provides on it: the
    /// `aws s3` transfers and the `aws sns` publish of the container, made as the task role
    /// ([`super::container_aws_cli`]). A session built without it provides no such program.
    #[must_use]
    pub fn with_aws_cli(mut self, aws: ContainerAwsCli) -> Self {
        self.aws = Some(Arc::new(aws));
        self
    }

    /// This session with the HTTP CLIENT its container's environment provides on it: the
    /// requests the container's program and its probe make to the servers of their own task
    /// ([`super::container_http_client`]). A session built without it provides no such program.
    #[must_use]
    pub fn with_http_client(mut self, http: ContainerHttpClient) -> Self {
        self.http = Some(Arc::new(http));
        self
    }

    /// This session accounting what it consumes into `consumption` — the accounting of the
    /// container's RUN, which the collections that report a container's utilization sample
    /// ([`super::container_consumption`]).
    #[must_use]
    pub fn with_consumption(mut self, consumption: Arc<ContainerConsumption>) -> Self {
        self.consumption = consumption;
        self.executing_since = Instant::now();
        self
    }

    /// This session attached to a TERMINAL of `rows` by `columns` — the pseudo-terminal an
    /// interactive session of an execute-command call runs in. A session built without one is
    /// attached to no terminal, which is what a container's own program and the agent's probes
    /// run as.
    #[must_use]
    pub fn with_terminal(mut self, rows: u32, columns: u32) -> Self {
        self.resize_terminal(rows, columns);
        self
    }

    /// Resize the terminal this session is attached to, as the client carrying it stated.
    ///
    /// The shell holds its terminal's size in `COLUMNS` and `LINES` — "the shell sets
    /// `COLUMNS` and `LINES` to the terminal's width and height" — so a program that reads
    /// either reads the size the client stated, and so does `stty size`
    /// ([`Self::run_terminal_report`]).
    pub fn resize_terminal(&mut self, rows: u32, columns: u32) {
        self.terminal = Some(TerminalSize { rows, columns });
        let _ = self.set_variable(TERMINAL_COLUMNS_VARIABLE.to_string(), columns.to_string());
        let _ = self.set_variable(TERMINAL_LINES_VARIABLE.to_string(), rows.to_string());
    }

    /// Run ONE command line a client typed into an interactive session of this container and
    /// answer the status the shell answered for it.
    ///
    /// Everything the session holds — its variables, its signal handlers, its terminal and the
    /// container's filesystem — is kept between lines, exactly as a shell reading its standard
    /// input keeps them, so the file one line wrote is the file the next line reads. What the
    /// line WROTE is delivered where this session's output goes before the status is answered,
    /// so a caller reading the output after the line has all of it.
    pub async fn run_typed_line(&mut self, line: &str) -> i32 {
        let mut status = self.run_text(line).await;
        // A line that ends while the signal it installed a handler for is outstanding still
        // runs that handler, exactly as a whole program does ([`Self::run`]).
        self.deliver_pending_signal().await;
        if let Some(latched) = self.exit_status {
            status = latched;
        }
        self.flush_pending_line().await;
        self.account_for_execution();
        status
    }

    /// Whether this session is OVER: a line performed `exit` (or a signal ended it), or a bound
    /// of this emulator was spent — after which it runs no further line, the way a shell that
    /// has exited reads no further input.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.stopped()
    }

    /// The status this session ended with, and `None` while it is still running.
    #[must_use]
    pub fn ended_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Run `program` to its end and answer what the container's run ended as.
    ///
    /// The status is the one the program ended with: `exit`'s, the last command's, or —
    /// for a session the termination signal ended with no handler installed for it — the
    /// status a shell reports for a command that signal ended ([`EXIT_SIGNAL_BASE`] plus
    /// the signal's number). A run a bound of this emulator ended instead is
    /// [`ContainerOutcome::Abandoned`], which claims nothing about the container.
    ///
    /// Every wait the program names is SPENT, wherever it stands in it and whichever spelling
    /// the program is written in ([`Self::run_sleep`]): the program has not ended while one of
    /// its waits is outstanding, so neither has the container. How much wall clock one wait may
    /// spend is this emulator's ([`wait_ceiling`]); see the module docs.
    pub async fn run(&mut self, program: &ContainerProgram) -> ContainerOutcome {
        let mut status = match program {
            ContainerProgram::Script { text, arguments } => {
                // The operands the invocation gave the program are its positional parameters
                // for the whole run ([`script_positional_parameters`]).
                self.positional = arguments.clone();
                self.run_text(text).await
            }
            ContainerProgram::Argv(argv) => self.run_argv(argv).await,
        };
        // A program that ends while the signal it installed a handler for is outstanding
        // still runs that handler: the signal arrived before the program ended.
        self.deliver_pending_signal().await;
        if let Some(latched) = self.exit_status {
            status = latched;
        }
        // The last line the program wrote may not have ended in a newline; the container's
        // output ends where the container does, so it is delivered as the line it is.
        self.flush_pending_line().await;
        // The last span of this run is accounted for where it ended, so what the run consumed
        // is complete when it is read ([`Self::account_for_execution`]).
        self.account_for_execution();
        if self.abandoned || self.unperformed {
            ContainerOutcome::Abandoned
        } else {
            ContainerOutcome::Exited(status)
        }
    }

    /// The status a shell answers INSIDE this session for a command, command FORM or
    /// expansion it does not perform ([`EXIT_COMMAND_NOT_FOUND`]) — recording that the run
    /// reached one, so nothing is claimed about the container it was for (see the module
    /// docs).
    ///
    /// The status is still what the program around the declined command reads, so an `&&`,
    /// an `||` or a loop condition over it behaves as a shell's does; what it is NOT is the
    /// container's own `exitCode`.
    fn declined(&mut self) -> i32 {
        self.unperformed = true;
        EXIT_COMMAND_NOT_FOUND
    }

    /// [`Self::declined`] for a command NAME this session has no program for, which is the
    /// name the detail such a run leaves on the container names
    /// ([`Self::nothing_performed_detail`]). The FIRST such name is kept: it is the command
    /// the run stopped performing the program at.
    fn declined_command(&mut self, name: &str) -> i32 {
        if self.declined_command.is_none() {
            self.declined_command = Some(name.to_string());
        }
        self.declined()
    }

    /// The additional detail a container whose run produced NO outcome reports for it, and
    /// `None` for a run that ended with a status of its own.
    ///
    /// `Container$reason` is "a short (1024 max characters) human-readable string to provide
    /// additional details about a running or stopped container", and the additional detail
    /// about a container of this emulator whose program produced nothing is WHY it produced
    /// nothing: the program it declared is one this emulator performs none of — named, where
    /// the thing it could not perform was a command name — or a bound of this emulator ended
    /// the run before the program finished. No status, no exit code and no stop is claimed
    /// with it: the detail is the whole of what is known about the run.
    #[must_use]
    pub fn nothing_performed_detail(&self) -> Option<String> {
        if self.unperformed {
            return Some(match self.declined_command.as_deref() {
                Some(name) => program_not_performed_detail(name),
                None => PROGRAM_FORM_NOT_PERFORMED_DETAIL.to_string(),
            });
        }
        self.abandoned.then(|| RUN_ENDED_BY_A_BOUND_DETAIL.to_string())
    }

    /// The status a command whose WORD had no expansion answers.
    ///
    /// Two different things end a word that way. A parameter this session holds nothing for
    /// while the program asked to be ended by exactly that (`set -u`) is the SHELL's own
    /// failure: "the shell shall write a message to standard error … and … shall exit", so the
    /// diagnostic is written and the session ends with the status a shell gives it — which is
    /// the container's own exit status, because that is what the program it ran did. Every
    /// other word is an expansion this container does not perform, which claims nothing about
    /// the container ([`Self::declined`]).
    async fn word_failure(&mut self) -> i32 {
        let Some(name) = self.unset_parameter.take() else {
            return self.declined();
        };
        self.write_output(&format!("{SHELL_NAME}: {name}: parameter not set\n"))
            .await;
        self.exit_status = Some(EXIT_FAILURE);
        EXIT_FAILURE
    }

    /// Run one shell PROGRAM text and answer the status the shell answered for it.
    ///
    /// A program that does not parse is the shell's own syntax error
    /// ([`EXIT_SYNTAX_ERROR`]); one outside the bounds this parse is given
    /// ([`within_parse_bounds`]) is a program this container did not run at all, which is
    /// reported as a command it could not run rather than as one that succeeded.
    fn run_text<'a>(&'a mut self, text: &'a str) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            if !within_parse_bounds(text) {
                return self.declined();
            }
            let Ok(tokens) = ::brush_parser::tokenize_str(text) else {
                return EXIT_SYNTAX_ERROR;
            };
            let options = ::brush_parser::ParserOptions::default();
            let Ok(program) = ::brush_parser::parse_tokens(&tokens, &options) else {
                return EXIT_SYNTAX_ERROR;
            };
            let mut status = EXIT_SUCCESS;
            for complete_command in &program.complete_commands {
                status = self.run_complete_command(complete_command).await;
                if self.stopped() {
                    return status;
                }
            }
            status
        })
    }

    /// Run one complete command of a program — the grammar's list of and-or lists — and
    /// answer the status of the last one that ran.
    fn run_complete_command<'a>(
        &'a mut self,
        command: &'a ::brush_parser::ast::CompleteCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            for item in &command.0 {
                status = self.run_and_or_list(&item.0).await;
                if self.stopped() {
                    return status;
                }
            }
            status
        })
    }

    /// Run one of the COMPOUND command forms this container performs.
    ///
    /// A brace group and a subshell are the list they hold; a `while`/`until` loop runs its
    /// body for as long as its condition says, a `for` loop runs it once per word of its
    /// list, an `if` runs the branch its condition chose, and a `case` runs the branch whose
    /// pattern its word matched. No loop needs a bound of its own: every command it performs
    /// comes out of the run's command budget, and a loop that outlasts it abandons the run.
    ///
    /// A subshell's effects are NOT confined to a child session here: there is no process
    /// to fork, so a variable it sets is set in this one. Every other compound form — an
    /// arithmetic command, a function definition — is one this container does not perform.
    ///
    /// This is the ONE place the walk re-enters a compound form, so it is where the walk's own
    /// nesting is bounded ([`MAX_COMPOUND_DEPTH`]): a form nested past that bound is one this
    /// container does not perform, rather than one more level of the thread's stack.
    fn run_compound_command<'a>(
        &'a mut self,
        compound: &'a ::brush_parser::ast::CompoundCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            if self.compound_depth >= MAX_COMPOUND_DEPTH {
                return self.declined();
            }
            self.compound_depth = self.compound_depth.saturating_add(1);
            let status = self.run_nested_compound_command(compound).await;
            self.compound_depth = self.compound_depth.saturating_sub(1);
            status
        })
    }

    /// The walk itself, one level deeper than its caller ([`Self::run_compound_command`], which
    /// is what counts that level).
    fn run_nested_compound_command<'a>(
        &'a mut self,
        compound: &'a ::brush_parser::ast::CompoundCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            match compound {
                ::brush_parser::ast::CompoundCommand::BraceGroup(group) => {
                    self.run_compound_list(&group.list).await
                }
                ::brush_parser::ast::CompoundCommand::Subshell(subshell) => {
                    self.run_compound_list(&subshell.list).await
                }
                ::brush_parser::ast::CompoundCommand::WhileClause(clause) => {
                    self.run_loop(clause, true).await
                }
                ::brush_parser::ast::CompoundCommand::UntilClause(clause) => {
                    self.run_loop(clause, false).await
                }
                ::brush_parser::ast::CompoundCommand::IfClause(clause) => {
                    self.run_if_clause(clause).await
                }
                ::brush_parser::ast::CompoundCommand::CaseClause(clause) => {
                    self.run_case_clause(clause).await
                }
                ::brush_parser::ast::CompoundCommand::ForClause(clause) => {
                    self.run_for_clause(clause).await
                }
                _ => self.declined(),
            }
        })
    }

    /// Run a `case WORD in PATTERN) LIST;; ... esac`: the list of the FIRST item one of whose
    /// patterns the word matches, and success for a `case` no pattern of which matched —
    /// "the exit status of case shall be zero if no patterns are matched".
    ///
    /// The word and every pattern are expanded first, as the shell expands them, and the
    /// comparison is the shell's pattern match ([`case_pattern_matches`]). A pattern this
    /// session does not read, or a word whose expansion it cannot resolve, is a form this
    /// container does not perform.
    ///
    /// Each ITEM takes a step out of the run's budget before its patterns are compared, the
    /// way a loop iteration does ([`Self::take_command_budget`]): that is what bounds the
    /// comparisons a program with many items can ask for, on top of the one step the `case`
    /// itself took as a command.
    fn run_case_clause<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::CaseClauseCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let Some(value) = self.expand_word(&clause.value.value).await else {
                return self.declined();
            };
            for item in &clause.cases {
                if !self.take_command_budget().await {
                    return EXIT_COMMAND_NOT_FOUND;
                }
                for pattern in &item.patterns {
                    let Some(expanded) = self.expand_word(&pattern.value).await else {
                        return self.declined();
                    };
                    let Some(matched) = case_pattern_matches(&expanded, &value) else {
                        return self.declined();
                    };
                    if !matched {
                        continue;
                    }
                    return match &item.cmd {
                        // An item with no list of its own runs nothing, which is the
                        // status POSIX gives it.
                        None => EXIT_SUCCESS,
                        Some(list) => self.run_compound_list(list).await,
                    };
                }
            }
            EXIT_SUCCESS
        })
    }

    /// Run an `if LIST; then LIST; elif LIST; then LIST; else LIST; fi`: the branch whose
    /// condition answered success, and success for an `if` no branch of which ran — "the
    /// exit status of the if compound command shall be ... zero, if no then compound-list or
    /// else compound-list is executed".
    fn run_if_clause<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::IfClauseCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let condition = self.run_condition(&clause.condition).await;
            if self.stopped() {
                return condition;
            }
            if condition == EXIT_SUCCESS {
                return self.run_compound_list(&clause.then).await;
            }
            for otherwise in clause.elses.iter().flatten() {
                match otherwise.condition.as_ref() {
                    // An `elif`, which carries its own condition and its own branch.
                    Some(condition) => {
                        let answered = self.run_condition(condition).await;
                        if self.stopped() {
                            return answered;
                        }
                        if answered == EXIT_SUCCESS {
                            return self.run_compound_list(&otherwise.body).await;
                        }
                    }
                    // The `else` branch, which runs when no condition before it did.
                    None => return self.run_compound_list(&otherwise.body).await,
                }
            }
            EXIT_SUCCESS
        })
    }

    /// Run a `while` (or `until`) loop: its condition, then its body for as long as that
    /// condition says — "the while loop shall continuously execute ... until the condition
    /// has a non-zero exit status", and the `until` loop until it has a zero one — and
    /// answer the status of the last body that ran ([`EXIT_SUCCESS`] for a loop whose body
    /// never ran, which is the status POSIX gives it).
    ///
    /// A body that performed a `break` or a `continue` ends the loop, or goes on with its
    /// next iteration, where that command stands ([`Self::taken_loop_control`]): this is one
    /// of the loops those commands are commands OF, which is what counting this level is for
    /// ([`Self::loop_depth`]).
    ///
    /// The loop is bounded by the run's own step budget and deadline: every ITERATION takes
    /// a step out of them, as does every command inside it, so a loop that outlasts either
    /// abandons the run rather than spinning for ever — including one whose condition and
    /// body are forms this container declines and so perform no command at all.
    fn run_loop<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::WhileOrUntilClauseCommand,
        while_zero: bool,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            self.loop_depth = self.loop_depth.saturating_add(1);
            let status = self.run_loop_iterations(clause, while_zero).await;
            self.loop_depth = self.loop_depth.saturating_sub(1);
            status
        })
    }

    /// The ITERATIONS of that loop, inside the level [`Self::run_loop`] counts.
    fn run_loop_iterations<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::WhileOrUntilClauseCommand,
        while_zero: bool,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            loop {
                // The iteration itself takes a step, before anything in it is reached:
                // that is what bounds a loop whose condition and body are forms this
                // container declines, which reach no command of their own.
                if !self.take_command_budget().await {
                    return status;
                }
                let condition = self.run_condition(&clause.0).await;
                if self.stopped() {
                    return status;
                }
                let again = if while_zero {
                    condition == EXIT_SUCCESS
                } else {
                    condition != EXIT_SUCCESS
                };
                if !again {
                    return status;
                }
                status = self.run_compound_list(&clause.1.list).await;
                if self.stopped() {
                    return status;
                }
                match self.taken_loop_control() {
                    Some(true) => continue,
                    Some(false) => return status,
                    None => {}
                }
            }
        })
    }

    /// Run a `for NAME in WORD…; do LIST; done`: the body once per word of the list, with
    /// the loop's variable holding that word — "the for loop shall execute a sequence of
    /// commands for each member in a list of items", with the variable "set to the next word
    /// taken from the `in` word list".
    ///
    /// The words are expanded first, as the shell expands them, so a list written as one
    /// expansion is the words that expansion produced. A `for` written with NO word list
    /// iterates over nothing here: it would take the positional parameters, which a container
    /// whose program is its argument vector has none of.
    ///
    /// A body that performed a `break` or a `continue` ends this loop, or goes on with its
    /// next word, exactly as a `while` body's does ([`Self::run_loop`]).
    ///
    /// Each ITERATION takes a step out of the run's budget before its body is reached, the
    /// way a `while` iteration does ([`Self::take_command_budget`]), so a list of many words
    /// is bounded exactly as a loop with no end of its own is.
    fn run_for_clause<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::ForClauseCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            self.loop_depth = self.loop_depth.saturating_add(1);
            let status = self.run_for_iterations(clause).await;
            self.loop_depth = self.loop_depth.saturating_sub(1);
            status
        })
    }

    /// The ITERATIONS of that `for`, inside the level [`Self::run_for_clause`] counts.
    fn run_for_iterations<'a>(
        &'a mut self,
        clause: &'a ::brush_parser::ast::ForClauseCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            let Some(values) = clause.values.as_ref() else {
                return status;
            };
            for word in values {
                if !self.take_command_budget().await {
                    return status;
                }
                let Some(value) = self.expand_word(&word.value).await else {
                    return self.declined();
                };
                if !self.set_variable(clause.variable_name.clone(), value) {
                    return self.declined();
                }
                status = self.run_compound_list(&clause.body.list).await;
                if self.stopped() {
                    return status;
                }
                match self.taken_loop_control() {
                    Some(true) => continue,
                    Some(false) => break,
                    None => {}
                }
            }
            status
        })
    }

    /// Run a compound LIST — the grammar's list of and-or lists inside a group, a loop body
    /// or a conditional — and answer the status of the last one that ran.
    ///
    /// A `break` or a `continue` one of them performed ends the list where it stands: what
    /// runs next is for the enclosing LOOP to decide ([`Self::taken_loop_control`]), so the
    /// lists between that command and the loop — the branch of an `if`, a brace group, a
    /// `case` item — stop too, exactly as the program's own `exit` stops them.
    fn run_compound_list<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::CompoundList,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            for item in &list.0 {
                status = self.run_and_or_list(&item.0).await;
                if self.stopped() || self.loop_control.is_some() {
                    return status;
                }
            }
            status
        })
    }

    /// What the `break` or `continue` the body just performed means FOR THIS LOOP:
    /// `Some(true)` to go on with its next iteration, `Some(false)` to end it, and `None`
    /// when the body performed neither — or when the one it performed is leaving an OUTER
    /// loop too, which is left in flight for that loop to take and makes this one end where
    /// it stands.
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

    /// Perform `break` or `continue` ([`BREAK_COMMAND`], [`CONTINUE_COMMAND`]): leave the
    /// enclosing loops the argument names — one by default — so what runs next is that
    /// loop's next iteration or the command after it.
    ///
    /// "If n is greater than the number of enclosing loops, the outermost enclosing loop
    /// shall be used", so a count past the loops being performed leaves all of them; one
    /// performed where there is no loop at all leaves nothing and answers success, and a
    /// count that is not a positive integer is the command's own usage failure.
    fn run_loop_control(&mut self, goes_on: bool, arguments: &[String]) -> i32 {
        let levels = match arguments.first() {
            Some(written) => match written.parse::<usize>() {
                Ok(levels) if levels >= 1 => levels,
                _ => return EXIT_FAILURE,
            },
            None => 1,
        };
        if self.loop_depth == 0 {
            return EXIT_SUCCESS;
        }
        self.loop_control = Some(LoopControl {
            goes_on,
            levels: levels.min(self.loop_depth),
        });
        EXIT_SUCCESS
    }

    /// Run a compound list as a CONDITION — an `if`'s, an `elif`'s or a loop's — which is
    /// what a command that fails inside it answers rather than ending the program under
    /// `set -e` ([`Self::run_and_or_list`]).
    fn run_condition<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::CompoundList,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            self.condition_depth = self.condition_depth.saturating_add(1);
            let status = self.run_compound_list(list).await;
            self.condition_depth = self.condition_depth.saturating_sub(1);
            status
        })
    }

    /// Run the argument vector of a container whose image exec's it directly: its first
    /// word is the utility and the rest are its arguments, with no shell to expand or split
    /// them.
    async fn run_argv(&mut self, argv: &[String]) -> i32 {
        let Some((name, arguments)) = argv.split_first() else {
            return EXIT_SUCCESS;
        };
        if !self.take_command_budget().await {
            return EXIT_COMMAND_NOT_FOUND;
        }
        // A vector the image exec's carries no redirection of its own: there is no shell to
        // read one.
        let redirections = Redirections::default();
        self.run_built_in(name, arguments, &redirections).await
    }

    /// Whether this session has stopped running commands: `exit` (or a signal) latched a
    /// status, or a bound of this emulator was spent.
    fn stopped(&self) -> bool {
        self.exit_status.is_some() || self.abandoned
    }

    /// Take one STEP out of this run's budget, hand the runtime back every
    /// [`COMMANDS_PER_YIELD`] steps, and answer whether the run may go on at all.
    ///
    /// Every command the walk visits takes a step, performed or declined
    /// ([`Self::run_command`]), and so does every iteration of a loop
    /// ([`Self::run_loop`]): that is what makes the budget, the deadline and the handback
    /// reachable from ANY program shape rather than only from one that keeps reaching a
    /// simple command — a loop whose condition and body are forms this container declines
    /// pays for its iterations exactly as one that performs them does.
    ///
    /// `false` ABANDONS the run: the budget or the deadline is spent, so the program did
    /// not end and nothing is claimed about the container's status.
    async fn take_command_budget(&mut self) -> bool {
        // Every step passes through here, so this is where the time the program has spent
        // EXECUTING is accounted for ([`Self::account_for_execution`]).
        self.account_for_execution();
        if self.commands_left == 0 || Instant::now() >= self.deadline {
            self.abandoned = true;
            return false;
        }
        self.commands_left -= 1;
        self.commands_since_yield += 1;
        if self.commands_since_yield >= COMMANDS_PER_YIELD {
            self.commands_since_yield = 0;
            ::tokio::task::yield_now().await;
            // The handback is not this program executing: the span another task had the
            // runtime for is none of the container's doing.
            self.executing_since = Instant::now();
        }
        true
    }

    /// Bring the accounting of what this container's run consumes up to date: the span since
    /// it was last brought up to date is time this program spent EXECUTING, and the bytes the
    /// session holds right now are the bytes it is occupying
    /// ([`super::container_consumption`]).
    ///
    /// Called from the two places a session's time divides at — the step it is about to take,
    /// and the wait it is about to enter — so the accounting separates the time the program
    /// held its performer from the time it was waiting on something.
    fn account_for_execution(&mut self) {
        let now = Instant::now();
        self.consumption
            .record_execution(now.saturating_duration_since(self.executing_since));
        self.executing_since = now;
        self.consumption.hold(self.occupied_bytes());
    }

    /// How many bytes of this process this session is holding: the variables it has set and
    /// the output line it is assembling. What the container's FILESYSTEM holds is the
    /// container's own rather than this session's, and is read off the layer itself
    /// ([`super::container_filesystem`]).
    fn occupied_bytes(&self) -> u64 {
        u64::try_from(self.held_bytes.saturating_add(self.pending_line.len()))
            .unwrap_or(u64::MAX)
    }

    /// Deliver the termination signal if it has arrived and its handler has not run yet.
    ///
    /// A shell runs a trapped signal's action between commands, and a session with no
    /// action recorded for the signal ends with the status that signal gives a command. An
    /// action recorded as the empty string is the program asking for the signal to be
    /// IGNORED, which is what `trap '' TERM` records.
    fn deliver_pending_signal(&mut self) -> ShellFuture<'_, ()> {
        Box::pin(async move {
            if !self.pending_term || self.delivering_term {
                return;
            }
            self.pending_term = false;
            match self.trap_action() {
                Some(action) if action.is_empty() => {}
                Some(action) => {
                    self.delivering_term = true;
                    // The handler runs with the latch clear: its own `exit` is what ends
                    // the session, and that is the status the container exits with.
                    self.run_text(&action).await;
                    self.delivering_term = false;
                }
                None => {
                    self.exit_status = Some(EXIT_SIGNAL_BASE + SIGTERM_NUMBER);
                }
            }
        })
    }

    /// The ceiling on the wall clock ONE wait of `stated` may spend in this session
    /// ([`wait_ceiling`]), and `None` for a session no ceiling applies to: the program of a
    /// container whose task a SERVICE maintains, which has no verdict waiting on its end (see
    /// the module docs).
    fn wait_ceiling_for(&self, stated: Duration) -> Option<Duration> {
        (!self.maintained_by_a_service).then(|| wait_ceiling(stated))
    }

    /// The action this session has recorded for the termination signal, under any of the
    /// names a program may have written it as ([`SIGTERM_NAMES`]).
    fn trap_action(&self) -> Option<String> {
        SIGTERM_NAMES
            .iter()
            .find_map(|name| self.traps.get(*name))
            .cloned()
    }

    /// Wait for `duration` — or until the deadline, when it names none — ending early if a
    /// signal arrives, and answer whether one did.
    ///
    /// Every wait is clamped to what is left of this run's deadline, so no wait a program
    /// asks for outlives the bound this emulator gave the run.
    async fn wait_for(&mut self, duration: Option<Duration>) -> bool {
        // A wait is not time this program spends executing, so what it has spent is accounted
        // for before the wait and measured again from the instant it ends.
        self.account_for_execution();
        let left = self.deadline.saturating_duration_since(Instant::now());
        let waiting = duration.map_or(left, |asked| asked.min(left));
        // The waiter is registered BEFORE the flag is read, so a signal raised between the
        // two is still what ends this wait.
        let signalled = self.signals.notified();
        if self.signals.terminating() {
            self.pending_term = true;
            return true;
        }
        let interrupted = ::tokio::time::timeout(waiting, signalled).await.is_ok();
        self.executing_since = Instant::now();
        if interrupted || self.signals.terminating() {
            self.pending_term = true;
            return true;
        }
        if Instant::now() >= self.deadline {
            self.abandoned = true;
        }
        false
    }

    /// Start the server `starting` names and hold the container's program until the container
    /// is stopped — which is what running a server in the FOREGROUND does.
    ///
    /// The server is published at the task's own address on the port it listens on, so whatever
    /// sends this task traffic reaches it ([`super::container_servers`]), and it stops being
    /// reachable when this session ends: the handle that keeps it listening is held here.
    ///
    /// A container whose task this emulator holds no address for is reached by nothing, so
    /// there is no server to start: the invocation is one this session does not perform
    /// ([`Self::declined`]), exactly as it was before this container shipped a server at all.
    ///
    /// A server that HAS started writes its own startup output
    /// ([`container_servers::startup_output`]) before it answers anything, on the stream that
    /// server writes it to — so the lines the log driver delivers for such a container are the
    /// ones its server really wrote. They travel the same writes every other command's output
    /// does, so `redirections` applies to them as it does to a command's.
    ///
    /// The command itself answers success, and what the CONTAINER then exits with is decided by
    /// the signal that ended the wait, not here: a foreground server runs until it is
    /// signalled, and the handler the program installed for that signal — or the signal itself —
    /// is what sets the session's status ([`Self::deliver_pending_signal`], whose latch takes
    /// precedence over any status a command answered). A wait this emulator's own bound on the
    /// run ended instead leaves the run claiming nothing ([`Self::wait_for`]).
    fn run_server<'a>(
        &'a mut self,
        starting: &'a container_servers::StartingServer,
        redirections: &'a Redirections,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let Some(serving) = self.serving.clone() else {
                return self.declined();
            };
            let document_root = starting.document_root(&self.filesystem);
            let files = container_servers::files_under(&self.filesystem, &document_root);
            let handle = serving.start(
                ::service_com_amazonaws_ec2_amazonec2::state::guest_services::GuestServer {
                    program: starting.program.clone(),
                    port: starting.port,
                    document_root,
                    lists_directories: starting.lists_directories,
                },
                files,
            );
            self.listening.push(handle);
            // The server is up, which is what it writes about: the instant it says so is this
            // one, the instant it started listening.
            for line in container_servers::startup_output(starting, ::std::time::SystemTime::now())
            {
                let written = format!("{}\n", line.text);
                let _ = if line.to_error {
                    self.write_stderr(&written, redirections).await
                } else {
                    self.write_stdout(&written, redirections).await
                };
            }
            // Until the container is stopped, or until this emulator's own bound on the run
            // ends it — which is the wait every other wait of this session is clamped to.
            self.wait_for(None).await;
            EXIT_SUCCESS
        })
    }

    /// Run an and-or list: its first pipeline, then each later pipeline only when the
    /// operator before it says so — `&&` runs the next on a zero status, `||` on a non-zero
    /// one — and answer the status of the last pipeline that ran.
    ///
    /// A list that ends in a failure ENDS the program when the program asked for that
    /// (`set -e`, [`Self::run_set`]) — and only then: POSIX exempts "any command of an
    /// AND-OR list other than the last", so a `write && echo done` whose WRITE failed is a
    /// list the program goes on from, while the same failure standing alone, or in the last
    /// command of such a list, ends it. The conditions of an `if` or a loop
    /// ([`Self::condition_depth`]) and a pipeline written with `!` are exempt for the same
    /// reason, as is a command inside a pipeline that is not its last.
    fn run_and_or_list<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::AndOrList,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let (status, ran_last) = self.run_and_or_pipelines(list).await;
            if self.errexit
                && status != EXIT_SUCCESS
                && ran_last
                && self.condition_depth == 0
                && !self.negated_pipeline
                // A command this session DECLINED is not a command that FAILED: it is one the
                // session does not perform, so the status it answers is read by the
                // conditions around it ([`Self::declined`]) and never ends the program. The
                // run claims nothing either way, and what the program wrote is delivered all
                // the same.
                && !self.unperformed
                && self.exit_status.is_none()
            {
                self.exit_status = Some(status);
            }
            status
        })
    }

    /// [`Self::run_and_or_list`]'s walk over the pipelines of the list: the status of the last
    /// pipeline that ran, and whether that pipeline was the LAST of the list — which is what
    /// decides whether `set -e` reads the status at all.
    fn run_and_or_pipelines<'a>(
        &'a mut self,
        list: &'a ::brush_parser::ast::AndOrList,
    ) -> ShellFuture<'a, (i32, bool)> {
        Box::pin(async move {
            let mut status = self.run_pipeline(&list.first).await;
            // The first pipeline is the last one when the list holds no other.
            let mut ran_last = list.additional.is_empty();
            let last = list.additional.len().saturating_sub(1);
            for (index, next) in list.additional.iter().enumerate() {
                if self.stopped() {
                    break;
                }
                let ran = match next {
                    ::brush_parser::ast::AndOr::And(pipeline) => {
                        if status == EXIT_SUCCESS {
                            status = self.run_pipeline(pipeline).await;
                            true
                        } else {
                            false
                        }
                    }
                    ::brush_parser::ast::AndOr::Or(pipeline) => {
                        if status != EXIT_SUCCESS {
                            status = self.run_pipeline(pipeline).await;
                            true
                        } else {
                            false
                        }
                    }
                };
                if ran {
                    ran_last = index == last;
                }
            }
            (status, ran_last)
        })
    }

    /// Run a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!` — or,
    /// where the program asked for it (`set -o pipefail`), the status of its last FAILING
    /// stage.
    ///
    /// A pipeline CONNECTS its commands: "the standard output of the first command shall be
    /// connected to the standard input of the second command", so every stage but the last
    /// writes to the stage after it rather than to the container's output, and what the last
    /// one wrote is what the pipeline wrote. A stage that redirects its own standard output
    /// sends it to the file instead and so passes nothing on, the way the shell's redirection
    /// takes precedence over the pipe.
    ///
    /// A stage that wrote more than [`MAX_CAPTURED_BYTES`] ends the pipeline: the bytes the
    /// pipe could not carry are bytes the next stage would have read, so the pipeline answers
    /// the failure of the write that did not happen rather than running the rest of it over a
    /// stream nothing produced.
    fn run_pipeline<'a>(
        &'a mut self,
        pipeline: &'a ::brush_parser::ast::Pipeline,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let mut status = EXIT_SUCCESS;
            // The last stage that FAILED, which is the pipeline's status where the program
            // asked for it (`set -o pipefail`).
            let mut failed: Option<i32> = None;
            let last = pipeline.seq.len().saturating_sub(1);
            // Which form the pipeline took is what `set -e` reads after it
            // ([`Self::run_and_or_list`]).
            self.negated_pipeline = pipeline.bang;
            // What the stage before the one being performed wrote.
            let mut upstream: Option<String> = None;
            for (index, command) in pipeline.seq.iter().enumerate() {
                let connected = index < last;
                if connected {
                    self.captures.push(Capture {
                        limit: MAX_CAPTURED_BYTES,
                        ..Capture::default()
                    });
                }
                // The input belongs to this one command: whatever the command around this
                // pipeline was reading is put back after it.
                let held_input = ::std::mem::replace(&mut self.piped_input, upstream.take());
                status = self.run_command(command).await;
                if status != EXIT_SUCCESS {
                    failed = Some(status);
                }
                self.piped_input = held_input;
                if connected {
                    let captured = self.captures.pop().unwrap_or_default();
                    if captured.overflowed {
                        return EXIT_FAILURE;
                    }
                    upstream = Some(captured.text);
                }
                if self.stopped() {
                    break;
                }
            }
            // "If set, the return value of a pipeline is the value of the last (rightmost)
            // command to exit with a non-zero status, or zero if all commands exit
            // successfully" — which is what `set -o pipefail` asked for.
            if self.pipefail {
                if let Some(answered) = failed {
                    status = answered;
                }
            }
            if pipeline.bang {
                status = if status == EXIT_SUCCESS {
                    EXIT_FAILURE
                } else {
                    EXIT_SUCCESS
                };
            }
            // "?: Expands to the decimal exit status of the most recent pipeline", which is
            // this one from here on.
            self.last_status = status;
            status
        })
    }

    /// Run one command of a pipeline: a SIMPLE command, or one of the compound forms this
    /// container performs ([`Self::run_compound_command`]).
    ///
    /// Every other form a shell grammar admits — a function definition, an extended test —
    /// is one this container does not perform, and a command that did not run answers as
    /// one that could not be run rather than as one that succeeded.
    fn run_command<'a>(
        &'a mut self,
        command: &'a ::brush_parser::ast::Command,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // Every command takes a step out of the run's budget, whether this container
            // performs it or declines it: a program made only of forms it declines is
            // bounded exactly as one it performs is ([`Self::take_command_budget`]).
            if !self.take_command_budget().await {
                return EXIT_COMMAND_NOT_FOUND;
            }
            match command {
                ::brush_parser::ast::Command::Simple(simple) => {
                    self.run_simple_command(simple).await
                }
                ::brush_parser::ast::Command::Compound(compound, redirects) => {
                    // A redirection names a file this container has no filesystem for, so a
                    // command carrying one did not run.
                    if redirects.is_some() {
                        return self.declined();
                    }
                    self.run_compound_command(compound).await
                }
                _ => self.declined(),
            }
        })
    }

    /// Run a simple command: its variable assignments and its REDIRECTIONS, then the command
    /// its first word names with the words after it as its arguments, with what it writes
    /// going where those redirections send it.
    ///
    /// A command with no word at all is the assignment form — "the variable assignments
    /// shall affect the current execution environment". An assignment written BEFORE a
    /// command word affects that command's environment alone, and persists in the session
    /// only for the special built-in utilities POSIX keeps it for ([`PERSISTING_COMMANDS`]);
    /// since every word is expanded before the assignments are applied, a command that does
    /// not keep them sees the value either way. Any prefix or suffix item that is neither a
    /// word, an assignment nor a redirection this container performs
    /// ([`Self::apply_redirect`]) is a form it does not perform, and the command is then not
    /// run.
    ///
    /// WHERE the word stands decides whether it is an assignment at all: "assignment to a
    /// variable shall occur only in the command prefix", so a word of the `NAME=VALUE` form
    /// standing AFTER the command name is one of the command's own arguments and nothing
    /// else — `echo IDX=1` writes `IDX=1` and sets no variable. The grammar reports such a
    /// word as an assignment word wherever it stands, because that is the form the utilities
    /// taking one read their operands in (`export NAME=VALUE`), so a suffix item of that kind
    /// reaches the command as the WORD it was written as.
    ///
    /// The files the redirections name are OPENED before the command is resolved, which is
    /// the order a shell opens them in: a file the container's filesystem refuses to open —
    /// a write through a read-only mount, a path whose directory is missing — is reported in
    /// the shell's own wording and the command does NOT run ([`Self::open_redirections`]).
    /// What a redirected command then writes is collected and written to the file as the
    /// command finishes, so one command's output is one write of the file.
    fn run_simple_command<'a>(
        &'a mut self,
        simple: &'a ::brush_parser::ast::SimpleCommand,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // A signal that arrived while the command before this one ran is delivered
            // first: a shell runs a trapped action between commands.
            self.deliver_pending_signal().await;
            if self.signals.terminating() && !self.delivering_term {
                self.pending_term = true;
                self.deliver_pending_signal().await;
            }
            if self.stopped() {
                return self.exit_status.unwrap_or(EXIT_SUCCESS);
            }
            // Each command gets the whole expansion budget: the words of one command are
            // live at the same instant, and the words of the command before it are not.
            self.expansion_left = MAX_LINE_EXPANSION_BYTES;
            // The status of a command with no command name is the status of the last command
            // substitution ITS OWN words performed, so what an earlier command substituted is
            // not read here.
            self.last_substitution_status = None;
            let mut assignments: Vec<(String, String)> = Vec::new();
            let mut words: Vec<String> = Vec::new();
            let mut redirections = Redirections::default();
            // The prefix's items and the suffix's are read in the order they were written,
            // gathered first so the walk over them has one type — each carrying WHERE it
            // stands, because that is what decides whether a `NAME=VALUE` word assigns
            // anything (see this method's own documentation).
            let mut items: Vec<(bool, &::brush_parser::ast::CommandPrefixOrSuffixItem)> =
                Vec::new();
            if let Some(prefix) = simple.prefix.as_ref() {
                items.extend(prefix.0.iter().map(|item| (true, item)));
            }
            if let Some(suffix) = simple.suffix.as_ref() {
                items.extend(suffix.0.iter().map(|item| (false, item)));
            }
            for (in_prefix, item) in items {
                match item {
                    ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word) => {
                        let Some(expanded) = self.expand_word(&word.value).await else {
                            return self.word_failure().await;
                        };
                        words.push(expanded);
                    }
                    ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(
                        assignment,
                        word,
                    ) => {
                        if !in_prefix {
                            // After the command name it is an ARGUMENT: the word itself,
                            // expanded like any other.
                            let Some(expanded) = self.expand_word(&word.value).await else {
                                return self.word_failure().await;
                            };
                            words.push(expanded);
                            continue;
                        }
                        let Some(pair) = self.assignment_of(assignment).await else {
                            return self.word_failure().await;
                        };
                        assignments.push(pair);
                    }
                    ::brush_parser::ast::CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                        if !self.apply_redirect(redirect, &mut redirections).await {
                            return self.word_failure().await;
                        }
                    }
                    _ => return self.declined(),
                }
            }
            let name = match simple.word_or_name.as_ref() {
                Some(word) => match self.expand_word(&word.value).await {
                    Some(expanded) => Some(expanded),
                    None => return self.word_failure().await,
                },
                None => None,
            };
            let persists = name
                .as_deref()
                .is_none_or(|name| PERSISTING_COMMANDS.contains(&name));
            if persists {
                for (name, value) in assignments {
                    if !self.set_variable(name, value) {
                        return self.declined();
                    }
                }
            }
            // The files are opened before the command is resolved, the way a shell opens
            // them: one the filesystem refuses means the command did not run at all.
            if let Err(status) = self.open_redirections(&redirections).await {
                return status;
            }
            // What a redirected command writes is collected for the file and written when the
            // command has finished, which is one write of what the command produced.
            //
            // The refused-write flag is this command's own: a command nested inside it (a
            // substitution, a stage of a pipeline) answers for its own refused write, and the
            // one held for a command still running is restored after it.
            let outer_refused = ::std::mem::replace(&mut self.redirect_failed, false);
            let redirected = redirections.out.take();
            if redirected.is_some() {
                self.captures.push(Capture {
                    limit: MAX_REDIRECTED_BYTES,
                    ..Capture::default()
                });
            }
            let status = match name {
                Some(name) => self.run_built_in(&name, &words, &redirections).await,
                // Nothing but assignments or a redirection alone: they have been applied,
                // which is the whole command. "If there is no command name, but the command
                // contained a command substitution, the command shall complete with the exit
                // status of the last command substitution performed."
                None => self
                    .last_substitution_status
                    .take()
                    .unwrap_or(EXIT_SUCCESS),
            };
            if let Some((path, _)) = redirected {
                let captured = self.captures.pop().unwrap_or_default();
                // The file was opened above, so what the command wrote is ADDED to what that
                // open left: a `>` truncated the file as it opened it, and a `>>` left what
                // the file held.
                if captured.overflowed {
                    self.redirect_failed = true;
                } else if !captured.text.is_empty() {
                    if let Err(refusal) = self.filesystem.write(&path, &captured.text, true) {
                        self.report_write_refusal(&path, refusal, &redirections).await;
                        self.redirect_failed = true;
                    }
                }
            }
            if ::std::mem::replace(&mut self.redirect_failed, outer_refused) {
                // A write the filesystem would not take did not happen, so the command did
                // not do what it was asked: it answers failure, the way a command whose
                // redirection failed does.
                return EXIT_FAILURE;
            }
            status
        })
    }

    /// RECORD one redirection of a simple command among the ones that command carries —
    /// answering `false` for a form this container does not perform, which makes the command
    /// one that did not run.
    ///
    /// WHICH forms are read is the shared reading of a redirection node
    /// ([`AppliedRedirect::of`], and [`HereInput::of`] for the ones whose text is written
    /// inside the command), over the target word this session expanded: the redirections of
    /// the three standard descriptors, the duplication of one standard stream onto the other
    /// — which is how a program asks for its diagnostics on the one stream its caller reads —
    /// and the HERE-DOCUMENT (`<<WORD`, `<<-WORD`) and HERE-STRING (`<<<WORD`) that ARE the
    /// command's standard input, which is how a program hands a document it composed to a
    /// command without a file to put it in.
    fn apply_redirect<'a>(
        &'a mut self,
        redirect: &'a ::brush_parser::ast::IoRedirect,
        redirections: &'a mut Redirections,
    ) -> ShellFuture<'a, bool> {
        Box::pin(async move {
            // The text written INSIDE the command is read first: it names no file, so nothing
            // of the filesystem is opened for it.
            if let Some(here) = HereInput::of(redirect) {
                let text = if here.requires_expansion {
                    match self.expand_word(here.written).await {
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
                    match self.expand_word(&word.value).await {
                        Some(expanded) => expanded,
                        None => return false,
                    }
                }
                _ => return false,
            };
            match AppliedRedirect::of(*descriptor, kind, &expanded) {
                Some(applied) => {
                    applied.record(redirections);
                    true
                }
                None => false,
            }
        })
    }

    /// OPEN the files this command's redirections name, the way a shell opens them before it
    /// resolves the command: a `>` truncates the file (creating it where there is none), a
    /// `>>` creates it and leaves what it holds, and a `<` has to be readable.
    ///
    /// `Err` is a redirection that could not be opened, which is reported on standard error
    /// in the shell's own wording — `sh: can't create <path>: <what the filesystem answered>`
    /// — and makes the command one that did not run at all: it answers failure, and the
    /// program around it reads that status.
    async fn open_redirections(&mut self, redirections: &Redirections) -> Result<(), i32> {
        for (path, append) in [redirections.out.clone(), redirections.error.clone()]
            .into_iter()
            .flatten()
        {
            // An open for writing creates the file; `>` also truncates what it held, which
            // is what writing nothing from the start of it does.
            if let Err(refusal) = self.filesystem.write(&path, "", append) {
                self.report_write_refusal(&path, refusal, redirections).await;
                return Err(EXIT_FAILURE);
            }
        }
        if let Some(path) = redirections.input.clone() {
            if let Err(refusal) = self.filesystem.read(&path) {
                self.write_stderr(
                    &format!("{SHELL_NAME}: can't open {path}: {}\n", refusal.message()),
                    redirections,
                )
                .await;
                return Err(EXIT_FAILURE);
            }
        }
        Ok(())
    }

    /// Report a write the container's filesystem refused, in the wording the shell reports a
    /// redirection it could not open in — which carries the text of the error the kernel
    /// answered, so a write through a read-only mount says `Read-only file system`.
    async fn report_write_refusal(
        &mut self,
        path: &str,
        refusal: FileRefusal,
        redirections: &Redirections,
    ) {
        self.write_stderr(
            &format!(
                "{SHELL_NAME}: can't create {path}: {}\n",
                refusal.message()
            ),
            redirections,
        )
        .await;
    }

    /// One assignment of a simple command as the name and value it sets, or `None` when it
    /// is a form this container does not perform — an array assignment, an appending
    /// assignment, or a value whose expansion it cannot resolve.
    fn assignment_of<'a>(
        &'a mut self,
        assignment: &'a ::brush_parser::ast::Assignment,
    ) -> ShellFuture<'a, Option<(String, String)>> {
        Box::pin(async move {
            if assignment.append {
                return None;
            }
            let ::brush_parser::ast::AssignmentName::VariableName(name) = &assignment.name else {
                return None;
            };
            let ::brush_parser::ast::AssignmentValue::Scalar(value) = &assignment.value else {
                return None;
            };
            Some((name.clone(), self.expand_word(&value.value).await?))
        })
    }

    /// Set `name` to `value` in this session, unless it is already holding
    /// [`MAX_SESSION_BYTES`] or [`MAX_SESSION_VARIABLES`] — `false` is then a command this
    /// container did not perform, which is what its caller reports.
    ///
    /// A change made while a command SUBSTITUTION is open is recorded on that substitution's
    /// undo log first ([`Self::record_for_rollback`]), so it is put back when the subshell
    /// ends.
    fn set_variable(&mut self, name: String, value: String) -> bool {
        let displaced = self
            .variables
            .get(&name)
            .map(|held| name.len().saturating_add(held.len()));
        if displaced.is_none() && self.variables.len() >= MAX_SESSION_VARIABLES {
            return false;
        }
        let would_hold = self
            .held_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(name.len())
            .saturating_add(value.len());
        if would_hold > MAX_SESSION_BYTES {
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

    /// Run the command `name` with `arguments`, when it is one this container provides.
    ///
    /// `echo` writes its arguments and a newline to its standard output (`-n` leaves the
    /// newline off, as that option is specified to). `true` and `:` answer success and
    /// `false` answers failure, which is the whole of what those utilities do. `exit` ends
    /// the session with the status its argument names, defaulting to success, and a status
    /// outside 0..=255 is taken modulo 256 the way a shell takes an exit status.
    /// `export`/`unset` change the session's variables, `trap` records what a signal does
    /// to it, `sleep` waits for the seconds its argument names, and `wait` answers success
    /// because this session starts no job to wait for.
    ///
    /// A name outside [`BUILT_IN_COMMANDS`] is one of the commands that work over the
    /// container's FILESYSTEM ([`FILE_COMMANDS`], [`Self::run_file_command`]), one of the
    /// TEXT FILTERS that compute over one input
    /// ([`::service_common::posix_shell_text_filters`], [`Self::run_text_filter`]), or one of
    /// the SERVERS the container's image ships ([`super::container_servers`],
    /// [`Self::run_server`]); a name outside all four needs a program or a network this
    /// container does not have.
    ///
    /// A command the session is REPLACED by is reached through here too: `exec` is peeled off
    /// the front of the command ([`Self::run_built_in`]'s caller-visible behaviour, stated on
    /// [`EXEC_COMMAND`]), so what runs is the command after it and the session ENDS with the
    /// status that command answered — "the shell shall replace itself with the command", so
    /// there is no session left to run what follows.
    fn run_built_in<'a>(
        &'a mut self,
        name: &'a str,
        arguments: &'a [String],
        redirections: &'a Redirections,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            // The leading `exec`s of one command, peeled rather than re-entered: the words come
            // out of the command's own vector, so following them by recursion would cost one
            // stack frame per word a caller wrote.
            let mut name = name;
            let mut arguments = arguments;
            let mut replaced = false;
            while name == EXEC_COMMAND {
                replaced = true;
                let Some((replacement, rest)) = arguments.split_first() else {
                    // An `exec` with no command at all has only the redirections it carried,
                    // which have been applied already, and the session is not replaced.
                    return EXIT_SUCCESS;
                };
                name = replacement.as_str();
                arguments = rest;
            }
            let status = self.run_named_command(name, arguments, redirections).await;
            if replaced && self.exit_status.is_none() {
                // The session WAS that command, so when the command is over the session is
                // over: its status is the status the container exits with.
                self.exit_status = Some(status);
            }
            status
        })
    }

    /// What one command NAME answers, once any `exec` in front of it has been peeled
    /// ([`Self::run_built_in`]).
    fn run_named_command<'a>(
        &'a mut self,
        name: &'a str,
        arguments: &'a [String],
        redirections: &'a Redirections,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            if FILE_COMMANDS.contains(&name) {
                return self.run_file_command(name, arguments, redirections).await;
            }
            if ::service_common::posix_shell_text_filters::TEXT_FILTER_COMMANDS.contains(&name) {
                return self.run_text_filter(name, arguments, redirections).await;
            }
            // A server the container's IMAGE ships, invoked the way that image's own command
            // invokes it: the command starts the server and then holds the container's program
            // until it is stopped ([`Self::run_server`]).
            if let Some(starting) =
                container_servers::server_started_by(name, arguments, self.filesystem.working_in())
            {
                return self.run_server(&starting, redirections).await;
            }
            // The AWS CLI, which is a program of the container's ENVIRONMENT rather than a
            // utility of this session: what it performs is the shared reading of that program
            // ([`super::container_aws_cli`]).
            if name == ::service_common::aws_s3_cli::CLI_PROGRAM {
                return self.run_aws_cli(arguments, redirections).await;
            }
            // An HTTP client, which is a program of the container's ENVIRONMENT for the same
            // reason: the request it makes is to a server of this container's own task
            // ([`super::container_http_client`]).
            if let Some(client) = container_http_client::http_client_invoked(name) {
                return self.run_http_client(client, arguments, redirections).await;
            }
            // The catalogs are what this container provides: a command outside them needs a
            // program or a network this container does not have — and the NAME of it is what
            // the detail such a container reports names
            // ([`Self::nothing_performed_detail`]).
            if !BUILT_IN_COMMANDS.contains(&name) {
                return self.declined_command(name);
            }
            match name {
                "echo" => {
                    let (newline, printed) = match arguments.split_first() {
                        Some((first, rest)) if first == "-n" => (false, rest),
                        _ => (true, arguments),
                    };
                    let mut written = printed.join(" ");
                    if newline {
                        written.push('\n');
                    }
                    if self.write_stdout(&written, redirections).await {
                        EXIT_SUCCESS
                    } else {
                        EXIT_FAILURE
                    }
                }
                "true" | ":" => EXIT_SUCCESS,
                // "The printf utility shall write formatted operands to the standard output":
                // the format and its arguments, written as this workspace reads one
                // ([`Self::run_printf`]).
                PRINTF_COMMAND => self.run_printf(arguments, redirections).await,
                // "hostname — show or set the system's host name": with no operand it WRITES
                // the name, which for a container is the one the runtime gave it
                // ([`super::container_runtime::HOSTNAME_VARIABLE`]). Only the unqualified name
                // this container has is answered, so a form that asks for anything else (a
                // name to SET, the fully qualified name, the addresses it resolves to) is one
                // this session does not perform.
                "hostname" => {
                    if !arguments.is_empty() && arguments != ["-s"] {
                        return self.declined();
                    }
                    let Some(hostname) = self
                        .variables
                        .get(super::container_runtime::HOSTNAME_VARIABLE)
                        .cloned()
                    else {
                        // Nothing gave this container a hostname, so it has none to write and
                        // no name is invented for it.
                        return self.declined();
                    };
                    if self
                        .write_stdout(&format!("{hostname}\n"), redirections)
                        .await
                    {
                        EXIT_SUCCESS
                    } else {
                        EXIT_FAILURE
                    }
                }
                "false" => EXIT_FAILURE,
                "exit" => {
                    let status = match arguments.first() {
                        Some(argument) => match argument.parse::<i64>() {
                            Ok(status) => {
                                i32::try_from(status.rem_euclid(256)).unwrap_or(EXIT_FAILURE)
                            }
                            // A status a shell cannot read is its own usage failure, and
                            // the session still ends.
                            Err(_) => EXIT_FAILURE,
                        },
                        None => EXIT_SUCCESS,
                    };
                    self.exit_status = Some(status);
                    status
                }
                "export" => {
                    for argument in arguments {
                        // `export NAME=VALUE` sets it; `export NAME` marks a variable this
                        // session already holds, and marking one it does not hold sets
                        // nothing.
                        if let Some((name, value)) = argument.split_once('=') {
                            if !self.set_variable(name.to_string(), value.to_string()) {
                                return self.declined();
                            }
                        }
                    }
                    EXIT_SUCCESS
                }
                "unset" => {
                    for argument in arguments {
                        self.unset_variable(argument);
                    }
                    EXIT_SUCCESS
                }
                "trap" => self.run_trap(arguments),
                "set" => self.run_set(arguments),
                // The commands of the LOOPS this walk performs: each leaves the enclosing
                // loops its argument names ([`Self::run_loop_control`]).
                BREAK_COMMAND => self.run_loop_control(false, arguments),
                CONTINUE_COMMAND => self.run_loop_control(true, arguments),
                SLEEP_COMMAND => self.run_sleep(arguments).await,
                // "wait: ... If no process ID is given, waits for all currently active
                // child processes" — this session has none, so there is nothing to wait
                // for and the utility answers success.
                "wait" => EXIT_SUCCESS,
                "test" | "[" => self.run_test(name, arguments),
                // "date — write the date and time": the instant this command ran at, in the
                // form the invocation asked for ([`CLOCK_COMMAND`]). A form that reading
                // renders nothing of is a command that did not run ([`Self::declined`]) rather
                // than an instant nothing stands behind.
                CLOCK_COMMAND => {
                    let written = ::service_common::posix_shell_clock::written(
                        arguments,
                        ::std::time::SystemTime::now(),
                    );
                    let Some(written) = written else {
                        return self.declined();
                    };
                    if self
                        .write_stdout(&format!("{written}\n"), redirections)
                        .await
                    {
                        EXIT_SUCCESS
                    } else {
                        EXIT_FAILURE
                    }
                }
                _ if PROCESS_REPORT_COMMANDS.contains(&name) => {
                    self.run_process_report(name, arguments, redirections).await
                }
                _ if TERMINAL_REPORT_COMMANDS.contains(&name) => {
                    self.run_terminal_report(name, arguments, redirections).await
                }
                _ => self.declined(),
            }
        })
    }

    /// Perform one of the commands that REPORT a fact about the process this session is
    /// ([`PROCESS_REPORT_COMMANDS`]): where it runs, who it runs as, the resource limits it
    /// runs under, or the space on the filesystem it writes on.
    ///
    /// Each answer is the one the container's launch resolved
    /// ([`super::container_process`]), written as the utility writes it. An invocation that
    /// asks for a form this session does not perform, or about a setting this emulator holds
    /// no value for, is a command that did not run ([`Self::declined`]) rather than a value
    /// nothing backs.
    async fn run_process_report(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        let written = match name {
            "pwd" => working_directory_report(arguments, self.filesystem.working_in()),
            "id" => self.process.identity_report(arguments),
            "ulimit" => self.process.limit_report(arguments),
            // WHICH filesystem each operand is on is resolved against the container's own
            // filesystem here, because that is what holds the mounts.
            "df" => {
                let in_a_mount = arguments
                    .iter()
                    .filter(|argument| !argument.starts_with('-'))
                    .any(|path| self.filesystem.is_in_a_mount(path));
                self.process
                    .storage_report(arguments, self.filesystem.layer_bytes(), in_a_mount)
            }
            _ => None,
        };
        let Some(written) = written else {
            return self.declined();
        };
        if self.write_stdout(&written, redirections).await {
            EXIT_SUCCESS
        } else {
            EXIT_FAILURE
        }
    }

    /// `stty size` and `tput cols`/`tput lines`: the size of the TERMINAL this session is
    /// attached to, which for an interactive session of an execute-command call is the size
    /// its client stated on the channel ([`Self::resize_terminal`]).
    ///
    /// `stty size` "print[s] the number of rows and columns" of that terminal, in that order;
    /// `tput` answers "the value of the terminal-dependent capability" asked for, of which the
    /// two this session holds a value for are the screen's columns and lines. A session
    /// attached to NO terminal, and any other form or capability, is a command that did not
    /// run ([`Self::declined`]) rather than a value nothing backs.
    async fn run_terminal_report(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        let Some(terminal) = self.terminal else {
            return self.declined();
        };
        let asked: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let written = match (name, asked.as_slice()) {
            ("stty", ["size"]) => Some(format!("{} {}\n", terminal.rows, terminal.columns)),
            ("tput", ["cols"]) => Some(format!("{}\n", terminal.columns)),
            ("tput", ["lines"]) => Some(format!("{}\n", terminal.rows)),
            _ => None,
        };
        let Some(written) = written else {
            return self.declined();
        };
        if self.write_stdout(&written, redirections).await {
            EXIT_SUCCESS
        } else {
            EXIT_FAILURE
        }
    }

    /// `test EXPRESSION`, and its `[ EXPRESSION ]` spelling: the question the expression
    /// asks, answered as a status — success for true, failure for false.
    ///
    /// Two kinds of question are answered. The ones about the container's own FILESYSTEM —
    /// `-e` (the path exists), `-f` (it is a regular file), `-d` (it is a directory), `-s`
    /// (it is a file with something in it), `-r` and `-w` (it can be read, it can be
    /// written, which is what a read-only mount answers false for) — and the ones about the
    /// words themselves, which is the half of the utility this workspace's own reading of it
    /// answers ([`::service_common::posix_shell_session::value_test_answer`]): an expression
    /// of one argument, `-z`/`-n`, the string comparisons and the integer comparisons. Each
    /// may be negated with a leading `!`.
    ///
    /// Every other form — an expression joined with `-a` or `-o`, a question about a file's
    /// mode or age — is one this session does not answer, so it is a command that did not run
    /// ([`Self::declined`]) rather than an answer of false.
    fn run_test(&mut self, name: &str, arguments: &[String]) -> i32 {
        let mut arguments = arguments.to_vec();
        if name == "[" {
            // The `[` spelling is the same utility, and its last argument is the closing
            // bracket rather than part of the question; an invocation that does not close
            // is the usage this session does not read.
            if arguments.last().map(String::as_str) != Some("]") {
                return self.declined();
            }
            arguments.pop();
        }
        let negated = arguments.first().map(String::as_str) == Some("!");
        if negated {
            arguments.remove(0);
        }
        let answer = match self.file_test_answer(&arguments) {
            Some(answer) => answer,
            None => match ::service_common::posix_shell_session::value_test_answer(&arguments) {
                Some(answer) => answer,
                None => return self.declined(),
            },
        };
        if answer != negated {
            EXIT_SUCCESS
        } else {
            EXIT_FAILURE
        }
    }

    /// What the `test` questions about a FILE answer for `arguments`: the shared reading of
    /// them ([`::service_common::posix_shell_files::file_test_answer`]) over this container's
    /// own filesystem, and `None` for an expression that asks about something else (see
    /// [`Self::run_test`]).
    fn file_test_answer(&self, arguments: &[String]) -> Option<bool> {
        ::service_common::posix_shell_files::file_test_answer(arguments, &self.filesystem)
    }

    /// `printf FORMAT [ARGUMENT…]`: the format with its arguments substituted, on the
    /// container's standard output.
    ///
    /// What one format writes is the workspace's one reading of a `printf` format
    /// ([`::service_common::posix_shell_session::printf_formatted`]), so a program that
    /// reports a value writes the same text whichever session performs it. An invocation with
    /// no format at all, and a format carrying a conversion or an escape that reading does not
    /// perform, writes NOTHING and answers the utility's own failure with its diagnostic on
    /// standard error — the utility ran and refused the format, so it is not a command this
    /// session could not run.
    async fn run_printf(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some((format, rest)) = arguments.split_first() else {
            self.write_stderr(
                &format!("{PRINTF_COMMAND}: usage: {PRINTF_COMMAND} format [arguments]\n"),
                redirections,
            )
            .await;
            return EXIT_FAILURE;
        };
        let Some(text) = ::service_common::posix_shell_session::printf_formatted(format, rest)
        else {
            self.write_stderr(
                &format!("{PRINTF_COMMAND}: {format}: invalid format\n"),
                redirections,
            )
            .await;
            return EXIT_FAILURE;
        };
        if self.write_stdout(&text, redirections).await {
            EXIT_SUCCESS
        } else {
            EXIT_FAILURE
        }
    }

    /// `set [-e|+e|-u|+u|-o pipefail|+o pipefail]`: turn the shell options its arguments name
    /// on or off.
    ///
    /// The options a container's program is written with, each performed as the shell
    /// specifies it:
    ///
    /// * `-e` — "when this option is on, if a simple command fails … the shell shall
    ///   immediately exit" — except where the command that failed is a CONDITION (an `if`'s, a
    ///   loop's), stands before a `&&` or `||`, or is a pipeline written with `!`, which are
    ///   the exemptions POSIX states and [`Self::run_and_or_list`] applies. A command this
    ///   SESSION declines is exempt too, for the reason stated there;
    /// * `-u` — "the shell shall write a message to standard error when it tries to expand a
    ///   variable that is not set and … shall exit", which is what
    ///   [`Self::expand_parameter`] then answers for such a parameter;
    /// * `-o pipefail` — the status of a pipeline is then its LAST FAILING stage's rather than
    ///   its last stage's ([`Self::run_pipeline`]).
    ///
    /// Options are read as the shell reads them: one argument may carry several letters
    /// (`set -eu`), `+` turns them off again, and `-o`/`+o` takes the long option's name as
    /// the argument after it.
    ///
    /// Every other invocation is one this session does not perform: a bare `set` writes a
    /// listing of the session's variables, which this writes none of, and the other options
    /// change how the shell itself behaves in ways nothing here implements — so a program
    /// asking for one did not run rather than running without it.
    fn run_set(&mut self, arguments: &[String]) -> i32 {
        if arguments.is_empty() {
            return self.declined();
        }
        let mut words = arguments.iter();
        while let Some(argument) = words.next() {
            let on = match argument.chars().next() {
                Some('-') => true,
                Some('+') => false,
                _ => return self.declined(),
            };
            let letters: Vec<char> = argument.chars().skip(1).collect();
            if letters.is_empty() {
                return self.declined();
            }
            for letter in letters {
                match letter {
                    'e' => self.errexit = on,
                    'u' => self.nounset = on,
                    // `-o NAME`: the long option the argument AFTER this one names.
                    'o' => match words.next().map(String::as_str) {
                        Some("pipefail") => self.pipefail = on,
                        _ => return self.declined(),
                    },
                    _ => return self.declined(),
                }
            }
        }
        EXIT_SUCCESS
    }

    /// `trap ACTION SIGNAL...`: record what each named signal does to this session.
    ///
    /// The forms POSIX gives the utility: an action of `-` resets the signal to its default
    /// (which ends the session), an empty action has it IGNORED, and any other action is
    /// the program the session runs when the signal arrives.
    ///
    /// Two limits of this session, stated because a program cannot see them: a call naming
    /// NO signal records nothing and writes nothing — a shell would list the traps it holds
    /// on its output, and this writes no listing — and an action is recorded for whatever
    /// signal the program names while only the TERMINATION signal is ever raised here
    /// ([`ContainerSignals`]), so a handler for any other signal is recorded and never runs.
    fn run_trap(&mut self, arguments: &[String]) -> i32 {
        let Some((action, signals)) = arguments.split_first() else {
            return EXIT_SUCCESS;
        };
        if signals.is_empty() {
            return EXIT_SUCCESS;
        }
        for signal in signals {
            let key = signal_key(signal);
            if action == "-" {
                self.traps.remove(&key);
                continue;
            }
            if !self.traps.contains_key(&key) && self.traps.len() >= MAX_TRAPS {
                return EXIT_FAILURE;
            }
            self.traps.insert(key, action.clone());
        }
        EXIT_SUCCESS
    }

    /// `sleep SECONDS`: wait for the seconds its argument names, or until a signal arrives.
    ///
    /// The argument is read as a number of seconds, decimals included, and `infinity` is a
    /// wait with no end of its own — the way the utility's own implementations read it. A
    /// wait is clamped to what this run has left ([`Self::wait_for`]), so it never outlives
    /// the bound the emulator gave the run. An argument that is no duration at all is the
    /// utility's usage failure.
    ///
    /// The seconds are SPENT, wherever this wait stands in the program: "the `sleep` utility
    /// shall suspend execution for at least the integral number of seconds specified by the
    /// `time` operand", so the program has not reached its next command — nor its own end, nor
    /// the status that end carries — while the wait is outstanding, and the container it runs in
    /// is still there throughout. How much wall clock one wait may spend on the caller's own
    /// seconds is this emulator's ([`wait_ceiling`], which a container whose task a SERVICE
    /// maintains takes none of); past it the program carries on from the wait. A wait that names
    /// no end of its own takes no ceiling either: the container waits for whatever will end it
    /// from outside, up to the run's own deadline (see the module docs).
    async fn run_sleep(&mut self, arguments: &[String]) -> i32 {
        let Some(argument) = arguments.first() else {
            return EXIT_FAILURE;
        };
        let waiting = if argument == "infinity" || argument == "inf" {
            None
        } else {
            // `try_from_secs_f64` rather than the panicking constructor: the argument is a
            // caller's, so a value no duration can hold is the utility's usage failure
            // rather than this session's.
            match argument
                .parse::<f64>()
                .ok()
                .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            {
                // The span the container is there for, bounded where this emulator bounds it.
                Some(waiting) => Some(match self.wait_ceiling_for(waiting) {
                    Some(ceiling) => waiting.min(ceiling),
                    None => waiting,
                }),
                None => return EXIT_FAILURE,
            }
        };
        if self.wait_for(waiting).await {
            // "If sleep receives a signal, it shall exit with a status greater than zero"
            // — the status a signal gives a command.
            return EXIT_SIGNAL_BASE + SIGTERM_NUMBER;
        }
        EXIT_SUCCESS
    }

    /// Write `text` to the container's output, delivering every COMPLETE line in it.
    ///
    /// The `awslogs` driver delivers one log event per line, so a write that does not end a
    /// line waits for the write that does — up to [`MAX_PENDING_LINE_BYTES`], past which
    /// what has been written is delivered as the line it is.
    async fn write_output(&mut self, text: &str) {
        for character in text.chars() {
            if character == '\n' {
                self.flush_pending_line().await;
                continue;
            }
            self.pending_line.push(character);
            if self.pending_line.len() >= MAX_PENDING_LINE_BYTES {
                self.flush_pending_line().await;
            }
        }
    }

    /// Deliver the line being assembled, if there is one.
    async fn flush_pending_line(&mut self) {
        if self.pending_line.is_empty() {
            return;
        }
        let line = ::std::mem::take(&mut self.pending_line);
        self.output.write_line(&line).await;
    }

    /// Write `text` to the standard OUTPUT of the command being performed: to the file a
    /// redirection names, to the capture an open substitution or pipeline stage is
    /// collecting, or — when neither — to the container's own output.
    ///
    /// `false` is a write that did not happen, which is the failure the command performing it
    /// reports: a write the filesystem refused, or a capture past its bound.
    async fn write_stdout(&mut self, text: &str, redirections: &Redirections) -> bool {
        if redirections.out_to_error {
            // `1>&2`: this command's output goes where its diagnostics go.
            return self.write_error_stream(text, redirections).await;
        }
        self.write_stream(redirections.out.clone(), text, redirections)
            .await
    }

    /// Write `text` to the standard ERROR of the command being performed, which is where a
    /// command reports what it could not do.
    ///
    /// With a redirection of its own it goes to the file that names. With none it is
    /// DELIVERED to the container's output: the log driver "sends container logs" — both of
    /// the streams a container writes — so a diagnostic reaches the container's log stream
    /// exactly as its output does. What it is never part of is a command SUBSTITUTION's
    /// value, which captures standard output alone.
    async fn write_stderr(&mut self, text: &str, redirections: &Redirections) -> bool {
        if redirections.error_to_out {
            // `2>&1`: this command's diagnostics go where its output goes, so they are what
            // a redirection takes and what a substitution captures.
            return self
                .write_stream(redirections.out.clone(), text, redirections)
                .await;
        }
        self.write_error_stream(text, redirections).await
    }

    /// [`Self::write_stderr`]'s write to the ERROR stream itself, with no duplication
    /// applied: the file a redirection of standard error names, and otherwise the container's
    /// own output.
    async fn write_error_stream(&mut self, text: &str, redirections: &Redirections) -> bool {
        if let Some((path, _)) = redirections.error.clone() {
            // The file was opened before the command ran, so each write ADDS to it.
            if self.filesystem.write(&path, text, true).is_err() {
                self.redirect_failed = true;
                return false;
            }
            return true;
        }
        self.write_output(text).await;
        true
    }

    /// [`Self::write_stdout`]'s and [`Self::write_stderr`]'s one write.
    async fn write_stream(
        &mut self,
        target: Option<(String, bool)>,
        text: &str,
        redirections: &Redirections,
    ) -> bool {
        if let Some((path, _)) = target {
            if let Err(refusal) = self.filesystem.write(&path, text, true) {
                // The diagnostic cannot go through a duplication onto the very stream whose
                // write just failed, so it is reported on the error stream itself.
                let reported = format!(
                    "{SHELL_NAME}: can't create {path}: {}\n",
                    refusal.message()
                );
                self.write_error_stream(&reported, redirections).await;
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
        self.write_output(text).await;
        true
    }

    /// What the command being performed reads from STANDARD INPUT, and `None` when there is
    /// none to read.
    ///
    /// The TEXT written inside the command, when it carries a here-document or a here-string;
    /// otherwise the file an input redirection names, and otherwise what the stage before it
    /// in a pipeline wrote. It is TAKEN: a stream is consumed by whoever reads it, so the
    /// command after it in the same pipeline reads nothing rather than the same bytes twice.
    /// There is no terminal behind this: a command with none of the three reads nothing at
    /// all.
    fn standard_input(&mut self, redirections: &Redirections) -> Option<String> {
        if let Some(text) = redirections.input_text.clone() {
            return Some(text);
        }
        if let Some(path) = redirections.input.clone() {
            return self.filesystem.read(&path).ok();
        }
        self.piped_input.take()
    }

    /// [`Self::standard_input`] for the utilities that READ it, with the status to answer
    /// when there is nothing to read: the utility's own failure for a file standard input was
    /// redirected from that cannot be read — reported on standard error in the utility's own
    /// wording — and a command that could not run when there was no input at all, since there
    /// is no terminal behind it here.
    async fn read_standard_input(
        &mut self,
        name: &str,
        redirections: &Redirections,
    ) -> Result<String, i32> {
        let redirected = redirections.input.clone();
        match self.standard_input(redirections) {
            Some(input) => Ok(input),
            None => match redirected {
                Some(path) => {
                    let refusal = self
                        .filesystem
                        .read(&path)
                        .err()
                        .unwrap_or(FileRefusal::NotFound);
                    self.write_stderr(
                        &format!("{name}: {path}: {}\n", refusal.message()),
                        redirections,
                    )
                    .await;
                    Err(EXIT_FAILURE)
                }
                None => Err(self.declined()),
            },
        }
    }

    /// Run one of the commands that work over the container's FILESYSTEM
    /// ([`FILE_COMMANDS`]), and write what it produced through this session's own streams.
    ///
    /// WHAT each of them does and SAYS is the shared reading of those utilities
    /// ([`::service_common::posix_shell_files::perform_file_command`]), performed over this
    /// container's own filesystem; what this session adds is where its two streams go and the
    /// standard input only it can resolve — the file one was redirected from, or what the
    /// stage before it in a pipeline wrote, which is what `cat` with no operand reads.
    fn run_file_command<'a>(
        &'a mut self,
        name: &'a str,
        arguments: &'a [String],
        redirections: &'a Redirections,
    ) -> ShellFuture<'a, i32> {
        Box::pin(async move {
            let reads_input = name == "cat"
                && !::service_common::posix_shell_files::names_an_operand(arguments);
            let input = if reads_input {
                match self.read_standard_input(name, redirections).await {
                    Ok(input) => Some(input),
                    Err(status) => return status,
                }
            } else {
                None
            };
            let Some(outcome) = ::service_common::posix_shell_files::perform_file_command(
                name,
                arguments,
                &mut self.filesystem,
                input.as_deref(),
            ) else {
                return self.declined();
            };
            if !outcome.errors.is_empty() {
                self.write_stderr(&outcome.errors, redirections).await;
            }
            if !outcome.output.is_empty()
                && !self.write_stdout(&outcome.output, redirections).await
            {
                return EXIT_FAILURE;
            }
            if outcome.succeeded {
                EXIT_SUCCESS
            } else {
                EXIT_FAILURE
            }
        })
    }

    /// Perform one of the TEXT FILTERS over its one input: the file it names, or standard
    /// input.
    ///
    /// What it writes is what [`::service_common::posix_shell_text_filters`] computed over
    /// those bytes — a count, a selection, a digest — and nothing of the invocation is
    /// interpreted here. An option or operand form that module does not perform is a command
    /// this container could not run, a file this filesystem will not read is reported the way
    /// the file commands report one, and a filter with no input at all could not run either:
    /// there is no terminal behind standard input here.
    ///
    /// The one option read HERE is `tail`'s FOLLOW ([`follow_free_arguments`]): what it asks for
    /// is not a computation over text but a program that does not END, which only a session with
    /// a clock can perform.
    async fn run_text_filter(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        use ::service_common::posix_shell_text_filters::{Filter, STANDARD_INPUT_LABEL};
        // "tail -f": the lines it has are written, and then the utility goes on waiting.
        let followed = follow_free_arguments(name, arguments);
        let read_as = followed.as_deref().unwrap_or(arguments);
        let Some(filter) = Filter::read(name, read_as) else {
            return self.declined();
        };
        let (label, input) = match filter.file() {
            Some(path) => match self.filesystem.read(path) {
                Ok(held) => (path.to_string(), held),
                Err(refusal) => {
                    self.write_stderr(
                        &format!("{name}: {path}: {}\n", refusal.message()),
                        redirections,
                    )
                    .await;
                    return EXIT_FAILURE;
                }
            },
            None => match self.read_standard_input(name, redirections).await {
                Ok(input) => (STANDARD_INPUT_LABEL.to_string(), input),
                Err(status) => return status,
            },
        };
        let written = match filter.perform(&label, &input) {
            Ok(performed) => performed,
            // An input the utility cannot compute over: what it writes about it goes to
            // standard error, and nothing is written to standard output.
            Err(refusal) => {
                self.write_stderr(&format!("{refusal}\n"), redirections).await;
                return EXIT_FAILURE;
            }
        };
        if !self.write_stdout(&written.written, redirections).await {
            return EXIT_FAILURE;
        }
        // A FOLLOW does not end where the input does: "tail shall not terminate after the last
        // line of the input file has been copied, but shall read and copy further bytes from the
        // input file when they become available" — so what the program does next is nothing, and
        // the container is up until it is stopped or this run's own bound ends it, exactly as a
        // wait with no end of its own leaves it ([`Self::run_sleep`]).
        if followed.is_some() && self.wait_for(None).await {
            // "If sleep receives a signal, it shall exit with a status greater than zero" —
            // the status a signal gives a command, which is what ends this one too.
            return EXIT_SIGNAL_BASE + SIGTERM_NUMBER;
        }
        // The status of a utility that reports a SELECTION is that selection
        // (`posix_shell_text_filters::Selection`): a `grep` of this container's filesystem
        // that selected nothing answers the status an `if` around it branches on.
        if written.selected == Some(false) {
            return EXIT_FAILURE;
        }
        EXIT_SUCCESS
    }

    /// Perform the container's `aws` command: the transfers and the publish of
    /// [`super::container_aws_cli`], against the account's own services as the task role.
    ///
    /// The invocation itself is read by the shared reading of that program, so an invocation
    /// it performs nothing of — another command group, another command, an option, an operand
    /// form — is a command this container could not run ([`Self::declined`]) rather than a
    /// transfer, and so is an `aws` command in a session the container's environment provides
    /// no CLI to. What the command WROTE goes through this session's own streams: the
    /// transfer's report on standard output, and the account of one that was not made on
    /// standard error, which is where the CLI writes it.
    ///
    /// Standard input is TAKEN only for the one invocation that reads it — an upload from the
    /// CLI's stream operand — so every other command leaves the stream to whoever reads it.
    async fn run_aws_cli(&mut self, arguments: &[String], redirections: &Redirections) -> i32 {
        let Some(cli) = self.aws.clone() else {
            return self.declined();
        };
        let input = if ::service_common::aws_s3_cli::reads_standard_input(arguments) {
            self.standard_input(redirections)
        } else {
            None
        };
        let performed = cli
            .perform(arguments, input.as_deref(), &mut self.filesystem)
            .await;
        let Some(outcome) = performed else {
            return self.declined();
        };
        if !outcome.errors.is_empty() {
            self.write_stderr(&outcome.errors, redirections).await;
        }
        if !outcome.output.is_empty() && !self.write_stdout(&outcome.output, redirections).await {
            return EXIT_FAILURE;
        }
        outcome.status
    }

    /// Perform one request of the container's HTTP client: the fetch of
    /// [`super::container_http_client`], answered by a server of this container's own task or of
    /// another workload the account's network delivers to it.
    ///
    /// The invocation is read by the shared reading of that client, so one it performs nothing
    /// of — a host this container has no network to reach, a scheme it makes no request over, an
    /// option form that reading does not read — is a command this container could not run
    /// ([`Self::declined`]), and so is such a request in a session whose task this emulator
    /// holds no address for. What was FETCHED goes through this session's own streams, or to the
    /// file the invocation asked it to be saved at — and a save that did not happen is the
    /// status that client reports for one. What the invocation asked the client to WRITE once
    /// the transfer is over goes to this session's own output whatever became of the document,
    /// which is what makes `-o /dev/null -w '%{http_code}'` report the status alone.
    async fn run_http_client(
        &mut self,
        name: &str,
        arguments: &[String],
        redirections: &Redirections,
    ) -> i32 {
        let Some(client) = self.http.clone() else {
            return self.declined();
        };
        let Some(fetched) = client.fetch(name, arguments) else {
            return self.declined();
        };
        let mut status = fetched.status;
        if let Some(document) = fetched.document {
            match fetched.save_to {
                Some(path) => {
                    if self.filesystem.write(&path, &document, false).is_err() {
                        return ::service_common::instance_metadata_service::WRITE_ERROR;
                    }
                }
                None => {
                    if !self.write_stdout(&document, redirections).await {
                        return EXIT_FAILURE;
                    }
                }
            }
        }
        if let Some(written) = fetched.written_out {
            if !self.write_stdout(&written, redirections).await {
                status = EXIT_FAILURE;
            }
        }
        status
    }

    /// `word` with its expansions resolved, or `None` when it carries an expansion this
    /// container cannot resolve.
    ///
    /// The pieces come from `brush-parser`'s word parser, so what is quoted, what is a
    /// parameter expansion, what is a command substitution and what is literal text is the
    /// shell's own reading of the word. A parameter this session holds nothing for expands
    /// to nothing, which is what an unset parameter expands to, and an escape sequence
    /// stands for the character it escapes ([`escaped_text`]). An arithmetic expansion or a
    /// tilde prefix is an expansion whose value this container cannot resolve — it would have
    /// to resolve a home directory it does not have — so the word has no expansion here and
    /// the command carrying it is not run. Nor does a word that expands past
    /// [`MAX_EXPANSION_BYTES`], or one whose expansion would take the command past
    /// [`MAX_LINE_EXPANSION_BYTES`].
    fn expand_word<'a>(&'a mut self, word: &str) -> ShellFuture<'a, Option<String>> {
        self.expand_word_at(word, MAX_WORD_DEPTH)
    }

    /// [`Self::expand_word`] with `depth` levels of nesting LEFT rather than the whole budget,
    /// for the expansion of a word that stands INSIDE another one: the text of an arithmetic
    /// expansion is itself expanded ([`Self::expand_arithmetic`]), so that nested expansion
    /// continues this walk's descent instead of starting a fresh one — which is what keeps the
    /// recursion of a word nesting arithmetic inside arithmetic inside [`MAX_WORD_DEPTH`].
    fn expand_word_at<'a>(&'a mut self, word: &str, depth: usize) -> ShellFuture<'a, Option<String>> {
        let word = word.to_string();
        Box::pin(async move {
            let options = ::brush_parser::ParserOptions::default();
            let pieces = ::brush_parser::word::parse(&word, &options).ok()?;
            let expanded = self.expand_pieces(pieces, depth, false).await?;
            self.expansion_left = self.expansion_left.checked_sub(expanded.len())?;
            Some(expanded)
        })
    }

    /// [`Self::expand_word`]'s walk over the pieces a word parsed into, with `depth` levels
    /// of quoting left: a word nested past [`MAX_WORD_DEPTH`] has no expansion here, so
    /// this walk's own recursion stays in range whatever the word nests.
    ///
    /// `double_quoted` is whether these pieces stand INSIDE a double-quoted sequence, which
    /// is what decides how an escape sequence reads ([`escaped_text`]).
    fn expand_pieces(
        &mut self,
        pieces: Vec<::brush_parser::word::WordPieceWithSource>,
        depth: usize,
        double_quoted: bool,
    ) -> ShellFuture<'_, Option<String>> {
        Box::pin(async move {
            let depth = depth.checked_sub(1)?;
            let mut expanded = String::new();
            for piece in pieces {
                let addition = match piece.piece {
                    ::brush_parser::word::WordPiece::Text(text)
                    | ::brush_parser::word::WordPiece::SingleQuotedText(text) => text,
                    ::brush_parser::word::WordPiece::DoubleQuotedSequence(inner) => {
                        self.expand_pieces(inner, depth, true).await?
                    }
                    ::brush_parser::word::WordPiece::ParameterExpansion(parameter) => {
                        self.expand_parameter(&parameter)?
                    }
                    ::brush_parser::word::WordPiece::CommandSubstitution(program)
                    | ::brush_parser::word::WordPiece::BackquotedCommandSubstitution(program) => {
                        self.expand_substitution(&program).await?
                    }
                    ::brush_parser::word::WordPiece::EscapeSequence(sequence) => {
                        escaped_text(&sequence, double_quoted)?
                    }
                    ::brush_parser::word::WordPiece::ArithmeticExpression(expression) => {
                        // `depth` and not the whole budget: the expression's own text is
                        // expanded, and that expansion is one level further down THIS walk.
                        self.expand_arithmetic(&expression.value, depth).await?
                    }
                    _ => return None,
                };
                if expanded.len().saturating_add(addition.len()) > MAX_EXPANSION_BYTES {
                    return None;
                }
                expanded.push_str(&addition);
            }
            Some(expanded)
        })
    }

    /// The value of a command SUBSTITUTION: what the commands inside it wrote to standard
    /// output, "with one or more trailing newline characters removed", as the shell
    /// specifies.
    ///
    /// The commands run in THIS session — the same variables, the same filesystem, the same
    /// budget and the same catalogs — so a substitution reaches nothing its session cannot.
    /// It is a SUBSHELL: an `exit` inside one ends the substitution rather than the session,
    /// and the variables it set or removed are put back off the undo log it recorded
    /// ([`Self::record_for_rollback`]), which costs what it changed rather than a copy of the
    /// store.
    ///
    /// Its own cost comes out of the same budgets: performing one spends a step of
    /// [`MAX_COMMANDS_PER_RUN`] before anything else — so how many a command may open is
    /// bounded whatever their text performs — and the commands inside it spend their own.
    ///
    /// `None` is a substitution with no value: one past [`MAX_SUBSTITUTIONS`],
    /// [`MAX_SUBSTITUTION_DEPTH`] or the run's budget does not run, and one whose output went
    /// past [`MAX_CAPTURED_BYTES`] has no value rather than a clipped one.
    fn expand_substitution<'a>(&'a mut self, program: &str) -> ShellFuture<'a, Option<String>> {
        let program = program.to_string();
        Box::pin(async move {
            if self.substitutions_left == 0
                || self.substitution_undo.len() >= MAX_SUBSTITUTION_DEPTH
            {
                return None;
            }
            if !self.take_command_budget().await {
                return None;
            }
            self.substitutions_left = self.substitutions_left.saturating_sub(1);
            self.captures.push(Capture {
                limit: MAX_CAPTURED_BYTES,
                ..Capture::default()
            });
            self.substitution_undo.push(BTreeMap::new());
            let held_exit = self.exit_status.take();
            let held_bytes = self.held_bytes;
            let status = self.run_text(&program).await;
            self.exit_status = held_exit;
            for (name, held) in self.substitution_undo.pop().unwrap_or_default() {
                match held {
                    Some(value) => self.variables.insert(name, value),
                    None => self.variables.remove(&name),
                };
            }
            // The store is back to what it was, so what it holds is back to what it held.
            self.held_bytes = held_bytes;
            // The LAST substitution performed is the one a command with no command name
            // answers with, so this replaces what a substitution performed earlier in the
            // same command left.
            self.last_substitution_status = Some(status);
            let capture = self.captures.pop()?;
            if capture.overflowed {
                return None;
            }
            Some(capture.text.trim_end_matches('\n').to_string())
        })
    }

    /// The value of an ARITHMETIC expansion (`$((…))`): the number its expression comes to in
    /// this session.
    ///
    /// The text's own parameter expansions are resolved first — `$((count + 1))` and
    /// `$(($count + 1))` are both written — and then the expression is parsed by
    /// `brush-parser`'s arithmetic grammar and evaluated over this session's variables by this
    /// workspace's one reading of shell arithmetic
    /// ([`::service_common::posix_shell_arithmetic`]), so an expansion that ASSIGNS sets the
    /// variable of this session.
    ///
    /// `None` is an expansion with no value, which makes the word carrying it one this
    /// container does not perform: text past the bound a parse is given
    /// ([`within_parse_bounds`], applied to what the expansion materialised rather than only
    /// to the line it came from), an expression nested past the `depth` its word has left,
    /// one that does not parse, one that module does not
    /// evaluate, and one whose arithmetic has no answer (a division by zero, an overflow).
    ///
    /// `depth` is how many levels of nesting the word carrying this expansion has left
    /// ([`Self::expand_word_at`]): resolving the text is a step DOWN that walk, so an
    /// expansion written inside another one spends a level rather than beginning again, and
    /// this recursion ends whatever the text nests.
    fn expand_arithmetic<'a>(
        &'a mut self,
        text: &str,
        depth: usize,
    ) -> ShellFuture<'a, Option<String>> {
        let text = text.to_string();
        Box::pin(async move {
            let resolved = self
                .expand_word_at(&text, depth)
                .await
                .unwrap_or_else(|| text.clone());
            // The expansion above can have MATERIALISED text far longer and far more deeply
            // nested than the line itself — a variable's value expands inside the expression
            // — so the same bound a program is read under is applied to it before it reaches
            // a grammar.
            if !within_parse_bounds(&resolved) {
                return None;
            }
            let expression = ::brush_parser::arithmetic::parse(&resolved).ok()?;
            let value =
                ::service_common::posix_shell_arithmetic::evaluate(&expression, MAX_WORD_DEPTH, self)?;
            Some(value.to_string())
        })
    }

    /// The value a parameter expansion resolves to in this session.
    ///
    /// The plain form (`$NAME`, `${NAME}`) is the variable's value, empty when the session
    /// holds none — unless the program asked to be ended by exactly that (`set -u`), in which
    /// case the expansion has no value and the session ends with the shell's own diagnostic
    /// ([`Self::word_failure`]). The default-value form is the value or the default the
    /// expansion names, and WHICH values take the default is the form's own: `${NAME:-WORD}`
    /// substitutes it for a parameter that is unset or empty, `${NAME-WORD}` only for one that
    /// is unset, so a variable the container's environment sets to the empty string keeps it
    /// under the second form — and neither form reads an unset parameter as an error, because
    /// each STATES what to use instead. The length form (`${#NAME}`) is how many characters
    /// that value has. The POSITIONAL parameters are the operands the program was invoked with
    /// ([`script_positional_parameters`]): `$1`, `$2` and so on are those arguments, and one the
    /// invocation gave nothing at expands to nothing, the way an unset parameter does. The
    /// SPECIAL parameters a session answers about itself are resolved too: `$?`, "the decimal
    /// exit status of the most recent pipeline", `$0`, "the name of the shell or shell script",
    /// `$#`, "the number of positional parameters", and `$@`/`$*`, those parameters — joined by
    /// one space, since field splitting is a form this session does not perform. Every other
    /// form — a substring, a replacement, an indirection or another special parameter — is one
    /// this container does not resolve.
    fn expand_parameter(
        &mut self,
        parameter: &::brush_parser::word::ParameterExpr,
    ) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => {
                let held = self.variables.get(name).map(String::as_str);
                if held.is_none() && self.nounset {
                    self.unset_parameter = Some(name.clone());
                    return None;
                }
                Some(held.unwrap_or_default().to_string())
            }
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter:
                    ::brush_parser::word::Parameter::Special(
                        ::brush_parser::word::SpecialParameter::LastExitStatus,
                    ),
                indirect: false,
            } => {
                // "?: Expands to the decimal exit status of the most recent pipeline", which
                // is how a program reads the status of the command before it.
                Some(self.last_status.to_string())
            }
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter:
                    ::brush_parser::word::Parameter::Special(
                        ::brush_parser::word::SpecialParameter::ShellName,
                    ),
                indirect: false,
            } => {
                // "0: Expands to the name of the shell or shell script", which is the name
                // this container's program was invoked under — the one its invocation gave it,
                // and the shell's own for a program invoked with no name at all.
                Some(
                    self.positional
                        .first()
                        .cloned()
                        .unwrap_or_else(|| SHELL_NAME.to_string()),
                )
            }
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Positional(position),
                indirect: false,
            } => {
                // "$1, $2, …": the arguments the program was invoked with, which stand after
                // the name at index 0.
                Some(
                    self.positional
                        .get(*position as usize)
                        .cloned()
                        .unwrap_or_default(),
                )
            }
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter:
                    ::brush_parser::word::Parameter::Special(
                        ::brush_parser::word::SpecialParameter::PositionalParameterCount,
                    ),
                indirect: false,
            } => {
                // "#: Expands to the decimal number of positional parameters", which counts the
                // arguments and not the name the program was invoked under.
                Some(self.positional.len().saturating_sub(1).to_string())
            }
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter:
                    ::brush_parser::word::Parameter::Special(
                        ::brush_parser::word::SpecialParameter::AllPositionalParameters { .. },
                    ),
                indirect: false,
            } => {
                // "@" and "*": the positional parameters, joined by one space — this session
                // performs no field splitting, so both forms expand to the same word.
                Some(
                    self.positional
                        .iter()
                        .skip(1)
                        .cloned()
                        .collect::<Vec<String>>()
                        .join(" "),
                )
            }
            ::brush_parser::word::ParameterExpr::UseDefaultValues {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
                test_type,
                default_value,
            } => {
                let held = self.variables.get(name).map(String::as_str);
                let defaulted = match test_type {
                    // `${NAME:-WORD}`: unset OR null takes the default.
                    ::brush_parser::word::ParameterTestType::UnsetOrNull => {
                        held.is_none_or(str::is_empty)
                    }
                    // `${NAME-WORD}`: only unset takes it.
                    ::brush_parser::word::ParameterTestType::Unset => held.is_none(),
                };
                if defaulted {
                    Some(default_value.clone().unwrap_or_default())
                } else {
                    Some(held.unwrap_or_default().to_string())
                }
            }
            ::brush_parser::word::ParameterExpr::ParameterLength {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => {
                // "${#parameter}: String Length. The length in characters of the value of
                // parameter shall be substituted." A parameter this session holds nothing
                // for has no value, so its length is zero.
                let held = self
                    .variables
                    .get(name)
                    .map(String::as_str)
                    .unwrap_or_default();
                Some(held.chars().count().to_string())
            }
            _ => None,
        }
    }
}

/// The variables an arithmetic expansion of this container's session reads and assigns: the
/// session's own, under the session's own bounds — an assignment the session will not take
/// (it is at [`MAX_SESSION_BYTES`] or [`MAX_SESSION_VARIABLES`]) is one the expansion could
/// not perform.
impl ::service_common::posix_shell_arithmetic::ArithmeticVariables for ContainerShell {
    fn arithmetic_value(&self, name: &str) -> Option<String> {
        self.variables.get(name).cloned()
    }

    fn set_arithmetic_value(&mut self, name: &str, value: i64) -> bool {
        self.set_variable(name.to_string(), value.to_string())
    }
}

/// The name a `trap` signal operand is recorded under: the spelling the program wrote,
/// upper-cased, so `term`, `TERM` and `SIGTERM` are the same signal to this session.
fn signal_key(operand: &str) -> String {
    operand.to_ascii_uppercase()
}

/// The text ONE escape sequence of a word stands for, and `None` for a sequence this
/// container does not read.
///
/// A backslash is the shell's escape character, and WHICH characters it escapes depends on
/// where it stands: unquoted, "the backslash shall preserve the literal value of the
/// following character", while inside double quotes it does so only before `$`, `` ` ``,
/// `"`, `\` and a newline — before anything else the backslash itself is literal. A
/// backslash before a newline is a line continuation either way, so it stands for nothing.
fn escaped_text(sequence: &str, double_quoted: bool) -> Option<String> {
    // The piece is the sequence as the word was written, so the backslash may or may not
    // still be on it; what it escapes is the one character after it either way.
    let escaped = sequence.strip_prefix('\\').unwrap_or(sequence);
    let mut characters = escaped.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        // More than one character after the backslash is no escape sequence this reads.
        return None;
    }
    match character {
        '\n' => Some(String::new()),
        '\\' | '"' | '$' | '`' => Some(character.to_string()),
        // Unquoted, the backslash preserves whatever follows it; inside double quotes it
        // keeps its own literal value before every other character.
        _ if !double_quoted => Some(character.to_string()),
        _ => Some(format!("\\{character}")),
    }
}

/// Whether the shell PATTERN `pattern` matches `value`, and `None` for a pattern this
/// container does not read.
///
/// A `case` pattern is a shell pattern: "an asterisk is a pattern that shall match any
/// string, including the null string" and a question mark matches any one character — the two
/// wildcards this workspace's own matcher walks
/// ([`::service_common::iam_authorization::wildcard_match`], whose single-pass walk costs the
/// product of the two lengths and never recurses, so a pattern a caller wrote cannot make one
/// match run away). The comparison is case-sensitive, as the shell's is.
///
/// A BRACKET EXPRESSION (`[abc]`, `[!a-z]`) is a third pattern construct that matcher does
/// not read, so a pattern carrying one is not matched here and the command carrying it did not
/// run — the fail-closed answer this session gives every other form it does not perform.
///
/// The pattern arrives EXPANDED, so a wildcard character that reached it through a quote or an
/// escape is read as the wildcard it spells rather than as the literal character the shell
/// would make of it.
fn case_pattern_matches(pattern: &str, value: &str) -> Option<bool> {
    if pattern.contains(BRACKET_EXPRESSION_OPEN) {
        return None;
    }
    Some(::service_common::iam_authorization::wildcard_match(
        pattern, value, true,
    ))
}

/// What opens a bracket expression in a shell pattern, which is the construct
/// [`case_pattern_matches`] does not read.
const BRACKET_EXPRESSION_OPEN: char = '[';
