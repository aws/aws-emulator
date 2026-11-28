// SPDX-License-Identifier: CC0-1.0

//! The PROGRAM a placed job's container runs, and what the job reports from it.
//!
//! # The rule this module implements
//!
//! A job that has been placed runs the program its container declares:
//! `ContainerProperties$command` is "the command that's passed to the container", which is
//! the argument vector the image runs. So what the job does from `STARTING` on is what that
//! program does: `JobStatus$RUNNING` is where "the job is in the `RUNNING` state"  — the
//! container of the job has started — `ContainerDetail$exitCode` is "the exit code returned
//! upon completion", and the job's terminal status is the one that exit code decides:
//! `SUCCEEDED` for a container that exited zero, `FAILED` for any other status, each
//! carrying the `statusReason` the compute plane words for a task whose essential container
//! exited ([`ESSENTIAL_CONTAINER_EXITED_REASON`]). Nothing about the run is authored here:
//! the status, the output and the instants come from performing the caller's own program.
//!
//! # How the program is performed — and what \"performed\" excludes
//!
//! IN THIS PROCESS, and no process is started for it: there is no `std::process::Command`
//! here, no host `PATH` resolution, no host file opened or written, no host environment
//! variable read and no socket. The one session this workspace holds for a caller-declared
//! program does the work ([`::service_common::posix_shell_session`], which
//! `service-com-amazonaws-sagemaker-sagemaker`'s own container runs and
//! `service-com-amazonaws-codebuild-codebuild-20161006`'s buildspec lines drive too): the
//! GRAMMAR is a parser crate's — it builds a syntax tree and runs nothing — and what answers
//! a command is a FIXED CATALOG of session-local utilities whose whole effect is inside the
//! session itself (its output, its status, its variables and the in-memory tree its file
//! utilities work over). So this is not a shell with the host behind it: a command the
//! catalog does not name, and every command form and expansion the session does not perform,
//! FAILS CLOSED with the status a shell answers for a command it could not run. A declared
//! program therefore chooses between those utilities and nothing at all; it cannot choose a
//! program on the machine running the emulator.
//!
//! Every bound is the emulator's, held in code the program cannot reach: the session's own
//! budget on how many commands one run performs, how long and how deeply nested a line may
//! be, how far a word may expand and how much its tree may hold, beside
//! [`MAX_CONTAINER_OUTPUT_LINES`] on what one run may leave behind and
//! [`MAX_CONTAINER_RUNS_PER_DRAIN`] on how many programs one call performs. The work is
//! synchronous and CPU-bound, so it is performed off the request worker
//! ([`run_container_program_off_worker`]) and the worker serving the request is released
//! while it runs.
//!
//! The vector is handed to the session as ONE command line, quoted word by word by the
//! maintained POSIX word quoter (`shell-words`) so a word carrying a space, a quote or a
//! shell operator stays ONE word rather than becoming syntax. Its first word is the PROGRAM,
//! which a container command names by path (`/bin/sh`), and the session's catalog names a
//! utility by the name `PATH` resolution would find it under, so that program's final path
//! component is the command name the session is given. The variables the session starts with
//! are the job's own effective `environment` ("the environment variables to pass to a
//! container") beside the ones AWS Batch sets itself ([`super::job_variables`]), and nothing
//! of this process's.
//!
//! # A program nothing performed CLAIMS NOTHING
//!
//! Whether the declared program is one this emulator can perform at all is decided BEFORE it
//! is performed ([`performable_program`]), so a container declaring NO command — whose
//! program is the one inside the caller's own image — and one naming a program the catalog
//! does not hold are both answered [`ContainerRun::NotPerformed`], which carries no exit
//! status and no output. Nothing ran, so nothing of a run is reported: such a job ends in
//! the modelled failure status with a reason NAMING what was not performed, never with an
//! exit code, a `startedAt` or the reason a container that really exited carries.
//!
//! # The MEMORY the job definition gives the container
//!
//! `ResourceRequirement$type` `MEMORY` is "the memory hard limit (in MiB) present to the
//! container", and "if your container attempts to exceed the memory specified, the container
//! is terminated" — so that figure is not a value the API echoes: it BOUNDS the run
//! ([`container_memory_limit_bytes`]). A program whose values would take the container past it
//! is KILLED where it stands ([`ContainerRun::KilledForMemory`]): the program never finishes,
//! so the job ends `FAILED` with the reason that kill carries
//! ([`OUT_OF_MEMORY_KILLED_REASON`]) and the exit status a killed process carries
//! ([`OUT_OF_MEMORY_KILL_STATUS`]) rather than with a status the program answered — while the
//! lines it had already written are delivered to its log stream, because those are what the
//! container really produced before the kill. A container that states no such requirement runs
//! under no limit of its own, and a container that states MORE memory than this emulator gives
//! one program ([`MAX_DECLARED_MEMORY_SESSION_BYTES`]) is held to that ceiling instead.
//!
//! # When it is performed
//!
//! `STARTING` is where "the relevant container initiation operations are underway", so the
//! program of a job the scheduler has just placed is performed while it is there: once per
//! job ([`ComAmazonawsBatchAwsbatchv20160810State::jobs_awaiting_container_run`]), by the
//! call that finds it waiting and with this store's lock RELEASED
//! ([`settle_job_container_runs`], the shape [`super::image_pull`] and [`super::job_logs`]
//! follow). The outcome is then recorded on the job, and the clock materialises it: the move
//! into `RUNNING` and the terminal status one span later are both read off that one
//! recording, so what a job reports never depends on how quickly anybody called.

