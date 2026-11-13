// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmonitor-networkmonitor service.
//!
//! CloudWatch Network Monitor's primary resource is the MONITOR, and a monitor
//! CONTAINS its probes, so the store holds exactly one record per monitor with
//! its probes nested inside it:
//!
//! * [`Monitor`] — what `CreateMonitor` inserts, `GetMonitor` / `ListMonitors`
//!   report and `DeleteMonitor` removes.
//! * [`Probe`] — what `CreateProbe` attaches to a monitor and `DeleteProbe`
//!   starts tearing down ([`Probe::start_teardown`]), which leaves the record in
//!   place until the clock has carried its teardown through. There is no second
//!   probe index: `GetMonitor`'s `probes` list is the stored map, so a probe can
//!   never appear in one place and not the other.
//!
//! Those records are held PER REGION
//! ([`ComAmazonawsNetworkmonitorNetworkmonitorState`]): the service answers on a
//! per-region endpoint and every ARN it mints names one region, so a request
//! reaches only the records of the region it was signed for
//! ([`request_region`]).
//!
//! Every value a handler reports is read back out of these records; nothing is
//! echoed straight from a request without being stored first.
//!
//! ## Facts this service RESOLVES rather than holds
//!
//! A probe's source is a VPC SUBNET, which is not a resource of this service:
//! which subnets the account holds, and which VPC each one sits in, are facts of
//! Amazon EC2's own inventory. So every operation that takes a `sourceArn`
//! resolves it there ([`resolve_source_subnet`]) — reached the way any emulated
//! service reaches another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — and refuses a probe
//! whose source EC2 holds no subnet for, while the `vpcId` a probe reports is
//! read off what EC2 answered rather than invented. Every refusal built there is
//! grounded in what EC2 REPORTED: an emulator with no EC2 to ask knows nothing
//! about the subnet, and nothing is what it refuses on.
//!
//! ## How a refusal this service authors is stated
//!
//! A rule the model states in prose but its constraints cannot express is
//! applied by the service, so the service also has to STATE the refusal. It
//! states it the way AWS states a constraint failure — the
//! `N validation error(s) detected: Value '<submitted>' at '<member path>'
//! failed to satisfy constraint: <requirement>` construction, built from the
//! request — and it builds it through the framework's OWN clause renderer
//! ([`stated_violation`], [`stated_requirement`]), so a refusal a handler raises
//! reads exactly like the one the boundary validator renders for a modelled
//! constraint. A refusal whose AWS sentence is known as a whole
//! ([`TCP_PORT_REQUIRED_MESSAGE`], [`aggregation_period_message`],
//! [`monitor_not_found_message`]) keeps that sentence instead.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_schema_server::constraints::{envelope, path, Violation, ViolationKind};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `networkmonitor`").
const ARN_NAMESPACE: &str = "networkmonitor";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `MonitorState` of a monitor that is provisioning the probes it holds and is
/// not aggregating metrics yet (model enum `MonitorState`). TRANSIENT: the clock
/// owes such a monitor the [`MONITOR_STATE_ACTIVE`] transition
/// ([`ACTIVATION_AFTER`]).
pub const MONITOR_STATE_PENDING: &str = "PENDING";
/// `MonitorState` of a monitor whose probes are live.
pub const MONITOR_STATE_ACTIVE: &str = "ACTIVE";
/// `MonitorState` of a monitor that has no probe in service.
///
/// A monitor aggregates what its probes measure, so one with nothing measuring
/// has nothing to activate: it is reported `INACTIVE` straight away and stays
/// there — nothing is due — until a probe of its own is in service again. This
/// is the state a monitor created with an empty (or absent) `probes` list is in,
/// the one a monitor returns to once its last probe has gone, and the one it
/// returns to when every probe it holds has been deactivated
/// ([`Monitor::follow_probe_states`]). A monitor already in it stays in it when a
/// probe of its own is deleted: taking a probe away gains a monitor nothing to
/// aggregate.
pub const MONITOR_STATE_INACTIVE: &str = "INACTIVE";

/// `ProbeState` of a probe that is not sending packets yet (model enum
/// `ProbeState`). SERVICE-ASSIGNED: the service puts a probe here while it
/// provisions, so a caller cannot ask for it ([`SETTABLE_PROBE_STATES`]).
pub const PROBE_STATE_PENDING: &str = "PENDING";
/// `ProbeState` of a live probe.
pub const PROBE_STATE_ACTIVE: &str = "ACTIVE";
/// `ProbeState` of a probe a caller has switched off: it keeps its
/// configuration but sends no packets.
pub const PROBE_STATE_INACTIVE: &str = "INACTIVE";
/// `ProbeState` of a probe whose teardown has been accepted and is under way
/// (model enum `ProbeState`). SERVICE-ASSIGNED and TRANSIENT: an accepted
/// `DeleteProbe` puts the probe here and the clock owes it
/// [`PROBE_STATE_DELETED`] ([`PROBE_TEARDOWN_STAGE`]).
pub const PROBE_STATE_DELETING: &str = "DELETING";
/// `ProbeState` of a probe whose teardown has finished. SERVICE-ASSIGNED and
/// TRANSIENT: the record is still readable, and the clock owes its removal one
/// more [`PROBE_TEARDOWN_STAGE`], after which a read of the probe reports it
/// not found.
pub const PROBE_STATE_DELETED: &str = "DELETED";

/// The `ProbeState` values a CALLER may ask a probe to be put into.
///
/// The model's `ProbeState` enum is the catalog of states a probe can be
/// REPORTED in (`PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED`),
/// and most of those the service assigns itself over a probe's lifecycle: it
/// provisions through `PENDING`, records its own failure as `ERROR` and tears a
/// probe down through `DELETING` / `DELETED`. The only states a request names
/// are the two halves of the documented activate/deactivate toggle, so
/// `UpdateProbeInput$state` ("The state of the probe update") admits exactly
/// these; see [`validate_settable_probe_state`].
pub const SETTABLE_PROBE_STATES: &[&str] = &[PROBE_STATE_ACTIVE, PROBE_STATE_INACTIVE];

/// `AddressFamily` of an IPv4 destination (model enum `AddressFamily`).
pub const ADDRESS_FAMILY_IPV4: &str = "IPV4";
/// `AddressFamily` of an IPv6 destination.
pub const ADDRESS_FAMILY_IPV6: &str = "IPV6";

/// The `Protocol` that makes `destinationPort` required, as the model documents
/// for `ProbeInput$destinationPort`: "This is required only if the `protocol` is
/// `TCP` and must be a number between `1` and `65536`."
pub const PROTOCOL_TCP: &str = "TCP";

/// The `aggregationPeriod` values the service accepts, from the model doc for
/// `CreateMonitorInput$aggregationPeriod`: "Valid values are either `30` or
/// `60`. `60` is the default if no period is chosen." The shape's own
/// `@range(min: 30)` admits more, so the handler applies this list.
pub const AGGREGATION_PERIODS: &[i64] = &[30, 60];

/// The `aggregationPeriod` a monitor gets when the create request named none —
/// the documented default (same doc sentence as [`AGGREGATION_PERIODS`]).
pub const DEFAULT_AGGREGATION_PERIOD: i64 = 60;

/// The refusal message for the cross-member rule the model states for both the
/// create and the update path: `ProbeInput$destinationPort` is "required only if
/// the `protocol` is `TCP`", and `UpdateProbeInput$protocol` — "If the protocol
/// is `TCP`, then `port` is also required".
///
/// A FIXED sentence determined by that rule rather than by the request: it names
/// no value the caller sent, so every operation that applies the rule
/// ([`validate_probe_spec`], reached by `CreateMonitor`, `CreateProbe` and
/// `UpdateProbe`) emits this one sentence.
pub const TCP_PORT_REQUIRED_MESSAGE: &str = "Destination port must be provided for TCP protocol.";

/// The smallest `destinationPort` the service accepts, from the model doc for
/// `ProbeInput$destinationPort` ("must be a number between `1` and `65536`").
/// The shape's `@range(min: 0, max: 65536)` admits `0`, so the handler applies
/// the documented floor.
pub const MIN_DESTINATION_PORT: i32 = 1;

/// What a probe `destination` has to be, as a refusal's clause states the
/// requirement: the model documents `ProbeInput$destination` as "The destination
/// IP address. This must be either `IPV4` or `IPV6`" and
/// `UpdateProbeInput$destination` as "The updated IP address for the probe
/// destination. This must be either an IPv4 or IPv6 address", and the shape
/// carries only `@length(1..255)`, so the format is the service's to check.
const DESTINATION_REQUIREMENT: &str = "Member must be an IPv4 or IPv6 address";

/// How long after the call that adds the first probe to a monitor the clock owes
/// that monitor, and that probe, the `PENDING` -> `ACTIVE` transition.
///
/// A CHOSEN span, not a documented one: AWS takes as long as provisioning the
/// probe's network path needs. It is deliberately non-zero, so a caller that
/// reads the monitor straight after creating it still observes `PENDING`, and
/// the due instant is stored on the record ([`Monitor::activates_at`],
/// [`Probe::activates_at`]) so no handler ever sleeps for it.
pub const ACTIVATION_AFTER: Duration = Duration::from_millis(500);

/// How long one stage of a probe's TEARDOWN lasts: the span from the accepted
/// delete to the `DELETING` -> `DELETED` transition, and the span from there to
/// the record being forgotten.
///
/// A CHOSEN span, not a documented one: AWS takes as long as releasing the
/// probe's network path needs. It is deliberately non-zero, so a caller that
/// reads the probe straight after deleting it still observes the teardown
/// states, and the due instant is stored on the record ([`Probe::deletes_at`])
/// so no handler ever sleeps for it.
pub const PROBE_TEARDOWN_STAGE: Duration = Duration::from_millis(300);

/// The HTTP status a successful TAG MUTATION answers with.
///
/// Both tag mutations leave their success code defaulted in the model (`@http` on
/// `TagResource` and on `UntagResource` carries no `code:`, i.e. `codeDefaulted`), so
/// Smithy's default of 200 is not a statement about this service: the success status is
/// a behavioural fact and the recorded AWS responses are what state it. A tag mutation
/// writes only the tag set of the resource its `resourceArn` names and reports nothing
/// back about it — `TagResourceOutput` and `UntagResourceOutput` declare no members —
/// so there is no entity for a caller to parse and the answer is 204 No Content. A 200
/// there promises a body the response does not carry, and every client path gated on
/// the no-content status (conditional body parsing, caches and proxies that key on it)
/// takes the wrong branch. The two operations share the `/tags/{resourceArn}` route and
/// this one status, so the rule lives here rather than twice in the handlers.
///
/// This does NOT extend to every output structure that happens to declare no members:
/// `DeleteMonitor` and `DeleteProbe` report nothing either, and the recorded AWS
/// responses answer those with the defaulted 200, which they keep.
pub const TAG_MUTATION_SUCCESS_STATUS: u16 = 204;

