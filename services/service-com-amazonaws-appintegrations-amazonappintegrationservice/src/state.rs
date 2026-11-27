// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appintegrations-amazonappintegrationservice service.
//!
//! Besides the state struct itself this module holds the few helpers the
//! application, event-integration and data-integration handlers share — ARN
//! construction, identifier resolution, the request's region, the not-found wording,
//! the record-to-modelled-shape conversions and the list cursor codec — so the
//! operation files agree on one implementation of each instead of carrying a copy
//! apiece that can drift apart.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    ApplicationAssociationSummary, ApplicationConfig, ApplicationSourceConfig, AuthConfig,
    ContactHandling, DataIntegrationAssociationSummary, DataIntegrationSummary, EventFilter,
    EventIntegration, EventIntegrationAssociation, ExecutionConfiguration, ExternalUrlConfig,
    FileConfiguration, IframeConfig, Publication, ScheduleConfiguration, Subscription,
};

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: app-integrations`).
pub const ARN_NAMESPACE: &str = "app-integrations";

/// The page size a paginated `List` uses when the caller sends no `MaxResults`: the
/// largest page the model allows (`MaxResults` range 1..50 on both
/// `ListEventIntegrations` and `ListDataIntegrations`).
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// The most event integrations this emulator stores at once.
///
/// AWS caps EventIntegrations with a service quota and reports an exceeded cap as
/// `ResourceQuotaExceededException` ("the allowed quota for the resource has been
/// exceeded"), which `CreateEventIntegration` models. The NUMBER here is this
/// emulator's own bound, not an AWS quota: it keeps an in-memory store that only a
/// caller's delete ever shrinks from growing without end, and sits far above anything a
/// test or a client script creates.
pub const MAX_STORED_EVENT_INTEGRATIONS: usize = 1_000;

/// The most applications this emulator stores at once.
///
/// AWS caps Applications with a service quota and reports an exceeded cap as
/// `ResourceQuotaExceededException` ("the allowed quota for the resource has been
/// exceeded"), which `CreateApplication` models. The NUMBER here is this emulator's own
/// bound, not an AWS quota: it keeps an in-memory store that only a caller's delete ever
/// shrinks from growing without end, and sits far above anything a test or a client
/// script creates.
pub const MAX_STORED_APPLICATIONS: usize = 1_000;

/// The most data integrations this emulator stores at once.
///
/// AWS caps DataIntegrations with a service quota and reports an exceeded cap as
/// `ResourceQuotaExceededException` ("the allowed quota for the resource has been
/// exceeded"), which `CreateDataIntegration` models. The NUMBER here is this emulator's
/// own bound, not an AWS quota: it keeps an in-memory store that only a caller's delete
/// ever shrinks from growing without end, and sits far above anything a test or a
/// client script creates.
pub const MAX_STORED_DATA_INTEGRATIONS: usize = 1_000;

/// The most data integration associations this emulator stores at once.
///
/// AWS caps DataIntegrationAssociations with a service quota and reports an exceeded cap
/// as `ResourceQuotaExceededException`, which `CreateDataIntegrationAssociation` models.
/// The NUMBER here is this emulator's own bound, for the same reason as
/// [`MAX_STORED_DATA_INTEGRATIONS`]: an in-memory store a caller must not be able to
/// grow without end.
pub const MAX_STORED_DATA_INTEGRATION_ASSOCIATIONS: usize = 1_000;

/// The most `ClientToken` -> data integration entries this emulator remembers at once.
///
/// The create writes one per stored integration and the delete drops the one that
/// minted the integration it removes, so this index can never hold more entries than
/// there are stored integrations: the ceiling is stated as the same number so the two
/// cannot drift apart. [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_client_token`]
/// enforces it without refusing anything — it first drops entries whose integration is
/// gone, and past the ceiling simply does not remember the token (a forgotten token
/// makes a replay create a fresh resource, which the integration ceiling already
/// bounds).
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = MAX_STORED_DATA_INTEGRATIONS;

/// The most `ClientToken` -> event integration entries this emulator remembers at
/// once, the [`MAX_REMEMBERED_CLIENT_TOKENS`] of the event-integration index and
/// enforced the same way, by
/// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_event_client_token`].
pub const MAX_REMEMBERED_EVENT_CLIENT_TOKENS: usize = MAX_STORED_EVENT_INTEGRATIONS;

/// The most `ClientToken` -> application entries this emulator remembers at once, the
/// [`MAX_REMEMBERED_CLIENT_TOKENS`] of the application index and enforced the same way,
/// by [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_application_client_token`].
pub const MAX_REMEMBERED_APPLICATION_CLIENT_TOKENS: usize = MAX_STORED_APPLICATIONS;

/// The most `ClientToken` -> data integration association entries this emulator
/// remembers at once, the [`MAX_REMEMBERED_CLIENT_TOKENS`] of the association index and
/// enforced the same way, by
/// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::remember_association_client_token`].
pub const MAX_REMEMBERED_ASSOCIATION_CLIENT_TOKENS: usize =
    MAX_STORED_DATA_INTEGRATION_ASSOCIATIONS;

/// The resource type name AppIntegrations calls an event integration in the text of a
/// `ResourceNotFoundException` — the Smithy shape name of the resource
/// (`smithy_query(query="operation", operation="GetEventIntegration")`: the resource is
/// addressed by its `Name`).
pub const EVENT_INTEGRATION_RESOURCE: &str = "EventIntegration";

/// The same, for a data integration, which is addressed by an opaque identifier rather
/// than by a name (`GetDataIntegration`'s `Identifier` label).
pub const DATA_INTEGRATION_RESOURCE: &str = "DataIntegration";

/// The same, for an application, which is addressed by an `ArnOrUUID` identifier
/// (`GetApplication`'s `Arn` label).
pub const APPLICATION_RESOURCE: &str = "Application";

/// The same, for a data integration association, addressed by an opaque identifier
/// (`UpdateDataIntegrationAssociation`'s `DataIntegrationAssociationIdentifier` label).
pub const DATA_INTEGRATION_ASSOCIATION_RESOURCE: &str = "DataIntegrationAssociation";

/// What a resource addressed by its `Name` member is called in a not-found message.
pub const ADDRESSED_BY_NAME: &str = "name";

/// What a resource addressed by an opaque `Identifier` is called in one.
pub const ADDRESSED_BY_IDENTIFIER: &str = "identifier";

/// What the PARENT identifier of a data integration association is called in the
/// two-part not-found message of that association (see
/// [`data_integration_association_not_found`]).
pub const ADDRESSED_BY_DATA_INTEGRATION_NAME: &str = "DataIntegrationName";

/// What the association's OWN identifier is called in the same message.
pub const ADDRESSED_BY_DATA_INTEGRATION_ASSOCIATION_ID: &str = "DataIntegrationAssociationId";

