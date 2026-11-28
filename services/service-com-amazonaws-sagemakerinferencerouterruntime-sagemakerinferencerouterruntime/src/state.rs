// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime service.
//!
//! This service is an inference router's DATA PLANE: it binds two operations, both
//! invocations, and both of them answer the same two questions — "which destination of the
//! named routing target takes this request?" and "what does that destination answer?".
//!
//! # The registry a request routes by
//!
//! The model binds no operation that creates a router or a routing target (see
//! `OPERATIONS.md`: two operations, both invocations), so the routers, their routing targets
//! and the destinations each target lists are REGISTRY state of this service, written through
//! this module's own registration seam
//! ([`ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState::put_router`]
//! and [`ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState::put_routing_target`])
//! rather than by a handler of this service. A destination is registered with the ARN of the
//! routed resource, whether it can take a request, and the answer it serves — which is what
//! an invocation reads to compute its response, so nothing in a handler is a constant.
//!
//! # One selection for both operations
//!
//! Which destination takes a request, and why, is ONE rule ([`select_destination`]), called
//! by the real-time route and the asynchronous one alike, so the two cannot drift apart in
//! how they distribute requests or in the reason they report. The rule reads and advances a
//! cursor held on the stored routing target, so the distribution is state a later request
//! observes rather than a per-call choice.
//!
//! # Clock-owed transitions
//!
//! The one transition this service's calls owe is an accepted asynchronous submission's
//! outcome, due [`ASYNC_OUTCOME_DUE_AFTER`] after the call that accepted it — see
//! [`AsyncSubmission::settle`].

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

/// The message a handler refuses with when this service is mounted on an engine that holds
/// no store for it. Unreachable in the assembled emulator (the router mounts every service
/// on the one shared `InMemoryEngine`); stated once so no handler invents its own wording —
/// and never by building a private engine, which would drop the request's writes.
pub const NO_STATE: &str = "SageMaker Inference Router Runtime state is unavailable: this \
                            service is not mounted on the shared in-memory engine";

/// How many invocations the log below keeps.
///
/// Bounded so a client invoking a router in a loop cannot grow this service's state without
/// end; the oldest entry is dropped when a new one arrives at the cap.
pub const MAX_RECORDED_INVOCATIONS: usize = 1024;

/// How many accepted asynchronous submissions the queue below keeps, bounded for the same
/// reason as [`MAX_RECORDED_INVOCATIONS`].
pub const MAX_QUEUED_ASYNC_SUBMISSIONS: usize = 1024;

/// How long after `InvokeInferenceRouterAsync` accepted a submission its outcome is due.
///
/// A CHOSEN span, not a published one: the real wait depends on the destination's backlog,
/// its instance count and its container. It is deliberately short, but never zero, so a
/// caller that reads the record straight back still observes
/// [`AsyncSubmissionStatus::InProgress`] — the state a submission the router has only
/// accepted is in.
pub const ASYNC_OUTCOME_DUE_AFTER: Duration = Duration::from_secs(3);

/// Why an accepted asynchronous submission ends in [`AsyncSubmissionStatus::Failed`].
///
/// The destination "reads the input from Amazon S3, and the inference response is written to
/// the Amazon S3 output path that the routing target specifies"
/// (`InvokeInferenceRouterAsync`'s model doc), and both are done by the destination's own
/// model container, which this emulator does not run. The submission is still ACCEPTED (HTTP
/// 202 answers the acceptance, which this service really performed), and the run nothing
/// performed ends in the failure state with this reason rather than in a completion claiming
/// an output object nothing wrote.
pub const NO_ASYNC_INFERENCE: &str =
    "No inference was performed: the destination serves an asynchronous request from its own \
     model container, which this emulator does not run, so no object was written to the \
     routing target's Amazon S3 output path.";

/// `RoutingReason` for a destination that is healthy and in the router's own Region: "The
/// destination is a healthy destination in the router's Region"
/// (`InvokeInferenceRouterOutput$RoutingReason`).
pub const REASON_LOCAL_PREFERRED: &str = "local-preferred";

/// `RoutingReason` for a request no destination in the router's Region could take, where two
/// or more destinations in other Regions could (same member's doc).
pub const REASON_CAPACITY_AVAILABLE: &str = "capacity-available";

