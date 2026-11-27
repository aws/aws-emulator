// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakertrainingsessionruntime-legorouterservice service.
//!
//! The service is the DATA PLANE of a SageMaker training session: a caller submits
//! asynchronous training and sampling operations against a session the control plane
//! created, then polls for each one's result. This module holds everything the six
//! handlers share, so no two operations grow their own copy of it:
//!
//! * the **session records** (`sessions`), this data plane's own view of a session the
//!   SageMaker control plane created, materialised on the first call against a name that
//!   directory holds and refused for one it does not ([`session_admission`]), because this
//!   runtime service models no create operation of its own — and refused as well, with the
//!   modelled `ConflictException`, for a session whose lifecycle state cannot accept
//!   requests ([`session_accepts_requests`]);
//! * each session's **training queue** (`sequence_index` + `next_sequence_number`),
//!   which is what makes operations execute in sequence-number order;
//! * each session's **checkpoints**, written by `saveWeights` / `saveState` and read by
//!   `loadState`, `loadWeightsForSampling` and both inference operations, which resolve
//!   the one they run against through [`resolve_sample_checkpoint`];
//! * the **clock-owed transition** that moves a submitted operation out of its
//!   non-terminal status (see [`EXECUTION_SPAN`] and [`Session::advance`]);
//! * the **validation rules the model states in prose** rather than in a trait, which
//!   the boundary validator therefore does not refuse for us.
//!
//! No language model or training engine runs inside this emulator, so nothing here
//! invents model output and presents it as a forward pass, an optimizer step or a
//! generated sample. What follows from that is a line between the operations this
//! emulator can SERVE and the ones it cannot:
//!
//! * an operation whose result this service computes settles in the status that outcome
//!   is — a `sample` operation completes and reports a `SampleOperationResult$sample`
//!   whose tokens say, in the vocabulary a real byte-pair encoder ships, that no model
//!   ran (see [`sampled_tokens`]), exactly as the synchronous `SampleTrainingSession`
//!   says it in its own body ([`sample_response_body`]) and the streaming variant in the
//!   one chunk it frames ([`sample_response_stream_body`]); a `computeLogprobs` operation
//!   completes and reports one score per prompt token, each the log-probability the
//!   maximum-entropy distribution over the served vocabulary gives it, which is the score
//!   a model that has learned nothing has and claims no forward pass (see
//!   [`prompt_logprobs`]);
//! * an operation whose whole result is a number only a model's forward pass has — a
//!   loss or a gradient — ends in the service's own `Failed` status with `errorDetails`
//!   that say so ([`no_model_runtime`]), which is the modelled failure channel reporting
//!   what is unsupported rather than a healthy request reported as broken;
//! * an operation the base model could not process at all — an input longer than its
//!   context window — ends in that same `Failed` status with the reason the service states
//!   for it ([`context_length_exceeded`]).
//!
//! Everything around that — session and operation records, sequence ordering,
//! idempotency, checkpoints and their ARNs, token accounting, status progression,
//! validation and the 404s — is real and computed from persisted state.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{Document, Number};

use crate::ops::types::{
    ComputeLogprobsResult, LoadStateResult, LoadWeightsForSamplingResult, ModelInput,
    OperationConfig, OperationResult, SampleOperationConfig, SampleOperationResult, SaveStateResult,
    SaveWeightsResult,
};

/// The token sequence a completed `sample` operation serves, and the tokenizer that
/// encodes it. Its own module because it is the one concern here that is about CONTENT
/// rather than about the records around it.
mod sampled_tokens;
pub use sampled_tokens::*;

/// The scores a completed `computeLogprobs` operation serves — the other half of that
/// content concern, kept beside it and apart from the records.
mod prompt_logprobs;
pub use prompt_logprobs::*;

/// What the base model a session adapts can hold, which is what decides whether an
/// operation's input could be processed at all.
mod base_model_context;
pub use base_model_context::*;

/// The region a checkpoint ARN names when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after it starts executing an operation's non-terminal status settles.
///
/// AWS documents no duration for a training or sampling operation, so this span is
/// CHOSEN: half a second, inside this project's 250–750 ms window for a transition the
/// clock owes. It is never zero, so a caller that polls straight after submitting still
/// observes `InProgress`. The due instant is stored on the operation record
/// ([`TrainingOp::due_at`] / [`SamplingOp::due_at`]) and applied the next time any
/// handler runs; nothing sleeps.
pub const EXECUTION_SPAN: Duration = Duration::from_millis(500);

/// How many session records this emulator retains.
///
/// Sessions are materialised on first use, so without a bound a caller naming a fresh
/// session each call would grow the store without end. The oldest session is released
/// once the store passes this; it is far above any client working set, so eviction never
/// hides a session a caller is still polling.
pub const MAX_SESSIONS: usize = 256;

/// How many operations of one kind a single session retains. A session that has reached
/// it cannot accept another submit (`ConflictException`), which is a refusal rather than
/// silent eviction: an operation id a caller still holds must keep resolving.
pub const MAX_OPERATIONS_PER_SESSION: usize = 1_024;

/// The `RequestStatus` values, from the model's own enum.
pub const STATUS_PENDING: &str = "Pending";
/// See [`STATUS_PENDING`].
pub const STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`STATUS_PENDING`].
pub const STATUS_COMPLETED: &str = "Completed";
/// See [`STATUS_PENDING`].
pub const STATUS_FAILED: &str = "Failed";

/// Whether `status` is terminal: "Poll this operation until the status reaches a terminal
/// state (Completed, Failed, or Skipped)" — `GetTrainingOperationResult`.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, STATUS_COMPLETED | STATUS_FAILED | "Skipped")
}

/// The id prefix of a training operation, and the namespace its ids are minted in.
pub const TRAINING_NAMESPACE: &str = "training";
/// The id prefix of a sampling operation. Sampling ids live in their own namespace, so a
/// training operation id never resolves on the sampling route or the other way round.
pub const SAMPLING_NAMESPACE: &str = "sampling";

/// The content type `SampleTrainingSession` answers with, and the only one it can serve.
pub const SAMPLE_RESPONSE_CONTENT_TYPE: &str = "application/json";

/// The content type `SampleTrainingSessionWithResponseStream` answers with: the
/// `@mediaType` the model puts on `SampleResponseStream`, whose events
/// [`sample_response_stream_body`] frames.
pub const SAMPLE_RESPONSE_STREAM_CONTENT_TYPE: &str = "text/event-stream";

/// How deep a caller-supplied `Document` is walked when it is rendered for a request
/// fingerprint, keeping the recursion bound LOCAL to this crate rather than inherited
/// from a deserializer.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// The keys `lossFnConfig` accepts, with the range each one's value must fall in.
///
/// From `ForwardBackwardConfig$lossFnConfig`: "Supported keys are `clip_low_threshold`
/// (valid range: 0 to 1) and `clip_high_threshold` (valid range: 1 to 10). Unsupported
/// keys are rejected." The same document member is declared on `ForwardConfig`.
pub const LOSS_FN_CONFIG_KEYS: &[(&str, f64, f64)] =
    &[("clip_low_threshold", 0.0, 1.0), ("clip_high_threshold", 1.0, 10.0)];

/// Why an operation whose whole result is a number only a model's forward pass has ends
/// in `Failed`.
///
/// `kind` is the union member that was submitted. No language model or training engine
/// runs inside this emulator, so there is nothing honest to report as `Completed` for a
/// loss or a gradient: the operation ends in the service's own modelled failure status
/// with a reason that says why. An operation this emulator CAN serve never reaches this —
/// see [`sampled_tokens`] for the sequence a `sample` operation completes with and
/// [`prompt_logprobs`] for the scores a `computeLogprobs` operation completes with.
pub fn no_model_runtime(kind: &str) -> String {
    format!(
        "This SageMaker training-session runtime emulator runs no language model or \
         training engine, so the {kind} operation was not executed and produced no result."
    )
}

/// Why an operation that names a checkpoint this session does not hold ends in `Failed`.
pub fn checkpoint_not_in_session(session: &str, arn: &str) -> String {
    format!(
        "Training session {session} holds no checkpoint {arn}; a checkpoint must be \
         written by a saveWeights or saveState operation on this session before it can \
         be loaded."
    )
}