/// The page size `ListMonitors` uses when the caller names no `maxResults`: the
/// model's own `MaxResults` maximum (`@range(min: 1, max: 25)`), so one
/// unpaginated call returns as much as a paginated one could.
pub const MAX_PAGE_SIZE: usize = 25;

/// The most monitors this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every request can add to it, so the service answers the modelled
/// `ServiceQuotaExceededException` past this point rather than growing without
/// bound. It bounds the store as a whole, across every region it holds. AWS
/// enforces a per-account, per-region monitor quota of its own and answers that
/// same modelled error.
pub const MAX_MONITORS: usize = 500;

/// The most probes this emulator keeps in ONE monitor. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_MONITORS`]; AWS caps the probes of a
/// monitor too. Like every probe-count ceiling this service applies it is
/// refused as a `ValidationException` (see [`PROBES_PER_SUBNET_MESSAGE`]).
pub const MAX_PROBES_PER_MONITOR: usize = 100;

/// The most probes one monitor holds in ONE source subnet: AWS's Network
/// Monitor quota "Number of probes per subnet for each monitor", whose default
/// value this is. The limit is per (monitor, source subnet) pair, so probes
/// spread over separate subnets of the same monitor do not share it.
pub const MAX_PROBES_PER_SUBNET: usize = 4;

/// What this service states when a request would leave a monitor holding more
/// than [`MAX_PROBES_PER_SUBNET`] probes in one source subnet.
///
/// A whole sentence rather than a constraint clause: the refusal is the one AWS
/// states for this limit, and it names neither the member nor the count, so it
/// reads the same whichever operation added the probes. The class is the
/// `ValidationException` AWS answers here, not the `ServiceQuotaExceededException`
/// the model also declares.
pub const PROBES_PER_SUBNET_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";

/// The same, for the probes of a monitor as a whole
/// ([`MAX_PROBES_PER_MONITOR`]): one refusal class and one sentence shape for
/// every probe-count ceiling, so a caller catching the refusal of one catches
/// the refusal of the other.
pub const PROBES_PER_MONITOR_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";

/// The most `clientToken` records this emulator keeps. An EMULATOR-CHOSEN
/// ceiling: the key is a string the CALLER picks, so this is the one store whose
/// keys a caller controls. Past the ceiling the oldest record is dropped — a
/// dropped token simply stops replaying, which is the same answer a caller gets
/// for a token it never sent.
pub const MAX_CLIENT_TOKENS: usize = 1_000;

