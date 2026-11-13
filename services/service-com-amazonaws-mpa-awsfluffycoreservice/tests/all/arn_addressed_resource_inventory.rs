// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE existence decision every operation that addresses a
//! resource by `ResourceArn` shares — the tagging reads and the resource-policy reads
//! alike.
//!
//! The rule under test: an ARN names an existing resource exactly when this store holds
//! a resource of one of its OWN kinds under it, so every operation that resolves a
//! resource ARN gives the same existence answer — it answers from the record for an ARN
//! the store holds a resource under, and `ResourceNotFoundException` (404) for one it
//! does not. What a stored resource CARRIES is a separate question from whether it
//! exists: an approval team's attached policies are the `Policies` references its record
//! holds, and a stored resource of a kind no policy attaches to carries none, so it is an
//! EMPTY `ListResourcePolicies` page rather than a missing resource.
//!
//! The table spans the class the rule generalises over rather than one recorded shape:
//! the resource KIND the store holds (the approval team and the identity source, the two
//! kinds every create of this service records), how many policies the resource attaches
//! (none at all, one, and `Policies`' published bound of ten), how many tags it carries
//! (none, two, three), and the status its record is in (a source still `CREATING` and one
//! whose removal is pending — a record the store holds either way). The contrast table
//! spans WHY an ARN names no stored resource: a resource kind of this service nothing was
//! created under, a resource id outside the form this service mints, another MPA resource
//! kind, another service's namespace, and a stored resource's own ARN with one character
//! changed.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the `GET /tags/{ResourceArn}`,
//! `POST /resource-policies/{ResourceArn}/?List` and `POST /GetResourcePolicy` routes,
//! the URI labels, the JSON envelope and the HTTP status are all exercised.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};
use crate::identity_source_deletion::{create_source, delete_source};
use crate::resource_policies::{create_body, get_resource_policy};

/// A policy name the catalog does not hold, so no resource of this store can carry it:
/// what a `GetResourcePolicy` row asks for to separate "this resource carries no such
/// policy" from "this resource does not exist".
const UNATTACHED_POLICY_NAME: &str = "NoPolicyOfThisNameIsInTheCatalog";

/// `Policies` is `@length(min: 1, max: 10)`, so a team attaches at most this many — and
/// at least one, which is why the zero-attachment row below is a resource of the other
/// kind.
const MAX_POLICIES: usize = 10;

