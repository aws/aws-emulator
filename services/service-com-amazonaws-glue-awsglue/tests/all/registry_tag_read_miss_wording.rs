// SPDX-License-Identifier: CC0-1.0

//! What the tag READ says when its `ResourceArn` names a schema REGISTRY this endpoint holds
//! none of.
//!
//! RULE: `GetTags` resolves its `ResourceArn` through the surface that OWNS the kind the
//! ARN's resource-type segment names, so the modelled `EntityNotFoundException` it answers
//! (HTTP 400) carries THAT surface's own miss sentence, with the sentence's variable parts
//! taken from the request — the name the ARN carries where the surface reports a name, and
//! the REFERENCE the lookup was addressed by where it reports that instead. The registry
//! surface reports the reference: it renders the registry name beside the registry ARN it
//! resolved by, and a member the request carried none of is rendered as the absent member, so
//! an address that is an ARN alone carries no name to report and the ARN the request sent is
//! echoed as sent.
//!
//! Every way a registry ARN can name nothing is the same miss: a name nothing was created
//! under, a name differing only in case from a stored one (the catalog folds a database's
//! names, not a registry's), another account's estate and another Region's — each of which is
//! THIS endpoint holding no such registry.
//!
//! The rows differ along WHY the registry ARN resolves to nothing, along which Region's
//! endpoint serves the call, along which tagging operation refuses (only the READ words the
//! miss through the owning surface — the two WRITES answer the one fixed sentence their own
//! recordings state), and along the KIND the ARN names, so a schema set, which lives inside a
//! registry and is addressed by both names, is pinned to keep the generic sentence.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The per-kind sentence is
//! the one the kinds suite pins ([`crate::tagging_resource_kinds`]).

use serde_json::json;

