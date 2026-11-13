// SPDX-License-Identifier: CC0-1.0

//! The PROGRAM an instance's own user data gives its guest to run at boot.
//!
//! "The user data to make available to the instance" (`RunInstancesRequest$UserData`) is not
//! a value a client sets to read it back: an instance's user data is what its init system
//! RUNS when the guest comes up, and what a client then observes of it is the output on that
//! instance's console ([`super::instance_console_text`]). So the stored user data is read
//! here into the commands a boot performs, and the boot performs them
//! ([`super::guest_console`]).
//!
//! # The forms read
//!
//! * a `#cloud-config` document — the YAML form cloud-init documents, read with
//!   `yaml-rust2`, the pure-Rust YAML 1.2 implementation the other services that read
//!   caller-supplied YAML use. Its `bootcmd` runs on EVERY boot and its `runcmd` on the
//!   first, which is the split cloud-init publishes for those two modules (`bootcmd` is run
//!   from the init stage of every boot, `runcmd` from the final stage once per instance);
//! * a text that starts with `#!` — a user-data SCRIPT, which cloud-init runs once per
//!   instance, so its lines are first-boot commands.
//!
//! Every other form — a MIME multipart archive, an `#include`, gzip-compressed bytes, or
//! user data that is not UTF-8 text at all — is one this emulator runs NOTHING for: the
//! guest's console then carries the boot itself and no user-data output, rather than output
//! invented for a program nothing read.
//!
//! # Bounds
//!
//! The user data a launch may store is already bounded by that operation
//! (`RunInstances`'s 16 KiB ceiling), and every step of reading it is bounded again here,
//! BEFORE the step it bounds does any work: how many bytes of document are read at all
//! ([`MAX_GUEST_CONFIG_BYTES`]), how deeply its text can make a parse nest
//! ([`MAX_GUEST_CONFIG_TEXT_NESTING`], a scan of the text that parses nothing), how many
//! nodes it may carry and whether it aliases at all ([`MAX_GUEST_CONFIG_NODES`], read off
//! the parser's own event stream, which copies nothing), how deeply the loaded document
//! nests ([`MAX_GUEST_CONFIG_DEPTH`]), and how many commands one boot performs
//! ([`MAX_GUEST_COMMANDS`]) and how long one of them may be ([`MAX_GUEST_COMMAND_BYTES`]) —
//! so a document cannot make a boot cost more than a fixed amount of work however it nests,
//! anchors or aliases.

use ::base64::Engine as _;

/// How many commands ONE boot performs out of the instance's user data, counting its
/// every-boot and its first-boot commands together. A document that names more is read up to
/// this many and no further.
pub const MAX_GUEST_COMMANDS: usize = 64;

/// How many bytes one of those commands may carry. A longer one is not performed at all.
pub const MAX_GUEST_COMMAND_BYTES: usize = 4 * 1024;

/// How deeply the `#cloud-config` document may nest before it is not read at all, which is
/// what keeps the YAML reader's own recursion in range.
pub const MAX_GUEST_CONFIG_DEPTH: usize = 16;

/// How many nodes the TEXT of the document may nest by, counted before anything parses it.
///
/// The shared pre-parse scan counts every nesting indicator the text carries PLUS its
/// deepest indentation, so a sequence of entries counts one per entry rather than one for
/// the sequence: the ceiling therefore sits above what a document of at most
/// [`MAX_GUEST_COMMANDS`] entries needs, and it is still a bound on the recursion a parse
/// can reach, because every real level of nesting costs at least one of those. The
/// DOCUMENT's own depth is bounded exactly, by [`MAX_GUEST_CONFIG_DEPTH`].
pub const MAX_GUEST_CONFIG_TEXT_NESTING: usize = 1_024;

/// How many bytes of `#cloud-config` document are read at all.
///
/// The launch already bounds the user data it stores, and this bounds what reaches a parser
/// independently of that: the two are different operations' ceilings, and a parse must not
/// depend on another one having been applied.
pub const MAX_GUEST_CONFIG_BYTES: usize = 64 * 1024;

