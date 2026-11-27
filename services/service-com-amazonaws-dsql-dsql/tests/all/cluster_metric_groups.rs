// SPDX-License-Identifier: CC0-1.0

//! The ADVANCED MONITORING a cluster collects, end to end over the wire.
//!
//! RULE: a cluster's enabled metric groups are a SET it holds, the create decides
//! and the update REPLACES: `CreateCluster` enables exactly the groups its
//! `metricGroups` names and none when the member is absent, `UpdateCluster` replaces
//! the whole set with the list it carries — so a group the new list omits stops being
//! collected and an empty list stops all of them, while an absent member changes
//! nothing — and every read of the cluster reports the set it holds, the empty list
//! included. Because the member that names them is `@uniqueItems`, a group is either
//! collected or not: the order a request lists them in carries nothing, and a list
//! naming a group twice, or naming a group the enum does not publish, is refused with
//! the modelled `ValidationException` (400) with the cluster left as it was.
//!
//! The tables below span what the rule generalises over: the number of groups a
//! request names (none, the member absent, one, every published group), the order it
//! names them in, and every kind of transition between two sets (onto groups, between
//! sets, back to none, and the request that asks for no change at all). The group
//! names themselves are not written out: they are read from the model's own value set
//! (`METRIC_GROUP_VALUES`, rendered from `smithy_query(query="shape",
//! shape="MetricGroup")`), so a model that publishes another group tests it too.
//!
//! Every call goes through the `Dsql` harness in `tests/all/harness.rs`, this crate's
//! one test client.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::METRIC_GROUP_VALUES;

use crate::harness::{wait_for_the_clock, Answer, Dsql};

/// A `metricGroups` naming `groups`, as `MetricGroupRequestList` carries it: one
/// `MetricGroupRequest` object per group.
fn requested(groups: &[&str]) -> Value {
    Value::Array(
        groups
            .iter()
            .map(|group| json!({"metricGroup": group}))
            .collect(),
    )
}

/// The groups a response REPORTS, or `None` when it carries no `metricGroups` member
/// at all — which the rule allows for no cluster, so the two are told apart.
fn reported(answer: &Answer) -> Option<Vec<String>> {
    Some(
        answer
            .get("metricGroups")
            .as_array()?
            .iter()
            .map(|group| {
                group
                    .get("metricGroup")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    )
}

/// Every published group, in the order the model's value set lists them — the whole
/// set a cluster can collect.
fn every_group() -> Vec<&'static str> {
    METRIC_GROUP_VALUES.to_vec()
}

/// Every published group, listed in REVERSE: the same set spelled the other way
/// round, which asks for the same configuration.
fn every_group_reversed() -> Vec<&'static str> {
    let mut groups = every_group();
    groups.reverse();
    groups
}

/// The groups a cluster reports through its own read, asserted to be a served read.
async fn read_back(dsql: &Dsql, case: &str, identifier: &str) -> Option<Vec<String>> {
    let fetched = dsql.get(identifier).await;
    assert_eq!(
        fetched.status, 200,
        "case {case:?}: GetCluster must succeed, got {} {:?}",
        fetched.status, fetched.body
    );
    reported(&fetched)
}

/// Create a cluster from `body`, asserted to be served, and answer the create's own
/// reply. Deletion protection is off so the cluster can be updated and deleted.
async fn create(dsql: &Dsql, case: &str, body: Value) -> Answer {
    let mut request = json!({"deletionProtectionEnabled": false});
    if let (Some(request), Some(body)) = (request.as_object_mut(), body.as_object()) {
        for (member, value) in body {
            request.insert(member.clone(), value.clone());
        }
    }
    let created = dsql.create(request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateCluster must succeed, got {} {:?}",
        created.status, created.body
    );
    created
}