/// AppIntegrations' wording for a resource the request named and this account does not
/// hold: the resource TYPE, how the request addressed it, and the value it sent —
/// `"Could not find EventIntegration with name: demo."`,
/// `"Could not find DataIntegration with identifier: <id>."`.
///
/// Request-derived and one template for every resource of this service, so each handler
/// words its 404 through this function rather than inventing a sentence of its own: a
/// client or operator that reads the not-found text sees one shape across the API.
pub fn not_found_message(resource_type: &str, addressed_by: &str, value: &str) -> String {
    format!("Could not find {resource_type} with {addressed_by}: {value}.")
}

/// [`not_found_message`] for the event integration `name` the request named.
pub fn event_integration_not_found(name: &str) -> String {
    not_found_message(EVENT_INTEGRATION_RESOURCE, ADDRESSED_BY_NAME, name)
}

/// [`not_found_message`] for the data integration `identifier` the request named.
pub fn data_integration_not_found(identifier: &str) -> String {
    not_found_message(DATA_INTEGRATION_RESOURCE, ADDRESSED_BY_IDENTIFIER, identifier)
}

/// [`not_found_message`] for the application `identifier` the request named.
pub fn application_not_found(identifier: &str) -> String {
    not_found_message(APPLICATION_RESOURCE, ADDRESSED_BY_IDENTIFIER, identifier)
}

/// AppIntegrations' wording for a resource the request addressed by a PAIR of
/// identifiers and this account does not hold: the resource TYPE, then each identifier
/// named by the member it arrived on together with the value the request sent.
///
/// A resource a request addresses by naming both its parent and itself is reported with
/// BOTH values, because either of the two can be the one that resolved to nothing and
/// the single-identifier template ([`not_found_message`]) would drop the other from the
/// diagnostic.
pub fn not_found_message_for_pair(
    resource_type: &str,
    first: (&str, &str),
    second: (&str, &str),
) -> String {
    let (first_label, first_value) = first;
    let (second_label, second_value) = second;
    format!(
        "Could not find {resource_type} with {first_label}: {first_value} and \
         {second_label}: {second_value}."
    )
}

/// [`not_found_message_for_pair`] for the data integration association a request
/// addressed by the two identifiers its path carries: the parent data integration and
/// the association within it.
///
/// The association is the resource such a call addresses, so this one sentence reports
/// it whichever of the two identifiers resolved to nothing — a parent that names no
/// stored integration is not reported as a missing integration, because the request
/// asked about the association.
pub fn data_integration_association_not_found(
    data_integration_identifier: &str,
    data_integration_association_identifier: &str,
) -> String {
    not_found_message_for_pair(
        DATA_INTEGRATION_ASSOCIATION_RESOURCE,
        (
            ADDRESSED_BY_DATA_INTEGRATION_NAME,
            data_integration_identifier,
        ),
        (
            ADDRESSED_BY_DATA_INTEGRATION_ASSOCIATION_ID,
            data_integration_association_identifier,
        ),
    )
}

/// The persisted tags as the modelled `TagMap`.
///
/// An untagged resource reports an EMPTY map, never an absent member: "this resource
/// has no tags" is an answer the service owns, and a client that reads an absent member
/// cannot tell it from "the tag set was not reported". The model constrains `TagMap` to
/// 1..200 entries, yet the recorded AWS responses in corpus `simparity-wire-aws-v1`
/// carry `"Tags": {}` for an untagged application and for an untagged data integration,
/// so the wire is the authority for the empty case and every read path of this service
/// reports it the same way.
pub fn modeled_tags(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// The most tags one resource of this service carries at once: the maximum the modelled
/// `TagMap` admits (`smithy_query(query="operation", operation="TagResource")`: `tags`
/// is a `TagMap` of `len 1..200`, and the same shape is what every read reports back),
/// so a tag set this service stores is one it can also report.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The ARN resource-type segment of an application, the part before the `/` in
/// `arn:aws:app-integrations:<region>:<account>:application/<id>`.
pub const APPLICATION_ARN_SEGMENT: &str = "application";

/// The ARN resource-type segment of an event integration.
pub const EVENT_INTEGRATION_ARN_SEGMENT: &str = "event-integration";

/// The ARN resource-type segment of a data integration.
pub const DATA_INTEGRATION_ARN_SEGMENT: &str = "data-integration";

/// The ARN resource-type segment of a data integration association.
pub const DATA_INTEGRATION_ASSOCIATION_ARN_SEGMENT: &str = "data-integration-association";

/// The ARN resource-type segment of an application association.
pub const APPLICATION_ARN_ASSOCIATION_SEGMENT: &str = "application-association";

/// The ARN resource-type segment of an event integration association.
pub const EVENT_INTEGRATION_ASSOCIATION_ARN_SEGMENT: &str = "event-integration-association";

/// The sentence the TAGGING operations give a `ResourceArn` this account holds no
/// taggable resource under: fixed text, then the ARN the request sent, verbatim.
///
/// Deliberately NOT the resource-scoped [`not_found_message`] family the
/// `Get`/`Update`/`Delete` paths word their 404s through. Those address ONE resource
/// type by that type's own identifier, so they can name it; a tagging operation is
/// handed an arbitrary ARN instead (`smithy_query(query="operation",
/// operation="TagResource,UntagResource,ListTagsForResource")`: `resourceArn` targets
/// the plain `Arn` shape, bound to no resource) and resolves no resource type before
/// refusing — so its refusal names none either, whatever type the ARN itself claims,
/// and a caller is told about exactly the value it sent.
pub fn tagged_resource_not_found(arn: &str) -> String {
    format!("Could not find resource with Arn: {arn}.")
}

/// The `ApplicationType` of an application whose create named none, which is a value
/// the SERVICE assigns rather than one the caller supplies: `STANDARD` for an ordinary
/// application, and `SERVICE` for one the deprecated `IsService` flag marks as a
/// service — the model deprecates `IsService` "in favor of ApplicationType", so the two
/// members name one fact about the application. Both are values of the modelled enum
/// (`smithy_query(query="shape", shape="ApplicationType")`: STANDARD | SERVICE |
/// MCP_SERVER | A2A_SERVER).
pub fn assigned_application_type(is_service: bool) -> &'static str {
    if is_service {
        APPLICATION_TYPE_SERVICE
    } else {
        APPLICATION_TYPE_STANDARD
    }
}

/// The two `ApplicationType` values this service assigns on its own (see
/// [`assigned_application_type`]); the other two are only ever a caller's choice.
pub const APPLICATION_TYPE_STANDARD: &str = "STANDARD";
pub const APPLICATION_TYPE_SERVICE: &str = "SERVICE";

/// The `IntegrationType` this service assigns an integration that `CreateDataIntegration`
/// minted: that operation creates an integration which PULLS data from an external
/// source into the service ("the URI of the data source", "the configuration for what
/// files should be pulled from the source"), and such an integration is classified as an
/// import. It is a constant of that class, not a per-run value: the recorded AWS
/// `GetDataIntegration` responses in corpus `simparity-wire-aws-v1` report it for the
/// integrations the corpus creates.
pub const INTEGRATION_TYPE_IMPORT: &str = "IMPORT";