/// `RoutingReason` for a request no destination in the router's Region could take, where
/// exactly one destination in another Region could (same member's doc).
pub const REASON_ONLY_HEALTHY: &str = "only-healthy";

/// `RoutingReason` for a request the router moved away from its own Region's destinations —
/// which is also what it reports "when no destination is healthy and the router tries one
/// anyway" (same member's doc), the case this emulator reaches it in.
pub const REASON_FAILOVER: &str = "failover";

/// The ARN namespace a router and every destination of a routing target are named in
/// (`aws.api#service$arnNamespace`, as `OPERATIONS.md` reports it for this service).
pub const ARN_NAMESPACE: &str = "sagemaker";

/// `RequestTTLSeconds`'s default: "The time to live of the request, in seconds, measured from
/// when the router accepts it. ... The default is 21,600 seconds (6 hours)"
/// (`InvokeInferenceRouterAsyncInput$RequestTTLSeconds`).
pub const DEFAULT_REQUEST_TTL_SECONDS: i32 = 21_600;

/// `InvocationTimeoutSeconds`'s default: "The maximum time, in seconds, that a destination can
/// spend processing the request before the request is marked as expired. The default is 900
/// seconds (15 minutes)" (`InvokeInferenceRouterAsyncInput$InvocationTimeoutSeconds`).
pub const DEFAULT_INVOCATION_TIMEOUT_SECONDS: i32 = 900;

/// The kind label the family's not-found sentence names an inference router by
/// (`service_common::sagemaker_resource_refusal::could_not_find`).
pub const ROUTER_KIND_LABEL: &str = "inference router";

/// The kind label that sentence names a routing target by; see [`ROUTER_KIND_LABEL`].
pub const ROUTING_TARGET_KIND_LABEL: &str = "routing target";

/// How this service states a router the registry does not hold: the SageMaker family's own
/// sentence for a resource the account does not hold, which lives once in
/// `service_common::sagemaker_resource_refusal` so this service words that fact the way its
/// family words it. Only the kind label and the identifier are this service's.
#[must_use]
pub fn could_not_find(kind: &str, identifier: &str) -> String {
    ::service_common::sagemaker_resource_refusal::could_not_find(kind, identifier)
}

/// How this service states a routing target the named router does not hold: the family's
/// not-found sentence for the target, and the router it was looked for in — which is what the
/// caller has to correct, since a target name is only meaningful within one router.
#[must_use]
pub fn no_such_routing_target(target_name: &str, router_name: &str) -> String {
    format!(
        "{} It belongs to no {ROUTER_KIND_LABEL} \"{router_name}\".",
        could_not_find(ROUTING_TARGET_KIND_LABEL, target_name)
    )
}

/// How this service states a routing target that lists no destination at all: there is
/// nothing to route to, which is the condition `ServiceUnavailableException` documents ("no
/// destination of the routing target could serve the request").
#[must_use]
pub fn no_destination(target_name: &str, router_name: &str) -> String {
    format!(
        "No destination of {ROUTING_TARGET_KIND_LABEL} \"{target_name}\" of \
         {ROUTER_KIND_LABEL} \"{router_name}\" could serve the request: the \
         {ROUTING_TARGET_KIND_LABEL} lists none."
    )
}

/// How this service states a payload with nothing in it: "The input data for the inference,
/// in the format that your model expects. The body can't be empty."
/// (`InvokeInferenceRouterInput$Body`) — the member's own sentence for the rule, which the
/// model declares no `@length` minimum for, so the request boundary admits an empty payload
/// and this is where it is refused.
pub const EMPTY_BODY: &str = "The body can't be empty.";

/// How this service states a destination that cannot take a request, where the registry
/// records no more specific reason: the condition `ModelNotReadyException` documents.
#[must_use]
pub fn not_ready(routed_resource_arn: &str) -> String {
    format!("Destination {routed_resource_arn} isn't ready to serve the model.")
}

/// How this service states a destination that took the request and answered nothing: the
/// condition `ServiceUnavailableException` documents beside the one [`no_destination`] states
/// ("no destination of the routing target could serve the request, or a destination didn't
/// respond").
#[must_use]
pub fn no_response(routed_resource_arn: &str) -> String {
    format!("Destination {routed_resource_arn} didn't respond to the request.")
}

