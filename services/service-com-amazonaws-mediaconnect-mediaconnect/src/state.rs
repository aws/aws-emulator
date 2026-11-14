// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediaconnect-mediaconnect service.
//!
//! The store is keyed by the flow's own ARN, which is what every flow operation
//! addresses a flow by (`@httpLabel FlowArn`) and which is the flow's identity: the ARN
//! the service mints carries a per-flow generated id as well as the name
//! ([`flow_resource_fragment`]), so two flows that share a name are still distinct
//! resources with distinct ARNs. There is no index by name beside it, because no
//! operation addresses a flow by name and the model puts no uniqueness rule on
//! `Flow.Name`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AddBridgeOutputRequest, AddBridgeSourceRequest, AddMediaStreamRequest, AddOutputRequest,
    AutomaticEncryptionKeyConfiguration,
    Bridge, BridgeFlowOutput, BridgeFlowSource, BridgeNetworkOutput, BridgeNetworkSource,
    BridgeOutput, BridgeSource,
    ContentQualityAnalysisFeatureConfiguration,
    DestinationConfiguration, EgressGatewayBridge, EncodingConfig, EncodingParameters, Encryption,
    Entitlement, FabricConfiguration, FailoverConfig, FailoverRouterInputIndexedStreamDetails,
    FailoverRouterInputProtocolConfiguration,
    FailoverRouterInputStreamDetails, Flow, FlowTransitEncryption,
    FlowTransitEncryptionKeyConfiguration, Gateway,
    GatewayInstance,
    GatewayNetwork,
    GatewayBridgeSource, GrantEntitlementRequest, IngressGatewayBridge, InputConfiguration,
    Interface, ListedBridge, ListedEntitlement, ListedFlow,
    ListedGateway,
    ListedGatewayInstance,
    ListedRouterInput,
    ListedRouterNetworkInterface,
    ListedRouterOutput,
    Maintenance, MaintenanceConfiguration, MaintenanceSchedule,
    MediaConnectFlowRouterInputStreamDetails,
    MediaConnectFlowRouterOutputStreamDetails, MediaStream,
    MediaLiveChannelRouterInputStreamDetails,
    MediaLiveTransitEncryption, MediaLiveTransitEncryptionKeyConfiguration,
    MediaStreamAttributes, MediaStreamAttributesRequest,
    MediaLiveInputRouterOutputStreamDetails,
    MediaStreamOutputConfiguration, MediaStreamOutputConfigurationRequest,
    MediaStreamSourceConfiguration, MediaStreamSourceConfigurationRequest,
    MergeRouterInputIndexedStreamDetails,
    MergeRouterInputProtocolConfiguration,
    MergeRouterInputStreamDetails, MonitoringConfig,
    NdiConfig, Offering, Output, Reservation, ResourceSpecification,
    RouterContentQualityAnalysisConfiguration, RouterInput,
    RouterInputConfiguration, RouterInputMessage, RouterInputProtocolConfiguration,
    RouterInputStreamDetails,
    RouterInputTransitEncryption,
    RouterInputTransitEncryptionKeyConfiguration,
    RouterNetworkInterface, RouterNetworkInterfaceConfiguration,
    RouterOutput, RouterOutputConfiguration, RouterOutputMessage,
    RouterOutputProtocolConfiguration, RouterOutputStreamDetails, SetSourceRequest, Source,
    SourcePriority,
    StandardRouterInputConfiguration,
    StandardRouterInputStreamDetails, StandardRouterOutputConfiguration,
    StandardRouterOutputStreamDetails, Transport, UpdateEncryption,
    UpdateFailoverConfig, VpcInterface,
    VpcInterfaceAttachment, VpcInterfaceRequest, WindowMaintenanceSchedule,
};
mod arns;
mod cidr_blocks;
mod offerings;
mod router_filters;
mod routers;
mod gateways;
mod flows;
mod source_report_preconditions;
mod vpc_interface_role;
mod vpc_placement;
pub use cidr_blocks::*;
pub use flows::*;
pub use source_report_preconditions::*;
pub use vpc_interface_role::*;
pub use gateways::*;
pub use router_filters::*;
pub use routers::*;
pub use offerings::*;
pub use arns::*;
pub use vpc_placement::*;

/// The `Status` enum the model declares for a flow
/// (`smithy_query(query="shape",shape="Status")`: STANDBY | ACTIVE | UPDATING | DELETING
/// | STARTING | STOPPING | ERROR). Only the ones a flow's lifecycle reaches, and the one a
/// documented precondition names ([`STATUS_UPDATING`]), are named here; the rest belong to
/// operations that are still scaffolded.
pub const STATUS_STANDBY: &str = "STANDBY";
/// See [`STATUS_STANDBY`].
pub const STATUS_STARTING: &str = "STARTING";
/// See [`STATUS_STANDBY`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_STANDBY`]. No lifecycle call of this service puts a flow in it, but a
/// precondition names it beside [`STATUS_ACTIVE`] as a status a source report is served in
/// (see [`flow_source_thumbnail_refusal`]), so the value the model declares is named here
/// rather than spelled out there.
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_STANDBY`].
pub const STATUS_STOPPING: &str = "STOPPING";
/// See [`STATUS_STANDBY`].
pub const STATUS_DELETING: &str = "DELETING";

/// The `ReservationState` enum the model declares for a reservation
/// (`smithy_query(query="shape",shape="ReservationState")`: ACTIVE | EXPIRED | PROCESSING
/// | CANCELED). All four are named here: a purchase is reported in the non-terminal
/// `PROCESSING` state until the clock settles it `ACTIVE` (see
/// [`StoredReservation::settle`]), the clock retires an active reservation into the
/// terminal `EXPIRED` at its end instant, and `CANCELED` is the other terminal value.
pub const RESERVATION_STATE_PROCESSING: &str = "PROCESSING";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_ACTIVE: &str = "ACTIVE";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_EXPIRED: &str = "EXPIRED";
/// See [`RESERVATION_STATE_PROCESSING`].
pub const RESERVATION_STATE_CANCELED: &str = "CANCELED";

/// The `GatewayState` enum the model declares for a gateway
/// (`smithy_query(query="shape",shape="GatewayState")`: CREATING | ACTIVE | UPDATING |
/// ERROR | DELETING | DELETED). Only the four a gateway's lifecycle reaches here are
/// named; the rest belong to operations that are still scaffolded.
pub const GATEWAY_STATE_CREATING: &str = "CREATING";
/// See [`GATEWAY_STATE_CREATING`].
pub const GATEWAY_STATE_ACTIVE: &str = "ACTIVE";
/// See [`GATEWAY_STATE_CREATING`]. The transient state a gateway enters when
/// DeleteGateway accepts the delete; the clock settles it into
/// [`GATEWAY_STATE_DELETED`] one [`TRANSITION_SPAN`] later.
pub const GATEWAY_STATE_DELETING: &str = "DELETING";
/// See [`GATEWAY_STATE_CREATING`]. The TERMINAL state of a deleted gateway: its presence
/// in the `GatewayState` enum is the model's own statement that a deleted gateway stays a
/// readable resource, so a delete retires the record rather than evicting it (see
/// [`StoredGateway::deleted_at`]).
pub const GATEWAY_STATE_DELETED: &str = "DELETED";

/// The `InstanceState` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="InstanceState")`: REGISTERING | ACTIVE |
/// DEREGISTERING | DEREGISTERED | REGISTRATION_ERROR | DEREGISTRATION_ERROR).
///
/// The non-terminal state an instance is reported in while its registration is in flight;
/// the clock settles it into [`INSTANCE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later (see
/// [`ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`]).
pub const INSTANCE_STATE_REGISTERING: &str = "REGISTERING";
/// See [`INSTANCE_STATE_REGISTERING`]. A registered instance that is hosting bridges.
pub const INSTANCE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`INSTANCE_STATE_REGISTERING`]. The transient state an instance enters when
/// DeregisterGatewayInstance accepts the deregistration; the clock settles it into
/// [`INSTANCE_STATE_DEREGISTERED`] one [`TRANSITION_SPAN`] later.
pub const INSTANCE_STATE_DEREGISTERING: &str = "DEREGISTERING";
/// See [`INSTANCE_STATE_REGISTERING`]. The TERMINAL state of a deregistered instance: its
/// presence in the `InstanceState` enum is the model's own statement that a deregistered
/// instance stays a readable resource, so a deregistration retires the record rather than
/// evicting it — the same reasoning as [`GATEWAY_STATE_DELETED`].
pub const INSTANCE_STATE_DEREGISTERED: &str = "DEREGISTERED";
/// See [`INSTANCE_STATE_REGISTERING`]. A terminal failure of the appliance-side
/// registration, which reaches the service out of band (see
/// [`ComAmazonawsMediaconnectMediaconnectState::register_gateway_instance`]) and which no
/// modelled operation can cause.
pub const INSTANCE_STATE_REGISTRATION_ERROR: &str = "REGISTRATION_ERROR";
/// See [`INSTANCE_STATE_REGISTRATION_ERROR`], for a deregistration that failed.
pub const INSTANCE_STATE_DEREGISTRATION_ERROR: &str = "DEREGISTRATION_ERROR";

/// The `BridgePlacement` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="BridgePlacement")`: AVAILABLE | LOCKED).
/// `GatewayInstance$BridgePlacement`: "the availability of the instance to host new
/// bridges … If it is AVAILABLE, new bridges can be added to this instance" — the value a
/// newly registered instance carries, and the one UpdateGatewayInstance moves.
pub const BRIDGE_PLACEMENT_AVAILABLE: &str = "AVAILABLE";
/// See [`BRIDGE_PLACEMENT_AVAILABLE`]: "if it is LOCKED, no new bridges can be deployed to
/// this instance".
pub const BRIDGE_PLACEMENT_LOCKED: &str = "LOCKED";

/// The `ConnectionStatus` enum the model declares for a gateway instance
/// (`smithy_query(query="shape",shape="ConnectionStatus")`: CONNECTED | DISCONNECTED) —
/// "the connection state of the instance", i.e. whether the on-premises appliance is
/// talking to the service.
pub const CONNECTION_STATUS_CONNECTED: &str = "CONNECTED";
/// See [`CONNECTION_STATUS_CONNECTED`].
pub const CONNECTION_STATUS_DISCONNECTED: &str = "DISCONNECTED";

/// The `BridgeState` enum the model declares for a bridge
/// (`smithy_query(query="shape",shape="BridgeState")`: CREATING | STANDBY | STARTING |
/// DEPLOYING | ACTIVE | STOPPING | DELETING | DELETED | START_FAILED | START_PENDING |
/// STOP_FAILED | UPDATING). The two a bridge's creation reaches are named first; the
/// states a STARTED bridge moves through follow, because DeleteBridge's documented
/// precondition is stated against them (see [`is_stopped_bridge_state`]). UpdateBridgeState
/// is what moves a bridge between the settled ones (see [`bridge_transition_to`]).
pub const BRIDGE_STATE_CREATING: &str = "CREATING";
/// See [`BRIDGE_STATE_CREATING`]. A bridge that finished being created is not running
/// yet: it waits in `STANDBY` until UpdateBridgeState asks for a desired state
/// (`DesiredState`: ACTIVE | DELETED | STANDBY).
pub const BRIDGE_STATE_STANDBY: &str = "STANDBY";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_STARTING: &str = "STARTING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_START_PENDING: &str = "START_PENDING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_DEPLOYING: &str = "DEPLOYING";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_STOPPING: &str = "STOPPING";
/// See [`BRIDGE_STATE_CREATING`]. A bridge whose stop did not take effect is not a
/// stopped bridge.
pub const BRIDGE_STATE_STOP_FAILED: &str = "STOP_FAILED";
/// See [`BRIDGE_STATE_CREATING`]. A bridge whose start did not take effect never ran, so
/// it needs no stop.
pub const BRIDGE_STATE_START_FAILED: &str = "START_FAILED";
/// See [`BRIDGE_STATE_CREATING`].
pub const BRIDGE_STATE_UPDATING: &str = "UPDATING";
/// See [`BRIDGE_STATE_CREATING`]. The transient state a bridge enters when
/// UpdateBridgeState asks it to reach [`BRIDGE_STATE_DELETED`]; the clock settles it into
/// that state one [`TRANSITION_SPAN`] later.
pub const BRIDGE_STATE_DELETING: &str = "DELETING";
/// See [`BRIDGE_STATE_CREATING`]. The TERMINAL state of a bridge asked to reach the
/// `DELETED` member of `DesiredState`: its presence in the `BridgeState` enum is the
/// model's own statement that such a bridge stays a readable resource, exactly as
/// [`GATEWAY_STATE_DELETED`] is for a gateway, so UpdateBridgeState retires the record in
/// this state rather than evicting it. DeleteBridge is the other thing: it deletes the
/// RESOURCE and its answer carries only the ARN of the record it removed, so it drops the
/// record — and a bridge sitting in `DELETED` is stopped
/// ([`is_stopped_bridge_state`]), so that delete is admitted whenever a caller wants the
/// record gone.
pub const BRIDGE_STATE_DELETED: &str = "DELETED";

/// The pair of `BridgeState` values a bridge passes through to reach the `DesiredState`
/// `desired_state` names — the transient state it enters at once, and the settled state
/// the clock owes it one [`TRANSITION_SPAN`] later — or `None` when the value is not one
/// the `DesiredState` enum declares.
///
/// `DesiredState` (`smithy_query(query="shape",shape="DesiredState")`: ACTIVE | DELETED |
/// STANDBY) names the state a bridge is to END UP in, and each of its three values is
/// also a value of `BridgeState`, so the settled state is the desired state itself; the
/// transient one is the `BridgeState` value that reports that move being in progress
/// (STARTING toward ACTIVE, STOPPING toward STANDBY, DELETING toward DELETED). This is the
/// one mapping, so UpdateBridgeState cannot word a transition two ways.
pub fn bridge_transition_to(desired_state: &str) -> Option<(&'static str, &'static str)> {
    match desired_state {
        BRIDGE_STATE_ACTIVE => Some((BRIDGE_STATE_STARTING, BRIDGE_STATE_ACTIVE)),
        BRIDGE_STATE_STANDBY => Some((BRIDGE_STATE_STOPPING, BRIDGE_STATE_STANDBY)),
        BRIDGE_STATE_DELETED => Some((BRIDGE_STATE_DELETING, BRIDGE_STATE_DELETED)),
        _ => None,
    }
}

/// Whether a bridge in `bridge_state` is STOPPED — the precondition DeleteBridge
/// documents ("before you can delete a bridge, you must stop the bridge").
///
/// A bridge needs stopping exactly while it is started or moving to or from being
/// started: [`BRIDGE_STATE_STARTING`], [`BRIDGE_STATE_START_PENDING`],
/// [`BRIDGE_STATE_DEPLOYING`], [`BRIDGE_STATE_ACTIVE`], [`BRIDGE_STATE_STOPPING`],
/// [`BRIDGE_STATE_STOP_FAILED`] and [`BRIDGE_STATE_UPDATING`]. Every other state of the
/// modelled enum is one in which nothing is running — a bridge still being created, one
/// waiting in `STANDBY`, one whose start failed, or one already on its way out — so the
/// documented precondition is met.
pub fn is_stopped_bridge_state(bridge_state: &str) -> bool {
    !matches!(
        bridge_state,
        BRIDGE_STATE_STARTING
            | BRIDGE_STATE_START_PENDING
            | BRIDGE_STATE_DEPLOYING
            | BRIDGE_STATE_ACTIVE
            | BRIDGE_STATE_STOPPING
            | BRIDGE_STATE_STOP_FAILED
            | BRIDGE_STATE_UPDATING
    )
}

/// Whether `bridge_state` is a state a bridge reaches only once its deletion has been
/// accepted: [`BRIDGE_STATE_DELETING`] while that deletion is in flight, and the terminal
/// [`BRIDGE_STATE_DELETED`] the clock settles it in.
pub fn is_bridge_retirement_state(bridge_state: &str) -> bool {
    matches!(bridge_state, BRIDGE_STATE_DELETING | BRIDGE_STATE_DELETED)
}

/// Why the bridge `bridge` may no longer be CHANGED, or `None` while it still may be.
///
/// A bridge whose move to the `DELETED` member of `DesiredState` has been accepted is on
/// its way out: it reports [`BRIDGE_STATE_DELETING`] until the clock settles it in the
/// terminal [`BRIDGE_STATE_DELETED`], and from the instant that move is accepted the record
/// is RETIRED — kept readable, but no longer a bridge a later call can act on, because the
/// deletion it is settling into is irreversible. So every operation that WRITES to a stored
/// bridge — its own settings, its sources, its outputs, and the state it is asked for —
/// refuses once the bridge is in such a state or headed for one
/// ([`StoredBridge::settling_state`]), through this one function, with `ConflictException`
/// (HTTP 409): the request "would cause a conflict with the current state of a service
/// resource associated with the request". DeleteBridge is the one call a retired bridge
/// still admits, because it removes the record rather than changing it.
pub fn retired_bridge_rejection(bridge: &StoredBridge) -> Option<String> {
    let retired = is_bridge_retirement_state(&bridge.bridge_state)
        || is_bridge_retirement_state(bridge.settling_state());
    retired.then(|| {
        format!(
            "Bridge {} is {} and can no longer be changed.",
            bridge.bridge_arn, bridge.bridge_state
        )
    })
}

/// The `RouterInputState` enum the model declares for a router input
/// (`smithy_query(query="shape",shape="RouterInputState")`: CREATING | STANDBY | STARTING
/// | ACTIVE | STOPPING | DELETING | UPDATING | ERROR | RECOVERING | MIGRATING). Only the
/// states a router input's create, start, stop, restart, reconfiguration and delete reach
/// are named here; the rest belong to the router-input operations that are still
/// scaffolded (migrate).
///
/// The non-terminal state a router input is answered in while it is being created; the
/// clock settles it into [`ROUTER_INPUT_STATE_STANDBY`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_INPUT_STATE_CREATING: &str = "CREATING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. A router input that finished being created is not
/// running yet: it waits in `STANDBY` until StartRouterInput starts it, exactly as a
/// created bridge waits (see [`BRIDGE_STATE_STANDBY`]) — and it is where a STOPPED input
/// settles, because stopping an input is what returns it to not running (see
/// [`ROUTER_INPUT_STATE_STOPPING`]).
pub const ROUTER_INPUT_STATE_STANDBY: &str = "STANDBY";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while StartRouterInput's start is in flight; the clock settles it into
/// [`ROUTER_INPUT_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later.
pub const ROUTER_INPUT_STATE_STARTING: &str = "STARTING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The state of a router input that is running — what
/// StartRouterInput settles an input into, and the state StopRouterInput takes it out of.
pub const ROUTER_INPUT_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while a stop is in flight: StopRouterInput accepts the stop and the clock settles the
/// input into [`ROUTER_INPUT_STATE_STANDBY`] one [`TRANSITION_SPAN`] later, so a caller
/// reading the input straight after the stop still sees it stopping — the input-side
/// counterpart of [`ROUTER_OUTPUT_STATE_STOPPING`].
pub const ROUTER_INPUT_STATE_STOPPING: &str = "STOPPING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The transient state an input is RECONFIGURED in:
/// UpdateRouterInput answers `202 Accepted`, so the settings it accepted are not in effect
/// the instant it answers, and the clock settles the input back into the state it was
/// heading for one [`TRANSITION_SPAN`] later ([`StoredRouterInput::begin_transition`]) —
/// the input-side counterpart of [`ROUTER_OUTPUT_STATE_UPDATING`].
pub const ROUTER_INPUT_STATE_UPDATING: &str = "UPDATING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while a restart is in flight: RestartRouterInput "can be used to recover from errors
/// or refresh the input state"
/// (`smithy_query(query="doc",shape="RestartRouterInput")`), which is what this state
/// reports is happening.
pub const ROUTER_INPUT_STATE_RECOVERING: &str = "RECOVERING";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The state of a router input that is not carrying
/// content because something is wrong with it — the state RestartRouterInput recovers
/// from. Nothing in this emulator PUTS an input here (there is no data plane to fail), so
/// it is reached only by a caller seeding the store; a restart of such an input returns it
/// to [`ROUTER_INPUT_STATE_ACTIVE`].
pub const ROUTER_INPUT_STATE_ERROR: &str = "ERROR";
/// See [`ROUTER_INPUT_STATE_CREATING`]. The non-terminal state a router input is answered
/// in while its deletion is in progress.
///
/// There is no terminal `DELETED` value beside it — unlike `GatewayState`, which carries
/// one (see [`GATEWAY_STATE_DELETED`]) — so a deleted router input is a resource that
/// stops existing rather than one that stays readable: it is reported `DELETING` while the
/// deletion is in progress and is then gone (see [`StoredRouterInput::removal_due_at`]).
pub const ROUTER_INPUT_STATE_DELETING: &str = "DELETING";