/// How long a poll waits for a clock-owed removal before giving up. Generous: the test
/// looks again until the record is gone rather than waiting a fixed time, so a loaded
/// host only takes more looks.
const REMOVAL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll pauses between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`.
async fn list_tags(harness: &Harness, resource_arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/tags/{}", encode_label(resource_arn)), None)
        .await
}

/// `POST /resource-policies/{ResourceArn}/?List` — `ListResourcePolicies`.
async fn list_resource_policies(harness: &Harness, resource_arn: &str) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/resource-policies/{}/?List", encode_label(resource_arn)),
            None,
        )
        .await
}

/// A reference to the catalog entry `MPAApprovalTeamPolicy<n>` the harness records,
/// attached as its default version — one of the two forms
/// `PolicyReference$PolicyArn`'s pattern allows.
fn attachment(n: usize) -> String {
    format!("arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy{n}/$DEFAULT")
}

/// `Tags` for a resource carrying `count` of them, inside `TagKey`'s `length(1..128)`
/// and `TagValue`'s `length(0..256)`.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (1..=count)
            .map(|t| (format!("key{t:02}"), json!(format!("value{t:02}"))))
            .collect(),
    )
}

/// Create an approval team attaching `attachments` catalog references and carrying
/// `tags` tags, and answer the ARN this service minted for it.
async fn create_team(harness: &Harness, name: &str, attachments: usize, tags: usize) -> String {
    let references: Vec<String> = (1..=attachments).map(attachment).collect();
    let mut body = create_body(name, &references);
    if tags > 0 {
        body["Tags"] = tag_map(tags);
    }
    let (status, created) = harness.create(&body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApprovalTeam returned {status} with {created:?}"
    );
    let arn = text(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "case {name:?}: CreateApprovalTeam returned no Arn: {created:?}"
    );
    arn
}

/// The tags the STORE holds for `arn`, or `None` when it records no resource under it.
fn stored_tags(harness: &Harness, arn: &str) -> Option<BTreeMap<String, String>> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.resource_tags(arn).cloned()
}

/// The policies the STORE resolves for `arn`, or `None` when it records no resource
/// under it — what every answer below must be computed from.
fn stored_attachments(harness: &Harness, arn: &str) -> Option<Vec<state::ResourcePolicy>> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.resource_policies(arn)
}

/// Push the clock-owed removal of the identity source `arn` out past this test, so the
/// record stays in the store in its pending-removal status while the reads below run.
///
/// No modelled operation postpones a removal, so the test sets the due instant directly,
/// the way `Harness::postpone_deletion_approval` sets a team's.
fn postpone_source_removal(harness: &Harness, arn: &str) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let source = guard
        .identity_source_mut(arn)
        .unwrap_or_else(|| panic!("the store holds the identity source {arn}"));
    source.deletion_due = Some(SystemTime::now() + REMOVAL_DEADLINE * 60);
}

/// Create one resource per row of the inventory table and answer `(case, ARN)` for each.
async fn stored_resources(harness: &Harness) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    rows.push((
        "an approval team attaching one policy, untagged",
        create_team(harness, "inventory-one", 1, 0).await,
    ));
    rows.push((
        "an approval team attaching every policy Policies allows, tagged",
        create_team(harness, "inventory-max", MAX_POLICIES, 2).await,
    ));
    rows.push((
        "an identity source attaching no policy, untagged",
        create_source(harness, 1, 0).await,
    ));
    rows.push((
        "an identity source attaching no policy, tagged",
        create_source(harness, 2, 3).await,
    ));

    // A source whose removal has been asked for is still a record this store holds, so
    // every read above still finds it; the removal is pushed out so the row does not
    // depend on how fast this test runs.
    let pending = create_source(harness, 3, 1).await;
    let (status, body) = delete_source(harness, &pending).await;
    assert_eq!(
        status, 200,
        "DeleteIdentitySource returned {status} with {body:?}"
    );
    postpone_source_removal(harness, &pending);
    rows.push(("an identity source whose removal is pending", pending));

    rows
}

/// Every resource KIND this store records is found by every ARN-addressed read, each
/// answering from that record: the tags the record holds, the policies it attaches (none
/// for a kind no policy attaches to, which is an empty page and not a missing resource),
/// and each attached policy singly.
#[tokio::test]
async fn every_stored_resource_kind_is_found_by_every_arn_addressed_read() {
    let harness = Harness::new();
    let rows = stored_resources(&harness).await;

    for (case, arn) in &rows {
        let expected_tags = stored_tags(&harness, arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store records nothing under {arn}"));
        let expected_attached = stored_attachments(&harness, arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store records nothing under {arn}"));

        // The tagging read: it finds the resource and reports the record's own map.
        let (status, tags) = list_tags(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource answered {status} with {tags:?}"
        );
        let reported: BTreeMap<String, String> = tags
            .get("Tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no Tags map in {tags:?}"))
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect();
        assert_eq!(
            reported, expected_tags,
            "case {case:?}: expected the record's tags {expected_tags:?}, got {tags:?}"
        );

        // The resource-policy listing: the SAME existence answer, and the attachments
        // the record holds — an empty list for a resource that attaches none.
        let (status, page) = list_resource_policies(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourcePolicies answered {status} with {page:?}"
        );
        let listed: Vec<String> = page
            .get("ResourcePolicies")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicies list in {page:?}"))
            .iter()
            .map(|entry| text(entry, "PolicyName"))
            .collect();
        let expected_names: Vec<String> = expected_attached
            .iter()
            .map(|policy| policy.policy_name.clone())
            .collect();
        assert_eq!(
            listed, expected_names,
            "case {case:?}: expected the record's attachments {expected_names:?}, got \
             {page:?}"
        );

        // Each attached policy reads back singly, from the same record.
        for policy in &expected_attached {
            let (status, single) = get_resource_policy(
                &harness,
                arn,
                &policy.policy_name,
                policy.policy_type.as_wire(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {:?}: GetResourcePolicy answered {status} with {single:?}",
                policy.policy_name
            );
            assert_eq!(
                text(&single, "ResourceArn"),
                *arn,
                "case {case:?} / {:?}: wrong ResourceArn in {single:?}",
                policy.policy_name
            );
            assert_eq!(
                text(&single, "PolicyDocument"),
                policy.document,
                "case {case:?} / {:?}: expected the catalog version's document, got \
                 {single:?}",
                policy.policy_name
            );
        }

        // A policy name no catalog entry carries is the not-found refusal — the answer
        // for a resource that exists and carries no such policy, which is exactly what a
        // resource attaching none at all earns, even though the two reads above found it.
        let (status, missing) =
            get_resource_policy(&harness, arn, UNATTACHED_POLICY_NAME, "AWS_MANAGED").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a policy name nothing carries answered {status} with {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {missing:?}"
        );
        assert_eq!(
            text(&missing, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: wrong refusal wording in {missing:?}"
        );
    }

    // These are reads: nothing they reported moved, so a later caller is answered the
    // same records.
    for (case, arn) in &rows {
        let (status, again) = list_tags(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource stopped finding {arn}: {again:?}"
        );
        assert_eq!(
            Some(
                again
                    .get("Tags")
                    .and_then(Value::as_object)
                    .map(|map| map
                        .iter()
                        .map(|(key, value)| (
                            key.clone(),
                            value.as_str().unwrap_or_default().to_string()
                        ))
                        .collect::<BTreeMap<String, String>>())
                    .unwrap_or_default()
            ),
            stored_tags(&harness, arn),
            "case {case:?}: a second read disagrees with the record's tags"
        );
        let (status, again) = list_resource_policies(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourcePolicies stopped finding {arn}: {again:?}"
        );
        let names: Vec<String> = again
            .get("ResourcePolicies")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicies list in {again:?}"))
            .iter()
            .map(|entry| text(entry, "PolicyName"))
            .collect();
        let expected: Vec<String> = stored_attachments(&harness, arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} stopped being recorded"))
            .iter()
            .map(|policy| policy.policy_name.clone())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: a second read disagrees with the record's attachments"
        );
    }
}

/// Values that are well-formed ARNs in the caller's own account under which this store
/// records no resource. Rows differ along the resource kind named, the form of the
/// resource id, the service namespace and the distance from a stored resource's ARN.
fn arns_no_resource_is_stored_under(stored_team: &str) -> Vec<(&'static str, String)> {
    let own = engine::emulator_account_id();
    vec![
        (
            "an approval team nothing was created under",
            format!("arn:aws:mpa:{REGION}:{own}:approval-team/never-created"),
        ),
        (
            // The id form this service mints ids in (`state::identity_source_arn`), for
            // a sequence number no create reached.
            "an identity-source ARN in the minted id form, nothing created under it",
            state::identity_source_arn(REGION, 9_999),
        ),
        (
            "an identity-source ARN whose id is outside the minted form",
            format!("arn:aws:mpa:{REGION}:{own}:identity-source/ssoins-0001"),
        ),
        (
            "an approval session's ARN, a kind no tag or policy read addresses",
            format!("arn:aws:mpa:{REGION}:{own}:session/some-team/session-000000000001"),
        ),
        (
            "another service's ARN in the caller's account",
            format!("arn:aws:iam:{REGION}:{own}:role/not-an-mpa-resource"),
        ),
        (
            "a stored team's ARN with one character appended",
            format!("{stored_team}x"),
        ),
    ]
}

/// An ARN this store records no resource under earns the not-found refusal from EVERY
/// ARN-addressed read — the contrast that keeps the existence decision from being "found
/// whatever is asked for".
#[tokio::test]
async fn an_arn_nothing_is_stored_under_is_not_found_by_every_arn_addressed_read() {
    let harness = Harness::new();
    // Stored resources of BOTH kinds, so no refusal below is an accident of an empty
    // store.
    let team = create_team(&harness, "inventory-bystander", 1, 1).await;
    let source = create_source(&harness, 1, 1).await;

    for (case, arn) in arns_no_resource_is_stored_under(&team) {
        let (status, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({arn:?}): ListTagsForResource answered {status} with {tags:?}"
        );
        assert_eq!(
            error_code(&tags),
            "ResourceNotFoundException",
            "case {case:?} ({arn:?}): wrong error code in {tags:?}"
        );

        let (status, page) = list_resource_policies(&harness, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({arn:?}): ListResourcePolicies answered {status} with {page:?}"
        );
        assert_eq!(
            error_code(&page),
            "ResourceNotFoundException",
            "case {case:?} ({arn:?}): wrong error code in {page:?}"
        );
        assert_eq!(
            text(&page, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?} ({arn:?}): wrong refusal wording in {page:?}"
        );

        let (status, single) =
            get_resource_policy(&harness, &arn, UNATTACHED_POLICY_NAME, "AWS_MANAGED").await;
        assert_eq!(
            status, 404,
            "case {case:?} ({arn:?}): GetResourcePolicy answered {status} with {single:?}"
        );
        assert_eq!(
            error_code(&single),
            "ResourceNotFoundException",
            "case {case:?} ({arn:?}): wrong error code in {single:?}"
        );
        assert_eq!(
            text(&single, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?} ({arn:?}): wrong refusal wording in {single:?}"
        );

        assert_eq!(
            stored_attachments(&harness, &arn),
            None,
            "case {case:?} ({arn:?}): the store began recording a resource under it"
        );
    }

    // The refusals are about the ARNs asked for: the stored resources of both kinds are
    // still found by both readers.
    for (case, arn) in [("the stored team", &team), ("the stored source", &source)] {
        let (status, tags) = list_tags(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource answered {status} with {tags:?}"
        );
        let (status, page) = list_resource_policies(&harness, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourcePolicies answered {status} with {page:?}"
        );
    }
}

/// A resource the store stops holding stops being found by every ARN-addressed read at
/// the same instant: once an identity source's clock-owed removal has been carried out,
/// the tagging read and both resource-policy reads answer the not-found refusal, as they
/// do for an ARN nothing was ever created under.
#[tokio::test]
async fn a_resource_the_store_stops_holding_is_found_by_no_read() {
    let harness = Harness::new();
    let arn = create_source(&harness, 1, 2).await;

    // While the record is held, both existence answers are "found".
    let (status, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "the stored source's tags answered {status} with {tags:?}"
    );
    let (status, page) = list_resource_policies(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "the stored source's policies answered {status} with {page:?}"
    );

    let (status, deleted) = delete_source(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "DeleteIdentitySource returned {status} with {deleted:?}"
    );

    // The removal is CLOCK-owed and materialised by the next call, so the test looks
    // again until the record is gone rather than waiting a fixed time.
    let started = Instant::now();
    loop {
        let (status, _) = list_tags(&harness, &arn).await;
        if status == 404 {
            break;
        }
        assert!(
            started.elapsed() < REMOVAL_DEADLINE,
            "the removal of {arn} was still not materialised; ListTagsForResource \
             answers {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert_eq!(
        stored_attachments(&harness, &arn),
        None,
        "the store still records a resource under the removed source's ARN"
    );
    for (reader, (status, body)) in [
        ("ListTagsForResource", list_tags(&harness, &arn).await),
        (
            "ListResourcePolicies",
            list_resource_policies(&harness, &arn).await,
        ),
        (
            "GetResourcePolicy",
            get_resource_policy(&harness, &arn, UNATTACHED_POLICY_NAME, "AWS_MANAGED").await,
        ),
    ] {
        assert_eq!(
            status, 404,
            "{reader}: the removed source answered {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{reader}: wrong error code in {body:?}"
        );
    }
}
