// SPDX-License-Identifier: CC0-1.0

//! The tests of the shared shell session ([`super`]), moved out of the module's own file so
//! each file stays inside this project's own length limit; the cases themselves are
//! unchanged.

use super::{
    Bounds, EnvironmentPrograms, OutputLimits, ProgramFiles, ProgramOutcome, SessionConfig,
    ShellSession, ShellVolume, Statuses, BUILT_IN_COMMANDS,
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

/// RULE: a command invoked through a wrapper that only changes HOW it is run is that
/// command — the wrapper's own words are the command line — and a wrapper naming no
/// utility runs nothing.
#[test]
fn a_wrapper_runs_the_command_it_names() {
    let statuses = Statuses::default();
    // (case, the line, the status, the lines it wrote)
    let rows: &[(&str, &str, i32, &[&str])] = &[
        ("nohup around a utility", "nohup echo kept", statuses.success, &["kept"]),
        (
            "setsid around a utility",
            "setsid echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "one wrapper around another",
            "nohup setsid echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "a wrapper around a command this session cannot run",
            "nohup nothing-of-this-session",
            statuses.could_not_run,
            &[],
        ),
        ("a wrapper naming no utility", "nohup", statuses.could_not_run, &[]),
    ];
    for (case, line, status, written) in rows {
        let mut held = session();
        let answered = held.run(line);
        assert_eq!(
            answered, *status,
            "case {case:?}: expected status {status}, got {answered}"
        );
        assert_eq!(
            held.output(),
            *written,
            "case {case:?}: expected {written:?}, got {:?}",
            held.output()
        );
    }
}

/// RULE: a command invoked through the wrapper that bounds how long it may run is that
/// command — the duration and the wrapper's own options are not the command's words — and a
/// command that ran out of that duration is reported as the wrapper having ended it.
#[test]
fn a_duration_wrapper_runs_the_command_it_names_for_the_duration_it_states() {
    let statuses = Statuses::default();
    // (case, the line, the status, the lines it wrote)
    let rows: &[(&str, &str, i32, &[&str])] = &[
        (
            "a duration around a utility",
            "timeout 5 echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "a duration written with a unit",
            "timeout 1m echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "a fractional duration",
            "timeout 0.5 echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "the signal and kill-after options are the wrapper's own words",
            "timeout -s KILL -k 1 5 echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "a command that stayed inside the duration",
            "timeout 10 sleep 5",
            statuses.success,
            &[],
        ),
        (
            "a command that ran out of the duration",
            "timeout 1 sleep 5",
            super::TIMED_OUT_STATUS,
            &[],
        ),
        (
            // "--preserve-status: exit with the same status as COMMAND, even when the
            // command times out".
            "a command that ran out of a duration asked to keep its own answer",
            "timeout --preserve-status 1 sleep 5",
            statuses.success,
            &[],
        ),
        (
            // "A duration of 0 disables the associated timeout."
            "a duration of zero, which bounds the command by nothing",
            "timeout 0 sleep 5",
            statuses.success,
            &[],
        ),
        (
            "another wrapper around it",
            "nohup timeout 5 echo kept",
            statuses.success,
            &["kept"],
        ),
        (
            "a wrapper around a command this session cannot run",
            "timeout 5 nothing-of-this-session",
            statuses.could_not_run,
            &[],
        ),
        (
            "a wrapper naming no duration at all",
            "timeout",
            statuses.could_not_run,
            &[],
        ),
        (
            "a wrapper naming a duration and no utility",
            "timeout 5",
            statuses.could_not_run,
            &[],
        ),
        (
            "a wrapper whose duration states none",
            "timeout later echo kept",
            statuses.could_not_run,
            &[],
        ),
        (
            // A number no span of seconds can hold once its suffix is applied to it.
            "a wrapper whose duration no span can hold",
            "timeout 1e308d echo kept",
            statuses.could_not_run,
            &[],
        ),
    ];
    for (case, line, status, written) in rows {
        // The rows that state a duration SPEND one, and `sleep` is a utility a session with a
        // tree of its own performs ([`super::VOLUME_COMMANDS`]).
        let mut held = session().with_file_utilities();
        let answered = held.run(line);
        assert_eq!(
            answered, *status,
            "case {case:?}: expected status {status}, got {answered}"
        );
        assert_eq!(
            held.output(),
            *written,
            "case {case:?}: expected {written:?}, got {:?}",
            held.output()
        );
    }
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
            "another host program, which the environment provides none of either",
            "whoami",
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
        12,
        "case the catalog: expected the twelve session-local utilities, got \
         {BUILT_IN_COMMANDS:?}"
    );
}

/// RULE: `seq` writes the NUMBERS from the first operand to the last in steps of the
/// increment between them — the sequence a program written as "try this a few times"
/// iterates over — and an invocation whose operands or options it performs nothing of writes
/// nothing at all.
///
/// The rows differ along what the rule generalises over: how many operands name the sequence,
/// the direction and size of the step, what separates the numbers, how they are padded,
/// whether the sequence reaches its last number, and which invocations are performed at all.
#[test]
fn seq_writes_the_numbers_the_operands_name() {
    let statuses = Statuses::default();
    // (case, the program, the status, what it wrote)
    let rows: [(&str, &str, i32, &[&str]); 12] = [
        ("one operand counts from one", "seq 3", statuses.success, &["1", "2", "3"]),
        ("two operands", "seq 2 4", statuses.success, &["2", "3", "4"]),
        ("three operands", "seq 1 2 7", statuses.success, &["1", "3", "5", "7"]),
        ("a step past the last number", "seq 1 4 6", statuses.success, &["1", "5"]),
        ("a negative step", "seq 3 -1 1", statuses.success, &["3", "2", "1"]),
        ("negative operands", "seq -2 0", statuses.success, &["-2", "-1", "0"]),
        ("a sequence of one", "seq 5 5", statuses.success, &["5"]),
        // A sequence that does not reach its last number writes nothing and succeeds.
        ("a sequence that goes nowhere", "seq 5 1", statuses.success, &[]),
        ("a separator of its own", "seq -s , 1 3", statuses.success, &["1,2,3"]),
        (
            "numbers padded to one width",
            "seq -w 8 11",
            statuses.success,
            &["08", "09", "10", "11"],
        ),
        // An invocation this command performs nothing of writes nothing.
        ("a step of nothing", "seq 1 0 3", statuses.could_not_run, &[]),
        ("an operand that is no integer", "seq 1.5", statuses.could_not_run, &[]),
    ];
    for (case, program, expected, written) in rows {
        let mut held = session();
        let status = held.run(program);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
        assert_eq!(
            held.output(),
            written,
            "case {case:?}: expected {written:?}, got {:?}",
            held.output()
        );
    }
}