use super::*;

/// How many lines of what the container wrote one run keeps.
///
/// THIS NUMBER IS THE EMULATOR'S: the program is caller-supplied, so what it can leave
/// behind is bounded here.
pub const MAX_CONTAINER_OUTPUT_LINES: usize = 200;

/// How many bytes ONE kept line may carry, under [`MAX_CONTAINER_OUTPUT_LINES`]' rule
/// applied to one line. A longer line is truncated rather than dropped, so what the
/// container wrote is still reported.
pub const MAX_CONTAINER_OUTPUT_LINE_BYTES: usize = 1_024;

/// How many words one container's command may carry, and how many bytes one of those words
/// may be.
///
/// EMULATOR ceilings, stated here so the vector is bounded where it is BUILT rather than
/// only where it is performed: a vector past either one names no program at all.
pub const MAX_PROGRAM_WORDS: usize = 200;
/// See [`MAX_PROGRAM_WORDS`].
pub const MAX_PROGRAM_WORD_BYTES: usize = 4 * 1024;

/// How many container programs ONE drain performs.
///
/// This store's OWN bound: performing a program is CPU-bound work, and the jobs waiting for
/// one are only bounded by the jobs store itself ([`limits::MAX_JOBS`]), so one call never
/// performs more than this many. What a drain leaves behind keeps the clock pump awake
/// ([`ComAmazonawsBatchAwsbatchv20160810State::has_pending_container_runs`]), so nothing is
/// dropped — it is performed a tick later.
pub const MAX_CONTAINER_RUNS_PER_DRAIN: usize = 10;

/// The `statusReason` of a job whose container EXITED, whatever status it exited with.
///
/// A Batch job's container is an Amazon ECS task's container, and the words are that
/// service's own for a task whose essential container has exited — the sibling constant in
/// the Amazon ECS crate carries the same sentence for the task's `stoppedReason`. Fixed
/// text with no variable part, so it reports nothing about the job.
pub const ESSENTIAL_CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// The `ContainerDetail$exitCode` of a container the runtime KILLED for going past the memory
/// its job definition gives it: a killed process exits with the status of the signal that
/// killed it, which is the status the session performing the program ends with
/// ([`::service_common::posix_shell_session::KILLED_BY_MEMORY_STATUS`], 128 plus `SIGKILL`'s
/// number) — read from that one constant rather than restated, so the status the session sets
/// and the status the job reports cannot drift.
pub const OUT_OF_MEMORY_KILL_STATUS: i32 =
    ::service_common::posix_shell_session::KILLED_BY_MEMORY_STATUS;

/// The `statusReason` of a job whose container the runtime killed for going past that memory,
/// in the container runtime's own words for such a kill — the sibling constant in the Amazon
/// ECS crate carries the same sentence for the container's `reason`, which is where a job whose
/// container runs as a task reads it from ([`super::ecs_task`]). Fixed text with no variable
/// part, so it reports nothing about the job.
pub const OUT_OF_MEMORY_KILLED_REASON: &str =
    "OutOfMemoryError: Container killed due to memory usage";