/// Whether two values of a modelled shape this service stores carry the same data.
///
/// The modules under [`crate::ops::types`] are rendered from the service's Smithy model
/// and derive `Debug`, `Clone` and `Default` only, so a stored configuration shape
/// cannot be compared with `==`. An update has to tell a member that actually CHANGED
/// from one re-sent exactly as it is already stored — that is what decides whether the
/// resource's own clock moves — so the comparison lives here, member by member, and is
/// written once for every handler instead of once per operation file.
///
/// Two values are the same when every member is: a member this service keeps as a plain
/// `String`, number or boolean compares with `==`, a nested shape compares through this
/// trait, and an absent member equals only an absent one.
pub trait SameValue {
    /// Whether `self` and `other` carry the same data.
    fn same_value(&self, other: &Self) -> bool;
}

impl<T: SameValue> SameValue for Option<T> {
    fn same_value(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(left), Some(right)) => left.same_value(right),
            (None, None) => true,
            _ => false,
        }
    }
}

/// A list is the same as another when it holds the same entries in the same ORDER: the
/// model's lists are sequences, so a reordering is a change.
impl<T: SameValue> SameValue for Vec<T> {
    fn same_value(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(left, right)| left.same_value(right))
    }
}

impl SameValue for ApplicationSourceConfig {
    fn same_value(&self, other: &Self) -> bool {
        self.external_url_config
            .same_value(&other.external_url_config)
    }
}

impl SameValue for ExternalUrlConfig {
    fn same_value(&self, other: &Self) -> bool {
        self.access_url == other.access_url && self.approved_origins == other.approved_origins
    }
}

impl SameValue for Subscription {
    fn same_value(&self, other: &Self) -> bool {
        self.event == other.event && self.description == other.description
    }
}

impl SameValue for Publication {
    fn same_value(&self, other: &Self) -> bool {
        self.event == other.event
            && self.schema == other.schema
            && self.description == other.description
    }
}

impl SameValue for ApplicationConfig {
    fn same_value(&self, other: &Self) -> bool {
        self.contact_handling.same_value(&other.contact_handling)
    }
}

impl SameValue for ContactHandling {
    fn same_value(&self, other: &Self) -> bool {
        self.scope == other.scope
    }
}

impl SameValue for IframeConfig {
    fn same_value(&self, other: &Self) -> bool {
        self.allow == other.allow && self.sandbox == other.sandbox
    }
}

impl SameValue for AuthConfig {
    fn same_value(&self, other: &Self) -> bool {
        self.auth_type == other.auth_type
            && self.credential_provider_identifier == other.credential_provider_identifier
            && self.audience_override == other.audience_override
    }
}

/// The persisted `EventFilter` of an event integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilterRecord {
    pub source: String,
}

/// One stored event integration: everything `CreateEventIntegration` was given
/// plus the ARN the service minted for it, so `Get`/`List` report the identical
/// values `Create` returned instead of recomputing or echoing them.
#[derive(Debug, Clone)]
pub struct EventIntegrationRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub event_bridge_bus: String,
    pub event_filter: EventFilterRecord,
    pub tags: BTreeMap<String, String>,
    pub client_token: Option<String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl EventIntegrationRecord {
    /// The persisted event filter as the modelled shape.
    pub fn modeled_event_filter(&self) -> EventFilter {
        EventFilter {
            source: self.event_filter.source.clone(),
        }
    }

    /// This record's tags as the modelled `TagMap` (see [`modeled_tags`]).
    pub fn modeled_tags(&self) -> HashMap<String, String> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListEventIntegrations` summary.
    pub fn to_event_integration(&self) -> EventIntegration {
        EventIntegration {
            event_integration_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            event_filter: Some(self.modeled_event_filter()),
            event_bridge_bus: Some(self.event_bridge_bus.clone()),
            tags: Some(self.modeled_tags()),
        }
    }
}

/// One stored application: every member `CreateApplication` was given plus the id
/// and ARN the service minted for it, so `GetApplication` reports the identical
/// values instead of echoing a request or recomputing them.
///
/// The nested configuration members are kept as the modelled shapes they arrive
/// as — they are plain data this service only stores and reports, so a parallel
/// record type for each would add conversions without adding meaning.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    /// The `UUID` this service minted for the application: its identity, and the
    /// last segment of [`ApplicationRecord::arn`].
    pub id: String,
    pub arn: String,
    pub name: String,
    pub namespace: String,
    pub description: Option<String>,
    pub application_source_config: ApplicationSourceConfig,
    /// The members below that are NOT optional are the ones this service fills in for
    /// an application whose create named none of them, so a read reports the stored
    /// default rather than nothing at all: the three collections start EMPTY, the
    /// iframe configuration starts as the empty structure, and the application type
    /// starts as the one [`assigned_application_type`] assigns. An update replaces each
    /// of them whole, exactly as it replaces a value a create did supply.
    pub subscriptions: Vec<Subscription>,
    pub publications: Vec<Publication>,
    pub permissions: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub is_service: bool,
    pub initialization_timeout: Option<i32>,
    pub application_config: Option<ApplicationConfig>,
    pub iframe_config: IframeConfig,
    pub application_type: String,
    pub auth_config: Option<AuthConfig>,
    pub client_token: Option<String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The region the call that last changed this application was made in — the create's
    /// own region until an update actually changes a member, and that update's region
    /// afterwards (model: `GetApplicationResponse.LastModifiedRegion` and
    /// `ApplicationSummary.LastModifiedRegion`, "The Amazon Web Services Region where
    /// the application was last modified"). It moves with
    /// [`ApplicationRecord::last_modified_time`] and stays put whenever that does.
    pub last_modified_region: String,
}

impl ApplicationRecord {
    /// This record's tags as the modelled `TagMap` (see [`modeled_tags`]).
    pub fn modeled_tags(&self) -> HashMap<String, String> {
        modeled_tags(&self.tags)
    }
}

/// One stored data integration: everything `CreateDataIntegration` was given, plus the
/// id and ARN the service minted for it, so `Get`/`List` report the identical values
/// `Create` returned instead of recomputing or echoing them.
#[derive(Debug, Clone)]
pub struct DataIntegrationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub kms_key: String,
    /// How this service classifies the integration — a value it assigns, not one the
    /// create carries (see [`INTEGRATION_TYPE_IMPORT`]). No `Create`/`Get`/`List`
    /// output of the shipped model declares a member for it
    /// (`smithy_query(query="operation", operation="GetDataIntegration")`), so no
    /// operation reports it today; it is persisted because the classification belongs
    /// to the record rather than to one operation's response.
    pub integration_type: String,
    pub source_uri: Option<String>,
    pub schedule_configuration: Option<ScheduleConfiguration>,
    pub file_configuration: Option<FileConfiguration>,
    pub object_configuration: Option<HashMap<String, HashMap<String, Vec<String>>>>,
    pub tags: BTreeMap<String, String>,
    pub client_token: Option<String>,
    /// When this record was created, and when it was last changed. No `Create`/`Get`/
    /// `List` output models a timestamp, so neither is reported today; they are
    /// persisted because `UpdateDataIntegration` (a scaffold stub) is what moves the
    /// second one, and a resource's own times are part of the record rather than of
    /// one operation's response.
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl DataIntegrationRecord {
    /// This record's tags as the modelled `TagMap` (see [`modeled_tags`]).
    pub fn modeled_tags(&self) -> HashMap<String, String> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListDataIntegrations` summary — the three members that
    /// shape carries (`smithy_query(query="shape", shape="DataIntegrationSummary")`:
    /// `Arn`, `Name`, `SourceURI`).
    pub fn to_summary(&self) -> DataIntegrationSummary {
        DataIntegrationSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            source_uri: self.source_uri.clone(),
        }
    }
}