/// One checkpoint of a training session, as this emulator persists it.
///
/// Written when a `saveWeights` or `saveState` operation completes, and read by
/// `loadState`, `loadWeightsForSampling` and `SampleTrainingSession`.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    /// The model package ARN the save operation reported, in the form
    /// `SaveStateResult$modelPackageArn` documents.
    pub model_package_arn: String,
    /// The model package version in that ARN: 1 for a session's first checkpoint.
    pub version: i32,
    pub created_by_operation_id: String,
    pub created_at: SystemTime,
    /// Whether this checkpoint carries optimizer state as well as weights. A
    /// `saveState` checkpoint does ("saving full training state, including model weights
    /// and optimizer state"); a `saveWeights` one does not.
    pub holds_optimizer_state: bool,
    /// `mergeWeights` as submitted: whether LoRA adapter weights were merged into the
    /// base model before saving.
    pub merged_weights: bool,
    pub description: Option<String>,
}

/// The checkpoint slot a save operation reserved when it was submitted.
///
/// The ARN is minted at SUBMIT time, from the region that request was signed for, so the
/// ARN a later poll reports does not depend on who polled; the operation publishes the
/// slot to [`Session::checkpoints`] when it completes, so a checkpoint only becomes
/// loadable once the save that writes it has finished.
#[derive(Debug, Clone)]
pub struct ReservedCheckpoint {
    pub model_package_arn: String,
    pub version: i32,
}

/// One submitted training operation.
#[derive(Debug, Clone)]
pub struct TrainingOp {
    pub operation_id: String,
    pub sequence_number: i32,
    pub client_token: Option<String>,
    /// Digest of the submitted parameters ([`training_request_fingerprint`]), compared
    /// when a `clientToken` is reused so an idempotent retry can be told from a reuse of
    /// the same token with different parameters.
    pub request_fingerprint: String,
    /// The configuration exactly as submitted. It is what decides which member of
    /// `OperationResult` a poll populates, and the record of what the caller asked for.
    pub config: OperationConfig,
    /// One of the `RequestStatus` values.
    pub status: &'static str,
    pub result: Option<OperationResult>,
    pub error_details: Option<String>,
    pub submitted_at: SystemTime,
    /// When this operation's transition out of `InProgress` falls due, set when it
    /// reaches the head of the session's sequence queue and starts executing.
    pub due_at: Option<SystemTime>,
    /// The checkpoint a `saveWeights` / `saveState` operation will publish.
    pub reserved: Option<ReservedCheckpoint>,
}

/// One submitted sampling operation. Sampling operations carry no sequence number, so
/// each one executes on its own rather than in a queue.
#[derive(Debug, Clone)]
pub struct SamplingOp {
    pub operation_id: String,
    pub config: SampleOperationConfig,
    /// One of the `RequestStatus` values.
    pub status: &'static str,
    pub result: Option<SampleOperationResult>,
    pub error_details: Option<String>,
    pub submitted_at: SystemTime,
    /// See [`TrainingOp::due_at`].
    pub due_at: Option<SystemTime>,
}

/// One training session's runtime state.
#[derive(Debug)]
pub struct Session {
    pub name: String,
    pub created_at: SystemTime,
    /// The sequence number of the next operation to execute. "The training session
    /// processes operations strictly in sequence number order. If a gap exists, the
    /// session blocks until the missing operation arrives" — so this only advances past
    /// a slot once the operation occupying it is terminal.
    pub next_sequence_number: i32,
    pub training_ops: BTreeMap<String, TrainingOp>,
    /// `sequenceNumber` -> `operationId`: the execution queue.
    pub sequence_index: BTreeMap<i32, String>,
    pub sampling_ops: BTreeMap<String, SamplingOp>,
    /// `clientToken` -> `operationId`, so a retry with the same token is ignored and
    /// answers the operation id the first call minted.
    pub client_tokens: HashMap<String, String>,
    /// Every checkpoint written on this session, keyed by its model package ARN.
    pub checkpoints: BTreeMap<String, Checkpoint>,
    /// The model package version the next reserved checkpoint carries.
    pub next_checkpoint_version: i32,
    /// The counter operation ids are minted from, so two submits never collide.
    pub next_operation_seq: u64,
    /// The instant the training queue last freed: the due instant of the operation that
    /// executed before the current head, so each operation's own due instant is chained
    /// from its predecessor's rather than from whenever somebody polled.
    pub queue_free_at: Option<SystemTime>,
    /// The checkpoint `loadState` last restored training state from.
    pub loaded_checkpoint_arn: Option<String>,
    /// The checkpoint `loadWeightsForSampling` last loaded for inference.
    pub sampling_checkpoint_arn: Option<String>,
    /// The sequence number of the last training operation this session RECEIVED, and `0`
    /// for a session that has received none: sequence numbers start at 1, so zero is the
    /// cursor of a session no training operation has reached.
    ///
    /// Distinct from [`Self::next_sequence_number`], which is the slot the queue will
    /// EXECUTE next: this one records receipt, so it moves on every accepted submit
    /// whether or not that operation has executed. It is what the control plane reports as
    /// `TrainingSessionDetail$LastReceivedSequenceNumber` ([`Session::accounted_usage`]).
    pub last_received_sequence_number: i32,
    /// How many tokens the base model this session adapts can attend over, and `None` for
    /// a base model this emulator publishes no window for
    /// ([`base_model_context_window`]).
    ///
    /// Read from the shared directory on every accepted submit, because the base model is
    /// the CONTROL plane's record of the session and this plane holds no create: it is the
    /// ceiling an operation's input is judged against when it executes, which is why it
    /// lives on the session rather than being looked up again by whoever polls.
    pub context_window: Option<i32>,
}

impl Session {
    /// A session materialised on first use of its name.
    pub fn new(name: String, now: SystemTime) -> Self {
        Self {
            name,
            created_at: now,
            next_sequence_number: 1,
            training_ops: BTreeMap::new(),
            sequence_index: BTreeMap::new(),
            sampling_ops: BTreeMap::new(),
            client_tokens: HashMap::new(),
            checkpoints: BTreeMap::new(),
            next_checkpoint_version: 1,
            next_operation_seq: 0,
            queue_free_at: None,
            loaded_checkpoint_arn: None,
            sampling_checkpoint_arn: None,
            last_received_sequence_number: 0,
            context_window: None,
        }
    }

    /// Record that this session RECEIVED a training operation carrying `sequence_number`,
    /// which is what the control plane reports as the last sequence number the session
    /// received. Called where a submit is ACCEPTED, so a refused one is not accounted.
    pub fn receive_training_operation(&mut self, sequence_number: i32) {
        self.last_received_sequence_number = sequence_number;
    }

    /// What this session has ACCOUNTED so far: the sequence number it last received, and
    /// the tokens its operations have consumed.
    ///
    /// COUNTED from this session's own records rather than estimated: the token totals are
    /// the sums of the usage the operations themselves report, so an operation that
    /// consumed no token adds none. A sampling operation this emulator serves reports the
    /// tokens it really prefilled and decoded; an operation that ended in the modelled
    /// failure status executed nothing and so reports no usage at all, and the sums follow
    /// whatever the operations do report.
    ///
    /// This is the value the data plane publishes into the account the SageMaker control
    /// plane reads ([`publish_session_usage`]), which reports both as members of its own
    /// `TrainingSessionDetail`.
    #[must_use]
    pub fn accounted_usage(&self) -> ::service_common::training_session_usage::SessionUsage {
        let mut usage = ::service_common::training_session_usage::SessionUsage {
            last_received_sequence_number: self.last_received_sequence_number,
            ..::service_common::training_session_usage::SessionUsage::default()
        };
        for op in self.training_ops.values() {
            // "The token usage statistics for metering purposes": the training operations
            // that put data through the model are the ones that report it.
            let reported = match op.result.as_ref() {
                Some(OperationResult::Forward(result)) => result.token_usage.as_ref(),
                Some(OperationResult::forwardBackward(result)) => result.token_usage.as_ref(),
                _ => None,
            };
            if let Some(training_tokens) = reported.and_then(|reported| reported.training_tokens) {
                usage.training_tokens = usage
                    .training_tokens
                    .saturating_add(i64::from(training_tokens));
            }
        }
        for op in self.sampling_ops.values() {
            // A sampling operation reports its usage per SAMPLED SEQUENCE, split into the
            // tokens the prompt was prefilled with and the tokens decoded for it.
            let Some(SampleOperationResult::Sample(response)) = op.result.as_ref() else {
                continue;
            };
            for sequence in &response.sequences {
                let Some(reported) = sequence.token_usage.as_ref() else {
                    continue;
                };
                usage.prefill_tokens = usage
                    .prefill_tokens
                    .saturating_add(i64::from(reported.prefill));
                usage.sample_tokens = usage
                    .sample_tokens
                    .saturating_add(i64::from(reported.decode));
            }
        }
        usage
    }