/// How this service states a routing target that specifies no Amazon S3 output path, so an
/// asynchronous submission to it has nowhere to report.
#[must_use]
pub fn no_output_path(target_name: &str, router_name: &str) -> String {
    format!(
        "{ROUTING_TARGET_KIND_LABEL} \"{target_name}\" of {ROUTER_KIND_LABEL} \
         \"{router_name}\" is not configured for asynchronous inference: it specifies no \
         Amazon S3 output path for the inference response."
    )
}

/// What a destination of a routing target answers a real-time invocation with: the payload
/// the destination's model container returned, and what it returned it as.
///
/// "The inference that the model returned" (`InvokeInferenceRouterOutput$Body`) is the
/// DESTINATION's answer, not the router's, so what the router reports is what the destination
/// it registered was registered as answering. A destination registered with no answer at all
/// is one nothing responds for, which the invocation reports as such
/// ([`no_response`]) rather than by inventing a body.
#[derive(Debug, Clone)]
pub struct DestinationResponse {
    /// The payload the destination answers with.
    pub body: Vec<u8>,
    /// "The MIME type of the inference returned in the response body"
    /// (`InvokeInferenceRouterOutput$ContentType`), when the destination declares one.
    pub content_type: Option<String>,
    /// "Provides additional information in the response about the inference. This is the
    /// value that the model container of the destination returned, unchanged"
    /// (`InvokeInferenceRouterOutput$CustomAttributes`) — so it is the DESTINATION's value,
    /// present only where the destination answers one. Never the request's own, which would
    /// report as the destination's answer something the destination never said.
    pub custom_attributes: Option<String>,
}

/// One destination of a routing target: a resource the router can send a request to.
#[derive(Debug, Clone)]
pub struct Destination {
    /// The ARN of the endpoint or inference component this destination routes to — what
    /// `InvokedRoutedResourceArn` reports for a request it served.
    pub routed_resource_arn: String,
    /// "The production variant of the destination endpoint that served the request"
    /// (`InvokeInferenceRouterOutput$InvokedProductionVariant`), when the destination is
    /// deployed on one.
    pub production_variant: Option<String>,
    /// Whether the destination can take a request: a destination whose model has no copies
    /// running is not ready to serve it, which is what `ModelNotReadyException` reports.
    pub ready: bool,
    /// What the destination answers a real-time invocation with; see [`DestinationResponse`].
    pub response: Option<DestinationResponse>,
}

impl Destination {
    /// A destination routing to `routed_resource_arn`, ready, answering nothing.
    #[must_use]
    pub fn new(routed_resource_arn: impl Into<String>) -> Self {
        Self {
            routed_resource_arn: routed_resource_arn.into(),
            production_variant: None,
            ready: true,
            response: None,
        }
    }

    /// The same destination, deployed on the endpoint's production variant `variant`.
    #[must_use]
    pub fn in_production_variant(mut self, variant: impl Into<String>) -> Self {
        self.production_variant = Some(variant.into());
        self
    }

    /// The same destination, answering `body` as `content_type`.
    #[must_use]
    pub fn answering(mut self, body: impl Into<Vec<u8>>, content_type: Option<String>) -> Self {
        self.response = Some(DestinationResponse {
            body: body.into(),
            content_type,
            custom_attributes: None,
        });
        self
    }

    /// The same destination, whose model container returns `custom_attributes` beside its
    /// answer; a destination that answers nothing returns none either.
    #[must_use]
    pub fn with_custom_attributes(mut self, custom_attributes: impl Into<String>) -> Self {
        if let Some(response) = self.response.as_mut() {
            response.custom_attributes = Some(custom_attributes.into());
        }
        self
    }

    /// The same destination, whose model has no copies running — so it cannot take a request.
    #[must_use]
    pub fn not_ready(mut self) -> Self {
        self.ready = false;
        self
    }

    /// The Region the destination's ARN names, empty for an ARN that names none and for a
    /// value that is not an ARN at all. What [`select_destination`] compares against the
    /// router's own Region.
    #[must_use]
    pub fn region(&self) -> &str {
        ::service_common::arn::parse(&self.routed_resource_arn).map_or("", |arn| arn.region)
    }
}

