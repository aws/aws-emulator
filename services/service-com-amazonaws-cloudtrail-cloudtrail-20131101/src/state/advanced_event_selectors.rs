// SPDX-License-Identifier: CC0-1.0

//! The rules an ADVANCED event selector set is held to, and the sentence each refusal
//! carries.
//!
//! `PutEventSelectors`, `CreateEventDataStore` and `UpdateEventDataStore` all store a
//! `AdvancedEventSelectors` list, and all three declare the one error a selector set
//! CloudTrail will not install is refused with — `InvalidEventSelectorsException`
//! (`smithy_query(query="operation",operation="PutEventSelectors")`: client error,
//! HTTP 400). The structure is one shape, so it is read HERE once, the way
//! [`super::channels`] holds the channel identifier reading, rather than once per
//! handler.
//!
//! Five rules hold over a set, each refused with the sentence CloudTrail words that
//! error class in — fixed text whose slots are taken from the request:
//!
//! * every field selector states at least one condition ([`empty_selector_message`]);
//! * every selector states the CATEGORY of events it selects, on
//!   [`super::EVENT_CATEGORY_SELECTOR_FIELD`] ([`missing_fields_message`]) — without
//!   which it selects no category at all;
//! * that category is ONE value ([`too_many_values_message`]);
//! * every other field it selects on is a field that category admits
//!   ([`field_not_allowed_message`]), and a field the model admits only `Equals` for
//!   ([`super::EQUALS_ONLY_SELECTOR_FIELDS`]) carries no other operator;
//! * a `resources.type` condition names a resource type data events are logged for
//!   ([`RESOURCES_TYPE_NOT_VALID_MESSAGE`]).
//!
//! On top of them a set is bounded: "You can add advanced event selectors, and
//! conditions for your advanced event selectors, up to a maximum of 500 values for all
//! conditions and selectors on a trail" (model doc for `PutEventSelectors`), which is
//! [`super::ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT`].
//!
//! Which fields a category admits depends on the RESOURCE the set is stored on
//! ([`SelectorTarget`]): the model's own doc for `AdvancedFieldSelector$Field` refers
//! the reader to the `AdvancedFieldSelector` page of the CloudTrail API Reference for
//! the field catalogue, and that page states the fields each event category supports
//! plus the additional ones available for event data stores. Nothing is written until
//! the whole set has been read, so a refusal leaves the selectors a resource already
//! carried exactly as they were.

use crate::ops::types::{AdvancedEventSelector, AdvancedFieldSelector};

use super::{
    ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT, EQUALS_ONLY_SELECTOR_FIELDS,
    EVENT_CATEGORY_SELECTOR_FIELD,
};

/// The resource an advanced event selector set is being stored on.
///
/// A trail and an event data store admit different fields for the same event category:
/// the API Reference page for `AdvancedFieldSelector` names, per category, the fields it
/// supports and then "the following additional fields are available for event data
/// stores". A selector a trail cannot carry is therefore not a selector a store cannot
/// carry, so the caller says which resource it is storing onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorTarget {
    /// `PutEventSelectors`, which configures a trail.
    Trail,
    /// `CreateEventDataStore` / `UpdateEventDataStore`, which configure a CloudTrail
    /// Lake event data store.
    EventDataStore,
}

/// The `eventCategory` value of CloudTrail MANAGEMENT events — the control-plane calls
/// a trail logs by default.
pub const MANAGEMENT_EVENT_CATEGORY: &str =
    ::service_common::management_events::EVENT_CATEGORY_MANAGEMENT;

/// The `eventCategory` value of CloudTrail DATA events, one of the categories the model
/// doc for `StopEventDataStoreIngestion` names ("the `eventCategory` must be
/// `Management`, `Data`, `NetworkActivity`, or `ConfigurationItem`").
pub const DATA_EVENT_CATEGORY: &str = "Data";

/// The `eventCategory` value of CloudTrail NETWORK ACTIVITY events, named in the same
/// model doc as [`DATA_EVENT_CATEGORY`].
pub const NETWORK_ACTIVITY_EVENT_CATEGORY: &str = "NetworkActivity";

/// The field a data-event selector names the RESOURCE TYPE it selects on, one of the
/// fields the model doc for `AdvancedFieldSelector$Equals` admits only that operator for
/// ([`EQUALS_ONLY_SELECTOR_FIELDS`]).
pub const RESOURCES_TYPE_FIELD: &str = "resources.type";

/// How many values `eventCategory` may carry: a selector selects ONE category of events,
/// which is why the model's doc for `AdvancedFieldSelector$Equals` admits only that
/// operator for the field.
pub const EVENT_CATEGORY_VALUE_LIMIT: usize = 1;

