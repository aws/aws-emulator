// SPDX-License-Identifier: CC0-1.0

//! What a notebook instance's LIFECYCLE CONFIGURATION HOOK does: the script one hook holds,
//! performed, and the CloudWatch Logs stream what it wrote is published under.
//!
//! # The rule this module implements
//!
//! A lifecycle configuration's hook is a script the PLATFORM runs for the notebook instance
//! that names the configuration — `NotebookInstanceLifecycleHook$Content` is "a
//! base64-encoded string that contains a shell script for a notebook instance lifecycle
//! configuration" — and what that script writes is observable in CloudWatch Logs: "View
//! Amazon CloudWatch Logs for notebook instance lifecycle configurations in log group
//! `/aws/sagemaker/NotebookInstances` in log stream `[notebook-instance-name]/\
//! [LifecycleConfigHook]`" (the hook shape's own doc). A configuration that is only STORED
//! is therefore half the behaviour: a client reads the effect of its script in the log
//! stream, and a notebook instance that reports the configuration attached while nothing ran
//! leaves that stream empty.
//!
//! The hook runs while the instance is still coming up, so its output is there by the time
//! the instance reports `InService` — which is when a client that waited starts reading it.
//!
//! # What performs the script
//!
//! `::service_common::posix_shell_session`, the workspace's ONE bounded, session-local
//! POSIX/bash session: the grammar is `brush-parser`'s, the catalog of utilities it performs
//! is session-local (output, status, variables), every other command and every form it does
//! not perform fails closed, and all of its bounds are stated there — the same session
//! CodeBuild's build environment performs a buildspec's command lines with. This module adds
//! only what is SageMaker's about a hook: the environment the hook's own doc documents, the
//! ceilings on the output a hook may publish, how a script is decoded, and the group and
//! stream it is published under.
//!
//! The one thing the session cannot report is a FAILED hook. "Lifecycle configuration
//! scripts cannot run for longer than 5 minutes. If a script runs for longer than 5 minutes,
//! it fails and the notebook instance is not created or started" is the only failure the
//! model documents, and this session has no wait, no sleep and no loop — its command budget
//! bounds a run far under that ceiling — so there is no run for it to cut short. A command
//! the session does not provide is not a failed script either: this emulator has no host for
//! that command to have had an effect on, so what it reports is the output of what it could
//! perform, and nothing is claimed about the rest. No notebook instance is therefore failed
//! over a hook here; the `FailureReason` SageMaker words for a hook that really did fail is
//! not among the values the model or any recording here carries, so it is left to whichever
//! issue records one rather than invented.

use super::*;
use std::collections::BTreeMap;

/// The log group a notebook instance's lifecycle-configuration hooks write to, from the hook
/// shape's own doc: "log group `/aws/sagemaker/NotebookInstances`".
pub const NOTEBOOK_LOG_GROUP: &str = "/aws/sagemaker/NotebookInstances";

/// One hook of a lifecycle configuration: which script of the configuration it is, and the
/// name the stream carrying its output is spelled with.
///
/// The hook shape's doc gives the stream as `[notebook-instance-name]/\
/// [LifecycleConfigHook]`, and the hooks a configuration declares are its `OnCreate` and
/// `OnStart` scripts, so the hook component is the configuration member's name prefixed with
/// `LifecycleConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleHook {
    /// The script that runs once, when the notebook instance is created.
    OnCreate,
    /// The script that runs every time the notebook instance is started, the first of which
    /// is the start the create itself performs.
    OnStart,
}

impl LifecycleHook {
    /// The `[LifecycleConfigHook]` component of this hook's log stream name.
    pub fn stream_component(self) -> &'static str {
        match self {
            Self::OnCreate => "LifecycleConfigOnCreate",
            Self::OnStart => "LifecycleConfigOnStart",
        }
    }

    /// The log stream this hook's output is published to for the notebook instance
    /// `notebook_instance_name`: `[notebook-instance-name]/[LifecycleConfigHook]`.
    pub fn log_stream_name(self, notebook_instance_name: &str) -> String {
        format!(
            "{notebook_instance_name}/{component}",
            component = self.stream_component()
        )
    }
}