/// How long a recorded `clientToken` stays replayable. An EMULATOR-CHOSEN span:
/// AWS documents no lifetime for Network Monitor's idempotency tokens, and an
/// in-memory store that never forgets a caller-chosen key grows with the request
/// count.
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The most tags one monitor or probe keeps: an EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_MONITORS`] — every tagging write can add to the
/// store, so it is bounded rather than left to grow with the request count.
///
/// Its VALUE is the modelled `TagMap`'s own `@length(min: 0, max: 200)`, which
/// is the most pairs any response can carry: a resource holding more could not
/// be reported. AWS enforces a tag quota of its own, which may differ; a write
/// past this ceiling is refused with the modelled `ValidationException` naming
/// the bound, since `TagResource` models no quota error. The ceiling being that
/// `@length` maximum is why the refusal is a CONSTRAINT clause for it
/// ([`check_tag_count`]) rather than a sentence of this service's own.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix AWS keeps for the tags it applies itself.
///
/// AWS's tagging rules reserve this namespace across every service: a tag whose
/// key carries it belongs to AWS, and a caller can neither create one nor change
/// or remove one. The comparison is case-insensitive, because the rule is stated
/// over the prefix in any mixture of cases.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What a tag key a CALLER names has to satisfy, as a refusal's clause states
/// the requirement: it is outside the namespace AWS reserves for its own tags
/// ([`RESERVED_TAG_KEY_PREFIX`]). The model types a tag key as a plain length-
/// bounded string, so the reservation is the service's to apply.
const TAG_KEY_REQUIREMENT: &str = "Member must not begin with the reserved prefix aws:";

/// The AWS service namespace a probe's `sourceArn` must name.
///
/// The model documents `ProbeInput$sourceArn` as "The ARN of the subnet", and a
/// VPC subnet is an Amazon EC2 resource, so the ARN's service segment is `ec2`.
/// The shape's own `@pattern("^arn:.*$")` cannot express that, so the service
/// checks it.
pub const SUBNET_ARN_SERVICE: &str = "ec2";

/// The resource type a VPC subnet ARN names:
/// `arn:aws:ec2:<region>:<account>:subnet/<subnetId>`.
pub const SUBNET_ARN_RESOURCE_TYPE: &str = "subnet";

/// The separator between the resource TYPE and the id in a subnet ARN.
///
/// An ARN's resource field is written in either of two forms, `<type>/<id>` and
/// `<type>:<id>`, and Amazon EC2 writes a subnet's in the first: the ARN that
/// service reports for a subnet it created is
/// `arn:aws:ec2:<region>:<account>:subnet/<subnetId>`. So a `sourceArn` is read
/// as naming a subnet only in that form.
pub const SUBNET_ARN_RESOURCE_SEPARATOR: char = '/';

/// The id prefix every VPC subnet carries (`subnet-0123456789abcdef0`).
pub const SUBNET_ID_PREFIX: &str = "subnet-";

/// The lengths of the id part that follows an EC2 resource-id prefix: AWS's
/// "Resource IDs" documentation gives EC2 ids in the short (8 character) and
/// long (17 character) forms, and nothing else.
pub const EC2_ID_SUFFIX_LENGTHS: &[usize] = &[8, 17];

/// The `ResourceNotFoundException` message for a monitor this service does not
/// hold in the region the request was signed for, built from the request and the
/// caller identity: the account the request was made under, and the
/// `monitorName` it named.
pub fn monitor_not_found_message(monitor_name: &str) -> String {
    format!(
        "No existing monitor found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a `CreateMonitor` naming a monitor this
/// service already holds — the same two facts as
/// [`monitor_not_found_message`], stated as a conflict.
///
/// It names nothing the service generated (no ARN): a message built from the
/// request and the caller identity reads the same in every environment.
pub fn monitor_conflict_message(monitor_name: &str) -> String {
    format!(
        "Existing monitor found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The `ResourceNotFoundException` message for a probe a stored monitor does not
/// hold — the probe's id beside the same account and `monitorName` as
/// [`monitor_not_found_message`].
pub fn probe_not_found_message(probe_id: &str, monitor_name: &str) -> String {
    format!(
        "No existing probe {probe_id} found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The `ValidationException` message a monitor's delete is refused with while
/// the monitor still holds a probe that is in service: the pre-delete guard
/// AWS applies to `DeleteMonitor`, which names the monitor the request asked to
/// delete and the state its probes have to be in first.
pub fn monitor_probes_in_service_message(monitor_name: &str) -> String {
    format!("All probes on monitor {monitor_name} must be inactive before deletion.")
}

/// The `ValidationException` message a probe's delete is refused with while the
/// probe has not left [`PROBE_STATE_PENDING`]: the pre-delete guard AWS applies
/// to `DeleteProbe`, which names the probe the request asked to delete and the
/// state it must not still be in.
pub fn probe_pending_message(probe_id: &str) -> String {
    format!("Probe {probe_id} must not be in pending state before deletion.")
}

/// The resource an ARN names, as a message states it: the last `/`- or
/// `:`-separated segment (`subnet-0123…` of a subnet ARN, `my-topic` of an SNS
/// topic ARN). The whole string when it carries no separator.
pub fn arn_resource_id(arn: &str) -> &str {
    arn.rsplit(['/', ':']).next().unwrap_or(arn)
}

/// The subnet id a probe's `sourceArn` names, when it names a VPC subnet of this
/// account at all; `None` for anything else.
///
/// The STRUCTURAL half of resolving a `sourceArn`: the ARN must be an
/// [`SUBNET_ARN_SERVICE`] ARN of this emulator's account
/// ([`::engine::emulator_account_id`]) whose resource is a
/// [`SUBNET_ARN_RESOURCE_TYPE`] with a well-formed EC2 id. The id it answers is
/// what the EXISTENCE half ([`resolve_source_subnet`]) looks up, since AWS
/// resolves `sourceArn` against the subnets the caller's account really holds
/// and refuses a probe whose source is not one of them.
///
/// What an ARN IS — its six fields and which of them may be empty — is read
/// through the workspace's single ARN parse (`service_common::arn`, the same
/// module this file reads a Region label from), so this service cannot drift
/// from the rest of the tree about the grammar. What stays here is this
/// service's own rules on top of those fields: the vendor the ARN must name, the
/// account that has to hold the subnet, the `<type>/<id>` form EC2 writes a
/// subnet's resource in ([`SUBNET_ARN_RESOURCE_SEPARATOR`]) and the EC2 id
/// grammar ([`EC2_ID_SUFFIX_LENGTHS`]).
pub fn subnet_id_of(source_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(source_arn, SUBNET_ARN_SERVICE)?;
    if arn.account != ::engine::emulator_account_id() {
        return None;
    }
    let subnet_id = arn
        .resource
        .strip_prefix(SUBNET_ARN_RESOURCE_TYPE)?
        .strip_prefix(SUBNET_ARN_RESOURCE_SEPARATOR)?;
    let suffix = subnet_id.strip_prefix(SUBNET_ID_PREFIX)?;
    if !EC2_ID_SUFFIX_LENGTHS.contains(&suffix.len())
        || !suffix.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(subnet_id)
}

/// What Amazon EC2 — which owns the account's VPC subnets — says about the
/// subnet a probe's `sourceArn` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSubnet {
    /// EC2 reports the subnet, in the VPC its record states (`None` for a record
    /// that states none, which is nothing to report rather than an id naming no
    /// VPC).
    Held { vpc_id: Option<String> },
    /// EC2 reports that the account holds NO subnet with that id, so the ARN
    /// names a subnet that does not exist.
    Absent,
    /// Nothing is known about it: there is no EC2 to ask, it answered something
    /// else, or the request named no source subnet to resolve (an operation that
    /// cannot set one). A refusal is grounded in what EC2 REPORTED rather than
    /// assumed from silence, so nothing is refused on this.
    Unresolved,
}

impl SourceSubnet {
    /// The VPC the source subnet sits in, as EC2 reports it — what a stored probe
    /// reports as its `vpcId` (`Probe$vpcId`: "The ID of the source VPC
    /// subnet"). `None` whenever the subnet was not resolved, so no probe reports
    /// a VPC that nothing created.
    pub fn vpc_id(&self) -> Option<String> {
        match self {
            Self::Held { vpc_id } => vpc_id.clone(),
            Self::Absent | Self::Unresolved => None,
        }
    }
}

/// The prefix of every EC2 error code that says a value names no subnet of the
/// account: `InvalidSubnetID.NotFound` for an id no subnet carries, and
/// `InvalidSubnetID.Malformed` for a value that is not a subnet id at all. Both
/// say the same thing about a probe's source.
const SUBNET_ID_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 client this service resolves a probe's source subnet through: an AWS
/// SDK client over the in-process callback into this same emulator, or `None`
/// when the request's own signing region is not a Region LABEL.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here.
/// `region` is the request's own signing region, which is where the subnet a
/// probe of that region is sourced from lives.
///
/// That region is CALLER-SUPPLIED text ([`request_region`]), and here it ends up
/// in an endpoint URL's AUTHORITY rather than in a response, so it is read as a
/// Region label (`service_common::arn::region_label`, the one place this tree
/// states that grammar) instead of being interpolated as given: a value carrying
/// a host or path delimiter would name an authority the caller chose rather than
/// the peer service this emulator asks. Such a request reaches no EC2, which is
/// nothing known about the subnet ([`SourceSubnet::Unresolved`]) and so refuses
/// nothing.
fn ec2_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!(
            "the request names no Region label, so EC2 cannot be asked about a probe's \
             source subnet"
        );
        return None;
    };
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{SUBNET_ARN_SERVICE}.{region}.amazonaws.com"
        ))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// Resolve a probe's `sourceArn` against the subnets the account really holds.
///
/// `ProbeInput$sourceArn` is "The ARN of the subnet", and which subnets exist —
/// and which VPC each one sits in — is a fact of Amazon EC2's own inventory
/// rather than of the string a caller supplied. So every operation that TAKES a
/// `sourceArn` resolves the subnet id the ARN names ([`subnet_id_of`]) against
/// EC2, through the in-process callback, and refuses an id EC2 holds no subnet
/// for ([`validate_probe_spec`]). An ARN that names no subnet of this account at
/// all carries no id to look up and is refused on the ARN itself.
///
/// Grounded in what EC2 ANSWERED: a read that resolves without reporting the
/// subnet, and a read EC2 refuses because the id names no subnet of the account
/// ([`SUBNET_ID_REFUSAL_PREFIX`]), both say the subnet does not exist. Any other
/// failure is this emulator failing to ask rather than EC2 reporting a bad
/// subnet, so it says nothing ([`SourceSubnet::Unresolved`]) and nothing is
/// refused on it — as does a request this emulator cannot ask EC2 for at all
/// ([`ec2_client`]).
///
/// Called with NO lock of this service's store held — it is an in-process request
/// to another service, which reaches its own store.
pub async fn resolve_source_subnet(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source_arn: &str,
) -> SourceSubnet {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let Some(subnet_id) = subnet_id_of(source_arn) else {
        return SourceSubnet::Unresolved;
    };
    // No client is built for a request whose signing region is not a Region
    // label: the endpoint that would be formatted from it is an authority the
    // caller chose, so EC2 is not asked at all ([`ec2_client`]).
    let Some(client) = ec2_client(ctx, region) else {
        return SourceSubnet::Unresolved;
    };
    let answer = client
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported = output
                .subnets()
                .iter()
                .find(|subnet| subnet.subnet_id() == Some(subnet_id));
            return match reported {
                Some(subnet) => SourceSubnet::Held {
                    vpc_id: subnet.vpc_id().map(str::to_string),
                },
                // A read that resolved but reports no subnet for the id says the
                // same thing the refusal below does.
                None => SourceSubnet::Absent,
            };
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code.starts_with(SUBNET_ID_REFUSAL_PREFIX) {
        return SourceSubnet::Absent;
    }
    ::tracing::debug!(
        %subnet_id,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the source subnet a probe named could not be resolved against EC2"
    );
    SourceSubnet::Unresolved
}

/// What this service states for a `sourceArn` that names no subnet of this
/// account: the AWS sentence for that class, naming the subnet by the id the
/// ARN carried ([`arn_resource_id`]) so the caller is told which of its values
/// resolved to nothing.
///
/// One sentence for both halves of the resolution — an ARN that is no subnet ARN
/// of this account, and a well-formed one whose subnet the account does not hold
/// — because both say the same thing about the source: there is no such subnet.
pub fn subnet_not_found_message(source_arn: &str) -> String {
    format!(
        "The specified subnet {} does not exist or could not be found.",
        arn_resource_id(source_arn)
    )
}

/// The state a monitor is in the instant it is stored: `INACTIVE` when it holds
/// no probe (nothing to aggregate, so nothing is due) and `PENDING` when it
/// holds at least one (the clock owes it [`MONITOR_STATE_ACTIVE`]).
pub fn initial_monitor_state(has_probes: bool) -> &'static str {
    if has_probes {
        MONITOR_STATE_PENDING
    } else {
        MONITOR_STATE_INACTIVE
    }
}

/// The ARN of a monitor: `arn:aws:networkmonitor:<region>:<account>:monitor/<name>`.
pub fn monitor_arn(region: &str, monitor_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("monitor/{monitor_name}"))
}

/// The ARN of a probe: `arn:aws:networkmonitor:<region>:<account>:probe/<probeId>`.
pub fn probe_arn(region: &str, probe_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("probe/{probe_id}"))
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The marker a cursor this service vended carries ahead of the key it
/// encodes.
///
/// A `@paginated` token is opaque, so the CALLER cannot tell a cursor apart
/// from text of its own; the marker is what lets the SERVICE tell them apart,
/// so a string nothing ever vended is still refused while every cursor this
/// service did vend is honoured. The version digit keeps a cursor minted in
/// some other format from being read as a key.
const PAGE_TOKEN_MARKER: &str = "monitor-page/1:";

/// Encode a pagination cursor: the key of the first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as a `@paginated` token is
/// meant to be, and safe to carry in a `@httpQuery` parameter.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_MARKER}{key}"))
}

/// The key a cursor produced by [`encode_page_token`] encodes; `None` when the
/// token was not produced by this service.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_MARKER).map(str::to_owned)
}

/// The cursor a listing was actually asked to resume from, as the request
/// names it: `None` when the request named no position at all.
///
/// A `@paginated` token is an opaque name for a POSITION in the listing's key
/// order, and `PaginationToken`'s modelled `@length(min: 0)` admits the empty
/// string — so the boundary validator hands `""` to the operation. An empty
/// cursor carries no key, so it names no position, which is the same request as
/// one that carried no cursor: the FIRST page. Only a non-empty value claims to
/// be a cursor, so only such a value is held to [`decode_page_token`] and can
/// be refused for naming a page of no listing.
pub fn requested_page_cursor(next_token: Option<&str>) -> Option<&str> {
    next_token.filter(|token| !token.is_empty())
}

/// Where a listing resumes from a cursor: the index of the first item of
/// `ordered` whose key does not sort before `cursor`.
///
/// A cursor names a POSITION in the key order, not a membership. The item it
/// names can be gone by the time the caller resumes — deleted, or no longer
/// matching the filter the listing was made with — and the items after it are
/// still owed to the caller, so the resume falls forward to the next key at or
/// after the cursor instead of requiring the cursor's own item to still be in
/// the set. `ordered.len()` (an empty final page) when every key sorts before
/// the cursor.
///
/// `ordered` must be sorted by `key`, which the `monitorName`-keyed
/// [`BTreeMap`] walk over one region's monitors
/// ([`ComAmazonawsNetworkmonitorNetworkmonitorState::monitors_in`]) already is.
pub fn resume_index<T>(ordered: &[T], cursor: &str, key: impl Fn(&T) -> &str) -> usize {
    ordered.partition_point(|item| key(item) < cursor)
}

/// The `AddressFamily` of a probe destination, derived from the address itself.
///
/// The model documents `ProbeInput$destination` as "The destination IP address.
/// This must be either `IPV4` or `IPV6`", so the family is a property of the
/// address the caller gave, not a separate input: an IPv4 literal is `IPV4` and
/// an IPv6 literal is `IPV6`. `None` means the destination is not an IP address
/// at all, which the handler refuses.
pub fn address_family_of(destination: &str) -> Option<&'static str> {
    match destination.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Some(ADDRESS_FAMILY_IPV4),
        Ok(IpAddr::V6(_)) => Some(ADDRESS_FAMILY_IPV6),
        Err(_) => None,
    }
}

/// The `aggregationPeriod` a create or update request asks for, checked against
/// the documented value list.
///
/// `None` means the request named none, which is the documented default
/// ([`DEFAULT_AGGREGATION_PERIOD`]). `Err` carries the sentence the service
/// states for this rule ([`aggregation_period_message`]): the rule is an
/// allow-list the model publishes, so what the refusal states is that list and
/// nothing of the value that missed it.
pub fn resolve_aggregation_period(requested: Option<i64>) -> Result<i64, String> {
    let Some(period) = requested else {
        return Ok(DEFAULT_AGGREGATION_PERIOD);
    };
    if AGGREGATION_PERIODS.contains(&period) {
        return Ok(period);
    }
    Err(aggregation_period_message())
}

/// The text that separates a constraint clause's subject from the requirement
/// it states, in the framework's own wording ([`Violation::message`]). SPLIT on,
/// never written: see [`stated_requirement`].
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// The longest submitted value a refusal echoes back before the rest is elided.
///
/// The cap and the elision text are the framework's own (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`), so a long value refused here reads
/// exactly as the same value refused at the boundary: a clause interpolates a
/// value whose length the CALLER chooses, so an unbounded echo would let a small
/// request name an arbitrarily large response. The checks themselves always read
/// the whole value; this is rendering only.
const MAX_ECHOED_VALUE: usize = 256;

