// SPDX-License-Identifier: CC0-1.0

//! The build environment's SHELL: what ONE command line of a buildspec does.
//!
//! # The rule this module implements
//!
//! A build's phase commands are command LINES, which CodeBuild hands to the shell of the
//! build environment; what the build reports for a phase is the status that shell answered
//! with — zero for a command that ran to success, non-zero otherwise — so the outcome of a
//! build follows from running its own commands and from nothing else.
//!
//! # What runs them
//!
//! [`::service_common::posix_shell_session`], the workspace's ONE bounded, session-local
//! POSIX/bash session: the grammar is `brush-parser`'s, the catalog of utilities it performs
//! is session-local, every other command and every command FORM and expansion it does not
//! perform fails closed with the status a shell answers for a command it cannot run, and all
//! of its bounds — the command budget, the line size, the nesting count, the expansion sizes
//! and the variable store — are stated there. That module also states why the project's
//! EXECUTING half, `brush-core`, is not a dependency of this workspace.
//!
//! A build's session is that session with the statuses POSIX gives those conditions, the
//! shared bounds, and NO output kept: the bytes a command writes are not delivered anywhere
//! — a build's log group is its own concern — so this shell keeps none of them. What a
//! performed command contributes to the build is its exit status, the variables it sets (a
//! buildspec's `env/exported-variables` are read back off this session) and the files it
//! writes into the session's own working tree, which the commands after it read, and which
//! is what [`BuildShell`] exposes.
//!
//! The commands the build ENVIRONMENT provides beyond the shell's own utilities are the
//! programs of the runtimes this build installed ([`super::runtime_versions`]), which the
//! session asks for a command its catalog does not name.

use std::collections::BTreeMap;

/// The session one build's commands run in: the shared session, configured as a build's,
/// with the programs the runtimes this build installed provide.
#[derive(Debug)]
pub(super) struct BuildShell {
    session: ::service_common::posix_shell_session::ShellSession,
}

impl BuildShell {
    /// A session holding `variables`, with `installed`'s programs and the build's whole
    /// command budget left.
    pub(super) fn new(
        variables: BTreeMap<String, String>,
        installed: super::InstalledRuntimes,
    ) -> Self {
        Self {
            session: ::service_common::posix_shell_session::ShellSession::with_programs(
                variables,
                // The shared statuses and bounds, and no output: see this module's header.
                ::service_common::posix_shell_session::SessionConfig::default(),
                Box::new(installed),
            ),
        }
    }

    /// The value this session holds for `name`, or `None` when it holds none.
    pub(super) fn variable(&self, name: &str) -> Option<&str> {
        self.session.variable(name)
    }

    /// The status `exit` ended this session with, or `None` while nothing has.
    pub(super) fn exit_status(&self) -> Option<i32> {
        self.session.exit_status()
    }

    /// Run one command LINE and answer the status the shell answered for it.
    pub(super) fn run(&mut self, line: &str) -> i32 {
        self.session.run(line)
    }
}