/// One routing target of an inference router: "The routing target lists the destinations that
/// can serve the request" (`InvokeInferenceRouterInput$RoutingTargetName`).
#[derive(Debug, Clone)]
pub struct RoutingTarget {
    pub name: String,
    /// The destinations that can serve a request, in the order the target lists them.
    pub destinations: Vec<Destination>,
    /// The Amazon S3 output path the target specifies, where an asynchronous inference
    /// response is written ("the inference response is written to the Amazon S3 output path
    /// that the routing target specifies", `InvokeInferenceRouterAsync`'s model doc).
    pub s3_output_path: Option<String>,
    /// Where a failure object is written for an asynchronous request no destination could
    /// take ("the router writes a failure object to the FailureLocation", same doc).
    pub s3_failure_path: Option<String>,
    /// `MaxTimeInBacklogSeconds`: how long a destination may leave an asynchronous request
    /// unstarted before the router moves it to another destination (same doc).
    pub max_time_in_backlog_seconds: Option<i32>,
    /// Where the next selection starts walking the destination list: the round-robin cursor,
    /// advanced past every destination this target's selections chose, so successive requests
    /// on a multi-destination target are distributed over them.
    next_destination: usize,
}

impl RoutingTarget {
    /// A routing target named `name` listing `destinations`, with no Amazon S3 paths and no
    /// backlog bound configured.
    #[must_use]
    pub fn new(name: impl Into<String>, destinations: Vec<Destination>) -> Self {
        Self {
            name: name.into(),
            destinations,
            s3_output_path: None,
            s3_failure_path: None,
            max_time_in_backlog_seconds: None,
            next_destination: 0,
        }
    }

    /// The same target, specifying `s3_output_path` for an asynchronous response and
    /// `s3_failure_path` for a failure object.
    #[must_use]
    pub fn writing_to(
        mut self,
        s3_output_path: impl Into<String>,
        s3_failure_path: Option<String>,
    ) -> Self {
        self.s3_output_path = Some(s3_output_path.into());
        self.s3_failure_path = s3_failure_path;
        self
    }

    /// The same target, moving an asynchronous request a destination has not started within
    /// `seconds` to another destination.
    #[must_use]
    pub fn with_max_time_in_backlog_seconds(mut self, seconds: i32) -> Self {
        self.max_time_in_backlog_seconds = Some(seconds);
        self
    }

    /// Where the next selection starts walking this target's destinations.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.next_destination
    }
}

/// An inference router: the resource an invocation names, holding the routing targets a
/// request can name.
#[derive(Debug, Clone)]
pub struct InferenceRouter {
    pub name: String,
    /// The router's own ARN, minted in the Region it was registered for.
    pub arn: String,
    /// "The router's Region" (`InvokeInferenceRouterOutput$RoutingReason`), which is what
    /// decides whether a destination is local to it.
    pub region: String,
    pub routing_targets: BTreeMap<String, RoutingTarget>,
}

/// The answer to "does this router hold that routing target?".
#[derive(Debug, Clone)]
pub enum TargetLookup {
    /// No router of that name is registered.
    RouterNotFound,
    /// The router holds no routing target of that name.
    RoutingTargetNotFound,
    /// The target, as the registry holds it.
    Found(TargetFacts),
}

/// What the registry holds about the routing target a request named.
#[derive(Debug, Clone)]
pub struct TargetFacts {
    pub router_arn: String,
    /// The Region the router itself is in; see [`InferenceRouter::region`].
    pub router_region: String,
    pub destinations: Vec<Destination>,
    pub s3_output_path: Option<String>,
    pub s3_failure_path: Option<String>,
    pub max_time_in_backlog_seconds: Option<i32>,
}

/// Which destination a selection chose, and why.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Where the chosen destination sits in the target's own destination list.
    pub index: usize,
    /// The chosen destination, as the target lists it.
    pub destination: Destination,
    /// Why the router chose it, from the vocabulary the output member documents
    /// ([`REASON_LOCAL_PREFERRED`] and the three beside it).
    pub reason: &'static str,
}