/// RULE on the way IN: a create enables exactly the groups it names — the empty set
/// when it names none and when the member is absent — and both its own answer and
/// every later read of the cluster report that set.
#[tokio::test]
async fn a_create_enables_exactly_the_groups_it_names() {
    let dsql = Dsql::new();
    let mut cases: Vec<(String, Value, Vec<String>)> = vec![
        (
            "no metricGroups at all".to_string(),
            json!({}),
            Vec::new(),
        ),
        (
            "an empty metricGroups".to_string(),
            json!({"metricGroups": []}),
            Vec::new(),
        ),
        (
            "every published group".to_string(),
            json!({"metricGroups": requested(&every_group())}),
            every_group().iter().map(|g| (*g).to_string()).collect(),
        ),
        (
            "every published group, listed in reverse".to_string(),
            json!({"metricGroups": requested(&every_group_reversed())}),
            every_group().iter().map(|g| (*g).to_string()).collect(),
        ),
    ];
    // One row per published group on its own, so a model that publishes more than one
    // is driven on each of them rather than only on the whole set.
    for group in every_group() {
        cases.push((
            format!("the one group {group}"),
            json!({"metricGroups": requested(&[group])}),
            vec![group.to_string()],
        ));
    }

    for (case, body, mut want) in cases {
        let created = create(&dsql, &case, body).await;
        want.sort();
        let answered = reported(&created);
        assert_eq!(
            answered.clone().map(|mut groups| {
                groups.sort();
                groups
            }),
            Some(want.clone()),
            "case {case:?}: the create must report the groups {want:?}, got {answered:?} in {:?}",
            created.body
        );
        let identifier = created.str("identifier");
        let read = read_back(&dsql, &case, &identifier).await;
        assert_eq!(
            read.clone().map(|mut groups| {
                groups.sort();
                groups
            }),
            Some(want.clone()),
            "case {case:?}: a read of the cluster must report the groups {want:?}, got {read:?}"
        );
    }
}

/// One transition of a cluster's set of metric groups: what it was created with, what
/// the update asks for, and what the cluster collects afterwards.
struct Transition {
    name: &'static str,
    created_with: Option<Vec<&'static str>>,
    /// The `metricGroups` the update carries, or `None` for an update that omits the
    /// member — which asks for no change to the set.
    updated_with: Option<Vec<&'static str>>,
    want: Vec<&'static str>,
}

/// Every kind of transition between two sets, with the published value set standing
/// in for "some groups".
fn transitions() -> Vec<Transition> {
    let all = every_group();
    vec![
        Transition {
            name: "from no groups onto every group",
            created_with: None,
            updated_with: Some(all.clone()),
            want: all.clone(),
        },
        Transition {
            name: "from every group back to none, with an empty list",
            created_with: Some(all.clone()),
            updated_with: Some(Vec::new()),
            want: Vec::new(),
        },
        Transition {
            name: "from every group, with the member omitted",
            created_with: Some(all.clone()),
            updated_with: None,
            want: all.clone(),
        },
        Transition {
            name: "from no groups, with the member omitted",
            created_with: None,
            updated_with: None,
            want: Vec::new(),
        },
        Transition {
            name: "from every group onto the same set",
            created_with: Some(all.clone()),
            updated_with: Some(all.clone()),
            want: all.clone(),
        },
        Transition {
            name: "from no groups onto an empty list",
            created_with: None,
            updated_with: Some(Vec::new()),
            want: Vec::new(),
        },
    ]
}

/// RULE on the way THROUGH: an update REPLACES the whole set — the list it carries is
/// what the cluster collects afterwards, an empty list leaves it collecting nothing,
/// and an update that omits the member leaves the set exactly as it was.
#[tokio::test]
async fn an_update_replaces_the_whole_set_of_groups() {
    let dsql = Dsql::new();
    let cases = transitions();

    // Every cluster is created before the one wait for the clock, so the suite waits
    // once rather than once per case: an update is refused while the create's
    // `CREATING -> ACTIVE` transition is still outstanding.
    let mut identifiers = Vec::with_capacity(cases.len());
    for case in &cases {
        let body = match &case.created_with {
            None => json!({}),
            Some(groups) => json!({"metricGroups": requested(groups)}),
        };
        identifiers.push(create(&dsql, case.name, body).await.str("identifier"));
    }
    wait_for_the_clock().await;

    for (case, identifier) in cases.iter().zip(identifiers) {
        let body = match &case.updated_with {
            None => json!({"deletionProtectionEnabled": false}),
            Some(groups) => json!({"metricGroups": requested(groups)}),
        };
        let updated = dsql.update(&identifier, body).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdateCluster must succeed, got {} {:?}",
            case.name, updated.status, updated.body
        );
        let mut want: Vec<String> = case.want.iter().map(|g| (*g).to_string()).collect();
        want.sort();
        let read = read_back(&dsql, case.name, &identifier).await;
        assert_eq!(
            read.clone().map(|mut groups| {
                groups.sort();
                groups
            }),
            Some(want.clone()),
            "case {:?}: the cluster must collect {want:?} afterwards, got {read:?}",
            case.name
        );
    }
}