/// The `$PATH` a hook's script runs with, from the hook shape's own doc: "the value of the
/// `$PATH` environment variable that is available to both scripts is
/// `/sbin:bin:/usr/sbin:/usr/bin`".
const HOOK_PATH: &str = "/sbin:bin:/usr/sbin:/usr/bin";

/// How many bytes of decoded script this module reads. The hook's own content is modelled
/// `@length 1..16384` (`NotebookInstanceLifecycleConfigContent`), so a decoded script is
/// shorter than that; the bound is stated here because the DECODED length is what is walked,
/// and a longer script is one no hook runs at all.
pub const MAX_SCRIPT_BYTES: usize = 16 * 1024;

/// How many lines of output one hook may publish. THIS NUMBER IS THE EMULATOR'S: the output
/// is what the log stream holds, so a script writing line after line cannot grow that stream
/// without a stated ceiling.
pub const MAX_HOOK_LOG_LINES: usize = 1_000;

/// How many bytes ONE published line may carry. THIS NUMBER IS THE EMULATOR'S, under
/// [`MAX_HOOK_LOG_LINES`]' rule applied to one line; a longer line is truncated to it rather
/// than dropped, so what the script wrote is still observable.
pub const MAX_HOOK_LOG_LINE_BYTES: usize = 8 * 1024;

/// What one hook's session is: the shared session's own statuses and bounds, keeping the
/// output a hook publishes.
fn hook_session_config() -> ::service_common::posix_shell_session::SessionConfig {
    ::service_common::posix_shell_session::SessionConfig {
        output: Some(::service_common::posix_shell_session::OutputLimits {
            lines: MAX_HOOK_LOG_LINES,
            line_bytes: MAX_HOOK_LOG_LINE_BYTES,
        }),
        ..::service_common::posix_shell_session::SessionConfig::default()
    }
}

/// Perform `script` and answer the lines it wrote, in the order it wrote them.
///
/// The script is read LINE BY LINE, which is how a shell reads a script file, and each line
/// is performed by the shared session, which starts with the `$PATH` the hook's doc
/// documents. A line the session performs nothing for contributes no output. The whole script
/// is left unread past [`MAX_SCRIPT_BYTES`], and reading stops at the first `exit`.
///
/// Pure computation over the script's own text: it starts nothing, opens nothing and reads
/// nothing off the host.
#[must_use]
pub fn hook_output(script: &str) -> Vec<String> {
    if script.len() > MAX_SCRIPT_BYTES {
        return Vec::new();
    }
    let mut environment = BTreeMap::new();
    environment.insert("PATH".to_string(), HOOK_PATH.to_string());
    let mut session = ::service_common::posix_shell_session::ShellSession::new(
        environment,
        hook_session_config(),
    );
    for line in script.lines() {
        let line = line.trim();
        // A blank line and a comment are not commands, and a shebang is the interpreter the
        // script names rather than a command of it.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        session.run(line);
        if session.exit_status().is_some() {
            break;
        }
    }
    session.into_output()
}

