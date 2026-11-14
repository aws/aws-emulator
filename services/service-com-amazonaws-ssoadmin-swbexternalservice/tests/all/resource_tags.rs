// SPDX-License-Identifier: CC0-1.0

//! The resource-tagging slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a tag set is keyed by `Tag$Key`, so `TagResource` MERGES: a key the resource
//!    already carries takes the requested value, a key it does not carry is added, and
//!    every other key it holds is left alone — which makes repeating a call change
//!    nothing further, and which is why a request naming one key twice is refused rather
//!    than merged (`duplicate_tag_keys.rs` owns that refusal);
//! 2. the merge is stored ON the resource the `ResourceArn` names — each of the four
//!    kinds the model's `TaggableResourceArn` admits (an instance, a permission set, an
//!    application and a trusted token issuer) — and `ListTagsForResource` REPORTS exactly
//!    the stored set, one entry per distinct key, in key order, writing nothing itself;
//! 3. a `ResourceArn` this account holds nothing under, and one held under an instance
//!    other than the `InstanceArn` the request carried, are refused with the modelled
//!    `ResourceNotFoundException` by both operations and store nothing, while a resource
//!    that exists and carries no tag lists EMPTY rather than being refused;
//! 4. a `NextToken` this enumeration did not issue is refused with the modelled
//!    `ValidationException`, and one it did issue resumes after its key;
//! 5. a write that would push a resource past the tag count the model's `TagList` bounds
//!    it at is refused with the modelled `ServiceQuotaExceededException` and stores
//!    nothing, while re-valuing a key the resource already carries never crosses it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the tests run
//! against an ORGANIZATION instance so that all four taggable kinds — permission sets
//! among them — can be created.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, organization_engine, organization_router, router_over, with_store,
    TEST_REGION, UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
};

/// The provider a customer managed application is created under: the `custom` entry of
/// the published catalog (`state::BUILT_IN_APPLICATION_PROVIDERS`), which is the one
/// `CreateApplication` serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// One `TagResource` call of the table: the tags it sends, in the order it sends them.
#[derive(Debug, Clone)]
struct Batch {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// Owned `(Key, Value)` pairs from literals, for a table row.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The batches rules 1 and 2 are asserted over, applied to one resource IN ORDER so each
/// row composes with everything the rows before it stored.
///
/// They differ along what the merge rule generalises over: the NUMBER of tags in one call
/// (none, one, several, and a key repeated inside a single call), whether a key is new or
/// already carried, and the length and character class of `Key` and `Value` at their
/// published bounds. The constraints are from
/// `smithy_query(query="shape",shape="Tag,TagList")`: `Key` `length 1..128`, `Value`
/// `length 0..256`, both `pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, and `TagList`
/// `length 0..75`.
fn batches() -> Vec<Batch> {
    vec![
        Batch {
            label: "one new key",
            tags: pairs(&[("env", "prod")]),
        },
        Batch {
            label: "two more new keys, one with the empty Value the 0 lower bound admits",
            tags: pairs(&[("owner", "platform"), ("cost.centre", "")]),
        },
        Batch {
            label: "a key already carried, re-valued in place",
            tags: pairs(&[("env", "staging")]),
        },
        Batch {
            label: "keys spanning the \\p{L} \\p{Z} \\p{N} _.:/=+-@ classes of the pattern",
            tags: pairs(&[
                ("Clé de voûte", "valeur Ünicode"),
                ("path/to:thing=1+2@x_.", "0123456789"),
                ("key with spaces", "value with spaces"),
            ]),
        },
        Batch {
            label: "a Key at the 128-character bound and a Value at the 256-character bound",
            tags: vec![("k".repeat(128), "v".repeat(256))],
        },
        Batch {
            label: "an empty Tags list, which the TagList lower bound of 0 admits",
            tags: Vec::new(),
        },
        Batch {
            label: "a second re-valuing of a key, after everything above",
            tags: pairs(&[("owner", "identity"), ("fresh", "tag")]),
        },
    ]
}

/// The `Tags` member of a request, as the wire carries it.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// `TagResource` for `resource_arn`, with the optional `InstanceArn` the caller chose.
async fn tag_resource(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn, "Tags": tag_list(tags)});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    call(router, "TagResource", &body).await
}

/// `ListTagsForResource` for `resource_arn`.
async fn list_tags(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, "ListTagsForResource", &body).await
}

/// The `(Key, Value)` pairs a `ListTagsForResource` answer reported, in the order it
/// reported them.
fn reported(body: &Value) -> Vec<(String, String)> {
    body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tag set a resource is expected to carry, as the ordered pairs a report must name.
fn expected_pairs(expected: &BTreeMap<String, String>) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Create one permission set under `instance_arn` and return its ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {body}"))
        .to_string()
}

