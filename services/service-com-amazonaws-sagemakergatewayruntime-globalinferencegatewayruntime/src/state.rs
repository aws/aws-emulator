// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime service.
//!
//! This service is a DATA PLANE with two operations, both invocations, so everything a
//! request is decided against lives here:
//!
//! * the GATEWAY REGISTRY ([`Gateway`], [`ModelTarget`]) — which gateways this account and
//!   Region hold, which models each one's targets serve, and whether a model has capacity
//!   right now. The API binds no `CreateGateway`, so the inventory is PUBLISHED by the
//!   emulator ([`gateway_registry`]) exactly as a catalogue of models a caller cannot
//!   create is published elsewhere in this project, and it is materialised when the store
//!   is created rather than by a request, so no read ever changes what a later caller
//!   sees. A test reaches the same registry through [`ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState::set_target_capacity`]
//!   and [`ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState::put_gateway`].
//! * the INVOCATION LOG ([`InvocationRecord`]) — what the two operations served, which is
//!   this emulator's counterpart of the per-gateway invocation metrics the real service
//!   publishes. The request BODY is never stored: "The gateway reads only `model` and
//!   `stream`, and does not store the body", so only its byte length is.
//! * the COMPLETION SEQUENCE — the source of each answer's completion id, so two calls of
//!   one gateway are two distinct completions rather than one canned body served twice.
//!
//! Both operations resolve a request through the ONE resolver below
//! ([`ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState::resolve`]),
//! so the buffered and the streaming route cannot disagree about which gateway exists,
//! which model is served, or which model has capacity.

/// The OpenAI chat-completions envelope the answers are computed into, and the text a
/// served turn carries.
pub mod completion;

/// The gateway inventory this emulator publishes, because the API binds no create
/// operation for one.
pub mod gateway_registry;

use std::collections::BTreeMap;
use std::time::SystemTime;

/// The message a handler refuses with when the service is mounted on an engine that holds
/// no store for it. Unreachable in the assembled emulator (the router mounts every service
/// on the one shared `InMemoryEngine`); stated once so no handler invents its own
/// wording — and never by building a private engine, which would drop the request's writes.
pub const NO_STATE: &str = "Global Inference Gateway: the gateway registry is unavailable: \
                            this service is not mounted on the shared in-memory engine";

/// The region an unsigned (anonymous) request is served in, and the region the published
/// registry's ARNs name.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The AWS region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How many invocations the log below keeps.
///
/// The log is bounded so a client invoking a gateway in a loop cannot grow this service's
/// state without end; the oldest entry is dropped when a new one arrives at the cap. The
/// completion sequence is NOT bounded by it: a dropped record does not make a later
/// completion id repeat an earlier one.
pub const MAX_RECORDED_INVOCATIONS: usize = 1024;

/// One target of a gateway: a model it serves, and whether it has capacity right now.
#[derive(Debug, Clone)]
pub struct ModelTarget {
    /// The model name a request's top-level `model` member names to reach this target.
    pub model: String,
    /// Whether an invocation of this model can be dispatched right now. `false` is the
    /// state the model documents as "a model that is served but has no available target
    /// right now", answered `ServiceUnavailableException` (503) rather than 404.
    pub available: bool,
}

/// One Global Inference Gateway: the id a request addresses it by, the ARN IAM policies
/// name it in, and the models its targets serve.
#[derive(Debug, Clone)]
pub struct Gateway {
    /// `GatewayId`, as stored (also this record's key in the registry).
    pub gateway_id: String,
    /// The gateway's ARN — "IAM policies refer to the gateway by its ARN" — in the Region
    /// the registry is published in ([`DEFAULT_REGION`]).
    pub arn: String,
    /// The models this gateway's targets serve, by model name.
    pub targets: BTreeMap<String, ModelTarget>,
    /// When this gateway entered the registry.
    pub created_at: SystemTime,
}

impl Gateway {
    /// A gateway serving `models`, each with the capacity the row states.
    pub fn new(gateway_id: &str, region: &str, models: &[(&str, bool)], now: SystemTime) -> Self {
        Self {
            gateway_id: gateway_id.to_string(),
            arn: gateway_arn(region, gateway_id),
            targets: models
                .iter()
                .map(|(model, available)| {
                    (
                        (*model).to_string(),
                        ModelTarget {
                            model: (*model).to_string(),
                            available: *available,
                        },
                    )
                })
                .collect(),
            created_at: now,
        }
    }
}

/// One gateway's ARN: this service's ARN namespace (`sagemaker`), the Region it is
/// published in, and the resource form the model states —
/// `arn:aws:sagemaker:<region>:<account>:global-inference-gateway/<id>`.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(
        "sagemaker",
        region,
        &format!("global-inference-gateway/{gateway_id}"),
    )
}

/// Which operation served an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationKind {
    /// `InvokeGlobalInferenceGateway` — one buffered JSON document.
    Buffered,
    /// `InvokeGlobalInferenceGatewayWithResponseStream` — an event stream of chunks.
    Streamed,
}

impl InvocationKind {
    /// Whether this invocation was served as a stream, as the log reports it.
    #[must_use]
    pub fn streaming(self) -> bool {
        matches!(self, Self::Streamed)
    }

    /// The operation's own name, for a trace line.
    #[must_use]
    pub fn operation(self) -> &'static str {
        match self {
            Self::Buffered => "InvokeGlobalInferenceGateway",
            Self::Streamed => "InvokeGlobalInferenceGatewayWithResponseStream",
        }
    }
}