/// One stored data integration association: the client the data integration was
/// associated for, where it writes to, and how its execution is configured, plus the id
/// and ARN the service minted — so `ListDataIntegrationAssociations` reports what
/// `CreateDataIntegrationAssociation` persisted rather than echoing a request.
#[derive(Debug, Clone)]
pub struct DataIntegrationAssociationRecord {
    /// The `UUID` this service minted for the association: its identity, and the last
    /// segment of [`DataIntegrationAssociationRecord::arn`].
    pub id: String,
    pub arn: String,
    /// The id of the data integration this association belongs to, which is how the
    /// associations of one integration are found (see
    /// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::associations_of`]).
    pub data_integration_id: String,
    pub data_integration_arn: String,
    pub client_id: Option<String>,
    pub destination_uri: Option<String>,
    pub object_configuration: Option<HashMap<String, HashMap<String, Vec<String>>>>,
    pub client_association_metadata: Option<HashMap<String, String>>,
    pub execution_configuration: Option<ExecutionConfiguration>,
    pub client_token: Option<String>,
    /// When this record was created, and when its `ExecutionConfiguration` was last
    /// changed by `UpdateDataIntegrationAssociation`. No output of this service models a
    /// timestamp, so neither is reported; they are part of the record rather than of one
    /// operation's response.
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl DataIntegrationAssociationRecord {
    /// This record as a `ListDataIntegrationAssociations` summary — the members that
    /// shape carries (`smithy_query(query="shape",
    /// shape="DataIntegrationAssociationSummary")`).
    ///
    /// `LastExecutionStatus` is "the execution status of the last job": this emulator
    /// runs no import/export job, so no job has ever run for this association and the
    /// member is absent rather than carrying a status for work nothing performed.
    pub fn to_summary(&self) -> DataIntegrationAssociationSummary {
        DataIntegrationAssociationSummary {
            data_integration_association_arn: Some(self.arn.clone()),
            data_integration_arn: Some(self.data_integration_arn.clone()),
            client_id: self.client_id.clone(),
            destination_uri: self.destination_uri.clone(),
            last_execution_status: None,
            execution_configuration: self.execution_configuration.clone(),
        }
    }
}

/// One stored application association: the client an application is associated with,
/// the application it belongs to, and the ARN this service minted for the association —
/// so `ListApplicationAssociations` reports persisted records rather than a fixed answer.
///
/// Nothing in THIS API creates one: AppIntegrations publishes no create for an
/// application association (`smithy_query(query="digest")` binds a create for every
/// other resource of this service and none for this one). The association comes into
/// being on the side of the client that associates the application, which is why the
/// record's `client_id` is that client's identifier rather than anything this service
/// mints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationAssociationRecord {
    /// The identity of the association, and the last segment of
    /// [`ApplicationAssociationRecord::arn`].
    pub id: String,
    pub arn: String,
    /// The id of the application this association belongs to, which is how the
    /// associations of one application are found (see
    /// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::application_associations_of`]).
    pub application_id: String,
    pub application_arn: String,
    pub client_id: String,
}

impl ApplicationAssociationRecord {
    /// This record as a `ListApplicationAssociations` summary — the three members that
    /// shape carries (`smithy_query(query="shape",
    /// shape="ApplicationAssociationSummary")`: `ApplicationAssociationArn`,
    /// `ApplicationArn`, `ClientId`).
    pub fn to_summary(&self) -> ApplicationAssociationSummary {
        ApplicationAssociationSummary {
            application_association_arn: Some(self.arn.clone()),
            application_arn: Some(self.application_arn.clone()),
            client_id: Some(self.client_id.clone()),
        }
    }
}

/// One stored event integration association: the client an event integration is
/// associated with, the EventBridge rule that client's association runs on, and the id
/// and ARN this service minted for the association — so
/// `ListEventIntegrationAssociations` reports persisted records rather than a fixed
/// answer.
///
/// Nothing in THIS API creates one: AppIntegrations publishes no create for an event
/// integration association (`smithy_query(query="digest")` binds a create for every
/// other resource of this service and none for this one). The association comes into
/// being on the side of the client that associates the event integration, which is why
/// the record's `client_id` and `event_bridge_rule_name` are that client's values rather
/// than anything this service mints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventIntegrationAssociationRecord {
    /// The identity of the association, and the last segment of
    /// [`EventIntegrationAssociationRecord::arn`].
    pub id: String,
    pub arn: String,
    /// The name of the event integration this association belongs to, which is how the
    /// associations of one integration are found (see
    /// [`ComAmazonawsAppintegrationsAmazonappintegrationserviceState::event_integration_associations_of`]).
    pub event_integration_name: String,
    pub client_id: Option<String>,
    pub event_bridge_rule_name: Option<String>,
    pub client_association_metadata: BTreeMap<String, String>,
}

impl EventIntegrationAssociationRecord {
    /// This record as the `EventIntegrationAssociation` a listing reports — the members
    /// that shape carries (`smithy_query(query="shape",
    /// shape="EventIntegrationAssociation")`: `EventIntegrationAssociationArn`,
    /// `EventIntegrationAssociationId`, `EventIntegrationName`, `ClientId`,
    /// `EventBridgeRuleName`, `ClientAssociationMetadata`).
    ///
    /// `ClientAssociationMetadata` is reported only when the association carries any:
    /// the metadata belongs to the client that made the association, so an association
    /// stored without it has no map to report rather than an empty one.
    pub fn to_association(&self) -> EventIntegrationAssociation {
        EventIntegrationAssociation {
            event_integration_association_arn: Some(self.arn.clone()),
            event_integration_association_id: Some(self.id.clone()),
            event_integration_name: Some(self.event_integration_name.clone()),
            client_id: self.client_id.clone(),
            event_bridge_rule_name: self.event_bridge_rule_name.clone(),
            client_association_metadata: (!self.client_association_metadata.is_empty()).then(
                || {
                    self.client_association_metadata
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                },
            ),
        }
    }
}

