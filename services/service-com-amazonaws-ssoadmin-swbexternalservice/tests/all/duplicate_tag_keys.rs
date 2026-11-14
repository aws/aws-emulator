// SPDX-License-Identifier: CC0-1.0

//! A REPEATED tag key in one request, end to end over the `awsJson1_1` wire.
//!
//! THE RULE: a tag set is a map keyed by `Tag$Key` — a resource carries one value per key —
//! so the tag list a request carries names each key AT MOST ONCE, and a request repeating a
//! key is refused with the modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="operation",operation="TagResource,CreatePermissionSet")` lists it)
//! carrying `state::duplicate_tag_key_message` for the key it repeated, before anything is
//! written: a create mints no resource and a tagging write leaves the set the resource
//! already carries exactly as it was. Keys are compared EXACTLY, because AWS tag keys are
//! case-sensitive, so two keys differing only in case are two keys and the write is served.
//!
//! The model cannot express it: `TagList`
//! (`smithy_query(query="shape",shape="Tag,TagList")`) is `length 0..75` of `Tag` with no
//! `@uniqueItems`, so every row below passes the boundary validator and reaches the handler.
//!
//! The rows span what the rule generalises over: WHERE the repeat stands (adjacent to its
//! first entry, separated from it, at the end of a longer list), HOW MANY times the key is
//! named, whether the repeated entries carry the same value or different ones, the character
//! class and the published length bound of the key that is repeated, and the lists that are
//! NOT repeats (distinct keys, keys sharing a prefix, keys differing only in case, one key,
//! none at all). Every operation that takes caller-supplied `Tags` is driven: the four
//! creates and `TagResource`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes; the harness is the one `ssoadmin_permission_sets.rs` owns.
//! `system_tag_mutations.rs` owns the other question the same shared validator asks (the
//! reserved `aws:` namespace), and `resource_tags.rs` the merge a served write performs.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, empty_account_router, organization_router, router, seeded_instance_arn,
};

/// The provider a customer managed application is created under: the `custom` entry of the
/// published catalog (`state::BUILT_IN_APPLICATION_PROVIDERS`), the one `CreateApplication`
/// serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// `TagKey`'s published upper length bound (`length(min: 1, max: 128)`).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// One row: the tag list a request carries, and the key it repeats — `None` for a list that
/// repeats none.
struct TagListCase {
    label: String,
    tags: Vec<(String, String)>,
    repeated: Option<String>,
}

/// Owned `(Key, Value)` pairs from literals.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The `Tags` member of a request, as the wire carries it.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The rows the rule is asserted over.
fn tag_list_cases() -> Vec<TagListCase> {
    let long_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let unicode_key = "Clé de voûte";
    vec![
        TagListCase {
            label: "the same key twice, adjacent".to_string(),
            tags: pairs(&[("env", "first"), ("env", "second")]),
            repeated: Some("env".to_string()),
        },
        TagListCase {
            label: "the same key twice, separated by another key".to_string(),
            tags: pairs(&[("env", "first"), ("owner", "platform"), ("env", "second")]),
            repeated: Some("env".to_string()),
        },
        TagListCase {
            label: "the same key three times".to_string(),
            tags: pairs(&[("env", "a"), ("env", "b"), ("env", "c")]),
            repeated: Some("env".to_string()),
        },
        TagListCase {
            label: "the same key AND the same value twice".to_string(),
            tags: pairs(&[("env", "prod"), ("env", "prod")]),
            repeated: Some("env".to_string()),
        },
        TagListCase {
            label: "the repeat at the end of a longer list".to_string(),
            tags: pairs(&[
                ("a", "1"),
                ("b", "2"),
                ("c", "3"),
                ("d", "4"),
                ("b", "5"),
            ]),
            repeated: Some("b".to_string()),
        },
        TagListCase {
            label: "a key outside ASCII, repeated".to_string(),
            tags: vec![
                (unicode_key.to_string(), "une".to_string()),
                ("owner".to_string(), "platform".to_string()),
                (unicode_key.to_string(), "deux".to_string()),
            ],
            repeated: Some(unicode_key.to_string()),
        },
        TagListCase {
            label: "a key at TagKey's 128-character bound, repeated".to_string(),
            tags: vec![
                (long_key.clone(), "first".to_string()),
                (long_key.clone(), "second".to_string()),
            ],
            repeated: Some(long_key),
        },
        TagListCase {
            label: "two keys differing only in case, which are two keys".to_string(),
            tags: pairs(&[("env", "lower"), ("Env", "upper")]),
            repeated: None,
        },
        TagListCase {
            label: "keys sharing a prefix".to_string(),
            tags: pairs(&[("cost", "x"), ("cost.centre", "y"), ("cost.centre.sub", "z")]),
            repeated: None,
        },
        TagListCase {
            label: "distinct keys carrying the same value".to_string(),
            tags: pairs(&[("env", "prod"), ("stage", "prod")]),
            repeated: None,
        },
        TagListCase {
            label: "one key".to_string(),
            tags: pairs(&[("env", "prod")]),
            repeated: None,
        },
        TagListCase {
            label: "an empty list, which the TagList lower bound of 0 admits".to_string(),
            tags: Vec::new(),
            repeated: None,
        },
    ]
}