/// One served invocation, as the log replays it.
///
/// The request body is not here: the gateway reads only `model` and `stream` out of it and
/// does not store it, so what the log carries is the body's SIZE and the two members that
/// were read. The customer identity token is `@sensitive` and "Redacted from logs,
/// metrics and traces", so only WHETHER one was presented is recorded.
#[derive(Debug, Clone)]
pub struct InvocationRecord {
    /// This invocation's position in the completion sequence, which its completion id is
    /// drawn from.
    pub seq: u64,
    /// The gateway that served it.
    pub gateway_id: String,
    /// The model its body named, as resolved against the gateway's targets.
    pub model: String,
    /// How many bytes of request body were read.
    pub request_bytes: usize,
    /// The `Content-Type` the request declared.
    pub content_type: String,
    /// The `Accept` the request asked the model server for, when it asked for one.
    pub accept: Option<String>,
    /// Whether the request carried a customer identity token.
    pub identity_token_present: bool,
    /// Whether the answer was streamed.
    pub streaming: bool,
    /// Whether the body asked for `"stream": true`. The buffered operation returns one
    /// JSON document whatever it says — "the request is always sent with `"stream":
    /// false`" — so this is what the caller asked for, not how it was served.
    pub stream_requested: bool,
    /// When it was served.
    pub at: SystemTime,
}

/// What a request's `{GatewayId}` and its body's `model` resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// No gateway with this id is in the registry: `ResourceNotFoundException` (404) about
    /// the gateway.
    NoSuchGateway,
    /// The gateway exists and no target of it serves this model:
    /// `ResourceNotFoundException` (404) about the model.
    NoSuchModel,
    /// The model is served and no target of it has capacity right now:
    /// `ServiceUnavailableException` (503), before the request is dispatched.
    NoCapacity,
    /// The gateway serves the model and a target has capacity: the invocation is served.
    Served,
}

/// Per-service state for com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState {
    /// The gateways of this account and Region, by `GatewayId`.
    gateways: BTreeMap<String, Gateway>,
    /// The invocations the two operations served, oldest first, bounded by
    /// [`MAX_RECORDED_INVOCATIONS`].
    invocations: Vec<InvocationRecord>,
    /// The next completion's position in the sequence: the source of the completion id
    /// each answer carries, so two calls never answer with one id.
    next_completion_seq: u64,
}

impl Default for ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState {
    /// The store as it exists before any request: the published registry already in it.
    ///
    /// The inventory is materialised HERE rather than at first access, so resolving a
    /// gateway is a pure read — nothing a reporting operation does changes what a later
    /// caller sees — and a test can mark a target out of capacity before the first
    /// invocation.
    fn default() -> Self {
        Self {
            gateways: gateway_registry::published(DEFAULT_REGION, SystemTime::now()),
            invocations: Vec::new(),
            next_completion_seq: 1,
        }
    }
}

impl ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState {
    /// Resolve a request's gateway id and model against the registry. A pure read: the one
    /// question both operations ask before anything is served or refused.
    #[must_use]
    pub fn resolve(&self, gateway_id: &str, model: &str) -> Resolution {
        let Some(gateway) = self.gateways.get(gateway_id) else {
            return Resolution::NoSuchGateway;
        };
        match gateway.targets.get(model) {
            None => Resolution::NoSuchModel,
            Some(target) if target.available => Resolution::Served,
            Some(_) => Resolution::NoCapacity,
        }
    }

    /// One gateway of the registry, as the next caller reads it.
    #[must_use]
    pub fn gateway(&self, gateway_id: &str) -> Option<&Gateway> {
        self.gateways.get(gateway_id)
    }

    /// Every gateway of the registry, in `GatewayId` order.
    pub fn gateways(&self) -> impl Iterator<Item = &Gateway> {
        self.gateways.values()
    }

    /// Put `gateway` in the registry, replacing any gateway already held under its id.
    ///
    /// The API binds no create operation, so this is how a caller OTHER than the published
    /// inventory — a test — states which gateways this account holds.
    pub fn put_gateway(&mut self, gateway: Gateway) {
        self.gateways.insert(gateway.gateway_id.clone(), gateway);
    }

    /// Set whether one target of one gateway has capacity, answering whether that target
    /// is in the registry at all.
    ///
    /// This is the state the 503 is decided from, so it is settable: a model with no
    /// available target is a condition of the fleet, which no API operation of this
    /// service changes.
    pub fn set_target_capacity(&mut self, gateway_id: &str, model: &str, available: bool) -> bool {
        match self
            .gateways
            .get_mut(gateway_id)
            .and_then(|gateway| gateway.targets.get_mut(model))
        {
            Some(target) => {
                target.available = available;
                true
            }
            None => false,
        }
    }

    /// Take the next completion's position in the sequence.
    ///
    /// Called once per SERVED invocation, so a refused request consumes nothing and two
    /// served ones never share a position — which is what makes their completion ids
    /// differ.
    pub fn next_completion_seq(&mut self) -> u64 {
        let seq = self.next_completion_seq;
        self.next_completion_seq = self.next_completion_seq.saturating_add(1);
        seq
    }

    /// Record one served invocation, dropping the oldest when the log is at its cap.
    pub fn record_invocation(&mut self, record: InvocationRecord) {
        if self.invocations.len() >= MAX_RECORDED_INVOCATIONS {
            self.invocations.remove(0);
        }
        self.invocations.push(record);
    }

    /// The invocations the log holds, oldest first.
    #[must_use]
    pub fn invocations(&self) -> &[InvocationRecord] {
        &self.invocations
    }

    /// How many invocations the log holds for one gateway.
    #[must_use]
    pub fn invocation_count(&self, gateway_id: &str) -> usize {
        self.invocations
            .iter()
            .filter(|record| record.gateway_id == gateway_id)
            .count()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState>)
}
