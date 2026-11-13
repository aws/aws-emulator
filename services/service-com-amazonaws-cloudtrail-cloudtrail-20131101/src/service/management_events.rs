// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `cloudtrail.amazonaws.com`, recorded in the account's event history where this
//! service's own `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event — read-only calls
//! included — which makes the recording a property of SERVING a call and not of any one
//! operation. So it is not written per handler: an operation that records its own call
//! records the same facts as its neighbour, and an operation nobody remembered to edit
//! records nothing at all. The shared wrapper ([`service_common::audited_operation`]) is
//! handed the deserialized input before the handler runs, the handler's own `Result`
//! after, and the rendered response last; this module is only what THIS service
//! contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the resources a call addressed ([`CloudTrailAudit::resources`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot
//!   read off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with
//! ([`service_common::audited_operation::AuditedDefinition`]), so no request can be served
//! by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{
    recorded_member_name, AuditedResource, ManagementEventLog, RecordedSession, ServedApiCall,
};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

use crate::state::ResourceKind;

/// The `eventSource` every record of this service carries: CloudTrail's endpoint prefix
/// (`awsService.endpointPrefix` in the model) in the `<prefix>.amazonaws.com` form
/// CloudTrail stamps on every event. Stated once for the whole service in
/// [`crate::state::EVENT_SOURCE`], so the recorder and anything else that names this
/// service's source cannot drift apart.
pub use crate::state::EVENT_SOURCE;

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// `LookupEvents` reads — so it is generic over the engine type exactly as the service is.
pub struct CloudTrailAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> CloudTrailAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        crate::state::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for CloudTrailAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session: the shared store the minting service wrote it to.
        crate::state::recorded_session(&*self.engine, Some(access_key_id))
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it — which is also the Region a lookup reports its events from.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    fn resources(
        &self,
        operation: &str,
        request_parameters: &serde_json::Value,
        region: &str,
    ) -> Vec<AuditedResource> {
        let _ = region;
        addressed_resources(operation, request_parameters)
    }

    fn sensitive_members(&self) -> &'static [&'static str] {
        // This service's model declares no `@sensitive` member at all, so no value a
        // request carries is one its records withhold.
        &[]
    }

    fn stopped_at_authorization(
        &self,
        operation: &str,
        shape_name: &str,
        http_status: u16,
    ) -> bool {
        // This service authorizes its OWN API: every call it serves is decided against the
        // calling principal's identity-based policies by one gate
        // ([`super::authorization`]) BEFORE the operation reads or writes anything, and the
        // refusal that gate answers is this code with this status. A call refused that way
        // reached no operation, so its record carries neither the parameters it would have
        // been dispatched with nor any response elements. No handler of this service
        // answers this error of its own — the two operations whose model declares the shape
        // (`EnableFederation`, `DisableFederation`) refuse for their own reasons in their
        // own error shapes — so the operation does not narrow it.
        let _ = operation;
        shape_name == crate::scaffolded::ACCESS_DENIED_CODE
            && http_status == crate::scaffolded::ACCESS_DENIED_STATUS
    }
}

/// The member every operation that addresses an existing resource of ONE kind carries,
/// with the kind it names.
///
/// Each pair is the identifier member of a request shape, as the model spells it
/// (`smithy_query(query="digest")` lists every input shape; the members below are the ones
/// those shapes carry): `TrailName` on the selector and event-configuration pairs,
/// `EventDataStore` on the Lake store lifecycle, `Channel` on the channel operations and
/// `DashboardId` on the dashboard ones. The member alone decides the kind, so it holds for
/// every operation that carries a member of that name.
const IDENTIFIER_MEMBERS: &[(&str, ResourceKind)] = &[
    ("TrailName", ResourceKind::Trail),
    ("EventDataStore", ResourceKind::EventDataStore),
    ("Channel", ResourceKind::Channel),
    ("DashboardId", ResourceKind::Dashboard),
];

/// The operations whose `Name` member names a TRAIL: the trail lifecycle and the two
/// logging switches, each of which carries the trail's name or ARN at `Name`
/// (`smithy_query(query="digest")` -> `CreateTrailRequest`, `UpdateTrailRequest`,
/// `DeleteTrailRequest`, `GetTrailRequest`, `GetTrailStatusRequest`,
/// `StartLoggingRequest`, `StopLoggingRequest`).
///
/// `Name` is the ONE identifier member whose resource kind is the operation's rather than
/// the member's: the three other creates carry a `Name` too, and each names a resource of
/// its own kind (see [`NAMED_BY_OPERATION`]).
const TRAIL_NAME_OPERATIONS: &[&str] = &[
    "CreateTrail",
    "UpdateTrail",
    "DeleteTrail",
    "GetTrail",
    "GetTrailStatus",
    "StartLogging",
    "StopLogging",
];