/// How many NODES the document may carry — scalars, sequences and mappings — counted on the
/// parser's own event stream before the document is loaded.
///
/// A byte ceiling does not bound the work a load performs: a loader resolves an ALIAS by
/// copying the anchored node, so a short document can expand enormously while it is built.
/// Counting the events the parser emits bounds that before anything is copied, and an alias
/// is refused outright beside it ([`within_load_budget`]).
pub const MAX_GUEST_CONFIG_NODES: usize = 4_096;

/// The header a `#cloud-config` user data starts with, as cloud-init documents it.
pub const CLOUD_CONFIG_HEADER: &str = "#cloud-config";

/// The key of the cloud-config module whose commands run on EVERY boot.
pub const CLOUD_CONFIG_BOOTCMD: &str = "bootcmd";

/// The key of the cloud-config module whose commands run on the FIRST boot only.
pub const CLOUD_CONFIG_RUNCMD: &str = "runcmd";

/// The commands one instance's guest runs at boot, read off its user data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestProgram {
    /// The commands the init system runs on EVERY boot — a cloud-config `bootcmd`.
    pub every_boot: Vec<String>,
    /// The commands it runs on the FIRST boot of the instance only — a cloud-config
    /// `runcmd`, or the lines of a user-data script.
    pub first_boot: Vec<String>,
}

impl GuestProgram {
    /// Whether this user data gives the guest nothing to run.
    pub fn is_empty(&self) -> bool {
        self.every_boot.is_empty() && self.first_boot.is_empty()
    }

    /// The commands boot number `boot` performs, in the order the init system runs them:
    /// the every-boot commands, and the first-boot ones only when this is the first boot.
    pub fn commands_for_boot(&self, boot: u32) -> Vec<&str> {
        let first_boot = if boot <= 1 {
            self.first_boot.as_slice()
        } else {
            &[]
        };
        self.every_boot
            .iter()
            .chain(first_boot)
            .map(String::as_str)
            .take(MAX_GUEST_COMMANDS)
            .collect()
    }
}

/// The program `user_data` gives the guest — empty for user data this emulator runs nothing
/// for (see the module docs).
///
/// `user_data` is the base64 text the launch stored ("the user data to make available to the
/// instance", which the API carries base64-encoded), so it is decoded here before it is
/// read.
pub fn guest_program(user_data: Option<&str>) -> GuestProgram {
    let Some(text) = user_data.and_then(decoded_user_data) else {
        return GuestProgram::default();
    };
    let trimmed = text.trim_start();
    if trimmed.starts_with(CLOUD_CONFIG_HEADER) {
        return cloud_config_program(trimmed);
    }
    if trimmed.starts_with("#!") {
        // A user-data script is run ONCE per instance, so its lines are the first boot's.
        return GuestProgram {
            every_boot: Vec::new(),
            first_boot: script_commands(trimmed),
        };
    }
    GuestProgram::default()
}

/// `user_data` as the text the guest was given, or `None` when it is not base64 at all or
/// not UTF-8 text once decoded — neither of which is a program this emulator can run.
fn decoded_user_data(user_data: &str) -> Option<String> {
    let decoded = ::base64::engine::general_purpose::STANDARD
        .decode(user_data.trim())
        .ok()?;
    String::from_utf8(decoded).ok()
}

