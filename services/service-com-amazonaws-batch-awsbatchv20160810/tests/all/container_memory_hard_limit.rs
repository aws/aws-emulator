// SPDX-License-Identifier: CC0-1.0

//! The MEMORY a job definition gives a container is a HARD limit on its run, end to end over
//! the wire.
//!
//! RULE under test: `ResourceRequirement$value` for the `MEMORY` type is "the memory hard
//! limit (in MiB) present to the container", and "if your container attempts to exceed the
//! memory specified, the container is terminated" — the deprecated
//! `ContainerProperties$memory` ("the hard limit (in MiB) of memory to present to the
//! container") states the same ceiling. So the figure is not something the API merely echoes:
//! a job whose container's program holds more than that much is KILLED part of the way
//! through it, which the job reports as `FAILED` with the kill's own reason and the exit
//! status a killed process carries — and whatever the container wrote BEFORE the kill is in
//! its log stream, while what it would have written afterwards is not. A program that stays
//! inside the limit runs to its end and the job succeeds.
//!
//! The rows differ along what the rule generalises over: the limit the definition states, HOW
//! it states it (the resource requirement or the deprecated member), how much memory the
//! program holds, and whether that much is inside or past the limit — including the same
//! program under two different limits, which is what makes the KILL the limit's doing rather
//! than the allocation's.
//!
//! Everything asserted is read back over the wire: the job through `DescribeJobs` and what the
//! container wrote through CloudWatch Logs' own `GetLogEvents`.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::array_jobs_and_placement::managed_compute_environment;
use crate::job_lifecycle::{job_definition, job_queue, until_status, Batch};

/// The log group AWS Batch delivers a job's container logs to when its configuration names no
/// other one, spelled out here rather than imported so a wrong constant in the implementation
/// cannot make a test pass. Source: `AttemptContainerDetail$logStreamName` ("The log group for
/// Batch jobs is `/aws/batch/job`").
const DEFAULT_LOG_GROUP: &str = "/aws/batch/job";

/// What the program writes BEFORE it allocates anything, which every row's container reaches.
const ALLOC_START: &str = "ALLOC_START";

/// What it writes once it has held everything it set out to hold — the completion marker a
/// container the runtime killed never reaches.
const ALLOC_DONE: &str = "ALLOC_DONE";

/// The `statusReason` of a job whose container the runtime killed for going past its memory
/// hard limit, spelled out here rather than imported for the reason above. The container
/// runtime's own sentence for such a kill, which is what AWS Batch reports for an
/// out-of-memory job.
const OUT_OF_MEMORY_REASON: &str = "OutOfMemoryError: Container killed due to memory usage";

/// The `statusReason` of a job whose container EXITED, whatever status it exited with.
const CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// The `exitCode` of a container a `SIGKILL` ended: a shell reports the status of a command a
/// signal ended as 128 plus the signal's number, and `SIGKILL` is signal 9.
const KILLED_EXIT_CODE: i64 = 137;

/// How many bytes the literal each program starts its chunk from carries.
const CHUNK_SEED_BYTES: usize = 1024;

/// How many times the program DOUBLES that literal, so one chunk is
/// `CHUNK_SEED_BYTES << CHUNK_DOUBLINGS` bytes — 64 KiB, small enough that the number of
/// copies a row holds decides how near the limit it gets.
const CHUNK_DOUBLINGS: u32 = 6;

/// How many bytes one chunk comes to.
const CHUNK_BYTES: usize = CHUNK_SEED_BYTES << CHUNK_DOUBLINGS;

/// How many bytes one mebibyte is — the unit a memory hard limit is stated in.
const BYTES_PER_MIB: usize = 1024 * 1024;

/// How many times a poll looks before it gives up: each clock-owed move of a job falls due
/// `state::JOB_PHASE_MILLIS` after the one before it, and a job makes a handful of them, so
/// this is many spans of headroom on a loaded machine. Every poll ends on its CONDITION.
const POLLS: usize = 240;