/// Which kind of resource an operation's `Name` member names, for the operations whose
/// `Name` is not a trail's: each create names the resource it creates.
const NAMED_BY_OPERATION: &[(&str, ResourceKind)] = &[
    ("CreateEventDataStore", ResourceKind::EventDataStore),
    ("CreateChannel", ResourceKind::Channel),
    ("CreateDashboard", ResourceKind::Dashboard),
];

/// The members carrying a CloudTrail resource ARN, whose own resource segment names both
/// the kind and the resource: `ResourceId` on the tagging pair, `ResourceArn` on the
/// resource-policy operations.
const ARN_MEMBERS: &[&str] = &["ResourceId", "ResourceArn"];

/// The members carrying a LIST of resource identifiers: the trail names or ARNs
/// `DescribeTrails` reports on, and the resource ARNs `ListTags` reads the tags of.
///
/// `trailNameList` is spelled as THIS model spells it — lower-case first, unlike every
/// other member name here — because the set is matched on the modelled name.
const IDENTIFIER_LIST_MEMBERS: &[(&str, Option<ResourceKind>)] = &[
    ("trailNameList", Some(ResourceKind::Trail)),
    // Every entry is an ARN, which names its own kind.
    ("ResourceIdList", None),
];

/// The resources the call `operation` served addressed, from the identifiers its request
/// carried.
///
/// A record names the resource a call acted on so that a lookup by `ResourceName` or
/// `ResourceType` selects it, and `Resource$ResourceName` is documented as "the name of
/// the resource referenced by the event … user-created names" — so an identifier is folded
/// to the resource's NAME whether the caller wrote the bare name or the resource's ARN. An
/// identifier that names no CloudTrail resource of a known kind contributes nothing rather
/// than a resource built from a value the service does not recognise.
fn addressed_resources(
    operation: &str,
    request_parameters: &serde_json::Value,
) -> Vec<AuditedResource> {
    let mut addressed: Vec<AuditedResource> = Vec::new();
    for (kind, identifier) in addressed_identifiers(operation, request_parameters).named {
        if let Some(resource) = named_resource(kind, identifier) {
            if !addressed.contains(&resource) {
                addressed.push(resource);
            }
        }
    }
    addressed
}

/// How many identifiers of ONE request are read, at most.
///
/// Two of the members above carry a LIST the caller writes ([`IDENTIFIER_LIST_MEMBERS`]),
/// so without a bound here the work one request costs — the resources its record names, and
/// the resources the decision ahead of it is taken on — is chosen by the caller. The bound
/// is far above every list CloudTrail itself accepts: `ListTags` "has a limit of 20 ARNs"
/// for its `ResourceIdList`
/// (`smithy_query(query="doc", shape="ListTagsRequest", member="ResourceIdList")`), and the
/// account's trails-per-Region quota is a single digit, so no request the service serves
/// reaches it; what it bounds is a request that names more identifiers than any list AWS
/// accepts.
pub(crate) const MAX_ADDRESSED_IDENTIFIERS: usize = 64;

/// The identifiers of one request that name a CloudTrail resource, read up to
/// [`MAX_ADDRESSED_IDENTIFIERS`].
pub(crate) struct AddressedIdentifiers<'a> {
    /// Each identifier, with the kind its MEMBER names — `None` for a member whose value is
    /// an ARN, which names its own kind.
    pub named: Vec<(Option<ResourceKind>, &'a str)>,
    /// Set when the request carried MORE identifiers than the bound, so `named` holds only
    /// the ones that were read.
    pub beyond_the_bound: bool,
}

