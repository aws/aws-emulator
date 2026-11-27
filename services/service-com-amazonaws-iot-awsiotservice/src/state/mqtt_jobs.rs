// SPDX-License-Identifier: CC0-1.0

//! JOBS over the reserved MQTT topics: how a device is handed the document of the job it has
//! to run, and how the status it reports back is recorded.
//!
//! A device does not poll an HTTP API for its work. It connects to the address
//! `DescribeEndpoint` published and runs the whole exchange on the reserved topics of its own
//! thing, `$aws/things/{thingName}/jobs/…`:
//!
//! * `get` — the executions the device still has to run, answered on `get/accepted` with the
//!   `inProgressJobs` and `queuedJobs` summaries (the set `GetPendingJobExecutions` reports).
//! * `{jobId}/get` — one execution, answered on `{jobId}/get/accepted` with that execution and
//!   the job DOCUMENT it has to run, carried as the document ITSELF
//!   ([`job_document_body`]). `$next` names the device's next pending execution, exactly
//!   as the `jobId` label of `DescribeJobExecution` does.
//! * `start-next` — the next pending execution, moved to `IN_PROGRESS` and answered on
//!   `start-next/accepted` with it and its document (the work
//!   `StartNextPendingJobExecution` hands out); a device that has nothing pending is answered
//!   with no execution at all rather than a refusal.
//! * `{jobId}/update` — the status the device has REACHED, answered on `{jobId}/update/accepted`
//!   and recorded on the execution itself, which is what makes the control plane's
//!   `DescribeJobExecution` report the device's own report.
//! * `notify-next` — what the SERVICE publishes about the device's next execution: it is
//!   published when an execution is enqueued for the device ([`notify_next`], called by the
//!   operation that creates one), so a device listening there learns of its work without
//!   asking.
//!
//! Every answer is the EXECUTION STORE's own record
//! ([`::service_common::iot_job_executions`]) — the very record the IoT Jobs data plane's
//! `DescribeJobExecution`, `StartNextPendingJobExecution` and `UpdateJobExecution` read and
//! write over HTTP, and the one the control plane's `DescribeJobExecution` reports — so a
//! device driving its execution over MQTT and a client reading it over HTTP cannot disagree,
//! and the rules a report is admitted by are that store's own
//! ([`JobExecutionRecord::transition_refusal`], [`JobExecutionRecord::try_replace_status_details`]).
//!
//! The answers go out through the broker's own fan-out
//! ([`::service_common::iot_mqtt_broker::publish_to_subscribers`]), on the `accepted` or
//! `rejected` topic of the request's own topic, which is where the device subscribed: a
//! request that cannot be carried out is answered with a refusal rather than left silent. The
//! CBOR half of each topic (`…/cbor`) is not served, exactly as the reserved provisioning
//! ([`super::mqtt_provisioning`]) and commands topics read a device's own message as JSON.
//!
//! LOCK ORDER: the execution store is taken on its own and released before the answer is
//! published, which takes the broker's store — no guard is held while another is taken.
//!
//! WHAT IS NOT PINNED BY A RECORDING: the topics, the members of each answer and the status a
//! report drives an execution to are AWS's; the `code` tokens a refusal carries are the ones
//! the Jobs device API publishes ([`Refusal::code`]) and the sentences they read with are the
//! execution store's own, so a device learns which part of its request was refused.

use ::serde_json::{json, Map, Value};
use ::service_common::iot_job_executions::{
    now_epoch_seconds, JobExecutionRecord, JobExecutionStore, NEXT_JOB_ID, STATUS_IN_PROGRESS,
    STATUS_QUEUED,
};
use ::service_common::iot_mqtt_broker::{
    broker_state, now_millis, publish_to_subscribers, DirectMessage,
};

/// The first two levels of the reserved namespace one thing's own topics hang off, and the
/// level that opens its jobs namespace: the topics are
/// `$aws/things/{thingName}/jobs/…`.
pub const THINGS_PREFIX: &str = "$aws/things";
/// See [`THINGS_PREFIX`].
pub const JOBS_LEVEL: &str = "jobs";

