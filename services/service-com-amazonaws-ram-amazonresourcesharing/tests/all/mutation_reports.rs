// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a MUTATION of a resource share reports, and for the
//! half of the association lifecycle that takes a record back OUT of the share.
//!
//! The rules under test, each asserted over a table of inputs rather than one case:
//!
//! * a tag mutation changes the stored tag set and nothing else: `TagResource` leaves the
//!   pair now in effect for every distinct key the request named, `UntagResource` drops
//!   exactly the keys the share carried and the request named, and both answer with an
//!   empty body, because `TagResourceResponse` and `UntagResourceResponse` declare no
//!   members — what changed is read back through `GetResourceShares`;
//! * `DisassociateResourceShare` takes an association to the terminal `DISASSOCIATED`
//!   status and the record SURVIVES there, so a read still reports the association,
//!   a status-filtered read selects it and a caller polling for the teardown sees it
//!   complete;
//! * a teardown that has access to revoke (the association had reached `ASSOCIATED`)
//!   is answered in the non-terminal `DISASSOCIATING` status and reaches
//!   `DISASSOCIATED` when the clock makes that move due; one that has nothing to
//!   revoke (the attach had not completed) is terminal at once;
//! * a terminal association reports no `statusMessage`: whatever it was waiting for
//!   no longer applies;
//! * associating an entity whose association was torn down starts the attach again.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    ASSOCIATION_READY_AFTER_MILLIS, DISASSOCIATION_READY_AFTER_MILLIS, MAX_ENTITIES_PER_CALL,
};

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

// ---------------------------------------------------------------------------
// What a tag mutation reports
// ---------------------------------------------------------------------------