/// Why `config` is not a valid `ExecutionConfiguration`, or `None` when it is.
///
/// ONE rule, shared by the create and the update of an association so the two cannot
/// disagree: `ExecutionMode` ("the mode for data import/export execution") selects which
/// nested configuration the association runs on, so exactly the block belonging to the
/// named mode has to be there — `ON_DEMAND` runs on an `OnDemandConfiguration`,
/// `SCHEDULED` on a `ScheduleConfiguration` — and the block belonging to the other mode
/// is never part of a configuration this service could act on. On top of that an
/// `OnDemandConfiguration`'s `StartTime` and `EndTime` are documented as "an Unix/epoch
/// string in milliseconds", so a value that is not one names no instant, and a window
/// whose end precedes its start names no window.
///
/// The returned sentence is built from the request (the member and the constraint it
/// broke), which is what the caller needs to fix it; the operations report it as the
/// `InvalidRequestException` ("the request is not valid") they model.
pub fn execution_configuration_problem(config: &ExecutionConfiguration) -> Option<String> {
    let on_demand_present = config.on_demand_configuration.is_some();
    let scheduled_present = config.schedule_configuration.is_some();
    let (expected, unexpected, required_block, other_block) = match config.execution_mode.as_str() {
        EXECUTION_MODE_ON_DEMAND => (
            on_demand_present,
            scheduled_present,
            "OnDemandConfiguration",
            "ScheduleConfiguration",
        ),
        EXECUTION_MODE_SCHEDULED => (
            scheduled_present,
            on_demand_present,
            "ScheduleConfiguration",
            "OnDemandConfiguration",
        ),
        // The boundary refuses a value outside the `ExecutionMode` enum before a handler
        // runs, so every mode reaching here is one of the two above.
        other => return Some(format!("ExecutionMode {other} is not supported.")),
    };
    if !expected {
        return Some(format!(
            "ExecutionConfiguration with ExecutionMode {} requires {required_block}.",
            config.execution_mode
        ));
    }
    if unexpected {
        return Some(format!(
            "ExecutionConfiguration with ExecutionMode {} does not accept {other_block}.",
            config.execution_mode
        ));
    }
    let on_demand = config.on_demand_configuration.as_ref()?;
    let start = epoch_millis(&on_demand.start_time).ok_or_else(|| {
        format!(
            "OnDemandConfiguration.StartTime must be a Unix/epoch time in milliseconds, got {}.",
            on_demand.start_time
        )
    });
    let start = match start {
        Ok(start) => start,
        Err(problem) => return Some(problem),
    };
    let end = on_demand.end_time.as_deref()?;
    match epoch_millis(end) {
        None => Some(format!(
            "OnDemandConfiguration.EndTime must be a Unix/epoch time in milliseconds, got {end}."
        )),
        Some(end_millis) if end_millis < start => Some(format!(
            "OnDemandConfiguration.EndTime {end} must not precede StartTime {}.",
            on_demand.start_time
        )),
        Some(_) => None,
    }
}

/// The class a failed validation of a data integration association is reported under in
/// the message of the refused create.
const ASSOCIATION_VALIDATION_FAILURE_CLASS: &str = "DATA_INTEGRATION_ASSOCIATION_VALIDATION_EXCEPTION";

/// The reason that wrapped validation failure carries. Fixed text: the service reports
/// the validation that declined the association, which is about reaching the data source
/// of its integration and so names no member of the request, and it reports the same
/// reason for every association it declines. It is reached ONLY for a request this
/// service declines ([`execution_configuration_problem`]); an association this service
/// can act on is still created.
const ASSOCIATION_SOURCE_ACCESS_DENIED: &str = "Access Denied to S3 Bucket";

/// How AppIntegrations refuses a `CreateDataIntegrationAssociation` it will not create
/// the association for: the failure is reported AS A FAILURE OF THAT CREATE, naming the
/// data integration the association would belong to, and the validation that declined it
/// is wrapped into the sentence under the class that raised it.
///
/// The only part taken from the request is the data integration it names, resolved to the
/// id this service minted for it; everything else is fixed, because the service reports
/// the validation that declined the association rather than a complaint about a single
/// request member (the member-level detail [`execution_configuration_problem`] computes
/// goes to the operator's log instead). The model states no cross-member rule for an
/// `ExecutionConfiguration`, so this refusal's wording is recorded behaviour.
pub fn data_integration_association_not_created(data_integration_id: &str) -> String {
    format!(
        "Failed to create DataIntegrationAssociation for DataIntegration \
         {data_integration_id}. {ASSOCIATION_VALIDATION_FAILURE_CLASS}: \
         {ASSOCIATION_SOURCE_ACCESS_DENIED}"
    )
}

/// The two values `ExecutionMode` accepts, from the model's enum
/// (`smithy_query(query="shape", shape="ExecutionMode")`).
pub const EXECUTION_MODE_ON_DEMAND: &str = "ON_DEMAND";
pub const EXECUTION_MODE_SCHEDULED: &str = "SCHEDULED";

/// `value` read as the "Unix/epoch string in milliseconds" an `OnDemandConfiguration`
/// time is documented to be, or `None` when it is not one.
fn epoch_millis(value: &str) -> Option<u64> {
    value.parse::<u64>().ok()
}

