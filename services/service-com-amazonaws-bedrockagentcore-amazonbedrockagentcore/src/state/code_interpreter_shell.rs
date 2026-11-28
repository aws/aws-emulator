// SPDX-License-Identifier: CC0-1.0

//! The COMMAND plane of a code interpreter session: the shell its `executeCommand` and
//! `startCommandExecution` tools perform a caller's command line in.
//!
//! The command is not read here: it is performed by the one bounded, session-local POSIX
//! shell this workspace drives for every service whose API carries caller-supplied shell text
//! (`service_common::posix_shell_session`), which tokenizes and parses the line with the
//! POSIX/bash grammar and performs its commands in this process. No host process is started,
//! no host `PATH` is resolved and no host file is opened: the only thing a command can reach
//! is the session's own workspace, handed to the shell as its working tree
//! ([`super::SandboxFilesystem`]) and taken back out of it when the command ends — so the
//! shell, the file tools and the code a run executes share ONE filesystem, and a command
//! cannot see another session's.
//!
//! What the shell answers is what the result reports: the lines the command wrote to standard
//! output, the lines it wrote to standard error, and the status it exited with — `0` for a
//! command that succeeded, the shell's own status for one that failed, and the status a shell
//! answers for a command it could not run when the program a line names is neither one of the
//! shell's own utilities nor one of the READ-ONLY utilities this sandbox has installed
//! ([`super::SandboxPrograms`]).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ::service_common::posix_shell_session::{
    stream_text, Bounds, OutputLimits, SessionConfig, ShellSession, Statuses,
};

use super::{SandboxFilesystem, SandboxPrograms, MAX_SESSION_FILES, MAX_SESSION_FILE_BYTES};

/// The most lines of one stream a command's result carries, and the most bytes of one line.
///
/// EMULATOR bounds, not modelled ones: the result travels in one response, so what a one-line
/// command can make this process hold and send is bounded here rather than by the command.
const MAX_OUTPUT_LINES: usize = 2_048;
/// See [`MAX_OUTPUT_LINES`].
const MAX_OUTPUT_LINE_BYTES: usize = 8 * 1024;

/// How many bytes of MEMORY one command of this sandbox may hold at once.
///
/// An EMULATOR bound like the two above, and for the same reason: the command line is the
/// caller's own, so what performing it can make this process hold is decided here. The
/// session's other ceilings bound other resources — its working tree
/// ([`MAX_SESSION_FILE_BYTES`]) and what reaches the two result streams
/// ([`MAX_OUTPUT_LINES`]) — while this one bounds the values the commands themselves hold, and
/// a command that would go past it is KILLED where it stands
/// (`ShellSession::killed_for_memory`) rather than left to allocate on.
///
/// Stated above the bytes of variables the shared session holds anyway
/// ([`Bounds::session_bytes`]), so it is this process's own last ceiling on the run rather
/// than a second, lower limit on the same values.
const MAX_SESSION_MEMORY_BYTES: usize = 8 * 1024 * 1024;

/// What one command run produced: the two streams, the status it ended with, how long the
/// shell spent on it and how long the command itself STATED it took.
#[derive(Debug, Clone)]
pub struct CommandRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// The shell's own time on this command, which is part of what a result reports as its
    /// `executionTime` — not how long the call around it took.
    pub elapsed: Duration,
    /// What the command's own `sleep` commands stated it took
    /// (`ShellSession::slept`). No thread waited for it: the CALLER honours it — a
    /// synchronous tool by answering no sooner, and a submitted task by not converging
    /// before it has passed — which is what makes a command that waits take that time.
    pub slept: Duration,
}

/// Perform `command` in `sandbox`, the session's own workspace.
///
/// The workspace is seeded into the shell's working tree before the command runs and the tree
/// is reflected back into it afterwards, so a file the command wrote is a file the session's
/// other two views read and a file it removed is gone from them. A workspace file whose bytes
/// are not text is left out of the tree and untouched by the reflection: a shell reads and
/// writes text, so the only files a command can address are the text ones.
pub fn run_command(command: &str, sandbox: &mut SandboxFilesystem) -> CommandRun {
    let started = Instant::now();
    let mut session = ShellSession::with_programs(
        BTreeMap::new(),
        SessionConfig {
            statuses: Statuses::default(),
            bounds: Bounds {
                // The tree IS the session workspace, so what it may hold is what the
                // workspace may hold; every other bound is the shared session's own.
                files: MAX_SESSION_FILES,
                file_bytes: MAX_SESSION_FILE_BYTES,
                tree_bytes: MAX_SESSION_FILE_BYTES,
                ..Bounds::default()
            },
            output: Some(OutputLimits {
                lines: MAX_OUTPUT_LINES,
                line_bytes: MAX_OUTPUT_LINE_BYTES,
            }),
            // What performing the caller's command line may make this process HOLD
            // ([`MAX_SESSION_MEMORY_BYTES`]): the sandbox's modelled environment states no
            // memory of its own, so the ceiling is this emulator's.
            memory: Some(MAX_SESSION_MEMORY_BYTES),
        },
        // The programs a code interpreter sandbox has installed beside the shell's own
        // utilities ([`super::SandboxPrograms`]), every one of which reports on this same
        // tree.
        Box::new(SandboxPrograms),
    )
    // The environment this command runs in HAS storage of its own — the session's
    // workspace — so the file utilities are commands the shell performs over that tree.
    .with_file_utilities();

    // What the shell was seeded with, so the reflection below can tell a file the command
    // wrote from one it only read.
    let mut seeded: BTreeMap<String, String> = BTreeMap::new();
    for (key, bytes) in sandbox.files() {
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if session.put_file(&key, &text) {
            seeded.insert(key, text);
        }
    }

    let status = session.run(command);
    let exit_code = session.exit_status().unwrap_or(status);
    let elapsed = started.elapsed();

    // The tree as the command left it, back into the session's workspace.
    let mut left: BTreeMap<String, String> = BTreeMap::new();
    for (path, contents) in session.tree_files() {
        left.insert(path.to_string(), contents.to_string());
    }
    for (path, contents) in &left {
        if seeded.get(path).is_some_and(|held| held == contents) {
            continue;
        }
        let _ = sandbox.write(path, contents.as_bytes().to_vec());
    }
    for path in seeded.keys() {
        if !left.contains_key(path) {
            let _ = sandbox.remove(path);
        }
    }

    CommandRun {
        stdout: stream_text(session.output()),
        stderr: stream_text(session.errors()),
        exit_code,
        elapsed,
        slept: session.slept(),
    }
}