/// One mutation of a table row: what is applied, and what the call must report.
enum Mutation {
    /// `TagResource` with these pairs, in the order the request lists them.
    Add(Vec<(&'static str, &'static str)>),
    /// `UntagResource` with these keys, in the order the request lists them.
    Remove(Vec<&'static str>),
}

/// A row of the tag-mutation report table.
struct ReportCase {
    case: &'static str,
    /// The tags the share is created with.
    initial: Vec<(&'static str, &'static str)>,
    /// The mutations applied, in order.
    steps: Vec<Mutation>,
}

#[tokio::test]
async fn a_tag_mutation_changes_exactly_the_tag_set_it_names() {
    let router = emulator();

    // The rule generalises over what a mutation does to a tag set, so the rows differ
    // along exactly that: adding to an empty set, adding beside an existing key,
    // replacing a key's value, adding several at once, naming one key twice in one
    // call, an empty value, removing a key that is present, removing one that never
    // was, removing a mix of both, removing every key, and removing twice.
    let cases = vec![
        ReportCase {
            case: "add one tag to an untagged share",
            initial: vec![],
            steps: vec![Mutation::Add(vec![("cost-center", "42")])],
        },
        ReportCase {
            case: "add a tag beside an existing one",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Add(vec![("cost-center", "42")])],
        },
        ReportCase {
            case: "replace the value of a key the share carries",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Add(vec![("team", "network")])],
        },
        ReportCase {
            case: "re-attach exactly what the share already carries",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Add(vec![("team", "platform")])],
        },
        ReportCase {
            case: "add several tags in one call",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Add(vec![
                ("owner", ""),
                ("cost-center", "42"),
                ("env", "test"),
            ])],
        },
        ReportCase {
            case: "name the same key twice in one call",
            initial: vec![],
            steps: vec![Mutation::Add(vec![("env", "test"), ("env", "prod")])],
        },
        ReportCase {
            case: "remove one of two keys",
            initial: vec![("cost-center", "42"), ("team", "platform")],
            steps: vec![Mutation::Remove(vec!["team"])],
        },
        ReportCase {
            case: "remove a key the share never carried",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Remove(vec!["absent"])],
        },
        ReportCase {
            case: "remove a present and an absent key together",
            initial: vec![("team", "platform")],
            steps: vec![Mutation::Remove(vec!["absent", "team"])],
        },
        ReportCase {
            case: "remove every key",
            initial: vec![("cost-center", "42"), ("env", "test"), ("team", "platform")],
            steps: vec![Mutation::Remove(vec!["team", "cost-center", "env"])],
        },
        ReportCase {
            case: "remove the same key twice",
            initial: vec![("team", "platform")],
            steps: vec![
                Mutation::Remove(vec!["team"]),
                Mutation::Remove(vec!["team"]),
            ],
        },
        ReportCase {
            case: "add then remove the added key",
            initial: vec![("team", "platform")],
            steps: vec![
                Mutation::Add(vec![("cost-center", "42")]),
                Mutation::Remove(vec!["cost-center"]),
            ],
        },
    ];

    for ReportCase {
        case,
        initial,
        steps,
    } in cases
    {
        let initial_tags: Vec<Value> = initial
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let created = create_ok(
            &router,
            case,
            &json!({
                "name": format!("reported-{}", case.replace(' ', "-")),
                "tags": initial_tags,
            }),
        )
        .await;
        let arn = arn_of(case, &created);

        // The tag set as the test tracks it, so each step's expected report is derived
        // from the state the mutation meets rather than from a remembered body.
        let mut carried: BTreeMap<String, String> = initial
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        for (index, step) in steps.iter().enumerate() {
            let label = format!("{case}/step {index}");
            match step {
                Mutation::Add(pairs) => {
                    // A key named twice in one call attaches one pair — the last value
                    // wins — so the set the share ends up with carries it once.
                    let mut attached: BTreeMap<String, String> = BTreeMap::new();
                    for (key, value) in pairs {
                        attached.insert((*key).to_string(), (*value).to_string());
                    }

                    let tags: Vec<Value> = pairs
                        .iter()
                        .map(|(key, value)| json!({ "key": key, "value": value }))
                        .collect();
                    let (status, body) =
                        tag_resource(&router, &json!({ "resourceShareArn": arn, "tags": tags }))
                            .await;
                    assert_eq!(
                        status, 200,
                        "case {label:?}: TagResource expected 200, got {status} with {body:?}"
                    );
                    assert_eq!(
                        body,
                        json!({}),
                        "case {label:?}: TagResource declares no response members, so the body \
                         must be empty, got {body:?}"
                    );
                    carried.extend(attached);
                }
                Mutation::Remove(keys) => {
                    let (status, body) = untag_resource(
                        &router,
                        &json!({ "resourceShareArn": arn, "tagKeys": keys }),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {label:?}: UntagResource expected 200, got {status} with {body:?}"
                    );
                    assert_eq!(
                        body,
                        json!({}),
                        "case {label:?}: UntagResource declares no response members, so the \
                         body must be empty, got {body:?}"
                    );
                    for key in keys {
                        carried.remove(*key);
                    }
                }
            }
        }

        // The mutations are what the stored share now carries, read back through the
        // operation that reports a share's tag set.
        let (_, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        let expected_tags: Vec<(String, String)> = carried.into_iter().collect();
        assert_eq!(
            tags_of(case, &only_share(case, &listed)),
            expected_tags,
            "case {case:?}: the share must carry what the mutations reported, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The teardown half of the association lifecycle
// ---------------------------------------------------------------------------

/// A row of the association-teardown table: one entity to associate and then
/// disassociate.
struct TeardownCase {
    association_type: &'static str,
    entity: &'static str,
    /// True when RAM invites the principal, so a PENDING association of this row
    /// reports what it is waiting for. A terminal one reports nothing either way.
    waits_for_invitation: bool,
}

/// Every entity form the teardown rule is asserted over: the rows differ along the
/// association type and the form of the entity (a resource inside and outside the
/// owning account, an account id, a role ARN in another account, a service principal
/// name, an organizational unit, and a source), because the rule is about the
/// association's lifecycle rather than about what is associated.
fn teardown_cases() -> Vec<TeardownCase> {
    vec![
        TeardownCase {
            association_type: "RESOURCE",
            entity: LOCAL_RESOURCE,
            waits_for_invitation: false,
        },
        TeardownCase {
            association_type: "RESOURCE",
            entity: FOREIGN_RESOURCE,
            waits_for_invitation: false,
        },
        TeardownCase {
            association_type: "PRINCIPAL",
            entity: "123456789012",
            waits_for_invitation: true,
        },
        TeardownCase {
            association_type: "PRINCIPAL",
            entity: ACCOUNT,
            waits_for_invitation: true,
        },
        TeardownCase {
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::777788889999:role/consumer",
            waits_for_invitation: true,
        },
        TeardownCase {
            association_type: "PRINCIPAL",
            entity: "lambda.amazonaws.com",
            waits_for_invitation: true,
        },
        TeardownCase {
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::000000000000:ou/o-abc/ou-def",
            waits_for_invitation: true,
        },
        TeardownCase {
            association_type: "SOURCE",
            entity: "111122223333",
            waits_for_invitation: false,
        },
    ]
}

/// The entities of `wanted` association type, as an associate/disassociate request
/// lists them.
fn entities(cases: &[TeardownCase], wanted: &str) -> Vec<&'static str> {
    cases
        .iter()
        .filter(|row| row.association_type == wanted)
        .map(|row| row.entity)
        .collect()
}

/// A request body that associates (or disassociates) every row's entity at once.
fn all_entities(cases: &[TeardownCase], arn: &str) -> Value {
    json!({
        "resourceShareArn": arn,
        "resourceArns": entities(cases, "RESOURCE"),
        "principals": entities(cases, "PRINCIPAL"),
        "sources": entities(cases, "SOURCE"),
    })
}

/// Create a share with every row's entity associated, and answer its ARN.
async fn share_with_every_entity(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    cases: &[TeardownCase],
) -> String {
    let created = create_ok(
        router,
        case,
        &json!({
            "name": name,
            "resourceArns": entities(cases, "RESOURCE"),
            "principals": entities(cases, "PRINCIPAL"),
            "sources": entities(cases, "SOURCE"),
        }),
    )
    .await;
    arn_of(case, &created)
}

/// The association `GetResourceShareAssociations` reports for one row, read back with
/// the filters that row's association type admits, or `None` when no association is
/// reported for it.
async fn reported(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
    row: &TeardownCase,
    status_filter: Option<&str>,
) -> Option<Value> {
    let mut request = json!({
        "associationType": row.association_type,
        "resourceShareArns": [arn],
    });
    let map = request.as_object_mut().expect("a JSON object body");
    // Each association type takes its own entity filter: `principal` for a PRINCIPAL
    // association, `resourceArn` for a RESOURCE one, and neither for a SOURCE one.
    match row.association_type {
        "PRINCIPAL" => {
            map.insert("principal".to_string(), json!(row.entity));
        }
        "RESOURCE" => {
            map.insert("resourceArn".to_string(), json!(row.entity));
        }
        _ => {}
    }
    if let Some(status) = status_filter {
        map.insert("associationStatus".to_string(), json!(status));
    }
    let (status, body) = get_associations(router, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}/{}: the read expected 200, got {status} with {body:?}",
        row.entity
    );
    associations_of(case, &body)
        .into_iter()
        .find(|association| {
            association.get("associatedEntity").and_then(Value::as_str) == Some(row.entity)
        })
}

/// Assert one row's association is reported in `expected_status`, with a status
/// message only while it is still waiting for something.
async fn assert_reported_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
    row: &TeardownCase,
    expected_status: &str,
    terminal: bool,
) -> Value {
    let label = format!("{case}/{} {}", row.association_type, row.entity);
    let association = reported(router, case, arn, row, None)
        .await
        .unwrap_or_else(|| panic!("case {label:?}: the read reported no association at all"));
    assert_eq!(
        association.get("status").and_then(Value::as_str),
        Some(expected_status),
        "case {label:?}: expected status {expected_status}, got {association:?}"
    );
    let message = association.get("statusMessage").and_then(Value::as_str);
    if terminal {
        assert_eq!(
            message, None,
            "case {label:?}: a terminal association waits for nothing, so no statusMessage \
             belongs here, got {association:?}"
        );
    } else if row.waits_for_invitation && expected_status == "ASSOCIATING" {
        assert!(
            message.is_some_and(|message| message.contains("invitation")),
            "case {label:?}: a pending principal association must say it is waiting for the \
             invitation to be answered, got {association:?}"
        );
    }
    // A status-filtered read selects on exactly the status the record is in.
    assert!(
        reported(router, case, arn, row, Some(expected_status))
            .await
            .is_some(),
        "case {label:?}: a read filtered on {expected_status} must report this association"
    );
    for other in ["ASSOCIATING", "ASSOCIATED", "DISASSOCIATING", "DISASSOCIATED"] {
        if other == expected_status {
            continue;
        }
        assert!(
            reported(router, case, arn, row, Some(other))
                .await
                .is_none(),
            "case {label:?}: a read filtered on {other} must not report an association in \
             {expected_status}"
        );
    }
    association
}