/// The level a device asks for its pending executions on.
pub const GET_LEVEL: &str = "get";
/// The level a device asks for its next execution to be STARTED on.
pub const START_NEXT_LEVEL: &str = "start-next";
/// The level a device reports a status on.
pub const UPDATE_LEVEL: &str = "update";
/// The level the service publishes a device's NEXT execution on.
pub const NOTIFY_NEXT_LEVEL: &str = "notify-next";

/// The level a request that was carried out is answered on.
pub const ACCEPTED_LEVEL: &str = "accepted";
/// The level a request that was refused is answered on.
pub const REJECTED_LEVEL: &str = "rejected";

/// The QoS the service's own publications on these topics are made at: the highest the broker
/// delivers at, so a device that asked for an acknowledged delivery of its work gets one.
const ANSWER_QOS: i32 = ::service_common::iot_mqtt_broker::wire::MAX_GRANTED_QOS;

/// The exchange one reserved jobs topic addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exchange {
    /// `$aws/things/{thingName}/jobs/get`: the device's pending executions.
    PendingExecutions,
    /// `$aws/things/{thingName}/jobs/start-next`: the next pending execution, started.
    StartNext,
    /// `$aws/things/{thingName}/jobs/{jobId}/get`: one execution and its document, where
    /// `$next` names the device's next pending one.
    Describe {
        /// The `jobId` level, as the request spelled it.
        job_id: String,
    },
    /// `$aws/things/{thingName}/jobs/{jobId}/update`: the status the device reached.
    Update {
        /// The `jobId` level, as the request spelled it.
        job_id: String,
    },
}

/// The thing and the exchange `topic` addresses, and `None` for every other topic — including
/// the `accepted`, `rejected` and `notify-next` topics of these very exchanges, which are the
/// service's own to write and no request of a device's.
#[must_use]
pub fn addressed(topic: &str) -> Option<(String, Exchange)> {
    let within = topic.strip_prefix(THINGS_PREFIX)?.strip_prefix('/')?;
    let mut levels = within.split('/');
    let thing_name = levels.next()?;
    if thing_name.is_empty() || levels.next() != Some(JOBS_LEVEL) {
        return None;
    }
    let exchange = match (levels.next()?, levels.next(), levels.next()) {
        (GET_LEVEL, None, _) => Exchange::PendingExecutions,
        (START_NEXT_LEVEL, None, _) => Exchange::StartNext,
        (job_id, Some(GET_LEVEL), None) if !job_id.is_empty() => Exchange::Describe {
            job_id: job_id.to_string(),
        },
        (job_id, Some(UPDATE_LEVEL), None) if !job_id.is_empty() => Exchange::Update {
            job_id: job_id.to_string(),
        },
        _ => return None,
    };
    Some((thing_name.to_string(), exchange))
}

/// The topic one answer to a request on `topic` is published on.
#[must_use]
pub fn answer_topic(topic: &str, accepted: bool) -> String {
    let level = if accepted {
        ACCEPTED_LEVEL
    } else {
        REJECTED_LEVEL
    };
    format!("{topic}/{level}")
}

/// The topic the service publishes a device's NEXT execution on.
#[must_use]
pub fn notify_next_topic(thing_name: &str) -> String {
    format!("{THINGS_PREFIX}/{thing_name}/{JOBS_LEVEL}/{NOTIFY_NEXT_LEVEL}")
}

/// Why a request on a reserved jobs topic was refused.
///
/// The `code` tokens are the ones the Jobs device API publishes for these topics, so a device
/// branching on them reads what it expects; the sentence beside each is the execution store's
/// own reason ([`JobExecutionRecord::transition_refusal`]) or this exchange's reading of the
/// request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The body is no request document, so nothing could be read out of it. It carries no
    /// sentence of its own: the message is this condition's fixed text
    /// ([`INVALID_JSON_MESSAGE`]) and nothing of the reading that failed.
    InvalidJson,
    /// The body is a document, and what it carries is not a request this exchange can serve.
    InvalidRequest(String),
    /// The device holds no execution the request's `jobId` names.
    ResourceNotFound(String),
    /// The status the device reported is one the execution's own state refuses, carrying the
    /// state it was refused against.
    InvalidStateTransition(String, JobExecutionState),
    /// The `expectedVersion` the request named is not the version the service holds, carrying
    /// the state it was decided against.
    VersionMismatch(String, JobExecutionState),
    /// This emulator holds no store to serve the request against.
    InternalError(String),
}