    /// Mint an operation id in `namespace`, within `OperationId`'s 1..64 length and
    /// `^[a-zA-Z0-9-]+$` pattern.
    ///
    /// Derived from this session's counter plus the submitting instant, so two submits
    /// never share an id and the id is not a guessable sequence; the loop skips an id the
    /// session already holds.
    pub fn mint_operation_id(&mut self, namespace: &str, now: SystemTime) -> String {
        use sha2::{Digest, Sha256};
        let millis = now
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or(0);
        loop {
            self.next_operation_seq = self.next_operation_seq.wrapping_add(1);
            let seed = format!(
                "{}:{namespace}:{}:{millis}",
                self.name, self.next_operation_seq
            );
            let digest = Sha256::digest(seed.as_bytes());
            let candidate = format!("{namespace}-{}", &hex::encode(digest)[..24]);
            let taken = self.training_ops.contains_key(&candidate)
                || self.sampling_ops.contains_key(&candidate);
            if !taken {
                return candidate;
            }
        }
    }

    /// Reserve the checkpoint a save operation will publish when it completes.
    ///
    /// The ARN has the form `SaveStateResult$modelPackageArn` documents:
    /// `arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION`.
    pub fn reserve_checkpoint(&mut self, region: &str) -> ReservedCheckpoint {
        let version = self.next_checkpoint_version;
        self.next_checkpoint_version = self.next_checkpoint_version.saturating_add(1);
        ReservedCheckpoint {
            model_package_arn: checkpoint_arn(region, &self.name, version),
            version,
        }
    }

    /// The checkpoint a sampling call runs against when the caller names none: the one
    /// last loaded for sampling, else the one training state was last loaded from, else
    /// the newest checkpoint this session wrote.
    pub fn current_checkpoint(&self) -> Option<&Checkpoint> {
        self.sampling_checkpoint_arn
            .as_deref()
            .or(self.loaded_checkpoint_arn.as_deref())
            .and_then(|arn| self.checkpoints.get(arn))
            .or_else(|| {
                self.checkpoints
                    .values()
                    .max_by_key(|checkpoint| checkpoint.version)
            })
    }

    /// Whether every operation this session holds has reached a terminal status, so
    /// releasing the session loses nothing a caller is still waiting on.
    pub fn is_settled(&self) -> bool {
        self.training_ops
            .values()
            .all(|op| is_terminal_status(op.status))
            && self
                .sampling_ops
                .values()
                .all(|op| is_terminal_status(op.status))
    }

    /// Whether this session's training queue is waiting on an operation nobody has
    /// submitted: the slot it would execute next holds nothing, while later slots do.
    ///
    /// Such a session executes nothing until a caller supplies the missing sequence
    /// number, so no instant the clock reaches settles it — which is what makes it the
    /// session to release when the store is full and nothing has settled.
    pub fn is_blocked_on_a_missing_operation(&self) -> bool {
        !self.sequence_index.is_empty()
            && !self.sequence_index.contains_key(&self.next_sequence_number)
    }

    /// Materialise every status transition the clock has already made due at `now`.
    ///
    /// A reporting operation calls this before it answers: each change was owed at the
    /// stored due instant whether or not anybody called, so applying it here is not a
    /// read with a side effect.
    pub fn advance(&mut self, now: SystemTime) {
        self.advance_training(now);
        self.advance_sampling(now);
    }

    /// Drive the training queue: operations execute strictly in sequence-number order,
    /// so the queue stops at the first slot that is empty (the session blocks on the gap)
    /// or still executing.
    fn advance_training(&mut self, now: SystemTime) {
        loop {
            let Some(op_id) = self.sequence_index.get(&self.next_sequence_number).cloned() else {
                return;
            };
            let Some(mut op) = self.training_ops.remove(&op_id) else {
                // The record behind the slot is gone, so nothing can execute it: release
                // the slot rather than block the session on it for ever.
                self.sequence_index.remove(&self.next_sequence_number);
                let Some(next) = self.next_sequence_number.checked_add(1) else {
                    return;
                };
                self.next_sequence_number = next;
                continue;
            };
            let terminal = self.start_or_settle_training(&mut op, now);
            self.training_ops.insert(op_id, op);
            if !terminal {
                return;
            }
            // The cursor must MOVE for the next turn of the loop to read another slot, so
            // a cursor that cannot be incremented ends the drain instead of re-reading
            // the slot it just settled.
            let Some(next) = self.next_sequence_number.checked_add(1) else {
                return;
            };
            self.next_sequence_number = next;
        }
    }

    /// Start the queue's head operation if it has not started, settle it if the clock has
    /// reached its due instant, and answer whether it is now terminal.
    ///
    /// An operation starts at the instant the slot ahead of it freed — not at the instant
    /// of whichever call happens to drive the queue — so the whole queue drains on
    /// instants that were owed whether or not anybody called.
    fn start_or_settle_training(&mut self, op: &mut TrainingOp, now: SystemTime) -> bool {
        if op.status == STATUS_PENDING {
            let started_at = self.queue_free_at.unwrap_or(op.submitted_at).max(op.submitted_at);
            if let Some(kind) = training_needs_model_runtime(&op.config) {
                // Nothing executes it, so it never enters `InProgress` and claims a
                // duration for work that did not run; the queue frees at the instant it
                // reached the head.
                op.status = STATUS_FAILED;
                op.error_details = Some(no_model_runtime(kind));
                self.queue_free_at = Some(started_at);
                return true;
            }
            op.status = STATUS_IN_PROGRESS;
            op.due_at = Some(started_at + EXECUTION_SPAN);
        }
        if op.status != STATUS_IN_PROGRESS {
            return true;
        }
        if op.due_at.is_none_or(|due| now < due) {
            return false;
        }
        self.queue_free_at = op.due_at;
        self.complete_training(op);
        true
    }