/// `ListedBridge$BridgeType` ("the type of the bridge") for a cloud-to-ground bridge —
/// the kind `Bridge$EgressGatewayBridge` makes it ("an egress bridge is a cloud-to-ground
/// bridge"). The two bridge-type members are exclusive, so a bridge reports exactly one of
/// these two.
pub const BRIDGE_TYPE_EGRESS: &str = "egress";
/// See [`BRIDGE_TYPE_EGRESS`]: a ground-to-cloud bridge, the kind
/// `Bridge$IngressGatewayBridge` makes it ("an ingress bridge is a ground-to-cloud
/// bridge").
pub const BRIDGE_TYPE_INGRESS: &str = "ingress";

/// `SourceType` (`smithy_query(query="shape",shape="SourceType")`): a source that
/// originated in this account is `OWNED`; one reached through another account's
/// entitlement is `ENTITLED`.
pub const SOURCE_TYPE_OWNED: &str = "OWNED";
/// See [`SOURCE_TYPE_OWNED`].
pub const SOURCE_TYPE_ENTITLED: &str = "ENTITLED";

/// `OutputStatus` / `EntitlementStatus` enum value the model documents as the default
/// MediaConnect applies when the request omits the member: "If you don't specify the
/// outputStatus field in your request, MediaConnect sets it to ENABLED"
/// (`AddOutputRequest$OutputStatus`), and the same sentence on
/// `GrantEntitlementRequest$EntitlementStatus`.
pub const STATUS_ENABLED: &str = "ENABLED";

/// The `State` enum (`smithy_query(query="shape",shape="State")`: ENABLED | DISABLED) —
/// the two values every on/off setting of this service targets, from a flow's failover
/// state to a source's or an output's router integration state. Named here once so the
/// operations that read a state and the projections that report one cannot drift apart.
pub const STATE_ENABLED: &str = "ENABLED";
/// See [`STATE_ENABLED`].
pub const STATE_DISABLED: &str = "DISABLED";

/// The `RouterIntegrationState` a source or an output reports, given the state the
/// request that configured it carried (`None` when it carried none).
///
/// `Source$RouterIntegrationState` ("indicates if router integration is enabled or
/// disabled on the flow source") and `Output$RouterIntegrationState` both target the
/// `State` enum and are reported on every source and every output, so a record whose
/// request never mentioned router integration still reports a state: the disabled one,
/// because router integration is off until a request asks for it. Every projection of a
/// source or an output resolves it HERE, so one stored record always reports one state.
pub fn reported_router_integration_state(configured: Option<&str>) -> Option<String> {
    Some(configured.unwrap_or(STATE_DISABLED).to_string())
}

/// How long a flow stays in a transient status (`STARTING`, `STOPPING`) before the
/// clock settles it into the steady one.
///
/// AWS documents no duration for this transition, so the span is this emulator's own
/// choice inside the project's 250-750 ms window: non-zero, so a caller that polls
/// immediately after StartFlow still observes `STARTING`, and short enough that a test
/// can wait it out.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`): the third
/// segment of every ARN the service mints.
pub const SERVICE_NAMESPACE: &str = "mediaconnect";

/// The prefix MediaConnect puts in front of the identifier a request named when it
/// resolves no resource: the whole `NotFoundException$Message` is this prefix followed by
/// that identifier, and nothing else.
///
/// `NotFoundException` models `Message` as **required** and the model states the precise
/// meaning of the failure "is documented in the error message", so the message is part of
/// what a client reads — a caller that parses the ARN back out of a not-found message
/// reads it from here. Every `NotFoundException` this crate returns is built through
/// [`not_found_message`] so no handler can word one its own way.
pub const NOT_FOUND_PREFIX: &str = "Not found: ";

/// The prefix MediaConnect puts in front of the detail of a refused request, followed by a
/// newline: `BadRequestException$Message` is `Validation failed:` then a line stating what
/// about the request was unacceptable.
///
/// Operators and clients tell a validation refusal from a semantic one by this prefix, so
/// every `BadRequestException` this crate returns ABOUT THE REQUEST'S CONTENT is built
/// through [`validation_failed_message`] — including the ones the framework's own boundary
/// walk raises, which `crate::service::EmulatedService::override_refusal` words through the
/// same function. A request refused by the addressed resource's STATE instead carries the
/// other envelope, [`BAD_REQUEST_PREFIX`].
pub const VALIDATION_FAILED_PREFIX: &str = "Validation failed:";

/// The prefix MediaConnect puts in front of the detail of a request refused for a
/// SEMANTIC reason — the request is well formed, but the resource it addresses is not in a
/// state that admits it: `BadRequestException$Message` is `Bad request: ` then the
/// sentence naming the precondition.
///
/// `BadRequestException` is "thrown if the request contains a semantic error. The precise
/// meaning depends on the API, and is documented in the error message", so the two kinds
/// of refusal this service answers 400 for read differently: a request whose CONTENT is
/// unacceptable carries [`VALIDATION_FAILED_PREFIX`], and one refused by the addressed
/// resource's STATE carries this prefix. The status and the error code are the contract;
/// the sentence after the envelope is this emulator's wording of the documented
/// precondition.
pub const BAD_REQUEST_PREFIX: &str = "Bad request: ";

/// MediaConnect's `NotFoundException` message for a request that named `identifier` — an
/// ARN, or whatever else the request addressed the resource by — and resolved no resource.
///
/// The message is derived from the REQUEST, never from the store, so it is the same
/// whether the resource never existed, was deleted, or was never an ARN at all.
///
/// TOTAL for every `identifier` a request can carry: one `format!` of a borrowed `&str`,
/// with no parsing, no indexing or slicing, no `unwrap`/`expect` and no arithmetic — so a
/// label that satisfies no pattern (the flow labels deliberately hand their value to the
/// handler unjudged; see the note on `DescribeFlowInput` in `crate::ops::types`) cannot
/// panic here, whatever bytes it holds. The identifier is a URI path label, so what it
/// adds to the response is bounded by the request line that carried it.
pub fn not_found_message(identifier: &str) -> String {
    format!("{NOT_FOUND_PREFIX}{identifier}")
}

/// MediaConnect's `BadRequestException` message for a request refused as unacceptable:
/// [`VALIDATION_FAILED_PREFIX`], a newline, then `detail`.
///
/// `detail` states what was wrong — the member and the rule it broke. The envelope is the
/// contract; the wording of the detail is this emulator's.
///
/// TOTAL for every `detail`, for the same reason as [`not_found_message`]: one `format!`
/// of a borrowed `&str`.
pub fn validation_failed_message(detail: &str) -> String {
    format!("{VALIDATION_FAILED_PREFIX}\n{detail}")
}

/// MediaConnect's `BadRequestException` message for a well-formed request the addressed
/// resource's STATE refuses: [`BAD_REQUEST_PREFIX`] then `detail`.
///
/// TOTAL for every `detail`, for the same reason as [`validation_failed_message`].
pub fn bad_request_message(detail: &str) -> String {
    format!("{BAD_REQUEST_PREFIX}{detail}")
}

/// The refusal message for a lifecycle precondition an operation documents: `resource`
/// (`Flow`, `Gateway`, `Bridge`, …) may be addressed by `operation` (the verb as the API
/// names it) only while it is in `required_state`.
///
/// Every operation whose documentation states such a precondition words its refusal HERE,
/// so one rule cannot be worded two ways: the operation names the state its documentation
/// requires and this builds the one envelope a client reads
/// ([`bad_request_message`]).
pub fn state_precondition_message(resource: &str, required_state: &str, operation: &str) -> String {
    bad_request_message(&format!(
        "{resource} must be in {required_state} in order for {operation} to be invoked."
    ))
}

/// The ceiling on how many flows this store holds: the cap that bounds caller-driven
/// growth of the flow map.
///
/// This emulator keeps its resources in memory for the life of the process and
/// CreateFlow is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// DescribeFlow must still answer what CreateFlow created. CreateFlow refuses past this
/// cap with the service's own modelled `CreateFlow420Exception`, "raised by Elemental
/// MediaConnect when creating the flow" and declared on CreateFlow alone (never on a
/// read). The number is the emulator's own bound, not a published AWS default — AWS
/// publishes the flows-per-Region quota as adjustable, so no single value is the
/// contract — and it is high enough that no ordinary workflow meets it.
pub const MAX_FLOWS: usize = 5_000;

/// The ceiling on how many entries one CreateFlow request may put in any one of a
/// flow's sub-collections (sources, outputs, entitlements, media streams, VPC
/// interfaces, tags), so a single request cannot store an arbitrarily large record.
///
/// For outputs and entitlements this is the limit the operation itself documents: "The
/// request optionally can include outputs (up to 50) and entitlements (up to 50)"
/// (`smithy_query(query="operation",operation="CreateFlow")`), and it is also the
/// tags-per-resource limit AWS applies service-wide. AWS documents no count for the
/// remaining collections, so the same number is applied to them as the emulator's own
/// bound. CreateFlow refuses past it with `BadRequestException`, which the model
/// describes as the error for "a semantic error" in the request.
pub const MAX_FLOW_COLLECTION_ENTRIES: usize = 50;

/// The ceiling on how many gateways this store holds, for the same reason as
/// [`MAX_FLOWS`]: nothing evicts a gateway and DescribeGateway must still answer what
/// CreateGateway created, so the store needs a maximum entry count. CreateGateway refuses
/// past it with the service's own modelled `CreateGateway420Exception`, "raised by
/// Elemental MediaConnect when creating the gateway" and declared on CreateGateway alone.
/// The number is the emulator's own bound, not a published AWS default, and it is high
/// enough that no ordinary workflow meets it.
pub const MAX_GATEWAYS: usize = 1_000;

/// How many RETIRED (deleted) gateway records this store keeps beside its live ones.
///
/// A gateway delete is a soft delete, so a deleted gateway stays readable in
/// [`GATEWAY_STATE_DELETED`] (see [`StoredGateway::deleted_at`]) — which, left unbounded,
/// would let a create-then-delete loop grow the gateway map without limit. So the store
/// retains the most recently deleted gateways up to this count and drops the oldest
/// retired record past it, exactly as [`MAX_GATEWAYS`] bounds the live ones. The number is
/// this emulator's own bound (AWS publishes no retention for a deleted gateway) and it is
/// high enough that a client polling a delete it just made always finds its record.
pub const MAX_RETIRED_GATEWAYS: usize = MAX_GATEWAYS;

/// How many networks one gateway may carry: "Creates a new gateway. The request must
/// include at least one network (up to four)"
/// (`smithy_query(query="operation",operation="CreateGateway")`). CreateGateway refuses a
/// request outside this range with `BadRequestException`, the error the model describes
/// as the one for "a semantic error" in the request.
pub const MAX_GATEWAY_NETWORKS: usize = 4;

/// The most egress CIDR blocks one gateway holds, so one request cannot store a record of
/// caller-chosen size in a store nothing evicts (and that DescribeGateway echoes back
/// whole).
///
/// AWS publishes no count for `EgressCidrBlocks` and the model puts no `@length` on it, so
/// the bound is the one this crate already applies to a request-sized collection
/// ([`MAX_FLOW_COLLECTION_ENTRIES`], CreateFlow's per-request bound), named through that
/// constant so the two cannot drift. CreateGateway refuses past it with
/// `BadRequestException`, the error the model describes as the one for "a semantic error"
/// in the request.
pub const MAX_GATEWAY_EGRESS_CIDR_BLOCKS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many bridges this store holds, for the same reason as
/// [`MAX_GATEWAYS`]: nothing evicts a bridge and DescribeBridge must still answer what
/// CreateBridge created, so the store needs a maximum entry count. CreateBridge refuses
/// past it with the service's own modelled `CreateBridge420Exception`, "raised by
/// Elemental MediaConnect when creating the bridge" and declared on CreateBridge alone
/// (never on a read). The number is the emulator's own bound, not a published AWS
/// default, and it is high enough that no ordinary workflow meets it.
pub const MAX_BRIDGES: usize = 1_000;

/// The ceiling on how many router inputs this store holds, for the same reason as
/// [`MAX_GATEWAYS`]: nothing evicts a router input and GetRouterInput must still answer
/// what CreateRouterInput created, so the store needs a maximum entry count.
/// CreateRouterInput refuses past it with the service's own modelled
/// `RouterInputServiceQuotaExceededException`, the 420 error declared on that create alone
/// (never on a read). The number is the emulator's own bound, not a published AWS default,
/// and it is high enough that no ordinary workflow meets it.
pub const MAX_ROUTER_INPUTS: usize = 1_000;

/// The ceiling on how many sources ONE BRIDGE holds while its source failover is ACTIVE,
/// however many calls fill it.
///
/// A bridge's sources are governed by the same source failover setting a flow's are:
/// `CreateBridgeRequest$SourceFailoverConfig` targets the very `FailoverConfig` shape whose
/// `State` publishes the count — "if the state is inactive, the flow can have only one
/// source. If the state is active, the flow can have one or two sources" — and UpdateBridge
/// merges the same `UpdateFailoverConfig` into it. So a bridge ingests a single source or a
/// failover pair rather than a request-sized collection, and the count is named through
/// [`MAX_FLOW_SOURCES`] so the two resources this one rule governs cannot drift apart. See
/// [`max_bridge_sources`], the one place that resolves the bound, and
/// [`MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE`] for the other half.
///
/// The bound is on the STORED bridge, so a sequence of adds cannot grow one past what a
/// single create could store. Neither CreateBridge nor AddBridgeSources models a count
/// exception for it, so both refuse past the bound with `BadRequestException`, the error the
/// model describes for "a semantic error" in the request.
pub const MAX_BRIDGE_SOURCES: usize = MAX_FLOW_SOURCES;

/// The ceiling on how many sources ONE BRIDGE holds while its source failover is INACTIVE:
/// "if the state is inactive, [it] can have only one source" (`FailoverConfig$State`). See
/// [`MAX_BRIDGE_SOURCES`] and [`max_bridge_sources`].
pub const MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE: usize = MAX_FLOW_SOURCES_FAILOVER_INACTIVE;

/// The ceiling on how many outputs one bridge carries.
///
/// AWS publishes no count for a bridge's outputs and the model puts no `@length` on
/// `__listOfAddBridgeOutputRequest`, so the bound is the one this crate already applies to
/// a request-sized collection ([`MAX_FLOW_COLLECTION_ENTRIES`]), named through that
/// constant so the two cannot drift. It is the only count a cloud-to-ground bridge's
/// outputs are bounded by, its `EgressGatewayBridge` declaring none of its own (see
/// [`bridge_output_capacity`]). AddBridgeOutputs refuses past it with
/// `BadRequestException`, the error the model describes as the one for "a semantic error"
/// in the request.
pub const MAX_BRIDGE_OUTPUTS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// How many outputs a bridge may hold, read off the bridge's own type.
///
/// `ingress_max_outputs` is `Some` for a ground-to-cloud bridge, carrying the value of its
/// REQUIRED `IngressGatewayBridge$MaxOutputs` — "the maximum number of outputs on the
/// ingress bridge" — and `None` for a cloud-to-ground bridge, whose `EgressGatewayBridge`
/// declares no output count at all, so the only bound on it is the one this crate applies
/// to every bridge ([`MAX_BRIDGE_OUTPUTS`]).
///
/// A declared maximum is never read as room for more than the store itself holds, and a
/// non-positive declaration is room for nothing: the member is an `Integer`, so the model
/// alone does not keep a negative one out.
///
/// What a ground-to-cloud bridge declares bounds the outputs it can ever hold — the flow
/// outputs the service materialises for it — and not the ones a request configures, of
/// which it takes none at all (see [`request_bridge_output_rejection`], which refuses
/// those by the bridge's direction before it counts anything).
pub fn bridge_output_capacity(ingress_max_outputs: Option<i32>) -> usize {
    match ingress_max_outputs {
        Some(declared) => usize::try_from(declared)
            .unwrap_or(0)
            .min(MAX_BRIDGE_OUTPUTS),
        None => MAX_BRIDGE_OUTPUTS,
    }
}

/// Why a bridge whose own type declares `ingress_max_outputs` cannot end up holding
/// `held + adding` outputs, or `None` when it can — the count half of the rule, worded
/// once so every path that decides how many outputs a bridge holds states the same bound
/// for the same reason.
///
/// `adding` is zero for a request that moves the BOUND rather than the output count (an
/// UpdateBridge lowering `IngressGatewayBridge$MaxOutputs` under the outputs the bridge
/// already holds), exactly as the source-count rule reads a request that only moves the
/// failover state ([`source_failover_bound_rejection`]).
///
/// Both kinds of output count against the one capacity, because the capacity is on the
/// BRIDGE: the network outputs a request configures ([`request_bridge_output_rejection`])
/// and the flow outputs the service materialises for the flow sources that name the bridge
/// ([`ComAmazonawsMediaconnectMediaconnectState::flow_source_bridge_output_rejection`]).
pub fn bridge_output_count_rejection(
    ingress_max_outputs: Option<i32>,
    held: usize,
    adding: usize,
) -> Option<String> {
    let capacity = bridge_output_capacity(ingress_max_outputs);
    let total = held + adding;
    if total <= capacity {
        return None;
    }
    Some(format!(
        "A bridge holds at most {capacity} output(s); this request would leave it with \
         {total} ({held} already stored, {adding} requested)."
    ))
}

/// Why a bridge cannot take `adding` more outputs a REQUEST configures on top of the
/// `held` it already holds, or `None` when it can. `ingress_max_outputs` is the bridge's
/// own type, as [`bridge_output_capacity`] reads it.
///
/// Two rules, both about the direction a bridge carries content in, decided in that
/// order — which bridge takes a request-configured output at all, and then how many it
/// may hold:
///
/// * an output a request configures is a `NetworkOutput` — the only member
///   `AddBridgeOutputRequest` has, "delivered to your premises" — which is what a
///   cloud-to-ground bridge delivers ("the content comes from an existing MediaConnect
///   flow and is delivered to your premises"). A ground-to-cloud bridge delivers the
///   other way ("the content originates at your premises and is delivered to the cloud"),
///   so its outputs are the `BridgeFlowOutput`s the service itself materialises when a
///   cloud flow takes the bridge as its source (`SetGatewayBridgeSourceRequest`), and
///   none a request carries; and
/// * a bridge that does take them holds no more outputs than its own type declares
///   ([`bridge_output_count_rejection`], which reads the capacity off
///   [`bridge_output_capacity`]), which for a cloud-to-ground bridge is the bound this
///   crate applies to every bridge, its `EgressGatewayBridge` declaring no output count.
///
/// A request that configures no output at all admits nothing, so it is nothing this
/// decides: the operations refuse an empty required list in their own words.
///
/// Every operation that puts request-configured outputs on a bridge (CreateBridge's
/// `Outputs`, AddBridgeOutputs) refuses through this one function, with
/// `BadRequestException` — the error the model describes as the one for "a semantic
/// error" in the request.
pub fn request_bridge_output_rejection(
    ingress_max_outputs: Option<i32>,
    held: usize,
    adding: usize,
) -> Option<String> {
    if adding == 0 {
        return None;
    }
    if ingress_max_outputs.is_some() {
        return Some(
            "An output a request configures is a network output, which is delivered to the \
             premises network — what a cloud-to-ground bridge delivers. This bridge is a \
             ground-to-cloud bridge: its outputs are the flow outputs created for it when a \
             flow takes the bridge as its source, so it accepts no output in a request."
                .to_string(),
        );
    }
    bridge_output_count_rejection(ingress_max_outputs, held, adding)
}

/// How many sources a bridge whose source failover config is `config` may hold:
/// [`MAX_BRIDGE_SOURCES`] while failover is active, and
/// [`MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE`] while it is not — read off the bridge's own
/// failover setting, exactly as a flow's bound is read off its own ([`max_flow_sources`]).
///
/// Every operation that decides a bridge's source count reads the bound HERE —
/// CreateBridge's `Sources`, AddBridgeSources' append and UpdateBridge's
/// `SourceFailoverConfig` (which moves the bound under the sources the bridge already
/// holds) — so the add side, the create side and the failover side cannot drift apart.
pub fn max_bridge_sources(config: Option<&FailoverConfig>) -> usize {
    if source_failover_is_active(config) {
        MAX_BRIDGE_SOURCES
    } else {
        MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE
    }
}

/// The detail a request is refused with when it would leave a bridge holding more sources
/// than its failover state allows: `held` sources already stored, `adding` more requested
/// (zero on a request that only moves the failover state), under `config`.
pub fn bridge_source_bound_rejection(
    config: Option<&FailoverConfig>,
    held: usize,
    adding: usize,
) -> Option<String> {
    source_failover_bound_rejection("bridge", max_bridge_sources(config), config, held, adding)
}