/// A value the caller submitted, as a clause echoes it: itself, or its first
/// [`MAX_ECHOED_VALUE`] bytes followed by the framework's own elision.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The dotted path of a top-level member of a request, spelled through the
/// framework's own path rule so a clause this service raises names a member
/// exactly as the boundary validator names it.
pub fn member_path(member: &str) -> String {
    path::member(member)
}

/// The dotted path of the `position`-th (1-BASED) element of a list member —
/// `probes.1.member` for the first entry of `CreateMonitor`'s `probes`.
pub fn element_path(member: &str, position: usize) -> String {
    path::join(&[path::member(member), path::element(position)])
}

/// The dotted path of ONE ENTRY of the map member sitting at `map_path` — the
/// map's own path followed by the entry's key, which is how the framework's walk
/// names a map entry whose value is what failed (`tags.team`).
///
/// `map_path` is a PATH rather than a member name, so a map nested inside a
/// request (`probes.1.member.probeTags`) is named exactly where the caller sent
/// it. The key is a value the caller chose, so it is echoed as sent and never
/// re-cased the way a model name is.
pub fn entry_path(map_path: &str, key: &str) -> String {
    path::join(&[map_path.to_string(), key.to_string()])
}

/// AWS's constraint-violation message for ONE rule this service's own code
/// applies, rendered by the framework's own clause builder.
///
/// `member_path` is where the offending member sat in the REQUEST (the operation
/// that holds the request supplies it, since only it knows that); `submitted` is
/// the value the caller sent, echoed single-quoted and capped ([`echoed`]), or
/// `None` for a member that carried none; `kind` is the constraint class the
/// value broke. The count envelope, the clause and the requirement for `kind`
/// are all [`envelope`]'s and [`Violation::message`]'s, so this cannot drift
/// from the wording a modelled constraint is refused with — and nothing is
/// rendered through Rust's `Debug`, which quotes a string and is a language
/// artefact no AWS service emits.
pub fn stated_violation(member_path: &str, submitted: Option<&str>, kind: ViolationKind) -> String {
    let violation = Violation {
        path: member_path.to_string(),
        kind,
        value: submitted.map(echoed),
        sensitive: false,
    };
    envelope(std::slice::from_ref(&violation)).unwrap_or_else(|| violation.message())
}

/// The same message for a requirement the framework carries no
/// [`ViolationKind`] for — a rule only this service can state, such as which
/// cursors a listing vends, or a format the model documents in prose.
///
/// [`ViolationKind`] is `#[non_exhaustive]` and lives in the framework crate, so
/// such a rule cannot be expressed as one. The envelope and the clause are still
/// the framework's: a clause is rendered for this member and value and only the
/// text after [`CONSTRAINT_MARKER`] is this service's, so the two refusal paths
/// cannot drift into two sentence shapes.
pub fn stated_requirement(
    member_path: &str,
    submitted: Option<&str>,
    requirement: &str,
) -> String {
    // Every class renders the same clause HEAD; this one is simply the class no
    // member of this service can break, so the requirement being replaced is
    // never one a caller could have been shown.
    let rendered = stated_violation(member_path, submitted, ViolationKind::UniqueItems);
    match rendered.rsplit_once(CONSTRAINT_MARKER) {
        Some((head, _)) => format!("{head}{CONSTRAINT_MARKER}{requirement}"),
        None => rendered,
    }
}

/// The requirement an enumerated value set states, over the values as AWS
/// renders them: BARE and SORTED, rather than in the order the model declares
/// them or in any language's quoting.
pub fn enum_value_set(values: &[&str]) -> ViolationKind {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort_unstable();
    ViolationKind::EnumValue(sorted)
}

/// The refusal of a value outside an enumerated type's value set, for a member
/// the model targets a plain `string` with — so the boundary validator admits
/// any text and the operation applies the enum itself.
pub fn enum_value_refusal(member: &str, allowed: &[&str], submitted: &str) -> String {
    stated_violation(
        &member_path(member),
        Some(submitted),
        enum_value_set(allowed),
    )
}

/// The refusal of a pagination cursor `operation` never vended.
///
/// A cursor is opaque — [`encode_page_token`] decides what one is — so the rule
/// a caller has to satisfy is WHERE the value came from: a token that same
/// listing handed back. The member and the rejected token are the request's, so
/// the clause is built from it like any other.
pub fn unknown_page_token_refusal(member: &str, operation: &str, submitted: &str) -> String {
    stated_requirement(
        &member_path(member),
        Some(submitted),
        &format!("Member must be a {member} returned by a previous {operation} call"),
    )
}

/// Whether `key` names a tag AWS owns: one whose key carries
/// [`RESERVED_TAG_KEY_PREFIX`], in any mixture of cases.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.to_ascii_lowercase().starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// Refuse ONE tag key a caller may not name, wherever the request carried it.
///
/// `key_path` is the dotted path the key sat at in the REQUEST — an entry of a
/// tag map ([`entry_path`]) or an element of a key list ([`element_path`]) — so
/// a refusal names the member the caller sent. `Err` carries the clause for
/// [`TAG_KEY_REQUIREMENT`], built like every other refusal this service authors.
pub fn check_tag_key(key_path: &str, key: &str) -> Result<(), String> {
    if is_reserved_tag_key(key) {
        return Err(stated_requirement(
            key_path,
            Some(key),
            TAG_KEY_REQUIREMENT,
        ));
    }
    Ok(())
}

/// Refuse a tag MAP that names a key a caller may not name, for every operation
/// that assigns tags (`CreateMonitor`'s `tags` and each probe's `probeTags`,
/// `CreateProbe`'s `tags` and `probe.tags`, `TagResource`'s `tags`).
///
/// `map_path` is where that map sat in the request. The keys are judged in key
/// ORDER, not in the order a hash map happens to yield them, so one request is
/// always refused for the same key.
pub fn check_tag_keys<'a>(
    map_path: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut reserved: Vec<&str> = keys.into_iter().filter(|key| is_reserved_tag_key(key)).collect();
    reserved.sort_unstable();
    match reserved.first() {
        Some(key) => Err(stated_requirement(
            &entry_path(map_path, key),
            Some(key),
            TAG_KEY_REQUIREMENT,
        )),
        None => Ok(()),
    }
}

/// How a clause ECHOES a map whose ENTRY COUNT is what broke a bound: the
/// rendering the framework's own validating walk writes into a clause for a
/// map past its `@length` (`<n> entries`, beside a list's `<n> items`), so a
/// count this service refuses reads exactly like one the boundary validator
/// refuses. A map has no scalar spelling, so the count is the value — there is
/// nothing of the caller's own text to echo.
fn echoed_entries(count: usize) -> String {
    format!("{count} entries")
}

/// Refuse a tag map that would leave a resource carrying more pairs than
/// [`MAX_TAGS_PER_RESOURCE`], for every operation that composes a resource's one
/// tag map out of MORE THAN ONE map — the pairs the store already holds plus a
/// request's (`TagResource`), and the two maps one request assigns the same new
/// resource (`CreateProbe`'s `tags` and `probe.tags`).
///
/// `map_path` is where the request's tag map sat, and `carried` is the other map
/// the merge keeps — the pairs the resource already holds, or the half of the
/// request already accounted for: the ceiling bounds what the resource WILL
/// carry, so the count judged is the MERGED set's — `carried` plus each
/// requested key it does not hold yet. A caller that only overwrites keys the
/// merge already carries adds nothing, so it is never refused. The count is the
/// union's and therefore the same whichever map is passed as `carried`.
///
/// `Err` carries the clause for the bound, built through the shared renderer
/// like every other refusal this service authors: the ceiling's value IS the
/// modelled `TagMap`'s own `@length` maximum, so what the clause states is that
/// maximum, in the framework's own wording for a collection past one
/// ([`stated_violation`]), and cannot drift from how any other bound is worded.
pub fn check_tag_count<'a>(
    map_path: &str,
    carried: &BTreeMap<String, String>,
    requested: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let added = requested
        .into_iter()
        .filter(|key| !carried.contains_key(*key))
        .count();
    let merged = carried.len() + added;
    if merged > MAX_TAGS_PER_RESOURCE {
        return Err(stated_violation(
            map_path,
            Some(&echoed_entries(merged)),
            ViolationKind::MaxLength(i64::try_from(MAX_TAGS_PER_RESOURCE).unwrap_or(i64::MAX)),
        ));
    }
    Ok(())
}

/// What the service states when an `aggregationPeriod` is outside the documented
/// allow-list: the list itself, rendered from [`AGGREGATION_PERIODS`] so the
/// sentence cannot drift from the values the handlers actually admit.
///
/// One sentence for the whole rule, shared by every operation that applies it
/// (`CreateMonitor`, `UpdateMonitor`), rather than one per handler. The list is
/// joined explicitly rather than `Debug`-formatted, so the sentence's shape is
/// this function's and not a formatting detail of the constant's type.
pub fn aggregation_period_message() -> String {
    let values: Vec<String> = AGGREGATION_PERIODS.iter().map(i64::to_string).collect();
    format!("The aggregation period must be one of [{}].", values.join(", "))
}

/// What a caller asks for when it asks for a probe: the members
/// `CreateMonitorProbeInput` and `ProbeInput` share, with the tags the request
/// assigns to the probe already resolved.
#[derive(Debug, Clone)]
pub struct ProbeSpec {
    pub source_arn: String,
    pub destination: String,
    pub destination_port: Option<i32>,
    pub protocol: String,
    pub packet_size: Option<i32>,
    pub tags: BTreeMap<String, String>,
}

