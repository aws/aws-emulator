// SPDX-License-Identifier: CC0-1.0

//! The set of values a resource type's REGISTRY SCHEMA states for a property, and the
//! sentence the schema validator reports a value outside it with.
//!
//! A resource type whose schema the registry holds is validated AGAINST that schema before
//! its handler is invoked, and a property the schema declares an `enum` for admits those
//! values and no other — so a declaration carrying one the schema does not state is one no
//! handler of the type could ever be given. The pre-deployment stage reports it, quoting the
//! rejected value and the values the property admits at that property's own path, and the
//! operation deploys nothing.
//!
//! That sentence is the SCHEMA VALIDATOR's ([`enum_failure_reason`]), which is what a client
//! reads for a type validated that way — not the `Property validation failure: […]` form
//! [`super::failure_reason`] words for a property whose admitted set this emulator knows from
//! the type's published property reference alone. That is why this is a module of its own
//! rather than another column on [`super::CONSTRAINED_PROPERTIES`]: the two catalogues answer
//! the same QUESTION from different sources — a type's registry schema here, its published
//! property reference there — and a source decides both the set and the sentence a value
//! outside it is refused with, which is the same split the sibling questions of that stage
//! are already in ([`super::declared_properties`], [`super::patterned_properties`]).
//!
//! A LIST-valued property is judged ELEMENT BY ELEMENT, because what the schema states the
//! `enum` on is the list's items: every element is held to the set, the first element outside
//! it is what the stage reports, and the path it reports names THAT element — the element's
//! INDEX under the property it sits in, which is the form a recorded refusal of a list-valued
//! property carries ([`element_path`]).
//!
//! Where the schema states the `enum` on a member NESTED inside a structured property, that
//! member is what is judged: a schema's `enum` belongs to the property it is declared on
//! wherever that property sits in the declaration, so the stage steps down to it through the
//! members above it and reports the refusal at that member's own place — which is why a row
//! names the CHAIN of members from the resource's property document down to the judged value
//! ([`EnumProperty::property`]) rather than one property name.
//!
//! The catalogue is the same kind as the rest of that stage's: one row per (resource type,
//! property) whose schema-stated value set this emulator knows, so a property no row covers
//! is left to the service that owns the resource rather than guessed at — and a value that is
//! not text (an expression this service left unevaluated, a structure the author wrote where
//! the schema states a string) is admitted here, because it is no value of the set either way.

use ::serde_json::Value;

use super::{member_path, PropertyFailure, StoredResource};

/// One property whose admitted values its type's registry schema states as an `enum`.
#[derive(Debug, Clone, Copy)]
pub struct EnumProperty {
    /// The resource type the property belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The chain of members leading from the resource's property document down to the
    /// property the schema states the `enum` on, each spelled as a template spells it: one
    /// name for a property of the resource itself, and the members stepped through for one
    /// nested inside a structured property.
    pub property: &'static [&'static str],
    /// The values the schema states, in the order it declares them — which is also the order
    /// a refusal lists them in, since what the author has to act on is the set as the schema
    /// publishes it.
    pub values: &'static [&'static str],
}

/// The `enum` the registry schema of `AWS::LakeFormation::PrincipalPermissions` states for
/// the items of its two permission lists, in the order that schema states them.
///
/// These are the values of Lake Formation's own `Permission`
/// (`smithy_query(query="shape", shape="Permission")` of the `lakeformation` model: "Lake
/// Formation defines privileges to grant and revoke access to metadata in the Data Catalog
/// and data organized in underlying data storage"), in that enum's declaration order, AS THE
/// TYPE'S SCHEMA CARRIES THEM. The model has since added three more —
/// `CREATE_LF_TAG_EXPRESSION`, `CREATE_CATALOG` and `SUPER_USER` — which this type's schema
/// does not state, so a declaration naming one of those is refused by the schema exactly as
/// any other value outside it is, and the set a refusal lists back is this one. What a
/// client may grant through Lake Formation's own `GrantPermissions` is the API's whole enum;
/// what a TEMPLATE of this type may declare is what its schema states.
pub const LAKEFORMATION_PERMISSIONS_VALUES: &[&str] = &[
    "ALL",
    "SELECT",
    "ALTER",
    "DROP",
    "DELETE",
    "INSERT",
    "DESCRIBE",
    "CREATE_DATABASE",
    "CREATE_TABLE",
    "DATA_LOCATION_ACCESS",
    "CREATE_LF_TAG",
    "ASSOCIATE",
    "GRANT_WITH_LF_TAG_EXPRESSION",
];