/// Why a bridge whose type is `bridge_type` ([`BRIDGE_TYPE_EGRESS`] /
/// [`BRIDGE_TYPE_INGRESS`]) does not ingest `source`, or `None` when it does.
///
/// A bridge's DIRECTION decides the kind of source it ingests. A cloud-to-ground (egress)
/// bridge's "content comes from an existing MediaConnect flow"
/// (`AddEgressGatewayBridgeRequest`), which is what a `BridgeFlowSource` is — "a flow
/// source originates in MediaConnect as an existing cloud flow". A ground-to-cloud
/// (ingress) bridge's "content originates at your premises"
/// (`AddIngressGatewayBridgeRequest`), which is what a `BridgeNetworkSource` is — "a
/// network source originates at your premises". So one bridge never mixes the two kinds,
/// and a source whose kind contradicts the bridge's direction describes content that
/// bridge does not carry.
///
/// Every operation that puts a source on a bridge (CreateBridge's `Sources`,
/// AddBridgeSources) refuses through this one function, with `BadRequestException` — the
/// error the model describes as the one for "a semantic error" in the request. The same
/// rule is why UpdateBridgeSource takes only the wrapper the addressed source already is.
pub fn bridge_source_kind_rejection(bridge_type: &str, source: &BridgeSource) -> Option<String> {
    let name = bridge_source_name(source);
    match (bridge_type, source.flow_source.is_some()) {
        (BRIDGE_TYPE_EGRESS, false) => Some(format!(
            "A cloud-to-ground (egress) bridge takes its content from an existing \
             MediaConnect flow, so its sources are flow sources. Source {name:?} is a \
             network source, which originates at your premises."
        )),
        (BRIDGE_TYPE_INGRESS, true) => Some(format!(
            "A ground-to-cloud (ingress) bridge takes content that originates at your \
             premises, so its sources are network sources. Source {name:?} is a flow \
             source, which originates in MediaConnect as an existing cloud flow."
        )),
        _ => None,
    }
}

/// The most tags one resource carries, checked against the MERGED set a tag write would
/// leave on the resource, so repeated tag writes cannot grow a resource's tag map past
/// what a single create could set.
///
/// The number is [`MAX_FLOW_COLLECTION_ENTRIES`] — the bound CreateFlow applies to one
/// request's `FlowTags`, and the tags-per-resource limit AWS applies service-wide — named
/// once so the create side and the tagging side cannot drift apart. TagResource refuses
/// past it with `BadRequestException`, which the model describes as the error for "a
/// semantic error" in the request.
pub const MAX_TAGS_PER_RESOURCE: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The longest tag key this service stores: "Tag keys can have a maximum character length
/// of 128 characters" (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`).
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service stores: "tag values can have a maximum length of 256
/// characters" (same member doc as [`MAX_TAG_KEY_LENGTH`]).
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Why `key` / `value` is not a tag pair this service stores, or `None` when it is.
///
/// The model declares `__mapOfString` without length bounds, so the boundary validator
/// passes a pair the service itself would refuse: the lengths come from the member's own
/// documentation ([`MAX_TAG_KEY_LENGTH`], [`MAX_TAG_VALUE_LENGTH`]) and a tag is addressed
/// by its key, so an empty key names no tag. Both bounds count CHARACTERS, as the
/// documentation does. Every operation that writes tags (CreateFlow's `FlowTags`, the
/// regional and the global TagResource) refuses through this one function.
pub fn tag_pair_rejection(key: &str, value: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key must not be empty.".to_string());
    }
    let key_length = key.chars().count();
    let value_length = value.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tag key {key:?} is {key_length} characters long; a tag key may have at most \
             {MAX_TAG_KEY_LENGTH} characters."
        ));
    }
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "The value of tag {key:?} is {value_length} characters long; a tag value may have \
             at most {MAX_TAG_VALUE_LENGTH} characters."
        ));
    }
    None
}

/// Why the pairs of one tag write are not pairs this service stores, or `None` when every
/// one of them is: [`tag_pair_rejection`] over the whole request, answered for the FIRST
/// unacceptable pair.
///
/// Every tag write checks its whole request through this before it resolves or writes
/// anything, so a refused write stores none of the pairs it carried.
pub fn tag_pairs_rejection<'a, I>(pairs: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    pairs
        .into_iter()
        .find_map(|(key, value)| tag_pair_rejection(key, value))
}

/// Why MERGING the keys `keys` into the tag map `tags` would leave the resource
/// `resource_arn` carrying more tags than one resource may, or `None` when it would not.
///
/// The bound ([`MAX_TAGS_PER_RESOURCE`]) applies to the MERGED set the write would leave,
/// so a key the request re-states does not grow it. Every tag write checks it before the
/// first insert, so a refused write changes nothing.
pub fn tag_capacity_rejection<'a, I>(
    tags: &BTreeMap<String, String>,
    keys: I,
    resource_arn: &str,
) -> Option<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let merged_entries =
        tags.len() + keys.into_iter().filter(|key| !tags.contains_key(*key)).count();
    (merged_entries > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "A resource keeps at most {MAX_TAGS_PER_RESOURCE} tags; tagging {resource_arn} \
             with these keys would give it {merged_entries}."
        )
    })
}

/// MERGE `pairs` into `tags`: a key `pairs` names is set or overwritten IN PLACE, and a key
/// it does not name keeps the value it had ("if existing tags on a resource are not
/// specified in the request parameters, they are not changed").
///
/// Answers whether the map CHANGED, which is what decides whether the record's `UpdatedAt`
/// moves: a write that re-states the values a resource already carries changes nothing.
#[must_use]
pub fn merge_tags(
    tags: &mut BTreeMap<String, String>,
    pairs: impl IntoIterator<Item = (String, String)>,
) -> bool {
    let mut changed = false;
    for (key, value) in pairs {
        if tags.get(&key).is_none_or(|stored| *stored != value) {
            changed = true;
        }
        tags.insert(key, value);
    }
    changed
}

/// The tag map a resource is created with, from the tag member of the request that created
/// it (`FlowTags`, `OutputTags`, `SourceTags`, `EntitlementTags`, `MediaStreamTags`): the
/// pairs the member carries, ordered, and EMPTY when the member is absent — a resource
/// created without tags carries no tags, which is not the same as carrying none yet known.
pub fn tag_map_of(
    tags: Option<&std::collections::HashMap<String, String>>,
) -> BTreeMap<String, String> {
    tags.into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Why the tag member `tags` of a create request is not a tag map this service stores on
/// `resource`, or `None` when it is.
///
/// Every tag member of a create request writes the same kind of tag map TagResource writes
/// on the same resource, so it is bounded by the same two rules, read from the same place:
/// each pair through [`tag_pair_rejection`], and the count through
/// [`MAX_TAGS_PER_RESOURCE`] — a create cannot leave a resource carrying more tags than a
/// tag write could put on it. The caller refuses past either with its own
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub fn tag_set_rejection(
    resource: &str,
    tags: Option<&std::collections::HashMap<String, String>>,
) -> Option<String> {
    let pairs = tags?;
    if pairs.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "A resource keeps at most {MAX_TAGS_PER_RESOURCE} tags; this request would give \
             {resource} {}.",
            pairs.len()
        ));
    }
    pairs
        .iter()
        .find_map(|(key, value)| tag_pair_rejection(key, value))
}

/// The ceiling on how many outputs ONE FLOW holds, however many calls fill it: "You can
/// create up to 50 outputs per flow"
/// (`smithy_query(query="operation",operation="AddFlowOutputs")`), the one count AWS
/// publishes for a flow's outputs. It happens to equal
/// [`MAX_FLOW_COLLECTION_ENTRIES`] — CreateFlow's per-request bound — but it is a
/// different rule: this one bounds the STORED flow, so a sequence of adds cannot grow
/// one past it. AddFlowOutputs refuses past it with its own modelled
/// `AddFlowOutputs420Exception`, "raised by Elemental MediaConnect when adding the flow
/// output".
pub const MAX_FLOW_OUTPUTS: usize = 50;

/// The ceiling on how many entitlements ONE FLOW holds, however many calls fill it: "the
/// request optionally can include outputs (up to 50) and entitlements (up to 50)"
/// (`smithy_query(query="operation",operation="CreateFlow")`), the one count AWS
/// publishes for a flow's entitlements. Like [`MAX_FLOW_OUTPUTS`] it bounds the STORED
/// flow, so a sequence of grants cannot grow one past it. GrantFlowEntitlements refuses
/// past it with its own modelled `GrantFlowEntitlements420Exception`, "raised by Elemental
/// MediaConnect when granting the entitlement".
pub const MAX_FLOW_ENTITLEMENTS: usize = 50;

/// The ceiling on how many media streams ONE FLOW holds, however many calls fill it.
///
/// AWS publishes no count for a flow's media streams, so the number is
/// [`MAX_FLOW_COLLECTION_ENTRIES`] — the bound CreateFlow applies to one request's
/// `MediaStreams` — named once so the create side and the add side cannot drift apart.
/// Like [`MAX_FLOW_OUTPUTS`] it is a different RULE from that per-request bound: this one
/// bounds the STORED flow, so a sequence of adds cannot grow one past what a single
/// create could store. AddFlowMediaStreams models no 420, so it refuses past this bound
/// with `BadRequestException`, the error the model describes for "a semantic error" in
/// the request.
pub const MAX_FLOW_MEDIA_STREAMS: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many VPC interfaces ONE FLOW holds, however many calls fill it.
///
/// AWS publishes no count for a flow's VPC interfaces and the model puts no `@length` on
/// `__listOfVpcInterfaceRequest`, so the number is [`MAX_FLOW_COLLECTION_ENTRIES`] — the
/// bound CreateFlow applies to one request's `VpcInterfaces` — named once so the create
/// side and the add side cannot drift apart. Like [`MAX_FLOW_MEDIA_STREAMS`] it is a
/// different RULE from that per-request bound: this one bounds the STORED flow, so a
/// sequence of adds cannot grow one past what a single create could store.
/// AddFlowVpcInterfaces models no count exception, so it refuses past this bound with
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub const MAX_FLOW_VPC_INTERFACES: usize = MAX_FLOW_COLLECTION_ENTRIES;

/// The ceiling on how many sources ONE FLOW holds while its source failover is ACTIVE,
/// however many calls fill it.
///
/// `FailoverConfig$State` (`smithy_query(query="doc",shape="FailoverConfig",member="State")`)
/// publishes the count: "The state of source failover on the flow. If the state is
/// inactive, the flow can have only one source. If the state is active, the flow can have
/// one or two sources." So a flow's sources are bounded by its own failover state, not by
/// a generic collection bound — see [`max_flow_sources`], the one place that resolves the
/// bound, and [`MAX_FLOW_SOURCES_FAILOVER_INACTIVE`] for the other half.
///
/// Like [`MAX_FLOW_MEDIA_STREAMS`] this bounds the STORED flow, so a sequence of adds
/// cannot grow one past what a single create could store. Neither CreateFlow nor
/// AddFlowSources models a count exception for it, so both refuse past the bound with
/// `BadRequestException`, the error the model describes for "a semantic error" in the
/// request.
pub const MAX_FLOW_SOURCES: usize = 2;

/// The ceiling on how many sources ONE FLOW holds while its source failover is INACTIVE:
/// "if the state is inactive, the flow can have only one source" (`FailoverConfig$State`).
/// See [`MAX_FLOW_SOURCES`] and [`max_flow_sources`].
pub const MAX_FLOW_SOURCES_FAILOVER_INACTIVE: usize = 1;

/// Whether source failover is ACTIVE on a resource holding `config`.
///
/// `FailoverConfig$State` targets the `State` enum (ENABLED | DISABLED), so failover is
/// active exactly while that state is [`STATE_ENABLED`]: a resource that holds no failover
/// config at all, and one whose config leaves `State` unset, has failover inactive —
/// a setting nothing turned on is off, the same reading
/// [`reported_router_integration_state`] applies to the other members of this enum.
pub fn source_failover_is_active(config: Option<&FailoverConfig>) -> bool {
    config.and_then(|config| config.state.as_deref()) == Some(STATE_ENABLED)
}

/// How many sources a flow whose source failover config is `config` may hold:
/// [`MAX_FLOW_SOURCES`] while failover is active, and
/// [`MAX_FLOW_SOURCES_FAILOVER_INACTIVE`] while it is not.
///
/// Every operation that decides a flow's source count reads the bound HERE — CreateFlow's
/// `Sources`/`Source`, AddFlowSources' append and UpdateFlow's `SourceFailoverConfig`
/// (which moves the bound under the sources the flow already holds) — so the add side, the
/// create side and the failover side cannot drift apart.
pub fn max_flow_sources(config: Option<&FailoverConfig>) -> usize {
    if source_failover_is_active(config) {
        MAX_FLOW_SOURCES
    } else {
        MAX_FLOW_SOURCES_FAILOVER_INACTIVE
    }
}

/// The detail a request is refused with when it would leave a flow holding more sources
/// than its failover state allows: `held` sources already stored, `adding` more requested
/// (zero on a request that only moves the failover state), under `config`.
///
/// Both halves of the rule are worded here once, so the create path, the add path and
/// UpdateFlow state the same bound for the same reason.
pub fn flow_source_bound_rejection(
    config: Option<&FailoverConfig>,
    held: usize,
    adding: usize,
) -> Option<String> {
    source_failover_bound_rejection("flow", max_flow_sources(config), config, held, adding)
}

/// The detail a request is refused with when it would leave `resource` — a flow or a
/// bridge, the two resources whose sources one `SourceFailoverConfig` governs — holding
/// more sources than `bound`, the count its failover state allows
/// ([`max_flow_sources`] / [`max_bridge_sources`]).
///
/// One wording for one rule, so the create path, the add path and the update path of both
/// resources state the same bound for the same reason.
fn source_failover_bound_rejection(
    resource: &str,
    bound: usize,
    config: Option<&FailoverConfig>,
    held: usize,
    adding: usize,
) -> Option<String> {
    let total = held + adding;
    if total <= bound {
        return None;
    }
    let state = if source_failover_is_active(config) {
        format!("{STATE_ENABLED} (active)")
    } else {
        match config.and_then(|config| config.state.as_deref()) {
            Some(state) => format!("{state} (inactive)"),
            None => "unset (inactive)".to_string(),
        }
    };
    Some(format!(
        "A {resource} whose source failover state is {state} can hold at most {bound} \
         source(s); this request would leave it with {total} ({held} already stored, \
         {adding} requested). If the state is inactive, the {resource} can have only one \
         source; if the state is active, the {resource} can have one or two sources."
    ))
}

/// The `FlowSize` values (`smithy_query(query="shape",shape="FlowSize")`: MEDIUM | LARGE |
/// LARGE_4X) whose "processing capacity and feature set" carries NDI®.
///
/// `CreateFlowRequest$FlowSize` is "set this optional parameter to `LARGE` if you want to
/// enable NDI sources or outputs on the flow", and UpdateFlow's own documentation says "if
/// you have a MEDIUM flow and you want to change the flow source to NDI: first, use the
/// UpdateFlow operation to upgrade the flow size to LARGE". The capable sizes are therefore
/// an ALLOW-list — `LARGE` and the larger `LARGE_4X` — so a flow whose size was never set
/// is not NDI-capable either. Named ONCE here because every operation that can put NDI on a
/// flow reads it (CreateFlow, AddFlowSources, AddFlowOutputs, UpdateFlowSource,
/// UpdateFlowOutput, UpdateFlow).
pub const NDI_CAPABLE_FLOW_SIZES: [&str; 2] = ["LARGE", "LARGE_4X"];

/// The `Protocol` value that names the NDI® transport
/// (`smithy_query(query="shape",shape="Protocol")`: … | ndi-speed-hq). A source or an
/// output whose transport uses it is an NDI source or output.
pub const PROTOCOL_NDI_SPEED_HQ: &str = "ndi-speed-hq";

/// Whether `flow_size` is one of [`NDI_CAPABLE_FLOW_SIZES`]. A flow whose size was never
/// set is NOT: the capable sizes are the ones the documentation names.
pub fn flow_size_supports_ndi(flow_size: Option<&str>) -> bool {
    flow_size.is_some_and(|size| NDI_CAPABLE_FLOW_SIZES.contains(&size))
}

/// Whether `config` turns NDI® on: `NdiConfig$NdiState` is "a setting that controls whether
/// NDI sources or outputs can be used in the flow… The default value is DISABLED. This
/// value must be set as ENABLED for your flow to support NDI sources or outputs", so a flow
/// with no `NdiConfig`, or one whose state is unset, has NDI off.
pub fn ndi_is_enabled(config: Option<&NdiConfig>) -> bool {
    config.and_then(|config| config.ndi_state.as_deref()) == Some(STATE_ENABLED)
}

/// Whether a transport described by `protocol` and whether it carries `NdiSourceSettings`
/// is an NDI® transport.
///
/// The TWO markers of NDI on a source or an output, read here once: the protocol is
/// [`PROTOCOL_NDI_SPEED_HQ`], or the record carries `NdiSourceSettings` ("the settings for
/// the NDI® source… the exact name of the upstream NDI sender"), which only an NDI source
/// has. Every caller — the stored record, the create/add request shapes and the update
/// request shapes, which are all different types carrying the same two members — resolves
/// NDI through this function.
pub fn uses_ndi_transport(protocol: Option<&str>, ndi_source_settings: bool) -> bool {
    protocol == Some(PROTOCOL_NDI_SPEED_HQ) || ndi_source_settings
}

/// Whether `transport` is an NDI® transport — see [`uses_ndi_transport`].
pub fn transport_uses_ndi(transport: Option<&Transport>) -> bool {
    transport.is_some_and(|transport| {
        uses_ndi_transport(
            Some(transport.protocol.as_str()),
            transport.ndi_source_settings.is_some(),
        )
    })
}

/// Whether a stored source is an NDI® source — see [`transport_uses_ndi`].
pub fn stored_source_uses_ndi(source: &StoredSource) -> bool {
    transport_uses_ndi(source.transport.as_ref())
}

/// Whether a `SetSourceRequest` describes an NDI® source: the same two markers
/// [`uses_ndi_transport`] reads, on the request the source is materialised from.
pub fn source_request_uses_ndi(request: &SetSourceRequest) -> bool {
    uses_ndi_transport(
        request.protocol.as_deref(),
        request.ndi_source_settings.is_some(),
    )
}

/// Whether an `AddOutputRequest` describes an NDI® output: its protocol is the NDI
/// transport. `NdiSourceSettings` is a SOURCE's member, and the request's other `Ndi…`
/// members tune an NDI sender the output only has when that protocol is what carries it
/// (`AddOutputRequest$NdiProgramName` is "a suffix for the name of the NDI® sender that the
/// flow creates"), so the PROTOCOL is what makes an output an NDI output.
pub fn output_request_uses_ndi(request: &AddOutputRequest) -> bool {
    uses_ndi_transport(request.protocol.as_deref(), false)
}

/// `NdiOutputTimecodeSource`'s documented default: "controls how MediaConnect generates
/// timecodes for NDI output frames. If you don't specify this field, MediaConnect uses
/// EMBEDDED_TIMECODE" (`smithy_query(query="doc",shape="AddOutputRequest",
/// member="NdiOutputTimecodeSource")`), which the reported shape states as a presence rule:
/// "for NDI outputs, this field is always present and defaults to EMBEDDED_TIMECODE"
/// (`Transport$NdiOutputTimecodeSource`).
pub const NDI_OUTPUT_TIMECODE_SOURCE_EMBEDDED: &str = "EMBEDDED_TIMECODE";

/// `NdiAudioReferenceLevel`'s documented default, in decibels relative to full scale:
/// "the audio reference level of the NDI receiver… If you don't specify this field,
/// MediaConnect uses -20" (`smithy_query(query="doc",shape="AddOutputRequest",
/// member="NdiAudioReferenceLevel")`), which the reported shape states as a presence rule:
/// "for NDI outputs, this field is always present and defaults to -20"
/// (`Transport$NdiAudioReferenceLevel`). It sits inside the member's modelled range
/// (-40..=0).
pub const DEFAULT_NDI_AUDIO_REFERENCE_LEVEL_DBFS: i32 = -20;

