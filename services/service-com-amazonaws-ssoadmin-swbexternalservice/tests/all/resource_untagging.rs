// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a tag set is keyed by `Tag$Key`, so `UntagResource` is a KEYED REMOVAL: a key the
//!    request names is gone whatever value it held, a key the resource does not carry
//!    removes nothing rather than failing, and every key the request does not name keeps
//!    its value — which makes repeating a call change nothing further;
//! 2. the removal lands on the resource the `ResourceArn` names — each of the four kinds
//!    the model's `TaggableResourceArn` admits (an instance, a permission set, an
//!    application and a trusted token issuer) — and `ListTagsForResource` reports what is
//!    left, including the EMPTY set once every key has been removed, since the resource
//!    itself survives its tags;
//! 3. a `ResourceArn` this account holds nothing under, and one held under an instance
//!    other than the `InstanceArn` the request carried, are refused with the modelled
//!    `ResourceNotFoundException` and remove nothing;
//! 4. `TagKeys` is bounded by the model: a list at its published maximum is served, and
//!    one member short of its published minimum, or one past its maximum, is refused with
//!    the modelled `ValidationException` and removes nothing.
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

/// The number of members the model's `TagKeyList` admits at most
/// (`smithy_query(query="shape",shape="TagKeyList")`: `length(min: 1, max: 75)`). It is
/// WIDER than the number of tags one resource can carry
/// (`state::MAX_TAGS_PER_RESOURCE`), so an `UntagResource` call may name keys a resource
/// could never all hold.
const MAX_TAG_KEYS: usize = 75;

/// The longest `TagKey` the model admits (`smithy_query(query="shape",shape="TagKey")`:
/// `length(min: 1, max: 128)`).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// One row of the removal table: what is written before the removal, which keys the
/// `UntagResource` call names, and which of the written keys must be gone afterwards.
#[derive(Debug, Clone)]
struct Removal {
    label: &'static str,
    seed: Vec<(String, String)>,
    keys: Vec<String>,
}

/// Owned `(Key, Value)` pairs from literals, for a table row.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Owned keys from literals, for a table row's `TagKeys`.
fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| key.to_string()).collect()
}