#[tokio::test]
async fn tearing_down_an_attach_that_granted_nothing_is_terminal_at_once() {
    let router = emulator();
    let case = "teardown while pending";
    let cases = teardown_cases();
    let arn = share_with_every_entity(&router, case, "share-teardown-pending", &cases).await;

    // Straight after the create nothing has settled, so no entity has been granted
    // anything yet: the teardown has nothing to revoke.
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "ASSOCIATING", false).await;
    }

    let (status, body) = disassociate(&router, &all_entities(&cases, &arn)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the teardown expected 200, got {status} with {body:?}"
    );
    assert!(
        body.get("clientToken").and_then(Value::as_str).is_some(),
        "case {case:?}: a teardown reports the idempotency token it used, got {body:?}"
    );
    let reported_now = associations_of(case, &body);
    assert_eq!(
        reported_now.len(),
        cases.len(),
        "case {case:?}: the teardown must report one association per entity, got {body:?}"
    );
    for association in &reported_now {
        assert_eq!(
            association.get("status").and_then(Value::as_str),
            Some("DISASSOCIATED"),
            "case {case:?}: an attach that granted nothing is torn down at once, got \
             {association:?}"
        );
        assert_eq!(
            association.get("statusMessage"),
            None,
            "case {case:?}: a terminal association reports no statusMessage, got {association:?}"
        );
    }

    // The record survives the teardown, so the read still reports it — in the terminal
    // status, and with nothing it is waiting for.
    for row in &cases {
        let association =
            assert_reported_status(&router, case, &arn, row, "DISASSOCIATED", true).await;
        assert!(
            timestamp(case, &association, "lastUpdatedTime")
                >= timestamp(case, &association, "creationTime"),
            "case {case:?}/{}: the teardown moved lastUpdatedTime, got {association:?}",
            row.entity
        );
    }

    // Repeating the teardown is the same success and moves nothing: the record is
    // already where the first call left it.
    let before = reported(&router, case, &arn, &cases[0], None).await;
    let (repeat_status, repeat) = disassociate(&router, &all_entities(&cases, &arn)).await;
    assert_eq!(
        repeat_status, 200,
        "case {case:?}: a repeated teardown expected 200, got {repeat_status} with {repeat:?}"
    );
    assert_eq!(
        reported(&router, case, &arn, &cases[0], None).await,
        before,
        "case {case:?}: a repeated teardown must leave the record exactly as it was"
    );

    // Associating an entity whose association was torn down starts the attach again.
    let (again_status, again) = associate(&router, &all_entities(&cases, &arn)).await;
    assert_eq!(
        again_status, 200,
        "case {case:?}: re-associating expected 200, got {again_status} with {again:?}"
    );
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "ASSOCIATING", false).await;
    }
}