    /// Apply a due training operation's effect to the session and record its result.
    fn complete_training(&mut self, op: &mut TrainingOp) {
        let created_at = op.due_at.unwrap_or(op.submitted_at);
        match &op.config {
            OperationConfig::saveWeights(config) => {
                let description = config.description.clone();
                let merged = config.merge_weights.unwrap_or(false);
                self.publish_checkpoint(op, created_at, false, merged, description);
            }
            OperationConfig::saveState(config) => {
                let description = config.description.clone();
                self.publish_checkpoint(op, created_at, true, false, description);
            }
            OperationConfig::loadState(config) => {
                let arn = config.model_package_arn.clone();
                let load_optimizer = config.load_optimizer;
                match self.checkpoints.get(&arn) {
                    None => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                    }
                    Some(checkpoint) if load_optimizer && !checkpoint.holds_optimizer_state => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(format!(
                            "Checkpoint {arn} was written by a saveWeights operation and \
                             carries no optimizer state, so loadOptimizer cannot be \
                             satisfied from it."
                        ));
                    }
                    Some(_) => {
                        self.loaded_checkpoint_arn = Some(arn);
                        op.status = STATUS_COMPLETED;
                        op.result = Some(OperationResult::loadState(LoadStateResult {}));
                    }
                }
            }
            // Every remaining member needs a model runtime, and was refused before it
            // reached `InProgress`.
            other => {
                op.status = STATUS_FAILED;
                op.error_details = Some(no_model_runtime(training_config_kind(other)));
            }
        }
    }

    /// Publish a save operation's reserved checkpoint and record its result.
    fn publish_checkpoint(
        &mut self,
        op: &mut TrainingOp,
        created_at: SystemTime,
        holds_optimizer_state: bool,
        merged_weights: bool,
        description: Option<String>,
    ) {
        let Some(reserved) = op.reserved.clone() else {
            // A save operation reserves its checkpoint at submit time, so this is
            // unreachable; report it as a failure of the operation rather than inventing
            // an ARN that names nothing.
            op.status = STATUS_FAILED;
            op.error_details = Some(
                "No checkpoint slot was reserved for this save operation, so no model \
                 package was written."
                    .to_string(),
            );
            return;
        };
        self.checkpoints.insert(
            reserved.model_package_arn.clone(),
            Checkpoint {
                model_package_arn: reserved.model_package_arn.clone(),
                version: reserved.version,
                created_by_operation_id: op.operation_id.clone(),
                created_at,
                holds_optimizer_state,
                merged_weights,
                description,
            },
        );
        op.status = STATUS_COMPLETED;
        op.result = Some(if holds_optimizer_state {
            OperationResult::saveState(SaveStateResult {
                model_package_arn: reserved.model_package_arn,
            })
        } else {
            OperationResult::saveWeights(SaveWeightsResult {
                model_package_arn: Some(reserved.model_package_arn),
            })
        });
    }

    /// Drive the sampling operations. They carry no sequence number, so each settles on
    /// its own due instant.
    fn advance_sampling(&mut self, now: SystemTime) {
        let pending: Vec<String> = self
            .sampling_ops
            .iter()
            .filter(|(_, op)| op.status == STATUS_PENDING || op.status == STATUS_IN_PROGRESS)
            .map(|(id, _)| id.clone())
            .collect();
        for id in pending {
            let Some(mut op) = self.sampling_ops.remove(&id) else {
                continue;
            };
            if op.status == STATUS_PENDING {
                op.status = STATUS_IN_PROGRESS;
                // Sampling operations are not queued, so each one's due instant is
                // chained from its own submit rather than from whenever this call
                // happened to be made.
                op.due_at = Some(op.submitted_at + EXECUTION_SPAN);
            }
            if op.status == STATUS_IN_PROGRESS && op.due_at.is_some_and(|due| now >= due) {
                self.complete_sampling(&mut op);
            }
            self.sampling_ops.insert(id, op);
        }
    }

    /// The model package a sampling configuration names that this session does not hold,
    /// and `None` when it names one the session holds or names none at all.
    ///
    /// "The ARN of the model package whose checkpoint this operation should run against.
    /// ... When absent, the operation runs against the session's latest checkpoint." A
    /// session that has written no checkpoint still has the base model its control-plane
    /// record was created with, so only a NAMED model package that is not one of this
    /// session's checkpoints fails the operation.
    fn unheld_checkpoint(&self, model_package_arn: Option<&str>) -> Option<String> {
        model_package_arn
            .filter(|arn| !self.checkpoints.contains_key(*arn))
            .map(str::to_string)
    }

    /// Apply a due sampling operation's effect to the session and record its result.
    fn complete_sampling(&mut self, op: &mut SamplingOp) {
        // An input the base model cannot hold is not something the model could process, so
        // the operation is not executed at all and reports the reason the service states
        // for that condition. Judged before anything else about the request, because it is
        // the input itself that does not fit.
        if let (Some(window), Some(needed)) = (self.context_window, context_tokens(&op.config)) {
            if needed > window {
                op.status = STATUS_FAILED;
                op.error_details = Some(context_length_exceeded());
                return;
            }
        }
        match &op.config {
            SampleOperationConfig::loadWeightsForSampling(config) => {
                let arn = config.model_package_arn.clone();
                if self.checkpoints.contains_key(&arn) {
                    self.sampling_checkpoint_arn = Some(arn);
                    op.status = STATUS_COMPLETED;
                    op.result = Some(SampleOperationResult::loadWeightsForSampling(
                        LoadWeightsForSamplingResult {},
                    ));
                } else {
                    op.status = STATUS_FAILED;
                    op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                }
            }
            SampleOperationConfig::Sample(config) => {
                match self.unheld_checkpoint(config.model_package_arn.as_deref()) {
                    Some(arn) => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                    }
                    None => match sample_response(config) {
                        Some(response) => {
                            op.status = STATUS_COMPLETED;
                            op.result = Some(SampleOperationResult::Sample(response));
                        }
                        None => {
                            op.status = STATUS_FAILED;
                            op.error_details = Some(no_sampling_vocabulary());
                        }
                    },
                }
            }
            // "Computing log-probabilities of prompt tokens without generating new text":
            // one score per prompt token, from the one place those scores are decided.
            SampleOperationConfig::computeLogprobs(config) => {
                match self.unheld_checkpoint(config.model_package_arn.as_deref()) {
                    Some(arn) => {
                        op.status = STATUS_FAILED;
                        op.error_details = Some(checkpoint_not_in_session(&self.name, &arn));
                    }
                    None => {
                        op.status = STATUS_COMPLETED;
                        op.result = Some(SampleOperationResult::computeLogprobs(
                            ComputeLogprobsResult {
                                logprobs: prompt_logprobs(&config.prompt),
                            },
                        ));
                    }
                }
            }
        }
    }
}

/// Per-service state for com-amazonaws-sagemakertrainingsessionruntime-legorouterservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState {
    /// Every training session this emulator holds, keyed by `trainingSessionName`.
    /// Bounded by [`MAX_SESSIONS`].
    pub sessions: BTreeMap<String, Session>,
}

impl ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState {
    /// The session `name` names, with every transition the clock already made due
    /// applied — for a REPORTING operation, so a name this emulator has never seen
    /// answers `None` rather than being created.
    pub fn advance_session(&mut self, name: &str, now: SystemTime) -> Option<&mut Session> {
        let session = self.sessions.get_mut(name)?;
        session.advance(now);
        Some(session)
    }

    /// The session `name` names, for a SUBMIT, materialising this service's own record of
    /// a session the CONTROL PLANE already created.
    ///
    /// This runtime service models no create operation — a session exists only because
    /// `CreateTrainingSession` minted it — so a caller must resolve the name against the
    /// directory both services read ([`session_admission`]) BEFORE asking for the record
    /// here: a name no session was created for is answered
    /// `ResourceNotFoundException` ([`session_not_found`]), never by bringing a record
    /// into existence. What this materialises is the data plane's own view of a session
    /// that does exist: its queue, its operations and its checkpoints, which only traffic
    /// against it fills.
    ///
    /// Answers `None` when the store holds [`MAX_SESSIONS`] and none of them can be
    /// released: a session is released only once nothing in it is still owed, so an
    /// operation a caller is still polling never vanishes into a 404 that cannot be told
    /// from "never existed".
    ///
    /// The oldest SETTLED session goes first, and each session is brought up to the
    /// instant the clock has already reached before that is decided — an operation the
    /// clock finished settles the session that holds it whether or not anybody polled it,
    /// so eviction does not depend on who read what. Where nothing has settled, the
    /// oldest session whose queue is waiting on an operation nobody submitted goes
    /// instead: it executes nothing until a caller supplies that sequence number, so
    /// keeping it would refuse every new session for as long as the caller never does.
    /// Every session that survives both passes has an operation the clock is going to
    /// settle, so the refusal below is one a retry gets past.
    pub fn session_for_submit(&mut self, name: &str, now: SystemTime) -> Option<&mut Session> {
        if !self.sessions.contains_key(name) {
            while self.sessions.len() >= MAX_SESSIONS {
                for session in self.sessions.values_mut() {
                    session.advance(now);
                }
                let releasable = self
                    .sessions
                    .iter()
                    .filter(|(_, session)| session.is_settled())
                    .min_by_key(|(key, session)| (session.created_at, (*key).clone()))
                    .map(|(key, _)| key.clone())
                    .or_else(|| {
                        self.sessions
                            .iter()
                            .filter(|(_, session)| session.is_blocked_on_a_missing_operation())
                            .min_by_key(|(key, session)| (session.created_at, (*key).clone()))
                            .map(|(key, _)| key.clone())
                    });
                // Every session still has an operation the clock has yet to settle; the
                // submit is refused rather than one of them being thrown away.
                let key = releasable?;
                self.sessions.remove(&key);
            }
        }
        let session = self
            .sessions
            .entry(name.to_string())
            .or_insert_with(|| Session::new(name.to_string(), now));
        session.advance(now);
        Some(session)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState>)
}

/// The SHARED training-session account in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// The SageMaker control plane creates and REPORTS a training session, and two members of
/// the detail it reports — the sequence number the session last received and the tokens
/// billed for its traffic — are accounted quantities of the traffic THIS service serves
/// and nothing else observes. So this service publishes what its own records add up to
/// into one account under one shared type
/// ([`::service_common::training_session_usage::TrainingSessionUsageDirectory`]); the
/// engine keys state by TYPE, so a copy of its own inside this service's state would be
/// invisible to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this account second, and no
/// reader holds this one while taking another store, so the order cannot invert.
pub fn training_session_usage<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::training_session_usage::TrainingSessionUsageDirectory>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::training_session_usage::TrainingSessionUsageDirectory,
            >,
        )
}

/// Publish what the session `name` has accounted ([`Session::accounted_usage`]) into the
/// shared account the SageMaker control plane reads.
///
/// Called where this service ACCEPTS or PERFORMS work for a session, which is where those
/// totals change. Publishing is an aid to another service and never part of this
/// operation's own answer, so an engine holding no account leaves it unwritten rather than
/// failing the call.
pub fn publish_session_usage<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    usage: ::service_common::training_session_usage::SessionUsage,
) {
    if let Some(account) = training_session_usage(engine) {
        account.lock().publish(name, usage);
    }
}