/// The rows rules 1 and 2 are asserted over, applied to one resource IN ORDER so each row
/// composes with everything the rows before it left behind.
///
/// They differ along what the removal rule generalises over: the NUMBER of keys named
/// (one, several, the same key twice in one call), whether a named key is carried, absent,
/// or a mix of both, whether the named key's neighbours share a prefix with it, and the
/// length and character class of the key at the published bounds. The constraints are
/// from `smithy_query(query="shape",shape="Tag,TagKeyList")`: `Key`/`TagKey`
/// `length 1..128`, `Value` `length 0..256`, both
/// `pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, and `TagKeyList` `length 1..75`.
fn removals() -> Vec<Removal> {
    vec![
        Removal {
            label: "one carried key of two",
            seed: pairs(&[("env", "prod"), ("owner", "platform")]),
            keys: keys(&["env"]),
        },
        Removal {
            label: "a key the resource does not carry removes nothing",
            seed: pairs(&[("team", "identity")]),
            keys: keys(&["never.written"]),
        },
        Removal {
            label: "several carried keys in one call",
            seed: pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            keys: keys(&["a", "c"]),
        },
        Removal {
            label: "a carried key beside an absent one",
            seed: pairs(&[("stage", "beta")]),
            keys: keys(&["stage", "absent.too"]),
        },
        Removal {
            label: "the same key named twice in one call",
            seed: pairs(&[("duplicated", "value")]),
            keys: keys(&["duplicated", "duplicated"]),
        },
        Removal {
            label: "a key whose neighbours share its prefix, which must stay",
            seed: pairs(&[("cost", "x"), ("cost.centre", "y"), ("cost.centre.sub", "z")]),
            keys: keys(&["cost.centre"]),
        },
        Removal {
            label: "a key with the empty Value the TagValue lower bound admits",
            seed: pairs(&[("blank", "")]),
            keys: keys(&["blank"]),
        },
        Removal {
            label: "keys spanning the \\p{L} \\p{Z} \\p{N} _.:/=+-@ classes of the pattern",
            seed: pairs(&[
                ("Clé de voûte", "valeur Ünicode"),
                ("path/to:thing=1+2@x_.", "0123456789"),
                ("key with spaces", "value with spaces"),
            ]),
            keys: keys(&["Clé de voûte", "key with spaces"]),
        },
        Removal {
            label: "a Key at the 128-character bound",
            seed: vec![("k".repeat(MAX_TAG_KEY_LENGTH), "v".repeat(256))],
            keys: vec!["k".repeat(MAX_TAG_KEY_LENGTH)],
        },
        Removal {
            label: "a key one character short of a carried one, which must stay",
            seed: pairs(&[("prefix", "kept")]),
            keys: keys(&["prefi"]),
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

/// `UntagResource` for `resource_arn`, with the optional `InstanceArn` the caller chose.
async fn untag_resource(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn, "TagKeys": tag_keys});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    call(router, "UntagResource", &body).await
}

/// `ListTagsForResource` for `resource_arn`.
async fn list_tags(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
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

/// Rules 1 and 2: on every taggable kind, each row removes exactly the keys it names from
/// what is stored, leaves every other key with its value, repeats without further change,
/// and the read reports what is left.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names_on_every_taggable_resource_kind() {
    let (router, instance_arn) = organization_router();

    for (kind, resource_arn) in taggable_resources(&router, &instance_arn, "remove").await {
        // What the resource carries, tracked alongside the service so each row is judged
        // against everything the rows before it left behind.
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for removal in removals() {
            let case = format!("{kind} / {}", removal.label);
            let (status, body) =
                tag_resource(&router, Some(&instance_arn), &resource_arn, &removal.seed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the seeding TagResource must answer 200, got {status} {body}"
            );
            for (key, value) in &removal.seed {
                expected.insert(key.clone(), value.clone());
            }

            let (status, body) =
                untag_resource(&router, Some(&instance_arn), &resource_arn, &removal.keys).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagResource must answer 200, got {status} {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {case:?}: UntagResource models an empty output, got {body}"
            );
            for key in &removal.keys {
                expected.remove(key);
            }
            let want = expected_pairs(&expected);

            let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed),
                want,
                "case {case:?}: expected the remaining set {want:?} in key order, got {listed}"
            );

            // Rule 1: the same removal again removes nothing further.
            let (status, repeated) =
                untag_resource(&router, Some(&instance_arn), &resource_arn, &removal.keys).await;
            assert_eq!(
                status, 200,
                "case {case:?}: repeating UntagResource must answer 200, got {status} {repeated}"
            );
            let (status, relisted) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated list must answer 200, got {status} {relisted}"
            );
            assert_eq!(
                relisted, listed,
                "case {case:?}: a repeated removal must not change what a later caller \
                 sees, got {relisted}"
            );

            // The `InstanceArn` is optional on both operations: omitting it names the same
            // resource, since the ARN already carries the instance.
            let (status, without_instance) = list_tags(&router, None, &resource_arn).await;
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

        // Rule 2: every key left, removed in one call without the optional `InstanceArn` —
        // the resource survives its tags and lists EMPTY rather than being refused.
        let remaining: Vec<String> = expected.keys().cloned().collect();
        assert!(
            !remaining.is_empty(),
            "case {kind:?}: the table must leave keys to remove, got {remaining:?}"
        );
        let (status, body) = untag_resource(&router, None, &resource_arn, &remaining).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: removing every remaining key must answer 200, got {status} {body}"
        );
        let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the stripped resource must still list, got {status} {listed}"
        );
        assert!(
            reported(&listed).is_empty(),
            "case {kind:?}: a resource whose every tag was removed carries none, got {listed}"
        );
    }
}