/// The `Message` an error body carries, whichever member the renderer put it in.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert `body` is the repeated-key refusal, naming the key the request repeated.
fn assert_repeat_refusal(case: &str, what: &str, repeated: &str, status: u16, body: &Value) {
    assert_refused(case, what, "ValidationException", status, body);
    let message = message_of(body);
    assert!(
        message.contains(repeated),
        "case {case:?}: {what} must name the key it refused, got {message:?}"
    );
    assert_ne!(
        message,
        state::SYSTEM_TAG_MUTATION_MESSAGE,
        "case {case:?}: {what} must not answer the reserved-namespace refusal, got {message:?}"
    );
    assert_eq!(
        message,
        state::duplicate_tag_key_message(repeated),
        "case {case:?}: {what} must refuse through the one shared wording, got {message:?}"
    );
}

/// The `(Key, Value)` pairs `ListTagsForResource` reports for `resource_arn`.
async fn tags_of(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"ResourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200 for {resource_arn}, got {status} {body}"
    );
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

/// THE RULE on `TagResource`: a request repeating a key is refused and writes nothing,
/// while a list of distinct keys is served and read back.
#[tokio::test]
async fn tag_resource_refuses_a_repeated_tag_key_and_writes_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    // One tag already on the resource, so a refused write can be shown to have left a real
    // tag set exactly as it was.
    let (status, seeded) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": instance_arn,
            "Tags": [{"Key": "Owner", "Value": "platform"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the seeding TagResource expected 200, got {status} {seeded}"
    );

    for row in tag_list_cases() {
        let case = row.label.clone();
        let before = tags_of(&router, &case, &instance_arn).await;
        let (status, answer) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": instance_arn, "Tags": tag_list(&row.tags)}),
        )
        .await;

        match &row.repeated {
            Some(repeated) => {
                assert_repeat_refusal(&case, "TagResource", repeated, status, &answer);
                let after = tags_of(&router, &case, &instance_arn).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused tag write must store nothing, got {after:?} vs \
                     {before:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: TagResource expected 200, got {status} {answer}"
                );
                // The ROUND TRIP: every key the write named is readable back with its value.
                let after = tags_of(&router, &case, &instance_arn).await;
                for (key, value) in &row.tags {
                    assert!(
                        after.contains(&(key.clone(), value.clone())),
                        "case {case:?}: the stored set must carry ({key:?}, {value:?}), got \
                         {after:?}"
                    );
                }
            }
        }
    }
}