use crate::resource_tagging::{glue_arn, map_of, read_tags, resource_arn};
use crate::tagging_resource_kinds::read_miss_message_of_arn;
use crate::wire::{
    call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The registry the rows below address, and the sibling registry that is never addressed by
/// a refusal, so a refused read is visible as its tag set changing or disappearing.
const REGISTRY: &str = "tag-read-registry";
const SIBLING: &str = "tag-read-registry-kept";

/// The registry a schema set is created inside, and that schema set's own name: a schema
/// ARN's resource portion carries both.
const SCHEMA_REGISTRY: &str = "tag-read-schema-registry";
const SCHEMA: &str = "tag-read-schema";

/// A well-formed Avro schema definition, for the schema row.
const AVRO_DEFINITION: &str =
    r#"{"type":"record","name":"Row","fields":[{"name":"id","type":"string"}]}"#;

/// The Region whose ARNs and whose endpoint stand in for "not the one serving the request".
const OTHER_REGION: &str = "us-west-2";

/// An account other than the one this emulator serves, so an ARN of it names a registry this
/// endpoint holds nothing for whatever the name inside it is.
const OTHER_ACCOUNT: &str = "111122223333";

/// The tag set every registry here is created with.
fn registry_tags() -> serde_json::Value {
    json!({ "k": "v" })
}

/// Register the registries and the schema set the rows read back.
async fn registries(router: &smithy_server_runtime::MultiServiceRouter) {
    for name in [REGISTRY, SIBLING, SCHEMA_REGISTRY] {
        ok(
            router,
            "CreateRegistry",
            &json!({ "RegistryName": name, "Tags": registry_tags() }),
        )
        .await;
    }
    ok(
        router,
        "CreateSchema",
        &json!({
            "RegistryId": { "RegistryName": SCHEMA_REGISTRY },
            "SchemaName": SCHEMA,
            "DataFormat": "AVRO",
            "SchemaDefinition": AVRO_DEFINITION,
            "Tags": registry_tags(),
        }),
    )
    .await;
}

/// Assert that `GetTags` of `arn`, served in `region`, is refused with `expected`.
async fn assert_miss(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
    expected: &str,
) {
    let (status, body) =
        call_in_region(router, region, "GetTags", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        status, 400,
        "case {case}: {arn} names no registry in {region} and must be refused, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException, got {} in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        expected,
        "case {case}: unexpected wording for the tag read's miss of {arn}, got {:?}",
        error_message(&body)
    );
    assert!(
        body.get("Tags").is_none(),
        "case {case}: a refused read reports no tags, got {body}"
    );
}

/// A registry ARN that names no registry this endpoint holds earns the registry surface's own
/// miss sentence, which reports the reference the lookup was addressed by: no registry name,
/// and the ARN the request sent.
#[tokio::test]
async fn the_tag_read_of_an_absent_registry_reports_the_arn_it_was_addressed_by() {
    let router = emulator();
    registries(&router).await;

    // (case, the Region serving the request, the ARN it sends).
    let rows: Vec<(String, &str, String)> = vec![
        (
            "a name nothing was created under".to_string(),
            REGION,
            resource_arn("registry", "registry-never-created"),
        ),
        (
            "a name that is a prefix of a stored one".to_string(),
            REGION,
            resource_arn("registry", "tag-read-registr"),
        ),
        (
            // A registry's names are not folded the way the catalog folds a database's, so
            // another casing of a stored name names no registry at all.
            "a stored name in another case".to_string(),
            REGION,
            resource_arn("registry", &REGISTRY.to_uppercase()),
        ),
        (
            "another account's ARN under a stored name".to_string(),
            REGION,
            glue_arn(REGION, OTHER_ACCOUNT, &format!("registry/{REGISTRY}")),
        ),
        (
            "another Region's ARN at this endpoint".to_string(),
            REGION,
            glue_arn(OTHER_REGION, ACCOUNT_ID, &format!("registry/{REGISTRY}")),
        ),
        (
            "this Region's ARN at another endpoint".to_string(),
            OTHER_REGION,
            resource_arn("registry", REGISTRY),
        ),
    ];

    for (case, region, arn) in &rows {
        // The sentence is the registry surface's, with the ARN the request sent echoed as
        // sent and no registry name to report.
        let expected = format!("Registry is not found. RegistryName: null, RegistryArn: {arn}");
        assert_eq!(
            expected,
            read_miss_message_of_arn("registry", REGISTRY, arn),
            "case {case}: the per-kind sentence the kinds suite pins must be this one"
        );
        assert_miss(&router, case, region, arn, &expected).await;
    }

    // The registries that DO exist still report the tags their creates recorded, so no
    // refusal above touched a stored tag set.
    for name in [REGISTRY, SIBLING] {
        let got = read_tags(&router, name, &resource_arn("registry", name)).await;
        assert_eq!(
            got,
            map_of(&registry_tags()),
            "case {name}: a refused read must leave the registry's tags alone, got {got:?}"
        );
    }
}

/// Only the READ words a registry miss through the registry surface: the two tag WRITES
/// answer the one fixed, generic sentence their own recordings state, and a schema set — the
/// kind that lives INSIDE a registry — keeps the generic sentence on the read as well.
#[tokio::test]
async fn the_registry_sentence_is_the_read_s_and_the_registry_kind_s_alone() {
    let router = emulator();
    registries(&router).await;

    let absent_registry = resource_arn("registry", "registry-never-created");
    let absent_schema = resource_arn("schema", &format!("{SCHEMA_REGISTRY}/never-created"));

    // (case, the ARN, the operation, the sentence it must carry).
    let rows: Vec<(String, &String, &str, String)> = vec![
        (
            "a registry ARN on the read".to_string(),
            &absent_registry,
            "GetTags",
            read_miss_message_of_arn("registry", REGISTRY, &absent_registry),
        ),
        (
            "a registry ARN on the merge".to_string(),
            &absent_registry,
            "TagResource",
            "Entity not found".to_string(),
        ),
        (
            "a registry ARN on the removal".to_string(),
            &absent_registry,
            "UntagResource",
            "Entity not found".to_string(),
        ),
        (
            "a schema ARN on the read".to_string(),
            &absent_schema,
            "GetTags",
            read_miss_message_of_arn("schema", SCHEMA, &absent_schema),
        ),
        (
            "a schema ARN on the merge".to_string(),
            &absent_schema,
            "TagResource",
            "Entity not found".to_string(),
        ),
    ];

    for (case, arn, operation, expected) in &rows {
        let body = match *operation {
            "GetTags" => json!({ "ResourceArn": arn }),
            "TagResource" => json!({ "ResourceArn": arn, "TagsToAdd": { "written": "no" } }),
            _ => json!({ "ResourceArn": arn, "TagsToRemove": ["k"] }),
        };
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {arn} names nothing and must be refused, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {} in {answer}",
            error_code(&answer)
        );
        assert_eq!(
            error_message(&answer),
            *expected,
            "case {case}: unexpected wording for a tagging miss, got {:?}",
            error_message(&answer)
        );
    }

    // The registry and the schema set that exist still answer their own exact tag maps, so
    // the refusals above wrote nothing.
    let got = read_tags(&router, REGISTRY, &resource_arn("registry", REGISTRY)).await;
    assert_eq!(
        got,
        map_of(&registry_tags()),
        "the stored registry must still report its create's tags, got {got:?}"
    );
    let schema_arn = resource_arn("schema", &format!("{SCHEMA_REGISTRY}/{SCHEMA}"));
    let got = read_tags(&router, SCHEMA, &schema_arn).await;
    assert_eq!(
        got,
        map_of(&registry_tags()),
        "the stored schema set must still report its create's tags, got {got:?}"
    );
}