/// The commands a `#cloud-config` document names, from its `bootcmd` and its `runcmd`.
///
/// Nothing is loaded until the text is known to be inside every bound a load is given: its
/// size, how deeply it nests (the shared pre-parse scan, which parses nothing) and what its
/// own event stream reports ([`within_load_budget`]).
fn cloud_config_program(text: &str) -> GuestProgram {
    if text.len() > MAX_GUEST_CONFIG_BYTES {
        return GuestProgram::default();
    }
    // The ONE pre-parse nesting bound this workspace states, taken on the TEXT: a parser
    // costs a level of recursion per nested collection, so the depth is bounded before the
    // parse rather than observed during it.
    if ::service_common::yaml_nesting::nesting_depth_bound(text, MAX_GUEST_CONFIG_TEXT_NESTING)
        > MAX_GUEST_CONFIG_TEXT_NESTING
    {
        return GuestProgram::default();
    }
    if !within_load_budget(text) {
        return GuestProgram::default();
    }
    let Ok(documents) = ::yaml_rust2::YamlLoader::load_from_str(text) else {
        return GuestProgram::default();
    };
    let Some(document) = documents.first() else {
        return GuestProgram::default();
    };
    if !within_depth(document, MAX_GUEST_CONFIG_DEPTH) {
        return GuestProgram::default();
    }
    GuestProgram {
        every_boot: module_commands(document, CLOUD_CONFIG_BOOTCMD),
        first_boot: module_commands(document, CLOUD_CONFIG_RUNCMD),
    }
}

/// Whether `text` is inside the budget a LOAD of it is given, read off the parser's own
/// event stream — which reports what the document holds and copies nothing.
///
/// Two things are bounded here that a byte ceiling does not bound. An ALIAS is refused
/// outright: a loader resolves one by copying the anchored node into each referencing
/// position, so a document that nests shallowly and weighs little can still expand
/// multiplicatively while it is built. And the NODES the stream reports are counted, so the
/// work the load would do is known to be bounded before any of it is done.
///
/// A document the parser cannot read at all is not inside the budget either: there is nothing
/// to load.
fn within_load_budget(text: &str) -> bool {
    /// What the event stream reported: how many nodes, how deeply, and whether an alias was
    /// among them.
    #[derive(Default)]
    struct Structure {
        nodes: usize,
        depth: usize,
        deepest: usize,
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for Structure {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            match event {
                ::yaml_rust2::parser::Event::SequenceStart(..)
                | ::yaml_rust2::parser::Event::MappingStart(..) => {
                    self.nodes = self.nodes.saturating_add(1);
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                ::yaml_rust2::parser::Event::SequenceEnd
                | ::yaml_rust2::parser::Event::MappingEnd => {
                    self.depth = self.depth.saturating_sub(1);
                }
                ::yaml_rust2::parser::Event::Scalar(..) => {
                    self.nodes = self.nodes.saturating_add(1);
                }
                ::yaml_rust2::parser::Event::Alias(_) => self.saw_alias = true,
                _ => {}
            }
        }
    }

    let mut structure = Structure::default();
    if ::yaml_rust2::parser::Parser::new_from_str(text)
        .load(&mut structure, true)
        .is_err()
    {
        return false;
    }
    !structure.saw_alias
        && structure.nodes <= MAX_GUEST_CONFIG_NODES
        && structure.deepest <= MAX_GUEST_CONFIG_DEPTH
}

/// The commands the cloud-config module `key` names: each entry of its list, as one command
/// line per line of that entry.
///
/// An entry cloud-init documents as a LIST ("`[ cloud-init-per, once, mymkfs, mkfs, /dev/vdb
/// ]`") is the argument vector of one command, which cloud-init runs WITHOUT a shell — so
/// its words are quoted back into the one command line that stands for that vector, and a
/// word of it carrying a space or an operator is still one word; an entry written as text is
/// the text itself, which may carry several lines.
fn module_commands(document: &::yaml_rust2::Yaml, key: &str) -> Vec<String> {
    let Some(entries) = document[key].as_vec() else {
        // A module written as one scalar is one entry, which is the other form cloud-init
        // accepts for these keys.
        return document[key]
            .as_str()
            .map(script_commands)
            .unwrap_or_default();
    };
    let mut commands: Vec<String> = Vec::new();
    for entry in entries {
        if commands.len() >= MAX_GUEST_COMMANDS {
            break;
        }
        match entry {
            ::yaml_rust2::Yaml::String(text) => commands.extend(script_commands(text)),
            ::yaml_rust2::Yaml::Array(words) => {
                let argv: Option<Vec<String>> = words
                    .iter()
                    .map(|word| match word {
                        ::yaml_rust2::Yaml::String(word) => Some(word.clone()),
                        _ => None,
                    })
                    .collect();
                if let Some(argv) = argv {
                    commands.extend(script_commands(&::shell_words::join(&argv)));
                }
            }
            _ => {}
        }
    }
    commands.truncate(MAX_GUEST_COMMANDS);
    commands
}

/// The command lines of a script: its lines, without the ones a shell performs nothing for —
/// the blank ones and the comments, the interpreter line included.
fn script_commands(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.len() <= MAX_GUEST_COMMAND_BYTES)
        .map(str::to_string)
        .take(MAX_GUEST_COMMANDS)
        .collect()
}