/// THE SAME RULE on every CREATE that takes caller-supplied `Tags`: the create is refused
/// and mints NO resource, so a repeated key cannot be attached at creation time either.
#[tokio::test]
async fn every_create_that_carries_tags_refuses_a_repeated_tag_key() {
    for row in tag_list_cases() {
        let Some(repeated) = row.repeated.clone() else {
            continue;
        };
        let tags = tag_list(&row.tags);

        // The three creates that hold a resource under an instance, on an instance that
        // serves all of them.
        let (router, instance_arn) = organization_router();
        let creates: Vec<(&str, Value, &str, &str)> = vec![
            (
                "CreatePermissionSet",
                json!({"InstanceArn": instance_arn, "Name": "Refused", "Tags": tags}),
                "ListPermissionSets",
                "PermissionSets",
            ),
            (
                "CreateApplication",
                json!({
                    "InstanceArn": instance_arn,
                    "ApplicationProviderArn": PROVIDER_CUSTOM,
                    "Name": "refused-application",
                    "Tags": tags,
                }),
                "ListApplications",
                "Applications",
            ),
            (
                "CreateTrustedTokenIssuer",
                json!({
                    "InstanceArn": instance_arn,
                    "Name": "refused-issuer",
                    "TrustedTokenIssuerType": ISSUER_TYPE,
                    "TrustedTokenIssuerConfiguration": {
                        "OidcJwtConfiguration": {
                            "IssuerUrl": "https://issuer.example.com/oidc",
                            "ClaimAttributePath": "email",
                            "IdentityStoreAttributePath": "emails.primary.value",
                            "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                        }
                    },
                    "Tags": tags,
                }),
                "ListTrustedTokenIssuers",
                "TrustedTokenIssuers",
            ),
        ];
        for (operation, body, reader, member) in creates {
            let case = format!("{} / {operation}", row.label);
            let (status, refused) = call(&router, operation, &body).await;
            assert_repeat_refusal(&case, operation, &repeated, status, &refused);

            let (status, listed) =
                call(&router, reader, &json!({"InstanceArn": instance_arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {reader} expected 200, got {status} {listed}"
            );
            assert_eq!(
                listed[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: a refused create must mint no resource, got {listed}"
            );
        }

        // And the create of the INSTANCE itself, whose tags only a create can attach: an
        // account that holds none, so the create is the one the operation is documented for.
        let case = format!("{} / CreateInstance", row.label);
        let router = empty_account_router();
        let (status, refused) = call(
            &router,
            "CreateInstance",
            &json!({"Name": "RefusedInstance", "Tags": tags}),
        )
        .await;
        assert_repeat_refusal(&case, "CreateInstance", &repeated, status, &refused);
        let (status, listed) = call(&router, "ListInstances", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {listed}"
        );
        assert_eq!(
            listed["Instances"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused create must enable no instance, got {listed}"
        );
    }
}

/// The other side of the rule: a create whose keys are DISTINCT is served, and every pair it
/// carried is readable back off the resource it minted — including two keys that differ only
/// in case, which are two keys.
#[tokio::test]
async fn a_create_whose_keys_are_distinct_stores_every_pair() {
    let (router, instance_arn) = organization_router();

    for (index, row) in tag_list_cases()
        .into_iter()
        .filter(|row| row.repeated.is_none())
        .enumerate()
    {
        let case = row.label.clone();
        let (status, created) = call(
            &router,
            "CreatePermissionSet",
            &json!({
                "InstanceArn": instance_arn,
                "Name": format!("tagged{index}"),
                "Tags": tag_list(&row.tags),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePermissionSet expected 200, got {status} {created}"
        );
        let arn = created["PermissionSet"]["PermissionSetArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create minted no ARN: {created}"))
            .to_string();

        let stored = tags_of(&router, &case, &arn).await;
        assert_eq!(
            stored.len(),
            row.tags.len(),
            "case {case:?}: the stored set must hold one entry per key sent, got {stored:?}"
        );
        for (key, value) in &row.tags {
            assert!(
                stored.contains(&(key.clone(), value.clone())),
                "case {case:?}: the stored set must carry ({key:?}, {value:?}), got {stored:?}"
            );
        }
    }
}