/// The `enum` the registry schema of `AWS::DirectConnect::ResiliencyGroup` states for the
/// resiliency model the group is created to meet.
///
/// These are the values of AWS Direct Connect's own `ResiliencyModel`
/// (`smithy_query(query="shape", shape="ResiliencyModel")` of the
/// `com-amazonaws-directconnect-overtureservice` model), in that enum's declaration order,
/// which is also what `CreateResiliencyGroup$intendedResiliencyModel` documents: "The valid
/// values are maximum-resiliency, high-resiliency, and basic-resiliency." The order is the
/// one a refusal lists them back in, since what the author has to act on is the set as the
/// schema publishes it.
pub const RESILIENCY_MODEL_VALUES: &[&str] = &[
    "maximum-resiliency",
    "high-resiliency",
    "basic-resiliency",
];

/// The `enum` the registry schema of `AWS::SageMaker::Domain` states for the mode the
/// domain's members authenticate with, in the order that schema states them.
///
/// These are the values of SageMaker's own `AuthMode` (`smithy_query(query="shape",
/// shape="AuthMode")` of the `sagemaker` model), the enum `CreateDomain$AuthMode` targets:
/// "The mode of authentication that members use to access the domain" is either IAM Identity
/// Center or IAM itself. The order is the one a refusal lists them back in, since what the
/// author has to act on is the set as the schema publishes it.
pub const SAGEMAKER_AUTH_MODE_VALUES: &[&str] = &["SSO", "IAM"];

/// The `enum` the registry schema of `AWS::HealthLake::FHIRDatastore` states for the FHIR
/// release version the data store hosts.
///
/// These are the values of AWS HealthLake's own `FHIRVersion`
/// (`smithy_query(query="shape", shape="FHIRVersion")` of the
/// `com-amazonaws-healthlake-healthlake` model: "The FHIR version supported by the data
/// store"), in that enum's declaration order, which is also what
/// `CreateFHIRDatastore$DatastoreTypeVersion` documents: "Current support is for version
/// R4." The order is the one a refusal lists them back in.
pub const FHIR_VERSION_VALUES: &[&str] = &["R4"];

/// The `enum` the registry schema of `AWS::HealthLake::DataTransformationProfile` states for
/// the source data format the profile converts from.
///
/// These are the values of that service's own `SourceFormat` (`smithy_query(query="shape",
/// shape="SourceFormat")` of the same model), in that enum's declaration order, which
/// `CreateDataTransformationProfile$SourceFormat` documents as "Consolidated Clinical
/// Document Architecture (C-CDA) or Comma-separated values (CSV)".
pub const SOURCE_FORMAT_VALUES: &[&str] = &["CCDA", "CSV"];