/// The SHARED directory of the training sessions the SageMaker control plane created: call
/// it with a handler's `&*self.engine`.
///
/// This service is the DATA PLANE of a training session and models no create of its own:
/// a request carries a `trainingSessionName` and nothing else, so the only way to tell a
/// session of this account from a name that was never created is the record the control
/// plane wrote when `CreateTrainingSession` minted it
/// ([`::service_common::training_sessions::TrainingSessionDirectory`]). The engine keys
/// state by TYPE, so a copy of its own inside this service's state would be invisible to
/// that writer.
///
/// LOCK ORDER: a caller takes this directory BEFORE [`service_state`] and holds neither
/// while taking the other, so the order cannot invert.
pub fn training_session_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::training_sessions::TrainingSessionDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::training_sessions::TrainingSessionDirectory,
            >,
        )
}

/// The `TrainingSessionStatus` values a training session accepts new operations in: "The
/// session must be in `Running` or `Blocked` state to accept requests" —
/// `ConflictException`, and the doc of every submit operation of this service.
pub const SESSION_STATUS_RUNNING: &str = "Running";
/// See [`SESSION_STATUS_RUNNING`].
pub const SESSION_STATUS_BLOCKED: &str = "Blocked";

/// Whether a session whose lifecycle state is `status` can accept a call that submits
/// work. Every other state of the session's lifecycle — still provisioning, stopping,
/// stopped, failed, deleted — cannot.
pub fn session_accepts_requests(status: &str) -> bool {
    matches!(status, SESSION_STATUS_RUNNING | SESSION_STATUS_BLOCKED)
}

/// How a call against a session whose lifecycle state cannot accept it is worded: the
/// fixed text around the session's CURRENT `TrainingSessionStatus`, which is the one fact
/// about the refusal that varies.
pub fn session_cannot_accept_requests(status: &str) -> String {
    format!("Session is in {status} state and cannot accept requests")
}

/// What the shared directory says about the session a data-plane call names.
///
/// A session exists only because the CONTROL plane created it, and it accepts work only in
/// some of its lifecycle states, so these are the two things every call against a session
/// resolves before it serves anything — together with the base model's context window,
/// which is the same record's answer to what that session's model can hold.
#[derive(Debug, Clone)]
pub enum SessionAdmission {
    /// No `CreateTrainingSession` minted a session of that name, so it does not exist.
    NotCreated,
    /// The control plane holds it, in this `TrainingSessionStatus`, adapting a base model
    /// whose context window is `context_window` tokens
    /// ([`base_model_context_window`]).
    Created {
        status: String,
        context_window: Option<i32>,
    },
}

impl SessionAdmission {
    /// The refusal this session's lifecycle owes a call that submits work, and `None` when
    /// it accepts one: a name the control plane does not hold is the modelled
    /// `ResourceNotFoundException`, and a session in a state that cannot accept requests
    /// the modelled `ConflictException`. The caller maps each onto its own operation's
    /// error channel.
    pub fn refusal(&self, training_session_name: &str, region: &str) -> Option<SampleRefusal> {
        match self {
            Self::NotCreated => Some(SampleRefusal::NotFound(session_not_found(
                training_session_name,
                region,
            ))),
            Self::Created { status, .. } if !session_accepts_requests(status) => Some(
                SampleRefusal::Conflict(session_cannot_accept_requests(status)),
            ),
            Self::Created { .. } => None,
        }
    }

    /// The context window of the session's base model, and `None` for a session the
    /// control plane does not hold or one whose base model publishes no window.
    #[must_use]
    pub fn context_window(&self) -> Option<i32> {
        match self {
            Self::NotCreated => None,
            Self::Created { context_window, .. } => *context_window,
        }
    }
}

/// What the SageMaker control plane says about the training session `name` at `now`, which
/// is what every data-plane call against a session has to resolve before it serves
/// anything.
///
/// Read from the directory both services share, so a name no `CreateTrainingSession`
/// minted answers [`SessionAdmission::NotCreated`] rather than being served against a
/// record this service invented, and a session the control plane has moved out of a state
/// that accepts requests is refused here too. A status the control plane reaches on a
/// clock-owed instant is published with that instant, so the status read here is the one
/// the control plane would report at `now` — and reading it writes nothing.
pub fn session_admission<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    now: SystemTime,
) -> SessionAdmission {
    let Some(directory) = training_session_directory(engine) else {
        return SessionAdmission::NotCreated;
    };
    let directory = directory.lock();
    match directory.get(name) {
        None => SessionAdmission::NotCreated,
        Some(record) => SessionAdmission::Created {
            status: record.lifecycle.status_at(now).to_string(),
            context_window: base_model_context_window(&record.base_model_arn),
        },
    }
}

/// The `ResourceNotFoundException` message `GetTrainingOperationResult` carries for an
/// operation id that does not resolve: `Operation not found: <operationId>`.
///
/// The operation is what the read resolves, so the operation is what the refusal names —
/// the id the request's URI carried, echoed verbatim — whether the session holding it is
/// unknown too. Deliberately different text from [`sample_request_not_found`], so a
/// caller polling the two sibling routes can tell which lookup missed.
pub fn operation_not_found(operation_id: &str) -> String {
    format!("Operation not found: {operation_id}")
}

/// The `ResourceNotFoundException` message `GetSamplingOperationResult` carries for an
/// operation id that does not resolve: `Sample request not found: <operationId>`.
///
/// See [`operation_not_found`]: the sampling route words the same condition its own way.
pub fn sample_request_not_found(operation_id: &str) -> String {
    format!("Sample request not found: {operation_id}")
}

/// The region a request is addressed to: the third field of its SigV4 credential scope,
/// falling back to [`DEFAULT_REGION`] for a request that carries none.
///
/// The value is interpolated into an ARN, so it is honoured only when it has an AWS
/// region's shape: lowercase alphanumeric segments joined by `-`.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:sagemaker:<region>:<account>:model-package/<session>/<version>` — the form
/// `SaveStateResult$modelPackageArn` and `LoadStateConfig$modelPackageArn` document.
pub fn checkpoint_arn(region: &str, session: &str, version: i32) -> String {
    ::engine::emulator_arn(
        "sagemaker",
        region,
        &format!("model-package/{session}/{version}"),
    )
}

/// The `OperationConfig` member that was submitted, by its wire name.
pub fn training_config_kind(config: &OperationConfig) -> &'static str {
    match config {
        OperationConfig::Forward(_) => "forward",
        OperationConfig::forwardBackward(_) => "forwardBackward",
        OperationConfig::optimizerStep(_) => "optimizerStep",
        OperationConfig::saveWeights(_) => "saveWeights",
        OperationConfig::saveState(_) => "saveState",
        OperationConfig::loadState(_) => "loadState",
    }
}

/// The `SampleOperationConfig` member that was submitted, by its wire name.
pub fn sampling_config_kind(config: &SampleOperationConfig) -> &'static str {
    match config {
        SampleOperationConfig::Sample(_) => "sample",
        SampleOperationConfig::computeLogprobs(_) => "computeLogprobs",
        SampleOperationConfig::loadWeightsForSampling(_) => "loadWeightsForSampling",
    }
}

/// The member name when executing `config` would need a model, tokenizer or training
/// engine — none of which runs in this emulator — and `None` when it would not.
///
/// `saveWeights`, `saveState` and `loadState` move a checkpoint around, which this
/// emulator does for real; the other three members put data through the model.
pub fn training_needs_model_runtime(config: &OperationConfig) -> Option<&'static str> {
    match config {
        OperationConfig::Forward(_)
        | OperationConfig::forwardBackward(_)
        | OperationConfig::optimizerStep(_) => Some(training_config_kind(config)),
        OperationConfig::saveWeights(_)
        | OperationConfig::saveState(_)
        | OperationConfig::loadState(_) => None,
    }
}

/// How many tokens of the base model's context window executing `config` would need: the
/// prompt it submitted plus whatever it asks the model to generate, and `None` for a
/// configuration that puts no input through the model at all.
///
/// `SampleConfig` prefills its prompt and then generates up to `samplingParams.maxTokens`
/// more, so both ends of it occupy the window; `ComputeLogprobsConfig` scores the prompt
/// "without generating new text", so only the prompt does; `loadWeightsForSamplingConfig`
/// carries no input. Saturating, so an enormous request is judged over the window rather
/// than wrapping below it.
pub fn context_tokens(config: &SampleOperationConfig) -> Option<i32> {
    match config {
        SampleOperationConfig::Sample(inner) => Some(
            prompt_token_count(&inner.prompt).saturating_add(inner.sampling_params.max_tokens),
        ),
        SampleOperationConfig::computeLogprobs(inner) => Some(prompt_token_count(&inner.prompt)),
        SampleOperationConfig::loadWeightsForSampling(_) => None,
    }
}