/// The fields a MANAGEMENT-event selector may name on a trail, from the
/// `AdvancedFieldSelector` page of the CloudTrail API Reference the model's own doc for
/// `AdvancedFieldSelector$Field` refers to: for CloudTrail management events the
/// supported fields are `eventCategory`, `eventSource` and `readOnly`.
const MANAGEMENT_TRAIL_FIELDS: &[&str] =
    &[EVENT_CATEGORY_SELECTOR_FIELD, "eventSource", "readOnly"];

/// The fields a DATA-event selector may name on a trail, from the same page: for
/// CloudTrail data events the supported fields are `eventCategory`, `resources.type`,
/// `readOnly`, `eventName` and `resources.ARN`.
const DATA_TRAIL_FIELDS: &[&str] = &[
    EVENT_CATEGORY_SELECTOR_FIELD,
    RESOURCES_TYPE_FIELD,
    "readOnly",
    "eventName",
    "resources.ARN",
];

/// The fields a NETWORK-ACTIVITY selector may name, from the same page: for CloudTrail
/// network activity events the supported fields are `eventCategory`, `eventSource`,
/// `eventName`, `errorCode` and `vpcEndpointId`.
const NETWORK_ACTIVITY_FIELDS: &[&str] = &[
    EVENT_CATEGORY_SELECTOR_FIELD,
    "eventSource",
    "eventName",
    "errorCode",
    "vpcEndpointId",
];

/// The fields an EVENT DATA STORE may name on top of the ones a trail may, from the same
/// page: "the following additional fields are available for event data stores" —
/// `eventName`, `eventType`, `sessionCredentialFromConsole` and `userIdentity.arn` for
/// management events, and `eventSource` beside them for data events.
const EVENT_DATA_STORE_FIELDS: &[&str] = &[
    "eventName",
    "eventSource",
    "eventType",
    "sessionCredentialFromConsole",
    "userIdentity.arn",
];

/// The resource types a `resources.type` condition may name, from the
/// `AdvancedFieldSelector` page of the CloudTrail API Reference the model's own doc for
/// `AdvancedFieldSelector$Field` refers to: the resource types CloudTrail logs data
/// events for. The three a BASIC event selector can also name
/// ([`super::BASIC_DATA_RESOURCE_TYPES`]) are among them — advanced selectors reach every
/// type, basic ones only those three.
pub const ADVANCED_DATA_RESOURCE_TYPES: &[&str] = &[
    "AWS::AppConfig::Configuration",
    "AWS::B2BI::Transformer",
    "AWS::Bedrock::AgentAlias",
    "AWS::Bedrock::FlowAlias",
    "AWS::Bedrock::Guardrail",
    "AWS::Bedrock::KnowledgeBase",
    "AWS::Cassandra::Table",
    "AWS::CloudFront::KeyValueStore",
    "AWS::CloudTrail::Channel",
    "AWS::CloudWatch::Metric",
    "AWS::CodeWhisperer::Customization",
    "AWS::CodeWhisperer::Profile",
    "AWS::Cognito::IdentityPool",
    "AWS::DataExchange::Asset",
    "AWS::Deadline::Fleet",
    "AWS::Deadline::Job",
    "AWS::Deadline::Queue",
    "AWS::Deadline::Worker",
    "AWS::DynamoDB::Stream",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Snapshot",
    "AWS::ElasticMapReduce::Editor",
    "AWS::EMRWAL::Workspace",
    "AWS::FinSpace::Environment",
    "AWS::Glue::Table",
    "AWS::GreengrassV2::ComponentVersion",
    "AWS::GreengrassV2::Deployment",
    "AWS::GuardDuty::Detector",
    "AWS::IoT::Certificate",
    "AWS::IoT::Thing",
    "AWS::IoTJobsDataPlane::JobExecution",
    "AWS::IoTSiteWise::Asset",
    "AWS::IoTSiteWise::TimeSeries",
    "AWS::IoTTwinMaker::Entity",
    "AWS::IoTTwinMaker::Workspace",
    "AWS::IVS::Channel",
    "AWS::IVS::PlaybackRestrictionPolicy",
    "AWS::IVS::Stage",
    "AWS::IVSChat::LoggingConfiguration",
    "AWS::IVSChat::Room",
    "AWS::KendraRanking::ExecutionPlan",
    "AWS::Kinesis::Stream",
    "AWS::Kinesis::StreamConsumer",
    "AWS::KinesisVideo::Stream",
    "AWS::Lambda::Function",
    "AWS::MachineLearning::MlModel",
    "AWS::ManagedBlockchain::Network",
    "AWS::ManagedBlockchain::Node",
    "AWS::MedicalImaging::Datastore",
    "AWS::MediaPackageV2::Channel",
    "AWS::MediaTailor::Channel",
    "AWS::MediaTailor::PlaybackConfiguration",
    "AWS::NeptuneGraph::Graph",
    "AWS::PaymentCryptography::Alias",
    "AWS::PaymentCryptography::Key",
    "AWS::PCAConnectorAD::Connector",
    "AWS::PCAConnectorSCEP::Connector",
    "AWS::QApps::QApp",
    "AWS::QBusiness::Application",
    "AWS::QBusiness::DataSource",
    "AWS::QBusiness::Index",
    "AWS::QBusiness::WebExperience",
    "AWS::RDS::DBCluster",
    "AWS::RUM::AppMonitor",
    "AWS::S3::AccessPoint",
    "AWS::S3::Object",
    "AWS::S3Express::Object",
    "AWS::S3ObjectLambda::AccessPoint",
    "AWS::S3Outposts::Object",
    "AWS::SageMaker::Endpoint",
    "AWS::SageMaker::ExperimentTrialComponent",
    "AWS::SageMaker::FeatureGroup",
    "AWS::SCN::Instance",
    "AWS::ServiceDiscovery::Namespace",
    "AWS::ServiceDiscovery::Service",
    "AWS::SNS::PlatformEndpoint",
    "AWS::SNS::Topic",
    "AWS::SQS::Queue",
    "AWS::SSM::ManagedNode",
    "AWS::SSMMessages::ControlChannel",
    "AWS::StepFunctions::StateMachine",
    "AWS::SWF::Domain",
    "AWS::ThinClient::Device",
    "AWS::ThinClient::Environment",
    "AWS::Timestream::Database",
    "AWS::Timestream::Table",
    "AWS::VerifiedPermissions::PolicyStore",
    "AWS::XRay::Trace",
];