/// Turn a [`ProbeSpec`] into the record the store keeps, applying the rules the
/// model documents but its constraints cannot express.
///
/// * `destination` is "the destination IP address … either `IPV4` or `IPV6`", so
///   a destination that is not an IP literal is refused and the stored
///   `addressFamily` is derived from the one that is.
/// * `destinationPort` "is required only if the `protocol` is `TCP` and must be
///   a number between `1` and `65536`", so TCP without a port, and a port of
///   `0` (which the shape's `@range(min: 0)` admits), are refused.
/// * `sourceArn` is "The ARN of the subnet", so an ARN that names anything other
///   than a VPC subnet of this account ([`subnet_id_of`]), and one naming a
///   subnet the account does not hold (`source`), are refused — the shape's
///   `@pattern("^arn:.*$")` admits any ARN at all.
///
/// `Err` carries a message naming the offending member.
///
/// `source` is what the account's own inventory said about the subnet that ARN
/// names ([`resolve_source_subnet`]), which the handler resolved before taking
/// this store's lock; it is also where the stored `vpcId` is read from, so a
/// probe reports the VPC its source subnet really sits in.
///
/// `probe_path` is where this probe sits in the REQUEST, for the clause a
/// refusal states; see [`validate_probe_spec`].
pub fn build_probe(
    spec: ProbeSpec,
    probe_id: String,
    region: &str,
    now: SystemTime,
    probe_path: &str,
    source: &SourceSubnet,
) -> Result<Probe, String> {
    let address_family = validate_probe_spec(&spec, probe_path, source)?;
    Ok(Probe {
        probe_arn: probe_arn(region, &probe_id),
        probe_id,
        source_arn: spec.source_arn,
        destination: spec.destination,
        destination_port: spec.destination_port,
        protocol: spec.protocol,
        packet_size: spec.packet_size,
        address_family: address_family.to_string(),
        vpc_id: source.vpc_id(),
        state: PROBE_STATE_PENDING.to_string(),
        activates_at: now
            .checked_add(ACTIVATION_AFTER)
            .unwrap_or(now),
        deletes_at: None,
        tags: spec.tags,
        created_at: now,
        modified_at: now,
    })
}

/// Check the rules every stored probe satisfies — the ones the model documents
/// in prose but its constraints cannot express — and answer the
/// `addressFamily` the spec's `destination` implies.
///
/// Shared by the operations that WRITE a probe record ([`build_probe`] for a
/// create, and `UpdateProbe` on the record it has merged the request into), so a
/// probe an update leaves behind satisfies exactly what a create would have
/// refused. `Err` carries a message naming the offending member.
///
/// `probe_path` is the dotted path of the probe WITHIN the request this spec was
/// built from, which is what a refusal's clause names the member by: `probe` for
/// `CreateProbe`, `probes.<position>.member` for one entry of `CreateMonitor`'s
/// list ([`element_path`]), and the EMPTY path for `UpdateProbe`, which binds a
/// probe's members at the top level of its own input. Only the operation holding
/// the request knows where its probe sat, so it supplies it.
///
/// `source` is what the account's own subnet inventory said about the subnet the
/// spec's `sourceArn` names ([`resolve_source_subnet`]): a well-formed ARN whose
/// subnet the account does not hold names a subnet that does not exist, which is
/// the same refusal as an ARN that names no subnet of this account at all
/// ([`subnet_not_found_message`]). An operation that binds no `sourceArn` of its
/// own, and so names no subnet to resolve, passes
/// [`SourceSubnet::Unresolved`] and is refused on nothing.
pub fn validate_probe_spec(
    spec: &ProbeSpec,
    probe_path: &str,
    source: &SourceSubnet,
) -> Result<&'static str, String> {
    if subnet_id_of(&spec.source_arn).is_none() || *source == SourceSubnet::Absent {
        return Err(subnet_not_found_message(&spec.source_arn));
    }
    let Some(address_family) = address_family_of(&spec.destination) else {
        return Err(stated_requirement(
            &probe_member_path(probe_path, "destination"),
            Some(&spec.destination),
            DESTINATION_REQUIREMENT,
        ));
    };
    if spec.protocol == PROTOCOL_TCP && spec.destination_port.is_none() {
        return Err(TCP_PORT_REQUIRED_MESSAGE.to_string());
    }
    if let Some(port) = spec.destination_port {
        if port < MIN_DESTINATION_PORT {
            // The documented floor, stated in the framework's own wording for a
            // numeric bound: the model's `@range(min: 0)` admits `0`, so this
            // bound is the service's but the clause is the one a `@range`
            // refusal already reads like.
            return Err(stated_violation(
                &probe_member_path(probe_path, "destinationPort"),
                Some(&port.to_string()),
                ViolationKind::MinValue(MIN_DESTINATION_PORT.to_string()),
            ));
        }
    }
    Ok(address_family)
}

/// Which source subnet a probe's `sourceArn` counts against, for every bound
/// and grouping stated per (monitor, source SUBNET) pair.
///
/// The subnet the ARN names ([`subnet_id_of`]) — a subnet id identifies the
/// subnet, so two `sourceArn` strings that name one subnet count as one subnet
/// however else they are spelled. The whole ARN is the key only for a string
/// that names no subnet of this account at all, which is a `sourceArn`
/// [`validate_probe_spec`] refuses outright: such a probe is never stored, so
/// the fallback only keeps an unnameable source from collapsing into another
/// one's group.
pub fn probe_subnet_key(source_arn: &str) -> &str {
    subnet_id_of(source_arn).unwrap_or(source_arn)
}

/// Check the per-subnet probe limit for a request that would ADD probes to a
/// monitor ([`MAX_PROBES_PER_SUBNET`]).
///
/// The limit is counted per (monitor, source subnet) pair, so the probes
/// `existing` already holds are grouped by the SUBNET their `sourceArn` names
/// ([`probe_subnet_key`]) and each requested `sourceArn` is counted on top of
/// its subnet's group, in request order: one call that adds several probes to
/// one subnet is bounded exactly as several calls adding one each are, and two
/// spellings of one subnet's ARN share that subnet's single bound rather than
/// doubling it. `existing` is `None` for a monitor the request is creating,
/// which holds no probe yet.
///
/// Shared by every operation that ADDS a probe to a monitor (`CreateProbe`, and
/// `CreateMonitor`'s inline `probes` list), so the limit cannot be enforced on
/// one path and not the other. `Err` carries
/// [`PROBES_PER_SUBNET_MESSAGE`].
///
/// Applied only once each requested probe has satisfied the probe rules on its
/// own, for the reason given on [`check_probes_per_monitor`].
pub fn check_probes_per_subnet<'a>(
    existing: Option<&'a Monitor>,
    requested: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut per_subnet: BTreeMap<&str, usize> = BTreeMap::new();
    if let Some(monitor) = existing {
        // Every probe the monitor holds occupies a slot in its own subnet,
        // including one whose teardown is still draining: such a probe is still
        // attached, and the store drops it once the teardown has run its course
        // ([`Probe::is_forgotten_at`]), which frees the slot.
        for probe in monitor.probes.values() {
            *per_subnet
                .entry(probe_subnet_key(probe.source_arn.as_str()))
                .or_default() += 1;
        }
    }
    for source_arn in requested {
        let count = per_subnet.entry(probe_subnet_key(source_arn)).or_default();
        *count += 1;
        if *count > MAX_PROBES_PER_SUBNET {
            return Err(PROBES_PER_SUBNET_MESSAGE.to_string());
        }
    }
    Ok(())
}

/// Check the per-monitor probe limit for a request that would ADD probes to a
/// monitor ([`MAX_PROBES_PER_MONITOR`]).
///
/// The limit bounds the probes ONE monitor holds, so the probes `existing`
/// already holds and the `requested` count the call adds are counted together:
/// one call adding several probes is bounded exactly as several calls adding one
/// each are. `existing` is `None` for a monitor the request is creating, which
/// holds no probe yet.
///
/// Shared by every operation that ADDS a probe to a monitor (`CreateProbe`, and
/// `CreateMonitor`'s inline `probes` list), so the limit cannot be enforced on
/// one path and not the other. `Err` carries
/// [`PROBES_PER_MONITOR_MESSAGE`].
///
/// A count is what a request asks for, so every caller applies it only once
/// each requested probe has satisfied the probe rules on its own
/// ([`validate_probe_spec`]): a malformed probe is refused by the clause naming
/// the member it got wrong, never by a count sentence that names none. The
/// sibling [`check_probes_per_subnet`] sits at the same point, so the two
/// ceilings of one family resolve the same way.
pub fn check_probes_per_monitor(
    existing: Option<&Monitor>,
    requested: usize,
) -> Result<(), String> {
    // Every probe the monitor holds occupies a slot, including one whose
    // teardown is still draining: such a probe is still attached, and the store
    // drops it once the teardown has run its course
    // ([`Probe::is_forgotten_at`]), which frees the slot — the same population
    // [`check_probes_per_subnet`] counts.
    let held = existing.map_or(0, |monitor| monitor.probes.len());
    if held.saturating_add(requested) > MAX_PROBES_PER_MONITOR {
        return Err(PROBES_PER_MONITOR_MESSAGE.to_string());
    }
    Ok(())
}

/// The dotted path of one member of the probe sitting at `probe_path`, which is
/// empty for an operation that binds a probe's members at the top level of its
/// own input (`UpdateProbe`).
pub fn probe_member_path(probe_path: &str, member: &str) -> String {
    if probe_path.is_empty() {
        return member_path(member);
    }
    path::join(&[probe_path.to_string(), path::member(member)])
}

/// Check that `requested` is a state a CALLER may put a probe into.
///
/// The modelled `ProbeState` enum carries the service's own lifecycle values
/// beside the settable ones, so the enum alone cannot express this: the
/// allow-list ([`SETTABLE_PROBE_STATES`]) is enforced by the operation that
/// takes the member. `Err` carries the message that names the rejected status,
/// which is what the caller has to fix in its request.
pub fn validate_settable_probe_state(requested: &str) -> Result<(), String> {
    if SETTABLE_PROBE_STATES.contains(&requested) {
        return Ok(());
    }
    Err(format!("{requested} is not a valid probe status."))
}

/// A probe: one source-subnet-to-destination path inside a monitor.
#[derive(Debug, Clone)]
pub struct Probe {
    pub probe_id: String,
    pub probe_arn: String,
    pub source_arn: String,
    pub destination: String,
    pub destination_port: Option<i32>,
    pub protocol: String,
    pub packet_size: Option<i32>,
    /// Derived from `destination` by [`address_family_of`].
    pub address_family: String,
    /// The VPC of the source subnet (`Probe$vpcId`: "The ID of the source VPC
    /// subnet"). A subnet ARN does not carry its VPC id, so this is READ OFF
    /// what EC2 answered when the create resolved the subnet
    /// ([`resolve_source_subnet`]) and is `None` otherwise — a probe never
    /// reports a VPC that nothing created.
    pub vpc_id: Option<String>,
    pub state: String,
    /// When the clock owes this probe its `PENDING` -> `ACTIVE` transition. See
    /// [`ACTIVATION_AFTER`].
    pub activates_at: SystemTime,
    /// When the clock owes this probe its `DELETING` -> `DELETED` transition,
    /// set by the accepted `DeleteProbe` that started its teardown; `None` for a
    /// probe nothing has asked to delete. A probe that carries one is being torn
    /// down ([`Probe::is_being_torn_down`]) and leaves the store one
    /// [`PROBE_TEARDOWN_STAGE`] after that instant
    /// ([`Probe::is_forgotten_at`]).
    pub deletes_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

impl Probe {
    /// What this probe asks for, as the shared [`ProbeSpec`] states it: the
    /// starting point an update merges a request into, so a stored probe and a
    /// requested one go through the same [`validate_probe_spec`].
    pub fn spec(&self) -> ProbeSpec {
        ProbeSpec {
            source_arn: self.source_arn.clone(),
            destination: self.destination.clone(),
            destination_port: self.destination_port,
            protocol: self.protocol.clone(),
            packet_size: self.packet_size,
            tags: self.tags.clone(),
        }
    }