#[tokio::test]
async fn tearing_down_a_granted_association_settles_through_disassociating() {
    let router = emulator();
    let case = "teardown after settle";
    let cases = teardown_cases();
    let arn = share_with_every_entity(&router, case, "share-teardown-settled", &cases).await;

    // Nothing sleeps server-side; the owed move is materialised by the first call at
    // or after the instant it fell due.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(ASSOCIATION_READY_AFTER_MILLIS).unwrap_or_default() + 100,
    ));
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "ASSOCIATED", true).await;
    }

    let (status, body) = disassociate(&router, &all_entities(&cases, &arn)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the teardown expected 200, got {status} with {body:?}"
    );
    for association in associations_of(case, &body) {
        assert_eq!(
            association.get("status").and_then(Value::as_str),
            Some("DISASSOCIATING"),
            "case {case:?}: revoking access that was granted is owed, so the teardown is \
             answered in the non-terminal status, got {association:?}"
        );
    }

    // The owed span is never zero, so a caller reading straight back still observes
    // the transient status.
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "DISASSOCIATING", false).await;
    }

    std::thread::sleep(Duration::from_millis(
        u64::try_from(DISASSOCIATION_READY_AFTER_MILLIS).unwrap_or_default() + 100,
    ));
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "DISASSOCIATED", true).await;
    }
}