/// How CloudTrail words a member a selector has to carry and does not: the members it is
/// missing, in a bracketed list.
///
/// Fixed text with one slot — the MEMBER that is missing, which is the member's name in
/// the API and not a value the request carried.
pub fn missing_fields_message(members: &str) -> String {
    format!("The following fields are required: [{members}].")
}

/// How CloudTrail words a field selector that states no condition at all: no operator
/// member, or one carrying no value.
///
/// Fixed text with one slot — the `Field` the request named, bare.
pub fn empty_selector_message(field: &str) -> String {
    format!("One or more of the selectors for the {field} field is empty.")
}

/// How CloudTrail words a field the selector's event category does not admit — a field
/// that is not a selector field at all, and a real field that category does not support
/// alike.
///
/// Fixed text with two slots, both copied from the request: the selector's `eventCategory`
/// value and the offending `Field`.
pub fn field_not_allowed_message(category: &str, field: &str) -> String {
    format!(
        "The following field is not allowed when the eventCategory field value equals \
         {category}: {field}."
    )
}

/// How CloudTrail words a field carrying more values than it admits.
///
/// Fixed text with the field and its bound in the slots: the `Field` the request named
/// and the number of values that field can have.
pub fn too_many_values_message(field: &str, limit: usize) -> String {
    format!(
        "There are too many values specified for {field}. The {field} field can have a \
         maximum of {limit} values."
    )
}

/// How CloudTrail words a `resources.type` value outside the set of resource types data
/// events are logged for.
///
/// Entirely fixed text for this error class — it names the field whose value set was
/// broken and nothing of the request.
pub const RESOURCES_TYPE_NOT_VALID_MESSAGE: &str = "The resources.type field value is not valid.";

/// How CloudTrail words a selector set past the documented condition-value bound: the
/// remedy the model's own `InvalidEventSelectorsException` doc states for it ("Specify up
/// to 500 values for all conditions in all advanced event selectors for a trail").
pub fn too_many_condition_values_message() -> String {
    format!(
        "Specify up to {ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT} values for all conditions in \
         all advanced event selectors for a trail"
    )
}

/// Every condition value a field selector states, across all six operators.
fn condition_values(field_selector: &AdvancedFieldSelector) -> usize {
    [
        &field_selector.equals,
        &field_selector.starts_with,
        &field_selector.ends_with,
        &field_selector.not_equals,
        &field_selector.not_starts_with,
        &field_selector.not_ends_with,
    ]
    .iter()
    .map(|condition| condition.as_ref().map_or(0, Vec::len))
    .sum()
}