/// Refuse an `OperationConfig` the boundary validator accepted but the model's prose
/// rules out, answering the reason to put in a `ValidationException`.
pub fn validate_operation_config(config: &OperationConfig) -> Result<(), String> {
    let kind = training_config_kind(config);
    match config {
        OperationConfig::Forward(inner) => {
            validate_datum_list(kind, &inner.datum_list)?;
            validate_loss_fn_config(kind, inner.loss_fn_config.as_ref())
        }
        OperationConfig::forwardBackward(inner) => {
            validate_datum_list(kind, &inner.datum_list)?;
            validate_loss_fn_config(kind, inner.loss_fn_config.as_ref())
        }
        OperationConfig::optimizerStep(_) => Ok(()),
        OperationConfig::saveWeights(_) | OperationConfig::saveState(_) => Ok(()),
        OperationConfig::loadState(inner) => validate_model_package_arn(
            &format!("operationConfig.{kind}.modelPackageArn"),
            &inner.model_package_arn,
        ),
    }
}

/// The `SampleOperationConfig` counterpart of [`validate_operation_config`].
pub fn validate_sample_operation_config(config: &SampleOperationConfig) -> Result<(), String> {
    let kind = sampling_config_kind(config);
    match config {
        SampleOperationConfig::Sample(inner) => {
            validate_model_input(&format!("operationConfig.{kind}.prompt"), &inner.prompt)?;
            // "The number of top-k log-probabilities to return per prompt token",
            // `@default(0)`: a count, so a negative one names no response at all.
            if inner.topk_prompt_logprobs < 0 {
                return Err(format!(
                    "operationConfig.{kind}.topkPromptLogprobs must be at least 0, got {}",
                    inner.topk_prompt_logprobs
                ));
            }
            if let Some(arn) = inner.model_package_arn.as_deref() {
                validate_model_package_arn(&format!("operationConfig.{kind}.modelPackageArn"), arn)?;
            }
            Ok(())
        }
        SampleOperationConfig::computeLogprobs(inner) => {
            validate_model_input(&format!("operationConfig.{kind}.prompt"), &inner.prompt)?;
            if let Some(arn) = inner.model_package_arn.as_deref() {
                validate_model_package_arn(&format!("operationConfig.{kind}.modelPackageArn"), arn)?;
            }
            Ok(())
        }
        SampleOperationConfig::loadWeightsForSampling(inner) => validate_model_package_arn(
            &format!("operationConfig.{kind}.modelPackageArn"),
            &inner.model_package_arn,
        ),
    }
}

/// "Must contain at least one datum, and each datum must have a non-empty modelInput
/// with at least one chunk" — `ForwardBackwardConfig$datumList`.
fn validate_datum_list(kind: &str, datum_list: &[crate::ops::types::Datum]) -> Result<(), String> {
    if datum_list.is_empty() {
        return Err(format!(
            "operationConfig.{kind}.datumList must contain at least one datum"
        ));
    }
    for (index, datum) in datum_list.iter().enumerate() {
        validate_model_input(
            &format!("operationConfig.{kind}.datumList[{index}].modelInput"),
            &datum.model_input,
        )?;
    }
    Ok(())
}

/// A `ModelInput` is "the ordered list of encoded text chunks forming the input
/// sequence", and each chunk carries "the token IDs representing the encoded text": an
/// input with no chunks, or a chunk with no tokens, encodes nothing.
fn validate_model_input(member: &str, input: &ModelInput) -> Result<(), String> {
    if input.chunks.is_empty() {
        return Err(format!("{member}.chunks must contain at least one chunk"));
    }
    for (index, chunk) in input.chunks.iter().enumerate() {
        if chunk.tokens.is_empty() {
            return Err(format!(
                "{member}.chunks[{index}].tokens must contain at least one token id"
            ));
        }
    }
    Ok(())
}

/// Refuse a `lossFnConfig` document that carries a key the engine does not support, or a
/// supported key outside its range. See [`LOSS_FN_CONFIG_KEYS`] for the source.
fn validate_loss_fn_config(kind: &str, config: Option<&Document>) -> Result<(), String> {
    let Some(config) = config else {
        return Ok(());
    };
    let member = format!("operationConfig.{kind}.lossFnConfig");
    let Document::Object(entries) = config else {
        return Err(format!("{member} must be an object"));
    };
    for (key, value) in entries {
        let Some((_, min, max)) = LOSS_FN_CONFIG_KEYS
            .iter()
            .find(|(supported, _, _)| supported == key)
        else {
            let supported: Vec<&str> = LOSS_FN_CONFIG_KEYS.iter().map(|(key, _, _)| *key).collect();
            return Err(format!(
                "{member} does not support the key {key:?}; supported keys are {}",
                supported.join(", ")
            ));
        };
        let Some(number) = document_number(value) else {
            return Err(format!("{member}.{key} must be a number"));
        };
        if number < *min || number > *max {
            return Err(format!(
                "{member}.{key} must be between {min} and {max}, got {number}"
            ));
        }
    }
    Ok(())
}

/// The numeric value of a document node, or `None` when it is not a number.
fn document_number(value: &Document) -> Option<f64> {
    match value {
        Document::Number(Number::PosInt(value)) => Some(*value as f64),
        Document::Number(Number::NegInt(value)) => Some(*value as f64),
        Document::Number(Number::Float(value)) => Some(*value),
        _ => None,
    }
}

/// Refuse anything that is not a SageMaker model package ARN.
///
/// "Must be a valid SageMaker model package ARN in the format
/// `arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION`. Model package
/// group ARNs are not accepted." — `LoadStateConfig$modelPackageArn`.
pub fn validate_model_package_arn(member: &str, arn: &str) -> Result<(), String> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let malformed = |reason: &str| {
        Err(format!(
            "{member} must be a SageMaker model package ARN of the form \
             arn:aws:sagemaker:REGION:ACCOUNT:model-package/PACKAGE_NAME/VERSION ({reason}), \
             got {arn:?}"
        ))
    };
    if fields.len() != 6 || fields[0] != "arn" {
        return malformed("six colon-separated fields beginning with arn");
    }
    if !matches!(fields[1], "aws" | "aws-cn" | "aws-us-gov" | "aws-iso" | "aws-iso-b") {
        return malformed("an AWS partition");
    }
    if fields[2] != "sagemaker" {
        return malformed("the sagemaker service namespace");
    }
    if fields[3].is_empty() {
        return malformed("a region");
    }
    if fields[4].len() != 12 || !fields[4].chars().all(|c| c.is_ascii_digit()) {
        return malformed("a 12-digit account id");
    }
    let Some(resource) = fields[5].strip_prefix("model-package/") else {
        return malformed("a model-package resource, not a model package group");
    };
    let mut segments = resource.split('/');
    let name = segments.next().unwrap_or_default();
    let version = segments.next().unwrap_or_default();
    if segments.next().is_some() {
        return malformed("exactly one PACKAGE_NAME and one VERSION");
    }
    if name.is_empty() {
        return malformed("a package name");
    }
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return malformed("a numeric package version");
    }
    Ok(())
}

/// The body `SampleTrainingSession` answers with.
///
/// No model runs in this emulator, so this says so plainly rather than returning
/// synthesised inference output or the request's own body echoed back as if generated.
/// Besides the explanation it carries only facts about what was actually served: the
/// session, the checkpoint the request resolved to and how many bytes of request payload
/// arrived.
pub fn sample_response_body(session: &str, model_package_arn: &str, request_bytes: usize) -> Vec<u8> {
    let body = serde_json::json!({
        "message": "This SageMaker training-session runtime emulator runs no model, so no \
                    inference output was generated for this request.",
        "trainingSessionName": session,
        "modelPackageArn": model_package_arn,
        "requestBodyBytes": request_bytes,
    });
    serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec())
}