/// A row of the teardown-refusal table: a request that must be refused, and the
/// modelled error shape RAM words that refusal with.
struct RefusalCase {
    case: String,
    request: Value,
    expected: &'static str,
}

#[tokio::test]
async fn a_teardown_the_service_cannot_apply_is_refused_as_ram_words_it() {
    let router = emulator();
    let case = "teardown refusals";
    let cases = teardown_cases();
    let arn = share_with_every_entity(&router, case, "share-teardown-refusals", &cases).await;
    let unknown_arn =
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-0000-0000-000000000000");

    // The rows differ along WHICH part of the request the service cannot act on: the
    // share ARN's form, the share it names, the entity lists being empty, an entity
    // the share holds no association with, an entity value that cannot be stored, and
    // more entities than one call takes.
    let too_many: Vec<String> = (0..=MAX_ENTITIES_PER_CALL)
        .map(|index| format!("55556666{index:04}"))
        .collect();
    let refusals = vec![
        RefusalCase {
            case: "a resourceShareArn that is not a share ARN".to_string(),
            request: json!({
                "resourceShareArn": "arn:aws:ram:us-west-2:000000000000:permission/x",
                "principals": ["123456789012"],
            }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "a resourceArns entry that is not an ARN".to_string(),
            request: json!({ "resourceShareArn": arn, "resourceArns": ["not-an-arn"] }),
            expected: "MalformedArnException",
        },
        RefusalCase {
            case: "a share ARN no share has".to_string(),
            request: json!({ "resourceShareArn": unknown_arn, "principals": ["123456789012"] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "no entity to disassociate".to_string(),
            request: json!({ "resourceShareArn": arn }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "an empty principal".to_string(),
            request: json!({ "resourceShareArn": arn, "principals": [""] }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "a principal the share is not shared with".to_string(),
            request: json!({ "resourceShareArn": arn, "principals": ["999988887777"] }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: "a resource the share does not hold".to_string(),
            request: json!({
                "resourceShareArn": arn,
                "resourceArns": ["arn:aws:ec2:us-west-2:000000000000:subnet/subnet-9999"],
            }),
            expected: "UnknownResourceException",
        },
        RefusalCase {
            case: format!("more than the {MAX_ENTITIES_PER_CALL} principals one call takes"),
            request: json!({ "resourceShareArn": arn, "principals": too_many }),
            expected: "ResourceShareLimitExceededException",
        },
        RefusalCase {
            case: "a clientToken that is empty".to_string(),
            request: json!({
                "resourceShareArn": arn,
                "principals": ["123456789012"],
                "clientToken": "",
            }),
            expected: "InvalidClientTokenException",
        },
    ];

    for RefusalCase {
        case,
        request,
        expected,
    } in refusals
    {
        let (status, body) = disassociate(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal says why, got {body:?}"
        );
    }

    // Nothing above tore anything down: every association is where the create left it.
    for row in &cases {
        assert_reported_status(&router, case, &arn, row, "ASSOCIATING", false).await;
    }

    // One token, two different requests: the second is the idempotency mismatch, and
    // repeating the first is the first call's answer.
    let token = "teardown-token";
    let first = json!({
        "resourceShareArn": arn,
        "principals": ["123456789012"],
        "clientToken": token,
    });
    let (first_status, first_body) = disassociate(&router, &first).await;
    assert_eq!(
        first_status, 200,
        "case {case:?}: the first tokened teardown expected 200, got {first_body:?}"
    );
    let (repeat_status, repeat_body) = disassociate(&router, &first).await;
    assert_eq!(
        (repeat_status, &repeat_body),
        (first_status, &first_body),
        "case {case:?}: a repeat of a tokened teardown is the first call's answer"
    );
    let (mismatch_status, mismatch_body) = disassociate(
        &router,
        &json!({
            "resourceShareArn": arn,
            "principals": [ACCOUNT],
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        mismatch_status, 400,
        "case {case:?}: the same token with other parameters is refused, got {mismatch_body:?}"
    );
    assert_eq!(
        error_code(&mismatch_body),
        "IdempotentParameterMismatchException",
        "case {case:?}: expected the idempotency mismatch, got {mismatch_body:?}"
    );
}