/// Which destination of `target` takes the next request, and the reason the router chose it.
///
/// THE SELECTION RULE, for both of this service's routes: among the destinations that can
/// take the request, the router prefers the ones in its OWN Region and distributes successive
/// requests over them in the order the target lists them (the stored round-robin cursor,
/// advanced past the chosen destination); where its own Region has none, it takes one in
/// another Region; where nothing can take the request at all, it tries one anyway. Those four
/// cases are exactly the reasons the output member documents, so the reason reported is
/// derived from the same read that chose the destination:
///
/// * [`REASON_LOCAL_PREFERRED`] — the chosen destination is ready and in the router's Region;
/// * [`REASON_ONLY_HEALTHY`] — no ready destination in the router's Region, and exactly one
///   ready elsewhere;
/// * [`REASON_CAPACITY_AVAILABLE`] — no ready destination in the router's Region, and two or
///   more ready elsewhere;
/// * [`REASON_FAILOVER`] — "no destination is healthy and the router tries one anyway".
///
/// `None` for a target that lists no destination at all: there is nothing to choose.
pub fn select_destination(target: &mut RoutingTarget, router_region: &str) -> Option<Selection> {
    let count = target.destinations.len();
    if count == 0 {
        return None;
    }
    let is = |index: usize, local: bool, ready: bool| {
        target.destinations.get(index).is_some_and(|destination| {
            destination.ready == ready && (destination.region() == router_region) == local
        })
    };
    let local: Vec<usize> = (0..count).filter(|index| is(*index, true, true)).collect();
    let remote: Vec<usize> = (0..count).filter(|index| is(*index, false, true)).collect();
    let (candidates, reason) = if !local.is_empty() {
        (local, REASON_LOCAL_PREFERRED)
    } else if remote.len() == 1 {
        (remote, REASON_ONLY_HEALTHY)
    } else if remote.len() > 1 {
        (remote, REASON_CAPACITY_AVAILABLE)
    } else {
        ((0..count).collect(), REASON_FAILOVER)
    };
    // The cursor is where this target's last selection left off, so the first candidate at or
    // after it is the one that takes this request: a target whose destinations can all take
    // it hands them out in turn, and one whose next destination cannot skips to the one that
    // can.
    let index = (0..count)
        .map(|step| (target.next_destination.wrapping_add(step)) % count)
        .find(|index| candidates.contains(index))?;
    let destination = target.destinations.get(index)?.clone();
    target.next_destination = (index + 1) % count;
    Some(Selection {
        index,
        destination,
        reason,
    })
}

/// What this service did with one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationOutcome {
    /// No inference router of that name is registered.
    RouterNotFound,
    /// The router holds no routing target of that name.
    RoutingTargetNotFound,
    /// The routing target lists no destination, so there was nothing to route to.
    NoDestination,
    /// The request carries an empty payload, which is no input to infer from.
    EmptyPayload,
    /// The destination the router chose cannot take the request.
    DestinationNotReady,
    /// The destination the router chose took the request and answered nothing.
    DestinationDidNotRespond,
    /// The routing target specifies no Amazon S3 output path, so an accepted asynchronous
    /// submission would have nowhere to report.
    NoOutputPath,
    /// The destination the router chose served the inference this call answered with.
    Served,
    /// The asynchronous submission was accepted and recorded.
    Queued,
    /// The asynchronous submission names an `InferenceId` this service already accepted, and
    /// was answered from that record.
    AlreadyAccepted,
}

impl InvocationOutcome {
    /// A stable short name for the outcome, for logs and test assertions.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::RouterNotFound => "RouterNotFound",
            Self::RoutingTargetNotFound => "RoutingTargetNotFound",
            Self::NoDestination => "NoDestination",
            Self::EmptyPayload => "EmptyPayload",
            Self::DestinationNotReady => "DestinationNotReady",
            Self::DestinationDidNotRespond => "DestinationDidNotRespond",
            Self::NoOutputPath => "NoOutputPath",
            Self::Served => "Served",
            Self::Queued => "Queued",
            Self::AlreadyAccepted => "AlreadyAccepted",
        }
    }
}

/// One invocation this service served, as the log below replays it.
///
/// The request BODY and `CustomAttributes` are `@sensitive` in the model, so neither is
/// stored: the record keeps the body's SIZE, which is what a metric would report, and the
/// routing members of the request.
#[derive(Debug, Clone)]
pub struct RouterInvocation {
    pub operation: &'static str,
    pub inference_router_name: String,
    pub routing_target_name: String,
    /// The destination the router chose; absent for a call refused before one was chosen.
    pub routed_resource_arn: Option<String>,
    /// Why the router chose it ([`select_destination`]).
    pub routing_reason: Option<&'static str>,
    pub production_variant: Option<String>,
    /// `Body`'s length in bytes; the body itself is `@sensitive` and is not stored.
    pub request_bytes: usize,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub inference_id: Option<String>,
    pub outcome: InvocationOutcome,
    pub at: SystemTime,
}