/// How many bytes of its values one container's program may hold at once when its job
/// definition states a MEMORY requirement — the ceiling that requirement is read UNDER
/// ([`container_memory_bounds`]).
///
/// THIS NUMBER IS THE EMULATOR'S, and it is what makes the declared requirement the operative
/// limit for a container sized near the smallest one a definition may state
/// (`ResourceRequirement$value` for `MEMORY` is "the memory hard limit (in MiB)", and "you
/// must specify at least 4 MiB of memory"): such a container is held to its OWN figure and
/// killed for going past it, which is the rule, rather than to a ceiling of this emulator's
/// below that figure. A container that states MORE memory than this is held to this instead —
/// this process cannot give one container hundreds of mebibytes of itself — and a program past
/// it is one whose assignment did not happen, as it is for every program this session performs
/// for a job that states no requirement at all.
///
/// Sized against what one pass can come to: at most [`MAX_CONTAINER_RUNS_PER_DRAIN`] programs
/// are performed per drain, and only a program that deliberately fills its session reaches
/// this at all.
pub const MAX_DECLARED_MEMORY_SESSION_BYTES: usize = 8 * 1024 * 1024;

/// How many bytes one mebibyte is, which is the unit the `MEMORY` resource requirement and the
/// deprecated `memory` member state a container's hard limit in.
const BYTES_PER_MIB: u64 = 1024 * 1024;

/// How many times over one program may MATERIALISE the memory its container holds, as it
/// builds the values it ends up holding ([`container_memory_bounds`]).
///
/// THIS NUMBER IS THE EMULATOR'S: a program fills its memory by expanding values it already
/// holds — each expansion materialised, held and then let go of as the next one replaces it —
/// so what it materialises over a whole program is a multiple of what it holds at any instant,
/// and that multiple is bounded here rather than by the request.
const EXPANSIONS_PER_HELD_BYTE: usize = 4;

/// What one placed job's container program did, or the fact that nothing was performed for
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerRun {
    /// The declared program WAS performed: the status it exited with and the lines it wrote.
    Performed {
        /// The status the program exited with, as the session performed it — the job's
        /// `ContainerDetail$exitCode`.
        status: i32,
        /// The lines the program wrote, in the order it wrote them, bounded by
        /// [`MAX_CONTAINER_OUTPUT_LINES`]. They are the container's own output, so they are
        /// what its log stream holds ([`super::job_logs`]).
        output: Vec<String>,
    },
    /// NOTHING was performed for the container, and this is why. No status and no output are
    /// claimed, because none was produced.
    NotPerformed {
        /// The job's `statusReason`: what was not performed, in this emulator's own words,
        /// because no container exited.
        reason: String,
    },
    /// The declared program WAS performed and the RUNTIME KILLED the container part of the way
    /// through it: what the program held went past the memory hard limit the job definition
    /// gives the container, so the container was killed rather than left to finish.
    ///
    /// No status of the PROGRAM is carried, because the program did not end: the status such a
    /// container exits with is the signal's ([`OUT_OF_MEMORY_KILL_STATUS`]). What it had
    /// already written IS carried: those lines are what the container produced before the
    /// kill, and its log stream holds exactly them.
    KilledForMemory {
        /// The lines the program wrote before the kill, under
        /// [`MAX_CONTAINER_OUTPUT_LINES`] as [`Self::Performed`]'s are.
        output: Vec<String>,
    },
}

impl ContainerRun {
    /// Whether a program was performed at all — which is what decides whether the job ever
    /// reaches `RUNNING` and reports an instant its work started.
    #[must_use]
    pub fn performed(&self) -> bool {
        matches!(self, Self::Performed { .. } | Self::KilledForMemory { .. })
    }

    /// The lines the program wrote, which are the container's own output and so what its log
    /// stream holds ([`super::job_logs`]) — including the ones a program the runtime KILLED
    /// had written before the kill. Empty for a run nothing performed, which wrote nothing.
    #[must_use]
    pub fn output(&self) -> &[String] {
        match self {
            Self::Performed { output, .. } | Self::KilledForMemory { output } => output,
            Self::NotPerformed { .. } => &[],
        }
    }

