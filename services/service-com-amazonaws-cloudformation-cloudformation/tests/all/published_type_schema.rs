// SPDX-License-Identifier: CC0-1.0

//! The `Schema` a `DescribeType` read of an Amazon-Web-Services-published resource type
//! reports: the document that defines which properties the type has.
//!
//! The rule every case here holds the service to: **`DescribeType` reports `Schema` — "the
//! schema that defines the extension" — for every extension version the registry holds a
//! schema for, and the registry holds, for each public resource type Amazon Web Services
//! publishes into it, a JSON resource schema naming that type and defining exactly the
//! properties the type publishes.** A client reads the registry to learn which properties
//! a resource type accepts, so a record answered without its schema cannot be told apart
//! from a type that is not registered at all.
//!
//! The table below is one row per shape a published type's property set can take — one
//! property, several, and a property that is a path through another (`Endpoint.Address`) —
//! read by `Type`/`TypeName` and by the public ARN, and then the WHOLE catalogue of
//! published types, so no entry of it is exempt. The recorded trace this suite was written
//! for is one row: `AWS::S3::Bucket`.

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    public_type_arn, registry_type,
};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_schema::resource_type_schema;
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    published_attribute_names, IMPLEMENTED_RESOURCE_TYPES,
};
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call, member, router, xml_text, REGION};

async fn describe_type(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "DescribeType", &members).await
}

/// The `<Schema>` of a successful read, parsed.
async fn schema_of(router: &MultiServiceRouter, case: &str, members: &[(&str, &str)]) -> Value {
    let (status, body) = describe_type(router, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    let schema = xml_text(&body, "Schema")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <Schema>, got {body:?}"));
    assert!(
        !schema.is_empty(),
        "case {case:?}: expected a non-empty schema, got {schema:?}"
    );
    serde_json::from_str(&schema).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the schema to be JSON, got {schema:?} ({error})")
    })
}

/// One row of the table: a published resource type, and the properties its schema must
/// define — the type's own published `Fn::GetAtt` return values, as JSON pointers into the
/// document.
///
/// Source for each property set: the type's "Return values" section, which is what
/// `state::template::published_attribute_names` carries; the pointers are spelled out here
/// rather than derived, so a row is a statement about the type and not a restatement of
/// the code under test.
struct SchemaCase {
    type_name: &'static str,
    properties: Vec<&'static str>,
}

fn table() -> Vec<SchemaCase> {
    vec![
        // The recorded row: the five DNS/identity properties an S3 bucket publishes.
        SchemaCase {
            type_name: "AWS::S3::Bucket",
            properties: vec![
                "/properties/Arn",
                "/properties/DomainName",
                "/properties/DualStackDomainName",
                "/properties/RegionalDomainName",
                "/properties/WebsiteURL",
            ],
        },
        // A type whose two published properties are both SERVICE-ASSIGNED: a CloudFront
        // distribution's id and the `cloudfront.net` domain name it is served on.
        SchemaCase {
            type_name: "AWS::CloudFront::Distribution",
            properties: vec!["/properties/DomainName", "/properties/Id"],
        },
        // An API Gateway REST API publishes its own id and the id of the root resource
        // created with it.
        SchemaCase {
            type_name: "AWS::ApiGateway::RestApi",
            properties: vec!["/properties/RestApiId", "/properties/RootResourceId"],
        },
        // Two published properties, the second of which a table has only where its own
        // declaration enables a stream ("You must specify the StreamSpecification property
        // to use this attribute") — the type publishes both names either way.
        SchemaCase {
            type_name: "AWS::DynamoDB::Table",
            properties: vec!["/properties/Arn", "/properties/StreamArn"],
        },
        // Three, one of which is the queue's URL rather than an ARN or a name.
        SchemaCase {
            type_name: "AWS::SQS::Queue",
            properties: vec![
                "/properties/Arn",
                "/properties/QueueName",
                "/properties/QueueUrl",
            ],
        },
        // Published names that are PATHS through a property: an endpoint's address and
        // port are members of the resource's `Endpoint`, so the document nests them.
        SchemaCase {
            type_name: "AWS::RDS::DBInstance",
            properties: vec![
                "/properties/DBInstanceArn",
                "/properties/DbiResourceId",
                "/properties/Endpoint/properties/Address",
                "/properties/Endpoint/properties/Port",
            ],
        },
        // The same shape with a second nested property beside the first.
        SchemaCase {
            type_name: "AWS::RDS::DBCluster",
            properties: vec![
                "/properties/DBClusterArn",
                "/properties/DBClusterResourceId",
                "/properties/Endpoint/properties/Address",
                "/properties/Endpoint/properties/Port",
                "/properties/ReadEndpoint/properties/Address",
            ],
        },
        // A type whose published names are an identifier and an ARN of the service's own
        // spelling, neither of them the conventional `Arn`.
        SchemaCase {
            type_name: "AWS::BedrockAgentCore::Memory",
            properties: vec!["/properties/MemoryArn", "/properties/MemoryId"],
        },
    ]
}