/// Create one customer managed application under `instance_arn` and return its ARN.
async fn create_application(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": PROVIDER_CUSTOM,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApplication {name:?}: expected 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication {name:?} minted no ARN: {body}"))
        .to_string()
}

/// Create one trusted token issuer under `instance_arn` and return its ARN.
async fn create_trusted_token_issuer(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateTrustedTokenIssuer",
        &json!({
            "InstanceArn": instance_arn,
            "Name": name,
            "TrustedTokenIssuerType": ISSUER_TYPE,
            "TrustedTokenIssuerConfiguration": {
                "OidcJwtConfiguration": {
                    "IssuerUrl": "https://issuer.example.com/oidc",
                    "ClaimAttributePath": "email",
                    "IdentityStoreAttributePath": "emails.primary.value",
                    "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTrustedTokenIssuer {name:?}: expected 200, got {status} {body}"
    );
    body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateTrustedTokenIssuer {name:?} minted no ARN: {body}"))
        .to_string()
}

/// One resource of every kind the model's `TaggableResourceArn` admits, under
/// `instance_arn`: the kind's label and the resource's ARN. The instance is a taggable
/// resource itself, so it is the first row.
async fn taggable_resources(
    router: &MultiServiceRouter,
    instance_arn: &str,
    suffix: &str,
) -> Vec<(String, String)> {
    vec![
        ("an instance".to_string(), instance_arn.to_string()),
        (
            "a permission set".to_string(),
            create_permission_set(router, instance_arn, &format!("ps-{suffix}")).await,
        ),
        (
            "an application".to_string(),
            create_application(router, instance_arn, &format!("app {suffix}")).await,
        ),
        (
            "a trusted token issuer".to_string(),
            create_trusted_token_issuer(router, instance_arn, &format!("tti-{suffix}")).await,
        ),
    ]
}

/// Rules 1 and 2: on every taggable kind, each batch merges into what is already stored,
/// the report names exactly the merged set in key order, and repeating either call
/// changes nothing.
#[tokio::test]
async fn tagging_merges_into_the_stored_set_on_every_taggable_resource_kind() {
    let (router, instance_arn) = organization_router();

    for (kind, resource_arn) in taggable_resources(&router, &instance_arn, "merge").await {
        // Rule 3's other half: a resource that exists and carries no tag lists empty.
        let (status, fresh) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: listing a fresh resource must answer 200, got {status} {fresh}"
        );
        assert!(
            reported(&fresh).is_empty(),
            "case {kind:?}: a resource nothing has tagged carries no tag, got {fresh}"
        );
        assert!(
            fresh["NextToken"].is_null(),
            "case {kind:?}: a single page carries no NextToken, got {fresh}"
        );

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for batch in batches() {
            let case = format!("{kind} / {}", batch.label);
            let (status, body) =
                tag_resource(&router, Some(&instance_arn), &resource_arn, &batch.tags).await;
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource must answer 200, got {status} {body}"
            );
            // The same merge the service must have performed: last value per key wins.
            for (key, value) in &batch.tags {
                expected.insert(key.clone(), value.clone());
            }
            let want = expected_pairs(&expected);

            let (status, listed) =
                list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed),
                want,
                "case {case:?}: expected the merged set {want:?} in key order, got {listed}"
            );

            // Rule 1: the same write again is the same set, not a second copy of it.
            let (status, repeated) =
                tag_resource(&router, Some(&instance_arn), &resource_arn, &batch.tags).await;
            assert_eq!(
                status, 200,
                "case {case:?}: repeating TagResource must answer 200, got {status} {repeated}"
            );
            // Rule 2: the list reports rather than mutates, so it answers identically.
            let (status, relisted) =
                list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated list must answer 200, got {status} {relisted}"
            );
            assert_eq!(
                relisted, listed,
                "case {case:?}: neither a repeated write nor a repeated read may change what \
                 a later caller sees, got {relisted}"
            );

            // The `InstanceArn` is optional on both operations: omitting it names the
            // same resource, since the ARN already carries the instance.
            let (status, without_instance) = list_tags(&router, None, &resource_arn, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: listing without InstanceArn must answer 200, got {status} \
                 {without_instance}"
            );
            assert_eq!(
                reported(&without_instance),
                want,
                "case {case:?}: the answer must not depend on the optional InstanceArn, got \
                 {without_instance}"
            );
        }
    }
}