/// How long one look waits before the next, well under one clock-owed span.
fn poll_interval() -> std::time::Duration {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    std::time::Duration::from_millis((span / 10).max(5))
}

/// The program one row's container runs: it writes the start marker, builds one chunk by
/// doubling a literal, holds `copies` of that chunk in variables of its own, and writes the
/// completion marker only if it got that far.
///
/// Every value it holds is its own — no file, no network and no other process is involved — so
/// what the container occupies is `(copies + 1)` chunks, which is what the limit is weighed
/// against.
fn program(copies: usize) -> String {
    let mut lines = vec![
        format!("echo {ALLOC_START}"),
        format!("c={}", "x".repeat(CHUNK_SEED_BYTES)),
        "i=0".to_string(),
        format!("while [ $i -lt {CHUNK_DOUBLINGS} ]; do c=\"$c$c\"; i=$((i+1)); done"),
    ];
    for copy in 0..copies {
        lines.push(format!("v{copy}=$c"));
    }
    lines.push(format!("echo {ALLOC_DONE}"));
    lines.join("; ")
}

/// How many bytes of its own values the program of `copies` copies holds at its peak.
fn held_bytes(copies: usize) -> usize {
    CHUNK_BYTES.saturating_mul(copies.saturating_add(1))
}

/// One row: the limit the definition states, how it states it, and how much the program holds.
struct Row {
    case: &'static str,
    /// The memory hard limit, in MiB.
    limit_mib: usize,
    /// Whether the definition states it as the deprecated `memory` member instead of as a
    /// `MEMORY` resource requirement.
    as_deprecated_member: bool,
    /// How many copies of the chunk the program holds beside the chunk itself.
    copies: usize,
}

/// The rows of the table.
const ROWS: &[Row] = &[
    Row {
        case: "a program that holds more than its definition's MEMORY requirement",
        limit_mib: 4,
        as_deprecated_member: false,
        copies: 64,
    },
    Row {
        // The SAME program as the row above, under a limit that admits what it holds: the
        // kill is the limit's doing, not the allocation's.
        case: "the same program under a limit twice as high",
        limit_mib: 8,
        as_deprecated_member: false,
        copies: 64,
    },
    Row {
        case: "a program that holds well inside the same small limit",
        limit_mib: 4,
        as_deprecated_member: false,
        copies: 32,
    },
    Row {
        case: "the same limit stated as the deprecated memory member",
        limit_mib: 4,
        as_deprecated_member: true,
        copies: 64,
    },
    Row {
        case: "a limit far above what the program holds",
        limit_mib: 64,
        as_deprecated_member: false,
        copies: 64,
    },
];

impl Row {
    /// Whether this row's program asks for more memory than its container is given, which is
    /// what the runtime kills the container for.
    fn killed(&self) -> bool {
        held_bytes(self.copies) > self.limit_mib.saturating_mul(BYTES_PER_MIB)
    }

    /// The `containerProperties` of this row's job definition: the program, and the memory
    /// hard limit stated the way the row states it.
    fn container_properties(&self) -> Value {
        let mut properties = json!({
            "image": "busybox",
            "command": ["sh", "-c", program(self.copies)],
        });
        if self.as_deprecated_member {
            properties["vcpus"] = json!(1);
            properties["memory"] = json!(self.limit_mib);
        } else {
            properties["resourceRequirements"] = json!([
                {"type": "VCPU", "value": "1"},
                {"type": "MEMORY", "value": self.limit_mib.to_string()},
            ]);
        }
        properties
    }
}