    /// Accept this probe's deletion: tearing a probe down is asynchronous, so
    /// the record STAYS, moves into the teardown half of its lifecycle
    /// ([`PROBE_STATE_DELETING`]) and the clock owes it the rest
    /// ([`PROBE_TEARDOWN_STAGE`]).
    ///
    /// Idempotent, as the modelled operation that calls it is: a probe already
    /// being torn down keeps the instants the first accepted delete set, so a
    /// repeat neither postpones the teardown nor restarts it.
    ///
    /// The accepted delete CHANGES this record — the `state` every read of it
    /// reports, and the teardown instant the clock now owes it — so, exactly as
    /// for any other accepted change to a probe, `modified_at` moves to the
    /// instant of the call that made it.
    pub fn start_teardown(&mut self, now: SystemTime) {
        if self.is_being_torn_down() {
            return;
        }
        self.state = PROBE_STATE_DELETING.to_string();
        self.deletes_at = Some(now.checked_add(PROBE_TEARDOWN_STAGE).unwrap_or(now));
        self.modified_at = now;
    }

    /// Whether this probe's teardown has been accepted. Such a probe is still
    /// reported, in its teardown state, but it is on its way out: its teardown
    /// is irreversible, so no later call puts it back into service.
    pub fn is_being_torn_down(&self) -> bool {
        self.deletes_at.is_some()
    }

    /// Whether this probe's teardown finished long enough ago that the record is
    /// no longer readable at `now` — from then on a read of it is not-found.
    pub fn is_forgotten_at(&self, now: SystemTime) -> bool {
        self.deletes_at.is_some_and(|deletes_at| {
            deletes_at
                .checked_add(PROBE_TEARDOWN_STAGE)
                .is_some_and(|forgotten_at| now >= forgotten_at)
        })
    }

    /// Whether this probe is still being provisioned, i.e. has not left
    /// [`PROBE_STATE_PENDING`].
    ///
    /// Read AFTER the clock has been settled
    /// ([`ComAmazonawsNetworkmonitorNetworkmonitorState::settle_monitor`]), so a
    /// probe whose activation was already due answers `false`: what this reports
    /// is a probe that is not settled YET, which is the state a delete of it is
    /// refused on.
    pub fn is_provisioning(&self) -> bool {
        self.state == PROBE_STATE_PENDING
    }

    /// Whether a caller has switched this probe off.
    ///
    /// A deactivated probe keeps its configuration but sends no packets, so it
    /// measures nothing its monitor could aggregate
    /// ([`Monitor::follow_probe_states`]).
    pub fn is_deactivated(&self) -> bool {
        self.state == PROBE_STATE_INACTIVE
    }

    /// Whether this probe is in service, i.e. measures for its monitor or is
    /// provisioning to.
    ///
    /// A probe is in service while it is neither switched off by a caller
    /// ([`Self::is_deactivated`]) nor on its way out
    /// ([`Self::is_being_torn_down`]): a teardown is irreversible, so a probe
    /// draining through it will never measure again and is not something a
    /// monitor can bring into service. Such a probe is still HELD and still
    /// reported — the monitor keeps aggregating what it already had until the
    /// record is gone ([`Monitor::follow_probe_states`]) — but it can never be
    /// what makes a monitor's activation due.
    pub fn is_in_service(&self) -> bool {
        !self.is_deactivated() && !self.is_being_torn_down()
    }