/// The Server-Sent Events body `SampleTrainingSessionWithResponseStream` answers with.
///
/// "The streaming response body, delivered as Server-Sent Events (`text/event-stream`).
/// Each event is a `data: {JSON}` line containing a chat completion chunk, and the stream
/// is terminated by `data: [DONE]`." — `SampleResponseStream`. No model runs in this
/// emulator, so the one chunk carries exactly what the synchronous call answers
/// ([`sample_response_body`]) rather than invented tokens, and the stream is framed and
/// terminated as the model documents.
pub fn sample_response_stream_body(
    session: &str,
    model_package_arn: &str,
    request_bytes: usize,
) -> Vec<u8> {
    let chunk = sample_response_body(session, model_package_arn, request_bytes);
    let mut body = Vec::with_capacity(chunk.len() + SSE_FRAMING_OVERHEAD);
    body.extend_from_slice(b"data: ");
    body.extend_from_slice(&chunk);
    body.extend_from_slice(b"\n\n");
    body.extend_from_slice(b"data: [DONE]\n\n");
    body
}

/// How many bytes of Server-Sent Events framing [`sample_response_stream_body`] adds
/// around its one chunk: the `data: ` prefixes and the blank-line separators.
const SSE_FRAMING_OVERHEAD: usize = 24;

/// The identity a training session is resolved by: its name, the account that owns it and
/// the region it lives in, joined by `-`.
///
/// A session name is scoped to one account and one region, so the name alone does not name
/// a session; this is the whole identity a lookup failed on, and it is what a refusal
/// reports.
pub fn session_identity(training_session_name: &str, region: &str) -> String {
    format!(
        "{training_session_name}-{}-{region}",
        ::engine::emulator_account_id()
    )
}

/// How a data-plane call reports a training session it cannot resolve: the fixed text
/// `Session not found: ` followed by the session identity it looked for
/// ([`session_identity`]).
pub fn session_not_found(training_session_name: &str, region: &str) -> String {
    format!(
        "Session not found: {}",
        session_identity(training_session_name, region)
    )
}

/// Why an inference call cannot be served, in the three classes this service's inference
/// operations model for it.
#[derive(Debug, Clone)]
pub enum SampleRefusal {
    /// A request the model's prose rules out: a 400 `ValidationException`.
    Validation(String),
    /// A session, or a checkpoint of it, this emulator does not hold: a 404
    /// `ResourceNotFoundException`.
    NotFound(String),
    /// A session that holds no model to sample: a 409 `ConflictException`.
    Conflict(String),
}

/// The checkpoint an inference call runs against, resolved from persisted session state.
///
/// Both inference operations take the same input members and model the same errors, so
/// they resolve their target through this one function: a session the CONTROL plane never
/// created does not exist at all and one it has moved out of a state that accepts requests
/// cannot be sampled (`admission`, read from the directory both services share through
/// [`session_admission`] — this service models no create, so that record is the only thing
/// that makes a session exist and the only place its lifecycle state is known), a session
/// that does exist but has no runtime record yet holds no checkpoint to sample, a named
/// model package must be one of the session's own checkpoints, and a caller that names none
/// gets the checkpoint the session currently resolves to ([`Session::current_checkpoint`]).
/// An inference call never materialises a session record: only a submit does that.
pub fn resolve_sample_checkpoint(
    store: &mut ComAmazonawsSagemakertrainingsessionruntimeLegorouterserviceState,
    training_session_name: &str,
    region: &str,
    model_package_arn: Option<&str>,
    now: SystemTime,
    admission: &SessionAdmission,
) -> Result<String, SampleRefusal> {
    if let Some(arn) = model_package_arn {
        if let Err(reason) = validate_model_package_arn("modelPackageArn", arn) {
            return Err(SampleRefusal::Validation(reason));
        }
    }

    if let Some(refusal) = admission.refusal(training_session_name, region) {
        return Err(refusal);
    }

    let Some(session) = store.advance_session(training_session_name, now) else {
        // The control plane holds the session, so what is missing is a checkpoint: no
        // traffic has written one, which is the conflict below rather than a not-found.
        return Err(SampleRefusal::Conflict(no_checkpoint_to_sample(
            training_session_name,
        )));
    };

    match model_package_arn {
        Some(arn) if !session.checkpoints.contains_key(arn) => {
            Err(SampleRefusal::NotFound(format!(
                "Model package {arn} is not a checkpoint of training session \
                 {training_session_name}."
            )))
        }
        Some(arn) => Ok(arn.to_string()),
        None => match session.current_checkpoint() {
            Some(checkpoint) => Ok(checkpoint.model_package_arn.clone()),
            None => Err(SampleRefusal::Conflict(no_checkpoint_to_sample(
                training_session_name,
            ))),
        },
    }
}

/// Why an inference call against a session that holds no checkpoint conflicts: there is no
/// model to serve the request from until one has been written.
///
/// Stated once because both paths to that condition answer it — the session with no
/// runtime record at all, and the one whose record holds no checkpoint.
fn no_checkpoint_to_sample(training_session_name: &str) -> String {
    format!(
        "Training session {training_session_name} has no model checkpoint to sample from: \
         write one with a saveWeights or saveState operation, or name a model package on \
         the X-Amzn-SageMaker-Model-Package header."
    )
}