/// How this exchange states that a device's message was no request document at all.
///
/// FIXED text with no variable part: the condition is about the WHOLE body, so nothing of it —
/// and nothing of the reading that failed — is echoed back.
pub const INVALID_JSON_MESSAGE: &str = "Invalid JSON";

/// How this exchange states that an `update` carried no `status`, the one member it cannot be
/// served without.
///
/// FIXED text naming that member, the way this service states a missing member elsewhere.
pub const MISSING_STATUS_MESSAGE: &str = "Required field status is missing";

/// How this exchange states that `statusDetails` is no map of name to value at all.
pub const UNREADABLE_STATUS_DETAILS_MESSAGE: &str =
    "Field statusDetails must be a map of string to string";

/// How this exchange states that ONE pair of `statusDetails` carries a value that is no string:
/// the name the device sent is the sentence's only variable part, so the caller is told which
/// pair to fix.
fn unreadable_detail_message(name: &str) -> String {
    format!("Field statusDetails.{name} must be a string")
}

impl Refusal {
    /// The `code` the refusal is published with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson => "InvalidJson",
            Self::InvalidRequest(_) => "InvalidRequest",
            Self::ResourceNotFound(_) => "ResourceNotFound",
            Self::InvalidStateTransition(_, _) => "InvalidStateTransition",
            Self::VersionMismatch(_, _) => "VersionMismatch",
            Self::InternalError(_) => "InternalError",
        }
    }

    /// The body the refusal is published with: the code, the sentence, the client token the
    /// request carried and the instant — plus the execution's own state for the two refusals
    /// that were decided against a stored execution, so a device learns the state its report
    /// was refused against without a second call.
    #[must_use]
    fn body(&self, client_token: Option<&str>) -> Value {
        let (message, state) = match self {
            Self::InvalidJson => (INVALID_JSON_MESSAGE, None),
            Self::InvalidRequest(message)
            | Self::ResourceNotFound(message)
            | Self::InternalError(message) => (message.as_str(), None),
            Self::InvalidStateTransition(message, state)
            | Self::VersionMismatch(message, state) => (message.as_str(), Some(state)),
        };
        let mut body = Map::new();
        body.insert("code".to_string(), json!(self.code()));
        body.insert("message".to_string(), json!(message));
        if let Some(state) = state {
            body.insert("executionState".to_string(), state.body());
        }
        stamped(body, client_token)
    }
}

/// The STATE of one execution, as the member of that name carries it in an answer and in a
/// refusal: the status it is in, the details it carries and the version those were last
/// written at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobExecutionState {
    /// The execution's status.
    status: String,
    /// Its `statusDetails`, left out while it carries none.
    status_details: ::std::collections::BTreeMap<String, String>,
    /// The version the two above were last written at.
    version_number: i64,
}

impl JobExecutionState {
    /// The state of `record`.
    fn of(record: &JobExecutionRecord) -> Self {
        Self {
            status: record.status.clone(),
            status_details: record.status_details.clone(),
            version_number: record.version_number,
        }
    }

    /// The state as the `executionState` member carries it.
    fn body(&self) -> Value {
        let mut state = Map::new();
        state.insert("status".to_string(), json!(self.status));
        if !self.status_details.is_empty() {
            state.insert("statusDetails".to_string(), json!(self.status_details));
        }
        state.insert("versionNumber".to_string(), json!(self.version_number));
        Value::Object(state)
    }
}