    /// The `JobStatus` the job this run belongs to ends in, and the `statusReason` that goes
    /// with it.
    ///
    /// The exit status decides: zero is the job that succeeded, and every other status is a
    /// job that failed, each reported with the reason a container that exited carries
    /// ([`ESSENTIAL_CONTAINER_EXITED_REASON`]). A container the runtime KILLED for going past
    /// its memory hard limit failed too, with the reason that kill carries
    /// ([`OUT_OF_MEMORY_KILLED_REASON`]) — the program never finished, so no status of its own
    /// is what ends the job. A program nothing performed ends the job with the reason it
    /// carries instead.
    #[must_use]
    pub fn ending(&self) -> (&'static str, String) {
        match self {
            Self::Performed { status: 0, .. } => (
                JOB_SUCCEEDED,
                ESSENTIAL_CONTAINER_EXITED_REASON.to_string(),
            ),
            Self::Performed { .. } => (JOB_FAILED, ESSENTIAL_CONTAINER_EXITED_REASON.to_string()),
            Self::KilledForMemory { .. } => (JOB_FAILED, OUT_OF_MEMORY_KILLED_REASON.to_string()),
            Self::NotPerformed { reason } => (JOB_FAILED, reason.clone()),
        }
    }

    /// The exit status this run reports, and `None` for a run nothing performed.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Performed { status, .. } => Some(*status),
            Self::KilledForMemory { .. } => Some(OUT_OF_MEMORY_KILL_STATUS),
            Self::NotPerformed { .. } => None,
        }
    }
}

/// How this emulator states that a job's container declared no program for it to run: the
/// image's own entrypoint is what AWS would start, and this emulator starts no image.
///
/// Not an AWS sentence, and deliberately not the reason a container that exited carries: no
/// container exited, so nothing about an exit status may be claimed. A job that declares a
/// command this emulator CAN perform is reported from that program's own outcome instead.
#[must_use]
pub fn no_declared_program(image: Option<&str>) -> String {
    let runs = match image {
        Some(image) => format!("the image \"{image}\""),
        None => "the job's container".to_string(),
    };
    format!(
        "This emulator starts no container image, so the program {runs} declares as its own \
         entrypoint was never started: declare a command this emulator performs."
    )
}

/// How this emulator states that the program a job's container DECLARED is not one it
/// performs: the container's program was never started, so no status is claimed for it.
#[must_use]
pub fn program_not_performed(command: &str) -> String {
    format!(
        "This emulator performs a job container's declared command itself and provides no \
         program named \"{command}\", so the container's program was never started and the \
         job performed no work."
    )
}

/// The argument vector a container runs: the command the job's effective container carries.
///
/// Empty when it carries none — a container running the program its IMAGE declares, which no
/// request here names — and empty too for a vector past this emulator's own ceilings
/// ([`MAX_PROGRAM_WORDS`], [`MAX_PROGRAM_WORD_BYTES`]): a caller supplies the words, so how
/// many of them there may be and how long each may be is decided here and not by the request.
#[must_use]
pub fn container_program(command: Option<&[String]>) -> Vec<String> {
    let declared: Vec<String> = command
        .unwrap_or_default()
        .iter()
        .take(MAX_PROGRAM_WORDS + 1)
        .cloned()
        .collect();
    if declared.len() > MAX_PROGRAM_WORDS
        || declared
            .iter()
            .any(|word| word.len() > MAX_PROGRAM_WORD_BYTES)
    {
        return Vec::new();
    }
    declared
}

/// The command name the session would be given for `program`, when that name is one the
/// session's catalog performs — and `None` for a vector naming a program it provides none
/// for, which is a program nothing starts.
///
/// A container command names its program by path (`/bin/sh`) and the catalog names a utility
/// by the name `PATH` resolution would find it under, so the final path component is the
/// command name. The catalog is the session's own: its built-in utilities, the text filters
/// it performs, and the file utilities a session working over a tree provides (which is how
/// `sh -c` is performed).
#[must_use]
pub fn performable_program(program: &[String]) -> Option<&str> {
    let executable = program.first()?;
    let name = executable.rsplit('/').next().unwrap_or(executable);
    let performed = ::service_common::posix_shell_session::BUILT_IN_COMMANDS.contains(&name)
        || ::service_common::posix_shell_session::VOLUME_COMMANDS.contains(&name)
        || ::service_common::posix_shell_text_filters::TEXT_FILTER_COMMANDS.contains(&name);
    performed.then_some(name)
}