/// The properties this stage knows a schema-stated value set of.
///
/// Source for each row: the `enum` the resource type's REGISTRY SCHEMA states for the
/// property. That is the set the schema validation holds a declaration to — a value outside
/// it never reaches the owning service at all — and it is not always the whole of the enum
/// that service's own API publishes today ([`LAKEFORMATION_PERMISSIONS_VALUES`] says where
/// the two differ for the rows below).
///
/// The first two rows are the permission lists an
/// `AWS::LakeFormation::PrincipalPermissions` grants with: `Permissions`, "the permissions
/// granted … on the resource", and `PermissionsWithGrantOption`, "indicates a list of the
/// granted permissions that the principal may pass to other users" — each a `PermissionList`
/// of Lake Formation's own `Permission` enum
/// ([`LAKEFORMATION_PERMISSIONS_VALUES`]). The third is the `IntendedResiliencyModel` an
/// `AWS::DirectConnect::ResiliencyGroup` is created to meet ([`RESILIENCY_MODEL_VALUES`]).
/// The fourth is the `MergeType` of an `AWS::AppSync::SourceApiAssociation`'s merge
/// configuration ([`APPSYNC_MERGE_TYPE_VALUES`]), a member nested one level down in that
/// property's structure. The fifth is the `AuthMode` an `AWS::SageMaker::Domain`'s members
/// authenticate with ([`SAGEMAKER_AUTH_MODE_VALUES`]). The next two are the AWS HealthLake
/// pair: the `DatastoreTypeVersion` of a FHIR data store ([`FHIR_VERSION_VALUES`]) and the
/// `SourceFormat` of a data transformation profile ([`SOURCE_FORMAT_VALUES`]). The last two are
/// the Amazon API Gateway pair: the `KeyType` of an association
/// ([`APIGATEWAY_USAGE_PLAN_KEY_TYPE_VALUES`]) and the `Period` of a usage plan's quota
/// ([`APIGATEWAY_QUOTA_PERIOD_VALUES`]), the second of them a member nested one level down in
/// that structured property.
pub const ENUM_PROPERTIES: &[EnumProperty] = &[
    EnumProperty {
        resource_type:
            crate::state::provisioner::lake_formation_permissions::LAKEFORMATION_PRINCIPAL_PERMISSIONS,
        property: &["Permissions"],
        values: LAKEFORMATION_PERMISSIONS_VALUES,
    },
    EnumProperty {
        resource_type:
            crate::state::provisioner::lake_formation_permissions::LAKEFORMATION_PRINCIPAL_PERMISSIONS,
        property: &["PermissionsWithGrantOption"],
        values: LAKEFORMATION_PERMISSIONS_VALUES,
    },
    // The resiliency model an `AWS::DirectConnect::ResiliencyGroup` is created to meet: a
    // value outside that enum is one no handler of the type could be given, so the schema
    // validation refuses it — and the RESOURCE fails on it rather than the template.
    EnumProperty {
        resource_type: crate::state::template::DIRECT_CONNECT_RESILIENCY_GROUP,
        property: &["IntendedResiliencyModel"],
        values: RESILIENCY_MODEL_VALUES,
    },
    // `AWS::AppSync::SourceApiAssociation`'s merge configuration carries which merging option
    // the link is made under, and the schema states that member's `enum`: the value set is
    // AppSync's own `MergeType` ([`APPSYNC_MERGE_TYPE_VALUES`]), and the member sits one level
    // down, inside the `SourceApiAssociationConfig` structure.
    EnumProperty {
        resource_type: crate::state::template::APPSYNC_SOURCE_API_ASSOCIATION,
        property: &["SourceApiAssociationConfig", "MergeType"],
        values: APPSYNC_MERGE_TYPE_VALUES,
    },
    // The mode an `AWS::SageMaker::Domain`'s members authenticate with: a value outside that
    // enum is one no handler of the type could be given, so the schema validation refuses it
    // before the domain is created anywhere — which is what rolls the stack back rather than
    // leaving a domain the owning service would have refused.
    EnumProperty {
        resource_type: crate::state::provisioning::sagemaker_studio::SAGEMAKER_DOMAIN,
        property: &["AuthMode"],
        values: SAGEMAKER_AUTH_MODE_VALUES,
    },
    // The FHIR release version an `AWS::HealthLake::FHIRDatastore` hosts, and the source data
    // format an `AWS::HealthLake::DataTransformationProfile` converts from: each is a value
    // no handler of its type could be given outside the set its schema states, so the schema
    // validation refuses it — and the RESOURCE fails on it rather than the template.
    EnumProperty {
        resource_type: crate::state::template::HEALTHLAKE_FHIR_DATASTORE,
        property: &["DatastoreTypeVersion"],
        values: FHIR_VERSION_VALUES,
    },
    EnumProperty {
        resource_type: crate::state::template::HEALTHLAKE_DATA_TRANSFORMATION_PROFILE,
        property: &["SourceFormat"],
        values: SOURCE_FORMAT_VALUES,
    },
    // The kind of key an `AWS::ApiGateway::UsagePlanKey` puts in the plan, and the time period
    // an `AWS::ApiGateway::UsagePlan`'s quota applies over: each is a value no handler of its
    // type could be given outside the set its schema states, so the schema validation refuses
    // it before anything is asked of API Gateway — which is what rolls the stack back rather
    // than leaving an association or a plan that service would have refused.
    EnumProperty {
        resource_type: crate::state::template::APIGATEWAY_USAGE_PLAN_KEY,
        property: &["KeyType"],
        values: APIGATEWAY_USAGE_PLAN_KEY_TYPE_VALUES,
    },
    EnumProperty {
        resource_type: crate::state::template::APIGATEWAY_USAGE_PLAN,
        property: &["Quota", "Period"],
        values: APIGATEWAY_QUOTA_PERIOD_VALUES,
    },
];