/// Per-service state for com-amazonaws-appintegrations-amazonappintegrationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppintegrationsAmazonappintegrationserviceState {
    /// Every stored application, keyed by the `UUID` this service minted for it —
    /// the resource's identity in this API, and what both identifier forms resolve
    /// to (see [`Self::application_id`]). A `BTreeMap` so list order, and therefore
    /// pagination, is deterministic. Bounded at [`MAX_STORED_APPLICATIONS`] entries
    /// by the create that writes it ([`Self::has_room_for_another_application`]), so
    /// a caller cannot grow it without end.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// `ClientToken` -> application id, so a retried `CreateApplication` carrying
    /// the token of a completed one answers with the id and ARN it already minted
    /// instead of creating a second application or failing as a duplicate. A
    /// separate map from [`Self::idempotency`] and [`Self::data_integration_idempotency`]
    /// because one token namespace belongs to one resource type. Bounded at
    /// [`MAX_REMEMBERED_APPLICATION_CLIENT_TOKENS`] by
    /// [`Self::remember_application_client_token`].
    pub application_idempotency: BTreeMap<String, String>,
    /// Every stored event integration, keyed by its name — the resource's
    /// identity in this API. A `BTreeMap` so list order, and therefore
    /// pagination, is deterministic. Bounded at [`MAX_STORED_EVENT_INTEGRATIONS`]
    /// entries by the create that writes it
    /// ([`Self::has_room_for_another_event_integration`]), so a caller cannot grow it
    /// without end.
    pub event_integrations: BTreeMap<String, EventIntegrationRecord>,
    /// `ClientToken` -> integration name, so a retried `CreateEventIntegration`
    /// carrying the token of a completed one answers with the ARN it already
    /// minted instead of creating a second resource or failing as a duplicate. One
    /// entry per stored integration, which the delete drops with it, so the ceiling
    /// above bounds this too.
    pub idempotency: BTreeMap<String, String>,
    /// Every stored data integration, keyed by the id the service minted for it —
    /// this resource's identity, which `Get`/`Delete` address it by. A `BTreeMap`
    /// for the same reason as above, and bounded at [`MAX_STORED_DATA_INTEGRATIONS`]
    /// entries by the create that writes it ([`Self::has_room_for_another_data_integration`]),
    /// so a caller cannot grow it without end.
    pub data_integrations: BTreeMap<String, DataIntegrationRecord>,
    /// `ClientToken` -> data integration id, the `data_integrations` counterpart of
    /// [`Self::idempotency`]. A separate map because the two tokens address
    /// different resources: one token namespace per resource, so a token used on a
    /// `CreateEventIntegration` cannot resolve to a data integration. Bounded at
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] by [`Self::remember_client_token`].
    pub data_integration_idempotency: BTreeMap<String, String>,
    /// Every stored data integration association, keyed by the id the service minted for
    /// it — this resource's identity, which `UpdateDataIntegrationAssociation` addresses
    /// it by. One map for every integration's associations rather than one per
    /// integration, so an association is stored exactly once and
    /// [`Self::associations_of`] is the one way to find the ones a given integration has.
    /// A `BTreeMap` so list order, and therefore pagination, is deterministic, and
    /// bounded at [`MAX_STORED_DATA_INTEGRATION_ASSOCIATIONS`] entries by the create that
    /// writes it ([`Self::has_room_for_another_data_integration_association`]).
    /// `DeleteDataIntegration` reads it: AWS deletes only a DataIntegration that has no
    /// DataIntegrationAssociations.
    pub data_integration_associations: BTreeMap<String, DataIntegrationAssociationRecord>,
    /// `ClientToken` -> data integration association id, the association counterpart of
    /// [`Self::data_integration_idempotency`] and a separate map for the same reason:
    /// one token namespace per resource type. Bounded at
    /// [`MAX_REMEMBERED_ASSOCIATION_CLIENT_TOKENS`] by
    /// [`Self::remember_association_client_token`].
    pub data_integration_association_idempotency: BTreeMap<String, String>,
    /// Every stored application association, keyed by its id. One map for every
    /// application's associations rather than one per application, so an association is
    /// stored exactly once and [`Self::application_associations_of`] is the one way to
    /// find the ones a given application has. A `BTreeMap` so list order, and therefore
    /// pagination, is deterministic.
    ///
    /// No operation of this API writes it — see [`ApplicationAssociationRecord`] — so no
    /// request path grows it and it needs no stored-resource ceiling of its own: an
    /// application this service created has no associations until the client side
    /// associates it, and `ListApplicationAssociations` reports whatever is stored here.
    pub application_associations: BTreeMap<String, ApplicationAssociationRecord>,
    /// Every stored event integration association, keyed by its id. One map for every
    /// event integration's associations rather than one per integration, so an
    /// association is stored exactly once and [`Self::event_integration_associations_of`]
    /// is the one way to find the ones a given integration has. A `BTreeMap` so list
    /// order, and therefore pagination, is deterministic.
    ///
    /// No operation of this API writes it — see [`EventIntegrationAssociationRecord`] —
    /// so no request path grows it and it needs no stored-resource ceiling of its own: an
    /// event integration this service created has no associations until the client side
    /// associates it, and `ListEventIntegrationAssociations` reports whatever is stored
    /// here.
    pub event_integration_associations: BTreeMap<String, EventIntegrationAssociationRecord>,
}

impl ComAmazonawsAppintegrationsAmazonappintegrationserviceState {
    /// The id of the application `identifier` names, or `None` when this account holds
    /// none.
    ///
    /// The application operations address the resource by `ArnOrUUID` (model:
    /// `GetApplication.Arn`, pattern `^(arn:aws:…|[a-f0-9]{8}-…)(:[\w\$]+)?$`), so both
    /// forms this service hands out resolve: the `Id` it minted and the `Arn` built
    /// from that id, either of them optionally carrying the trailing `:<qualifier>` the
    /// pattern allows. Nothing else does — an ARN is matched against the stored one in
    /// full, exactly as [`Self::data_integration_id`] matches a data integration's, so
    /// a foreign ARN ending in the same id (another account, Region, vendor or resource
    /// type) names no application here.
    pub fn application_id(&self, identifier: &str) -> Option<String> {
        if let Some(id) = self.exact_application_id(identifier) {
            return Some(id);
        }
        let (base, _qualifier) = identifier.rsplit_once(':')?;
        self.exact_application_id(base)
    }

    /// [`Self::application_id`] for an identifier carrying no qualifier: the minted id
    /// itself, or the whole ARN built from it.
    fn exact_application_id(&self, identifier: &str) -> Option<String> {
        if self.applications.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.applications
            .values()
            .find(|record| record.arn == identifier)
            .map(|record| record.id.clone())
    }

    /// Whether the store can hold another application, which is what
    /// `CreateApplication` checks BEFORE it mints one: past
    /// [`MAX_STORED_APPLICATIONS`] it refuses with the quota error that operation
    /// models, for the same reason as [`Self::has_room_for_another_data_integration`].
    pub fn has_room_for_another_application(&self) -> bool {
        self.applications.len() < MAX_STORED_APPLICATIONS
    }

    /// Remember that the create carrying `token` minted the application `id`, so a
    /// retry under that token answers with the resource it already made.
    ///
    /// Bounded exactly as [`Self::remember_client_token`] is, and for the same reason:
    /// the key is caller-supplied, so the index it is written into cannot be allowed to
    /// grow one request at a time.
    pub fn remember_application_client_token(&mut self, token: String, id: String) {
        if self.application_idempotency.len() >= MAX_REMEMBERED_APPLICATION_CLIENT_TOKENS {
            let Self {
                applications,
                application_idempotency,
                ..
            } = self;
            application_idempotency.retain(|_, minted| applications.contains_key(minted));
        }
        if self.application_idempotency.len() < MAX_REMEMBERED_APPLICATION_CLIENT_TOKENS {
            self.application_idempotency.insert(token, id);
        }
    }

    /// The id of the data integration `identifier` names, or `None` when this account
    /// holds none.
    ///
    /// `Identifier` is documented as "a unique identifier for the DataIntegration"
    /// (`smithy_query(query="doc", shape="GetDataIntegrationRequest",
    /// member="Identifier")`) and is constrained only to a non-blank string, so both
    /// forms the service hands out address the resource: the `Id` it minted and the
    /// `Arn` built from that id. Nothing else resolves — not a name, because two
    /// resources could not then be told apart from one, and not an ARN this service
    /// did not mint, which is matched against the stored one in full so that a
    /// foreign ARN ending in the same id (another account, Region, vendor or resource
    /// type) names no resource here.
    pub fn data_integration_id(&self, identifier: &str) -> Option<String> {
        if self.data_integrations.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.data_integrations
            .values()
            .find(|record| record.arn == identifier)
            .map(|record| record.id.clone())
    }

