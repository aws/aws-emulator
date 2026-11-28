// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two things RAM reports differently from the bare
//! modelled structure: the BODY of a tag mutation, and the association records of a
//! TEARDOWN.
//!
//! * **A tag mutation answers with nothing and is read back from the share.**
//!   `TagResourceResponse` and `UntagResourceResponse` declare no members
//!   (`smithy_query(query="shape",shape="TagResourceResponse")`), so a successful
//!   mutation answers with an empty body; what it changed is the share's own `tags`,
//!   whose entries carry exactly the members `Tag` declares
//!   (`smithy_query(query="shape",shape="Tag")` declares exactly `key` and `value`) and
//!   no structure discriminator. The rows below span the COUNT and the SHAPE of the
//!   mutated set — the dimension the rule generalises over — which the per-pair table in
//!   `response_payload_fidelity.rs` does not.
//! * **A teardown report carries `external = false`.**
//!   `DisassociateResourceShare` "removes the specified principals, resources, or
//!   source constraints from participating in the specified resource share" and answers
//!   with the records it removed; the call leaves no principal participating in the
//!   share, so there is no principal to place in the owning account's organization and
//!   every record it reports carries false — whatever the entity, however many entities
//!   and association types one call names, and on an idempotent repeat of the same
//!   call. The STORED derivation is untouched: the read after the teardown reports the
//!   value derived from the entity and the share's owner again.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The member name an AWS JSON structure discriminator would travel under: a tag inside
/// a share's own state carries none.
const DISCRIMINATOR: &str = "__type";

// ---------------------------------------------------------------------------
// A tag mutation answers with nothing; the share's own tags are plain pairs
// ---------------------------------------------------------------------------

/// A row of the tag-mutation table: how many pairs a single mutation changes, and
/// which of their keys the share already carried.
struct DocumentCase {
    case: &'static str,
    /// The pairs the share is created with.
    initial: Vec<(&'static str, &'static str)>,
    /// The pairs one `TagResource` call attaches.
    add: Vec<(&'static str, &'static str)>,
    /// The keys one `UntagResource` call detaches.
    remove: Vec<&'static str>,
}

/// The entries of a reported tag list.
fn entries(label: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: response carried no {member}, got {body:?}"))
}

/// Assert `entry` carries exactly the members `Tag` declares, and answer its key.
fn plain_key(label: &str, member: &str, entry: &Value) -> String {
    let object = entry
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: {member} entry is not an object: {entry:?}"));
    let declared: BTreeSet<&str> = ["key", "value"].into_iter().collect();
    let reported: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    assert!(
        reported.is_subset(&declared),
        "case {label:?}: a {member} entry carries a member the model does not declare: \
         {entry:?}"
    );
    object
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label:?}: a {member} entry carries no key: {entry:?}"))
        .to_string()
}