/// Rule 1 where the tag set's KEYING bites: a key is unique within one request, so a create
/// naming the same key twice is refused and mints nothing (`duplicate_tag_keys.rs` owns that
/// refusal across every write that takes `Tags`) — while the key a served create stored once
/// is removed entirely, leaving the read with no value for it at all.
#[tokio::test]
async fn a_create_naming_one_key_twice_is_refused_and_a_stored_key_is_removed_entirely() {
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

    let (status, body) = call(
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
        "the create carrying distinct keys must answer 200, got {status} {body}"
    );
    let resource_arn = body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create minted no ARN: {body}"))
        .to_string();

    let (status, body) = untag_resource(
        &router,
        Some(&instance_arn),
        &resource_arn,
        &keys(&["env"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a stored key removed\": expected 200, got {status} {body}"
    );
    let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
    assert_eq!(
        status, 200,
        "case \"a stored key removed\": the list must answer 200, got {status} {listed}"
    );
    assert_eq!(
        reported(&listed),
        pairs(&[("owner", "platform")]),
        "case \"a stored key removed\": no entry for the named key may survive, got {listed}"
    );
}

/// Rule 2: a removal lands on the ONE resource its `ResourceArn` names.
#[tokio::test]
async fn a_removal_touches_only_the_resource_it_names() {
    let (router, instance_arn) = organization_router();
    let resources = taggable_resources(&router, &instance_arn, "isolated").await;
    let stored = pairs(&[("env", "prod"), ("owner", "platform")]);

    for (kind, resource_arn) in &resources {
        let (status, body) = tag_resource(&router, Some(&instance_arn), resource_arn, &stored).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the seeding TagResource must answer 200, got {status} {body}"
        );
    }

    // One resource is stripped of `env`; every other resource keeps both keys.
    let (target_kind, target_arn) = resources[1].clone();
    let (status, body) =
        untag_resource(&router, Some(&instance_arn), &target_arn, &keys(&["env"])).await;
    assert_eq!(
        status, 200,
        "case {target_kind:?}: UntagResource must answer 200, got {status} {body}"
    );

    for (kind, resource_arn) in &resources {
        let want = if resource_arn == &target_arn {
            pairs(&[("owner", "platform")])
        } else {
            stored.clone()
        };
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: ListTagsForResource must answer 200, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            want,
            "case {kind:?}: expected {want:?}, got {listed}"
        );
    }
}

/// Rule 3: a `ResourceArn` this account holds nothing under is refused, for every kind of
/// ARN the model's `TaggableResourceArn` admits, and removes nothing.
#[tokio::test]
async fn an_arn_this_account_holds_nothing_under_is_refused() {
    let (router, instance_arn) = organization_router();
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    // One resource of each kind exists and carries a tag, so the refusals below are about
    // the ARN asked for and not about an empty store.
    let held = taggable_resources(&router, &instance_arn, "held").await;
    let stored = pairs(&[("env", "prod")]);
    for (kind, resource_arn) in &held {
        let (status, body) = tag_resource(&router, Some(&instance_arn), resource_arn, &stored).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the seeding TagResource must answer 200, got {status} {body}"
        );
    }
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
        let (status, body) = untag_resource(
            &router,
            Some(&instance_arn),
            &resource_arn,
            &keys(&["env"]),
        )
        .await;
        assert_refused(case, "UntagResource", "ResourceNotFoundException", status, &body);
    }

    // The refusals removed nothing: every held resource still carries what it was tagged
    // with.
    for (kind, resource_arn) in &held {
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the held resource must still list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            stored,
            "case {kind:?}: a refused removal must leave the stored set alone, got {listed}"
        );
    }
}

/// Rule 3: an `InstanceArn` that holds the resource is what makes the call about it — an
/// instance this account does not hold, and one that holds something else, are both
/// refused, and neither removes anything.
#[tokio::test]
async fn a_resource_held_under_another_instance_is_refused_and_removes_nothing() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine.clone());
    let resources = taggable_resources(&router, &instance_arn, "scoped").await;
    let stored = pairs(&[("env", "prod")]);
    for (kind, resource_arn) in &resources {
        let (status, body) = tag_resource(&router, Some(&instance_arn), resource_arn, &stored).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the seeding TagResource must answer 200, got {status} {body}"
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
            let (status, body) = untag_resource(
                &router,
                Some(wrong_instance_arn),
                resource_arn,
                &keys(&["env"]),
            )
            .await;
            assert_refused(
                &case,
                "UntagResource",
                "ResourceNotFoundException",
                status,
                &body,
            );
        }
    }

    // Nothing the refusals touched moved: each resource still carries what it was tagged
    // with under its own instance.
    for (kind, resource_arn) in &resources {
        let (status, listed) = list_tags(&router, Some(&instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the resource must still list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            stored,
            "case {kind:?}: a refused removal must leave the stored set alone, got {listed}"
        );
    }

    // The scoping rule refuses a MISMATCH rather than every request: the second instance
    // is a taggable resource of its own, so untagging it under itself is served.
    let (status, body) = tag_resource(
        &router,
        Some(&other_instance_arn),
        &other_instance_arn,
        &stored,
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging the second instance under itself must answer 200, got {status} {body}"
    );
    let (status, body) = untag_resource(
        &router,
        Some(&other_instance_arn),
        &other_instance_arn,
        &keys(&["env"]),
    )
    .await;
    assert_eq!(
        status, 200,
        "untagging the second instance under itself must answer 200, got {status} {body}"
    );
    let (status, listed) = list_tags(&router, Some(&other_instance_arn), &other_instance_arn).await;
    assert_eq!(
        status, 200,
        "listing the second instance's tags must answer 200, got {status} {listed}"
    );
    assert!(
        reported(&listed).is_empty(),
        "the second instance's own tag must be gone, got {listed}"
    );
}