/// Every identifier the call `operation` carried that names a CloudTrail resource, each
/// with the kind its MEMBER names.
///
/// The ONE reading of which member of a request addresses which resource, so the record of
/// a call ([`addressed_resources`]) and the authorization decision taken ahead of it
/// ([`super::authorization`]) cannot disagree about what a request addressed. Order is the
/// order the member tables above state, a member the request did not carry contributes
/// nothing, and no more than [`MAX_ADDRESSED_IDENTIFIERS`] are read.
pub(crate) fn addressed_identifiers<'a>(
    operation: &str,
    request_parameters: &'a serde_json::Value,
) -> AddressedIdentifiers<'a> {
    let mut named: Vec<(Option<ResourceKind>, &'a str)> = Vec::new();
    let mut beyond_the_bound = false;
    let mut add = |kind: Option<ResourceKind>, identifier: &'a str| {
        if named.len() < MAX_ADDRESSED_IDENTIFIERS {
            named.push((kind, identifier));
        } else {
            beyond_the_bound = true;
        }
    };
    for (member, kind) in IDENTIFIER_MEMBERS {
        if let Some(identifier) = member_text(request_parameters, member) {
            add(Some(*kind), identifier);
        }
    }
    if let Some(kind) = name_member_kind(operation) {
        if let Some(identifier) = member_text(request_parameters, "Name") {
            add(Some(kind), identifier);
        }
    }
    for member in ARN_MEMBERS {
        if let Some(identifier) = member_text(request_parameters, member) {
            add(None, identifier);
        }
    }
    for (member, kind) in IDENTIFIER_LIST_MEMBERS {
        for identifier in member_list(request_parameters, member) {
            add(*kind, identifier);
        }
    }
    AddressedIdentifiers {
        named,
        beyond_the_bound,
    }
}

/// Which resource kind `operation`'s `Name` member names, or `None` for an operation that
/// carries no `Name` naming a resource.
fn name_member_kind(operation: &str) -> Option<ResourceKind> {
    if TRAIL_NAME_OPERATIONS.contains(&operation) {
        return Some(ResourceKind::Trail);
    }
    NAMED_BY_OPERATION
        .iter()
        .find(|(named, _)| *named == operation)
        .map(|(_, kind)| *kind)
}

/// The resource `identifier` names: its kind and its user-created name.
///
/// An `arn:` value names its own kind, read off the ARN's resource segment, so a caller
/// that addresses a resource by ARN and one that addresses it by name are recorded as the
/// same resource. `kind` is the kind the MEMBER names, used for a bare identifier; an ARN
/// that is not a CloudTrail resource ARN of a known kind names no resource.
fn named_resource(kind: Option<ResourceKind>, identifier: &str) -> Option<AuditedResource> {
    if identifier.starts_with("arn:") {
        let (_region, _account, resource) = crate::state::cloudtrail_arn_fields(identifier)?;
        let (kind, name) = ResourceKind::ALL.into_iter().find_map(|candidate| {
            resource
                .strip_prefix(candidate.resource_prefix())
                .map(|name| (candidate, name))
        })?;
        return (!name.is_empty())
            .then(|| AuditedResource::new(kind.audited_resource_type(), name.to_string()));
    }
    let kind = kind?;
    (!identifier.is_empty())
        .then(|| AuditedResource::new(kind.audited_resource_type(), identifier.to_string()))
}

/// The text a record's `requestParameters` holds at the modelled member `member`.
///
/// The document names its members the way a record spells them
/// ([`recorded_member_name`]), which is the one place that spelling is decided.
fn member_text<'a>(request_parameters: &'a serde_json::Value, member: &str) -> Option<&'a str> {
    request_parameters
        .get(recorded_member_name(member))
        .and_then(serde_json::Value::as_str)
}

