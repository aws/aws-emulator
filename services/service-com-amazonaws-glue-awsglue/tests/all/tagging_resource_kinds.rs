// SPDX-License-Identifier: CC0-1.0

//! The tagging surface across EVERY kind of Glue resource that carries a tag set:
//! `GetTags`, `TagResource` and `UntagResource` driven against one resource of each kind.
//!
//! RULE: a tagging call resolves its `ResourceArn` to the Glue resource the ARN's resource
//! portion names — the resource-type segment picks the kind, the rest of it names the
//! resource — and reads or writes THAT resource's own tag set, for every kind of Glue
//! resource whose record carries one. The modelled `ResourceArn` is a `GlueResourceArn`,
//! constrained only by the prefix every Glue ARN shares, so it places no restriction on
//! which kind of Glue resource the ARN may name. An ARN of a taggable kind that names no
//! resource this endpoint holds is a MISS (400 `EntityNotFoundException`), while a value
//! that names no taggable Glue resource at all is a bad parameter (400
//! `InvalidInputException`).
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The tag-set helpers are
//! the tag-write suite's ([`crate::resource_tagging`]), so one harness drives every kind.
//!
//! The rows differ along what the rule generalises over: which KIND of resource the ARN
//! names — one row per kind whose create models a `Tags` map and whose resource this
//! emulator holds — how many path segments that kind's resource portion carries (a schema
//! set carries two), whether the name is a client's or an id the create minted, and whether
//! the ARN resolves to a stored resource, to none, or to no kind at all.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tagging::{
    glue_arn, map_of, read_tags, resource_arn, UNPLACEABLE_RESOURCE_ARN_NOT_FOUND,
};
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The IAM role the kinds that name one are created with. A role is account-global, so one
/// backs every resource here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-kinds-role";

/// The job a trigger's action names: a trigger's action has to name a job of the Region the
/// create is served in, so that job is registered before the trigger.
const TRIGGER_TARGET_JOB: &str = "kinds-trigger-target-job";

/// The registry a schema set is created inside, which is also the first of the two path
/// segments a schema ARN's resource portion carries.
const SCHEMA_REGISTRY: &str = "kinds-schema-registry";

/// A well-formed Avro schema definition, for the schema row.
const AVRO_DEFINITION: &str =
    r#"{"type":"record","name":"Row","fields":[{"name":"id","type":"string"}]}"#;

/// The `Message` a tagging WRITE answers for an ARN of a taggable kind that names no resource
/// this endpoint holds. Recorded wording, which names neither the ARN nor the kind.
const ENTITY_NOT_FOUND: &str = "Entity not found";

/// The `Message` the tagging READ answers for such an ARN of the kind `segment`, whose name
/// after the type segment is `name`, for an ARN of this harness's own Region and account.
pub fn read_miss_message(segment: &str, name: &str) -> String {
    read_miss_message_of_arn(segment, name, &resource_arn(segment, name))
}

/// The same, for a request that sent `arn` — which for the kinds whose sentence reports the
/// REFERENCE it was addressed by rather than a name is the value that sentence echoes, so an
/// ARN of another Region or another account is echoed as sent.
///
/// The read resolves the ARN through the surface that OWNS the kind, so its miss is that
/// surface's own sentence: recorded as "Job not found." for a job ARN, as the database
/// surface's sentence naming the database for a database ARN, as the crawler-definition
/// surface's naming the crawler for a crawler ARN, and for a registry ARN as the registry
/// surface's rendering of the reference it resolved by — the registry name it was handed
/// none of spelled as the absent member, beside the ARN the request sent. Every other kind
/// answers the generic entity sentence, which is what the read is recorded to answer for a
/// workflow ARN.
pub fn read_miss_message_of_arn(segment: &str, name: &str, arn: &str) -> String {
    match segment {
        "job" => "Job not found.".to_string(),
        "database" => format!("Database {name} not found."),
        "crawler" => format!("Crawler entry with name {name} does not exist"),
        "registry" => format!("Registry is not found. RegistryName: null, RegistryArn: {arn}"),
        _ => ENTITY_NOT_FOUND.to_string(),
    }
}