/// The memory HARD LIMIT one container runs under, in bytes, and `None` for a container whose
/// job definition states none — which runs under no limit of its own.
///
/// `ResourceRequirement$type` `MEMORY` is "the memory hard limit (in MiB) present to the
/// container", and "if your container attempts to exceed the memory specified, the container
/// is terminated", so a definition stating it states the ceiling the container runs under. A
/// definition that states it the older way instead — `ContainerProperties$memory`, "the hard
/// limit (in MiB) of memory to present to the container", which the same member's
/// documentation deprecates in favour of the requirement above — states the same ceiling, so
/// both are read and the requirement decides where a definition carries both.
#[must_use]
pub fn container_memory_limit_bytes(container: &ContainerDetail) -> Option<u64> {
    let requirements = container.resource_requirements.as_deref().unwrap_or_default();
    let declared = stated_resource_value(requirements, RESOURCE_TYPE_MEMORY)
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| container.memory.map(i64::from))
        .filter(|mib| *mib > 0)?;
    Some(
        u64::try_from(declared)
            .unwrap_or_default()
            .saturating_mul(BYTES_PER_MIB),
    )
}

/// The bounds and the memory a session performing one container's program is given, for a
/// container whose hard limit is `memory_limit_bytes`.
///
/// The limit is the CONTAINER's own, so it is what the session is killed for going past
/// ([`::service_common::posix_shell_session::SessionConfig::memory`]), and how many bytes the
/// session may hold is that same figure under this emulator's own ceiling
/// ([`MAX_DECLARED_MEMORY_SESSION_BYTES`]) — which is what makes the container's figure the
/// operative one rather than a bound of this emulator below it. A container stating no limit
/// keeps the bounds every other program this service performs is held to.
fn container_memory_bounds(
    memory_limit_bytes: Option<u64>,
) -> (
    ::service_common::posix_shell_session::Bounds,
    Option<usize>,
) {
    let mut bounds = ::service_common::posix_shell_session::Bounds::default();
    let Some(limit) = memory_limit_bytes else {
        return (bounds, None);
    };
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let cap = limit.min(MAX_DECLARED_MEMORY_SESSION_BYTES);
    bounds.session_bytes = bounds.session_bytes.max(cap);
    // What one WORD may expand to, and what one program may materialise as it goes, are the
    // same question asked of the memory the container has: a program fills its memory by
    // expanding the values it already holds, so a word may expand to as much as the container
    // may hold, and the program may materialise several times that much on the way there — a
    // value is expanded to build the next one, and the one it replaces is let go of. Neither
    // is lowered below the figure every other program this service performs is held to.
    bounds.expansion_bytes = bounds.expansion_bytes.max(cap);
    bounds.line_expansion_bytes = bounds
        .line_expansion_bytes
        .max(cap.saturating_mul(EXPANSIONS_PER_HELD_BYTE));
    (bounds, Some(limit))
}