/// Materialise, on the transport of an output named `output_name`, the NDI® sender
/// settings whose documentation names the value the service uses when the request carries
/// none — the same way a request that names no `OutputStatus` gets the documented
/// [`STATUS_ENABLED`].
///
/// The three members are the NDI sender's, so the condition the documentation attaches
/// them to is the one that makes an OUTPUT an NDI output ([`uses_ndi_transport`] on the
/// output's protocol, with the source marker excluded exactly as
/// [`stored_output_uses_ndi`] excludes it: `NdiSourceSettings` is a source's member, which
/// no output carries): a transport on any other protocol is left exactly as the request
/// described it. Each documented value fills only a member the request left unset, so a
/// value the caller named is never overwritten:
///
/// * `NdiOutputTimecodeSource` → [`NDI_OUTPUT_TIMECODE_SOURCE_EMBEDDED`];
/// * `NdiAudioReferenceLevel` → [`DEFAULT_NDI_AUDIO_REFERENCE_LEVEL_DBFS`];
/// * `NdiProgramName` → the output's own name, because it is "a suffix for the name of the
///   NDI® sender that the flow creates. If a custom name isn't specified, MediaConnect uses
///   the output name".
///
/// Every path that writes an output's transport applies it HERE — the add/create path
/// ([`StoredOutput::from_add_request`]) and the update path, which can move an output onto
/// the NDI transport without naming these members — so the stored record an NDI output is
/// read back from always carries them.
pub fn apply_ndi_output_defaults(transport: &mut Transport, output_name: &str) {
    if !uses_ndi_transport(Some(transport.protocol.as_str()), false) {
        return;
    }
    if transport.ndi_output_timecode_source.is_none() {
        transport.ndi_output_timecode_source =
            Some(NDI_OUTPUT_TIMECODE_SOURCE_EMBEDDED.to_string());
    }
    if transport.ndi_audio_reference_level.is_none() {
        transport.ndi_audio_reference_level = Some(DEFAULT_NDI_AUDIO_REFERENCE_LEVEL_DBFS);
    }
    if transport.ndi_program_name.is_none() {
        transport.ndi_program_name = Some(output_name.to_string());
    }
}

/// Whether a stored output is an NDI® output: the same single marker
/// [`output_request_uses_ndi`] reads — the protocol that carries it — on the record the
/// request was materialised into, so the stored output and the request it came from are
/// NDI for the same reason.
pub fn stored_output_uses_ndi(output: &StoredOutput) -> bool {
    uses_ndi_transport(
        output
            .transport
            .as_ref()
            .map(|transport| transport.protocol.as_str()),
        false,
    )
}

/// The detail a request is refused with when it would leave a flow including NDI® at a size
/// that does not carry it: `ndi_feature` names what uses NDI, `flow_size` is the size the
/// flow ends up with. `None` when that size is NDI-capable ([`flow_size_supports_ndi`]), so
/// a caller states the rule by naming the feature and the size and nothing else.
///
/// Every operation that can put NDI on a flow words the refusal HERE, so the create side,
/// the add sides and the update sides state one rule for one reason.
pub fn ndi_flow_size_rejection(ndi_feature: &str, flow_size: Option<&str>) -> Option<String> {
    if flow_size_supports_ndi(flow_size) {
        return None;
    }
    Some(format!(
        "{ndi_feature} needs a flow of size {}; this flow's size is {}. Set the flow size to \
         one that supports NDI before enabling NDI sources or outputs on it.",
        NDI_CAPABLE_FLOW_SIZES.join(" or "),
        flow_size.unwrap_or("unset")
    ))
}

/// The ceiling on how many NDI® discovery servers ONE FLOW's `NdiConfig` declares: "a list
/// of up to three NDI discovery server configurations"
/// (`smithy_query(query="doc",shape="NdiConfig",member="NdiDiscoveryServers")`), the same
/// count the member's target shape publishes ("a maximum of 3 servers is allowed",
/// `smithy_query(query="shape",shape="NdiDiscoveryServerConfig")`).
///
/// Like [`MAX_FLOW_MEDIA_STREAMS`] this bounds the STORED flow: the config is kept verbatim
/// on a record nothing evicts and every later DescribeFlow reports it, so the count is
/// bounded before it is stored. Named ONCE here because every operation that writes a
/// flow's `NdiConfig` reads it (CreateFlow and UpdateFlow). Neither models a count
/// exception for it, so both refuse past the bound with `BadRequestException`, the error
/// the model describes for "a semantic error" in the request.
pub const MAX_NDI_DISCOVERY_SERVERS: usize = 3;

/// The detail a request is refused with when the `NdiConfig` it carries under `member`
/// declares more discovery servers than [`MAX_NDI_DISCOVERY_SERVERS`], or `None` when the
/// config is one a flow may hold (including a request that carries no config, and one whose
/// `NdiDiscoveryServers` is absent or empty — the member is optional).
///
/// One wording for one rule, so the create side and the update side state the same bound
/// for the same reason.
pub fn ndi_discovery_servers_rejection(member: &str, config: Option<&NdiConfig>) -> Option<String> {
    let declared = config
        .and_then(|config| config.ndi_discovery_servers.as_ref())
        .map_or(0, |servers| servers.len());
    if declared <= MAX_NDI_DISCOVERY_SERVERS {
        return None;
    }
    Some(format!(
        "{member}.NdiDiscoveryServers may declare at most {MAX_NDI_DISCOVERY_SERVERS} NDI \
         discovery server configurations; this one declares {declared}."
    ))
}

/// How many characters the NDI® source-name prefix this service generates has:
/// `NdiConfig$MachineName` is "a prefix for the names of the NDI sources that the flow
/// creates. If a custom name isn't specified, MediaConnect generates a unique 12-character
/// ID as the prefix" (`smithy_query(query="doc",shape="NdiConfig",member="MachineName")`).
pub const NDI_MACHINE_NAME_LEN: usize = 12;

/// The index of the resource-id segment of an ARN this service mints
/// (`arn:<partition>:mediaconnect:<region>:<account>:<type>:<id>:<name>`), the segment
/// [`flow_resource_fragment`] fills with the per-flow generated id.
const ARN_RESOURCE_ID_SEGMENT: usize = 6;

/// The NDI® machine name this service generates for the flow `flow_arn`: the "unique
/// 12-character ID" [`NDI_MACHINE_NAME_LEN`] quotes, derived from the flow's OWN identity.
///
/// The id comes from the per-flow generated id the flow's ARN carries
/// ([`flow_resource_fragment`] mints it from the store's counter), with the separator
/// dropped and the result left-padded to the published width — so it is unique per flow,
/// and it is the SAME value every time it is derived, which is what lets a flow keep one
/// prefix for its whole life however many times its config is rewritten. The last
/// [`NDI_MACHINE_NAME_LEN`] alphanumerics are what is kept, so the answer is exactly that
/// many characters for any ARN, including one whose segments this service did not mint.
pub fn generated_ndi_machine_name(flow_arn: &str) -> String {
    let flow_id = flow_arn
        .split(':')
        .nth(ARN_RESOURCE_ID_SEGMENT)
        .filter(|segment| !segment.is_empty())
        .unwrap_or(flow_arn);
    let characters: Vec<char> = flow_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect();
    let tail: String = characters[characters.len().saturating_sub(NDI_MACHINE_NAME_LEN)..]
        .iter()
        .collect();
    format!("{tail:0>width$}", width = NDI_MACHINE_NAME_LEN)
}

/// The `NdiConfig` the flow `flow_arn` STORES for the config `requested` (the one the
/// request writes, or the one the flow already holds where the request writes none), with
/// every member the SERVICE assigns materialised on it.
///
/// `MachineName` is such a member: "a prefix for the names of the NDI sources that the flow
/// creates. If a custom name isn't specified, MediaConnect generates a unique 12-character
/// ID as the prefix" — so a config whose request named no custom prefix is stored carrying
/// the one [`generated_ndi_machine_name`] mints for the flow, exactly as a source whose
/// request named no name is stored under the one [`resolve_source_name`] mints. A prefix the
/// caller DID name is kept as given.
///
/// Every operation that writes a flow's `NdiConfig` resolves it HERE (CreateFlow and
/// UpdateFlow), so the record every later read reports carries the prefix whichever call
/// wrote it.
pub fn resolved_ndi_config(requested: Option<NdiConfig>, flow_arn: &str) -> Option<NdiConfig> {
    let mut config = requested?;
    if config
        .machine_name
        .as_deref()
        .is_none_or(|name| name.is_empty())
    {
        config.machine_name = Some(generated_ndi_machine_name(flow_arn));
    }
    Some(config)
}

/// Whether `hour` is a maintenance start hour: "UTC time when the maintenance will happen.
/// Use 24-hour HH:MM format. Minutes must be 00. Example: 13:00."
/// (`smithy_query(query="doc",shape="AddMaintenance",member="MaintenanceStartHour")`, the
/// same sentence `UpdateMaintenance$MaintenanceStartHour` and
/// `Maintenance$MaintenanceStartHour` carry).
pub fn is_maintenance_start_hour(hour: &str) -> bool {
    let Some((hours, minutes)) = hour.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes == "00"
        && hours.bytes().all(|byte| byte.is_ascii_digit())
        && hours
            .parse::<u8>()
            .is_ok_and(|hours| (0..=23).contains(&hours))
}

/// Whether `date` is a scheduled maintenance date: "a scheduled date in ISO UTC format
/// when the maintenance will happen. Use YYYY-MM-DD format. Example: 2021-01-30."
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceScheduledDate")`).
/// The day is bounded by the longest month rather than by the month named, which is as far
/// as the documented format goes.
pub fn is_maintenance_scheduled_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        && year.parse::<u16>().is_ok()
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
        && day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
}

/// The detail a request asking for a maintenance window is refused with when one of the
/// two free-form members it carries is not in its documented format, or `None` when the
/// window is expressible as one.
///
/// Every operation that WRITES a flow's maintenance window — the create shape
/// (`AddMaintenance`, which requires the start hour) and the update shape
/// (`UpdateMaintenance`, whose members are each optional) — checks it here, so one rule
/// refuses one value for one reason whichever path stores it.
pub fn maintenance_window_rejection(
    maintenance_start_hour: Option<&str>,
    maintenance_scheduled_date: Option<&str>,
) -> Option<String> {
    if let Some(hour) = maintenance_start_hour.filter(|hour| !is_maintenance_start_hour(hour)) {
        return Some(format!(
            "Invalid MaintenanceStartHour {hour:?}. Use 24-hour HH:MM format with minutes 00, \
             for example 13:00."
        ));
    }
    if let Some(date) =
        maintenance_scheduled_date.filter(|date| !is_maintenance_scheduled_date(date))
    {
        return Some(format!(
            "Invalid MaintenanceScheduledDate {date:?}. Use YYYY-MM-DD format, for example \
             2021-01-30."
        ));
    }
    None
}

/// The detail a request naming an Availability Zone is refused with when that zone is not
/// one of `region`, or `None` when it is.
///
/// Every member a request can name a zone in is "limited to the Availability Zones within
/// the current Region" (`CreateFlowRequest$AvailabilityZone`) or "must be a valid
/// Availability Zone for the region specified by regionName, or the current region if no
/// regionName is provided" (`CreateRouterInputRequest$AvailabilityZone`), so each create
/// path words the refusal HERE, over the one encoding of the zone naming
/// ([`is_zone_of_region`]).
fn availability_zone_rejection(zone: &str, region: &str) -> Option<String> {
    if is_zone_of_region(zone, region) {
        return None;
    }
    Some(format!(
        "AvailabilityZone {zone:?} is not an Availability Zone of Region {region:?}."
    ))
}

/// The Availability Zone a create STORES for a request that named `requested` in `region`,
/// or `Err` with the detail the request is refused with when that zone is not one of
/// `region` ([`availability_zone_rejection`]). A request that names none is given a zone of
/// the Region ([`default_availability_zone`]), because every resource that carries a zone
/// reports one.
///
/// This is the crate's ONE resolution of the member, so a zone the flow create stores is a
/// zone the router creates would also accept, and no path stores one its own Region has
/// not got.
pub fn resolved_availability_zone(requested: Option<&str>, region: &str) -> Result<String, String> {
    match requested {
        Some(zone) => match availability_zone_rejection(zone, region) {
            Some(detail) => Err(detail),
            None => Ok(zone.to_string()),
        },
        None => Ok(default_availability_zone(region)),
    }
}

/// The first port handed to a source that did not ask for one. MediaConnect assigns an
/// ingest port from its own pool when the request omits `IngestPort`; the emulator
/// assigns from a WRAPPING pool that starts at this base, so a source's port is drawn
/// from the pool rather than guaranteed unique — two sources far enough apart in the
/// ARN counter can share one, which is harmless because nothing in the emulator binds
/// these ports.
const INGEST_PORT_BASE: i32 = 5000;

/// The highest port the ingest-port pool hands out, so the assignment stays inside the
/// 16-bit port space however many sources are created.
const INGEST_PORT_CEILING: i32 = 65_535;

/// `MediaStreamType` (`smithy_query(query="shape",shape="MediaStreamType")`: video |
/// audio | ancillary-data).
pub const MEDIA_STREAM_TYPE_VIDEO: &str = "video";
/// See [`MEDIA_STREAM_TYPE_VIDEO`].
pub const MEDIA_STREAM_TYPE_AUDIO: &str = "audio";
/// See [`MEDIA_STREAM_TYPE_VIDEO`].
pub const MEDIA_STREAM_TYPE_ANCILLARY_DATA: &str = "ancillary-data";

/// The sample rate the service applies to a video or ancillary-data media stream whose
/// request names none: `AddMediaStreamRequest$ClockRate` documents "If the media stream
/// type is video or ancillary data, set this value to 90000." Audio has two documented
/// values (48000 or 96000) and so no single default — an audio stream that names no
/// clock rate keeps none.
pub const VIDEO_CLOCK_RATE_HZ: i32 = 90_000;

/// The first dynamic RTP payload type (RFC 3551 §6: 96-127 are dynamic), and so the
/// first `Fmt` this service hands a media stream. `MediaStream$Fmt` is "the format type
/// number (sometimes referred to as RTP payload type) of the media stream. MediaConnect
/// assigns this value to the media stream."
pub const DYNAMIC_RTP_PAYLOAD_TYPE_BASE: i32 = 96;

/// The `Protocol` enum values the smoothing max bitrate applies to:
/// `Transport$MaxBitrate` is "the smoothing max bitrate (in bps) for RIST, RTP, and
/// RTP-FEC streams", which are the `rist`, `rtp` and `rtp-fec` values of `Protocol`
/// (`smithy_query(query="shape",shape="Protocol")`). A source on any other protocol
/// carries no smoothing bitrate unless its request named one.
pub const SMOOTHING_BITRATE_PROTOCOLS: [&str; 3] = ["rist", "rtp", "rtp-fec"];

/// The smoothing max bitrate (in bits per second) MediaConnect assigns a source on one
/// of [`SMOOTHING_BITRATE_PROTOCOLS`] whose request named none.
///
/// `SetSourceRequest$MaxBitrate` is optional and the service reports a populated
/// `Transport$MaxBitrate` on such a source either way, so the value is service-assigned
/// rather than echoed: the recorded CreateFlow and DescribeFlow bodies this emulator is
/// replayed against report 160 Mbps for an `rtp` source, and they report it for a source
/// whose request named NO bitrate — a request that names one already round-trips
/// unchanged (see `tests/all/flow_projection.rs`), so an echo would have matched the
/// recording before this assignment existed, and it did not. The model publishes no
/// default on the member, so the number is named here once and nowhere else.
pub const ASSIGNED_SOURCE_MAX_BITRATE_BPS: i32 = 160_000_000;

/// The `Transport$MaxBitrate` a source on `protocol` reports, given the `requested`
/// bitrate its request carried (`None` when it carried none).
///
/// A request that names a bitrate keeps it, whatever the protocol; one that names none
/// gets the service's assignment where the member applies, and nothing where it does
/// not. Every operation that materialises a source resolves it HERE, so one request
/// shape always becomes the same stored transport.
pub fn assigned_source_max_bitrate(protocol: &str, requested: Option<i32>) -> Option<i32> {
    requested.or_else(|| {
        SMOOTHING_BITRATE_PROTOCOLS
            .contains(&protocol)
            .then_some(ASSIGNED_SOURCE_MAX_BITRATE_BPS)
    })
}

/// The `Protocol` enum values that are Zixi protocols: the `zixi-push` and `zixi-pull`
/// values of `Protocol` (`smithy_query(query="shape",shape="Protocol")`).
pub const ZIXI_PROTOCOLS: [&str; 2] = ["zixi-push", "zixi-pull"];

/// The one port a source on one of [`ZIXI_PROTOCOLS`] listens on: "the port that the flow
/// listens on for incoming content. If the protocol of the source is Zixi, the port must
/// be set to 2088"
/// (`smithy_query(query="doc",shape="UpdateFlowSourceRequest",member="IngestPort")`).
pub const ZIXI_INGEST_PORT: i32 = 2088;

/// Whether `protocol` is one of [`ZIXI_PROTOCOLS`].
pub fn is_zixi_protocol(protocol: &str) -> bool {
    ZIXI_PROTOCOLS.contains(&protocol)
}

/// The `Source$IngestPort` a source on `protocol` listens on, given the `port` it would
/// otherwise carry — `None` for a source that listens on no ingest endpoint of its own,
/// which keeps none.
///
/// A source on a Zixi protocol listens on [`ZIXI_INGEST_PORT`] and on no other port, so
/// that is the port the service hands one whichever call put it on that protocol. Every
/// operation that materialises or updates a source resolves the port HERE, exactly as it
/// resolves the smoothing bitrate through [`assigned_source_max_bitrate`], so a source
/// whose protocol moves onto Zixi reports the port it would have been created with. A port
/// the request itself named is admitted before this (see [`zixi_ingest_port_rejection`]),
/// so this never silently replaces a value a client chose and was answered 2xx for.
pub fn assigned_source_ingest_port(protocol: Option<&str>, port: Option<i32>) -> Option<i32> {
    match port {
        Some(_) if protocol.is_some_and(is_zixi_protocol) => Some(ZIXI_INGEST_PORT),
        port => port,
    }
}

/// The ingest endpoint — the address and the port TOGETHER — a source reports, given the
/// `entitlement_arn` it carries, the `protocol` it is on, and the endpoint it would
/// otherwise hold.
///
/// A source reached through another account's entitlement is fed by that account's flow, so
/// this service assigns it no ingest endpoint of its own — neither half of the pair — while
/// an owned source listens on the address and the port the service assigned it, the port
/// resolved against the protocol through [`assigned_source_ingest_port`]. The two halves are
/// one service-derived relation keyed on the same marker a flow's `SourceType` is read off
/// ([`StoredFlow::source_type`]), so every operation that materialises or UPDATES a source
/// resolves them HERE: the pair cannot drift apart into a source that is entitled and still
/// advertises an endpoint of this account, or that holds a port with no address beside it.
pub fn assigned_source_ingest_endpoint(
    entitlement_arn: Option<&str>,
    protocol: Option<&str>,
    ingest_ip: Option<String>,
    ingest_port: Option<i32>,
) -> (Option<String>, Option<i32>) {
    if entitlement_arn.is_some() {
        return (None, None);
    }
    (ingest_ip, assigned_source_ingest_port(protocol, ingest_port))
}

/// Why `ingest_port` cannot be the port a source on `protocol` listens on, or `None` when
/// it can — including when the request named no port at all, because the service then
/// assigns the source its port ([`assigned_source_ingest_port`]). `member` is the request
/// member that named it.
///
/// A Zixi source listens on one port and the model states which: "if the protocol of the
/// source is Zixi, the port must be set to 2088". So a request that names any other port
/// for a source that would be on a Zixi protocol describes a source this service does not
/// listen for, and is refused with `BadRequestException` — the error the model describes as
/// the one for "a semantic error" in the request. Every operation that stores a source's
/// ingest port (CreateFlow's `Source` / `Sources`, AddFlowSources, UpdateFlowSource)
/// refuses through this one function, so one request value cannot be admitted by one of
/// them and refused by another.
pub fn zixi_ingest_port_rejection(
    member: &str,
    protocol: Option<&str>,
    ingest_port: Option<i32>,
) -> Option<String> {
    let port = ingest_port?;
    let protocol = protocol.filter(|protocol| is_zixi_protocol(protocol))?;
    if port == ZIXI_INGEST_PORT {
        return None;
    }
    Some(format!(
        "{member} {port} cannot be the port a source on the {protocol:?} protocol listens \
         on: if the protocol of the source is Zixi, the port must be set to \
         {ZIXI_INGEST_PORT}."
    ))
}

/// The recovery window (in milliseconds) a flow's source failover config reports when
/// the request that set it named none.
///
/// `FailoverConfig$RecoveryWindow` / `UpdateFailoverConfig$RecoveryWindow` are the
/// "search window time to look for dash-7 packets" and are optional on the way in, yet a
/// flow that carries a failover config reports a window either way: 200 ms, the
/// documented default search window for dash-7 merge. The model publishes no default on
/// the member, so the number is named here once.
pub const ASSIGNED_RECOVERY_WINDOW_MS: i32 = 200;