/// The `enum` the registry schema of `AWS::ApiGateway::UsagePlanKey` states for the kind of key
/// an association puts in a plan.
///
/// An API key is the only kind of key a usage plan meters: `CreateUsagePlanKey$keyType` is "the
/// type of a UsagePlanKey resource for a plan customer", and that type's entry in the "AWS
/// resource and property types reference" publishes `API_KEY` as the one allowed value of
/// `KeyType`, which is the `enum` its schema states.
pub const APIGATEWAY_USAGE_PLAN_KEY_TYPE_VALUES: &[&str] = &["API_KEY"];

/// The `enum` the registry schema of `AWS::ApiGateway::UsagePlan` states for the time period its
/// quota applies over, in the order that schema states them.
///
/// These are the values of API Gateway's own `QuotaPeriodType`
/// (`smithy_query(query="shape", shape="QuotaPeriodType")`), which is what
/// `QuotaSettings$period` documents: "the time period in which the limit applies. Valid values
/// are "DAY", "WEEK" or "MONTH"" — the order a refusal lists the set back in.
pub const APIGATEWAY_QUOTA_PERIOD_VALUES: &[&str] = &["DAY", "WEEK", "MONTH"];

/// The `enum` the registry schema of `AWS::AppSync::SourceApiAssociation` states for the
/// `MergeType` of its merge configuration, in the order that schema states them.
///
/// These are the values of AppSync's own `MergeType` (`smithy_query(query="shape",
/// shape="MergeType")` of the `appsync` model), the enum `SourceApiAssociationConfig$mergeType`
/// targets: "valid merge types are MANUAL_MERGE (default) and AUTO_MERGE". The type's schema
/// states them in the order below, which is the order a refusal lists the set back in.
pub const APPSYNC_MERGE_TYPE_VALUES: &[&str] = &["AUTO_MERGE", "MANUAL_MERGE"];

/// `Some(failure)` for a resource carrying a value outside the set its type's schema states
/// for a property ([`ENUM_PROPERTIES`]) — the first such property in the order the catalogue
/// states them, and within a list-valued one the first element outside the set; `None` for a
/// type with no row, for a property the declaration does not carry, and for every value the
/// schema admits.
///
/// A row's chain of members is walked by the ONE walker of this crate's state that steps a
/// declaration's property document down a nested member path
/// ([`crate::state::template::property_at`]), so a chain that runs into a value that is no
/// object judges nothing — the same reading an absent member gets, since neither is a value of
/// the set.
pub(super) fn value_outside_its_enum(resource: &StoredResource) -> Option<PropertyFailure> {
    ENUM_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .find_map(|row| {
            let declared =
                crate::state::template::property_at(&resource.properties, row.property);
            let (at, rejected) = judged_values(declared)
                .into_iter()
                .find(|(_, value)| !row.values.contains(value))?;
            Some(PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: enum_failure_reason(
                    super::reported_value(&resource.resource_type, rejected),
                    row.values,
                ),
                path: element_path(&resource.logical_id, row.property, at),
            })
        })
}