/// What one request on these topics carries, read out of its JSON body.
///
/// Every member is optional — a device that sends an empty document asks for the exchange's
/// own defaults — and a body that is no document at all is a refusal
/// ([`Refusal::InvalidJson`]), because the answer's own `clientToken` could not be read
/// from it either.
#[derive(Debug, Clone, Default)]
struct Request {
    /// `clientToken`, echoed in the answer so a device can tell its own answer apart.
    client_token: Option<String>,
    /// `executionNumber`, which selects ONE execution of the thing.
    execution_number: Option<i64>,
    /// `includeJobDocument`: whether the answer carries the job document. The describing
    /// exchanges carry it unless the request says otherwise, which is what a device asking
    /// for its work needs.
    include_job_document: Option<bool>,
    /// `includeJobExecutionState`: whether an `update` answer carries the execution's state.
    include_execution_state: Option<bool>,
    /// `status`, the status an `update` reports.
    status: Option<String>,
    /// `statusDetails`, the name/value pairs an `update` records.
    status_details: Option<::std::collections::HashMap<String, String>>,
    /// `expectedVersion`, the optimistic-concurrency guard an `update` may carry.
    expected_version: Option<i64>,
    /// `stepTimeoutInMinutes`, the step timer an `update` arms.
    step_timeout_minutes: Option<i64>,
}

impl Request {
    /// The request `payload` carries, or why nothing could be read out of it.
    fn of(payload: &[u8]) -> Result<Self, Refusal> {
        // An EMPTY body is the shortest request these topics take — a device asking for its
        // next execution carries nothing to say — so it reads as the exchange's defaults.
        if payload.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        // A body that is no request DOCUMENT is refused as such, with nothing of the reading
        // itself in the sentence: a device is told its message was not a request, not how this
        // emulator's parser reads one.
        let document: Value =
            ::serde_json::from_slice(payload).map_err(|_| Refusal::InvalidJson)?;
        let Value::Object(members) = document else {
            return Err(Refusal::InvalidJson);
        };
        let text = |name: &str| members.get(name).and_then(Value::as_str).map(str::to_string);
        let flag = |name: &str| members.get(name).and_then(Value::as_bool);
        let number = |name: &str| members.get(name).and_then(Value::as_i64);
        // `statusDetails` is a map of NAME to VALUE, both strings
        // (`JobExecution$statusDetails`), so a value of any other kind is a request this
        // exchange cannot record — refused by naming the pair, rather than recorded as the
        // empty string the device never sent.
        let details = match members.get("statusDetails") {
            None => None,
            Some(Value::Object(pairs)) => {
                let mut read = ::std::collections::HashMap::with_capacity(pairs.len());
                for (name, value) in pairs {
                    let Some(value) = value.as_str() else {
                        return Err(Refusal::InvalidRequest(unreadable_detail_message(name)));
                    };
                    read.insert(name.clone(), value.to_string());
                }
                Some(read)
            }
            Some(_) => {
                return Err(Refusal::InvalidRequest(
                    UNREADABLE_STATUS_DETAILS_MESSAGE.to_string(),
                ))
            }
        };
        Ok(Self {
            client_token: text("clientToken"),
            execution_number: number("executionNumber"),
            include_job_document: flag("includeJobDocument"),
            include_execution_state: flag("includeJobExecutionState"),
            status: text("status"),
            status_details: details,
            expected_version: number("expectedVersion"),
            step_timeout_minutes: number("stepTimeoutInMinutes"),
        })
    }

    /// Whether the answer to a DESCRIBING exchange carries the job document: these topics are
    /// how a device learns the work it has to run, so the document travels unless the request
    /// asked for it to be left out.
    fn wants_job_document(&self) -> bool {
        self.include_job_document.unwrap_or(true)
    }
}