/// RULE: a pipeline connects its stages — what one stage writes to standard output is the
/// standard input of the next — so only the last stage's output is the pipeline's, and a
/// filter computes over the bytes the stage before it produced.
///
/// The rows differ along what the rule generalises over: how many stages the pipeline has,
/// which filter reads the stage before it, whether a stage redirects its own output away
/// from the pipe, and whether a stage writes anything at all.
#[test]
fn a_pipeline_connects_what_one_stage_wrote_to_the_next() {
    let statuses = Statuses::default();
    // (case, the program, the status its last line answered, the lines it wrote)
    let rows: [(&str, &str, i32, &[&str]); 8] = [
        (
            "one stage reads what the stage before it wrote",
            "echo abc | cat",
            statuses.success,
            &["abc"],
        ),
        (
            "the stage before the last one writes nothing to the session",
            "echo abc | wc -c",
            statuses.success,
            // `echo` ends its line with a newline, so four bytes reach the filter.
            &["4"],
        ),
        (
            "three stages, each over what the one before it produced",
            "echo abcdef | wc -c | cut -c1",
            statuses.success,
            &["7"],
        ),
        (
            // The session this test drives keeps lines of at most 64 bytes, so the digest
            // is read back through the selection the recorded buildspec writes.
            "a digest over the bytes the stage before it wrote",
            "echo -n abc | sha256sum | cut -c1-64",
            statuses.success,
            &["ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"],
        ),
        (
            "a filter with nothing piped into it could not run",
            "wc -c",
            statuses.could_not_run,
            &[],
        ),
        (
            "a stage that redirects its own output passes an empty stream on",
            "echo abc > kept.txt | wc -c",
            statuses.success,
            &["0"],
        ),
        (
            "the pipeline's status is its last stage's",
            "echo abc | false",
            statuses.failure,
            &[],
        ),
        (
            "a stage this session has no utility for wrote nothing to pass on",
            "mvn package | wc -c",
            statuses.success,
            &["0"],
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

/// RULE: a stage whose output is past what a pipe may carry ends the pipeline: the stage
/// after it is not run over a stream nothing produced.
#[test]
fn a_stage_past_the_pipe_bound_ends_the_pipeline() {
    let statuses = Statuses::default();
    let mut held = ShellSession::new(
        BTreeMap::new(),
        SessionConfig {
            bounds: Bounds {
                // Smaller than the bytes the first stage writes, which is what puts that
                // stage past the bound.
                substitution_bytes: 4,
                ..Bounds::default()
            },
            output: Some(OutputLimits {
                lines: 8,
                line_bytes: 64,
            }),
            ..SessionConfig::default()
        },
    );
    let status = held.run("echo abcdefgh | wc -c");
    assert_eq!(
        status, statuses.failure,
        "case a stage past the pipe bound: expected the failure of the write that did not \
             happen, got {status}"
    );
    assert!(
        held.output().is_empty(),
        "case a stage past the pipe bound: the stage after it must not have run, got {:?}",
        held.output()
    );
}

/// RULE: a command substitution over a pipeline is worth what the LAST stage wrote, with    /// its trailing newlines removed — so a program's own computation is the word it
/// substituted.
#[test]
fn a_substituted_pipeline_is_worth_its_last_stage() {
    // (case, the program, the value the variable ends holding)
    let rows: [(&str, &str, &str); 4] = [
        (
            "a digest of a variable's value",
            "SECRET=abc\nDIGEST=$(printf '%s' \"$SECRET\" | sha256sum | cut -c1-64)",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "the byte count of a variable's value",
            "SECRET=abcdef\nLENGTH=$(printf '%s' \"$SECRET\" | wc -c)",
            "6",
        ),
        (
            "a count over a value with multi-byte characters",
            "SECRET=h\u{e9}llo\nLENGTH=$(printf '%s' \"$SECRET\" | wc -c)",
            "6",
        ),
        (
            "a selection out of what the stage before it wrote",
            "SECRET=one:two\nFIELD=$(printf '%s' \"$SECRET\" | cut -d: -f2)",
            "two",
        ),
    ];
    for (case, program, expected) in rows {
        // The file utilities are what `printf` is one of, so the session is the one an
        // environment with storage of its own drives.
        let mut held = session().with_file_utilities();
        for line in program.lines() {
            held.run(line);
        }
        let assigned = program
            .lines()
            .next_back()
            .and_then(|line| line.split_once('='))
            .map(|(name, _)| name)
            .unwrap_or_default();
        let value = held.variable(assigned);
        assert_eq!(
            value,
            Some(expected),
            "case {case:?}: expected {assigned} to hold {expected:?}, got {value:?}"
        );
    }
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

/// RULE: `$(…)` is performed by this session — over the same catalog, the same
/// environment programs and the same budget — and what it wrote, with its trailing
/// newlines removed, is the word. It is a subshell: what it sets and an `exit` it
/// performs do not reach the session that substituted it. The status of a command with
/// no command name is the last substitution's; every other command answers for itself.
#[test]
fn command_substitution_is_performed_in_a_subshell() {
    let statuses = Statuses::default();
    // (case, the program, the line it wrote last)
    let written: [(&str, &str, &str); 7] = [
        ("output becomes the word", "A=$(echo one)\necho [$A]", "[one]"),
        (
            "trailing newlines are removed, inner ones kept",
            "A=$(echo one; echo two)\necho \"[$A]\"",
            "[one\ntwo]",
        ),
        (
            "a substitution nested in a substitution",
            "A=$(echo $(echo deep))\necho [$A]",
            "[deep]",
        ),
        (
            "a substituted command the session does not perform substitutes nothing",
            "A=$(whoami)\necho [$A]",
            "[]",
        ),
        (
            "the variables it sets are its own",
            "export KEPT=outer\nA=$(export KEPT=inner)\necho [$KEPT]",
            "[outer]",
        ),
        (
            "and so are the ones it removes",
            "export KEPT=outer\nA=$(unset KEPT)\necho [$KEPT]",
            "[outer]",
        ),
        (
            "the backquoted spelling of the same substitution",
            "A=`echo one`\necho [$A]",
            "[one]",
        ),
    ];
    for (case, program, expected) in written {
        let held = run(program);
        // What the program wrote, as the one text it wrote: a value carrying a newline is
        // kept as two lines of the session's output, and it is the TEXT the word became
        // that each row is about.
        let wrote = held.output().join("\n");
        assert_eq!(
            wrote, expected,
            "case {case:?}: expected {expected:?}, got {wrote:?}"
        );
    }

    // An `exit` inside a substitution ends that subshell alone: the session goes on.
    let held = run("A=$(exit 3)\necho after");
    assert_eq!(
        held.exit_status(),
        None,
        "case \"an exit inside a substitution\": expected the session to go on, got {:?}",
        held.exit_status()
    );
    assert_eq!(
        held.output(),
        ["after"],
        "case \"an exit inside a substitution\": expected the next line to be performed, \
             got {:?}",
        held.output()
    );

    // (case, the line, the status it answers)
    let answered: [(&str, String, i32); 5] = [
        (
            "a command with a name of its own answers for itself",
            "echo $(false)".to_string(),
            statuses.success,
        ),
        (
            "a command with no name answers with its substitution's status",
            "A=$(false)".to_string(),
            statuses.failure,
        ),
        (
            "… and with its LAST substitution's status",
            "A=$(false) B=$(true)".to_string(),
            statuses.success,
        ),
        (
            "… including a substituted command the session does not perform",
            "A=$(whoami)".to_string(),
            statuses.could_not_run,
        ),
        (
            "a word nesting more substitutions than one word may",
            format!("A={}true{}", "$(".repeat(12), ")".repeat(12)),
            statuses.could_not_run,
        ),
    ];
    for (case, line, expected) in answered {
        let mut held = session();
        let status = held.run(&line);
        assert_eq!(
            status, expected,
            "case {case:?}: expected status {expected}, got {status}"
        );
    }

    // The substituted commands come out of the SAME command budget, and performing a
    // substitution AT ALL spends one of it, so neither the commands inside one nor the
    // number of them a line opens can outrun the budget.
    let two_commands = SessionConfig {
        bounds: Bounds {
            commands: 2,
            ..Bounds::default()
        },
        output: Some(OutputLimits {
            lines: 8,
            line_bytes: 64,
        }),
        ..SessionConfig::default()
    };
    let mut held = ShellSession::new(BTreeMap::new(), two_commands);
    held.run("A=$(echo one)");
    held.run("echo [$A]");
    assert!(
        held.output().is_empty(),
        "case \"the shared command budget\": expected the budget to be spent by the \
             substitution, got {:?}",
        held.output()
    );

    // A substitution whose text performs no command at all still spends one, so a line
    // carrying many of them is bounded by the same budget.
    // (case, the whole command budget, the line, the status it answers)
    for (case, commands, line, expected) in [
        (
            "a budget the command and its substitution spend between them",
            2,
            "A=$(true)",
            statuses.could_not_run,
        ),
        (
            "a budget for the command, the substitution and the command inside it",
            3,
            "A=$(true)",
            statuses.success,
        ),
        (
            "a second substitution on the same line, past the budget",
            3,
            "echo $(true) $(true)",
            statuses.could_not_run,
        ),
    ] {
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    commands,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        );
        let status = held.run(line);
        assert_eq!(
            status, expected,
            "case {case:?}: expected status {expected}, got {status}"
        );
    }

    // The variables a session STARTS with are held to the store's own ceilings, so what
    // its caller hands it cannot grow the session past them.
    let handed: BTreeMap<String, String> = ["A", "B", "C"]
        .into_iter()
        .map(|name| (name.to_string(), "value".to_string()))
        .collect();
    let held = ShellSession::new(
        handed,
        SessionConfig {
            bounds: Bounds {
                session_variables: 2,
                ..Bounds::default()
            },
            ..SessionConfig::default()
        },
    );
    let kept = ["A", "B", "C"]
        .into_iter()
        .filter(|name| held.variable(name).is_some())
        .count();
    assert_eq!(
        kept, 2,
        "case \"the variables a session starts with\": expected the store's ceiling to \
             hold for them too, got {kept} of them"
    );
}

/// An environment that provides ONE program, so a session's catalog is not all it can
/// name — and that program answers only the one invocation it is given.
#[derive(Debug)]
struct OneProgram;

impl EnvironmentPrograms for OneProgram {
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        _files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        let queried: Vec<&str> = arguments.iter().map(String::as_str).collect();
        (name == "report" && queried == ["--version"]).then(|| ProgramOutcome {
            status: 0,
            output: vec!["report 1.2.3".to_string()],
        })
    }
}

/// An environment whose one program CARRIES a file: `carry <from> <to>` moves what the
/// session's tree holds at one path to the other, which is the whole of what a program
/// that takes a file somewhere does to the tree.
#[derive(Debug)]
struct CarryingProgram;

impl EnvironmentPrograms for CarryingProgram {
    fn run(
        &self,
        name: &str,
        arguments: &[String],
        files: &mut dyn ProgramFiles,
    ) -> Option<ProgramOutcome> {
        if name != "carry" || arguments.len() != 2 {
            return None;
        }
        let Some(carried) = files.read(&arguments[0]) else {
            return Some(ProgramOutcome {
                status: 1,
                output: vec![format!("carry: {}: no such file", arguments[0])],
            });
        };
        if !files.write(&arguments[1], &carried) {
            return Some(ProgramOutcome {
                status: 1,
                output: vec![format!("carry: {}: not written", arguments[1])],
            });
        }
        files.remove(&arguments[0]);
        Some(ProgramOutcome {
            status: 0,
            output: vec![format!("carry: {} to {}", arguments[0], arguments[1])],
        })
    }
}

/// RULE: a program of the environment reads, writes and removes the FILES of the session
/// it was invoked in, so a file the commands around it wrote is the one it carries, a file
/// it leaves is one they read back, and a file it took away is gone — and a path the tree
/// holds nothing at is the program's own failure.
#[test]
fn an_environments_program_carries_the_sessions_own_files() {
    /// One row: the program, the status it answers, and what the tree then holds at the
    /// destination and at the source.
    struct Row {
        case: &'static str,
        program: &'static str,
        status: i32,
        destination_holds: Option<&'static str>,
        source_holds: Option<&'static str>,
    }

    let statuses = Statuses::default();
    let rows = [
        Row {
            case: "a file a redirection left behind",
            program: "echo carried > /tmp/source && carry /tmp/source /tmp/destination",
            status: statuses.success,
            destination_holds: Some("carried\n"),
            source_holds: None,
        },
        Row {
            case: "a file the program left, read back by the catalog",
            program: "echo carried > /tmp/source; carry /tmp/source /tmp/destination; \
                          cat /tmp/destination > /tmp/read-back",
            status: statuses.success,
            destination_holds: Some("carried\n"),
            source_holds: None,
        },
        Row {
            case: "a source the tree holds nothing at",
            program: "echo kept > /tmp/source; carry /tmp/absent /tmp/destination",
            status: statuses.failure,
            destination_holds: None,
            source_holds: Some("kept\n"),
        },
        Row {
            case: "an invocation this environment performs nothing of",
            program: "echo kept > /tmp/source; carry /tmp/source",
            status: statuses.could_not_run,
            destination_holds: None,
            source_holds: Some("kept\n"),
        },
    ];
    for Row {
        case,
        program,
        status: expected_status,
        destination_holds,
        source_holds,
    } in rows
    {
        let mut session = ShellSession::with_programs(
            BTreeMap::new(),
            SessionConfig::default(),
            Box::new(CarryingProgram),
        );
        let status = session.run(program);
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected status {expected_status}, got {status}"
        );
        let stored = session.tree_files();
        let held = |path: &str| {
            stored
                .iter()
                .find(|(held, _)| *held == path)
                .map(|(_, contents)| *contents)
        };
        let destination = held("/tmp/destination");
        assert_eq!(
            destination, destination_holds,
            "case {case:?}: expected the destination to hold {destination_holds:?}, got \
                 {destination:?}"
        );
        let source = held("/tmp/source");
        assert_eq!(
            source, source_holds,
            "case {case:?}: expected the source to hold {source_holds:?}, got {source:?}"
        );
        // The read-back row proves the catalog's own `cat` reads the file the program
        // left, over the same tree.
        if case.contains("read back") {
            let read_back = held("/tmp/read-back");
            assert_eq!(
                read_back, destination_holds,
                "case {case:?}: expected `cat` to read the carried file back, got \
                     {read_back:?}"
            );
        }
    }
}

/// RULE: a command the catalog does not name is asked of the programs the ENVIRONMENT
/// provides, and what that program answered — its status and the lines it wrote — is what
/// the command answers; a name, or an invocation, the environment provides no program
/// for is still a command that could not be run.
#[test]
fn the_environments_programs_answer_beside_the_catalog() {
    let statuses = Statuses::default();
    let configured = SessionConfig {
        output: Some(OutputLimits {
            lines: 8,
            line_bytes: 64,
        }),
        ..SessionConfig::default()
    };
    // (case, the line, the status it answers, the lines it wrote)
    let rows: [(&str, &str, i32, &[&str]); 4] = [
        (
            "the invocation the environment provides",
            "report --version",
            statuses.success,
            &["report 1.2.3"],
        ),
        (
            "the same program, invoked another way",
            "report --help",
            statuses.could_not_run,
            &[],
        ),
        (
            "a program this environment provides none of",
            "other --version",
            statuses.could_not_run,
            &[],
        ),
        (
            "the catalog's own utilities still answer first",
            "echo one",
            statuses.success,
            &["one"],
        ),
    ];
    for (case, line, expected_status, expected_output) in rows {
        let mut held =
            ShellSession::with_programs(BTreeMap::new(), configured, Box::new(OneProgram));
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

    // What such a program wrote is substitutable, like any other command's output.
    let mut held =
        ShellSession::with_programs(BTreeMap::new(), configured, Box::new(OneProgram));
    held.run("V=$(report --version)");
    assert_eq!(
        held.variable("V"),
        Some("report 1.2.3"),
        "case \"a substituted environment program\": expected its line as the word, got {:?}",
        held.variable("V")
    );
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

/// RULE: a session working over a machine's tree is IN a directory of it — `cd` changes
/// which, `pwd` writes it, and a path a program names without a directory is resolved
/// against it, so a file saved under a bare name is the same file read back under one.
#[test]
fn a_path_without_a_directory_is_under_the_one_cd_changed_to() {
    let statuses = Statuses::default();
    let mut held = in_volume();
    /// The last line the session wrote, which is what the command just performed wrote.
    fn last(session: &ShellSession) -> String {
        session.output().last().cloned().unwrap_or_default()
    }

    assert_eq!(
        held.run("pwd"),
        statuses.success,
        "case the directory a session starts in: expected it to be written"
    );
    assert_eq!(
        last(&held),
        "/",
        "case the directory a session starts in: expected the tree's root, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("cd /var/lib && pwd"),
        statuses.success,
        "case a directory the tree holds: expected the change to happen"
    );
    assert_eq!(
        last(&held),
        "/var/lib",
        "case a directory the tree holds: expected it to be the directory now, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("cat counter"),
        statuses.success,
        "case a bare name after a change: expected the file under that directory to be read"
    );
    assert_eq!(
        last(&held),
        "1",
        "case a bare name after a change: expected the file's own contents, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("echo kept > made"),
        statuses.success,
        "case a write under a bare name: expected it to happen"
    );
    assert_eq!(
        held.run("cat /var/lib/made"),
        statuses.success,
        "case a write under a bare name: expected it under the directory the session is in"
    );
    assert_eq!(
        last(&held),
        "kept",
        "case a write under a bare name: expected what the program wrote, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("cd /var/lib/absent"),
        statuses.failure,
        "case a path the tree holds nothing at: expected the change to fail"
    );
    assert_eq!(
        held.run("cd counter"),
        statuses.failure,
        "case a path the tree holds a FILE at: expected the change to fail"
    );
    assert_eq!(
        held.run("pwd"),
        statuses.success,
        "case a change that failed: expected the session to answer where it still is"
    );
    assert_eq!(
        last(&held),
        "/var/lib",
        "case a change that failed: expected the session where it was, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("mkdir deeper && cd deeper && pwd"),
        statuses.success,
        "case a relative operand: expected the change to walk from the directory it is in"
    );
    assert_eq!(
        last(&held),
        "/var/lib/deeper",
        "case a relative operand: expected the directory under it, got {:?}",
        last(&held)
    );

    assert_eq!(
        held.run("cd -"),
        statuses.success,
        "case the directory before this one: expected the change back"
    );
    assert_eq!(
        last(&held),
        "/var/lib",
        "case the directory before this one: expected it written, got {:?}",
        last(&held)
    );
}

/// RULE: a session whose tree is its OWN has no directory to be in, so neither command is
/// one it performs.
#[test]
fn a_session_with_no_machine_tree_performs_no_directory_change() {
    let statuses = Statuses::default();
    let mut held = ShellSession::new(BTreeMap::new(), SessionConfig::default());
    for line in ["cd /tmp", "pwd"] {
        assert_eq!(
            held.run(line),
            statuses.could_not_run,
            "case {line:?}: expected a command this session could not run"
        );
    }
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

/// RULE: `tee` writes what it reads into each file it names and passes it on, from the
/// start or appended, and a session given no tree performs none of it.
#[test]
fn tee_writes_what_it_reads_into_the_files_it_names() {
    let statuses = Statuses::default();
    let mut held = in_volume();
    assert_eq!(
        held.run("echo first | tee /var/lib/page"),
        statuses.success,
        "expected the write to be taken"
    );
    assert_eq!(
        held.run("echo second | tee -a /var/lib/page > /dev/console"),
        statuses.success,
        "expected the appending write to be taken"
    );
    let volume = held.into_volume().expect("the session ran in a volume");
    assert_eq!(
        volume.read("/var/lib/page").as_deref(),
        Some("first\nsecond\n"),
        "expected the file to hold both writes, got {:?}",
        volume.read("/var/lib/page")
    );
    // What it read is passed on, which is what the redirection above collected.
    assert_eq!(
        volume.device_lines("/dev/console"),
        ["second"],
        "expected the input to be passed on, got {:?}",
        volume.device_lines("/dev/console")
    );
}

/// RULE: a HERE-DOCUMENT is the command's standard input, written inside the command
/// itself — taken literally for a quoted delimiter, expanded for an unquoted one, with
/// `<<-` stripping the leading tabs — and a here-string is the one word it names.
#[test]
fn a_here_document_is_the_commands_standard_input() {
    let statuses = Statuses::default();
    // (case, the program, the contents the file then holds)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a quoted delimiter takes the document literally",
            "cat > /var/lib/page <<'EOF'\nMARKER $HOME\nEOF",
            "MARKER $HOME\n",
        ),
        (
            "an unquoted delimiter expands the document",
            "NAME=marker\ncat > /var/lib/page <<EOF\nvalue $NAME\nEOF",
            "value marker\n",
        ),
        (
            "the tab-stripping form",
            "cat > /var/lib/page <<-'EOF'\n\tindented\nEOF",
            "indented\n",
        ),
        (
            "a here-string",
            "cat > /var/lib/page <<<'one word'",
            "one word\n",
        ),
        (
            "a document of several lines",
            "cat > /var/lib/page <<'EOF'\nfirst\nsecond\nEOF",
            "first\nsecond\n",
        ),
    ];
    for (case, program, expected) in rows {
        let mut held = in_volume();
        let status = held.run(program);
        assert_eq!(
            status, statuses.success,
            "case {case:?}: expected the command to run, got {status}"
        );
        let volume = held.into_volume().expect("the session ran in a volume");
        let held = volume.read("/var/lib/page");
        assert_eq!(
            held.as_deref(),
            Some(*expected),
            "case {case:?}: expected the file to hold {expected:?}, got {held:?}"
        );
    }

    // A session given no tree performs none of those forms.
    let mut own = session();
    assert_eq!(
        own.run("cat <<'EOF'\nnothing\nEOF"),
        statuses.could_not_run,
        "a session with no tree performs no here-document"
    );
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

    // How deeply an ARITHMETIC expansion may be resolved inside another: resolving one
    // expands the expression's own text as a WORD, and a word is given the word budget
    // afresh, so what bounds that recursion is the count of expansions being resolved inside
    // one another — an expression nesting past it has no value rather than driving the
    // recursion as far as its own text reaches.
    // (case, how many levels the expression nests, the status the command answers)
    let nesting = |levels: usize| {
        let mut expression = "1".to_string();
        for _ in 0..levels {
            expression = format!("$(( {expression} + 1 ))");
        }
        format!("echo {expression} > /dev/console")
    };
    for (case, levels, expected) in [
        ("inside the depth", 2, statuses.success),
        ("past the depth", 64, statuses.could_not_run),
    ] {
        let mut held = ShellSession::in_volume(
            BTreeMap::new(),
            config(Bounds {
                // Wide enough that the LINE's own nesting count is not what refuses it, which
                // is the bound a caller handing a whole script states.
                nesting: 512,
                word_depth: 4,
                ..Bounds::default()
            }),
            volume(),
        );
        assert_eq!(
            held.run(&nesting(levels)),
            expected,
            "case {case:?}: expected the status {expected} for an arithmetic expansion nesting \
             {levels} level(s)"
        );
    }
}

/// RULE: a program of the tree is RESOLVED the way the shell's own command search resolves
/// one — the file a name with a slash points at, or the first directory of the session's own
/// `PATH` holding a file of that name — and performed as a script with its operands, wherever
/// the name was written; and a program that runs ITSELF is ended by the budget running a
/// program spends rather than by this module's own recursion.
///
/// The rows differ along how the program is named (a bare name on `PATH`, an absolute path, a
/// name inside another program), what it is given (no operand, operands it reports), and
/// whether it can be run at all (a file naming another interpreter, a name no directory of the
/// path holds) — a session with a machine's tree keeps a diagnostic on its console, which is
/// where the rows that run nothing report it.
#[test]
fn a_program_of_the_tree_is_resolved_and_a_program_that_runs_itself_is_bounded() {
    let statuses = Statuses::default();
    let config = SessionConfig {
        output: Some(OutputLimits {
            lines: 64,
            line_bytes: 256,
        }),
        ..SessionConfig::default()
    };
    let volume = || {
        let mut volume = ShellVolume::default();
        volume.put_file("/opt/bin/greet", "#!/bin/sh\necho \"hello $1\"\n");
        volume.put_file("/opt/bin/caller", "#!/bin/sh\ngreet \"from the sibling\"\n");
        volume.put_file("/opt/bin/native", "#!/usr/bin/env python3\nprint('hi')\n");
        volume.put_file("/opt/bin/forever", "#!/bin/sh\nsh /opt/bin/forever\n");
        volume
    };
    let variables = || BTreeMap::from([("PATH".to_string(), "/usr/bin:/opt/bin".to_string())]);

    // (case, the line, the status it answers, the lines it wrote)
    let rows: [(&str, &str, i32, &[&str]); 6] = [
        ("a bare name on the path", "greet world", 0, &["hello world"]),
        (
            "the program's own path",
            "/opt/bin/greet world",
            0,
            &["hello world"],
        ),
        (
            "a name inside another program of the tree",
            "caller",
            0,
            &["hello from the sibling"],
        ),
        (
            // The program is resolved the same way as a stage of a pipeline, and what it
            // wrote is what the stage after it reads.
            "a stage of a pipeline",
            "greet piped | cat",
            0,
            &["hello piped"],
        ),
        (
            // A file asking for another interpreter needs a program this environment
            // provides none of, so the search ends with nothing — reported for the
            // INTERPRETER the file names — and nothing runs.
            "a file naming another interpreter",
            "native",
            statuses.could_not_run,
            &["sh: python3: command not found"],
        ),
        (
            "a name no directory of the path holds",
            "absent-program",
            statuses.could_not_run,
            &["sh: absent-program: command not found"],
        ),
    ];
    for (case, line, expected, written) in rows {
        let mut held = ShellSession::in_volume(variables(), config, volume());
        let status = held.run(line);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status} with {:?}",
            held.output()
        );
        assert_eq!(
            held.output(),
            written,
            "case {case:?}: expected {written:?} to have been written, got {:?}",
            held.output()
        );
    }

    // A program that runs ITSELF spends one of the programs a session may run per level, so
    // it ends on that bound with the status a command that could not run answers — rather
    // than driving this module's own recursion until the process has no stack left.
    let mut held = ShellSession::in_volume(
        variables(),
        SessionConfig {
            bounds: Bounds {
                substitutions: 8,
                substitution_depth: 4,
                ..Bounds::default()
            },
            ..config
        },
        volume(),
    );
    let status = held.run("forever");
    assert_eq!(
        status, statuses.could_not_run,
        "case a program that runs itself: expected the bound's own status, got {status}"
    );
}