/// The texts a record's `requestParameters` holds in the list at the modelled member
/// `member`.
fn member_list<'a>(request_parameters: &'a serde_json::Value, member: &str) -> Vec<&'a str> {
    request_parameters
        .get(recorded_member_name(member))
        .and_then(serde_json::Value::as_array)
        .map(|entries| entries.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default()
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsJson1_1`, which is the same value the generated per-operation
/// shim passes.
pub(crate) const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.1",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// A call that only REPORTS state, as the table's rows name that kind.
const fn reports() -> CallKind {
    CallKind::Reporting
}

/// A call that CHANGES the account's resources.
const fn mutates() -> CallKind {
    CallKind::Mutating
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// The operation is served behind this service's AUTHORIZATION gate
/// ([`super::authorization::Authorized`]), INSIDE the audit recorder: IAM decides a call
/// before the operation acts on it, and a call refused that way is still a management event
/// of the account.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve(
                super::authorization::Authorized::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                    ::std::sync::Arc::clone($engine),
                ),
                $kind(),
            );
        )*
    }};
}

/// Serve one operation audited BEHIND this service's modelled-refusal wrapper, as a call
/// of the stated kind.
///
/// The audited call stays inside the wrapper, so the record reports the operation's own
/// input and output whether or not the wrapper refused the request ahead of the handler.
macro_rules! audited_in_modelled_envelope {
    (
        $table:expr, $engine:expr, $refusal:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve_behind(
                super::authorization::Authorized::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                    ::std::sync::Arc::clone($engine),
                ),
                $kind(),
                |audited, _pre_dispatch| super::InModelledEnvelope::new(audited, $refusal),
            );
        )*
    }};
}

/// Every operation this service serves, each constructed from the shared engine and served
/// behind the audit recorder.
///
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Get*` / `List*` /
/// `Describe*` / `Search*` readers and for `LookupEvents` and `GenerateQuery`, which report
/// state and change none of it, and `mutates` for every operation that creates, changes,
/// starts, stops or removes a resource — `StartQuery` and `StartDashboardRefresh`
/// included, since each one submits work the account then holds.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(CloudTrailAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, JSON_ENVELOPE);
    audited!(
        table, engine,
        cancel_query::CancelQuery => mutates,
        delete_channel::DeleteChannel => mutates,
        delete_dashboard::DeleteDashboard => mutates,
        delete_event_data_store::DeleteEventDataStore => mutates,
        delete_resource_policy::DeleteResourcePolicy => mutates,
        delete_trail::DeleteTrail => mutates,
        deregister_organization_delegated_admin::DeregisterOrganizationDelegatedAdmin => mutates,
        describe_query::DescribeQuery => reports,
        describe_trails::DescribeTrails => reports,
        disable_federation::DisableFederation => mutates,
        enable_federation::EnableFederation => mutates,
        generate_query::GenerateQuery => reports,
        get_channel::GetChannel => reports,
        get_dashboard::GetDashboard => reports,
        get_event_configuration::GetEventConfiguration => reports,
        get_event_data_store::GetEventDataStore => reports,
        get_event_selectors::GetEventSelectors => reports,
        get_import::GetImport => reports,
        get_insight_selectors::GetInsightSelectors => reports,
        get_query_results::GetQueryResults => reports,
        get_resource_policy::GetResourcePolicy => reports,
        get_trail::GetTrail => reports,
        get_trail_status::GetTrailStatus => reports,
        list_channels::ListChannels => reports,
        list_dashboards::ListDashboards => reports,
        list_import_failures::ListImportFailures => reports,
        list_imports::ListImports => reports,
        list_insights_data::ListInsightsData => reports,
        list_insights_metric_data::ListInsightsMetricData => reports,
        list_public_keys::ListPublicKeys => reports,
        list_tags::ListTags => reports,
        list_trails::ListTrails => reports,
        lookup_events::LookupEvents => reports,
        put_event_configuration::PutEventConfiguration => mutates,
        put_event_selectors::PutEventSelectors => mutates,
        put_insight_selectors::PutInsightSelectors => mutates,
        put_resource_policy::PutResourcePolicy => mutates,
        register_organization_delegated_admin::RegisterOrganizationDelegatedAdmin => mutates,
        restore_event_data_store::RestoreEventDataStore => mutates,
        search_sample_queries::SearchSampleQueries => reports,
        start_dashboard_refresh::StartDashboardRefresh => mutates,
        start_event_data_store_ingestion::StartEventDataStoreIngestion => mutates,
        start_import::StartImport => mutates,
        start_logging::StartLogging => mutates,
        start_query::StartQuery => mutates,
        stop_event_data_store_ingestion::StopEventDataStoreIngestion => mutates,
        stop_import::StopImport => mutates,
        stop_logging::StopLogging => mutates,
        update_channel::UpdateChannel => mutates,
        update_dashboard::UpdateDashboard => mutates,
        update_event_data_store::UpdateEventDataStore => mutates,
        update_trail::UpdateTrail => mutates,
    );
    // The operations whose input carries a tag list: a tag key or value outside its
    // modelled length is refused in this service's own error shape ahead of the handler,
    // which is what `super::InModelledEnvelope` is for.
    audited_in_modelled_envelope!(
        table,
        engine,
        super::ModelledRefusal::InvalidTagParameter,
        add_tags::AddTags => mutates,
        create_channel::CreateChannel => mutates,
        create_dashboard::CreateDashboard => mutates,
        create_event_data_store::CreateEventDataStore => mutates,
        create_trail::CreateTrail => mutates,
        remove_tags::RemoveTags => mutates,
    );
    // The paged operations whose model declares `InvalidMaxResultsException` for the
    // member it declares as its `@paginated` page size.
    audited_in_modelled_envelope!(
        table,
        engine,
        super::ModelledRefusal::InvalidMaxResults,
        list_event_data_stores::ListEventDataStores => reports,
        list_queries::ListQueries => reports,
    );
    table.finish()
}