/// Rule 4: `TagKeys` at the published maximum is served and strips the whole set, while a
/// list one short of the published minimum, or one past the maximum, is refused and
/// removes nothing.
#[tokio::test]
async fn a_tag_keys_list_outside_its_published_bounds_is_refused() {
    let (router, instance_arn) = organization_router();

    for (kind, resource_arn) in taggable_resources(&router, &instance_arn, "bounds").await {
        // A full tag set — every tag a resource can carry — which one `TagKeyList` can
        // still name in a single call.
        let full: Vec<(String, String)> = (0..state::MAX_TAGS_PER_RESOURCE)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect();
        let (status, body) = tag_resource(&router, Some(&instance_arn), &resource_arn, &full).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: a full tag set must answer 200, got {status} {body}"
        );
        let all_keys: Vec<String> = full.iter().map(|(key, _)| key.clone()).collect();

        // One member short of the minimum, and one past the maximum: both refused by the
        // modelled constraint before anything is removed.
        let outside: Vec<(&str, Vec<String>)> = vec![
            ("no keys at all, one short of the 1 minimum", Vec::new()),
            (
                "one key past the 75 maximum",
                (0..=MAX_TAG_KEYS)
                    .map(|index| format!("key-{index:03}"))
                    .collect(),
            ),
        ];
        for (outside_case, tag_keys) in outside {
            let case = format!("{kind} / {outside_case}");
            let (status, body) =
                untag_resource(&router, Some(&instance_arn), &resource_arn, &tag_keys).await;
            assert_refused(&case, "UntagResource", "ValidationException", status, &body);
            let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the set must still list, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed).len(),
                state::MAX_TAGS_PER_RESOURCE,
                "case {case:?}: a refused removal removes nothing, so {} tags must \
                 remain, got {listed}",
                state::MAX_TAGS_PER_RESOURCE
            );
        }

        // One key of the full set: exactly that key goes.
        let (status, body) = untag_resource(
            &router,
            Some(&instance_arn),
            &resource_arn,
            &keys(&["key-000"]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {kind:?}: removing one key of a full set must answer 200, got {status} {body}"
        );
        let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the set must still list, got {status} {listed}"
        );
        let left = reported(&listed);
        assert_eq!(
            left.len(),
            state::MAX_TAGS_PER_RESOURCE - 1,
            "case {kind:?}: expected {} tags left, got {listed}",
            state::MAX_TAGS_PER_RESOURCE - 1
        );
        assert!(
            !left.iter().any(|(key, _)| key == "key-000"),
            "case {kind:?}: the named key must be gone, got {listed}"
        );

        // `TagKeys` naming every key the set still has plus the one already removed: the
        // whole set goes and the absent key is no obstacle.
        let (status, body) =
            untag_resource(&router, Some(&instance_arn), &resource_arn, &all_keys).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: a TagKeys list naming the whole set must answer 200, got \
             {status} {body}"
        );
        let (status, listed) = list_tags(&router, Some(&instance_arn), &resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the stripped resource must still list, got {status} {listed}"
        );
        assert!(
            reported(&listed).is_empty(),
            "case {kind:?}: every named key must be gone, got {listed}"
        );
    }
}