/// The fields `category` admits on `target`, or `None` for a category whose field
/// catalogue is not documented.
///
/// An event data store also collects categories a trail never logs — Config
/// configuration items, Audit Manager evidence, CloudTrail Insights events, events from
/// outside Amazon Web Services — and for those "the only supported field is
/// `eventCategory`", so the category alone is the catalogue. A category this service has
/// no catalogue for is read as a field catalogue it cannot judge: the fields beside it
/// are then stored as given rather than refused against a set this emulator invented.
fn fields_for_category(target: SelectorTarget, category: &str) -> Option<Vec<&'static str>> {
    let documented = match category {
        MANAGEMENT_EVENT_CATEGORY => MANAGEMENT_TRAIL_FIELDS,
        DATA_EVENT_CATEGORY => DATA_TRAIL_FIELDS,
        NETWORK_ACTIVITY_EVENT_CATEGORY => NETWORK_ACTIVITY_FIELDS,
        _ => return None,
    };
    let mut fields: Vec<&'static str> = documented.to_vec();
    if target == SelectorTarget::EventDataStore && category != NETWORK_ACTIVITY_EVENT_CATEGORY {
        fields.extend(EVENT_DATA_STORE_FIELDS);
    }
    Some(fields)
}

/// The one value a selector's `eventCategory` field selectors state, with how many
/// values they carried in total — `None` when the selector states no `eventCategory` at
/// all.
fn stated_category(selector: &AdvancedEventSelector) -> Option<(String, usize)> {
    let mut values: Vec<&String> = Vec::new();
    for field_selector in &selector.field_selectors {
        if field_selector.field != EVENT_CATEGORY_SELECTOR_FIELD {
            continue;
        }
        for condition in [
            &field_selector.equals,
            &field_selector.starts_with,
            &field_selector.ends_with,
            &field_selector.not_equals,
            &field_selector.not_starts_with,
            &field_selector.not_ends_with,
        ] {
            values.extend(condition.iter().flatten());
        }
    }
    let first = values.first()?;
    Some(((*first).clone(), values.len()))
}

/// Check an advanced selector set against the rules above, answering the refusal text
/// `InvalidEventSelectorsException` carries for the FIRST rule it breaks.
///
/// Nothing here writes: the whole set is read before the caller stores any of it, so a
/// refused request leaves the previously applied set untouched.
pub fn validate(
    selectors: &[AdvancedEventSelector],
    target: SelectorTarget,
) -> Result<(), String> {
    let mut values = 0usize;
    for selector in selectors {
        // A field selector with no operator member, or one whose operator carries no
        // value, states no condition on its field at all.
        for field_selector in &selector.field_selectors {
            let field_values = condition_values(field_selector);
            if field_values == 0 {
                return Err(empty_selector_message(&field_selector.field));
            }
            values += field_values;
        }

        // A selector that never states which category of events it selects selects no
        // event at all, whatever else its field selectors say — so the category is a
        // required member of every advanced event selector.
        let Some((category, category_values)) = stated_category(selector) else {
            return Err(missing_fields_message(EVENT_CATEGORY_SELECTOR_FIELD));
        };
        if category_values > EVENT_CATEGORY_VALUE_LIMIT {
            return Err(too_many_values_message(
                EVENT_CATEGORY_SELECTOR_FIELD,
                EVENT_CATEGORY_VALUE_LIMIT,
            ));
        }

        let admitted = fields_for_category(target, &category);
        for field_selector in &selector.field_selectors {
            let field = field_selector.field.as_str();
            // Which fields are legal depends on the category the selector selects, so a
            // field outside that category's catalogue is refused whether it is a field of
            // another category or no selector field at all.
            if admitted
                .as_ref()
                .is_some_and(|fields| !fields.contains(&field))
            {
                return Err(field_not_allowed_message(&category, field));
            }
            // The model names the fields `Equals` is "the only valid operator" for, so a
            // condition stated under any other operator on one of them is a condition
            // the service cannot evaluate.
            if EQUALS_ONLY_SELECTOR_FIELDS.contains(&field)
                && condition_values(field_selector)
                    != field_selector.equals.as_ref().map_or(0, Vec::len)
            {
                return Err(format!("The only valid operator for the {field} field is Equals"));
            }
            // A resource type data events are not logged for selects nothing, however
            // well-formed it looks.
            if field == RESOURCES_TYPE_FIELD
                && field_selector
                    .equals
                    .iter()
                    .flatten()
                    .any(|value| !ADVANCED_DATA_RESOURCE_TYPES.contains(&value.as_str()))
            {
                return Err(RESOURCES_TYPE_NOT_VALID_MESSAGE.to_string());
            }
        }
    }
    if values > ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT {
        return Err(too_many_condition_values_message());
    }
    Ok(())
}