/// The path the judged value sits at: the property itself where the declaration states a
/// single value, and the ELEMENT of it where the declaration states a list.
///
/// The pointer of the member chain is the ONE spelling of that rule
/// ([`super::member_path`]); what this adds is a list-valued property's INDEX, because what
/// was judged there is one element of it and that element is what the author has to act on
/// rather than the list.
fn element_path(logical_id: &str, property: &[&str], at: Option<usize>) -> String {
    let path = member_path(logical_id, property);
    match at {
        Some(at) => format!("{path}/{at}"),
        None => path,
    }
}

/// The values of one declared property this stage judges, each with its place in the list
/// where the declaration states one: the value itself where the declaration states a single
/// value, and every element where it states a LIST of them.
///
/// Only TEXT is judged, for the reason the module docs give — a value that is not text is no
/// value of the set either way — so an element that is not text is passed over while every
/// element keeps its own place, and the index a failure reports is the element's own.
fn judged_values(declared: Option<&Value>) -> Vec<(Option<usize>, &str)> {
    match declared {
        Some(Value::String(value)) => vec![(None, value.as_str())],
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .filter_map(|(at, value)| value.as_str().map(|value| (Some(at), value)))
            .collect(),
        _ => Vec::new(),
    }
}

/// How the schema validator reports a value outside the set its property's schema states:
/// fixed text around the value that was judged and the values the property admits.
///
/// Both variable parts are what was judged — the set is the one its property is held to
/// ([`ENUM_PROPERTIES`]) and the value the declaration's own, as far as a failure of this stage
/// may say it ([`super::reported_value`]) — and the value is named, as it is for a value
/// outside its pattern, because a rejected value is only actionable beside the set it had to be
/// in.
pub fn enum_failure_reason(value: &str, values: &[&str]) -> String {
    format!(
        "{value} is not a valid enum value. Supported values: [{}]",
        values.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde_json::json;
    use ::std::collections::BTreeMap;

    /// One declared resource of `resource_type` carrying `properties`.
    fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: format!("{logical_id}-physical"),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// The type whose rows the table below is about.
    const PRINCIPAL_PERMISSIONS: &str =
        crate::state::provisioner::lake_formation_permissions::LAKEFORMATION_PRINCIPAL_PERMISSIONS;

    /// The type whose nested row the table below is also about.
    const SOURCE_API_ASSOCIATION: &str = crate::state::template::APPSYNC_SOURCE_API_ASSOCIATION;

    /// One row of the table: the case, the type declared, its property document, and the
    /// `(member chain, index, value)` the stage must report — the index being `None` for a
    /// property stating a single value — or `None` where every judged value is one the schema
    /// admits.
    type EnumRow<'a> = (
        &'a str,
        &'a str,
        Value,
        Option<(&'a [&'a str], Option<usize>, &'a str)>,
    );

    /// RULE: a value outside the set its type's schema states for the property fails the
    /// validation, quoting that value and that set at the path of the member that carries it,
    /// and a value the set admits does not — in a list-valued property, element by element,
    /// and for a member nested inside a structured property, at that member's own path.
    ///
    /// The rows span what the rule generalises over: which property carries the value, how
    /// deep in the declaration it sits, where in the list it sits, how many values the list
    /// carries, values the set admits (including ones the report that led here did not cite),
    /// the forms a value can take, and a type the catalogue holds no row for.
    #[test]
    fn a_value_outside_its_schemas_enum_fails_the_validation() {
        let rows: &[EnumRow] = &[
            (
                "the only permission granted is not one of them",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["NOT_A_PERMISSION"]}),
                Some((&["Permissions"], Some(0), "NOT_A_PERMISSION")),
            ),
            (
                "the second of three is not one of them",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["SELECT", "WRITE", "DESCRIBE"]}),
                Some((&["Permissions"], Some(1), "WRITE")),
            ),
            (
                "a permission that differs from one of them only in case",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["describe"]}),
                Some((&["Permissions"], Some(0), "describe")),
            ),
            (
                "the grant-option list carries one that is not",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["SELECT"],
                       "PermissionsWithGrantOption": ["SELECT", "GRANT"]}),
                Some((&["PermissionsWithGrantOption"], Some(1), "GRANT")),
            ),
            (
                "every value the set admits, in both lists",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": LAKEFORMATION_PERMISSIONS_VALUES,
                       "PermissionsWithGrantOption": LAKEFORMATION_PERMISSIONS_VALUES}),
                None,
            ),
            (
                "a value the set admits that the report leading here did not cite",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["DATA_LOCATION_ACCESS", "GRANT_WITH_LF_TAG_EXPRESSION"]}),
                None,
            ),
            (
                // A value the OWNING API's enum carries and this type's schema does not: the
                // schema is what a declaration is held to, so it is refused like any other
                // value outside the set.
                "a permission the API has added since the type's schema was published",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": ["SELECT", "SUPER_USER"]}),
                Some((&["Permissions"], Some(1), "SUPER_USER")),
            ),
            (
                "an empty list, which carries no value to judge",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": []}),
                None,
            ),
            (
                "a list carrying an element that is not text",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": [{"Fn::ImportValue": "Granted"}]}),
                None,
            ),
            (
                "a list whose first element is not text and whose second is rejected",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": [{"Fn::ImportValue": "Granted"}, "NOT_A_PERMISSION"]}),
                Some((&["Permissions"], Some(1), "NOT_A_PERMISSION")),
            ),
            (
                "a property reading a value that has not landed yet",
                PRINCIPAL_PERMISSIONS,
                json!({"Permissions": Value::Null}),
                None,
            ),
            (
                "a type this stage knows no value set of",
                crate::state::provisioner::lake_formation_permissions::LAKEFORMATION_PERMISSIONS,
                json!({"Permissions": ["NOT_A_PERMISSION"]}),
                None,
            ),
            (
                "a nested member carrying a value the schema does not state",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"MergeType": "NOT_A_MERGE_TYPE"}}),
                Some((
                    &["SourceApiAssociationConfig", "MergeType"],
                    None,
                    "NOT_A_MERGE_TYPE",
                )),
            ),
            (
                // Each value the set admits, including the one the report leading here did
                // not cite.
                "a nested member carrying the default the schema states",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"MergeType": "MANUAL_MERGE"}}),
                None,
            ),
            (
                "a nested member carrying the other value the schema states",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"MergeType": "AUTO_MERGE"}}),
                None,
            ),
            (
                "a nested member spelled in the owning API's own casing",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"MergeType": "auto_merge"}}),
                Some((
                    &["SourceApiAssociationConfig", "MergeType"],
                    None,
                    "auto_merge",
                )),
            ),
            (
                "a declaration stating the structure but not the nested member",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {}}),
                None,
            ),
            (
                "a declaration stating none of the structure the member sits in",
                SOURCE_API_ASSOCIATION,
                json!({"Description": "no merge configuration at all"}),
                None,
            ),
            (
                "a structure the declaration states as an unevaluated expression",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"Fn::ImportValue": "Config"}}),
                None,
            ),
            (
                "a nested member whose value is not text",
                SOURCE_API_ASSOCIATION,
                json!({"SourceApiAssociationConfig": {"MergeType": {"Ref": "Merge"}}}),
                None,
            ),
        ];
        for (case, resource_type, properties, reported) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = super::super::failure(&resources);
            let expected = reported.map(|(property, at, value)| {
                let values = ENUM_PROPERTIES
                    .iter()
                    .find(|row| row.resource_type == *resource_type && row.property == property)
                    .map(|row| row.values)
                    .unwrap_or_default();
                let at = at.map_or(String::new(), |at| format!("/{at}"));
                (
                    enum_failure_reason(value, values),
                    format!("/Resources/R/Properties/{}{at}", property.join("/")),
                )
            });
            assert_eq!(
                failed
                    .as_ref()
                    .map(|failure| (failure.reason.clone(), failure.path.clone())),
                expected,
                "case {case:?}: expected the failure {expected:?}, got {failed:?}"
            );
        }
    }
}