/// Every row's schema names its own type and defines the properties that type publishes.
#[tokio::test]
async fn a_published_resource_types_schema_defines_the_properties_it_publishes() {
    let router = router();

    for SchemaCase {
        type_name,
        properties,
    } in table()
    {
        let document = schema_of(
            &router,
            type_name,
            &[("Type", registry_type::RESOURCE), ("TypeName", type_name)],
        )
        .await;
        let named = document.get("typeName").and_then(Value::as_str);
        assert_eq!(
            named,
            Some(type_name),
            "case {type_name:?}: expected the schema to name its own type, got {named:?} \
             in {document:?}"
        );
        for pointer in &properties {
            let defined = document.pointer(pointer).and_then(|property| {
                property.get("type").and_then(Value::as_str)
            });
            assert_eq!(
                defined,
                Some("string"),
                "case {type_name:?}: expected {pointer:?} defined as a string, got \
                 {defined:?} in {document:?}"
            );
        }
        // And nothing beyond them: the schema defines what the type publishes, not a
        // property nobody knows the type has. The row's own pointers say how many
        // top-level properties that is — a nested pointer names its parent.
        let defined = document
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.len())
            .unwrap_or_default();
        let expected = properties
            .iter()
            .filter_map(|pointer| pointer.split('/').nth(2))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        assert_eq!(
            defined, expected,
            "case {type_name:?}: expected {expected} top-level properties, got {defined} \
             in {document:?}"
        );
    }
}

/// EVERY published resource type in the registry carries a schema, whichever way the
/// request identifies it — by `Type` and `TypeName`, or by the public extension ARN — and
/// the two reads report the SAME document, because a schema is a property of the type
/// rather than of how it was addressed.
#[tokio::test]
async fn every_published_resource_type_reports_the_same_schema_by_name_and_by_arn() {
    let router = router();

    for type_name in IMPLEMENTED_RESOURCE_TYPES {
        let by_name = schema_of(
            &router,
            type_name,
            &[("Type", registry_type::RESOURCE), ("TypeName", type_name)],
        )
        .await;
        let arn = public_type_arn(REGION, registry_type::RESOURCE, type_name);
        let by_arn = schema_of(&router, type_name, &[("Arn", &arn)]).await;
        assert_eq!(
            by_name, by_arn,
            "case {type_name:?}: expected one schema however the extension is \
             identified, got {by_name:?} by name and {by_arn:?} by ARN"
        );
        // Every property the type publishes is defined, under the path its published name
        // spells: a dotted name is a member of the property it passes through.
        for published in published_attribute_names(type_name) {
            let pointer = format!(
                "/properties/{}",
                published.split('.').collect::<Vec<_>>().join("/properties/")
            );
            let defined = by_name.pointer(&pointer);
            assert!(
                defined.is_some(),
                "case {type_name:?}: expected the published {published:?} to be defined \
                 at {pointer:?}, got {by_name:?}"
            );
        }
        // The document declares itself against the resource provider meta-schema and says
        // where the type's provider is published, which is the record's own `SourceUrl`.
        for member in ["$schema", "sourceUrl"] {
            let actual = by_name.get(member).and_then(Value::as_str);
            assert!(
                actual.is_some_and(|value| value.starts_with("https://")),
                "case {type_name:?}: expected a {member:?} URL, got {actual:?} in \
                 {by_name:?}"
            );
        }
        // And it does NOT claim to be closed: which properties an AWS-published type
        // accepts beyond the ones it publishes is not known here, so the document asserts
        // nothing about them.
        let closed = by_name.get("additionalProperties");
        assert!(
            closed.is_none(),
            "case {type_name:?}: expected no additionalProperties claim, got {closed:?}"
        );
    }
}

/// A read is a read: the schema a second `DescribeType` reports is the one the first did,
/// byte for byte, and the registry holds no record — so no schema — for a resource type
/// this emulator does not carry.
#[tokio::test]
async fn a_schema_is_stable_across_reads_and_absent_for_an_uncarried_type() {
    let router = router();
    let type_name = "AWS::S3::Bucket";
    let members = [("Type", registry_type::RESOURCE), ("TypeName", type_name)];

    let (_, first) = describe_type(&router, &members).await;
    let (_, second) = describe_type(&router, &members).await;
    let (first, second) = (xml_text(&first, "Schema"), xml_text(&second, "Schema"));
    assert_eq!(
        first, second,
        "case \"two reads of one type\": expected the same schema, got {first:?} then \
         {second:?}"
    );

    // A name outside the catalogue is not in this registry at all, which is a 404 rather
    // than a record with an empty schema.
    let (status, body) = describe_type(
        &router,
        &[
            ("Type", registry_type::RESOURCE),
            ("TypeName", "AWS::NotAService::NotAType"),
        ],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a type the registry holds no record for\": expected HTTP 404, got \
         {status} with {body:?}"
    );
    let schema = resource_type_schema("AWS::NotAService::NotAType");
    assert!(
        schema.is_none(),
        "case \"a type the registry holds no record for\": expected no schema, got \
         {schema:?}"
    );
}

/// A MODULE or a HOOK of a published resource type's name is not that resource type, so
/// the registry answers neither a record nor a schema for it: the schema belongs to the
/// extension the request identifies, not to the name it spells.
#[tokio::test]
async fn a_published_resource_type_is_not_a_module_or_a_hook() {
    let router = router();

    for type_kind in [registry_type::MODULE, registry_type::HOOK] {
        let (status, body) = describe_type(
            &router,
            &[("Type", type_kind), ("TypeName", "AWS::S3::Bucket")],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {type_kind:?}: expected HTTP 404, got {status} with {body:?}"
        );
        let schema = xml_text(&body, "Schema");
        assert!(
            schema.is_none(),
            "case {type_kind:?}: expected no schema in a refusal, got {schema:?}"
        );
    }
}