/// The lifecycle of an accepted asynchronous submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncSubmissionStatus {
    /// Accepted, and its outcome not yet due.
    InProgress,
    /// Its outcome was due and nothing performed the inference — see [`NO_ASYNC_INFERENCE`].
    Failed,
}

/// One accepted `InvokeInferenceRouterAsync` submission, as the queue below holds it.
///
/// `CustomAttributes` is `@sensitive`, so it is not kept. The two locations are the ones the
/// response reported, stored so a resubmission under the same `InferenceId` answers the same
/// strings.
#[derive(Debug, Clone)]
pub struct AsyncSubmission {
    pub inference_id: String,
    pub inference_router_name: String,
    pub routing_target_name: String,
    /// The destination the router sent the request to, and why it chose it.
    pub routed_resource_arn: String,
    pub routing_reason: &'static str,
    pub input_location: String,
    pub output_location: String,
    pub failure_location: Option<String>,
    pub request_ttl_seconds: i32,
    pub invocation_timeout_seconds: i32,
    pub status: AsyncSubmissionStatus,
    /// Why the submission failed, once its outcome is due.
    pub failure_reason: Option<String>,
    pub submitted_at: SystemTime,
    /// When the submission's outcome falls due — [`ASYNC_OUTCOME_DUE_AFTER`] after
    /// [`Self::submitted_at`].
    pub outcome_due_at: SystemTime,
}

impl AsyncSubmission {
    /// Materialise the outcome the clock already owed this submission.
    ///
    /// Owed at [`Self::outcome_due_at`] whether or not anybody read the record, so a reader
    /// makes no difference to the result. Answers whether this call moved it.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status == AsyncSubmissionStatus::InProgress && now >= self.outcome_due_at {
            self.status = AsyncSubmissionStatus::Failed;
            self.failure_reason = Some(NO_ASYNC_INFERENCE.to_string());
            return true;
        }
        false
    }
}

/// Per-service state for com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState {
    /// The inference routers this emulator is configured with, keyed by name.
    routers: BTreeMap<String, InferenceRouter>,
    /// Every invocation this service served, oldest first, capped at
    /// [`MAX_RECORDED_INVOCATIONS`].
    invocations: VecDeque<RouterInvocation>,
    /// Every asynchronous submission this service accepted, oldest first, capped at
    /// [`MAX_QUEUED_ASYNC_SUBMISSIONS`].
    async_submissions: VecDeque<AsyncSubmission>,
    /// How many asynchronous submissions this store has ever accepted.
    ///
    /// MONOTONIC, unlike the queue's length, which falls back as the cap evicts: it is what a
    /// generated `InferenceId` is mixed from, so two submissions cannot share one even after
    /// eviction.
    accepted_submissions: u64,
}