/// Whether `node` nests inside `depth` levels — what keeps this walk, and the document it
/// walks, in range.
fn within_depth(node: &::yaml_rust2::Yaml, depth: usize) -> bool {
    let Some(depth) = depth.checked_sub(1) else {
        return false;
    };
    match node {
        ::yaml_rust2::Yaml::Array(entries) => {
            entries.iter().all(|entry| within_depth(entry, depth))
        }
        ::yaml_rust2::Yaml::Hash(entries) => entries
            .iter()
            .all(|(_, value)| within_depth(value, depth)),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{guest_program, GuestProgram, MAX_GUEST_COMMANDS};
    use ::base64::Engine as _;

    /// `text` as a launch stores user data: base64.
    fn encoded(text: &str) -> String {
        ::base64::engine::general_purpose::STANDARD.encode(text)
    }

    /// RULE: a `#cloud-config` document's `bootcmd` runs on every boot and its `runcmd` on
    /// the first, a `#!` script runs once, and every other form of user data gives the guest
    /// nothing to run.
    #[test]
    fn the_program_is_what_the_user_data_names() {
        // (case, the user data, its every-boot commands, its first-boot commands)
        let rows: [(&str, String, Vec<&str>, Vec<&str>); 8] = [
            (
                "a cloud-config with both modules",
                encoded(
                    "#cloud-config\nbootcmd:\n  - echo every > /dev/console\nruncmd:\n  - echo once > /dev/console\n",
                ),
                vec!["echo every > /dev/console"],
                vec!["echo once > /dev/console"],
            ),
            (
                "a cloud-config whose entry is a block of lines",
                encoded(
                    "#cloud-config\nbootcmd:\n  - |\n    C=1\n    echo $C > /dev/console\n",
                ),
                vec!["C=1", "echo $C > /dev/console"],
                vec![],
            ),
            (
                "a cloud-config whose entry is an argument vector",
                encoded("#cloud-config\nbootcmd:\n  - [ echo, marker ]\n"),
                vec!["echo marker"],
                vec![],
            ),            (
                "a cloud-config whose module is one scalar",
                encoded("#cloud-config\nruncmd: echo scalar\n"),
                vec![],
                vec!["echo scalar"],
            ),
            (
                "a cloud-config with comments and blank lines",
                encoded("#cloud-config\nbootcmd:\n  - |\n    # a comment\n\n    echo kept\n"),
                vec!["echo kept"],
                vec![],
            ),
            (
                "a user-data script",
                encoded("#!/bin/sh\necho from-the-script\n"),
                vec![],
                vec!["echo from-the-script"],
            ),
            (
                "an include form this emulator runs nothing for",
                encoded("#include\nhttps://example.invalid/script\n"),
                vec![],
                vec![],
            ),
            ("user data that is not base64", "not base64!".to_string(), vec![], vec![]),
        ];
        for (case, user_data, every_boot, first_boot) in rows {
            let program = guest_program(Some(&user_data));
            assert_eq!(
                program.every_boot, every_boot,
                "case {case:?}: expected the every-boot commands {every_boot:?}, got {:?}",
                program.every_boot
            );
            assert_eq!(
                program.first_boot, first_boot,
                "case {case:?}: expected the first-boot commands {first_boot:?}, got {:?}",
                program.first_boot
            );
        }
        assert!(
            guest_program(None).is_empty(),
            "an instance with no user data must have no program"
        );
    }

    /// RULE: which commands a boot performs is the boot NUMBER's question — the every-boot
    /// commands always, the first-boot ones only on the first.
    #[test]
    fn a_later_boot_performs_only_the_every_boot_commands() {
        let program = GuestProgram {
            every_boot: vec!["echo every".to_string()],
            first_boot: vec!["echo once".to_string()],
        };
        // (the boot, the commands it performs)
        for (boot, expected) in [
            (1_u32, vec!["echo every", "echo once"]),
            (2, vec!["echo every"]),
            (7, vec!["echo every"]),
        ] {
            let commands = program.commands_for_boot(boot);
            assert_eq!(
                commands, expected,
                "case boot {boot}: expected {expected:?}, got {commands:?}"
            );
        }
    }

    /// RULE: what is read out of one document is bounded, so a document cannot make a boot
    /// perform more than a fixed number of commands.
    #[test]
    fn the_commands_read_out_of_one_document_are_bounded() {
        let many: String = (0..MAX_GUEST_COMMANDS * 4)
            .map(|index| format!("  - echo line-{index}\n"))
            .collect();
        let program = guest_program(Some(&encoded(&format!("#cloud-config\nbootcmd:\n{many}"))));
        assert_eq!(
            program.every_boot.len(),
            MAX_GUEST_COMMANDS,
            "expected the commands to stop at the bound, got {}",
            program.every_boot.len()
        );
        let long = format!("echo {}", "x".repeat(super::MAX_GUEST_COMMAND_BYTES));
        let program = guest_program(Some(&encoded(&format!(
            "#cloud-config\nbootcmd:\n  - {long}\n"
        ))));
        assert!(
            program.is_empty(),
            "expected a command past the size bound to be left out, got {:?}",
            program.every_boot
        );
    }

    /// RULE: a document is bounded BEFORE it is loaded — its size, how deeply its text can
    /// make a parse nest, how many nodes it carries and whether it aliases at all — so one
    /// that would cost the load more than a fixed amount of work runs nothing rather than
    /// being built first.
    #[test]
    fn a_document_outside_the_load_budget_runs_nothing() {
        // (case, the document)
        let rows = [
            (
                "past the byte ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo {}\n",
                    "x".repeat(super::MAX_GUEST_CONFIG_BYTES)
                ),
            ),
            (
                "past the nesting ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo nested\nnested: {}\n",
                    "[".repeat(super::MAX_GUEST_CONFIG_TEXT_NESTING + 2)
                ),
            ),
            (
                "past the node ceiling",
                format!(
                    "#cloud-config\nbootcmd:\n  - echo many\nmany: [{}]\n",
                    (0..super::MAX_GUEST_CONFIG_NODES + 2)
                        .map(|index| format!("node-{index},"))
                        .collect::<String>()
                ),
            ),
            (
                // A loader resolves an alias by COPYING the anchored node, so a document
                // that aliases is refused outright rather than expanded.
                "an alias, which a load would copy",
                "#cloud-config\nanchored: &anchor [a, b, c]\nbootcmd:\n  - echo aliased\naliased: *anchor\n"
                    .to_string(),
            ),
        ];
        for (case, document) in rows {
            let program = guest_program(Some(&encoded(&document)));
            assert!(
                program.is_empty(),
                "case {case:?}: expected a document outside the load budget to run nothing, \
                 got {program:?}"
            );
        }

        // The same document inside every one of those bounds is read.
        let program = guest_program(Some(&encoded(
            "#cloud-config\nbootcmd:\n  - echo inside the budget\n",
        )));
        assert_eq!(
            program.every_boot,
            ["echo inside the budget"],
            "expected a document inside the budget to be read, got {program:?}"
        );
    }
}