    /// Whether the store can hold another event integration, which is what
    /// `CreateEventIntegration` checks BEFORE it mints one: past
    /// [`MAX_STORED_EVENT_INTEGRATIONS`] it refuses with the quota error that operation
    /// models, for the same reason as [`Self::has_room_for_another_data_integration`].
    pub fn has_room_for_another_event_integration(&self) -> bool {
        self.event_integrations.len() < MAX_STORED_EVENT_INTEGRATIONS
    }

    /// Whether the store can hold another data integration, which is what
    /// `CreateDataIntegration` checks BEFORE it mints one: past
    /// [`MAX_STORED_DATA_INTEGRATIONS`] it refuses with the quota error the operation
    /// models, so this in-memory store cannot be grown without end one request at a
    /// time. A caller's delete frees a slot again.
    pub fn has_room_for_another_data_integration(&self) -> bool {
        self.data_integrations.len() < MAX_STORED_DATA_INTEGRATIONS
    }

    /// Whether the store can hold another data integration association, which is what
    /// `CreateDataIntegrationAssociation` checks BEFORE it mints one: past
    /// [`MAX_STORED_DATA_INTEGRATION_ASSOCIATIONS`] it refuses with the quota error that
    /// operation models, for the same reason as
    /// [`Self::has_room_for_another_data_integration`].
    pub fn has_room_for_another_data_integration_association(&self) -> bool {
        self.data_integration_associations.len() < MAX_STORED_DATA_INTEGRATION_ASSOCIATIONS
    }

    /// The associations the data integration `integration_id` has, in the deterministic
    /// order of the ids this service minted for them.
    pub fn associations_of<'a>(
        &'a self,
        integration_id: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a DataIntegrationAssociationRecord)> {
        self.data_integration_associations
            .iter()
            .filter(move |(_, record)| record.data_integration_id == integration_id)
    }

    /// The associations the application `application_id` has, in the deterministic order
    /// of the association ids.
    pub fn application_associations_of<'a>(
        &'a self,
        application_id: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ApplicationAssociationRecord)> {
        self.application_associations
            .iter()
            .filter(move |(_, record)| record.application_id == application_id)
    }

    /// The associations the event integration `name` has, in the deterministic order of
    /// the association ids.
    ///
    /// The integration is named, not resolved: an event integration is addressed by its
    /// name throughout this API, so the associations of one are exactly the stored ones
    /// carrying that name.
    pub fn event_integration_associations_of<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a EventIntegrationAssociationRecord)> {
        self.event_integration_associations
            .iter()
            .filter(move |(_, record)| record.event_integration_name == name)
    }

    /// Whether the application `application_id` is associated with any client, which is
    /// what `DeleteApplication` refuses an unforced delete on.
    pub fn has_application_associations(&self, application_id: &str) -> bool {
        self.application_associations_of(application_id).next().is_some()
    }

    /// Drop every association of the application `application_id` and answer how many
    /// were dropped: what a `DeleteApplication` carrying `Force` does with them, since
    /// an association exists only as part of the application it belongs to.
    pub fn remove_application_associations_of(&mut self, application_id: &str) -> usize {
        let before = self.application_associations.len();
        self.application_associations
            .retain(|_, record| record.application_id != application_id);
        before - self.application_associations.len()
    }

    /// Whether the data integration `integration_id` has ever been associated, which is
    /// what `CreateDataIntegrationAssociation` refuses a second association on and
    /// `DeleteDataIntegration` refuses a delete on.
    pub fn has_data_integration_associations(&self, integration_id: &str) -> bool {
        self.associations_of(integration_id).next().is_some()
    }

    /// The id of the association `identifier` names WITHIN the data integration
    /// `integration_id`, or `None` when that integration has none.
    ///
    /// Both forms this service hands out resolve — the minted `Id` and the `Arn` built
    /// from it — and both are matched only against the associations of this integration,
    /// so an association of another integration is not reachable through it.
    pub fn data_integration_association_id(
        &self,
        integration_id: &str,
        identifier: &str,
    ) -> Option<String> {
        self.associations_of(integration_id)
            .find(|(id, record)| id.as_str() == identifier || record.arn == identifier)
            .map(|(id, _)| id.clone())
    }

    /// Remember that the create carrying `token` minted the association `id`, so a retry
    /// under that token answers with the association it already made.
    ///
    /// Bounded exactly as [`Self::remember_client_token`] is, and for the same reason:
    /// the key is caller-supplied, so the index it is written into cannot be allowed to
    /// grow one request at a time.
    pub fn remember_association_client_token(&mut self, token: String, id: String) {
        if self.data_integration_association_idempotency.len()
            >= MAX_REMEMBERED_ASSOCIATION_CLIENT_TOKENS
        {
            let Self {
                data_integration_associations,
                data_integration_association_idempotency,
                ..
            } = self;
            data_integration_association_idempotency
                .retain(|_, minted| data_integration_associations.contains_key(minted));
        }
        if self.data_integration_association_idempotency.len()
            < MAX_REMEMBERED_ASSOCIATION_CLIENT_TOKENS
        {
            self.data_integration_association_idempotency
                .insert(token, id);
        }
    }

    /// Remember that the create carrying `token` minted the data integration `id`, so a
    /// retry under that token answers with the resource it already made.
    ///
    /// Bounded, and it refuses nothing: entries whose integration is no longer stored
    /// are dropped first (they can only resolve to nothing), and past
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] the token is simply not remembered — a
    /// forgotten token makes a replay create a fresh resource, which
    /// [`Self::has_room_for_another_data_integration`] already bounds.
    pub fn remember_client_token(&mut self, token: String, id: String) {
        if self.data_integration_idempotency.len() >= MAX_REMEMBERED_CLIENT_TOKENS {
            let Self {
                data_integrations,
                data_integration_idempotency,
                ..
            } = self;
            data_integration_idempotency
                .retain(|_, minted| data_integrations.contains_key(minted));
        }
        if self.data_integration_idempotency.len() < MAX_REMEMBERED_CLIENT_TOKENS {
            self.data_integration_idempotency.insert(token, id);
        }
    }

    /// Remember that the create carrying `token` minted the event integration `name`,
    /// so a retry under that token answers with the resource it already made.
    ///
    /// Bounded exactly as [`Self::remember_client_token`] is, and for the same reason:
    /// the key is caller-supplied (`ClientToken`, `len 1..2048` — the length the
    /// boundary already enforces), so the index it is written into cannot be allowed to
    /// grow one request at a time. Entries whose integration is no longer stored are
    /// dropped first, and past [`MAX_REMEMBERED_EVENT_CLIENT_TOKENS`] the token is
    /// simply not remembered rather than the create refused.
    pub fn remember_event_client_token(&mut self, token: String, name: String) {
        if self.idempotency.len() >= MAX_REMEMBERED_EVENT_CLIENT_TOKENS {
            let Self {
                event_integrations,
                idempotency,
                ..
            } = self;
            idempotency.retain(|_, minted| event_integrations.contains_key(minted));
        }
        if self.idempotency.len() < MAX_REMEMBERED_EVENT_CLIENT_TOKENS {
            self.idempotency.insert(token, name);
        }
    }

    /// The stored tag set of the resource `arn` names, or `None` when this account holds
    /// no taggable resource under that ARN.
    ///
    /// A tag set is part of the record of the resource it belongs to — an application,
    /// an event integration or a data integration, the three resources whose create
    /// takes `Tags` — so the tagging operations and every `Get`/`List` that reports
    /// `Tags` read the one copy. The ARN is matched against the stored one in FULL,
    /// exactly as [`Self::data_integration_id`] matches a data integration's, so an ARN
    /// of another account, Region, vendor or resource type names no resource here.
    pub fn tags_of(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(record) = self.applications.values().find(|record| record.arn == arn) {
            return Some(&record.tags);
        }
        if let Some(record) = self
            .event_integrations
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(&record.tags);
        }
        self.data_integrations
            .values()
            .find(|record| record.arn == arn)
            .map(|record| &record.tags)
    }

    /// [`Self::tags_of`] for a caller that changes the set: the tag set `TagResource`
    /// merges into and `UntagResource` removes keys from, on the resource's own record,
    /// so a later read of that resource reports the change.
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(record) = self
            .applications
            .values_mut()
            .find(|record| record.arn == arn)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .event_integrations
            .values_mut()
            .find(|record| record.arn == arn)
        {
            return Some(&mut record.tags);
        }
        self.data_integrations
            .values_mut()
            .find(|record| record.arn == arn)
            .map(|record| &mut record.tags)
    }
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an application in `region`:
/// `arn:aws:app-integrations:<region>:<account>:application/<id>`.
pub fn application_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APPLICATION_ARN_SEGMENT}/{id}"),
    )
}