/// Rule 1 where the tag set's KEYING bites: a key is unique within one request, so a create
/// naming the same key twice is refused and mints nothing — and the key is then stored once
/// and re-valued by a later write, which is what a merge is for.
/// `duplicate_tag_keys.rs` owns that refusal across every write that takes `Tags`.
#[tokio::test]
async fn a_create_naming_one_key_twice_is_refused_and_the_key_is_stored_once() {
    let (router, instance_arn) = organization_router();
    let (status, refused) = call(
        &router,
        "CreatePermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "Name": "duplicate-keyed-set",
            "Tags": [
                {"Key": "env", "Value": "first"},
                {"Key": "env", "Value": "second"},
                {"Key": "owner", "Value": "platform"},
            ],
        }),
    )
    .await;
    assert_refused(
        "a create naming one key twice",
        "CreatePermissionSet",
        "ValidationException",
        status,
        &refused,
    );
    let (status, listed) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a create naming one key twice\": ListPermissionSets expected 200, got \
         {status} {listed}"
    );
    assert_eq!(
        listed["PermissionSets"].as_array().map(Vec::len),
        Some(0),
        "case \"a create naming one key twice\": a refused create must mint no resource, \
         got {listed}"
    );

    // The same key, named ONCE by a create that is served, and then re-valued by a write.
    let (status, created) = call(
        &router,
        "CreatePermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "Name": "distinct-keyed-set",
            "Tags": [
                {"Key": "env", "Value": "second"},
                {"Key": "owner", "Value": "platform"},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"distinct keys\": CreatePermissionSet expected 200, got {status} {created}"
    );
    let resource_arn = created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create minted no ARN: {created}"))
        .to_string();

    let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
    assert_eq!(
        status, 200,
        "case \"distinct keys\": expected 200, got {status} {listed}"
    );
    assert_eq!(
        reported(&listed),
        pairs(&[("env", "second"), ("owner", "platform")]),
        "case \"distinct keys\": one value per key, got {listed}"
    );

    let (status, body) = tag_resource(
        &router,
        Some(&instance_arn),
        &resource_arn,
        &pairs(&[("env", "third")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"re-valuing a stored key\": expected 200, got {status} {body}"
    );
    let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
    assert_eq!(
        status, 200,
        "case \"re-valuing a stored key\": the list must answer 200, got {status} {listed}"
    );
    assert_eq!(
        reported(&listed),
        pairs(&[("env", "third"), ("owner", "platform")]),
        "case \"re-valuing a stored key\": the report must name the value just \
         written, got {listed}"
    );
}

/// Rule 2: a write lands on the ONE resource its `ResourceArn` names.
#[tokio::test]
async fn a_write_touches_only_the_resource_it_names() {
    let (router, instance_arn) = organization_router();
    let resources = taggable_resources(&router, &instance_arn, "isolated").await;

    // One distinct tag per resource, written one resource at a time.
    for (index, (kind, resource_arn)) in resources.iter().enumerate() {
        let tags = vec![("owned.by".to_string(), format!("resource-{index}"))];
        let (status, body) = tag_resource(&router, Some(&instance_arn), resource_arn, &tags).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: TagResource must answer 200, got {status} {body}"
        );
    }

    for (index, (kind, resource_arn)) in resources.iter().enumerate() {
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: ListTagsForResource must answer 200, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            vec![("owned.by".to_string(), format!("resource-{index}"))],
            "case {kind:?}: a resource must carry only the tag written to it, got {listed}"
        );
    }
}