    /// Materialise the lifecycle transition the clock has already made due for
    /// THIS probe: `PENDING` -> `ACTIVE` once its activation instant has passed,
    /// and `DELETING` -> `DELETED` once its teardown instant has. A probe being
    /// torn down owes nothing else: the teardown outranks the activation, so a
    /// probe deleted while still provisioning never becomes `ACTIVE`.
    fn settle(&mut self, now: SystemTime) {
        if let Some(deletes_at) = self.deletes_at {
            if now >= deletes_at {
                self.state = PROBE_STATE_DELETED.to_string();
            }
            return;
        }
        if self.state == PROBE_STATE_PENDING && now >= self.activates_at {
            self.state = PROBE_STATE_ACTIVE.to_string();
        }
    }
}

/// Project a STORED [`Probe`] record onto one of the structurally identical
/// per-operation probe response shapes — `GetProbeOutput`, `CreateProbeOutput`,
/// `UpdateProbeOutput` and the modelled `Probe` a monitor lists — named at the
/// call site as `$out`.
///
/// ONE projection rather than a hand-written copy of the same member moves per
/// operation: every member is read out of the record, so what `CreateProbe`
/// answers is what `GetProbe` reads back and what `GetMonitor` lists, and a
/// change to how a probe is reported lands in every read path at once. The four
/// shapes model the same 13 members (`smithy_query(query="shape",
/// shape="GetProbeOutput,CreateProbeOutput,UpdateProbeOutput,Probe")`), so the
/// only thing that differs between the call sites is the type's name.
macro_rules! probe_report {
    ($out:ident, $probe:expr) => {{
        let probe: &$crate::state::Probe = $probe;
        $out {
            probe_id: Some(probe.probe_id.clone()),
            probe_arn: Some(probe.probe_arn.clone()),
            source_arn: probe.source_arn.clone(),
            destination: probe.destination.clone(),
            destination_port: probe.destination_port,
            protocol: probe.protocol.clone(),
            packet_size: probe.packet_size,
            address_family: Some(probe.address_family.clone()),
            vpc_id: probe.vpc_id.clone(),
            state: Some(probe.state.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(probe.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(probe.modified_at)),
            tags: Some(
                probe
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }};
}

pub(crate) use probe_report;

/// A monitor: the container `CreateMonitor` inserts, holding its own probes.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub monitor_name: String,
    pub monitor_arn: String,
    pub state: String,
    pub aggregation_period: i64,
    pub tags: BTreeMap<String, String>,
    /// Probes by `probeId`. A `BTreeMap`, so `GetMonitor` reports them in a
    /// stable order across reads.
    pub probes: BTreeMap<String, Probe>,
    /// When the clock owes this monitor its `PENDING` -> `ACTIVE` transition,
    /// set by the call that gave it its first probe. `None` while the monitor
    /// holds no probes: it has nothing to aggregate, so it stays
    /// [`MONITOR_STATE_INACTIVE`].
    pub activates_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

impl Monitor {
    /// Record that this monitor now has a probe in service, so the clock owes
    /// it the `PENDING` -> `ACTIVE` transition [`ACTIVATION_AFTER`] from `now`.
    ///
    /// Called by the mutating operations that put a probe in service and by
    /// [`Self::follow_probe_states`], which is the one place that decides this
    /// from the probe records. A monitor that already has an activation due
    /// keeps the earlier instant: a second probe going into service does not
    /// postpone the first one's activation.
    ///
    /// A monitor that was `INACTIVE` — nothing to aggregate — is provisioning
    /// what it just gained, so it moves to `PENDING` and the clock owes it
    /// `ACTIVE`.
    pub fn schedule_activation(&mut self, now: SystemTime) {
        if self.activates_at.is_none() {
            self.activates_at = now.checked_add(ACTIVATION_AFTER);
            if self.state == MONITOR_STATE_INACTIVE {
                self.state = MONITOR_STATE_PENDING.to_string();
            }
        }
    }

    /// Give up the activation the clock owed: a monitor with nothing in service
    /// has nothing to aggregate, so it goes back to `INACTIVE` with nothing due.
    pub fn clear_activation(&mut self) {
        self.activates_at = None;
        self.state = MONITOR_STATE_INACTIVE.to_string();
    }

    /// Whether any probe this monitor holds is in service, i.e. is something
    /// the monitor can aggregate ([`Probe::is_in_service`]).
    pub fn has_probe_in_service(&self) -> bool {
        self.probes.values().any(Probe::is_in_service)
    }

    /// Whether any probe this monitor holds is draining through its teardown
    /// ([`Probe::is_being_torn_down`]) — a probe the monitor still holds and
    /// still reports, which is why such a probe leaves the monitor's own state
    /// where it is ([`Self::follow_probe_states`]).
    pub fn has_probe_being_torn_down(&self) -> bool {
        self.probes.values().any(Probe::is_being_torn_down)
    }

    /// Make this monitor's `state` follow the probes it holds, which is the one
    /// rule that decides it: a monitor aggregates what its probes measure.
    ///
    /// A monitor with nothing in service — no probe at all, or every probe it
    /// holds deactivated — has nothing to aggregate, so it goes back to
    /// `INACTIVE` with nothing due. One that has a probe in service is owed the
    /// `PENDING` -> `ACTIVE` transition ([`Self::schedule_activation`]) and
    /// reaches `ACTIVE` once that instant has passed.
    ///
    /// A probe on its way out is neither: taking a probe away gains a monitor
    /// nothing to aggregate, so a teardown never makes an activation due, and
    /// the probe is still held and reported, so it does not idle the monitor
    /// either. A monitor whose every remaining probe is draining therefore keeps
    /// the state it had reached, with nothing new due, and falls back to
    /// `INACTIVE` when the last of those records is gone.
    ///
    /// Called by every operation that can change which probes are in service,
    /// and by the settle walk every read performs, so one monitor-level rule is
    /// applied however a probe got into or out of service. Every half is decided
    /// by the probe records themselves, so applying it twice changes nothing:
    /// the activation a monitor is already owed keeps its instant.
    pub fn follow_probe_states(&mut self, now: SystemTime) {
        if !self.has_probe_in_service() {
            if self.has_probe_being_torn_down() {
                return;
            }
            if self.state != MONITOR_STATE_INACTIVE || self.activates_at.is_some() {
                self.clear_activation();
            }
            return;
        }
        self.schedule_activation(now);
        if self.state == MONITOR_STATE_PENDING
            && self
                .activates_at
                .is_some_and(|activates_at| now >= activates_at)
        {
            self.state = MONITOR_STATE_ACTIVE.to_string();
        }
    }

    /// Apply a call's write to one of THIS monitor's probes: when that write
    /// stored something new (`stored_change`) the monitor's own `modifiedAt`
    /// moves to the instant of the call, and the monitor's state then follows
    /// the probes it now holds ([`Self::follow_probe_states`]).
    ///
    /// The probes are part of the monitor record every read reports
    /// (`GetMonitor` lists them with their own members), so a call that adds a
    /// probe, takes one away or rewrites one CHANGED the monitor — the same rule
    /// a changed tag map follows
    /// ([`ComAmazonawsNetworkmonitorNetworkmonitorState::mark_modified`]). A
    /// call that stored nothing new changed nothing, so it moves no
    /// `modifiedAt`: the probe-writing operations are `@idempotent`, and a
    /// repeat of one leaves both records exactly as they were.
    ///
    /// Called by every operation that writes a probe of a stored monitor, so
    /// one rule is applied however the probes changed — the probe-shaped
    /// operations directly, and a tag write against a probe's ARN through
    /// [`ComAmazonawsNetworkmonitorNetworkmonitorState::mark_modified`]. NOT
    /// part of the settle walk a read performs: a transition the clock made due
    /// was already owed by the call that scheduled it, and a read modifies
    /// nothing.
    pub fn follow_probe_write(&mut self, stored_change: bool, now: SystemTime) {
        if stored_change {
            self.modified_at = now;
        }
        self.follow_probe_states(now);
    }
}

/// Project a STORED [`Monitor`] record onto one of the per-operation monitor
/// response shapes — `CreateMonitorOutput`, `UpdateMonitorOutput` and the
/// modelled `MonitorSummary` a listing vends, which are structurally identical,
/// and `GetMonitorOutput`, which is their superset — named at the call site as
/// `$out`.
///
/// ONE projection rather than a hand-written copy of the same member moves per
/// operation: every member is read out of the record, so what `CreateMonitor`
/// answers is what `UpdateMonitor` answers, what `GetMonitor` reads back and
/// what `ListMonitors` summarises, and a change to how a monitor is reported
/// lands in every read path at once — the rule the probe-side twin
/// [`probe_report`] already follows.
///
/// The three summary shapes model the same five members
/// (`smithy_query(query="shape",
/// shape="CreateMonitorOutput,UpdateMonitorOutput,MonitorSummary")`), so the
/// only thing that differs between those call sites is the type's name and the
/// one-argument form projects all five. `GetMonitorOutput` models those five
/// plus `probes`, `createdAt` and `modifiedAt`, and models `aggregationPeriod`
/// as required rather than optional, so the longer form takes that member's
/// value and the extra members from the call site and projects the rest the same
/// way.
macro_rules! monitor_report {
    ($out:ident, $monitor:expr) => {{
        let monitor: &$crate::state::Monitor = $monitor;
        $crate::state::monitor_report!(
            $out,
            monitor,
            aggregation_period: ::core::option::Option::Some(monitor.aggregation_period),
        )
    }};
    (
        $out:ident,
        $monitor:expr,
        aggregation_period: $aggregation_period:expr
        $(, $member:ident: $value:expr)* $(,)?
    ) => {{
        let monitor: &$crate::state::Monitor = $monitor;
        $out {
            monitor_arn: monitor.monitor_arn.clone(),
            monitor_name: monitor.monitor_name.clone(),
            state: monitor.state.clone(),
            aggregation_period: $aggregation_period,
            tags: ::core::option::Option::Some(
                monitor
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            $($member: $value,)*
        }
    }};
}

pub(crate) use monitor_report;

/// Which stored resource the `resourceArn` of a tagging request names.
///
/// The tagging operations address their target by ARN rather than by name
/// (`TagResource`, `UntagResource` and `ListTagsForResource` all bind
/// `resourceArn` to `/tags/{resourceArn}`), and the model documents that target
/// as "a monitor or probe", so an ARN resolves to one of these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A monitor, by the region that holds it and its `monitorName`.
    Monitor {
        region: String,
        monitor_name: String,
    },
    /// A probe, by the region and monitor that hold it and its own `probeId`.
    Probe {
        region: String,
        monitor_name: String,
        probe_id: String,
    },
}

/// What a recorded `clientToken` replays, and when it was recorded so it can be
/// forgotten after [`CLIENT_TOKEN_TTL`].
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub monitor_name: String,
    /// The probe the token minted, for a `CreateProbe` token; `None` for a
    /// `CreateMonitor` token.
    pub probe_id: Option<String>,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-networkmonitor-networkmonitor. A handler reaches it through [`service_state`].
///
/// A monitor is a REGIONAL resource: the service has one endpoint per region
/// (`https://networkmonitor.<region>.<dnsSuffix>`) and every ARN it mints names
/// that region, so one `monitorName` identifies a different resource in each
/// region, and a request reaches only the records of the region its SigV4
/// credential scope names ([`request_region`]). The store is therefore keyed by
/// region first, and each region's own map by `monitorName`.
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// Region -> (`monitorName` -> record), each record holding its own probes.
    /// Bounded by [`MAX_MONITORS`] across every region; a region whose last
    /// monitor is deleted keeps no empty map behind
    /// ([`Self::remove_monitor`]).
    monitors: BTreeMap<String, BTreeMap<String, Monitor>>,
    /// (region, `clientToken`) -> what it replays. A token is scoped to the
    /// region it was sent to, like the monitor it minted. Bounded by
    /// [`MAX_CLIENT_TOKENS`] and [`CLIENT_TOKEN_TTL`].
    ///
    /// The ONLY record of which token minted which resource: a monitor and a
    /// probe keep no token of their own, so there is nothing for this map to
    /// disagree with once a record is forgotten (TTL, ceiling, or the resource
    /// it named being gone). Because it is the only record, a record that still
    /// replays is never written over by a later create reusing the same token
    /// ([`Self::record_client_token`]): a token identifies the create that used
    /// it FIRST, for as long as that create's resource is stored.
    client_tokens: BTreeMap<(String, String), ClientTokenRecord>,
    /// Monotonic counter behind [`Self::next_probe_id`]; never reused within a
    /// run.
    probe_seq: u64,
}

impl ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// The monitors of `region`, in `monitorName` order; `None` when that region
    /// holds none.
    pub fn monitors_in(&self, region: &str) -> Option<&BTreeMap<String, Monitor>> {
        self.monitors.get(region)
    }

    /// One region's monitor, for reporting.
    pub fn monitor(&self, region: &str, monitor_name: &str) -> Option<&Monitor> {
        self.monitors.get(region)?.get(monitor_name)
    }

    /// One region's monitor, for mutation. Does NOT create the region's map, so
    /// a request naming a monitor nothing stored leaves the store as it found
    /// it.
    pub fn monitor_mut(&mut self, region: &str, monitor_name: &str) -> Option<&mut Monitor> {
        self.monitors.get_mut(region)?.get_mut(monitor_name)
    }

    /// Store a monitor in `region`, opening that region's map if this is the
    /// first record kept there. For a create path only, AFTER the request is
    /// known to be acceptable.
    pub fn insert_monitor(&mut self, region: &str, monitor: Monitor) {
        self.monitors
            .entry(region.to_string())
            .or_default()
            .insert(monitor.monitor_name.clone(), monitor);
    }

    /// Remove one region's monitor, with the probes it holds. A region left
    /// holding no monitor is dropped with it, so an empty partition never
    /// lingers.
    pub fn remove_monitor(&mut self, region: &str, monitor_name: &str) -> Option<Monitor> {
        let region_monitors = self.monitors.get_mut(region)?;
        let removed = region_monitors.remove(monitor_name);
        if region_monitors.is_empty() {
            self.monitors.remove(region);
        }
        removed
    }

    /// How many monitors this store holds across every region — what
    /// [`MAX_MONITORS`] bounds, since the ceiling exists to bound the store's
    /// memory.
    pub fn monitor_count(&self) -> usize {
        self.monitors.values().map(BTreeMap::len).sum()
    }

    /// What a `clientToken` sent to `region` replays, when that region holds a
    /// record for it.
    ///
    /// Private: which token minted which resource is recorded in ONE place, and
    /// [`Self::monitor_minted_by_token`] / [`Self::probe_minted_by_token`] are
    /// the whole of what a create asks of it, so no caller outside this module
    /// re-derives the retry decision from the raw record.
    fn client_token_record(&self, region: &str, token: &str) -> Option<&ClientTokenRecord> {
        self.client_tokens
            .get(&(region.to_string(), token.to_string()))
    }

    /// The monitor a `clientToken` sent to `region` already minted under
    /// `monitor_name`, when that token makes the repeat a RETRY of the create
    /// that minted it: this region holds a record for the token, that record was
    /// made by a monitor create (it names no probe) and it names this monitor,
    /// and the monitor it names is still stored.
    ///
    /// `None` for every other token — one this region never saw, one the store
    /// has since forgotten, one that minted a probe, one that minted a
    /// differently named monitor, or one whose monitor is gone — so the repeat
    /// is judged on its own, as a request carrying no token is.
    pub fn monitor_minted_by_token(
        &self,
        region: &str,
        token: &str,
        monitor_name: &str,
    ) -> Option<&Monitor> {
        let record = self.client_token_record(region, token)?;
        if record.probe_id.is_some() || record.monitor_name != monitor_name {
            return None;
        }
        self.monitor(region, monitor_name)
    }

    /// The probe a `clientToken` sent to `region` already minted on
    /// `monitor_name`, under the same rule as
    /// [`Self::monitor_minted_by_token`]: the record has to name a probe of this
    /// monitor, and that probe has to be stored still.
    pub fn probe_minted_by_token(
        &self,
        region: &str,
        token: &str,
        monitor_name: &str,
    ) -> Option<&Probe> {
        let record = self.client_token_record(region, token)?;
        if record.monitor_name != monitor_name {
            return None;
        }
        let probe_id = record.probe_id.as_deref()?;
        self.monitor(region, monitor_name)?.probes.get(probe_id)
    }

    /// Whether a record still replays a STORED resource: `region` holds the
    /// monitor it names, and — for a record made by a probe create — that
    /// monitor still holds the probe it names.
    ///
    /// A record that replays nothing decides no retry
    /// ([`Self::monitor_minted_by_token`], [`Self::probe_minted_by_token`] both
    /// require the resource to be stored), so it is spent and may be written
    /// over.
    fn record_replays_a_resource(&self, region: &str, record: &ClientTokenRecord) -> bool {
        let Some(monitor) = self.monitor(region, &record.monitor_name) else {
            return false;
        };
        match record.probe_id.as_deref() {
            Some(probe_id) => monitor.probes.contains_key(probe_id),
            None => true,
        }
    }

    /// Record what a `clientToken` sent to `region` replays — the resource of
    /// the create that used the token FIRST.
    ///
    /// A token whose record still replays a stored resource KEEPS that record: a
    /// later create carrying the same token is a different request, not the
    /// retry of the one that recorded it, and since this map is the only record
    /// of which token minted which resource, rewriting it would leave the first
    /// resource with no idempotency at all — a retry of that first create would
    /// then be judged as a fresh request and conflict, or mint a duplicate. So a
    /// record is replaced only once it replays nothing
    /// ([`Self::record_replays_a_resource`]), and is otherwise forgotten only by
    /// [`CLIENT_TOKEN_TTL`], the [`MAX_CLIENT_TOKENS`] ceiling, or the delete of
    /// the monitor it named ([`Self::forget_client_tokens_of_monitor`]).
    ///
    /// So a token already spoken for buys the LATER create no idempotency of its
    /// own: that create is answered from its own request, and every repeat of it
    /// is judged on its own too — the same answer a request carrying no token
    /// gets (a stored monitor name conflicts, a probe create mints another
    /// probe). A token is unique per request by contract, and the model declares
    /// no mismatch error for one that is not, so a second request reusing a
    /// token is served rather than refused; what it cannot do is take over the
    /// record that identifies the first.
    pub fn record_client_token(&mut self, region: &str, token: String, record: ClientTokenRecord) {
        let key = (region.to_string(), token);
        let replaying = self
            .client_tokens
            .get(&key)
            .is_some_and(|existing| self.record_replays_a_resource(region, existing));
        if replaying {
            return;
        }
        self.client_tokens.insert(key, record);
    }

    /// Forget every `clientToken` of `region` that named this monitor: once the
    /// monitor is gone those records replay nothing.
    pub fn forget_client_tokens_of_monitor(&mut self, region: &str, monitor_name: &str) {
        self.client_tokens.retain(|(token_region, _), record| {
            token_region != region || record.monitor_name != monitor_name
        });
    }

    /// A fresh, unique `probeId` in the form the model's `ProbeId` pattern
    /// requires: `^probe-[a-z0-9A-Z-]{21,64}$`, here 24 lowercase hex digits
    /// after the prefix.
    ///
    /// Uniqueness comes from `probe_seq`, a per-engine counter, so two ids
    /// minted within one nanosecond still differ; the creation instant supplies
    /// the high bits so ids from separate runs do not collide.
    pub fn next_probe_id(&mut self, now: SystemTime) -> String {
        self.probe_seq = self.probe_seq.wrapping_add(1);
        let nanos = u64::try_from(
            now.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        // The low 32 bits of the counter render as exactly 8 hex digits, so the
        // id is always 24 characters after the prefix — inside the pattern's
        // 21..64.
        let seq = self.probe_seq & 0xffff_ffff;
        format!("probe-{nanos:016x}{seq:08x}")
    }

    /// Materialise the transitions the clock has already made due, for ONE
    /// monitor and its probes.
    ///
    /// These are the ONLY writes a reporting operation performs: a monitor whose
    /// `activates_at` has passed was owed its activation at that instant whether
    /// or not anybody called, and a probe whose teardown instants have passed
    /// was owed its `DELETED` state, and then its removal, at those instants, so
    /// a read settles them before reporting. Nothing else about the monitor
    /// changes, and `modified_at` is NOT touched: the caller modified nothing.
    ///
    /// The order is removal first, then the per-probe transition: a probe whose
    /// teardown finished long enough ago is not reported at all, and one still
    /// within its teardown is reported in the state that teardown has reached.
    pub fn settle_monitor(&mut self, region: &str, monitor_name: &str, now: SystemTime) {
        let Some(monitor) = self.monitor_mut(region, monitor_name) else {
            return;
        };
        monitor.probes.retain(|_, probe| !probe.is_forgotten_at(now));
        for probe in monitor.probes.values_mut() {
            probe.settle(now);
        }
        // The monitor-level half of the same rule: a monitor left with nothing
        // in service — the teardown of its last probe has run its course, or
        // every probe it holds has been deactivated — has nothing to aggregate,
        // which is the state a monitor created without probes is in. Both halves
        // are read off the probe records, so this reports the state the monitor
        // was already in at `now` whether or not anybody called.
        monitor.follow_probe_states(now);
    }

    /// The same, for every monitor of ONE region: what `ListMonitors` reports is
    /// the state each monitor is in at the instant of the call, so that region's
    /// whole set is settled before the page is built. Bounded by
    /// [`MAX_MONITORS`].
    pub fn settle_monitors_in(&mut self, region: &str, now: SystemTime) {
        let names: Vec<String> = self
            .monitors_in(region)
            .map(|monitors| monitors.keys().cloned().collect())
            .unwrap_or_default();
        for name in names {
            self.settle_monitor(region, &name, now);
        }
    }

    /// Which stored resource carries `resource_arn`, or `None` when nothing this
    /// service holds does.
    ///
    /// The ARN is compared against the one STORED on each record — the ARN the
    /// service minted and reports — so a tagging call reaches exactly the
    /// resource a read of that ARN reports, and an ARN for an account this store
    /// never minted resolves to nothing. Only `region`'s own records are
    /// considered, so an ARN naming another region's resource resolves to
    /// nothing here, just as that resource is unreachable through this endpoint.
    pub fn resolve_tagged_resource(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<TaggedResource> {
        for monitor in self.monitors_in(region)?.values() {
            if monitor.monitor_arn == resource_arn {
                return Some(TaggedResource::Monitor {
                    region: region.to_string(),
                    monitor_name: monitor.monitor_name.clone(),
                });
            }
            for probe in monitor.probes.values() {
                if probe.probe_arn == resource_arn {
                    return Some(TaggedResource::Probe {
                        region: region.to_string(),
                        monitor_name: monitor.monitor_name.clone(),
                        probe_id: probe.probe_id.clone(),
                    });
                }
            }
        }
        None
    }

    /// Which stored resource a MUTATING call's `resource_arn` names: the lookup
    /// [`Self::resolve_tagged_resource`] performs, minus a resource whose
    /// teardown has already been accepted.
    ///
    /// A resource on its way out ([`Probe::is_being_torn_down`]) is no longer a
    /// target for a call that would change what it still reports: the teardown
    /// is irreversible, so nothing puts the resource back into service, and the
    /// record leaves the store once the clock has carried it through, so a write
    /// to it would persist nothing a caller can read back. Such an ARN resolves
    /// to nothing here, which is what makes a mutation of it refused exactly as
    /// an ARN no stored resource carries is. The reporting resolver still
    /// answers with it, because a read still reports it — in the teardown state
    /// it has reached — until the record is gone.
    pub fn resolve_tagged_resource_for_mutation(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<TaggedResource> {
        let resource = self.resolve_tagged_resource(region, resource_arn)?;
        let being_torn_down = match &resource {
            // A monitor's removal is immediate ([`Self::remove_monitor`]): a
            // monitor the store still holds has no teardown under way.
            TaggedResource::Monitor { .. } => false,
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => self
                .monitor(region, monitor_name)
                .and_then(|monitor| monitor.probes.get(probe_id))
                .is_some_and(Probe::is_being_torn_down),
        };
        (!being_torn_down).then_some(resource)
    }

    /// The tag map stored on one resolved resource, for reporting.
    ///
    /// The same single map [`Self::tags_mut`] hands a tagging write, so what
    /// `ListTagsForResource` reports and what `GetMonitor` reports for the same
    /// resource can never disagree.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => self
                .monitor(region, monitor_name)
                .map(|monitor| &monitor.tags),
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => self
                .monitor(region, monitor_name)
                .and_then(|monitor| monitor.probes.get(probe_id))
                .map(|probe| &probe.tags),
        }
    }

    /// The tag map stored on one resolved resource, for writing.
    ///
    /// There is ONE tag map per resource — the same one `GetMonitor` reports —
    /// so a tagging write and a read can never disagree.
    pub fn tags_mut(&mut self, resource: &TaggedResource) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => self
                .monitor_mut(region, monitor_name)
                .map(|monitor| &mut monitor.tags),
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => self
                .monitor_mut(region, monitor_name)
                .and_then(|monitor| monitor.probes.get_mut(probe_id))
                .map(|probe| &mut probe.tags),
        }
    }

    /// Record that a call CHANGED one resolved resource, moving its
    /// `modifiedAt` to `now`.
    ///
    /// Tags are part of the resource every read reports, so a call that removes
    /// or adds one modified the resource. Called only when the stored tag map
    /// actually changed: a tagging call that is a no-op modified nothing.
    ///
    /// A write that landed in a PROBE rewrote a record the monitor holds and
    /// every read of the monitor reports, so it changed the monitor too and the
    /// monitor's own `modifiedAt` moves with the probe's
    /// ([`Monitor::follow_probe_write`]) — the one rule every probe-writing path
    /// of this crate applies, whatever member of the probe it rewrote.
    pub fn mark_modified(&mut self, resource: &TaggedResource, now: SystemTime) {
        match resource {
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => {
                if let Some(monitor) = self.monitor_mut(region, monitor_name) {
                    monitor.modified_at = now;
                }
            }
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => {
                let Some(monitor) = self.monitor_mut(region, monitor_name) else {
                    return;
                };
                let Some(probe) = monitor.probes.get_mut(probe_id) else {
                    return;
                };
                probe.modified_at = now;
                monitor.follow_probe_write(true, now);
            }
        }
    }

    /// Forget every `clientToken` record past [`CLIENT_TOKEN_TTL`], then the
    /// oldest records beyond [`MAX_CLIENT_TOKENS`].
    ///
    /// This is the one store whose KEYS the caller chooses, so it is pruned on
    /// every create rather than left to grow with the request count.
    pub fn prune_client_tokens(&mut self, now: SystemTime) {
        self.client_tokens.retain(|_, record| {
            record
                .recorded_at
                .checked_add(CLIENT_TOKEN_TTL)
                .is_none_or(|expires_at| now < expires_at)
        });
        while self.client_tokens.len() > MAX_CLIENT_TOKENS {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.client_tokens.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmonitorNetworkmonitorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmonitorNetworkmonitorState>)
}