/// RULE: a session that was given no MACHINE's tree performs none of the forms a machine's
/// program is written in — there is no device to write, no descriptor to report on and no
/// storage of the machine to read — so what such a session performs is its own catalog
/// over its own tree. (A command SUBSTITUTION is not one of those forms: it is the
/// shell's own, performed by every session over whatever that session performs, which
/// `command_substitution_is_performed_in_a_subshell` is the subject of.)
#[test]
fn a_session_with_no_volume_performs_none_of_them() {
    let statuses = Statuses::default();
    // (case, the line a program of a machine is written with)
    let rows = [
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

/// A session whose own tree is the WORKING tree of its environment, which is what a
/// caller holding storage of its own hands its program.
fn working_tree_session() -> ShellSession {
    session().with_file_utilities()
}

/// RULE: a session working over its own WORKING tree performs the file utilities over
/// that tree — where a path is a key, so a directory of it exists exactly as long as a
/// file under it does — and the arithmetic its program computes with.
///
/// The rows differ along what the rule generalises over: which utility is named, whether
/// the tree already holds the path, how the path is spelled, and what the arithmetic
/// comes to.
#[test]
fn a_session_over_a_working_tree_performs_the_file_utilities() {
    let statuses = Statuses::default();
    // (case, the line, the status it answers)
    let rows: [(&str, &str, i32); 12] = [
        ("a formatted line", "printf '%s' value", statuses.success),
        ("a directory, which the tree needs none of", "mkdir -p dist", statuses.success),
        ("a file written under it", "echo built > dist/app.txt", statuses.success),
        ("the file read back", "cat dist/app.txt", statuses.success),
        ("a question about that file", "test -f dist/app.txt", statuses.success),
        (
            "a question about the directory above it",
            "test -d dist",
            statuses.success,
        ),
        ("a question about a path nothing wrote", "test -f missing.txt", statuses.failure),
        ("the file touched again", "touch dist/app.txt", statuses.success),
        ("the file removed", "rm dist/app.txt", statuses.success),
        ("the same file removed twice", "rm dist/app.txt", statuses.failure),
        ("a removal that passes over it", "rm -f dist/app.txt", statuses.success),
        ("a nested program", "sh -c 'echo inside'", statuses.success),
    ];
    let mut held = working_tree_session();
    for (case, line, expected) in rows {
        let status = held.run(line);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
    }
    let mut computing = working_tree_session();
    assert_eq!(
        computing.run("export SEED=6"),
        statuses.success,
        "case \"the value the arithmetic reads\": the assignment must be performed"
    );
    assert_eq!(
        computing.run("export RESULT=\"$(( SEED * 7 + 3 ))\""),
        statuses.success,
        "case \"the arithmetic\": a session over a working tree performs the expansion"
    );
    assert_eq!(
        computing.variable("RESULT"),
        Some("45"),
        "case \"the arithmetic\": expected what the expression comes to, got {:?}",
        computing.variable("RESULT")
    );
}

/// RULE: the environment SEEDS the files its program is to find in the working tree, and
/// takes back out what the program left there — which is how a service hands a run its
/// input and keeps its output.
#[test]
fn the_working_trees_files_go_in_and_come_back_out() {
    let mut held = working_tree_session();
    assert!(
        held.put_file("payload.txt", "SEEDED"),
        "the tree must take the file the environment seeded"
    );
    assert_eq!(
        held.run("export READ=\"$(cat payload.txt)\""),
        Statuses::default().success,
        "the program must read the seeded file"
    );
    assert_eq!(
        held.variable("READ"),
        Some("SEEDED"),
        "expected the seeded bytes, got {:?}",
        held.variable("READ")
    );
    held.run("echo produced > out.txt");
    let files = held.tree_files();
    assert_eq!(
        files,
        [("out.txt", "produced\n"), ("payload.txt", "SEEDED")],
        "expected the seeded file and the one the program wrote, got {files:?}"
    );
}

/// RULE: a session working over a tree performs the grammar's own control flow around the
/// commands inside it — `if`/`elif`/`else`, `while`, `until`, `for` and a brace group —
/// and the forms that need an environment of their own are still ones it does not
/// perform. A session with no tree performs none of them.
///
/// The rows differ along what the rule generalises over: the form, whether its condition
/// holds, how many times a body runs, and whether a command inside it failed.
#[test]
fn a_session_over_a_tree_performs_the_grammars_control_flow() {
    let statuses = Statuses::default();
    // (case, the program, the status it answers, what it left in STATE)
    let rows: [(&str, &str, i32, &str); 9] = [
        ("an if that holds", "if true; then export STATE=then; fi", statuses.success, "then"),
        (
            "an if that does not, with an else",
            "if false; then export STATE=then; else export STATE=else; fi",
            statuses.success,
            "else",
        ),
        (
            "an elif",
            "if false; then export STATE=then; elif true; then export STATE=elif; fi",
            statuses.success,
            "elif",
        ),
        (
            "an if no branch of which runs",
            "if false; then export STATE=then; fi",
            statuses.success,
            "",
        ),
        (
            "a while loop",
            "export STATE=0; while [ $STATE -lt 3 ]; do export STATE=\"$(( STATE + 1 ))\"; done",
            statuses.success,
            "3",
        ),
        (
            "an until loop",
            "export STATE=0; until [ $STATE -ge 2 ]; do export STATE=\"$(( STATE + 1 ))\"; done",
            statuses.success,
            "2",
        ),
        (
            "a for loop over its words",
            "for WORD in a b c; do export STATE=\"$WORD\"; done",
            statuses.success,
            "c",
        ),
        ("a brace group", "{ export STATE=grouped; }", statuses.success, "grouped"),
        (
            "a command inside that failed",
            "if true; then export STATE=entered; false; fi",
            statuses.failure,
            "entered",
        ),
    ];
    for (case, program, expected, state) in rows {
        let mut held = working_tree_session();
        let status = held.run(program);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
        assert_eq!(
            held.variable("STATE").unwrap_or_default(),
            state,
            "case {case:?}: expected STATE {state:?}, got {:?}",
            held.variable("STATE")
        );
        // The same program in a session with no tree of its own performs nothing.
        let mut without = session();
        assert_eq!(
            without.run(program),
            statuses.could_not_run,
            "case {case:?}: a session with no tree must perform no compound command"
        );
    }
    // A loop with no end of its own ends, whatever its condition and its body are made
    // of: every command spends one of the session's budget, a form this session does not
    // perform included, so no iteration is free.
    // (case, the program)
    let endless = [
        ("built from simple commands", "while true; do : ; done"),
        ("a condition and a body of subshells", "until ( : ); do ( : ); done"),
        (
            "a condition this session does not perform",
            "until case x in esac; do ( : ); done",
        ),
        (
            "a condition that is itself a loop",
            "while for i; do :; done; do ( : ); done",
        ),
        (
            "a condition no branch of which runs",
            "while if false; then :; fi; do ( : ); done",
        ),
    ];
    for (case, program) in endless {
        let mut held = ShellSession::new(
            BTreeMap::new(),
            SessionConfig {
                bounds: Bounds {
                    commands: 16,
                    ..Bounds::default()
                },
                ..SessionConfig::default()
            },
        )
        .with_file_utilities();
        assert_eq!(
            held.run(program),
            statuses.could_not_run,
            "case {case:?}: expected the budget to end the loop"
        );
    }
}

/// RULE: `break` leaves the enclosing LOOP and `continue` goes on with its next iteration, so
/// a loop written as "try this a few times, and stop once it worked" stops at the iteration
/// that worked — and the count either takes names how many enclosing loops it leaves.
///
/// The rows differ along what the rule generalises over: which of the two commands runs, which
/// loop form encloses it, how deeply the loops nest, how many levels the command leaves, and
/// whether there is a loop to leave at all.
#[test]
fn break_and_continue_leave_the_enclosing_loop() {
    let statuses = Statuses::default();
    // (case, the program, the status it answers, what it left in STATE)
    let rows: [(&str, &str, i32, &str); 9] = [
        (
            "a break at the iteration that worked",
            "for WORD in a b c; do export STATE=\"$WORD\"; if [ \"$WORD\" = b ]; then break; fi; done",
            statuses.success,
            "b",
        ),
        (
            "a break out of a while loop",
            "export STATE=0; while true; do export STATE=\"$(( STATE + 1 ))\"; break; done",
            statuses.success,
            "1",
        ),
        (
            "a break out of an until loop",
            "export STATE=start; until false; do export STATE=once; break; done",
            statuses.success,
            "once",
        ),
        (
            "a continue that skips the rest of the iteration",
            "export STATE=none; for WORD in a b; do continue; export STATE=\"$WORD\"; done",
            statuses.success,
            "none",
        ),
        (
            "a continue that goes on to the last word",
            "for WORD in a b c; do export STATE=\"$WORD\"; continue; export STATE=skipped; done",
            statuses.success,
            "c",
        ),
        (
            "a break of one level leaves the inner loop only",
            "for OUTER in a b; do for INNER in x y; do break; done; export STATE=\"$OUTER\"; done",
            statuses.success,
            "b",
        ),
        (
            "a break of two levels leaves both loops",
            "export STATE=none; for OUTER in a b; do for INNER in x y; do break 2; done; \
             export STATE=\"$OUTER\"; done",
            statuses.success,
            "none",
        ),
        (
            "a break where there is no loop at all",
            "export STATE=outside; break; export STATE=after",
            statuses.success,
            "after",
        ),
        (
            "a count that is no positive number",
            "export STATE=0; for WORD in a; do break nought; export STATE=reached; done",
            statuses.success,
            "reached",
        ),
    ];
    for (case, program, expected, state) in rows {
        let mut held = working_tree_session();
        let status = held.run(program);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
        assert_eq!(
            held.variable("STATE").unwrap_or_default(),
            state,
            "case {case:?}: expected STATE {state:?}, got {:?}",
            held.variable("STATE")
        );
    }
}

/// RULE: the standard output of a whole COMPOUND command is redirected where the
/// compound command carries such a redirection, so what every command inside it wrote is
/// ONE write of that file — and a compound command redirecting any other descriptor is a
/// form this session does not perform.
///
/// The rows differ along what the rule generalises over: the compound form, whether the
/// write replaces the file or adds to it, how many commands inside wrote, and which
/// descriptor the redirection names.
#[test]
fn a_redirected_compound_command_writes_one_file() {
    let statuses = Statuses::default();
    // (case, the program, the status it answers, what the tree holds at `out.txt`)
    let rows: [(&str, &str, i32, Option<&str>); 6] = [
        (
            "a brace group of two commands",
            "{ echo one; echo two; } > out.txt",
            statuses.success,
            Some("one\ntwo\n"),
        ),
        (
            "a brace group that ADDS to the file",
            "echo kept > out.txt; { echo added; } >> out.txt",
            statuses.success,
            Some("kept\nadded\n"),
        ),
        (
            "a for loop over its words",
            "for WORD in a b; do echo $WORD; done > out.txt",
            statuses.success,
            Some("a\nb\n"),
        ),
        (
            "an if whose branch wrote",
            "if true; then echo branch; fi > out.txt",
            statuses.success,
            Some("branch\n"),
        ),
        (
            "a brace group whose last command failed",
            "{ echo written; false; } > out.txt",
            statuses.failure,
            Some("written\n"),
        ),
        (
            "a brace group redirecting another descriptor",
            "{ echo one; } 2> out.txt",
            statuses.could_not_run,
            None,
        ),
    ];
    for (case, program, expected, held_at) in rows {
        let mut held = working_tree_session();
        let status = held.run(program);
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
        let files = held.tree_files();
        let written = files
            .iter()
            .find(|(path, _)| *path == "out.txt")
            .map(|(_, contents)| *contents);
        assert_eq!(
            written, held_at,
            "case {case:?}: expected the file to hold {held_at:?}, got {written:?}"
        );
        // The program writes nothing anywhere else: one redirection is one write.
        assert!(
            files.iter().all(|(path, _)| *path == "out.txt"),
            "case {case:?}: expected only the redirected file, got {files:?}"
        );
    }
}

/// RULE: the status an `exit` ended a program with is what the session reports until the
/// caller that hands this session SEVERAL programs over one tree says the next one
/// starts, and the commands of that next program are then performed.
#[test]
fn a_cleared_exit_lets_the_next_program_run() {
    let mut held = working_tree_session();
    held.run("exit 7");
    assert_eq!(
        held.exit_status(),
        Some(7),
        "the program ended with the status it named"
    );
    held.clear_exit();
    assert_eq!(
        held.exit_status(),
        None,
        "the session no longer carries the status the ended program left"
    );
    held.run("export AFTER=performed");
    assert_eq!(
        held.variable("AFTER"),
        Some("performed"),
        "the next program runs, got {:?}",
        held.variable("AFTER")
    );
    assert_eq!(
        held.exit_status(),
        None,
        "a program that ended no session leaves no status behind"
    );
}

/// RULE: `.` (and its `source` spelling) performs the commands of a file the tree holds
/// in THIS session, so the assignments that file carries are the session's variables
/// afterwards; a path the tree holds no file for is one the utility could not read, and
/// a session with no tree of its own performs none of it.
///
/// The rows differ along what the rule generalises over: the spelling of the utility,
/// how the file's values are quoted, whether the file is there, and whether the
/// variables are read back by a later command or by a command substitution.
#[test]
fn a_file_of_assignments_is_performed_in_the_session() {
    let statuses = Statuses::default();
    // (case, the spelling, the file's contents, the status it answers, what READ holds)
    let rows: [(&str, &str, &str, i32, &str); 5] = [
        (
            "a file of quoted assignments",
            ".",
            "NAME=\"Some Distribution\"\nID=\"somedist\"\n",
            statuses.success,
            "somedist",
        ),
        (
            "the same file through the bash spelling",
            "source",
            "ID=\"somedist\"\n",
            statuses.success,
            "somedist",
        ),
        (
            "an unquoted value",
            ".",
            "ID=somedist\nID_LIKE=other\n",
            statuses.success,
            "somedist",
        ),
        (
            "a value the file computes from the one above it",
            ".",
            "BASE=some\nID=\"${BASE}dist\"\n",
            statuses.success,
            "somedist",
        ),
        (
            "a file whose last assignment wins",
            ".",
            "ID=first\nID=somedist\n",
            statuses.success,
            "somedist",
        ),
    ];
    for (case, spelling, contents, expected, read) in rows {
        let mut held = working_tree_session();
        assert!(
            held.put_file("/etc/release-file", contents),
            "case {case:?}: the tree must take the file the environment published"
        );
        let status = held.run(&format!("{spelling} /etc/release-file"));
        assert_eq!(
            status, expected,
            "case {case:?}: expected the status {expected}, got {status}"
        );
        assert_eq!(
            held.variable("ID").unwrap_or_default(),
            read,
            "case {case:?}: expected ID {read:?}, got {:?}",
            held.variable("ID")
        );
        // The same file read inside a command substitution: what the sourced
        // assignments set is the WORD, and the subshell leaves the session as it was.
        let mut substituting = working_tree_session();
        assert!(
            substituting.put_file("/etc/release-file", contents),
            "case {case:?}: the tree must take the file the environment published"
        );
        assert_eq!(
            substituting.run(&format!(
                "export READ=\"$({spelling} /etc/release-file && echo $ID)\""
            )),
            statuses.success,
            "case {case:?}: the substitution must be performed"
        );
        assert_eq!(
            substituting.variable("READ").unwrap_or_default(),
            read,
            "case {case:?}: expected READ {read:?}, got {:?}",
            substituting.variable("READ")
        );
        assert_eq!(
            substituting.variable("ID"),
            None,
            "case {case:?}: the subshell's own variables do not reach the session, got {:?}",
            substituting.variable("ID")
        );
    }
    // A path the tree holds no file for: the utility could not read it, and nothing was
    // performed.
    let mut missing = working_tree_session();
    assert_eq!(
        missing.run(". /etc/absent-file"),
        statuses.failure,
        "case \"a path the tree holds no file for\": expected the status for a file that \
             could not be read"
    );
    // A session with no tree of its own has no file to read commands from at all.
    let mut without = session();
    assert_eq!(
        without.run(". /etc/release-file"),
        statuses.could_not_run,
        "case \"a session with no tree\": the utility is one it does not perform"
    );
}

/// RULE: a command NAME the shell's own search resolved nothing for — no utility of the
/// session's catalogs and no program of its environment — is reported the way a shell reports
/// one: the diagnostic naming that command on standard ERROR, beside the status for a command
/// it could not run. So a caller can tell a command this environment does not have from one
/// that ran and wrote nothing.
///
/// The rows differ along what the rule generalises over: which name was not resolved, whether
/// the command carried arguments, whether it was reached through a wrapper, whether it was a
/// STAGE of a pipeline (whose own status is its last stage's, so the diagnostic is the only
/// signal the stage did not run) and whether the name is one another session WOULD have a
/// utility for.
#[test]
fn a_command_nothing_resolved_says_so_on_standard_error() {
    let statuses = Statuses::default();
    // (case, the line, the name the diagnostic names, the status the line answered)
    let rows: &[(&str, &str, &str, i32)] = &[
        (
            "a host program",
            "curl https://example.invalid",
            "curl",
            statuses.could_not_run,
        ),
        ("a name with no arguments at all", "jq", "jq", statuses.could_not_run),
        (
            "a name reached through a wrapper",
            "nohup jq -r .",
            "jq",
            statuses.could_not_run,
        ),
        (
            // The pipeline's status is its LAST stage's, so a stage that did not run leaves
            // no status behind at all.
            "the first stage of a pipeline",
            "jq . | cat",
            "jq",
            statuses.success,
        ),
        (
            // A file utility only a session that works over a tree provides, so this
            // session's search reaches its environment and finds nothing.
            "a utility this session has no tree for",
            "mkdir held",
            "mkdir",
            statuses.could_not_run,
        ),
    ];
    for (case, line, name, expected) in rows {
        let mut held = session();
        let status = held.run(line);
        assert_eq!(
            status, *expected,
            "case {case:?}: expected status {expected}, got {status}"
        );
        let said = held.errors().join("\n");
        assert!(
            said.contains(name) && said.contains("not found"),
            "case {case:?}: expected a diagnostic naming {name:?}, got {said:?}"
        );
    }

    // `2>&1`: the diagnostic goes where the command's own output goes, which is what a caller
    // reading one merged stream sees — and nothing is left on the error stream.
    let mut merged = session();
    let status = merged.run("jq 2>&1 | cat");
    assert_eq!(
        status, statuses.success,
        "case \"a diagnostic sent into the pipe\": the pipeline is worth its last stage, got \
         {status}"
    );
    let written = merged.output().join("\n");
    assert!(
        written.contains("jq") && written.contains("not found"),
        "case \"a diagnostic sent into the pipe\": expected the stage's diagnostic to reach \
         standard output, got {written:?}"
    );
    assert!(
        merged.errors().is_empty(),
        "case \"a diagnostic sent into the pipe\": the duplication moved it off the error \
         stream, got {:?}",
        merged.errors()
    );

    // A command the search DID resolve says nothing: the diagnostic is the search's own
    // failure, not something every command writes.
    let mut performed = session();
    let status = performed.run("echo kept");
    assert_eq!(
        status, statuses.success,
        "case \"a command that ran\": expected success, got {status}"
    );
    assert!(
        performed.errors().is_empty(),
        "case \"a command that ran\": expected nothing on the error stream, got {:?}",
        performed.errors()
    );
}

/// RULE: a session COUNTS the commands of its program that named something nothing resolved,
/// so a caller can tell a program this session performed in full from one that asked for
/// something this environment provides none of — whose effect is held nowhere.
///
/// The rows differ along what the rule generalises over: a program every command of which ran,
/// one command nothing resolved, several of them, a program that mixes the two, one whose
/// unresolved command stands inside a pipeline, and one reached through a wrapper.
#[test]
fn a_session_counts_the_commands_nothing_resolved() {
    // (case, the program, how many of its commands named something nothing resolved)
    let rows: &[(&str, &str, usize)] = &[
        ("a program every command of which ran", "echo kept\necho again", 0),
        ("one command nothing resolved", "jq", 1),
        ("two of them", "jq\ncurl https://example.invalid", 2),
        (
            "a program that mixes the two",
            "echo kept\n/opt/sidecar --listen 9090\necho again",
            1,
        ),
        ("one inside a pipeline", "jq . | cat", 1),
        ("one reached through a wrapper", "nohup jq -r .", 1),
        ("no program at all", "", 0),
    ];
    for (case, program, expected) in rows {
        let held = run(program);
        let counted = held.unresolved_commands();
        assert_eq!(
            counted, *expected,
            "case {case:?}: expected {expected} unresolved command(s), got {counted}"
        );
    }
}

/// RULE: `set` turns the shell OPTIONS its words name on and off, and the options hold for
/// the commands AFTER it — `-e` ends the program at the first command that fails other than
/// one whose status the program itself reads, `-u` makes the expansion of a variable the
/// session holds none of an error, and `-o pipefail` makes a pipeline worth its rightmost
/// failing stage.
///
/// The rows differ along what the rule generalises over: which option the program sets and in
/// which spelling (a letter, several letters in one word, the long `-o` name, the `+` form),
/// where the failing command stands (alone, inside a tested condition, behind `!`, inside a
/// pipeline), and whether the program ends early.
#[test]
fn the_options_a_program_sets_hold_for_the_commands_after_it() {
    let statuses = Statuses::default();
    // (case, the program, the status it ended on, the lines it wrote)
    let rows: [(&str, &str, i32, &[&str]); 13] = [
        (
            "errexit ends the program at the failing command",
            "set -e\necho before\nfalse\necho after",
            statuses.failure,
            &["before"],
        ),
        (
            // "part of an AND-OR list other than the last" is a status the program itself is
            // reading, which this option passes over.
            "errexit passes over a command another tests with &&",
            "set -e\nfalse && echo taken\necho after",
            statuses.success,
            &["after"],
        ),
        (
            "errexit passes over a negated pipeline",
            "set -e\n! false\necho after",
            statuses.success,
            &["after"],
        ),
        (
            "errexit passes over a command another tests with ||",
            "set -e\nfalse || echo recovered\necho after",
            statuses.success,
            &["recovered", "after"],
        ),
        (
            "errexit turned off again",
            "set -e\nset +e\nfalse\necho after",
            statuses.success,
            &["after"],
        ),
        (
            "nounset refuses the expansion of a variable nothing set",
            "set -u\necho ${NOTHING_AT_ALL}\necho after",
            statuses.failure,
            &[],
        ),
        (
            "nounset leaves a variable the session holds alone",
            "set -u\nHELD=value\necho ${HELD}",
            statuses.success,
            &["value"],
        ),
        (
            "nounset leaves the default-value form alone",
            "set -u\necho ${NOTHING_AT_ALL:-fallback}",
            statuses.success,
            &["fallback"],
        ),
        (
            "pipefail answers the stage that failed",
            "set -o pipefail\nfalse | cat",
            statuses.failure,
            &[],
        ),
        (
            "a pipeline without pipefail answers its last stage",
            "false | cat",
            statuses.success,
            &[],
        ),
        (
            // Several letters in one word, which is how a script's own first line is written.
            "both options in one word",
            "set -eu\necho before\nfalse\necho after",
            statuses.failure,
            &["before"],
        ),
        (
            // The long option's NAME is the word after the group, wherever in the group its
            // letter stands: `set -uo pipefail` is the line a real script opens with.
            "a letter group ending in the long option",
            "set -uo pipefail\nfalse | cat\necho ${NOTHING_AT_ALL}",
            statuses.failure,
            &[],
        ),
        (
            "every option in one word",
            "set -euo pipefail\necho before\nfalse | cat\necho after",
            statuses.failure,
            &["before"],
        ),
    ];
    for (case, program, expected_status, expected_output) in rows {
        let mut held = session();
        let mut status = statuses.success;
        for line in program.lines() {
            status = held.run(line);
            if let Some(exited) = held.exit_status() {
                status = exited;
                break;
            }
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

    // The diagnostic `set -u` writes for the expansion it refused, which is the shell's own.
    let mut held = session();
    held.run("set -u");
    held.run("echo ${NOTHING_AT_ALL}");
    assert_eq!(
        held.errors(),
        ["sh: NOTHING_AT_ALL: parameter not set"],
        "case \"the nounset diagnostic\": expected the shell's own wording, got {:?}",
        held.errors()
    );

    // An option whose effect this session performs nothing of fails closed, with nothing
    // changed: the command after it still runs, and the failing command before it did not end
    // the program.
    let mut held = session();
    assert_eq!(
        held.run("set -o noclobber"),
        statuses.could_not_run,
        "case \"an option no session performs\": expected the status of a form it does not \
         perform"
    );
    assert_eq!(
        held.run("false"),
        statuses.failure,
        "case \"an option no session performs\": expected the next command to run"
    );
    assert!(
        held.exit_status().is_none(),
        "case \"an option no session performs\": expected no option to have been turned on, \
         got {:?}",
        held.exit_status()
    );

    // `set` with no word at all writes the names and values of the variables the session
    // holds, which is what the utility does when it is given none.
    let mut held = ShellSession::new(
        BTreeMap::from([("ONE".to_string(), "1".to_string())]),
        SessionConfig {
            output: Some(OutputLimits {
                lines: 8,
                line_bytes: 64,
            }),
            ..SessionConfig::default()
        },
    );
    assert_eq!(
        held.run("set"),
        statuses.success,
        "case \"set with no word\": expected it to write the variables"
    );
    assert_eq!(
        held.output(),
        ["ONE=1"],
        "case \"set with no word\": expected the session's own variables, got {:?}",
        held.output()
    );
}

/// RULE: a caller that reports the two streams as members of its own reads them apart, so a
/// diagnostic is not part of what the program wrote — while the console of a machine's
/// session, which carries one stream, keeps both.
#[test]
fn a_caller_reporting_both_streams_keeps_a_diagnostic_off_the_console() {
    let statuses = Statuses::default();
    // The condition of an `if` is a status the program itself reads, which `set -e` passes
    // over: a compound command is a form a session working over a TREE performs, so the case
    // is driven on one of those.
    let mut machine = in_volume().reporting_both_streams();
    machine.run("set -e");
    let status = machine.run("if false; then echo taken; fi\necho after");
    assert_eq!(
        status, statuses.success,
        "case \"a status a condition reads\": expected the program to go on, got {status}"
    );
    assert_eq!(
        machine.output(),
        ["after"],
        "case \"a status a condition reads\": expected the line after the `if`, got {:?}",
        machine.output()
    );
    assert!(
        machine.exit_status().is_none(),
        "case \"a status a condition reads\": expected the program not to have ended, got {:?}",
        machine.exit_status()
    );

    let mut console = in_volume();
    assert_eq!(
        console.run("definitely-not-a-program"),
        statuses.could_not_run,
        "case \"a machine's console\": expected the status of a command nothing resolved"
    );
    assert_eq!(
        console.output(),
        ["sh: definitely-not-a-program: command not found"],
        "case \"a machine's console\": expected the diagnostic on the console too, got {:?}",
        console.output()
    );

    let mut apart = in_volume().reporting_both_streams();
    assert_eq!(
        apart.run("echo written\ndefinitely-not-a-program"),
        statuses.could_not_run,
        "case \"the two streams apart\": expected the status of a command nothing resolved"
    );
    assert_eq!(
        apart.output(),
        ["written"],
        "case \"the two streams apart\": expected only what the program wrote, got {:?}",
        apart.output()
    );
    assert_eq!(
        apart.errors(),
        ["sh: definitely-not-a-program: command not found"],
        "case \"the two streams apart\": expected the diagnostic on the error stream, got {:?}",
        apart.errors()
    );
}

/// RULE: the operands a nested `sh -c PROGRAM [NAME [ARGUMENT…]]` invocation carries after the
/// program text are that program's POSITIONAL PARAMETERS — POSIX (`sh`): "command_name … shall
/// be assigned to special parameter 0" and the operands after it "shall be assigned to the
/// positional parameters" from `1` on, while an invocation with "no operands after
/// command_string" leaves "the name that the shell was invoked by" as `$0`. `$#` counts the
/// arguments without that name, `$@` and `$*` are those arguments, and a position the
/// invocation gave nothing at expands to nothing, the way an unset parameter does.
#[test]
fn a_nested_programs_operands_are_its_positional_parameters() {
    // (case, the line, the line it wrote last)
    let written: [(&str, &str, &str); 8] = [
        (
            "the first operand is the name the program runs under",
            "sh -c 'echo name=$0' runner",
            "name=runner",
        ),
        (
            "an invocation with no operands keeps the shell's own name",
            "sh -c 'echo name=$0'",
            "name=sh",
        ),
        (
            "the operands after that name are its arguments",
            "sh -c 'echo args=$1,$2' runner alpha beta",
            "args=alpha,beta",
        ),
        (
            "a position the invocation gave nothing at expands to nothing",
            "sh -c 'echo third=[$3]' runner alpha",
            "third=[]",
        ),
        (
            "the count is of the arguments and not of the name",
            "sh -c 'echo count=$#' runner alpha beta",
            "count=2",
        ),
        (
            "a program given a name only has no arguments to count",
            "sh -c 'echo count=$#' runner",
            "count=0",
        ),
        (
            "all the arguments, joined",
            "sh -c 'echo all=$@' runner alpha beta",
            "all=alpha beta",
        ),
        (
            "and the other spelling of them",
            "sh -c 'echo star=$*' runner alpha beta",
            "star=alpha beta",
        ),
    ];
    for (case, line, expected) in written {
        let mut held = working_tree_session();
        held.run(line);
        let wrote = held.output().last().cloned().unwrap_or_default();
        assert_eq!(
            wrote, expected,
            "case {case:?}: expected {expected:?}, got {wrote:?}"
        );
    }

    // And what the program DOES with one: the status it exits with is the operand's.
    // (case, the line, the status the session ends with)
    let statuses: [(&str, &str, i32); 3] = [
        (
            "the status is the name the program was invoked under",
            "sh -c 'exit \"$0\"' 3",
            3,
        ),
        (
            "the status is one of its arguments",
            "sh -c 'exit \"$1\"' runner 4",
            4,
        ),
        (
            "an operand that is no number at all is the shell's own usage failure",
            "sh -c 'exit \"$1\"' runner not-a-number",
            Statuses::default().failure,
        ),
    ];
    for (case, line, expected) in statuses {
        let mut held = working_tree_session();
        let answered = held.run(line);
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the status {expected}, got {answered}"
        );
    }
}