/// Every kind of Glue resource that carries a tag set, as its ARN's resource-type segment.
///
/// Source: the creates that model a `Tags` map — `CreateBlueprint`, `CreateCatalog`,
/// `CreateConnection`, `CreateCrawler`, `CreateCustomEntityType`, `CreateDatabase`,
/// `CreateDataQualityRuleset`, `CreateJob`, `CreateMLTransform`, `CreateRegistry`,
/// `CreateSchema`, `CreateSession`, `CreateTrigger`, `CreateUsageProfile` and
/// `CreateWorkflow` (from `smithy_query` on each) — paired with the resource portion of that
/// resource's Glue ARN.
const TAGGABLE_KINDS: [&str; 15] = [
    "blueprint",
    "catalog",
    "connection",
    "crawler",
    "customEntityType",
    "database",
    "dataQualityRuleset",
    "job",
    "mlTransform",
    "registry",
    "schema",
    "session",
    "trigger",
    "usageProfile",
    "workflow",
];

/// Create one resource of the kind `segment` carrying `tags`, and answer the name its ARN
/// carries after the resource-type segment — the name the create chose, the id the create
/// minted (a machine learning transform), or both names a schema set is addressed by.
async fn create_resource(
    router: &MultiServiceRouter,
    segment: &str,
    name: &str,
    tags: &Value,
) -> String {
    match segment {
        "blueprint" => {
            ok(
                router,
                "CreateBlueprint",
                &json!({
                    "Name": name,
                    "BlueprintLocation": "s3://glue-blueprints/layout.zip",
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "catalog" => {
            ok(
                router,
                "CreateCatalog",
                &json!({
                    "Name": name,
                    "CatalogInput": {
                        "FederatedCatalog": {
                            "Identifier":
                                format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
                            "ConnectionName": "aws:s3tables",
                        },
                    },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "connection" => {
            ok(
                router,
                "CreateConnection",
                &json!({
                    "ConnectionInput": {
                        "Name": name,
                        "ConnectionType": "JDBC",
                        "ConnectionProperties": {
                            "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                            "USERNAME": "etl",
                        },
                    },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "crawler" => {
            ok(
                router,
                "CreateCrawler",
                &json!({
                    "Name": name,
                    "Role": ROLE_ARN,
                    "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "customEntityType" => {
            ok(
                router,
                "CreateCustomEntityType",
                &json!({ "Name": name, "RegexString": "KEEP-[0-9]+", "Tags": tags }),
            )
            .await;
            name.to_string()
        }
        "database" => {
            ok(
                router,
                "CreateDatabase",
                &json!({ "DatabaseInput": { "Name": name }, "Tags": tags }),
            )
            .await;
            name.to_string()
        }
        "dataQualityRuleset" => {
            ok(
                router,
                "CreateDataQualityRuleset",
                &json!({ "Name": name, "Ruleset": "Rules = [ RowCount > 0 ]", "Tags": tags }),
            )
            .await;
            name.to_string()
        }
        "job" => {
            ok(
                router,
                "CreateJob",
                &json!({
                    "Name": name,
                    "Role": ROLE_ARN,
                    "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "mlTransform" => {
            // The transform's ARN names it by the `TransformId` the create minted, because
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`) — so the id the create answered is the ARN's name.
            let created = ok(
                router,
                "CreateMLTransform",
                &json!({
                    "Name": name,
                    "InputRecordTables": [],
                    "Parameters": {
                        "TransformType": "FIND_MATCHES",
                        "FindMatchesParameters": { "PrimaryKeyColumnName": "id" },
                    },
                    "Role": ROLE_ARN,
                    "Tags": tags,
                }),
            )
            .await;
            created["TransformId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("CreateMLTransform must answer a TransformId, got {created}")
                })
                .to_string()
        }
        "registry" => {
            ok(
                router,
                "CreateRegistry",
                &json!({ "RegistryName": name, "Tags": tags }),
            )
            .await;
            name.to_string()
        }
        "schema" => {
            ok(
                router,
                "CreateRegistry",
                &json!({ "RegistryName": SCHEMA_REGISTRY }),
            )
            .await;
            ok(
                router,
                "CreateSchema",
                &json!({
                    "RegistryId": { "RegistryName": SCHEMA_REGISTRY },
                    "SchemaName": name,
                    "DataFormat": "AVRO",
                    "SchemaDefinition": AVRO_DEFINITION,
                    "Tags": tags,
                }),
            )
            .await;
            format!("{SCHEMA_REGISTRY}/{name}")
        }
        "session" => {
            ok(
                router,
                "CreateSession",
                &json!({
                    "Id": name,
                    "Role": ROLE_ARN,
                    "Command": { "Name": "glueetl", "PythonVersion": "3" },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "trigger" => {
            ok(
                router,
                "CreateJob",
                &json!({
                    "Name": TRIGGER_TARGET_JOB,
                    "Role": ROLE_ARN,
                    "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
                }),
            )
            .await;
            ok(
                router,
                "CreateTrigger",
                &json!({
                    "Name": name,
                    "Type": "ON_DEMAND",
                    "Actions": [{ "JobName": TRIGGER_TARGET_JOB }],
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "usageProfile" => {
            ok(
                router,
                "CreateUsageProfile",
                &json!({
                    "Name": name,
                    "Configuration": {
                        "JobConfiguration": {
                            "numberOfWorkers": {
                                "DefaultValue": "5",
                                "MinValue": "1",
                                "MaxValue": "10",
                            },
                            // Both halves of a job's capacity pair, which the service
                            // requires defaulted together
                            // (`crate::usage_profile_job_worker_defaults`).
                            "workerType": { "DefaultValue": "G.1X" },
                        },
                    },
                    "Tags": tags,
                }),
            )
            .await;
            name.to_string()
        }
        "workflow" => {
            ok(
                router,
                "CreateWorkflow",
                &json!({ "Name": name, "Tags": tags }),
            )
            .await;
            name.to_string()
        }
        other => panic!("no taggable Glue resource kind spells {other:?}"),
    }
}

/// The name one row's create asks for, which is also the name its ARN carries for every
/// kind but the machine learning transform.
fn created_name(segment: &str) -> String {
    match segment {
        // The catalog folds a database's name and the Hive-compatible spelling of one uses
        // underscores, so this row's name is spelled that way.
        "database" => "kinds_database".to_string(),
        other => format!("kinds-{other}"),
    }
}

/// Create one resource of every kind, each carrying the tag set its own create recorded, and
/// answer the ARN of each beside the kind it is of.
async fn every_kind(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut created: Vec<(String, String)> = Vec::new();
    for segment in TAGGABLE_KINDS {
        let name = created_name(segment);
        let arn_name =
            create_resource(router, segment, &name, &json!({ "created": segment })).await;
        created.push((segment.to_string(), resource_arn(segment, &arn_name)));
    }
    created
}

/// `GetTags` reports the tag set the create recorded for EVERY kind of Glue resource that
/// carries one, `TagResource` merges into that same record, and `UntagResource` drops from
/// it — while every other resource's tag set is left alone.
#[tokio::test]
async fn every_taggable_kind_round_trips_its_tags() {
    let router = emulator();
    let created = every_kind(&router).await;

    // What each create recorded is what the read reports, before any tag write.
    let mut expected: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (segment, arn) in &created {
        let got = read_tags(&router, segment, arn).await;
        let want = map_of(&json!({ "created": segment }));
        assert_eq!(
            got, want,
            "case {segment}: GetTags must report the tags the create recorded for {arn}, got \
             {got:?}"
        );
        expected.insert(arn.clone(), want);
    }

    for (segment, arn) in &created {
        // The write merges into this resource's own tag set.
        let added = json!({ "owner": "data-platform", "env": "dev" });
        ok(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "TagsToAdd": added }),
        )
        .await;
        let after_write = expected
            .get_mut(arn)
            .unwrap_or_else(|| panic!("case {segment}: {arn} was created"));
        for (key, value) in map_of(&added) {
            after_write.insert(key, value);
        }
        let got = read_tags(&router, segment, arn).await;
        assert_eq!(
            got,
            expected[arn],
            "case {segment}: the write must leave the merge of what was held and what it sent \
             on {arn}, got {got:?}"
        );

        // And the removal drops exactly the key it names from that same record.
        ok(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagsToRemove": ["env"] }),
        )
        .await;
        expected
            .get_mut(arn)
            .unwrap_or_else(|| panic!("case {segment}: {arn} was created"))
            .remove("env");
        let got = read_tags(&router, segment, arn).await;
        assert_eq!(
            got,
            expected[arn],
            "case {segment}: the removal must drop only the key it named on {arn}, got {got:?}"
        );

        // No other kind's tag set moved.
        for (other, bystander) in &created {
            if bystander == arn {
                continue;
            }
            let got = read_tags(&router, other, bystander).await;
            assert_eq!(
                got, expected[bystander],
                "case {segment}: writing {arn} must leave {bystander} alone, got {got:?}"
            );
        }
    }
}

/// An ARN of a taggable kind that names no resource this endpoint holds is a MISS for all
/// three tagging operations — 400 `EntityNotFoundException`, worded as the refusing call
/// words one ([`read_miss_message`]) — and the refused call changes nothing.
#[tokio::test]
async fn an_arn_of_a_taggable_kind_naming_nothing_is_a_miss() {
    let router = emulator();
    let created = every_kind(&router).await;

    for segment in TAGGABLE_KINDS {
        // A name of the right shape for the kind that nothing was created under: for a
        // schema set, both segments its resource portion carries.
        let absent = if segment == "schema" {
            format!("{SCHEMA_REGISTRY}/never-created")
        } else {
            "never-created".to_string()
        };
        let arn = resource_arn(segment, &absent);
        for (operation, body) in [
            ("GetTags", json!({ "ResourceArn": arn })),
            (
                "TagResource",
                json!({ "ResourceArn": arn, "TagsToAdd": { "written": "no" } }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "TagsToRemove": ["created"] }),
            ),
        ] {
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {segment} / {operation}: {arn} names no resource and must be refused, \
                 got {status} with {answer}"
            );
            assert!(
                error_code(&answer).ends_with("EntityNotFoundException"),
                "case {segment} / {operation}: expected EntityNotFoundException, got {}",
                error_code(&answer)
            );
            // The READ words the miss as the surface owning the kind does; the two WRITES
            // answer the one fixed sentence their own recordings state.
            let expected = if operation == "GetTags" {
                read_miss_message(segment, &absent)
            } else {
                ENTITY_NOT_FOUND.to_string()
            };
            assert_eq!(
                error_message(&answer),
                expected,
                "case {segment} / {operation}: unexpected wording for a tagging miss"
            );
        }
    }

    // Nothing the misses touched: every created resource still reports its own tag set.
    for (segment, arn) in &created {
        let got = read_tags(&router, segment, arn).await;
        assert_eq!(
            got,
            map_of(&json!({ "created": segment })),
            "case {segment}: a refused call must change nothing, got {got:?} for {arn}"
        );
    }
}

/// A `ResourceArn` that is a Glue resource ARN but places no entity of this service at all is
/// a MISS: 400 `EntityNotFoundException` carrying the sentence recorded for that ARN form,
/// which no owning surface words. The ARN is a legal address — the member's modelled
/// constraint pins the Glue prefix and nothing more — so what it names is a lookup, and a
/// lookup that places nothing is an absence rather than a bad parameter.
///
/// The rows are Glue ARNs whose resource portion names a kind that carries no tag set at
/// all — a catalog table (`CreateTable` models no `Tags`), a user defined function, a
/// development endpoint, whose resource this emulator holds none of — and ARNs that name a
/// taggable kind but no one resource of it: a type segment with nothing after it, a
/// one-segment kind carrying a second segment, and a schema set missing one of its two.
#[tokio::test]
async fn an_arn_that_names_no_taggable_kind_is_a_miss() {
    let router = emulator();
    let created = every_kind(&router).await;

    for resource in [
        "table/kinds_database/orders",
        "userDefinedFunction/kinds_database/to_upper",
        "devEndpoint/dev-1",
        "tableVersion/kinds_database/orders/1",
        "catalog",
        "database/",
        "database/kinds_database/extra",
        "schema/kinds-schema-registry",
        "schema/kinds-schema-registry/kinds-schema/extra",
    ] {
        let arn = glue_arn(REGION, ACCOUNT_ID, resource);
        for (operation, body) in [
            ("GetTags", json!({ "ResourceArn": arn })),
            (
                "TagResource",
                json!({ "ResourceArn": arn, "TagsToAdd": { "written": "no" } }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "TagsToRemove": ["created"] }),
            ),
        ] {
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {resource:?} / {operation}: a Glue ARN that places no entity must be \
                 refused, got {status} with {answer}"
            );
            assert!(
                error_code(&answer).ends_with("EntityNotFoundException"),
                "case {resource:?} / {operation}: expected EntityNotFoundException, got {}",
                error_code(&answer)
            );
            assert_eq!(
                error_message(&answer),
                UNPLACEABLE_RESOURCE_ARN_NOT_FOUND,
                "case {resource:?} / {operation}: unexpected wording for a tagging miss"
            );
        }
    }

    // And the resources this endpoint does serve are untouched by the refusals.
    for (segment, arn) in &created {
        let got = read_tags(&router, segment, arn).await;
        assert_eq!(
            got,
            map_of(&json!({ "created": segment })),
            "case {segment}: a refused call must change nothing, got {got:?} for {arn}"
        );
    }
}