/// The messages CloudWatch Logs holds in `stream` of `DEFAULT_LOG_GROUP`, oldest first. A
/// stream it holds nothing for reads as no messages.
async fn delivered(batch: &Batch, stream: &str) -> Vec<String> {
    let (status, body) = batch
        .logs(
            "GetLogEvents",
            json!({
                "logGroupName": DEFAULT_LOG_GROUP,
                "logStreamName": stream,
                "startFromHead": true,
            }),
        )
        .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Poll the stream `name` until it holds at least one message, then answer what it holds.
///
/// The events are delivered while the container is RUNNING, so a reader that found the job
/// terminal may still be ahead of the delivery: this waits for the CONDITION.
async fn until_delivered(batch: &Batch, case: &str, name: &str) -> Vec<String> {
    for _ in 0..POLLS {
        let messages = delivered(batch, name).await;
        if !messages.is_empty() {
            return messages;
        }
        tokio::time::sleep(poll_interval()).await;
    }
    panic!("case {case:?}: expected the container's log stream {name:?} to hold what it wrote");
}

/// RULE: a container that holds more memory than its job definition gives it is KILLED — the
/// job ends FAILED with the kill's reason and the status a killed process exits with, and its
/// log stream holds what the container wrote before the kill and nothing it would have written
/// after — while one that stays inside that limit runs to its end and the job succeeds.
#[tokio::test]
async fn a_container_that_goes_past_its_declared_memory_is_killed_part_way_through() {
    let batch = Batch::new();
    let environment = managed_compute_environment(&batch, "ce-memory", state::STATE_ENABLED).await;
    let queue_arn = job_queue(&batch, "q-memory", &environment).await;

    // Every row's job is submitted first and read afterwards, so the runs happen together in
    // the one emulator rather than one after another.
    let mut submitted: Vec<(&Row, String, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let definition_name = format!("jd-memory-{index}");
        let (definition, _) =
            job_definition(&batch, &definition_name, row.container_properties()).await;
        let answered = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("j-memory-{index}"),
                    "jobQueue": &queue_arn,
                    "jobDefinition": definition,
                }),
            )
            .await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a jobId, got {answered}", row.case))
            .to_string();
        submitted.push((row, definition_name, job_id));
    }

    for (row, definition_name, job_id) in &submitted {
        let case = row.case;
        let wanted = if row.killed() {
            state::JOB_FAILED
        } else {
            state::JOB_SUCCEEDED
        };
        let detail = until_status(&batch, case, job_id, wanted).await;

        let (expected_reason, expected_exit) = if row.killed() {
            (OUT_OF_MEMORY_REASON, KILLED_EXIT_CODE)
        } else {
            (CONTAINER_EXITED_REASON, 0)
        };
        assert_eq!(
            detail["statusReason"].as_str(),
            Some(expected_reason),
            "case {case:?}: a container holding {} bytes under a {} MiB limit must report \
             {expected_reason:?}, got {detail}",
            held_bytes(row.copies),
            row.limit_mib
        );
        assert_eq!(
            detail["container"]["exitCode"].as_i64(),
            Some(expected_exit),
            "case {case:?}: expected the container to report exit code {expected_exit}, got \
             {detail}"
        );
        // `AttemptContainerDetail$reason` / `ContainerDetail$reason` is "a short (255 max
        // characters) human-readable string to provide additional details for a running or
        // stopped container", which for a killed one is the kill.
        assert_eq!(
            detail["container"]["reason"].as_str(),
            Some(expected_reason),
            "case {case:?}: expected the container's detail to be {expected_reason:?}, got \
             {detail}"
        );
        assert_eq!(
            detail["attempts"][0]["statusReason"].as_str(),
            Some(expected_reason),
            "case {case:?}: the attempt that ran it reports the same, got {detail}"
        );

        // What the container WROTE: the start marker always, and the completion marker only
        // where the program really reached it.
        let stream = detail["attempts"][0]["container"]["logStreamName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a log stream, got {detail}"))
            .to_string();
        assert!(
            stream.starts_with(definition_name.as_str()),
            "case {case:?}: the stream of this job's attempt is its own, got {stream:?}"
        );
        let messages = until_delivered(&batch, case, &stream).await;
        assert!(
            messages.iter().any(|message| message == ALLOC_START),
            "case {case:?}: every container reaches the start marker, got {messages:?}"
        );
        let completed = messages.iter().any(|message| message == ALLOC_DONE);
        assert_eq!(
            completed,
            !row.killed(),
            "case {case:?}: a killed container never writes the completion marker and one that \
             ran to its end always does, got {messages:?}"
        );
    }
}