/// Rule 3: a `ResourceArn` this account holds nothing under is refused by both
/// operations, for every kind of ARN the model's `TaggableResourceArn` admits.
#[tokio::test]
async fn an_arn_this_account_holds_nothing_under_is_refused_by_both_operations() {
    let (router, instance_arn) = organization_router();
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One resource of each kind exists, so the refusals below are about the ARN asked for
    // and not about an empty store.
    let held = taggable_resources(&router, &instance_arn, "held").await;
    let application_arn = held[2].1.clone();
    // The emulator's own account, read off an ARN it minted rather than restated here.
    let account = application_arn
        .split(':')
        .nth(4)
        .unwrap_or_default()
        .to_string();

    // The rows span the four resource kinds the pattern admits, plus a held resource in
    // another partition — pattern-valid, and a different resource.
    let cases: Vec<(&str, String)> = vec![
        (
            "an instance ARN nothing was created under",
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "a permissionSet ARN no create minted",
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "an application ARN no create minted",
            format!("arn:aws:sso::{account}:application/{instance_id}/apl-0000000000000000"),
        ),
        (
            "a trustedTokenIssuer ARN no create minted",
            format!(
                "arn:aws:sso::{account}:trustedTokenIssuer/{instance_id}/\
                 tti-00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a held application in another partition",
            application_arn.replacen("arn:aws:", "arn:aws-us-gov:", 1),
        ),
    ];

    for (case, resource_arn) in cases {
        let (status, body) = tag_resource(
            &router,
            Some(&instance_arn),
            &resource_arn,
            &pairs(&[("env", "prod")]),
        )
        .await;
        assert_refused(case, "TagResource", "ResourceNotFoundException", status, &body);

        let (status, body) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
        assert_refused(
            case,
            "ListTagsForResource",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // The refusals wrote nothing: every held resource still carries no tag.
    for (kind, resource_arn) in &held {
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the held resource must still list, got {status} {listed}"
        );
        assert!(
            reported(&listed).is_empty(),
            "case {kind:?}: a refused tagging must store nothing, got {listed}"
        );
    }
}

/// Rule 3: an `InstanceArn` that holds the resource is what makes the call about it — an
/// instance this account does not hold, and one that holds something else, are both
/// refused, and neither writes.
#[tokio::test]
async fn a_resource_held_under_another_instance_is_refused_and_writes_nothing() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine.clone());
    let resources = taggable_resources(&router, &instance_arn, "scoped").await;
    let stored = pairs(&[("env", "prod")]);
    for (kind, resource_arn) in &resources {
        let (status, body) = tag_resource(&router, Some(&instance_arn), resource_arn, &stored).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the first tagging must answer 200, got {status} {body}"
        );
    }

    // A SECOND instance this account holds, seeded through the same `state::Instance`
    // constructor the handlers use: no operation of this API enables an organization
    // instance, and the rule under test needs an instance that exists but holds none of
    // the resources above.
    let other_instance_arn = with_store(&engine, |store| {
        let instance = state::Instance::active(
            "ssoins-f1e2d3c4b5a60798",
            "d-0f1e2d3c4b",
            state::InstanceType::Organization,
            Some("Other".to_string()),
            aws_smithy_types::DateTime::from(std::time::SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });

    // The rows: an instance this account does not hold at all, and one it does hold that
    // holds none of these resources.
    let cases: Vec<(&str, &str)> = vec![
        (
            "an InstanceArn nothing was enabled under",
            UNKNOWN_INSTANCE_ARN,
        ),
        (
            "an InstanceArn that holds another instance's resources",
            other_instance_arn.as_str(),
        ),
    ];

    for (case, wrong_instance_arn) in cases {
        for (kind, resource_arn) in &resources {
            // The instance resource itself IS the instance, so under another
            // `InstanceArn` it names a different resource than the one asked for.
            let case = format!("{case} / {kind}");
            let (status, body) = tag_resource(
                &router,
                Some(wrong_instance_arn),
                resource_arn,
                &pairs(&[("env", "overwritten")]),
            )
            .await;
            assert_refused(
                &case,
                "TagResource",
                "ResourceNotFoundException",
                status,
                &body,
            );

            let (status, body) = list_tags(&router, Some(wrong_instance_arn), resource_arn, None).await;
            assert_refused(
                &case,
                "ListTagsForResource",
                "ResourceNotFoundException",
                status,
                &body,
            );
        }
    }

    // Nothing the refusals touched moved: each resource still carries what it was tagged
    // with under its own instance.
    for (kind, resource_arn) in &resources {
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource must still list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            stored,
            "case {kind:?}: a refused tagging must leave the stored set alone, got {listed}"
        );
    }

    // The second instance is a taggable resource of its own, so the scoping rule refuses
    // a MISMATCH rather than every request: tagging it under itself is served.
    let (status, body) = tag_resource(
        &router,
        Some(&other_instance_arn),
        &other_instance_arn,
        &pairs(&[("env", "other")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging the second instance under itself must answer 200, got {status} {body}"
    );
    let (status, listed) = list_tags(&router, Some(&other_instance_arn), &other_instance_arn, None).await;
    assert_eq!(
        status, 200,
        "listing the second instance's tags must answer 200, got {status} {listed}"
    );
    assert_eq!(
        reported(&listed),
        pairs(&[("env", "other")]),
        "the second instance must carry its own tag, got {listed}"
    );
}

/// Rule 4: a `NextToken` this enumeration did not issue is refused; one it issued resumes
/// after the key it names.
#[tokio::test]
async fn a_next_token_this_enumeration_did_not_issue_is_refused() {
    let (router, instance_arn) = organization_router();
    let resource_arn = create_permission_set(&router, &instance_arn, "cursor-set").await;
    let other_arn = create_permission_set(&router, &instance_arn, "other-set").await;
    let stored = pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4"), ("e", "5")]);
    let (status, body) = tag_resource(&router, Some(&instance_arn), &resource_arn, &stored).await;
    assert_eq!(status, 200, "the seed tagging must answer 200, got {status} {body}");

    // A cursor this enumeration DID issue: the page resumes strictly after its key.
    let resumed = state::encode_page_token(&state::cursor_tags(&resource_arn), "b");
    let (status, body) = list_tags(&router, Some(&instance_arn), &resource_arn, Some(&resumed)).await;
    assert_eq!(
        status, 200,
        "case \"a cursor of this enumeration\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported(&body),
        pairs(&[("c", "3"), ("d", "4"), ("e", "5")]),
        "case \"a cursor of this enumeration\": the page must resume after its key, got {body}"
    );

    // The rows span the ways a token can fail to be one of this enumeration's: not a
    // cursor at all, a cursor of the same operation on ANOTHER resource, and a cursor of
    // another enumeration entirely. All stay inside `Token`'s `^[-a-zA-Z0-9+=/_]*$`.
    let cases: Vec<(&str, String)> = vec![
        ("not a cursor at all", "not-a-cursor".to_string()),
        (
            "a cursor of this operation on another resource",
            state::encode_page_token(&state::cursor_tags(&other_arn), "a"),
        ),
        (
            "a cursor of another enumeration",
            state::encode_page_token(state::CURSOR_INSTANCES, &instance_arn),
        ),
    ];

    for (case, token) in cases {
        let (status, body) =
            list_tags(&router, Some(&instance_arn), &resource_arn, Some(&token)).await;
        assert_refused(
            case,
            "ListTagsForResource",
            "ValidationException",
            status,
            &body,
        );
    }

    // The refusals changed nothing: the whole set still reports.
    let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
    assert_eq!(status, 200, "the resource must still list, got {status} {listed}");
    assert_eq!(
        reported(&listed),
        stored,
        "a refused cursor must leave the stored set alone, got {listed}"
    );
}

/// Rule 5: the tag count a resource can carry is bounded, a re-valuing never crosses the
/// bound, and the write that would is refused without storing anything.
#[tokio::test]
async fn a_write_past_the_tag_bound_is_refused_and_stores_nothing() {
    let (router, instance_arn) = organization_router();
    let ceiling = state::MAX_TAGS_PER_RESOURCE;

    for (kind, resource_arn) in taggable_resources(&router, &instance_arn, "ceiling").await {
        // A full set, in ONE call: `TagList` admits exactly this many members.
        let full: Vec<(String, String)> = (0..ceiling)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect();
        let (status, body) = tag_resource(&router, Some(&instance_arn), &resource_arn, &full).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: a full tag set must answer 200, got {status} {body}"
        );
        let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the full set must list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed).len(),
            ceiling,
            "case {kind:?}: expected {ceiling} tags, got {listed}"
        );

        // At the ceiling, re-valuing a key the resource already carries adds no tag.
        let revalued = pairs(&[("key-000", "revalued")]);
        let (status, body) =
            tag_resource(&router, Some(&instance_arn), &resource_arn, &revalued).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: re-valuing a carried key must answer 200, got {status} {body}"
        );
        let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the set must still list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed).len(),
            ceiling,
            "case {kind:?}: a re-valuing adds no tag, got {listed}"
        );
        assert!(
            reported(&listed).contains(&("key-000".to_string(), "revalued".to_string())),
            "case {kind:?}: the re-valued key must carry its new value, got {listed}"
        );

        // The whole set as it stands, so each refusal below can be shown to have left it
        // exactly as it was.
        let before = reported(&listed);

        // One key past the ceiling, alone and alongside a carried key: both refused.
        let over: Vec<(&str, Vec<(String, String)>)> = vec![
            ("one new key alone", pairs(&[("one.too.many", "x")])),
            (
                "one new key beside a carried one",
                pairs(&[("key-001", "again"), ("also.too.many", "x")]),
            ),
        ];
        for (over_case, tags) in over {
            let case = format!("{kind} / {over_case}");
            let (status, body) =
                tag_resource(&router, Some(&instance_arn), &resource_arn, &tags).await;
            assert_refused(
                &case,
                "TagResource",
                "ServiceQuotaExceededException",
                status,
                &body,
            );
            let (status, listed) =
                list_tags(&router, Some(&instance_arn), &resource_arn, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the set must still list, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed),
                before,
                "case {case:?}: a refused write stores nothing, so the set must be \
                 {before:?}, got {listed}"
            );
        }
    }
}