/// The script one hook of a lifecycle configuration holds, decoded from the base64 its
/// `Content` carries, or `None` when the configuration declares no such hook or its content
/// is not base64 this emulator can decode.
#[must_use]
pub fn hook_script(
    config: &NotebookInstanceLifecycleConfig,
    hook: LifecycleHook,
) -> Option<String> {
    let hooks = match hook {
        LifecycleHook::OnCreate => config.on_create.as_deref(),
        LifecycleHook::OnStart => config.on_start.as_deref(),
    }?;
    let content = hooks.iter().find_map(|held| held.content.as_deref())?;
    let decoded =
        ::base64::Engine::decode(&::base64::engine::general_purpose::STANDARD, content).ok()?;
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        hook_output, LifecycleHook, MAX_HOOK_LOG_LINES, MAX_HOOK_LOG_LINE_BYTES, MAX_SCRIPT_BYTES,
    };

    /// RULE: what a hook publishes is what its `echo` commands wrote — a command needing a
    /// host program, a redirection or a compound form writes nothing, and a command
    /// substitution writes whatever the command it substitutes wrote.
    #[test]
    fn only_the_catalog_writes_output() {
        // One script per shape a lifecycle script can carry, and the lines performing it
        // writes.
        let rows: [(&str, &str, Vec<&str>); 9] = [
            ("one echo", "echo one", vec!["one"]),
            (
                "several echoes keep their order",
                "echo one\necho two",
                vec!["one", "two"],
            ),
            (
                "a host program writes nothing",
                "curl https://example.invalid\necho after",
                vec!["after"],
            ),
            (
                "a redirection is not performed",
                "echo redirected > /tmp/out\necho plain",
                vec!["plain"],
            ),
            (
                // The substitution is performed in the session; `whoami` is a command it
                // has no program for, so it substitutes nothing and `echo` writes the
                // empty line that leaves.
                "a substituted command the session has no program for substitutes nothing",
                "echo $(whoami)\necho plain",
                vec!["", "plain"],
            ),
            (
                "a compound command is not performed",
                "if true; then echo inside; fi\necho plain",
                vec!["plain"],
            ),
            (
                "a variable this session set expands",
                "NAME=value\necho $NAME",
                vec!["value"],
            ),
            (
                "the documented PATH is in the environment",
                "echo $PATH",
                vec!["/sbin:bin:/usr/sbin:/usr/bin"],
            ),
            (
                "exit ends the script",
                "echo one\nexit 0\necho two",
                vec!["one"],
            ),
        ];
        for (case, script, expected) in rows {
            let written = hook_output(script);
            let expected: Vec<String> = expected.iter().map(|line| (*line).to_string()).collect();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// RULE: the output one hook publishes is bounded — at most [`MAX_HOOK_LOG_LINES`]
    /// lines, each at most [`MAX_HOOK_LOG_LINE_BYTES`] bytes.
    #[test]
    fn the_output_one_hook_keeps_is_bounded() {
        let mut script = String::new();
        for index in 0..(MAX_HOOK_LOG_LINES + 50) {
            script.push_str(&format!("echo line-{index}\n"));
        }
        let written = hook_output(&script);
        assert_eq!(
            written.len(),
            MAX_HOOK_LOG_LINES,
            "case many lines: expected the line ceiling, got {} lines",
            written.len()
        );

        // A line long enough to reach the per-line ceiling is built by EXPANSION: a word
        // written out that long would take the line itself past the session's parse bound,
        // so the ceiling is reached the way a real script would reach it.
        let chunk = "x".repeat(MAX_HOOK_LOG_LINE_BYTES / 4);
        let written = hook_output(&format!("WIDE={chunk}\necho $WIDE$WIDE$WIDE$WIDE$WIDE"));
        let length = written.first().map(String::len).unwrap_or_default();
        assert_eq!(
            length, MAX_HOOK_LOG_LINE_BYTES,
            "case one long line: expected the byte ceiling, got {length}"
        );
    }

    /// RULE: a script past [`MAX_SCRIPT_BYTES`] is not read at all, so one hook cannot cost
    /// the walk more than that bound.
    #[test]
    fn a_script_past_the_size_bound_is_not_read() {
        let mut script = String::from("echo first\n");
        while script.len() <= MAX_SCRIPT_BYTES {
            script.push_str("echo filler\n");
        }
        let written = hook_output(&script);
        assert!(
            written.is_empty(),
            "case oversized script: expected no output, got {} lines",
            written.len()
        );
    }

    /// RULE: each hook publishes under its own stream of the notebook instance, named
    /// `[notebook-instance-name]/[LifecycleConfigHook]`.
    #[test]
    fn each_hook_names_its_own_stream() {
        for (case, hook, expected) in [
            (
                "the create hook",
                LifecycleHook::OnCreate,
                "my-notebook/LifecycleConfigOnCreate",
            ),
            (
                "the start hook",
                LifecycleHook::OnStart,
                "my-notebook/LifecycleConfigOnStart",
            ),
        ] {
            let named = hook.log_stream_name("my-notebook");
            assert_eq!(
                named, expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }
}