/// One execution as the `execution` member of an answer carries it.
///
/// Every value is the stored record's own; `jobDocument` travels only when the answer carries
/// one, and the two optional instants and the details are left out while the record holds
/// none rather than sent empty.
fn execution_body(record: &JobExecutionRecord, with_document: bool, now: i64) -> Value {
    let mut execution = Map::new();
    execution.insert("jobId".to_string(), json!(record.job_id));
    execution.insert("thingName".to_string(), json!(record.thing_name));
    execution.insert("status".to_string(), json!(record.status));
    if !record.status_details.is_empty() {
        execution.insert("statusDetails".to_string(), json!(record.status_details));
    }
    execution.insert("queuedAt".to_string(), json!(record.queued_at));
    if let Some(started_at) = record.started_at {
        execution.insert("startedAt".to_string(), json!(started_at));
    }
    execution.insert("lastUpdatedAt".to_string(), json!(record.last_updated_at));
    execution.insert("versionNumber".to_string(), json!(record.version_number));
    execution.insert(
        "executionNumber".to_string(),
        json!(record.execution_number),
    );
    if let Some(seconds) = record.approximate_seconds_before_timed_out(now) {
        execution.insert(
            "approximateSecondsBeforeTimedOut".to_string(),
            json!(seconds),
        );
    }
    if with_document {
        if let Some(document) = &record.job_document {
            execution.insert("jobDocument".to_string(), job_document_body(document));
        }
    }
    Value::Object(execution)
}

/// The JOB DOCUMENT as these topics carry it: the document ITSELF, so a device reads its
/// members straight out of the message it was handed.
///
/// A job document is "The job document" a job was created with — a JSON document — and the
/// reserved MQTT topics carry a message that is itself JSON, so the document is embedded as
/// the value it is rather than as a string holding its text (which is how the HTTP jobs plane,
/// whose `jobDocument` member is modelled a string, has to carry it). A stored document that
/// is no JSON value at all is carried as the text it is, because that is all there is of it.
fn job_document_body(document: &str) -> Value {
    ::serde_json::from_str(document).unwrap_or_else(|_| json!(document))
}

/// One execution as a `JobExecutionSummary` of the pending lists carries it: what the device
/// needs to ask about the execution, and none of the work itself.
fn summary_body(record: &JobExecutionRecord) -> Value {
    let mut summary = Map::new();
    summary.insert("jobId".to_string(), json!(record.job_id));
    summary.insert("queuedAt".to_string(), json!(record.queued_at));
    if let Some(started_at) = record.started_at {
        summary.insert("startedAt".to_string(), json!(started_at));
    }
    summary.insert("lastUpdatedAt".to_string(), json!(record.last_updated_at));
    summary.insert("versionNumber".to_string(), json!(record.version_number));
    summary.insert(
        "executionNumber".to_string(),
        json!(record.execution_number),
    );
    Value::Object(summary)
}

/// One answer body, stamped the way every message on these topics is: the instant it was
/// published, in epoch SECONDS (the unit every instant of a job execution is reported in), and
/// the `clientToken` the request carried, so a device can tell its own answer apart.
fn stamped(mut body: Map<String, Value>, client_token: Option<&str>) -> Value {
    if let Some(token) = client_token {
        body.insert("clientToken".to_string(), json!(token));
    }
    body.insert("timestamp".to_string(), json!(now_epoch_seconds()));
    Value::Object(body)
}

/// Serve one publication a device made on a reserved jobs topic, answering it on that topic's
/// own `accepted` or `rejected` topic.
///
/// `false` for a topic this exchange does not serve, which the caller then treats as the
/// ordinary publication it is. A request on one it DOES serve is always answered, including one
/// this emulator could not carry out because it vends no store: a device waiting on its work is
/// told, never left silent.
pub fn served_jobs_request<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    topic: &str,
    payload: &[u8],
) -> bool {
    let Some((thing_name, exchange)) = addressed(topic) else {
        return false;
    };
    let answered = match Request::of(payload) {
        Err(refusal) => Err((refusal, None)),
        Ok(request) => {
            let token = request.client_token.clone();
            match super::job_execution_store(engine) {
                None => Err((
                    Refusal::InternalError(NO_STORE_MESSAGE.to_string()),
                    token.clone(),
                )),
                Some(state) => {
                    let mut store = state.lock();
                    let served = served(&mut store, &thing_name, &exchange, &request);
                    served.map_err(|refusal| (refusal, token.clone()))
                }
            }
        }
    };
    let (accepted, body) = match answered {
        Ok(body) => (true, body),
        Err((refusal, token)) => (false, refusal.body(token.as_deref())),
    };
    answer(engine, &answer_topic(topic, accepted), &body);
    true
}

