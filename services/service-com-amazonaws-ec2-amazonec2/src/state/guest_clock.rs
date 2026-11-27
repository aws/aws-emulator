// SPDX-License-Identifier: CC0-1.0

//! What one instance's GUEST reads as the TIME, and the program it reads it with.
//!
//! # Why a guest needs this
//!
//! A script run in a machine stamps what it does: it reads the clock before an operation and after
//! it, so that whoever reads the output knows the window the work happened in — which is how a
//! client then asks CloudWatch for the datapoints of exactly that window
//! (`date +%s` around the work, and `GetMetricStatistics` over the two instants). A guest whose
//! clock answers nothing leaves a successful script with no window at all, and nothing a reader can
//! correlate its output with.
//!
//! # What is real here
//!
//! The reading is this process's own clock, which is the clock every other instant of this emulator
//! is read off, rendered on the civil calendar in UTC — the zone a machine of this emulator runs in,
//! as the published machine facts report one. The program is a program OF THE GUEST, performed by
//! the same bounded, in-process shell session the guest's own boot script runs in
//! ([`super::guest_console`]): it starts no process and reaches no path of the host.
//!
//! WHAT an invocation asks for, and what it writes, is this workspace's one reading of that program
//! ([`::service_common::posix_shell_clock`]) — shared with every other session that performs
//! caller-supplied shell text, so a line a client wrote for it reads the same instant in the same
//! form wherever it runs. This module is where a GUEST offers it: the environment program a session
//! of this machine resolves that name through.
//!
//! An invocation asking for a form that reading renders nothing of — a format the civil calendar
//! cannot be written in, or a date to be PARSED out of prose — answers as a command that could not
//! be run, rather than an instant nothing stands behind.

use ::std::time::SystemTime;

use ::service_common::posix_shell_clock;
use ::service_common::posix_shell_session::{EnvironmentPrograms, ProgramFiles, ProgramOutcome};

/// The program a guest reads the time with.
pub const CLOCK_PROGRAM: &str = posix_shell_clock::CLOCK_PROGRAM;

/// The status a program of this guest exits with when it did what it was asked.
const SUCCESS: i32 = 0;

/// The programs a guest reads the time with, for a session that runs a program of that guest.
#[must_use]
pub fn clock_programs() -> Box<dyn EnvironmentPrograms> {
    Box::new(ClockPrograms)
}

/// The clock programs of one machine's guest.
///
/// It holds nothing: the reading is the clock itself, taken when the program runs.
#[derive(Debug)]
struct ClockPrograms;

impl EnvironmentPrograms for ClockPrograms {
    /// `date` writes the instant it was run at, in the form the invocation asks for.
    ///
    /// A program named by its absolute path is the same program, as a shell resolving a name on
    /// `PATH` finds. An invocation carrying an option or a format this program does not render
    /// answers `None` — a form this guest performs nothing of.
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        let _ = files;
        if !posix_shell_clock::is_clock_program(name) {
            return None;
        }
        Some(ProgramOutcome {
            status: SUCCESS,
            output: vec![posix_shell_clock::written(arguments, SystemTime::now())?],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree that holds nothing: the clock program reaches no file, so what it answers cannot
    /// depend on one.
    struct NoFiles;

    impl ProgramFiles for NoFiles {
        fn read(&self, _path: &str) -> Option<String> {
            None
        }

        fn write(&mut self, _path: &str, _contents: &str) -> bool {
            false
        }

        fn remove(&mut self, _path: &str) -> bool {
            false
        }
    }

    /// RULE: the guest offers the clock program under the name a script invokes it by, and offers
    /// nothing else.
    #[test]
    fn the_guest_offers_the_clock_program_and_nothing_else() {
        let programs = clock_programs();
        let mut files = NoFiles;
        // (case, the name a script invoked, the words it carried, whether the guest performs it)
        let rows: [(&str, &str, &[&str], bool); 4] = [
            ("the clock, by name", CLOCK_PROGRAM, &["+%s"], true),
            ("the clock, by path", "/bin/date", &["+%s"], true),
            ("a form it renders nothing of", CLOCK_PROGRAM, &["-Q"], false),
            ("another program entirely", "uptime", &[], false),
        ];
        for (case, name, words, performed) in rows {
            let arguments: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
            let answered = programs.run(name, &arguments, &mut files);
            assert_eq!(
                answered.is_some(),
                performed,
                "case {case:?}: expected performed == {performed}, got {answered:?}"
            );
            if let Some(outcome) = answered {
                assert_eq!(
                    outcome.status, SUCCESS,
                    "case {case:?}: a reading the guest took is a program that succeeded, got \
                     {outcome:?}"
                );
                let written = outcome.output.join("");
                assert!(
                    !written.is_empty() && written.chars().all(|digit| digit.is_ascii_digit()),
                    "case {case:?}: expected the epoch seconds the invocation asked for, got \
                     {written:?}"
                );
            }
        }
    }
}