/// A `metricGroups` the model's value set does not admit: a name no published group
/// carries, and the one published group named twice (`MetricGroupRequestList` is
/// `@uniqueItems`).
fn refused_lists() -> Vec<(&'static str, Value)> {
    let first = METRIC_GROUP_VALUES.first().copied().unwrap_or_default();
    vec![
        (
            "a group the enum does not publish",
            requested(&["NO_SUCH_METRIC_GROUP"]),
        ),
        ("the same group named twice", requested(&[first, first])),
        (
            "a published group beside one the enum does not publish",
            requested(&[first, "NO_SUCH_METRIC_GROUP"]),
        ),
    ]
}

/// RULE at the boundary: a `metricGroups` outside what the model admits is refused
/// with the modelled `ValidationException` (400) naming the member, by the create and
/// by the update alike — and the update leaves the cluster collecting exactly what it
/// collected before.
#[tokio::test]
async fn a_metric_groups_outside_the_model_is_refused_and_changes_nothing() {
    let dsql = Dsql::new();
    let seeded = every_group();
    let identifier = create(
        &dsql,
        "the cluster the refused updates address",
        json!({"metricGroups": requested(&seeded)}),
    )
    .await
    .str("identifier");
    wait_for_the_clock().await;

    for (name, list) in refused_lists() {
        for operation in ["CreateCluster", "UpdateCluster"] {
            let case = format!("{operation} with {name}");
            let answer = match operation {
                "CreateCluster" => {
                    dsql.create(json!({
                        "deletionProtectionEnabled": false,
                        "metricGroups": list.clone(),
                    }))
                    .await
                }
                _ => {
                    dsql.update(&identifier, json!({"metricGroups": list.clone()}))
                        .await
                }
            };
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a value outside the model is refused 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: the wire error code must be ValidationException, got {:?}",
                answer.error
            );
            let field_list = answer.get("fieldList");
            let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
            assert!(
                fields.iter().any(|field| {
                    field
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.contains("metricGroup"))
                        && field
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(|message| !message.is_empty())
                }),
                "case {case:?}: fieldList must name the metricGroups member with a message \
                 saying why, got {field_list:?}"
            );
        }
    }

    let mut want: Vec<String> = seeded.iter().map(|g| (*g).to_string()).collect();
    want.sort();
    let read = read_back(&dsql, "the cluster the refused updates address", &identifier).await;
    assert_eq!(
        read.clone().map(|mut groups| {
            groups.sort();
            groups
        }),
        Some(want.clone()),
        "case a refused update: the cluster must still collect {want:?}, got {read:?}"
    );
}

/// RULE for the idempotency token: `metricGroups` is one of the parameters a
/// `clientToken` is bound to, so a retry naming the same groups answers the result the
/// first call produced and creates nothing new, while the same token naming a
/// DIFFERENT set is another request wearing a used token — a conflict (409).
#[tokio::test]
async fn the_client_token_is_bound_to_the_groups_the_request_named() {
    let dsql = Dsql::new();
    let groups = every_group();
    let token = "a-token-bound-to-one-set-of-groups";
    let first = create(
        &dsql,
        "the first call",
        json!({"metricGroups": requested(&groups), "clientToken": token}),
    )
    .await;
    let mut want: Vec<String> = groups.iter().map(|g| (*g).to_string()).collect();
    want.sort();

    let retry = create(
        &dsql,
        "a retry naming the same groups",
        json!({"metricGroups": requested(&every_group_reversed()), "clientToken": token}),
    )
    .await;
    assert_eq!(
        retry.str("identifier"),
        first.str("identifier"),
        "case a retry naming the same groups: it must answer the cluster the first call \
         minted, got {:?}",
        retry.body
    );
    let answered = reported(&retry);
    assert_eq!(
        answered.clone().map(|mut groups| {
            groups.sort();
            groups
        }),
        Some(want.clone()),
        "case a retry naming the same groups: it must answer the groups the first call \
         reported {want:?}, got {answered:?}"
    );

    let conflicting = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "metricGroups": [],
            "clientToken": token,
        }))
        .await;
    assert_eq!(
        conflicting.status, 409,
        "case the same token naming another set: it must be refused 409, got {} {:?}",
        conflicting.status, conflicting.body
    );
    assert_eq!(
        conflicting.error.as_deref(),
        Some("ConflictException"),
        "case the same token naming another set: the wire error code must be \
         ConflictException, got {:?}",
        conflicting.error
    );
}