/// What a request this emulator holds no execution store for is refused with.
pub const NO_STORE_MESSAGE: &str = "The IoT emulator has no job execution store attached.";

/// Carry out one exchange against the execution store, answering the body the device is
/// handed.
fn served(
    store: &mut JobExecutionStore,
    thing_name: &str,
    exchange: &Exchange,
    request: &Request,
) -> Result<Value, Refusal> {
    let now = now_epoch_seconds();
    // A timer the job or the device armed moves an execution to `TIMED_OUT` by itself, so the
    // transitions the CLOCK has already made due are recorded before anything is read: each
    // was owed at its own deadline whether or not this device published, so what the device is
    // handed is never work its deadline has already taken away.
    store.time_out_due_executions(thing_name, now);
    match exchange {
        Exchange::PendingExecutions => {
            let mut body = Map::new();
            body.insert(
                "inProgressJobs".to_string(),
                Value::Array(
                    store
                        .with_status(thing_name, STATUS_IN_PROGRESS)
                        .iter()
                        .map(|record| summary_body(record))
                        .collect(),
                ),
            );
            body.insert(
                "queuedJobs".to_string(),
                Value::Array(
                    store
                        .with_status(thing_name, STATUS_QUEUED)
                        .iter()
                        .map(|record| summary_body(record))
                        .collect(),
                ),
            );
            Ok(stamped(body, request.client_token.as_deref()))
        }
        Exchange::Describe { job_id } => {
            let record = store
                .resolve_record(thing_name, job_id, request.execution_number)
                .ok_or_else(|| Refusal::ResourceNotFound(not_found_message(job_id, thing_name)))?;
            let mut body = Map::new();
            body.insert(
                "execution".to_string(),
                execution_body(record, request.wants_job_document(), now),
            );
            Ok(stamped(body, request.client_token.as_deref()))
        }
        Exchange::StartNext => {
            let mut body = Map::new();
            // "If the next job execution does not exist" the device is told so by an answer
            // carrying no execution: a device with nothing to do is not a device whose
            // request was wrong.
            if let Some(index) = store.next_pending_index(thing_name, request.execution_number) {
                let record = store
                    .record_mut(thing_name, index)
                    .ok_or_else(|| Refusal::InternalError(NO_STORE_MESSAGE.to_string()))?;
                // Starting IS the write this exchange makes: the execution the device is
                // handed is `IN_PROGRESS` from here on, which is what
                // `StartNextPendingJobExecution` does over HTTP, and the step timer the
                // request asked for is armed with it.
                record.status = STATUS_IN_PROGRESS.to_string();
                record.arm_step_timer(request.step_timeout_minutes, now);
                record.start(now);
                record.last_updated_at = now;
                record.version_number += 1;
                body.insert(
                    "execution".to_string(),
                    execution_body(record, request.wants_job_document(), now),
                );
            }
            Ok(stamped(body, request.client_token.as_deref()))
        }
        Exchange::Update { job_id } => {
            let Some(status) = request.status.clone() else {
                return Err(Refusal::InvalidRequest(
                    MISSING_STATUS_MESSAGE.to_string(),
                ));
            };
            let index = store
                .resolve_index(thing_name, job_id, request.execution_number)
                .ok_or_else(|| Refusal::ResourceNotFound(not_found_message(job_id, thing_name)))?;
            let record = store
                .record_mut(thing_name, index)
                .ok_or_else(|| Refusal::InternalError(NO_STORE_MESSAGE.to_string()))?;
            // Everything that can refuse the report is decided BEFORE the record is touched,
            // so a refused report leaves the execution — and its version — exactly as it was:
            // the concurrency guard first, then the move the execution's own state admits,
            // then the details it can hold. Each is the SHARED store's own rule, so a report
            // this exchange admits is one the HTTP update admits too.
            if let Some(expected) = request.expected_version {
                if expected != record.version_number {
                    return Err(Refusal::VersionMismatch(
                        version_mismatch_message(expected, record.version_number),
                        JobExecutionState::of(record),
                    ));
                }
            }
            if let Some(reason) = record.transition_refusal(&status) {
                return Err(Refusal::InvalidStateTransition(
                    reason,
                    JobExecutionState::of(record),
                ));
            }
            if let Err(reason) = record.try_replace_status_details(request.status_details.as_ref()) {
                return Err(Refusal::InvalidRequest(reason));
            }
            record.status = status;
            record.arm_step_timer(request.step_timeout_minutes, now);
            if record.status == STATUS_IN_PROGRESS {
                // The device reporting IN_PROGRESS is a start: it records `startedAt` and arms
                // the job's in-progress timer on the first one.
                record.start(now);
            }
            record.last_updated_at = now;
            // "Job execution versions are incremented each time they are updated by a device"
            // (`JobExecution$versionNumber`).
            record.version_number += 1;
            let mut body = Map::new();
            // Both members are OPT-IN, as they are on the HTTP update: a device that asked for
            // neither is answered that its report was taken and nothing more.
            if request.include_execution_state.unwrap_or(false) {
                body.insert(
                    "executionState".to_string(),
                    JobExecutionState::of(record).body(),
                );
            }
            if request.include_job_document.unwrap_or(false) {
                if let Some(document) = &record.job_document {
                    body.insert("jobDocument".to_string(), job_document_body(document));
                }
            }
            Ok(stamped(body, request.client_token.as_deref()))
        }
    }
}