/// The ARN of an event integration in `region`:
/// `arn:aws:app-integrations:<region>:<account>:event-integration/<name>`.
pub fn event_integration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{EVENT_INTEGRATION_ARN_SEGMENT}/{name}"),
    )
}

/// The ARN of a data integration in `region`:
/// `arn:aws:app-integrations:<region>:<account>:data-integration/<id>`.
///
/// Built from the minted `Id`, not from the caller's `Name`: `Id` is what this
/// resource's identity is, and the ARN is the other form of it.
pub fn data_integration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DATA_INTEGRATION_ARN_SEGMENT}/{id}"),
    )
}

/// The ARN of a data integration association in `region`:
/// `arn:aws:app-integrations:<region>:<account>:data-integration-association/<integration-id>/<association-id>`.
///
/// Nested under the integration it belongs to, because an association exists only as
/// part of one (its operations address it by both identifiers), and built from the
/// minted ids rather than from anything the caller sent.
pub fn data_integration_association_arn(
    region: &str,
    integration_id: &str,
    association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DATA_INTEGRATION_ASSOCIATION_ARN_SEGMENT}/{integration_id}/{association_id}"),
    )
}

/// The ARN of an application association in `region`:
/// `arn:aws:app-integrations:<region>:<account>:application-association/<application-id>/<association-id>`.
///
/// Nested under the application it belongs to, for the same reason as
/// [`data_integration_association_arn`]: an association exists only as part of one
/// resource, and the ARN is built from stored ids rather than from anything a caller
/// sent.
pub fn application_association_arn(
    region: &str,
    application_id: &str,
    association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{APPLICATION_ARN_ASSOCIATION_SEGMENT}/{application_id}/{association_id}"),
    )
}

/// The ARN of an event integration association in `region`:
/// `arn:aws:app-integrations:<region>:<account>:event-integration-association/<event-integration-name>/<association-id>`.
///
/// Nested under the event integration it belongs to, for the same reason as
/// [`data_integration_association_arn`]: an association exists only as part of one
/// resource, and an event integration is addressed by its name.
pub fn event_integration_association_arn(
    region: &str,
    event_integration_name: &str,
    association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "{EVENT_INTEGRATION_ASSOCIATION_ARN_SEGMENT}/{event_integration_name}/{association_id}"
        ),
    )
}

/// The pagination cursor for a page that starts at the store key `name` (an event
/// integration's name, a data integration's id).
///
/// Hex of the key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `NextToken` is opaque and
/// only ever travels back unchanged.
pub fn encode_cursor(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or
/// `None` when the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppintegrationsAmazonappintegrationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppintegrationsAmazonappintegrationserviceState>)
}

#[cfg(test)]
mod tests {
    //! The bound a wire test cannot see: an idempotency index never grows past its
    //! ceiling, however many distinct `ClientToken`s a caller cycles through.

    use super::*;

    /// RULE: remembering a token is bounded. Every entry a caller can add is for a
    /// resource it created; once the index is full of entries whose resource is gone,
    /// those are dropped, and a token is never remembered past the ceiling — so a
    /// create/delete loop carrying a fresh token each time cannot grow the index
    /// without end. Both indexes, because both keys are caller-supplied.
    #[test]
    fn an_idempotency_index_never_grows_past_its_ceiling() {
        let cycles = MAX_REMEMBERED_CLIENT_TOKENS + 25;
        let mut state = ComAmazonawsAppintegrationsAmazonappintegrationserviceState::default();
        for cycle in 0..cycles {
            // A create/delete loop: the record is never there by the time the next
            // token arrives, which is the shape that would grow an unbounded index.
            state.remember_client_token(format!("data-token-{cycle}"), format!("id-{cycle}"));
            state.remember_event_client_token(
                format!("event-token-{cycle}"),
                format!("name-{cycle}"),
            );
            assert!(
                state.data_integration_idempotency.len() <= MAX_REMEMBERED_CLIENT_TOKENS,
                "case data cycle {cycle}: expected at most {MAX_REMEMBERED_CLIENT_TOKENS} \
                 remembered tokens, got {}",
                state.data_integration_idempotency.len()
            );
            assert!(
                state.idempotency.len() <= MAX_REMEMBERED_EVENT_CLIENT_TOKENS,
                "case event cycle {cycle}: expected at most \
                 {MAX_REMEMBERED_EVENT_CLIENT_TOKENS} remembered tokens, got {}",
                state.idempotency.len()
            );
        }
    }

    /// RULE: a token the index has room for is remembered, so bounding it does not stop
    /// idempotency working. Rows differ in the token's length, including the modelled
    /// maximum (`ClientToken`, `len 1..2048`).
    #[test]
    fn a_token_the_index_has_room_for_is_remembered() {
        let mut state = ComAmazonawsAppintegrationsAmazonappintegrationserviceState::default();
        for (case, token, id) in [
            ("a short token", "t1".to_string(), "id-1"),
            ("a longer token", "retry-token-0001".to_string(), "id-2"),
            (
                "a token at the modelled maximum length",
                "t".repeat(2048),
                "id-3",
            ),
        ] {
            state.remember_client_token(token.clone(), id.to_string());
            assert_eq!(
                state
                    .data_integration_idempotency
                    .get(&token)
                    .map(String::as_str),
                Some(id),
                "case {case:?}: expected the token to resolve to {id}, got {:?}",
                state.data_integration_idempotency.get(&token)
            );
        }
    }
}