impl ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState {
    /// Register an inference router named `name` in `region`, and answer its ARN.
    ///
    /// The registration seam this module's header describes: the model binds no operation
    /// that creates a router, so this is how the configuration a request routes by is
    /// written. Registering a name already held keeps that router's routing targets and
    /// their selection cursors, so a re-registration is not a way to reset the distribution.
    pub fn put_router(&mut self, region: &str, name: &str) -> String {
        let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("inference-router/{name}"));
        self.routers
            .entry(name.to_string())
            .or_insert_with(|| InferenceRouter {
                name: name.to_string(),
                arn,
                region: region.to_string(),
                routing_targets: BTreeMap::new(),
            })
            .arn
            .clone()
    }

    /// Give the router `router_name` the routing target `target`, answering whether that
    /// router is registered at all.
    pub fn put_routing_target(&mut self, router_name: &str, target: RoutingTarget) -> bool {
        let Some(router) = self.routers.get_mut(router_name) else {
            return false;
        };
        router.routing_targets.insert(target.name.clone(), target);
        true
    }

    /// The router `name`, as a reader sees it.
    #[must_use]
    pub fn router(&self, name: &str) -> Option<&InferenceRouter> {
        self.routers.get(name)
    }

    /// What the registry holds about the routing target `target_name` of `router_name`.
    #[must_use]
    pub fn resolve_target(&self, router_name: &str, target_name: &str) -> TargetLookup {
        let Some(router) = self.routers.get(router_name) else {
            return TargetLookup::RouterNotFound;
        };
        let Some(target) = router.routing_targets.get(target_name) else {
            return TargetLookup::RoutingTargetNotFound;
        };
        TargetLookup::Found(TargetFacts {
            router_arn: router.arn.clone(),
            router_region: router.region.clone(),
            destinations: target.destinations.clone(),
            s3_output_path: target.s3_output_path.clone(),
            s3_failure_path: target.s3_failure_path.clone(),
            max_time_in_backlog_seconds: target.max_time_in_backlog_seconds,
        })
    }

    /// Which destination of `router_name`'s routing target `target_name` takes this request,
    /// advancing that target's own round-robin cursor; `None` when the router or the target
    /// is not registered, or the target lists no destination.
    ///
    /// The ONE selection both routes make, applied to the STORED target so the distribution
    /// is state a later request reads ([`select_destination`]).
    pub fn select(&mut self, router_name: &str, target_name: &str) -> Option<Selection> {
        let router = self.routers.get_mut(router_name)?;
        let region = router.region.clone();
        let target = router.routing_targets.get_mut(target_name)?;
        select_destination(target, &region)
    }

    /// Record one invocation this service served, dropping the oldest once the log is at
    /// [`MAX_RECORDED_INVOCATIONS`].
    pub fn record_invocation(&mut self, invocation: RouterInvocation) {
        if self.invocations.len() >= MAX_RECORDED_INVOCATIONS {
            self.invocations.pop_front();
        }
        self.invocations.push_back(invocation);
    }

    /// Every invocation this service served, oldest first.
    pub fn invocations(&self) -> impl Iterator<Item = &RouterInvocation> {
        self.invocations.iter()
    }

    /// Take the next submission sequence, advancing it; see [`Self::accepted_submissions`].
    pub fn next_submission_sequence(&mut self) -> u64 {
        self.accepted_submissions = self.accepted_submissions.saturating_add(1);
        self.accepted_submissions
    }

    /// The submission this store accepted for `router_name`'s routing target `target_name`
    /// under `inference_id`, as the next request would read it.
    ///
    /// Keyed by all three: an `InferenceId` is the CALLER's identifier for its own request,
    /// and two callers naming one id on two different routing targets have submitted two
    /// requests, whose results are written under each target's own configured paths.
    #[must_use]
    pub fn accepted_submission(
        &self,
        router_name: &str,
        target_name: &str,
        inference_id: &str,
    ) -> Option<&AsyncSubmission> {
        self.async_submissions.iter().find(|submission| {
            submission.inference_router_name == router_name
                && submission.routing_target_name == target_name
                && submission.inference_id == inference_id
        })
    }

    /// Record `submission` unless this store already holds one under the same router, routing
    /// target and `InferenceId` — answering the record it already held, and inserting nothing
    /// for it.
    ///
    /// Check and insert in ONE locked call, so two submissions naming one id cannot both be
    /// accepted however they interleave. The oldest record is dropped once the queue is at
    /// [`MAX_QUEUED_ASYNC_SUBMISSIONS`].
    pub fn accept_async_submission(
        &mut self,
        submission: AsyncSubmission,
    ) -> Option<AsyncSubmission> {
        if let Some(accepted) = self.accepted_submission(
            &submission.inference_router_name,
            &submission.routing_target_name,
            &submission.inference_id,
        ) {
            return Some(accepted.clone());
        }
        if self.async_submissions.len() >= MAX_QUEUED_ASYNC_SUBMISSIONS {
            self.async_submissions.pop_front();
        }
        self.async_submissions.push_back(submission);
        None
    }

    /// Every asynchronous submission this service accepted, oldest first.
    pub fn async_submissions(&self) -> impl Iterator<Item = &AsyncSubmission> {
        self.async_submissions.iter()
    }

    /// Materialise the outcome the clock already owed every accepted submission, answering
    /// how many moved; see [`AsyncSubmission::settle`].
    pub fn settle_async_submissions(&mut self, now: SystemTime) -> usize {
        self.async_submissions
            .iter_mut()
            .map(|submission| usize::from(submission.settle(now)))
            .sum()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState>)
}