/// How this exchange states that the device holds no execution the request's `jobId` names.
///
/// The two things that identify one are its variable parts — the id the request asked about
/// and the device it asked about it on — and `$next` names whichever execution is next, so a
/// device with nothing pending is told about the label it used.
fn not_found_message(job_id: &str, thing_name: &str) -> String {
    if job_id == NEXT_JOB_ID {
        return format!("No pending job execution found for thing {thing_name}");
    }
    format!("Job execution for job {job_id} on thing {thing_name} cannot be found")
}

/// How this exchange states that the `expectedVersion` the device named is not the version the
/// service holds: the optimistic-concurrency guard failing, reported with both versions so the
/// device can resume from the one that stands.
fn version_mismatch_message(expected: i64, stored: i64) -> String {
    format!("Expected version {expected} but found version {stored}")
}

/// Publish what the service has to say about the device's NEXT execution: the execution it is
/// to run, with its document, on the thing's own `notify-next` topic.
///
/// Called by the operation that ENQUEUES an execution for the device, because that is the
/// moment the device's next work changes — which is what a device listening on this topic is
/// waiting for. Nothing is published for a device that has nothing pending, and nothing at all
/// for an engine that vends no store: there is no work to announce.
///
/// A READ of the execution store: the record is projected and left exactly as it stands,
/// because announcing work is not handing it out.
pub fn notify_next<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    thing_name: &str,
) {
    let Some(state) = super::job_execution_store(engine) else {
        return;
    };
    let now = now_epoch_seconds();
    let body = {
        let store = state.lock();
        let index = store.next_pending_index(thing_name, None);
        let Some(record) = index.and_then(|index| store.record(thing_name, index)) else {
            return;
        };
        let mut body = Map::new();
        body.insert(
            "execution".to_string(),
            execution_body(record, true, now),
        );
        stamped(body, None)
    };
    answer(engine, &notify_next_topic(thing_name), &body);
}

/// Publish one message of this exchange on `topic`: it goes to every session subscribed to a
/// filter covering it, which is where the device is listening.
fn answer<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    topic: &str,
    body: &Value,
) {
    let Some(broker) = broker_state(engine) else {
        // Only an engine with no broker store at all, which is one no device can be
        // listening on.
        ::tracing::debug!(topic, "no broker store answered the reserved jobs topic");
        return;
    };
    publish_to_subscribers(
        &broker,
        &DirectMessage {
            topic: topic.to_string(),
            payload: ::serde_json::to_vec(body).unwrap_or_default(),
            qos: ANSWER_QOS,
            delivered_at_millis: now_millis(),
            ..DirectMessage::default()
        },
    );
}