/// Perform `program` — a container's argument vector — and report what it did, or that
/// nothing was performed for it.
///
/// `image` is the image the job's container runs, which the reason for a container declaring
/// no program at all names. `memory_limit_bytes` is the memory hard limit the container runs
/// under ([`container_memory_limit_bytes`]): a program that goes past it is KILLED, exactly as
/// the runtime kills a container that attempts to exceed it, and what it had written before
/// the kill is what the run reports.
///
/// Pure computation over the caller's own words: the session starts no process, opens no host
/// file and reaches no network, and the work it performs is bounded by that session's own
/// bounds.
#[must_use]
pub fn run_container_program(
    program: &[String],
    image: &str,
    environment: &[(String, String)],
    memory_limit_bytes: Option<u64>,
) -> ContainerRun {
    if program.is_empty() {
        return ContainerRun::NotPerformed {
            reason: no_declared_program((!image.is_empty()).then_some(image)),
        };
    }
    let Some(name) = performable_program(program) else {
        let declared = program.first().map_or("", String::as_str);
        return ContainerRun::NotPerformed {
            reason: program_not_performed(declared),
        };
    };
    let mut words: Vec<&str> = vec![name];
    words.extend(program.iter().skip(1).map(String::as_str));
    let line = ::shell_words::join(words);
    let variables: BTreeMap<String, String> = environment.iter().cloned().collect();
    let (bounds, memory) = container_memory_bounds(memory_limit_bytes);
    let config = ::service_common::posix_shell_session::SessionConfig {
        output: Some(::service_common::posix_shell_session::OutputLimits {
            lines: MAX_CONTAINER_OUTPUT_LINES,
            line_bytes: MAX_CONTAINER_OUTPUT_LINE_BYTES,
        }),
        bounds,
        memory,
        ..::service_common::posix_shell_session::SessionConfig::default()
    };
    // The container has storage of its own — the writable layer and the volumes a task is
    // given — so the session performs the file utilities over its own tree, which is also
    // what makes `sh -c` a program it can run.
    let mut session =
        ::service_common::posix_shell_session::ShellSession::new(variables, config)
            .with_file_utilities();
    let status = session.run(&line);
    if session.killed_for_memory() {
        // The container went past the memory its definition gives it, so the runtime killed it:
        // the program never finished, so no status of its own is reported — only the lines it
        // had written by then.
        return ContainerRun::KilledForMemory {
            output: session.output().to_vec(),
        };
    }
    // An `exit` the program performed is the status it ended with, the way a shell takes it.
    let status = session.exit_status().unwrap_or(status);
    ContainerRun::Performed {
        status,
        output: session.output().to_vec(),
    }
}

/// [`run_container_program`] off the request worker: performing a caller's own program is
/// synchronous, CPU-bound work bounded by the session's own command budget, so it goes to the
/// blocking pool and the worker serving the request is released while it runs.
///
/// `None` is a fault of this process rather than of the request — the work could not be
/// performed at all — so nothing is recorded for it and the next drain attempts it again.
pub async fn run_container_program_off_worker(
    program: &[String],
    image: &str,
    environment: &[(String, String)],
    memory_limit_bytes: Option<u64>,
) -> Option<ContainerRun> {
    let program = program.to_vec();
    let image = image.to_string();
    let environment = environment.to_vec();
    ::tokio::task::spawn_blocking(move || {
        run_container_program(&program, &image, &environment, memory_limit_bytes)
    })
    .await
    .ok()
}