/// `config` with the members MediaConnect assigns itself resolved.
///
/// Every operation that writes a `SourceFailoverConfig` (CreateFlow's
/// `SourceFailoverConfig`, UpdateFlow's merge of `UpdateFailoverConfig`, CreateBridge's
/// `SourceFailoverConfig` — all three target the same `FailoverConfig` shape, whose
/// `RecoveryWindow` member carries the same documentation wherever it appears) stores the
/// config through this function, so failover settings never depend on which call wrote
/// them.
pub fn resolved_failover_config(config: FailoverConfig) -> FailoverConfig {
    FailoverConfig {
        recovery_window: Some(
            config
                .recovery_window
                .unwrap_or(ASSIGNED_RECOVERY_WINDOW_MS),
        ),
        ..config
    }
}

/// The failover setting a resource holds after `update` is applied to `stored`.
///
/// `UpdateFailoverConfig` mirrors `FailoverConfig` member for member and every member of
/// it is optional, so applying one is a member-wise merge: a member the request carries
/// replaces its counterpart, one it omits keeps the stored value. The merged result is
/// resolved through [`resolved_failover_config`], so the window MediaConnect assigns a
/// config that names none is the same here as on a create path.
///
/// Shared by every operation that applies an `UpdateFailoverConfig` — UpdateFlow's
/// `SourceFailoverConfig` and UpdateBridge's, which target the same request shape and the
/// same stored `FailoverConfig` — so one request shape always becomes the same stored
/// config.
pub fn merged_failover_config(
    stored: Option<&FailoverConfig>,
    update: &UpdateFailoverConfig,
) -> FailoverConfig {
    let mut merged = stored.cloned().unwrap_or_default();
    if let Some(mode) = update.failover_mode.clone() {
        merged.failover_mode = Some(mode);
    }
    if let Some(window) = update.recovery_window {
        merged.recovery_window = Some(window);
    }
    if let Some(priority) = update.source_priority.clone() {
        merged.source_priority = Some(priority);
    }
    if let Some(state) = update.state.clone() {
        merged.state = Some(state);
    }
    resolved_failover_config(merged)
}

/// Why `priority` cannot be the source priority of a FLOW holding the sources named by
/// `source_names`, or `None` when it can.
///
/// See [`primary_source_rejection`], which both the flow side and the bridge side resolve
/// the reference through.
pub fn flow_primary_source_rejection<'a>(
    priority: Option<&SourcePriority>,
    source_names: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    primary_source_rejection("flow", priority, source_names)
}

/// Why `priority` cannot be the source priority of a BRIDGE holding the sources named by
/// `source_names`, or `None` when it can.
///
/// See [`primary_source_rejection`], which both the flow side and the bridge side resolve
/// the reference through.
pub fn bridge_primary_source_rejection<'a>(
    priority: Option<&SourcePriority>,
    source_names: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    primary_source_rejection("bridge", priority, source_names)
}

/// The detail a request is refused with when the `SourcePriority` it carries names a
/// primary source that `resource` — a flow or a bridge, the two resources whose sources one
/// `SourceFailoverConfig` governs — does not have, or `None` when it names one of
/// `source_names` (or names no primary source at all).
///
/// `SourcePriority$PrimarySource` is "the name of the source you choose as the primary
/// source for this flow", so it is a reference to one of the resource's OWN sources, and a
/// failover setting may only be stored when that name resolves against the sources the
/// resource holds once the request has been applied — exactly as a source's or an output's
/// media-stream and VPC-interface names are resolved against the ones its resource holds
/// (see [`source_request_reference_rejection`]).
///
/// Every operation that writes a source priority resolves the reference HERE: the create
/// paths against the sources THAT request creates, which are all the sources the new
/// resource has, and the update paths against the sources the resource already holds. So a
/// stored failover setting never names a source its resource has not got, whichever call
/// wrote it. The refusal is `BadRequestException`, the error the model describes as the one
/// for "a semantic error" in the request.
fn primary_source_rejection<'a>(
    resource: &str,
    priority: Option<&SourcePriority>,
    source_names: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let primary = priority.and_then(|priority| priority.primary_source.as_deref())?;
    if source_names.into_iter().any(|name| name == primary) {
        return None;
    }
    Some(format!(
        "Source {primary:?} is not a source of this {resource}, so it cannot be its \
         primary source."
    ))
}

/// Why the source named `source_name` cannot be taken out of a FLOW whose stored source
/// failover setting is `config`, or `None` when taking it out leaves that setting naming a
/// source the flow still has.
///
/// See [`primary_source_removal_rejection`], which both the flow side and the bridge side
/// read the relation from.
pub fn flow_primary_source_removal_rejection(
    config: Option<&FailoverConfig>,
    source_name: &str,
) -> Option<String> {
    primary_source_removal_rejection("flow", config, source_name)
}

/// Why the source named `source_name` cannot be taken out of a BRIDGE whose stored source
/// failover setting is `config`, or `None` when taking it out leaves that setting naming a
/// source the bridge still has.
///
/// See [`primary_source_removal_rejection`], which both the flow side and the bridge side
/// read the relation from.
pub fn bridge_primary_source_removal_rejection(
    config: Option<&FailoverConfig>,
    source_name: &str,
) -> Option<String> {
    primary_source_removal_rejection("bridge", config, source_name)
}

/// The detail a REMOVE request is refused with when `source_name` is the source that
/// `resource` — a flow or a bridge, the two resources whose sources one
/// `SourceFailoverConfig` governs — stores as its primary source, or `None` when the stored
/// setting names some other source or none at all.
///
/// The removal side of the reference [`primary_source_rejection`] resolves on every write: a
/// stored failover setting may only name a source its resource HAS, so a request that would
/// take that source away is a semantic error and takes nothing away — the same reading the
/// other removals of this family apply to the references their resource holds (a media
/// stream a source or an output still associates, a VPC interface a source or an output
/// still references). A caller that wants the source gone points the priority at another
/// source of the resource first, through the update path that writes one.
fn primary_source_removal_rejection(
    resource: &str,
    config: Option<&FailoverConfig>,
    source_name: &str,
) -> Option<String> {
    let primary = config
        .and_then(|config| config.source_priority.as_ref())
        .and_then(|priority| priority.primary_source.as_deref())?;
    if primary != source_name {
        return None;
    }
    Some(format!(
        "Source {source_name:?} is the primary source of this {resource}'s source failover \
         configuration. A source can be removed only while the source failover priority of \
         its {resource} does not name it as the primary source."
    ))
}

/// The encryption a resource holds after `update` is applied to `stored`, or `None` when
/// the merge would leave an encryption with no role ARN.
///
/// `UpdateEncryption` mirrors `Encryption` member for member and every member of it is
/// optional, so applying one is a member-wise merge — the same merge
/// [`merged_failover_config`] performs for an `UpdateFailoverConfig`: a member the request
/// carries replaces its counterpart, one it omits keeps the stored value.
///
/// `Encryption$RoleArn` is REQUIRED (`smithy_query(query="shape",shape="Encryption")`)
/// while `UpdateEncryption$RoleArn` is not, so a resource that holds no encryption yet can
/// only be given one by a request that names the role: `None` says the merge is not
/// expressible, and the caller refuses the request with its own modelled client error.
///
/// Every operation that applies an `UpdateEncryption` resolves it HERE — UpdateFlowOutput's
/// `Encryption`, UpdateFlowEntitlement's and UpdateFlowSource's `Decryption`, which target
/// the same request shape and the same stored `Encryption` — so one request shape always
/// becomes the same stored record.
pub fn merged_encryption(stored: Option<&Encryption>, update: &UpdateEncryption) -> Option<Encryption> {
    let role_arn = update
        .role_arn
        .clone()
        .or_else(|| stored.map(|encryption| encryption.role_arn.clone()))?;
    let mut merged = stored.cloned().unwrap_or_default();
    merged.role_arn = role_arn;
    if let Some(algorithm) = update.algorithm.clone() {
        merged.algorithm = Some(algorithm);
    }
    if let Some(vector) = update.constant_initialization_vector.clone() {
        merged.constant_initialization_vector = Some(vector);
    }
    if let Some(device_id) = update.device_id.clone() {
        merged.device_id = Some(device_id);
    }
    if let Some(key_type) = update.key_type.clone() {
        merged.key_type = Some(key_type);
    }
    if let Some(region) = update.region.clone() {
        merged.region = Some(region);
    }
    if let Some(resource_id) = update.resource_id.clone() {
        merged.resource_id = Some(resource_id);
    }
    if let Some(secret_arn) = update.secret_arn.clone() {
        merged.secret_arn = Some(secret_arn);
    }
    if let Some(url) = update.url.clone() {
        merged.url = Some(url);
    }
    Some(merged)
}

/// A status change the clock owes: at `due_at` the flow's status becomes `status`.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The status the flow settles into.
    pub status: String,
    /// The instant from which the change is owed.
    pub due_at: SystemTime,
}