#[tokio::test]
async fn a_tag_mutation_answers_empty_and_the_share_reports_plain_pairs() {
    let router = emulator();

    // The rule generalises over the mutated SET, so the rows differ along its size
    // and along what the mutation met: one pair on an untagged share, one pair that
    // overwrites a key the share already carried, several pairs at once, a removal that
    // takes every key off, and a removal naming a key the share never carried (which
    // changes nothing).
    let cases = vec![
        DocumentCase {
            case: "one new pair",
            initial: vec![],
            add: vec![("cost-center", "42")],
            remove: vec!["cost-center"],
        },
        DocumentCase {
            case: "one pair overwriting a carried key",
            initial: vec![("team", "platform")],
            add: vec![("team", "network")],
            remove: vec!["team"],
        },
        DocumentCase {
            case: "four pairs in one call",
            initial: vec![("env", "test")],
            add: vec![("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
            remove: vec!["a", "b", "c", "d"],
        },
        DocumentCase {
            case: "a removal that empties the tag set",
            initial: vec![("env", "test"), ("team", "platform")],
            add: vec![("owner", "")],
            remove: vec!["env", "owner", "team"],
        },
        DocumentCase {
            case: "a removal naming only a key the share never carried",
            initial: vec![("env", "test")],
            add: vec![("region-hint", "us-west-2")],
            remove: vec!["absent"],
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let DocumentCase {
            case,
            initial,
            add,
            remove,
        } = row;

        let initial_tags: Vec<Value> = initial
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let created = create_ok(
            &router,
            case,
            &json!({ "name": format!("documents-{index}"), "tags": initial_tags }),
        )
        .await;
        let arn = arn_of(case, &created);

        let tags: Vec<Value> = add
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let (status, tagged) =
            tag_resource(&router, &json!({ "resourceShareArn": arn, "tags": tags })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} with {tagged:?}"
        );
        assert_eq!(
            tagged,
            json!({}),
            "case {case:?}: TagResource declares no response members, so the body must be \
             empty, got {tagged:?}"
        );

        // What it attached is the share's own tag set — the round trip through the
        // implemented read.
        let (status, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShares expected 200, got {status} with {listed:?}"
        );
        let share = only_share(case, &listed);
        let share_entries = entries(case, &share, "tags");
        for (key, value) in &add {
            assert!(
                share_entries.contains(&json!({ "key": key, "value": value })),
                "case {case:?}: the pair {key}={value} TagResource attached is not the object \
                 the share carries: {share_entries:?}"
            );
        }
        // A tag reported as part of the share's own state carries the modelled members
        // alone, with no structure discriminator.
        for entry in &share_entries {
            plain_key(case, "tags", entry);
            assert!(
                entry.get(DISCRIMINATOR).is_none(),
                "case {case:?}: a tag inside the share must carry no {DISCRIMINATOR:?}, \
                 got {entry:?}"
            );
        }

        let carried_before: BTreeSet<String> = share_entries
            .iter()
            .map(|entry| plain_key(case, "tags", entry))
            .collect();
        let expected_left: BTreeSet<String> = carried_before
            .iter()
            .filter(|key| !remove.contains(&key.as_str()))
            .cloned()
            .collect();

        let (status, untagged) = untag_resource(
            &router,
            &json!({ "resourceShareArn": arn, "tagKeys": remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} with {untagged:?}"
        );
        assert_eq!(
            untagged,
            json!({}),
            "case {case:?}: UntagResource declares no response members, so the body must be \
             empty, got {untagged:?}"
        );

        let (status, after) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read after the removal expected 200, got {status} with {after:?}"
        );
        let left: BTreeSet<String> = entries(case, &only_share(case, &after), "tags")
            .iter()
            .map(|entry| plain_key(case, "tags", entry))
            .collect();
        assert_eq!(
            left, expected_left,
            "case {case:?}: expected the keys the removal left behind, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A teardown report carries `external = false`
// ---------------------------------------------------------------------------

/// A row of the teardown table: the entities one `AssociateResourceShare` /
/// `DisassociateResourceShare` pair names, each with the value the stored derivation
/// gives `external` for a share owned by [`ACCOUNT`].
struct TeardownCase {
    case: &'static str,
    /// `(association type, entity, the stored, derived flag)`.
    entities: Vec<(&'static str, &'static str, bool)>,
}

/// A request body naming every entity of a row under the list its type is taken from.
fn entity_request(entities: &[(&str, &str, bool)], arn: &str, client_token: Option<&str>) -> Value {
    let mut body = json!({ "resourceShareArn": arn });
    for member in ["resourceArns", "principals", "sources"] {
        let association_type = match member {
            "resourceArns" => "RESOURCE",
            "sources" => "SOURCE",
            _ => "PRINCIPAL",
        };
        let named: Vec<&str> = entities
            .iter()
            .filter(|(kind, _, _)| *kind == association_type)
            .map(|(_, entity, _)| *entity)
            .collect();
        if !named.is_empty() {
            body[member] = json!(named);
        }
    }
    if let Some(token) = client_token {
        body["clientToken"] = json!(token);
    }
    body
}

/// The `external` member of the one association a response reports for `entity`.
fn external_for(label: &str, body: &Value, entity: &str) -> bool {
    let association = associations_of(label, body)
        .into_iter()
        .find(|association| {
            association.get("associatedEntity").and_then(Value::as_str) == Some(entity)
        })
        .unwrap_or_else(|| {
            panic!("case {label:?}: no association reported for {entity}: {body:?}")
        });
    association
        .get("external")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            panic!("case {label:?}: association reported no external: {association:?}")
        })
}

#[tokio::test]
async fn every_record_a_teardown_reports_carries_external_false() {
    let router = emulator();

    // The rows span the dimensions the rule generalises over: the entity form, the
    // association type, and HOW MANY entities (and types) one call names — including a
    // row whose entities derive BOTH values, so a report that merely echoed the stored
    // flag would fail on it.
    let cases = vec![
        TeardownCase {
            case: "one in-organization principal",
            entities: vec![("PRINCIPAL", ACCOUNT, false)],
        },
        TeardownCase {
            case: "one out-of-organization principal",
            entities: vec![("PRINCIPAL", "111122223333", true)],
        },
        TeardownCase {
            case: "a service principal",
            entities: vec![("PRINCIPAL", "lambda.amazonaws.com", true)],
        },
        TeardownCase {
            case: "an organization id and an organizational unit",
            entities: vec![
                ("PRINCIPAL", "o-abc123exam", false),
                ("PRINCIPAL", "ou-ab12-cd34efgh", false),
            ],
        },
        TeardownCase {
            case: "principals deriving both values at once",
            entities: vec![
                ("PRINCIPAL", ACCOUNT, false),
                ("PRINCIPAL", "111122223333", true),
                ("PRINCIPAL", "arn:aws:iam::777788889999:role/consumer", true),
            ],
        },
        TeardownCase {
            case: "all three association types in one call",
            entities: vec![
                ("RESOURCE", LOCAL_RESOURCE, false),
                ("RESOURCE", FOREIGN_RESOURCE, true),
                ("PRINCIPAL", "111122223333", true),
                ("SOURCE", ACCOUNT, false),
            ],
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let TeardownCase { case, entities } = row;

        let created = create_ok(
            &router,
            case,
            &json!({ "name": format!("teardown-{index}") }),
        )
        .await;
        let arn = arn_of(case, &created);
        let request = entity_request(&entities, &arn, None);

        let (status, associated) = associate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateResourceShare expected 200, got {status} with {associated:?}"
        );
        for (_, entity, external) in &entities {
            let on_associate = external_for(case, &associated, entity);
            assert_eq!(
                on_associate, *external,
                "case {case:?}: AssociateResourceShare expected external {external}, \
                 got {on_associate} for {entity}"
            );
        }

        // The teardown: every record it reports carries false, including the ones whose
        // stored flag is true.
        let token = format!("teardown-token-{index}");
        let teardown = entity_request(&entities, &arn, Some(&token));
        let (status, disassociated) = disassociate(&router, &teardown).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateResourceShare expected 200, got {status} \
             with {disassociated:?}"
        );
        for (_, entity, _) in &entities {
            let reported = external_for(case, &disassociated, entity);
            assert!(
                !reported,
                "case {case:?}: the teardown report expected external false, \
                 got {reported} for {entity}"
            );
        }

        // An idempotent repeat answers from the stored records and reports the same.
        let (status, repeated) = disassociate(&router, &teardown).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated DisassociateResourceShare expected 200, \
             got {status} with {repeated:?}"
        );
        for (_, entity, _) in &entities {
            let reported = external_for(case, &repeated, entity);
            assert!(
                !reported,
                "case {case:?}: the repeated teardown report expected external false, \
                 got {reported} for {entity}"
            );
        }

        // The stored derivation is untouched: the read after the teardown reports the
        // value derived from the entity and the share's owner again.
        for association_type in ["RESOURCE", "PRINCIPAL", "SOURCE"] {
            let named: Vec<&(&str, &str, bool)> = entities
                .iter()
                .filter(|(kind, _, _)| *kind == association_type)
                .collect();
            if named.is_empty() {
                continue;
            }
            let (status, read) = get_associations(
                &router,
                &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetResourceShareAssociations expected 200, got {status} \
                 with {read:?}"
            );
            for (_, entity, external) in named {
                let on_read = external_for(case, &read, entity);
                assert_eq!(
                    on_read, *external,
                    "case {case:?}: the read after the teardown expected external \
                     {external}, got {on_read} for {entity}"
                );
            }
        }
    }
}