/// The fingerprint of an `ExecuteTrainingOperation` request's parameters: a SHA-256
/// digest of a canonical rendering of them.
///
/// `clientToken` makes a retry of "the same client token and the same parameters" a
/// no-op, so the handler needs to know whether a retry's parameters are in fact the
/// same. A fixed-size digest rather than the rendering itself, so the record keeps no
/// second copy of the caller's data; map keys are rendered in sorted order, so two
/// renderings of one request always agree.
pub fn training_request_fingerprint(sequence_number: i32, config: &OperationConfig) -> String {
    use sha2::{Digest, Sha256};
    let mut canonical = format!("sequenceNumber={sequence_number};");
    write_operation_config(&mut canonical, config);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn write_operation_config(out: &mut String, config: &OperationConfig) {
    out.push_str(training_config_kind(config));
    out.push('{');
    match config {
        OperationConfig::Forward(inner) => write_pass(
            out,
            &inner.loss_fn_name,
            &inner.datum_list,
            inner.loss_fn_config.as_ref(),
        ),
        OperationConfig::forwardBackward(inner) => write_pass(
            out,
            &inner.loss_fn_name,
            &inner.datum_list,
            inner.loss_fn_config.as_ref(),
        ),
        OperationConfig::optimizerStep(inner) => {
            let adam = &inner.adam_params_config;
            out.push_str(&format!(
                "learningRate={};beta1={};beta2={};eps={};weightDecay={};gradClipNorm={};\
                 loraPlusLrRatio={:?}",
                adam.learning_rate,
                adam.beta1,
                adam.beta2,
                adam.eps,
                adam.weight_decay,
                adam.grad_clip_norm,
                adam.lora_plus_lr_ratio
            ));
        }
        OperationConfig::saveWeights(inner) => out.push_str(&format!(
            "mergeWeights={:?};description={:?}",
            inner.merge_weights, inner.description
        )),
        OperationConfig::saveState(inner) => {
            out.push_str(&format!("description={:?}", inner.description));
        }
        OperationConfig::loadState(inner) => out.push_str(&format!(
            "modelPackageArn={};loadOptimizer={}",
            inner.model_package_arn, inner.load_optimizer
        )),
    }
    out.push('}');
}

/// `ForwardConfig` and `ForwardBackwardConfig` carry the same three members, so one
/// renderer serves both.
fn write_pass(
    out: &mut String,
    loss_fn_name: &str,
    datum_list: &[crate::ops::types::Datum],
    loss_fn_config: Option<&Document>,
) {
    out.push_str("lossFnName=");
    out.push_str(loss_fn_name);
    out.push_str(";datumList=[");
    for datum in datum_list {
        write_model_input_canonical(out, &datum.model_input);
        out.push_str(";lossFnInputs=");
        match &datum.loss_fn_inputs {
            None => out.push_str("absent"),
            Some(inputs) => {
                let mut names: Vec<&String> = inputs.keys().collect();
                names.sort();
                out.push('{');
                for name in names {
                    out.push_str(name);
                    out.push('=');
                    if let Some(tensor) = inputs.get(name) {
                        out.push_str(&format!("{}/{:?}/", tensor.dtype, tensor.shape));
                        write_document_canonical(out, &tensor.data);
                    }
                    out.push(';');
                }
                out.push('}');
            }
        }
        out.push(',');
    }
    out.push_str("];lossFnConfig=");
    match loss_fn_config {
        None => out.push_str("absent"),
        Some(document) => write_document_canonical(out, document),
    }
}

fn write_model_input_canonical(out: &mut String, input: &ModelInput) {
    out.push_str("chunks=[");
    for chunk in &input.chunks {
        out.push_str(&format!("{:?}/{};", chunk.tokens, chunk.r#type));
    }
    out.push(']');
}

/// A document rendered with its object keys in sorted order, so the rendering does not
/// depend on the hash-map iteration order the deserializer happened to produce. The walk
/// stops at [`MAX_DOCUMENT_DEPTH`], so a deeply nested document cannot drive unbounded
/// recursion.
fn write_document_canonical(out: &mut String, document: &Document) {
    fn walk(out: &mut String, document: &Document, depth: usize) {
        if depth > MAX_DOCUMENT_DEPTH {
            out.push_str("...");
            return;
        }
        match document {
            Document::Object(members) => {
                let mut names: Vec<&String> = members.keys().collect();
                names.sort();
                out.push('{');
                for name in names {
                    out.push_str(name);
                    out.push('=');
                    if let Some(member) = members.get(name) {
                        walk(out, member, depth + 1);
                    }
                    out.push(';');
                }
                out.push('}');
            }
            Document::Array(items) => {
                out.push('[');
                for item in items {
                    walk(out, item, depth + 1);
                    out.push(',');
                }
                out.push(']');
            }
            Document::Number(Number::PosInt(value)) => out.push_str(&format!("u{value}")),
            Document::Number(Number::NegInt(value)) => out.push_str(&format!("i{value}")),
            Document::Number(Number::Float(value)) => out.push_str(&format!("f{value}")),
            Document::String(value) => out.push_str(&format!("s{value:?}")),
            Document::Bool(value) => out.push_str(&format!("b{value}")),
            Document::Null => out.push_str("null"),
        }
    }
    walk(out, document, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ops::types::{
        ComputeLogprobsConfig, ForwardBackwardResult, ForwardResult, LoadStateConfig,
        SampleResponse, SampleTokenUsage, SampledSequence, TrainingTokenUsage,
    };

    /// One settled training operation whose result reports `training_tokens`, at
    /// `sequence`. `kind` picks which result member carries the usage, because the two
    /// training results that report it are the ones that put data through the model.
    fn training_op(sequence: i32, kind: &str, training_tokens: Option<i32>) -> TrainingOp {
        let usage = Some(TrainingTokenUsage {
            training_tokens,
        });
        let result = match kind {
            "forward" => Some(OperationResult::Forward(ForwardResult {
                loss_fn_output_type: None,
                loss_fn_outputs: None,
                metrics: None,
                token_usage: usage,
            })),
            "forwardBackward" => Some(OperationResult::forwardBackward(ForwardBackwardResult {
                loss_fn_output_type: None,
                loss_fn_outputs: None,
                metrics: None,
                token_usage: usage,
            })),
            // A result member that carries no token usage at all, so nothing is accounted
            // for it.
            _ => Some(OperationResult::loadState(LoadStateResult {})),
        };
        TrainingOp {
            operation_id: format!("training-{sequence}-{kind}"),
            sequence_number: sequence,
            client_token: None,
            request_fingerprint: String::new(),
            config: OperationConfig::loadState(LoadStateConfig::default()),
            status: STATUS_COMPLETED,
            result,
            error_details: None,
            submitted_at: UNIX_EPOCH,
            due_at: None,
            reserved: None,
        }
    }

    /// One settled sampling operation whose sampled sequences report `(prefill, decode)`
    /// each, and `None` for an operation whose result carries no usage.
    fn sampling_op(id: &str, usage: &[(i32, i32)]) -> SamplingOp {
        let sequences = usage
            .iter()
            .map(|(prefill, decode)| SampledSequence {
                stop_reason: "stop".to_string(),
                tokens: vec![1, 2],
                logprobs: None,
                token_usage: Some(SampleTokenUsage {
                    prefill: *prefill,
                    decode: *decode,
                }),
            })
            .collect();
        SamplingOp {
            operation_id: id.to_string(),
            config: SampleOperationConfig::computeLogprobs(ComputeLogprobsConfig::default()),
            status: STATUS_COMPLETED,
            result: Some(SampleOperationResult::Sample(SampleResponse {
                sequences,
                prompt_logprobs: None,
                topk_prompt_logprobs: None,
            })),
            error_details: None,
            submitted_at: UNIX_EPOCH,
            due_at: None,
        }
    }

    /// One row of the accounting table: the traffic a session holds and the totals it must
    /// then have accounted.
    struct UsageRow {
        case: &'static str,
        /// The sequence number the session last received.
        received: i32,
        training_ops: Vec<TrainingOp>,
        sampling_ops: Vec<SamplingOp>,
        /// The expected `(cursor, prefill, sample, training)`.
        expected: (i32, i64, i64, i64),
    }

    /// RULE: what a session has accounted is COUNTED from its own records — the cursor it
    /// moved on each received training operation, and the sums of the token usage its
    /// operations' results report, each kind of usage into the counter it belongs to.
    #[test]
    fn accounted_usage_sums_what_the_operations_report() {
        // The rows span what the rule generalises over: a session with no operation at
        // all, each kind of traffic on its own, results that report no usage, several
        // operations and several sampled sequences of one operation.
        let rows = vec![
            UsageRow {
                case: "no operation at all",
                received: 0,
                training_ops: vec![],
                sampling_ops: vec![],
                expected: (0, 0, 0, 0),
            },
            UsageRow {
                case: "one forward pass reporting training tokens",
                received: 1,
                training_ops: vec![training_op(1, "forward", Some(12))],
                sampling_ops: vec![],
                expected: (1, 0, 0, 12),
            },
            UsageRow {
                case: "a result that reports no usage",
                received: 4,
                training_ops: vec![training_op(4, "loadState", None)],
                sampling_ops: vec![],
                expected: (4, 0, 0, 0),
            },
            UsageRow {
                case: "a usage structure whose count is absent",
                received: 2,
                training_ops: vec![training_op(2, "forwardBackward", None)],
                sampling_ops: vec![],
                expected: (2, 0, 0, 0),
            },
            UsageRow {
                case: "several training operations of both reporting kinds",
                received: 3,
                training_ops: vec![
                    training_op(1, "forward", Some(5)),
                    training_op(2, "forwardBackward", Some(7)),
                    training_op(3, "loadState", None),
                ],
                sampling_ops: vec![],
                expected: (3, 0, 0, 12),
            },
            UsageRow {
                case: "one sampling operation of several sequences",
                received: 0,
                training_ops: vec![],
                sampling_ops: vec![sampling_op("sampling-a", &[(10, 3), (20, 4)])],
                expected: (0, 30, 7, 0),
            },
            UsageRow {
                case: "both kinds of traffic",
                received: 9,
                training_ops: vec![training_op(9, "forward", Some(100))],
                sampling_ops: vec![
                    sampling_op("sampling-a", &[(1, 2)]),
                    sampling_op("sampling-b", &[(3, 4)]),
                ],
                expected: (9, 4, 6, 100),
            },
        ];

        for row in rows {
            let UsageRow {
                case,
                received,
                training_ops,
                sampling_ops,
                expected,
            } = row;
            let mut session = Session::new("accounted".to_string(), UNIX_EPOCH);
            session.receive_training_operation(received);
            for op in training_ops {
                session.training_ops.insert(op.operation_id.clone(), op);
            }
            for op in sampling_ops {
                session.sampling_ops.insert(op.operation_id.clone(), op);
            }

            let usage = session.accounted_usage();
            let actual = (
                usage.last_received_sequence_number,
                usage.prefill_tokens,
                usage.sample_tokens,
                usage.training_tokens,
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected (sequence, prefill, sample, training) {expected:?}, \
                 got {actual:?}"
            );
        }
    }

    /// RULE: the cursor records RECEIPT — it is the sequence number of the last operation
    /// the session took, not the highest one and not the slot the queue executes next.
    #[test]
    fn the_cursor_is_the_last_sequence_number_received() {
        // Each row is the order the submits arrived in; the cursor is the last of them.
        let rows: Vec<(&str, Vec<i32>, i32)> = vec![
            ("nothing received", vec![], 0),
            ("one operation", vec![1], 1),
            ("in order", vec![1, 2, 3], 3),
            ("a gap left behind", vec![1, 5], 5),
            ("the gap filled afterwards", vec![1, 5, 2], 2),
        ];
        for (case, received, expected) in rows {
            let mut session = Session::new("cursor".to_string(), UNIX_EPOCH);
            for sequence in received {
                session.receive_training_operation(sequence);
            }
            let actual = session.accounted_usage().last_received_sequence_number;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the last received sequence number {expected}, \
                 got {actual}"
            );
        }
    }
}