/// Per-service state for com-amazonaws-mediaconnect-mediaconnect. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediaconnectMediaconnectState {
    /// Every live flow, keyed by its ARN. Ordered, so ListFlows pages stably.
    ///
    /// There is no index by name beside it: the model puts no uniqueness rule on a flow
    /// name, and the flow id in the ARN is what keeps two same-named flows apart — the
    /// same reasoning as for [`Self::gateways`].
    pub flows: BTreeMap<String, StoredFlow>,
    /// Every gateway the store holds, keyed by its ARN. Ordered, so ListGateways pages
    /// stably.
    ///
    /// Live gateways and the retired (deleted) ones alike: a gateway delete is a soft
    /// delete, so a deleted gateway stays in this map — and readable — in its terminal
    /// `DELETED` state (see [`StoredGateway::deleted_at`]).
    ///
    /// There is no index by name beside it: like a flow name, a gateway name is under no
    /// uniqueness rule in the model, and the gateway id in the ARN is what keeps two
    /// same-named gateways apart.
    pub gateways: BTreeMap<String, StoredGateway>,
    /// Every gateway instance the store holds, keyed by its ARN — the identity every
    /// instance operation addresses one by (`@httpLabel GatewayInstanceArn`). Ordered, so
    /// ListGatewayInstances pages stably.
    ///
    /// Registered instances and deregistered ones alike: `InstanceState` carries the
    /// terminal `DEREGISTERED` value, so a deregistered instance stays in this map — and
    /// readable — exactly as a deleted gateway does (see [`StoredGatewayInstance`]).
    ///
    /// One map for every gateway's instances, not one per gateway: ListGatewayInstances'
    /// `FilterArn` is a FILTER on an otherwise whole-account listing (the member is
    /// optional), and an instance names its gateway on its own record.
    pub gateway_instances: BTreeMap<String, StoredGatewayInstance>,
    /// Every live bridge, keyed by its ARN. Ordered, so ListBridges pages stably.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]:
    /// the model puts no uniqueness rule on `Bridge$Name`, and the bridge id in the ARN
    /// is what keeps two same-named bridges apart.
    pub bridges: BTreeMap<String, StoredBridge>,
    /// Every router input the store holds, keyed by its ARN — the identity every router
    /// input operation addresses one by (`@httpLabel Arn`). Ordered, so ListRouterInputs
    /// can page stably once it is implemented.
    ///
    /// A deleted router input leaves this map one [`TRANSITION_SPAN`] after the delete
    /// (see [`StoredRouterInput::removal_due_at`]): the model declares no terminal
    /// `DELETED` state it could be reported in, so unlike a gateway it is not retained.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterInput$Name`, and the router input id in the
    /// ARN ([`router_resource_id`]) is what keeps two same-named inputs apart — the name is
    /// not even part of the ARN, which the modelled `RouterInputArn` pattern fixes to the
    /// id alone.
    pub router_inputs: BTreeMap<String, StoredRouterInput>,
    /// Every router output, keyed by its ARN — the identity every router output
    /// operation addresses one by (`@httpLabel Arn`, and the `Arns` BatchGetRouterOutput
    /// reads them by). Ordered, so ListRouterOutputs can page stably.
    ///
    /// A router output whose delete has been accepted is still in this map, in `DELETING`,
    /// until the clock completes the delete; then it LEAVES, because `RouterOutputState`
    /// has no terminal deleted value (see [`StoredRouterOutput::removal_due_at`]).
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterOutput$Name`, and the generated output id in
    /// the ARN is what keeps two same-named outputs apart.
    pub router_outputs: BTreeMap<String, StoredRouterOutput>,
    /// Every router network interface, keyed by its ARN — the identity every router network
    /// interface operation addresses one by (`@httpLabel Arn`, and the `Arns`
    /// BatchGetRouterNetworkInterface reads them by). Ordered, so
    /// ListRouterNetworkInterfaces can page stably.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `RouterNetworkInterface$Name`, and the generated
    /// interface id in the ARN is what keeps two same-named interfaces apart — the name is
    /// not part of the ARN at all, which the modelled `RouterNetworkInterfaceArn` pattern
    /// fixes to the id alone.
    pub router_network_interfaces: BTreeMap<String, StoredRouterNetworkInterface>,
    /// Every reservation this account holds, keyed by its ARN — the identity
    /// DescribeReservation addresses one by (`@httpLabel ReservationArn`). Ordered, so
    /// ListReservations pages stably.
    ///
    /// A reservation is never removed: ListReservations "displays a list of all
    /// reservations that have been purchased by this account in the current Amazon Web
    /// Services Region. This list includes all reservations in all states (such as active
    /// and expired)" (`smithy_query(query="operation",operation="ListReservations")`), so a
    /// reservation the clock has expired stays in this map and keeps being reported, in its
    /// terminal state.
    ///
    /// There is no index by name beside it, for the same reason as [`Self::gateways`]: the
    /// model puts no uniqueness rule on `Reservation$ReservationName`, and the reservation
    /// id in the ARN is what keeps two same-named reservations apart.
    pub reservations: BTreeMap<String, StoredReservation>,
    /// Monotonic counter minting the unique fragment of every flow / source / output /
    /// entitlement ARN and of the addresses and ports the service assigns.
    pub next_arn_suffix: u64,
}

/// A mutable handle on the tag state of ONE global resource, as
/// [`ComAmazonawsMediaconnectMediaconnectState::global_resource_tags_mut`] resolves it:
/// the record's tag map, and the record's `UpdatedAt`, which a write that CHANGES the map
/// moves (the same reading of "last changed" every router resource's `UpdatedAt` carries).
///
/// The three global resource records are different types with no shared field layout, so
/// the two things a tag write touches are handed back as borrows of the record rather than
/// as the record itself — which is also what keeps the write identical whichever of the
/// three it addresses.
#[derive(Debug)]
pub struct GlobalResourceTags<'a> {
    /// The resource's stored tag map, the one a tag read reports and a tag write merges
    /// into.
    pub tags: &'a mut BTreeMap<String, String>,
    /// The record's `UpdatedAt`.
    pub updated_at: &'a mut SystemTime,
}

impl ComAmazonawsMediaconnectMediaconnectState {
    /// The next value of the ARN-fragment counter. Never repeats within a process.
    pub fn next_suffix(&mut self) -> u64 {
        self.next_arn_suffix += 1;
        self.next_arn_suffix
    }

    /// The tags of the resource `resource_arn` addresses, or `None` when this service
    /// holds no tags for such a resource.
    ///
    /// A `ResourceArn` addresses "a resource ... in the current region", and every
    /// MediaConnect resource whose ARN this service mints and still holds in the flow
    /// hierarchy carries its OWN tag map: the flow itself, and each of its ARN-bearing
    /// sub-resources — an output (`Output$OutputArn`), a source (`Source$SourceArn`) and an
    /// entitlement (`Entitlement$EntitlementArn`), each taggable through its create
    /// request's own tag member (`AddOutputRequest$OutputTags`,
    /// `SetSourceRequest$SourceTags`, `GrantEntitlementRequest$EntitlementTags`). So an ARN
    /// this emulator minted and returned resolves here, whichever of those it names.
    ///
    /// What does NOT resolve: a resource that carries no tag map because no MediaConnect
    /// operation takes or reports its tags (a gateway, a gateway instance, a bridge, a
    /// reservation), a flow sub-resource with no ARN of its own (a media stream and a VPC
    /// interface are addressed by name within their flow, so no `ResourceArn` can name
    /// one), and a router input / router output / router network interface, whose tags are
    /// the GLOBAL tag operations' subject ([`Self::global_resource_tags_mut`], which covers
    /// exactly those three). Any other ARN addresses nothing whose tags these operations
    /// reach.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(flow) = self.flows.get(resource_arn) {
            return Some(&flow.tags);
        }
        self.flows
            .values()
            .find_map(|flow| flow.sub_resource_tags(resource_arn))
    }

    /// The tags of the GLOBAL resource `resource_arn` addresses, for a tag READ, or `None`
    /// when this service holds no such resource.
    ///
    /// The read side of [`Self::global_resource_tags_mut`], which is the ONE place a global
    /// `ResourceArn` is resolved: a reporting caller gets the record's own tag map and no
    /// handle on the record, so the lookup a tag read and a tag write agree on cannot drift
    /// between them. Which ARNs resolve, and why the clock's due work is materialised
    /// first, is documented there.
    pub fn global_tags_of(
        &mut self,
        resource_arn: &str,
        now: SystemTime,
    ) -> Option<&BTreeMap<String, String>> {
        // `&mut` reborrowed as `&`: the reporting caller can read the map it resolves and
        // cannot change it.
        Some(self.global_resource_tags_mut(resource_arn, now)?.tags)
    }

    /// The tags of the resource `resource_arn` addresses, for a tag write. See
    /// [`ComAmazonawsMediaconnectMediaconnectState::tags_of`] for which ARNs resolve.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        // Two lookups rather than one `get_mut`: returning the borrow from the first
        // branch would hold `flows` mutably borrowed across the fallback scan below, which
        // the borrow checker refuses (a conditional-return borrow the current region
        // inference cannot narrow).
        if self.flows.contains_key(resource_arn) {
            return self.flows.get_mut(resource_arn).map(|flow| &mut flow.tags);
        }
        self.flows
            .values_mut()
            .find_map(|flow| flow.sub_resource_tags_mut(resource_arn))
    }

    /// The tag state of the GLOBAL resource `resource_arn` addresses, or `None` when this
    /// service holds no such resource.
    ///
    /// A global tag operation "supports the following global resources: router inputs,
    /// router outputs and router network interfaces"
    /// (`smithy_query(query="operation",operation="TagGlobalResource")`), so a
    /// `ResourceArn` resolves against those three stores and nothing else — a flow, or one
    /// of its ARN-bearing sub-resources, is a REGIONAL resource, whose tags
    /// [`Self::tags_of`] is the lookup for. Each of the three ARN forms carries its own
    /// resource-type segment (`routerInput`, `routerOutput`, `routerNetworkInterface`), so
    /// one ARN can be a key in at most one of the maps and the order they are tried in
    /// cannot change which record is found.
    ///
    /// Everything the clock already made due is materialised first
    /// ([`Self::settle_due_transitions`]), exactly as [`Self::router_input_mut`] does, so a
    /// resource whose removal has already come due resolves to nothing here rather than
    /// taking tags no later caller could read.
    pub fn global_resource_tags_mut(
        &mut self,
        resource_arn: &str,
        now: SystemTime,
    ) -> Option<GlobalResourceTags<'_>> {
        self.settle_due_transitions(now);
        if let Some(router_input) = self.router_inputs.get_mut(resource_arn) {
            return Some(GlobalResourceTags {
                tags: &mut router_input.tags,
                updated_at: &mut router_input.updated_at,
            });
        }
        if let Some(router_output) = self.router_outputs.get_mut(resource_arn) {
            return Some(GlobalResourceTags {
                tags: &mut router_output.tags,
                updated_at: &mut router_output.updated_at,
            });
        }
        let interface = self.router_network_interfaces.get_mut(resource_arn)?;
        Some(GlobalResourceTags {
            tags: &mut interface.tags,
            updated_at: &mut interface.updated_at,
        })
    }

    /// Every entitlement the store holds that is granted TO the account `account_id`,
    /// keyed by the entitlement's own ARN.
    ///
    /// An entitlement is a sub-resource of the flow that GRANTS it, and the accounts it is
    /// granted to are its subscribers ([`StoredEntitlement::is_granted_to`]) — so the
    /// entitlements one account holds are gathered from every stored flow rather than read
    /// off one list. The keys are ARNs, so the answer is ordered and a listing can page
    /// from one stably, exactly as the ARN-keyed resource maps above let one page.
    pub fn entitlements_granted_to(
        &self,
        account_id: &str,
    ) -> BTreeMap<String, ListedEntitlement> {
        self.flows
            .values()
            .flat_map(|flow| flow.entitlements.iter())
            .filter(|entitlement| entitlement.is_granted_to(account_id))
            .map(|entitlement| {
                (
                    entitlement.arn().to_string(),
                    entitlement.to_listed_entitlement(),
                )
            })
            .collect()
    }

    /// Settle every flow, gateway, bridge, router input, router output and router network
    /// interface whose status change the clock already made due, and materialise the removal
    /// of every router input, router output and router network interface whose deletion has
    /// come due.
    pub fn settle_due_transitions(&mut self, now: SystemTime) {
        for flow in self.flows.values_mut() {
            flow.settle(now);
        }
        for gateway in self.gateways.values_mut() {
            gateway.settle(now);
        }
        for instance in self.gateway_instances.values_mut() {
            instance.settle(now);
        }
        for bridge in self.bridges.values_mut() {
            bridge.settle(now);
        }
        for router_input in self.router_inputs.values_mut() {
            router_input.settle(now);
        }
        for router_output in self.router_outputs.values_mut() {
            router_output.settle(now);
        }
        for interface in self.router_network_interfaces.values_mut() {
            interface.settle(now);
        }
        for reservation in self.reservations.values_mut() {
            reservation.settle(now);
        }
        // A router input, router output or router network interface whose removal the clock
        // has made due stops existing: the deletion was owed at that instant whether or not
        // anybody called, and none of `RouterInputState`, `RouterOutputState` and
        // `RouterNetworkInterfaceState` declares a terminal state the record could be
        // reported in afterwards (see [`StoredRouterInput::removal_due_at`],
        // [`StoredRouterOutput::removal_due_at`] and
        // [`StoredRouterNetworkInterface::removal_due_at`]).
        self.router_inputs
            .retain(|_, router_input| !router_input.is_removal_due(now));
        self.router_outputs
            .retain(|_, router_output| !router_output.is_removal_due(now));
        self.router_network_interfaces
            .retain(|_, interface| !interface.is_removal_due(now));
        // A route names a router input the store HOLDS: an input the clock has just taken
        // away carries content into nothing, so the association TakeRouterInput recorded
        // goes with it and the outputs that named it are UNROUTED again. Owed at the same
        // instant as the removal above, whether or not anybody called.
        let router_inputs = &self.router_inputs;
        for router_output in self.router_outputs.values_mut() {
            if router_output
                .routed_input_arn
                .as_ref()
                .is_some_and(|input_arn| !router_inputs.contains_key(input_arn))
            {
                router_output.routed_input_arn = None;
                router_output.routing_due_at = None;
            }
        }
    }

    /// Whether this account holds an ACTIVE reservation at `now` — the condition
    /// ListOfferings states its eligibility against: "if you have an active reservation
    /// (which means you've purchased an offering that has already started and hasn't
    /// expired yet), your account isn't eligible for other offerings"
    /// (`smithy_query(query="operation",operation="ListOfferings")`).
    ///
    /// Judged on the two INSTANTS the agreement runs between, because that sentence is
    /// what defines an active reservation: one that has STARTED and whose `End` has not
    /// passed. Reading the instants rather than the state string also means no read has to
    /// settle the store to answer — a reservation the clock has expired is past its `End`
    /// whether or not anything has written that state onto the record yet. A reservation in
    /// the terminal `CANCELED` state is no agreement any more, so it holds no bandwidth and
    /// leaves the account eligible.
    pub fn holds_active_reservation(&self, now: SystemTime) -> bool {
        self.reservations.values().any(|reservation| {
            reservation.reservation_state != RESERVATION_STATE_CANCELED
                && reservation.start_at <= now
                && now < reservation.end_at
        })
    }

    /// The router input `arn` addresses, with everything the clock already made due
    /// materialised first — the state changes it owes, and the removal of an input whose
    /// deletion has come due, which is why an input the clock has already taken away is
    /// `None` here.
    ///
    /// The one lookup every router input operation resolves an `@httpLabel` ARN through,
    /// so one request can never see a state another request would not.
    pub fn router_input_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut StoredRouterInput> {
        self.settle_due_transitions(now);
        self.router_inputs.get_mut(arn)
    }

    /// The router output `arn` addresses, with every state change the clock already made
    /// due materialised first.
    ///
    /// The router output counterpart of [`Self::router_input_mut`]: the one lookup a router
    /// output operation resolves an `@httpLabel` ARN through, so one request can never see
    /// a state another request would not.
    pub fn router_output_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut StoredRouterOutput> {
        self.settle_due_transitions(now);
        self.router_outputs.get_mut(arn)
    }

    /// How many router outputs are currently ROUTED to the router input `input_arn` —
    /// `RouterInput$RoutedOutputs`.
    ///
    /// The relation lives on the output records (`StoredRouterOutput::routed_input_arn`,
    /// which only TakeRouterInput sets), so it is counted here rather than held a second
    /// time on the input.
    pub fn routed_output_count(&self, input_arn: &str) -> i32 {
        let routed = self
            .router_outputs
            .values()
            .filter(|router_output| router_output.routed_input_arn.as_deref() == Some(input_arn))
            .count();
        i32::try_from(routed).unwrap_or(i32::MAX)
    }

    /// Why the router input `router_input` cannot be given the routing scope
    /// `routing_scope`, or `None` when every association it is currently part of survives
    /// that scope.
    ///
    /// The relation lives on the output records ([`StoredRouterOutput::routed_input_arn`]),
    /// so the outputs routed to this input are the pairs the narrower scope is judged over,
    /// through the same rule a take is judged through: an update may not leave standing an
    /// association the scope it stores forbids.
    pub fn router_input_routing_scope_rejection(
        &self,
        router_input: &StoredRouterInput,
        routing_scope: &str,
    ) -> Option<String> {
        self.router_outputs
            .values()
            .find(|router_output| {
                router_output.routed_input_arn.as_deref() == Some(router_input.arn.as_str())
                    && !routing_scopes_admit_association(
                        routing_scope,
                        &router_input.region_name,
                        &router_output.routing_scope,
                        &router_output.region_name,
                    )
            })
            .map(|router_output| {
                cross_region_association_detail(
                    &router_input.arn,
                    routing_scope,
                    &router_input.region_name,
                    &router_output.arn,
                    &router_output.routing_scope,
                    &router_output.region_name,
                )
            })
    }

    /// Why the router output `router_output` cannot be given the routing scope
    /// `routing_scope`, or `None` when the association it currently carries survives that
    /// scope — the router output side of [`Self::router_input_routing_scope_rejection`].
    pub fn router_output_routing_scope_rejection(
        &self,
        router_output: &StoredRouterOutput,
        routing_scope: &str,
    ) -> Option<String> {
        let router_input = self
            .router_inputs
            .get(router_output.routed_input_arn.as_deref()?)?;
        if routing_scopes_admit_association(
            &router_input.routing_scope,
            &router_input.region_name,
            routing_scope,
            &router_output.region_name,
        ) {
            return None;
        }
        Some(cross_region_association_detail(
            &router_input.arn,
            &router_input.routing_scope,
            &router_input.region_name,
            &router_output.arn,
            routing_scope,
            &router_output.region_name,
        ))
    }

    /// The modelled `RouterInput` the store holds for `arn`, with everything the clock
    /// already made due materialised first (as [`Self::router_input_mut`] does) and
    /// [`Self::routed_output_count`] resolved — the one projection every router input
    /// operation answers from.
    pub fn router_input_projection(&mut self, arn: &str, now: SystemTime) -> Option<RouterInput> {
        self.settle_due_transitions(now);
        let routed_outputs = self.routed_output_count(arn);
        self.router_inputs
            .get(arn)
            .map(|router_input| router_input.to_router_input(routed_outputs))
    }

    /// How many router inputs ingest on the router network interface `interface_arn` —
    /// `RouterNetworkInterface$AssociatedInputCount`, "the number of router inputs
    /// associated with the network interface".
    ///
    /// The relation lives on the input records (the `NetworkInterfaceArn` their
    /// configuration names, see [`router_input_network_interface_arn`]), so it is counted
    /// here rather than held a second time on the interface, where a create or delete of an
    /// input could leave it stale.
    pub fn associated_router_input_count(&self, interface_arn: &str) -> i32 {
        let associated = self
            .router_inputs
            .values()
            .filter(|router_input| {
                router_input_network_interface_arn(&router_input.configuration)
                    == Some(interface_arn)
            })
            .count();
        i32::try_from(associated).unwrap_or(i32::MAX)
    }

    /// How many router outputs egress from the router network interface `interface_arn` —
    /// `RouterNetworkInterface$AssociatedOutputCount`. Counted from the output records for
    /// the same reason as [`Self::associated_router_input_count`].
    pub fn associated_router_output_count(&self, interface_arn: &str) -> i32 {
        let associated = self
            .router_outputs
            .values()
            .filter(|router_output| {
                router_output_network_interface_arn(&router_output.configuration)
                    == Some(interface_arn)
            })
            .count();
        i32::try_from(associated).unwrap_or(i32::MAX)
    }

    /// Why the router network interface `interface_arn` cannot be removed from the store
    /// yet, or `None` when no router input and no router output is associated with it.
    ///
    /// A router input ingests on, and a router output egresses from, the interface its
    /// STORED configuration names in a REQUIRED member
    /// ([`router_input_network_interface_arn`], [`router_output_network_interface_arn`]) —
    /// the same two readings the two association counts above are taken from — and every
    /// operation that stores such a configuration resolves that member against this store
    /// rather than taking it on trust
    /// ([`Self::unknown_router_network_interface_rejection`]). Taking the interface away
    /// while an input or an output still names it would therefore leave records holding a
    /// reference nothing resolves, which no later call could even restate; and unlike a
    /// router output's route to a router input, which is optional and so is cleared when the
    /// input goes ([`Self::settle_due_transitions`]), the reference cannot be dropped off the
    /// record. So the removal is refused while any holder is in the store, and the caller
    /// clears the association first by deleting those records or pointing their configuration
    /// at another interface.
    pub fn associated_router_resource_rejection(&self, interface_arn: &str) -> Option<String> {
        let inputs = self.associated_router_input_count(interface_arn);
        let outputs = self.associated_router_output_count(interface_arn);
        if inputs == 0 && outputs == 0 {
            return None;
        }
        Some(format!(
            "Router network interface {interface_arn} still has {inputs} associated router \
             input(s) and {outputs} associated router output(s). Delete them, or point their \
             configuration at another router network interface, before deleting the interface."
        ))
    }

    /// Why `arn` names no router network interface this account holds, or `None` when the
    /// store holds one for it. `member` is the request member that named it, so the refusal
    /// states which reference did not resolve.
    ///
    /// A `RouterNetworkInterfaceArn` names a resource THIS service mints
    /// ([`is_mintable_router_network_interface_arn`]) and holds in
    /// [`Self::router_network_interfaces`], so a request that points at one is resolved
    /// against the store rather than taken on trust: an ARN of the right shape for an
    /// interface the account never created names nothing. The detail is the sentence the
    /// refusal's envelope carries ([`validation_failed_message`]).
    ///
    /// An interface whose deletion has already been accepted does not resolve either: the
    /// clock owes its removal whether or not anybody calls
    /// ([`StoredRouterNetworkInterface::removal_due_at`]), so a configuration stored against
    /// it would name nothing a moment later — the same unresolvable reference
    /// [`Self::associated_router_resource_rejection`] keeps a delete from creating.
    pub fn unknown_router_network_interface_rejection(
        &self,
        member: &str,
        arn: &str,
    ) -> Option<String> {
        if self
            .router_network_interfaces
            .get(arn)
            .is_some_and(|interface| !interface.is_deleting())
        {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a router network interface this account holds."
        ))
    }

    /// Why `arn` names no flow source this account holds, or `None` when the store holds
    /// one for it.
    ///
    /// A `FlowSourceArn` names a SOURCE of a flow, and a source lives on the flow record
    /// ([`StoredFlow::sources`]), so the source is resolved on the flow the same request
    /// names when it names one, and across the account's flows when it does not. `member`
    /// is the request member that named it, for the refusal detail.
    pub fn unknown_flow_source_rejection(
        &self,
        member: &str,
        flow_arn: Option<&str>,
        arn: &str,
    ) -> Option<String> {
        if self.holds_flow_source(flow_arn, arn) {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a flow source this account holds."
        ))
    }

    /// Whether the store holds the flow SOURCE `arn`: on the flow `flow_arn` when a caller
    /// names one, and across the account's flows when it does not.
    ///
    /// The crate's ONE reading of that relation: both the refusal a write path answers with
    /// ([`Self::unknown_flow_source_rejection`]) and the repair a removal performs
    /// ([`Self::clear_unresolvable_flow_references`]) ask here, so the two cannot disagree
    /// about what the account holds.
    pub fn holds_flow_source(&self, flow_arn: Option<&str>, arn: &str) -> bool {
        match flow_arn {
            Some(flow_arn) => self
                .flows
                .get(flow_arn)
                .is_some_and(|flow| flow.sources.iter().any(|source| source.source_arn == arn)),
            None => self
                .flows
                .values()
                .any(|flow| flow.sources.iter().any(|source| source.source_arn == arn)),
        }
    }

    /// Why `arn` names no flow OUTPUT this account holds, or `None` when the store holds one
    /// for it.
    ///
    /// The output-side counterpart of [`Self::unknown_flow_source_rejection`]: a
    /// `FlowOutputArn` names an OUTPUT of a flow, which lives on the flow record
    /// ([`StoredFlow::outputs`]), so it is resolved on the flow the same request names when it
    /// names one, and across the account's flows when it does not. `member` is the request
    /// member that named it, for the refusal detail.
    pub fn unknown_flow_output_rejection(
        &self,
        member: &str,
        flow_arn: Option<&str>,
        arn: &str,
    ) -> Option<String> {
        if self.holds_flow_output(flow_arn, arn) {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a flow output this account holds."
        ))
    }

    /// Whether the store holds the flow OUTPUT `arn`: on the flow `flow_arn` when a caller
    /// names one, and across the account's flows when it does not.
    ///
    /// The output-side counterpart of [`Self::holds_flow_source`], and the crate's one
    /// reading of that relation for the same reason.
    pub fn holds_flow_output(&self, flow_arn: Option<&str>, arn: &str) -> bool {
        match flow_arn {
            Some(flow_arn) => self
                .flows
                .get(flow_arn)
                .is_some_and(|flow| flow.outputs.iter().any(|output| output.output_arn == arn)),
            None => self
                .flows
                .values()
                .any(|flow| flow.outputs.iter().any(|output| output.output_arn == arn)),
        }
    }

    /// Why `arn` names no flow this account holds, or `None` when the store holds one for
    /// it. `member` is the request member that named it, for the refusal detail.
    pub fn unknown_flow_rejection(&self, member: &str, arn: &str) -> Option<String> {
        if self.holds_flow(arn) {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a flow this account holds."
        ))
    }

    /// Whether the store holds the flow `arn` — the flow-level counterpart of
    /// [`Self::holds_flow_source`] and [`Self::holds_flow_output`].
    pub fn holds_flow(&self, arn: &str) -> bool {
        self.flows.contains_key(arn)
    }

    /// Why `arn` names no bridge this account holds, or `None` when the store holds one for
    /// it. `member` is the request member that named it, for the refusal detail.
    ///
    /// The bridge-level counterpart of [`Self::unknown_flow_rejection`]: a `BridgeArn` names
    /// a resource THIS service mints ([`is_mintable_bridge_arn`]) and holds in
    /// [`Self::bridges`], so a request that points at one is resolved against the store
    /// rather than taken on trust.
    pub fn unknown_bridge_rejection(&self, member: &str, arn: &str) -> Option<String> {
        if self.holds_bridge(arn) {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a bridge this account holds."
        ))
    }

    /// Whether the store holds the bridge `arn` — the bridge-level counterpart of
    /// [`Self::holds_flow`]. A bridge leaves the store when DeleteBridge takes it out, so a
    /// reference to a deleted bridge resolves nothing — which is why that removal is refused
    /// while a stored flow source names the bridge
    /// ([`Self::bridge_gateway_source_rejection`]).
    pub fn holds_bridge(&self, arn: &str) -> bool {
        self.bridges.contains_key(arn)
    }

    /// Why `arn` names no gateway this account holds, or `None` when the store holds a live
    /// one for it. `member` is the request member that named it, for the refusal detail.
    ///
    /// The gateway-level counterpart of [`Self::unknown_bridge_rejection`]: a gateway ARN
    /// names a resource THIS service mints ([`is_mintable_gateway_arn`]) and holds in
    /// [`Self::gateways`], so a request that points at one is resolved against the store
    /// rather than taken on trust — an ARN of the right shape for a gateway the account
    /// never created names nothing.
    pub fn unknown_gateway_rejection(&self, member: &str, arn: &str) -> Option<String> {
        if self.holds_gateway(arn) {
            return None;
        }
        Some(format!(
            "{member} {arn:?} does not name a gateway this account holds."
        ))
    }

    /// Whether the store holds a LIVE gateway for `arn` — the gateway-level counterpart of
    /// [`Self::holds_bridge`].
    ///
    /// A gateway whose delete has been accepted does not count: the record is retired
    /// ([`StoredGateway::is_retired`]) on its way to the terminal `DELETED` state, and
    /// DeleteGateway only accepts a delete once nothing is placed on the gateway any more,
    /// so a record stored against a retired gateway would name something the account has
    /// already given up — the same reading [`Self::unknown_router_network_interface_rejection`]
    /// takes of an interface whose removal is already owed.
    pub fn holds_gateway(&self, arn: &str) -> bool {
        self.live_gateway(arn).is_some()
    }

    /// The LIVE gateway `arn`, or `None` when the store holds no gateway for it or holds a
    /// retired one. The crate's ONE reading of "a gateway of this account": every path that
    /// resolves a gateway reference asks here ([`Self::unknown_gateway_rejection`],
    /// [`Self::unknown_gateway_network_rejection`]), so they cannot disagree about which
    /// gateways the account has.
    pub fn live_gateway(&self, arn: &str) -> Option<&StoredGateway> {
        self.gateways
            .get(arn)
            .filter(|gateway| !gateway.is_retired())
    }

    /// Why the gateway bridge source a request would store on a FLOW SOURCE cannot be
    /// stored, or `None` when the reference it carries resolves — including when the request
    /// carries no bridge source, or a bridge source that names no bridge, because neither
    /// references anything. `prefix` is the request member that nests the source
    /// (`Sources[0]`, say; see [`qualified_member`]).
    ///
    /// `GatewayBridgeSource$BridgeArn` is "the ARN of the bridge feeding this flow", so it
    /// is a reference to a bridge of the ACCOUNT, not to a sub-resource of the flow — the
    /// account-level half of the references a source carries, whose flow-local half
    /// [`source_request_reference_rejection`] resolves. Every operation that writes one
    /// (CreateFlow's `Source` / `Sources`, AddFlowSources, UpdateFlowSource) resolves it
    /// HERE, so a stored source never names a bridge the account has not got — kept true on
    /// the removal side by [`Self::bridge_gateway_source_rejection`], and the same
    /// reading every other cross-resource ARN this store holds is resolved by
    /// ([`Self::unknown_flow_rejection`] for a bridge flow source's `FlowArn`,
    /// [`Self::unknown_router_network_interface_rejection`] for a router configuration's
    /// interface).
    pub fn source_bridge_reference_rejection(
        &self,
        prefix: &str,
        bridge_arn: Option<&str>,
    ) -> Option<String> {
        let arn = bridge_arn?;
        self.unknown_bridge_rejection(
            &qualified_member(prefix, "GatewayBridgeSource.BridgeArn"),
            arn,
        )
    }

    /// Why `network_name` names no network the gateway `placement_arn` declares, or `None`
    /// when that gateway declares one by that name. `member` is the request member that
    /// named it, for the refusal detail.
    ///
    /// A bridge member that carries a gateway network name — `BridgeNetworkSource$NetworkName`
    /// ("the network source's gateway network name") and `BridgeNetworkOutput$NetworkName`
    /// ("the network output's gateway network name") — references a network of the ONE gateway
    /// the bridge is placed on (`Bridge$PlacementArn`), and those names are declared per
    /// gateway by CreateGateway: `GatewayNetwork$Name` "is used to reference the network and
    /// must be unique among networks in this gateway". The valid set is therefore the
    /// placement gateway's own [`StoredGateway::networks`], read through the crate's one
    /// reading of a gateway reference ([`Self::live_gateway`]), and a name outside it
    /// references nothing — including every name when the store holds no live gateway for
    /// `placement_arn`, which declares no network at all. The detail is the sentence the
    /// refusal's envelope carries ([`validation_failed_message`]), as for every other
    /// reference this store resolves.
    pub fn unknown_gateway_network_rejection(
        &self,
        member: &str,
        placement_arn: &str,
        network_name: &str,
    ) -> Option<String> {
        let declared = self.live_gateway(placement_arn).is_some_and(|gateway| {
            gateway
                .networks
                .iter()
                .any(|network| network.name == network_name)
        });
        if declared {
            return None;
        }
        Some(format!(
            "{member} {network_name:?} does not name a network the gateway {placement_arn:?} \
             the bridge is placed on declares."
        ))
    }

    /// Why the bridge source `source` cannot be written onto a bridge placed on
    /// `placement_arn` because a resource it names does not exist, or `None` when every
    /// reference it carries resolves.
    ///
    /// A stored `BridgeSource` carries the two references a bridge source can hold, and each
    /// one names a resource this store holds: `BridgeFlowSource$FlowArn` ("the ARN of the
    /// cloud flow used as a source of this bridge" — a flow source "originates in
    /// MediaConnect as an existing cloud flow", so the ARN is resolved against the account's
    /// flows through [`Self::unknown_flow_rejection`]) and
    /// `BridgeNetworkSource$NetworkName` ("the network source's gateway network name",
    /// resolved against the placement gateway's own networks through
    /// [`Self::unknown_gateway_network_rejection`]). Resolving them HERE is what keeps every
    /// operation that writes a source onto a bridge the store holds answering one reference
    /// the same way. The detail is the sentence the refusal's envelope carries
    /// ([`validation_failed_message`]).
    pub fn bridge_source_reference_rejection(
        &self,
        placement_arn: &str,
        source: &BridgeSource,
    ) -> Option<String> {
        if let Some(flow_source) = source.flow_source.as_ref() {
            if let Some(rejection) =
                self.unknown_flow_rejection("FlowSource FlowArn", &flow_source.flow_arn)
            {
                return Some(rejection);
            }
        }
        let network_source = source.network_source.as_ref()?;
        self.unknown_gateway_network_rejection(
            "NetworkSource NetworkName",
            placement_arn,
            &network_source.network_name,
        )
    }

    /// The output-side counterpart of [`Self::bridge_source_reference_rejection`]: why the
    /// bridge output `output` cannot be written onto a bridge placed on `placement_arn`, or
    /// `None` when the reference it carries resolves.
    ///
    /// A network output is "delivered to your premises" over one of the networks the
    /// placement gateway declares, so `BridgeNetworkOutput$NetworkName` ("the network
    /// output's gateway network name") is resolved against that gateway's own networks. The
    /// other kind of bridge output, a `BridgeFlowOutput`, names the flow and the flow source
    /// THIS service minted when a cloud flow took the bridge as its source, so no reference
    /// a request carries reaches it.
    pub fn bridge_output_reference_rejection(
        &self,
        placement_arn: &str,
        output: &BridgeOutput,
    ) -> Option<String> {
        let network_output = output.network_output.as_ref()?;
        self.unknown_gateway_network_rejection(
            "NetworkOutput NetworkName",
            placement_arn,
            &network_output.network_name,
        )
    }

    /// Why the router input configuration `configuration` cannot be stored because a
    /// resource of THIS service it names does not exist, or `None` when every such
    /// reference resolves.
    ///
    /// A router input's configuration names what the input INGESTS ON, so a member of it that
    /// carries the ARN of a resource this service mints and holds must name one the account
    /// holds: the `NetworkInterfaceArn` of a STANDARD, FAILOVER or MERGE configuration
    /// (`smithy_query(query="shape",shape="StandardRouterInputConfiguration")`, required on
    /// each, a `routerNetworkInterface` ARN this service mints), and a MEDIACONNECT FLOW
    /// configuration's `FlowArn` / `FlowOutputArn` ("the ARN of the flow to connect to" / "the
    /// ARN of the flow output to connect to this router input"), each optional, because that
    /// configuration is stored with neither while the input is disconnected ("to disconnect
    /// it, call the update operation and omit both fields while keeping this configuration
    /// object"). The MEDIALIVE CHANNEL configuration names a resource of ANOTHER service,
    /// which this store does not hold, so there is nothing here to resolve it against. The
    /// input-side counterpart of [`Self::router_output_configuration_rejection`], and the one
    /// place every operation that stores a router input configuration resolves it, so two of
    /// them cannot answer one reference differently.
    pub fn router_input_configuration_rejection(
        &self,
        configuration: &RouterInputConfiguration,
    ) -> Option<String> {
        match configuration {
            RouterInputConfiguration::Standard(standard) => self
                .unknown_router_network_interface_rejection(
                    "Configuration Standard NetworkInterfaceArn",
                    &standard.network_interface_arn,
                ),
            RouterInputConfiguration::Failover(failover) => self
                .unknown_router_network_interface_rejection(
                    "Configuration Failover NetworkInterfaceArn",
                    &failover.network_interface_arn,
                ),
            RouterInputConfiguration::Merge(merge) => self
                .unknown_router_network_interface_rejection(
                    "Configuration Merge NetworkInterfaceArn",
                    &merge.network_interface_arn,
                ),
            RouterInputConfiguration::MediaConnectFlow(flow) => {
                // Neither member is filtered for emptiness: the modelled `FlowArn` /
                // `FlowOutputArn` patterns admit no empty string, so the boundary walk refuses
                // one before a handler runs — and an ARN that somehow arrived empty names no
                // flow this account holds, which is what the lookups answer.
                let flow_arn = flow.flow_arn.as_deref();
                if let Some(arn) = flow_arn {
                    if let Some(rejection) =
                        self.unknown_flow_rejection("Configuration MediaConnectFlow FlowArn", arn)
                    {
                        return Some(rejection);
                    }
                }
                let output_arn = flow.flow_output_arn.as_deref()?;
                self.unknown_flow_output_rejection(
                    "Configuration MediaConnectFlow FlowOutputArn",
                    flow_arn,
                    output_arn,
                )
            }
            RouterInputConfiguration::MediaLiveChannel(_) => None,
        }
    }

    /// Why the router output configuration `configuration` cannot be stored because a
    /// resource of THIS service it names does not exist, or `None` when every such
    /// reference resolves.
    ///
    /// A router output's configuration names what the output EGRESSES TO, so a member of it
    /// that carries the ARN of a resource this service mints and holds must name one the
    /// account holds: a STANDARD configuration's `NetworkInterfaceArn`
    /// (`smithy_query(query="shape",shape="StandardRouterOutputConfiguration")`, required, a
    /// `routerNetworkInterface` ARN) and a MEDIACONNECT FLOW configuration's `FlowArn` /
    /// `FlowSourceArn` ("the ARN of the flow to connect to this router output" / "the ARN of
    /// the flow source to connect to this router output"), each optional, because that
    /// configuration is stored with neither while the output is disconnected. The MEDIALIVE
    /// INPUT configuration names a resource of ANOTHER service, which this store does not
    /// hold, so there is nothing here to resolve it against. Every operation that stores a
    /// configuration (CreateRouterOutput, UpdateRouterOutput) resolves it HERE, so the two
    /// cannot answer one reference differently.
    pub fn router_output_configuration_rejection(
        &self,
        configuration: &RouterOutputConfiguration,
    ) -> Option<String> {
        match configuration {
            RouterOutputConfiguration::Standard(standard) => self
                .unknown_router_network_interface_rejection(
                    "Configuration Standard NetworkInterfaceArn",
                    &standard.network_interface_arn,
                ),
            RouterOutputConfiguration::MediaConnectFlow(flow) => {
                // Neither member is filtered for emptiness: the modelled `FlowArn` /
                // `FlowSourceArn` patterns admit no empty string, so the boundary walk
                // refuses one before a handler runs — and an ARN that somehow arrived empty
                // names no flow this account holds, which is what the lookups answer.
                let flow_arn = flow.flow_arn.as_deref();
                if let Some(arn) = flow_arn {
                    if let Some(rejection) =
                        self.unknown_flow_rejection("Configuration MediaConnectFlow FlowArn", arn)
                    {
                        return Some(rejection);
                    }
                }
                let source_arn = flow.flow_source_arn.as_deref()?;
                self.unknown_flow_source_rejection(
                    "Configuration MediaConnectFlow FlowSourceArn",
                    flow_arn,
                    source_arn,
                )
            }
            RouterOutputConfiguration::MediaLiveInput(_) => None,
        }
    }

    /// Drop every stored router reference to a flow, a flow OUTPUT or a flow SOURCE this
    /// store no longer holds, and move the `UpdatedAt` of each record a reference is dropped
    /// from.
    ///
    /// A reference a write path resolves against the store
    /// ([`Self::router_input_configuration_rejection`],
    /// [`Self::router_output_configuration_rejection`]) must not outlive what it names: a
    /// record that kept one would report a configuration no later call could restate, and the
    /// resource it points into cannot be read back from either side. Each of these references
    /// is OPTIONAL and its absence is a modelled state of the configuration — "to connect the
    /// router input to the flow output, provide both `flowArn` and `flowOutputArn`. To
    /// disconnect it … omit both fields while keeping this configuration object"
    /// (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`, and
    /// the same on `MediaConnectFlowRouterOutputConfiguration`) — so the connection goes with
    /// the resource and the holder is DISCONNECTED again, exactly as a router output's route
    /// into a router input the clock has taken away is cleared
    /// ([`Self::settle_due_transitions`]). A reference the model makes REQUIRED cannot be
    /// dropped off a record, which is why taking its target away is refused instead
    /// ([`Self::associated_router_resource_rejection`]).
    ///
    /// Called by every operation that takes a flow, a flow output or a flow source out of the
    /// store, as part of that same write: nothing else owes the repair, so no read has to
    /// perform it.
    pub fn clear_unresolvable_flow_references(&mut self, now: SystemTime) {
        // Which references still resolve is read FIRST, against the store as it stands and
        // through the same lookups the write paths resolve a configuration through; the
        // records are then rewritten, because one cannot be mutated while the store is read.
        let unresolved_inputs: Vec<(String, bool, bool)> = self
            .router_inputs
            .iter()
            .filter_map(|(arn, router_input)| {
                let RouterInputConfiguration::MediaConnectFlow(flow) = &router_input.configuration
                else {
                    return None;
                };
                let flow_arn = flow.flow_arn.as_deref();
                let drop_flow = flow_arn.is_some_and(|flow_arn| !self.holds_flow(flow_arn));
                let drop_output = flow
                    .flow_output_arn
                    .as_deref()
                    .is_some_and(|output_arn| !self.holds_flow_output(flow_arn, output_arn));
                (drop_flow || drop_output).then(|| (arn.clone(), drop_flow, drop_output))
            })
            .collect();
        for (arn, drop_flow, drop_output) in unresolved_inputs {
            let Some(router_input) = self.router_inputs.get_mut(&arn) else {
                continue;
            };
            if let RouterInputConfiguration::MediaConnectFlow(flow) =
                &mut router_input.configuration
            {
                if drop_flow {
                    flow.flow_arn = None;
                }
                if drop_output {
                    flow.flow_output_arn = None;
                }
                // The configuration the record holds changed, so its `UpdatedAt` moves with
                // it, as it does for every other write to the record.
                router_input.updated_at = now;
            }
        }

        let unresolved_outputs: Vec<(String, bool, bool)> = self
            .router_outputs
            .iter()
            .filter_map(|(arn, router_output)| {
                let RouterOutputConfiguration::MediaConnectFlow(flow) =
                    &router_output.configuration
                else {
                    return None;
                };
                let flow_arn = flow.flow_arn.as_deref();
                let drop_flow = flow_arn.is_some_and(|flow_arn| !self.holds_flow(flow_arn));
                let drop_source = flow
                    .flow_source_arn
                    .as_deref()
                    .is_some_and(|source_arn| !self.holds_flow_source(flow_arn, source_arn));
                (drop_flow || drop_source).then(|| (arn.clone(), drop_flow, drop_source))
            })
            .collect();
        for (arn, drop_flow, drop_source) in unresolved_outputs {
            let Some(router_output) = self.router_outputs.get_mut(&arn) else {
                continue;
            };
            if let RouterOutputConfiguration::MediaConnectFlow(flow) =
                &mut router_output.configuration
            {
                if drop_flow {
                    flow.flow_arn = None;
                }
                if drop_source {
                    flow.flow_source_arn = None;
                }
                router_output.updated_at = now;
            }
        }
    }

    /// Why the flow `flow_arn` cannot be taken out of the store yet, or `None` when no
    /// bridge the store holds names it.
    ///
    /// A bridge's flow source names the cloud flow it takes its content from in a REQUIRED
    /// member (`BridgeFlowSource$FlowArn`, "the ARN of the cloud flow used as a source of
    /// this bridge"), and every path that writes such a source resolves that member against
    /// this store rather than taking it on trust (CreateBridge's and AddBridgeSources'
    /// [`Self::bridge_source_reference_rejection`], UpdateBridgeSource's
    /// [`Self::unknown_flow_rejection`]). Taking the flow away while a bridge still names it
    /// would therefore leave that bridge holding a reference nothing resolves, which no later
    /// call could even restate; and unlike a router input's or a router output's connection to
    /// a flow, which the model makes optional and which a removal therefore DROPS
    /// ([`Self::clear_unresolvable_flow_references`]), a required member cannot be dropped off
    /// the record. So the removal is refused while a bridge source names the flow — the same
    /// reading of the two halves [`Self::associated_router_resource_rejection`] applies to a
    /// router network interface — and the caller clears the reference first, by removing those
    /// bridge sources, pointing them at another flow or deleting the bridges.
    pub fn flow_bridge_source_rejection(&self, flow_arn: &str) -> Option<String> {
        let bridges = self.bridge_flow_source_count(flow_arn);
        if bridges == 0 {
            return None;
        }
        Some(format!(
            "Flow {flow_arn} is still the source of {bridges} bridge(s). Remove those bridge \
             sources, point them at another flow, or delete the bridges, before deleting the \
             flow."
        ))
    }

    /// How many bridges hold a flow source naming the flow `flow_arn` — the store's ONE
    /// reading of that relation, which [`Self::flow_bridge_source_rejection`] states its
    /// refusal from.
    pub fn bridge_flow_source_count(&self, flow_arn: &str) -> usize {
        self.bridges
            .values()
            .filter(|bridge| {
                bridge.sources.iter().any(|source| {
                    source
                        .flow_source
                        .as_ref()
                        .is_some_and(|flow_source| flow_source.flow_arn == flow_arn)
                })
            })
            .count()
    }

    /// Why the bridge `bridge_arn` cannot be taken out of the store yet, or `None` when no
    /// flow source the store holds names it.
    ///
    /// The bridge-side mirror of [`Self::flow_bridge_source_rejection`], and the same reading
    /// of the same two halves: a flow source fed by a bridge names that bridge in a REQUIRED
    /// member (`GatewayBridgeSource$BridgeArn`, "the ARN of the bridge feeding this flow"), and
    /// every path that writes such a source resolves that member against this store rather
    /// than taking it on trust ([`Self::source_bridge_reference_rejection`], called by
    /// CreateFlow's `Source` / `Sources`, AddFlowSources and UpdateFlowSource). Taking the
    /// bridge away while a flow source still names it would therefore leave that source
    /// holding a reference nothing resolves ([`Self::holds_bridge`]), which no later call
    /// could even restate; and unlike the optional router references a removal DROPS
    /// ([`Self::clear_unresolvable_flow_references`]), a required member cannot be dropped off
    /// the record. So the removal is refused while a flow source names the bridge, and the
    /// caller clears the reference first, by removing those flow sources, pointing them at
    /// another bridge or deleting the flows.
    pub fn bridge_gateway_source_rejection(&self, bridge_arn: &str) -> Option<String> {
        let flows = self.gateway_bridge_source_flow_count(bridge_arn);
        if flows == 0 {
            return None;
        }
        Some(format!(
            "Bridge {bridge_arn} is still the source of {flows} flow(s). Remove those flow \
             sources, point them at another bridge, or delete the flows, before deleting the \
             bridge."
        ))
    }

    /// How many flows hold a source whose gateway bridge source names the bridge
    /// `bridge_arn` — the store's ONE reading of that relation, which
    /// [`Self::bridge_gateway_source_rejection`] states its refusal from, counting HOLDERS
    /// the way [`Self::bridge_flow_source_count`] counts them on the mirror side.
    pub fn gateway_bridge_source_flow_count(&self, bridge_arn: &str) -> usize {
        self.flows
            .values()
            .filter(|flow| {
                flow.sources.iter().any(|source| {
                    source
                        .gateway_bridge_source
                        .as_ref()
                        .is_some_and(|bridge_source| bridge_source.bridge_arn == bridge_arn)
                })
            })
            .count()
    }

    /// Why `adding` more flow sources cannot be pointed at the bridge `bridge_arn`, or
    /// `None` when they can. `prefix` is the request member that nests the source
    /// (`Sources[0]`, say; see [`qualified_member`]), exactly as
    /// [`Self::source_bridge_reference_rejection`] reads it.
    ///
    /// A flow source that names a bridge gives that bridge exactly one output — the
    /// `BridgeFlowOutput` [`Self::sync_bridge_flow_outputs`] materialises for it — and a
    /// bridge holds no more outputs than its own type declares, which for a ground-to-cloud
    /// bridge is its REQUIRED `IngressGatewayBridge$MaxOutputs` ("the maximum number of
    /// outputs on the ingress bridge"). These are precisely the outputs that declaration
    /// bounds, a bridge of that direction taking none a request configures
    /// ([`bridge_output_capacity`]), so a request that would leave the bridge holding more
    /// of them than it declares is refused — through the one function every count of a
    /// bridge's outputs is decided by ([`bridge_output_count_rejection`]), so the flow-source
    /// side, the request-configured side and the declaration side cannot drift apart.
    ///
    /// `adding` is how many flow sources the request NEWLY points at the bridge: a request
    /// that restates the bridge a stored source already names asks for no further output, so
    /// it is never refused for the output that source already has.
    ///
    /// Every operation that writes a flow source (CreateFlow's `Source` / `Sources`,
    /// AddFlowSources, UpdateFlowSource) asks HERE before it writes, with
    /// `BadRequestException` — the error the model describes as the one for "a semantic
    /// error" in the request — so nothing is stored for a request the bridge has no room
    /// for. A bridge this store does not hold bounds nothing: that reference is refused by
    /// [`Self::source_bridge_reference_rejection`] instead.
    pub fn flow_source_bridge_output_rejection(
        &self,
        prefix: &str,
        bridge_arn: &str,
        adding: usize,
    ) -> Option<String> {
        if adding == 0 {
            return None;
        }
        let bridge = self.bridges.get(bridge_arn)?;
        let detail = bridge_output_count_rejection(
            bridge
                .ingress_gateway_bridge
                .as_ref()
                .map(|ingress| ingress.max_outputs),
            bridge.outputs.len(),
            adding,
        )?;
        let member = qualified_member(prefix, "GatewayBridgeSource.BridgeArn");
        Some(format!(
            "{member} {bridge_arn:?} names a bridge with no room for the flow output a \
             source naming it is given. {detail}"
        ))
    }

    /// Make the flow outputs the stored bridges hold exactly the ones the account's cloud
    /// flows ask for: one per flow source that names a bridge this store holds.
    ///
    /// A flow source's `GatewayBridgeSource` is "the source configuration for cloud flows
    /// receiving a stream from a bridge", and the stream a bridge delivers into the cloud is
    /// an output OF THE BRIDGE — a `BridgeFlowOutput`, "the output of the bridge. A flow
    /// output is delivered to the Amazon Web Services cloud." So that output is not a
    /// resource a request configures (no request shape in this model carries one, which is
    /// why a ground-to-cloud bridge takes no output in a request at all — see
    /// [`request_bridge_output_rejection`]): it is the service's own record of the flow
    /// source that named the bridge, and it exists exactly while that flow source does.
    ///
    /// Reconciled rather than written by hand at each call site, so one function decides
    /// which flow outputs a bridge holds: every operation that writes a flow source
    /// (CreateFlow's `Source` / `Sources`, AddFlowSources, UpdateFlowSource) and every one
    /// that takes a flow or a flow source out of the store (RemoveFlowSource, DeleteFlow)
    /// calls it as part of that same write, and it both materialises the outputs the flows
    /// now ask for ([`materialise_bridge_flow_output`]) and drops the ones they no longer do
    /// — the counterpart, for an output the model's members make unrestatable by any
    /// request, of the reference repair [`Self::clear_unresolvable_flow_references`]
    /// performs. Nothing else owes the work, so no reporting operation performs it.
    ///
    /// IDEMPOTENT: an output already materialised for a flow source keeps the name it was
    /// materialised under, so repeated calls neither duplicate nor rename one. A network
    /// output a request configured is left alone, whichever bridge holds it.
    ///
    /// The outputs this materialises are ADMITTED before the write that asks for them: a
    /// bridge holds no more outputs than its own type declares, so every one of those
    /// operations asks [`Self::flow_source_bridge_output_rejection`] first and nothing is
    /// stored for a flow source a bridge has no room for.
    pub fn sync_bridge_flow_outputs(&mut self) {
        // What the flows ask for, read FIRST against the store as it stands — the bridges
        // cannot be rewritten while it is read. The flows and their sources are walked in
        // the store's own order, so which name an output materialised in one call takes does
        // not depend on the order the calls arrived in.
        let bridges = &self.bridges;
        let requested: Vec<(String, String, String, String)> = self
            .flows
            .values()
            .flat_map(|flow| {
                flow.sources.iter().filter_map(move |source| {
                    let bridge = source.gateway_bridge_source.as_ref()?;
                    bridges.contains_key(&bridge.bridge_arn).then(|| {
                        (
                            bridge.bridge_arn.clone(),
                            flow.flow_arn.clone(),
                            source.source_arn.clone(),
                            source.name.clone(),
                        )
                    })
                })
            })
            .collect();

        // A flow output no flow asks for any more goes with the flow source that asked for
        // it: it names that source in a REQUIRED member (`BridgeFlowOutput$FlowSourceArn`),
        // so an output that outlived it would report a source the account has not got. What
        // the flows ask for is indexed first, so deciding one output costs a lookup rather
        // than a walk of every flow source the account holds.
        let requested_keys: BTreeSet<(&str, &str, &str)> = requested
            .iter()
            .map(|(bridge_arn, flow_arn, source_arn, _)| {
                (
                    bridge_arn.as_str(),
                    flow_arn.as_str(),
                    source_arn.as_str(),
                )
            })
            .collect();
        for (bridge_arn, bridge) in self.bridges.iter_mut() {
            bridge.outputs.retain(|output| {
                let Some(flow_output) = output.flow_output.as_ref() else {
                    return true;
                };
                requested_keys.contains(&(
                    bridge_arn.as_str(),
                    flow_output.flow_arn.as_str(),
                    flow_output.flow_source_arn.as_str(),
                ))
            });
        }

        // And an output a flow asks for that its bridge has not got yet is materialised,
        // named against the names that bridge already addresses its outputs by. What each
        // bridge already holds is indexed the same way, and kept current as outputs are
        // added, so one call's work grows with the flow sources it finds and not with their
        // square.
        let mut held: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
        for (bridge_arn, bridge) in self.bridges.iter() {
            for output in &bridge.outputs {
                if let Some(flow_output) = output.flow_output.as_ref() {
                    held.insert((
                        bridge_arn.as_str(),
                        flow_output.flow_arn.as_str(),
                        flow_output.flow_source_arn.as_str(),
                    ));
                }
            }
        }
        let missing: Vec<&(String, String, String, String)> = requested
            .iter()
            .filter(|(bridge_arn, flow_arn, source_arn, _)| {
                !held.contains(&(bridge_arn.as_str(), flow_arn.as_str(), source_arn.as_str()))
            })
            .collect();
        for (bridge_arn, flow_arn, source_arn, source_name) in missing {
            let Some(bridge) = self.bridges.get_mut(bridge_arn) else {
                continue;
            };
            let name = resolve_bridge_flow_output_name(source_name, &bridge.output_names());
            bridge
                .outputs
                .push(materialise_bridge_flow_output(flow_arn, source_arn, name));
        }
    }

    /// Make the outputs the stored flows hold for bridges exactly the ones the account's
    /// bridges ask for: one per bridge flow source that names a flow this store holds, with
    /// the ARN of that output on the source that asked for it.
    ///
    /// A bridge's flow source "originates in MediaConnect as an existing cloud flow"
    /// (`BridgeFlowSource`), and the stream that flow delivers to the bridge leaves the flow
    /// through an output of it: `BridgeFlowSource$OutputArn` is "the Amazon Resource Number
    /// (ARN) of the output" and `Output$BridgeArn` is "the ARN of the bridge added to this
    /// output", the two ends of that one record. So the output is not a resource a request
    /// configures — `AddBridgeFlowSourceRequest` carries no output member at all — it is the
    /// service's own record of the bridge source that named the flow, and it exists exactly
    /// while that bridge source does.
    ///
    /// The mirror of [`Self::sync_bridge_flow_outputs`], which does the same for the output a
    /// BRIDGE is given when a cloud flow takes it as a source, and reconciled the same way
    /// rather than written by hand at each call site: every operation that writes a bridge
    /// source (CreateBridge's `Sources`, AddBridgeSources, UpdateBridgeSource) and every one
    /// that takes a bridge or a bridge source out of the store (RemoveBridgeSource,
    /// DeleteBridge) calls it as part of that same write, so one function decides which of a
    /// flow's outputs exist for a bridge. Nothing else owes the work, so no reporting
    /// operation performs it.
    ///
    /// IDEMPOTENT: an output already materialised for a bridge source keeps the name and the
    /// ARN it was materialised under, so repeated calls neither duplicate, rename nor remint
    /// one. An output a request configured is left alone, whichever flow holds it.
    ///
    /// An output this drops takes the stored references to it with it: the reconciliation ends
    /// with the one repair every operation that takes a flow output out of the store performs
    /// ([`Self::clear_unresolvable_flow_references`]), so a router record connected to the
    /// output a bridge source asked for is DISCONNECTED when that source stops asking rather
    /// than left holding an ARN nothing resolves.
    pub fn sync_bridge_source_flow_outputs(&mut self, region: &str, now: SystemTime) {
        // What the bridges ask for, read FIRST against the store as it stands — the flows
        // cannot be rewritten while it is read. The bridges and their sources are walked in
        // the store's own order, so which name an output materialised in one call takes does
        // not depend on the order the calls arrived in. A source whose flow this store does
        // not hold asks for nothing: there is no flow to put an output on.
        let flows = &self.flows;
        let requested: Vec<(String, BridgeSourceOrigin)> = self
            .bridges
            .values()
            .flat_map(|bridge| {
                let bridge_arn = bridge.bridge_arn.clone();
                bridge.sources.iter().filter_map(move |source| {
                    let flow_source = source.flow_source.as_ref()?;
                    flows.contains_key(&flow_source.flow_arn).then(|| {
                        (
                            flow_source.flow_arn.clone(),
                            BridgeSourceOrigin {
                                bridge_arn: bridge_arn.clone(),
                                source_name: flow_source.name.clone(),
                            },
                        )
                    })
                })
            })
            .collect();

        // An output no bridge source asks for any more goes with the source that asked for
        // it: it carries that bridge's ARN and nothing else records why it exists, so an
        // output that outlived the source would report a bridge the flow no longer feeds.
        // What the bridges ask for is indexed first, so deciding one output costs a lookup
        // rather than a walk of every bridge source the account holds.
        let requested_keys: BTreeSet<(&str, &str, &str)> = requested
            .iter()
            .map(|(flow_arn, origin)| {
                (
                    flow_arn.as_str(),
                    origin.bridge_arn.as_str(),
                    origin.source_name.as_str(),
                )
            })
            .collect();
        for (flow_arn, flow) in self.flows.iter_mut() {
            flow.outputs.retain(|output| {
                let Some(origin) = output.bridge_source.as_ref() else {
                    return true;
                };
                requested_keys.contains(&(
                    flow_arn.as_str(),
                    origin.bridge_arn.as_str(),
                    origin.source_name.as_str(),
                ))
            });
        }

        // And an output a bridge source asks for that its flow has not got yet is
        // materialised, named after that source and against the names the flow already
        // addresses its outputs by ("the name of the output … must be unique within the
        // current flow"). The ARN of each one is kept here, keyed by the source it belongs
        // to, so the sources can be filled in below without re-walking the flows.
        let mut minted: BTreeMap<(String, String, String), String> = BTreeMap::new();
        for (flow_arn, flow) in self.flows.iter() {
            for output in &flow.outputs {
                if let Some(origin) = output.bridge_source.as_ref() {
                    minted.insert(
                        (
                            flow_arn.clone(),
                            origin.bridge_arn.clone(),
                            origin.source_name.clone(),
                        ),
                        output.output_arn.clone(),
                    );
                }
            }
        }
        for (flow_arn, origin) in &requested {
            let key = (
                flow_arn.clone(),
                origin.bridge_arn.clone(),
                origin.source_name.clone(),
            );
            if minted.contains_key(&key) {
                continue;
            }
            let suffix = self.next_suffix();
            let Some(flow) = self.flows.get_mut(flow_arn) else {
                continue;
            };
            let name = next_free_name(&origin.source_name, &flow.output_names());
            let output = StoredOutput::for_bridge_source(origin.clone(), name, region, suffix);
            minted.insert(key, output.output_arn.clone());
            flow.outputs.push(output);
        }

        // Each bridge flow source then carries the ARN of the output materialised for it,
        // read back out of the flow that holds it rather than minted a second time — so the
        // two halves of the record cannot disagree, and a source whose flow this store does
        // not hold carries no ARN at all.
        for bridge in self.bridges.values_mut() {
            let bridge_arn = bridge.bridge_arn.clone();
            for source in bridge.sources.iter_mut() {
                if let Some(flow_source) = source.flow_source.as_mut() {
                    flow_source.output_arn = minted
                        .get(&(
                            flow_source.flow_arn.clone(),
                            bridge_arn.clone(),
                            flow_source.name.clone(),
                        ))
                        .cloned();
                }
            }
        }

        // And a reference held elsewhere in the store to an output this call dropped goes with
        // that output: a router record may be connected to any output of a flow, and the
        // connection is an optional member whose absence is the modelled disconnected state.
        self.clear_unresolvable_flow_references(now);
    }

    /// Why the output `output_arn` of the flow `flow_arn` cannot be removed by a request, or
    /// `None` when it can — including when the flow holds no output by that ARN, which is a
    /// resource the caller is told does not exist instead.
    ///
    /// The mirror of [`Self::bridge_flow_output_removal_rejection`], and the same reading: an
    /// output carrying a bridge's ARN is the service's own record of a bridge source that
    /// takes this flow ([`Self::sync_bridge_source_flow_outputs`]), and no request shape in
    /// this model configures one — so a caller that removed it could not ask for it back, and
    /// the bridge source that asked for it would still be naming the flow. The removal is
    /// refused and the caller clears the bridge source instead, by removing it, pointing it
    /// at another flow or deleting the bridge. An output a request DID configure is removable
    /// by the request that addresses it.
    pub fn flow_bridge_output_removal_rejection(
        &self,
        flow_arn: &str,
        output_arn: &str,
    ) -> Option<String> {
        let flow = self.flows.get(flow_arn)?;
        let output = flow
            .outputs
            .iter()
            .find(|output| output.output_arn == output_arn)?;
        let origin = output.bridge_source.as_ref()?;
        Some(format!(
            "Output {:?} is the output this flow was given when bridge {} took it as the \
             source {:?}. A request configures no such output: remove that bridge source, \
             point it at another flow, or delete the bridge.",
            output.name, origin.bridge_arn, origin.source_name
        ))
    }

    /// Why the output `output_name` of the bridge `bridge_arn` cannot be removed by a
    /// request, or `None` when it can — including when the bridge holds no output by that
    /// name, which is a resource the caller is told does not exist instead.
    ///
    /// A bridge's flow output is the service's own record of a cloud flow that takes the
    /// bridge as its source ([`Self::sync_bridge_flow_outputs`]), and no request shape in
    /// this model configures one: a caller that removed it could not ask for it back, and the
    /// flow source that asked for it would still be naming the bridge. So the removal is
    /// refused — the same reading as for the other reference this store will not let a
    /// request strand ([`Self::flow_bridge_source_rejection`]) — and the caller clears the
    /// flow source instead. An output a request DID configure, a network output, is removable
    /// by the request that addresses it.
    pub fn bridge_flow_output_removal_rejection(
        &self,
        bridge_arn: &str,
        output_name: &str,
    ) -> Option<String> {
        let bridge = self.bridges.get(bridge_arn)?;
        let position = bridge.output_position(output_name)?;
        let flow_output = bridge.outputs.get(position)?.flow_output.as_ref()?;
        Some(format!(
            "Output {output_name:?} is the flow output this bridge was given when flow {} \
             took it as the source of {}. A request configures network outputs only: remove \
             that flow source, point it at another bridge, or delete the flow.",
            flow_output.flow_arn, flow_output.flow_source_arn
        ))
    }

    /// The modelled `RouterNetworkInterface` the store holds for `arn`, with both
    /// association counts resolved — the one projection every router network interface
    /// operation answers from, so no two of them can report one interface differently.
    ///
    /// Does NOT settle the clock: a caller that reports an interface settles the store once
    /// ([`Self::settle_due_transitions`]) and then projects, so one request cannot see a
    /// state another request would not.
    pub fn router_network_interface_projection(&self, arn: &str) -> Option<RouterNetworkInterface> {
        let interface = self.router_network_interfaces.get(arn)?;
        Some(interface.to_router_network_interface(
            self.associated_router_input_count(arn),
            self.associated_router_output_count(arn),
        ))
    }

    /// The modelled `ListedRouterNetworkInterface` summary the store holds for `arn`, with
    /// both association counts resolved the same way
    /// [`Self::router_network_interface_projection`] resolves them — so the summary a listing
    /// reports and the resource a read reports can never disagree about one interface.
    ///
    /// Does NOT settle the clock, for the same reason the full projection does not: a caller
    /// settles the store once ([`Self::settle_due_transitions`]) and then projects every
    /// record it reports.
    pub fn listed_router_network_interface_projection(
        &self,
        arn: &str,
    ) -> Option<ListedRouterNetworkInterface> {
        let interface = self.router_network_interfaces.get(arn)?;
        Some(interface.to_listed_router_network_interface(
            self.associated_router_input_count(arn),
            self.associated_router_output_count(arn),
        ))
    }

    /// How many LIVE gateways the store holds — the count [`MAX_GATEWAYS`] bounds.
    ///
    /// A retired (deleted) gateway stays in the map so it remains readable in its terminal
    /// `DELETED` state, but it no longer occupies one of the account's gateway slots: the
    /// ceiling counts the gateways that exist, so deleting one makes room for another.
    pub fn live_gateway_count(&self) -> usize {
        self.gateways
            .values()
            .filter(|gateway| !gateway.is_retired())
            .count()
    }

    /// Register an instance with the gateway `gateway_arn` names, and hand back the record
    /// as it is now STORED — read back out of the map after the insert, so the ARN a caller
    /// sees is the key the store holds.
    ///
    /// This is the OUT-OF-BAND registration channel described on [`StoredGatewayInstance`]:
    /// the model declares no operation that registers an instance, because on AWS the
    /// MediaConnect Gateway software's SSM install on the on-premises appliance is what
    /// registers one. Nothing a client sends over the wire reaches here; the gateway
    /// operations only report, update and deregister what this recorded.
    ///
    /// A newly registered instance is not ready yet: it is reported in the non-terminal
    /// [`INSTANCE_STATE_REGISTERING`], which the clock settles into
    /// [`INSTANCE_STATE_ACTIVE`] one [`TRANSITION_SPAN`] later — the same shape as the
    /// gateway's own `CREATING` -> `ACTIVE`. It can host new bridges
    /// ([`BRIDGE_PLACEMENT_AVAILABLE`]) and it is connected to the service, which is what
    /// having just registered means.
    pub fn register_gateway_instance(
        &mut self,
        gateway_arn: &str,
        now: SystemTime,
    ) -> StoredGatewayInstance {
        let instance_id = gateway_instance_id(self.next_suffix());
        let instance_arn = gateway_instance_arn(gateway_arn, &instance_id);
        self.gateway_instances
            .entry(instance_arn.clone())
            .or_insert_with(|| StoredGatewayInstance {
                gateway_instance_arn: instance_arn,
                gateway_arn: gateway_arn.to_string(),
                instance_id,
                instance_state: INSTANCE_STATE_REGISTERING.to_string(),
                bridge_placement: BRIDGE_PLACEMENT_AVAILABLE.to_string(),
                connection_status: CONNECTION_STATUS_CONNECTED.to_string(),
                pending: Some(PendingStatus {
                    status: INSTANCE_STATE_ACTIVE.to_string(),
                    due_at: now + TRANSITION_SPAN,
                }),
            })
            .clone()
    }

    /// How many bridges are RUNNING on `instance` — `GatewayInstance$RunningBridgeCount`,
    /// and the count DeregisterGatewayInstance's documented precondition is stated against
    /// ("before you deregister an instance, all bridges running on the instance must be
    /// stopped").
    ///
    /// DERIVED from the bridge store rather than held on the instance record: a bridge
    /// names the gateway it is placed on (`ListedBridge$PlacementArn`: "the ARN of the
    /// gateway associated with the bridge") and carries its own `BridgeState`, so the
    /// bridges running on a gateway are the bridges placed on it that are not stopped —
    /// the same reading of "stopped" DeleteBridge's precondition uses
    /// ([`is_stopped_bridge_state`]). This emulator runs no gateway appliance and so models
    /// no per-instance deployment of a gateway's bridges: a bridge running on a gateway is
    /// counted on each of that gateway's registered instances. An instance that has reached
    /// the terminal `DEREGISTERED` state is no longer part of its gateway and hosts
    /// nothing, so its count is zero.
    pub fn running_bridge_count(&self, instance: &StoredGatewayInstance) -> i32 {
        if !instance.is_registered() {
            return 0;
        }
        let running = self
            .bridges
            .values()
            .filter(|bridge| {
                bridge.placement_arn == instance.gateway_arn
                    && !is_stopped_bridge_state(&bridge.bridge_state)
            })
            .count();
        i32::try_from(running).unwrap_or(i32::MAX)
    }

    /// The modelled `GatewayInstance` the store holds for `arn`, with its running-bridge
    /// count resolved — the one projection every gateway-instance operation answers from,
    /// so no two of them can report one instance differently.
    ///
    /// Does NOT settle the clock: a caller settles the store once
    /// ([`Self::settle_due_transitions`]) and then projects, so one request cannot see a
    /// state another request would not.
    pub fn gateway_instance_projection(&self, arn: &str) -> Option<GatewayInstance> {
        let instance = self.gateway_instances.get(arn)?;
        Some(instance.to_gateway_instance(self.running_bridge_count(instance)))
    }

    /// The instances still REGISTERED with the gateway `gateway_arn` names — the ones
    /// DeleteGateway's documented precondition requires to be deregistered first ("before
    /// you can delete a gateway, you must deregister its instances and delete its
    /// bridges").
    ///
    /// A deregistered instance stays readable in its terminal `DEREGISTERED` state but is
    /// no longer one of the gateway's instances, exactly as a retired gateway is no longer
    /// one of the account's ([`Self::live_gateway_count`]).
    pub fn registered_gateway_instance_count(&self, gateway_arn: &str) -> usize {
        self.gateway_instances
            .values()
            .filter(|instance| instance.gateway_arn == gateway_arn && instance.is_registered())
            .count()
    }

    /// The bridges still PLACED on the gateway `gateway_arn` names — the ones DeleteGateway's
    /// documented precondition requires to be deleted first ("before you can delete a
    /// gateway, you must deregister its instances and delete its bridges").
    ///
    /// A bridge names the gateway it is placed on by its placement ARN
    /// (`ListedBridge$PlacementArn`: "the ARN of the gateway associated with the bridge"),
    /// and a bridge that has reached its terminal `DELETED` state stays readable but is no
    /// longer one of the gateway's bridges ([`StoredBridge::is_placed`]) — the SAME reading
    /// of "its <children>" the instance half of that one sentence uses
    /// ([`Self::registered_gateway_instance_count`]), so the two halves cannot word the
    /// precondition differently.
    pub fn placed_bridge_count(&self, gateway_arn: &str) -> usize {
        self.bridges
            .values()
            .filter(|bridge| bridge.placement_arn == gateway_arn && bridge.is_placed())
            .count()
    }

    /// Drop the oldest retired gateway records once the store holds more than
    /// [`MAX_RETIRED_GATEWAYS`] of them, so a create-then-delete loop cannot grow the
    /// gateway map without limit.
    ///
    /// Called from DeleteGateway — the call that retires a record — and never from a
    /// read: what a reporting operation reports is the record the store holds.
    pub fn evict_oldest_retired_gateways(&mut self) {
        let mut retired: Vec<(SystemTime, String)> = self
            .gateways
            .values()
            .filter_map(|gateway| {
                gateway
                    .deleted_at
                    .map(|deleted_at| (deleted_at, gateway.gateway_arn.clone()))
            })
            .collect();
        if retired.len() <= MAX_RETIRED_GATEWAYS {
            return;
        }
        // Oldest first, so the records dropped are the ones retired longest ago; the ARN
        // breaks a tie between two retired in the same instant.
        retired.sort();
        let excess = retired.len() - MAX_RETIRED_GATEWAYS;
        for (_, gateway_arn) in retired.into_iter().take(excess) {
            self.gateways.remove(&gateway_arn);
        }
    }
}