/// What ONE job's container run needs: the job it belongs to, the program its container
/// declares, the image that container runs and the environment it was given.
#[derive(Debug, Clone)]
pub struct JobContainerRun {
    /// The job the outcome belongs to.
    pub job_id: String,
    /// The argument vector its container runs ([`container_program`]).
    pub program: Vec<String>,
    /// The image its container runs, as `ContainerDetail$image` holds it.
    pub image: String,
    /// The environment variables the job's effective container carries, in the order they
    /// were merged, followed by the ones AWS Batch sets itself
    /// ([`super::job_variables`]).
    pub environment: Vec<(String, String)>,
    /// The memory HARD LIMIT its container runs under, in bytes
    /// ([`container_memory_limit_bytes`]) — `None` for a container whose definition states
    /// none.
    pub memory_limit_bytes: Option<u64>,
}

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// Whether `job` is a placed job whose container program has not been performed yet.
    ///
    /// The ONE predicate behind both the list of such jobs
    /// ([`Self::jobs_awaiting_container_run`]) and the question the clock pump asks
    /// ([`Self::has_pending_container_runs`]), so the two cannot disagree and asking costs
    /// nothing but a read of the record.
    fn awaits_a_container_run(job: &Job) -> bool {
        job.status == JOB_STARTING
            && job.container_run.is_none()
            && job.task_arn.is_none()
            && matches!(job.image_fetch, Some(ImageFetch::Served) | None)
    }

    /// The placed jobs whose container program has not been performed yet.
    ///
    /// A job is one of them from the instant the placement put it into `STARTING` — where the
    /// container initiation is underway — until its outcome is recorded, and only when the
    /// image its container runs was there to be fetched: a fetch the registry refused ended
    /// the initiation before any container was created, so there is no program to perform.
    ///
    /// A job whose container RUNS as an Amazon ECS task is not one of them: that task performs
    /// the program, and everything the job reports of the run is read off it
    /// ([`super::ecs_task`]), so performing it here as well would run one container's program
    /// twice. This is the program of a job placed on capacity that can take no task.
    /// Costs one walk of the jobs map, which [`limits::MAX_JOBS`] bounds, and answers at most
    /// [`MAX_CONTAINER_RUNS_PER_DRAIN`] of them.
    pub fn jobs_awaiting_container_run(&self) -> Vec<JobContainerRun> {
        self.jobs
            .values()
            .filter(|job| Self::awaits_a_container_run(job))
            .filter_map(|job| self.job_container_run_request(&job.job_id))
            .take(MAX_CONTAINER_RUNS_PER_DRAIN)
            .collect()
    }

    /// What performing the container program of the job `id` needs: the program its effective
    /// container declares and the environment that container is given.
    ///
    /// The ONE place a job's run is read off its record, so the program this service performs
    /// ([`settle_job_container_runs`]) and the duration that program states
    /// ([`super::attempt_work`]) are read from the same container — the one the job is
    /// actually RUN with, which is its effective container with this job's parameters
    /// substituted into its command and its environment ([`super::job_parameters`]).
    /// `None` for a job this store holds no container for.
    pub(super) fn job_container_run_request(&self, id: &str) -> Option<JobContainerRun> {
        let job = self.jobs.get(id)?;
        let container = self.job_run_container(&job.job_id)?;
        let mut environment: Vec<(String, String)> = container
            .environment
            .iter()
            .flatten()
            .filter_map(|pair| Some((pair.name.clone()?, pair.value.clone().unwrap_or_default())))
            .collect();
        // And the variables AWS Batch sets itself, after the declared ones so the service's
        // own prefix decides them ([`super::job_variables`]).
        environment.extend(self.reserved_job_variables(&job.job_id));
        Some(JobContainerRun {
            job_id: job.job_id.clone(),
            program: container_program(container.command.as_deref()),
            image: container.image.clone().unwrap_or_default(),
            environment,
            memory_limit_bytes: container_memory_limit_bytes(&container),
        })
    }

    /// Record what each job's container program did.
    ///
    /// Written once per job: a job that already carries an outcome keeps it, so a later call
    /// re-performs nothing and what a job reports cannot change under a reader.
    pub fn record_container_runs(&mut self, outcomes: Vec<(String, ContainerRun)>) {
        for (id, outcome) in outcomes {
            if let Some(job) = self.jobs.get_mut(&id) {
                if job.container_run.is_none() {
                    job.container_run = Some(outcome);
                }
            }
        }
    }

    /// Whether a placed job is still waiting for its container program to be performed, which
    /// is what keeps the clock pump awake until every job can take its next move.
    ///
    /// Answers the PREDICATE rather than building the runs: the records are what decide it, so
    /// a tick of the clock costs one walk of the jobs map and never a substitution
    /// ([`super::job_parameters`]) or a clone of a container.
    pub(super) fn has_pending_container_runs(&self) -> bool {
        self.jobs.values().any(Self::awaits_a_container_run)
    }

    /// What `id`'s container program did, or `None` while it has not been performed.
    ///
    /// A job whose container this store holds none of has no program to perform — nothing of
    /// a container job is stored for it — so nothing was performed for it either.
    pub(super) fn container_run_of(&self, id: &str) -> Option<ContainerRun> {
        let job = self.jobs.get(id)?;
        if job.container.is_none() {
            return Some(ContainerRun::NotPerformed {
                reason: no_declared_program(None),
            });
        }
        job.container_run.clone()
    }

    /// Move `id` into `RUNNING` at `at`, owing its end one span later.
    ///
    /// `JobStatus$RUNNING` is where the job's container is running the program it declared,
    /// and `JobDetail$startedAt` is "when the job transitioned from the `STARTING` state to
    /// the `RUNNING` state" — which is also what `AttemptDetail$startedAt` records for the
    /// attempt that placement started, so the two report one instant.
    ///
    /// The lines the program wrote are delivered from here, while the job is RUNNING: a
    /// container writes its output as it runs rather than when it exits, so a client watching
    /// the job's log stream finds them before the job reaches its terminal status.
    pub(super) fn start_running_job(&mut self, id: &str, at: i64) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.started_at = Some(at);
            if let Some(attempt) = job
                .attempts
                .last_mut()
                .filter(|attempt| attempt.ended_at.is_none())
            {
                attempt.started_at = Some(at);
            }
        }
        self.queue_container_output(id, at);
        self.set_job_status(id, JOB_RUNNING, at);
        // One span, or the instant this attempt runs out of the time its own `timeout`
        // allows it, whichever falls first ([`super::attempt_timeout`]).
        self.set_job_due_bounded_by_attempt_deadline(id, at.saturating_add(JOB_PHASE_MILLIS));
    }

    /// End the ATTEMPT `id` was running at `at` with what its container's program did: the
    /// status that program exited with decides the job's own, and the exit code is reported on
    /// the container and on the attempt that ran it.
    ///
    /// Whether a failing program ends the JOB is the retry strategy's to decide
    /// ([`super::job_retries`]), which is what the answer reports: a job with attempts left
    /// goes back to `RUNNABLE` for another run of the same program.
    ///
    /// What the program WROTE was delivered when the job entered `RUNNING`
    /// ([`Self::start_running_job`]), as a container's output is.
    pub(super) fn end_running_job(
        &mut self,
        id: &str,
        run: &ContainerRun,
        at: i64,
    ) -> AttemptEnding {
        let (status, reason) = run.ending();
        let code = run.exit_code();
        if let Some(job) = self.jobs.get_mut(id) {
            if let Some(container) = job.container.as_mut() {
                container.exit_code = code;
            }
            if let Some(attempt) = job
                .attempts
                .last_mut()
                .filter(|attempt| attempt.ended_at.is_none())
            {
                attempt.exit_code = code;
            }
        }
        self.end_job_attempt(id, status, Some(&reason), at)
    }

    /// Queue the lines `id`'s container wrote, for delivery to the log stream the `awslogs`
    /// driver opened for its task ([`super::job_logs`]).
    ///
    /// The destination is the one the placement decided — the driver's own options over the
    /// stream that placement named — so the events land in the stream the job reports and
    /// nowhere else. A container that wrote nothing queues nothing, and a container whose
    /// output goes to another log driver has no CloudWatch Logs stream to deliver to.
    pub(super) fn queue_container_output(&mut self, id: &str, started_at: i64) {
        let Some(job) = self.jobs.get(id) else {
            return;
        };
        let Some(ContainerRun::Performed { output, .. } | ContainerRun::KilledForMemory { output }) =
            job.container_run.as_ref()
        else {
            return;
        };
        if output.is_empty() {
            return;
        }
        let events: Vec<(i64, String)> = output
            .iter()
            .enumerate()
            .map(|(index, line)| {
                (
                    started_at.saturating_add(i64::try_from(index).unwrap_or_default()),
                    line.clone(),
                )
            })
            .collect();
        let region = arn_region(&job.job_arn).unwrap_or(DEFAULT_REGION).to_string();
        let configuration = job
            .container
            .as_ref()
            .and_then(|container| container.log_configuration.clone());
        let Some(stream) = job
            .attempts
            .last()
            .and_then(|attempt| attempt.log_stream_name.clone())
        else {
            return;
        };
        let Some(target) = job_log_target(configuration.as_ref(), &stream, &region) else {
            return;
        };
        self.queue_log_stream(JobLogStream { events, ..target });
    }
}

/// Perform the container program of every placed job waiting for one, and record what each
/// one did.
///
/// Called by the drain every operation of this service's job plane makes, and by the clock
/// pump, with this store's guard dropped: performing a caller's program is CPU-bound work, so
/// no lock of this store is held while it runs. Nothing is performed twice — a job that
/// already carries an outcome is not collected.
pub async fn settle_job_container_runs<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard is taken and dropped inside this statement: nothing of this store is held
    // while the programs below are performed.
    let waiting = state.lock().jobs_awaiting_container_run();
    if waiting.is_empty() {
        return;
    }
    let mut outcomes: Vec<(String, ContainerRun)> = Vec::with_capacity(waiting.len());
    for job in waiting {
        if let Some(outcome) = run_container_program_off_worker(
            &job.program,
            &job.image,
            &job.environment,
            job.memory_limit_bytes,
        )
        .await
        {
            outcomes.push((job.job_id, outcome));
        }
    }
    state.lock().record_container_runs(outcomes);
}
