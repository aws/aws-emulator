// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteTrafficPolicy`.
//!
//! The rule under test: a delete FLAGS the one traffic policy version its
//! `(Id, Version)` names. `DeleteTrafficPolicy`'s own documentation states both
//! halves of what that means — "Route 53 sets a flag on the policy to indicate
//! that it has been deleted. However, Route 53 never fully deletes the traffic
//! policy … Deleted traffic policies aren't listed if you run
//! ListTrafficPolicies … If you retain the ID of the policy, you can get
//! information about the policy, including the traffic policy document, by
//! running GetTrafficPolicy" — so from the delete on:
//!
//! 1. the flagged version is no longer one of the account's traffic policy
//!    versions: no listing reports it, no further version and no traffic policy
//!    instance is built on it, its comment cannot be updated, and addressing it
//!    for deletion again is the modelled `NoSuchTrafficPolicy` (HTTP 404);
//! 2. `GetTrafficPolicy` still reports it, exactly as it was created — the one
//!    reader the documentation keeps;
//! 3. a policy whose every version is flagged is no longer a policy the account
//!    HOLDS: it drops out of `ListTrafficPolicies` and frees its slot in the
//!    account's traffic policy quota — while its record, and so its `Id`,
//!    survives, so its version listing is an empty collection and a further
//!    version can still be added to it;
//! 4. the version NUMBER stays taken, so the policy's next version counts from
//!    the highest it ever reached rather than reusing a flagged one;
//! 5. a version traffic policy instances were created from cannot be deleted at
//!    all (`TrafficPolicyInUse`, HTTP 400) and stays exactly as it was until the
//!    last instance built on it is gone;
//! 6. nothing but the pair the request names moves: every other version of the
//!    policy, and every other policy of the account, reads back unchanged.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! renaming a policy: how many versions the policy holds (one, two, three,
//! four), WHICH version is deleted (the only one, the first, one in the middle,
//! the latest, and every one of them in an order that is not the numerical one),
//! the record type and entry-point form each version carries, and — for the
//! refusals — the form of an `(Id, Version)` the account does not hold.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy,
    policy_count, policy_of, PolicyView,
};
use crate::traffic_policy_instance_deletion::delete_instance;
use crate::traffic_policy_instances::create_instance;
use crate::traffic_policy_not_found_envelope::{
    assert_identity_only, assert_no_such_traffic_policy,
};
use crate::traffic_policy_versions::create_version;

/// The one sentence `TrafficPolicyInUse` reports, which names neither the
/// policy nor the instances still built on it.
///
/// The shape's own documentation states the rule it reports: "One or more
/// traffic policy instances were created by using the specified traffic
/// policy."
const IN_USE_MESSAGE: &str =
    "One or more traffic policy instances were created by using the specified traffic policy.";

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Send one `DeleteTrafficPolicy` request, with the `{Id}` and `{Version}`
/// labels written exactly as the caller spells them.
///
/// `pub(crate)` so the suite asserting a policy record outlives its versions
/// deletes them through the SAME request rather than growing a second copy of
/// it.
pub(crate) async fn delete_policy(router: &MultiServiceRouter, id: &str, version: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("/2013-04-01/trafficpolicy/{id}/{version}"),
        "",
    )
    .await
}

/// Send one `ListTrafficPolicyVersions` request for the policy `id` names.
async fn list_versions(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicies/{id}/versions"),
        "",
    )
    .await
}

/// Send one `ListTrafficPolicies` request over the whole account.
async fn list_policies(router: &MultiServiceRouter) -> Reply {
    call(router, "GET", "/2013-04-01/trafficpolicies", "").await
}

/// Update one version's comment, which is the other operation addressed at a
/// single `(Id, Version)` pair.
async fn update_comment(
    router: &MultiServiceRouter,
    id: &str,
    version: &str,
    comment: &str,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/trafficpolicy/{id}/{version}"),
        &format!(
            "<UpdateTrafficPolicyCommentRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Comment>{comment}</Comment></UpdateTrafficPolicyCommentRequest>"
        ),
    )
    .await
}

// ---------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------

/// The version numbers a policy's version listing reports, in the order it
/// reports them.
async fn listed_versions(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<String> {
    let reply = list_versions(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTrafficPolicyVersions, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .all("TrafficPolicy/Version")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One `TrafficPolicySummary` as the account listing reports it:
/// `(Id, LatestVersion, TrafficPolicyCount)`.
async fn listed_policies(router: &MultiServiceRouter, case: &str) -> Vec<(String, String, String)> {
    let reply = list_policies(router).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTrafficPolicies, got {} with body {}",
        reply.status, reply.body
    );
    let ids = reply.all("TrafficPolicySummary/Id");
    let latest = reply.all("TrafficPolicySummary/LatestVersion");
    let counts = reply.all("TrafficPolicySummary/TrafficPolicyCount");
    assert_eq!(
        (latest.len(), counts.len()),
        (ids.len(), ids.len()),
        "case {case:?}: every listed policy must be summarised whole, got {} latest version(s) \
         and {} count(s) beside {} id(s); body {}",
        latest.len(),
        counts.len(),
        ids.len(),
        reply.body
    );
    (0..ids.len())
        .map(|at| {
            (
                ids[at].to_string(),
                latest[at].to_string(),
                counts[at].to_string(),
            )
        })
        .collect()
}

/// Assert that `(id, version)` is refused as a traffic policy version the
/// account does not hold, on an operation that STATES the refusal's sentence.
fn assert_no_such_policy(case: &str, reply: &Reply) {
    assert_no_such_traffic_policy(reply, case);
}

/// Assert the same refusal as the DELETE words it: the refusal's identity alone, with no
/// `Message` element on the wire.
fn assert_delete_refused(case: &str, reply: &Reply) {
    assert_identity_only(reply, case, "NoSuchTrafficPolicy");
}

// ---------------------------------------------------------------------------
// Populations
// ---------------------------------------------------------------------------

/// The document one version of a test policy carries: a value of the right
/// shape for the record type, and an entry-point form that alternates with the
/// version number, so no two versions of a policy carry the same document and a
/// reader reporting the wrong one cannot pass.
fn document_for(rr_type: &str, nth: i32) -> String {
    let value = match rr_type {
        "A" => format!("192.0.2.{nth}"),
        "AAAA" => format!("2001:db8::{nth}"),
        "CNAME" => format!("v{nth}.example.com"),
        "TXT" => format!("version={nth}"),
        "MX" => format!("{nth}0 mail.example.com"),
        "SRV" => format!("1 1 {nth}0 a.example.com"),
        other => panic!("the suite has no endpoint value for the record type {other:?}"),
    };
    if nth % 2 == 0 {
        document_with_start_rule(rr_type, &value, &format!("alt-{value}"))
    } else {
        document_with_start_endpoint(rr_type, &value)
    }
}

/// Create a policy whose versions carry `version_types` in order, and return
/// what every version reported when it was created — index 0 is version 1.
///
/// `pub(crate)` so the suite asserting a policy record outlives its versions
/// builds its multi-version policies the SAME way this one does.
pub(crate) async fn policy_with_versions(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version_types: &[&str],
) -> Vec<PolicyView> {
    let mut created: Vec<PolicyView> = Vec::new();
    for (at, rr_type) in version_types.iter().enumerate() {
        let nth = i32::try_from(at).expect("the suite builds few versions") + 1;
        let document = document_for(rr_type, nth);
        let comment = format!("{name} version {nth}");
        let reply = if at == 0 {
            create_policy(router, name, &document, Some(&comment)).await
        } else {
            create_version(router, &created[0].id, &document, Some(&comment)).await
        };
        assert_eq!(
            reply.status, 201,
            "case {case:?}: expected 201 while building version {nth} of {name:?}, got {} with \
             body {}",
            reply.status, reply.body
        );
        let view = policy_of(&reply, case);
        assert_eq!(
            view.version,
            nth.to_string(),
            "case {case:?}: expected version {nth} of {name:?}, got {:?}",
            view.version
        );
        created.push(view);
    }
    created
}

/// Read one version back and assert it reports exactly `expected`.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    case: &str,
    expected: &PolicyView,
) {
    let read = get_policy(router, &expected.id, &expected.version).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetTrafficPolicy for version {}, got {} with body {}",
        expected.version, read.status, read.body
    );
    let got = policy_of(&read, case);
    assert_eq!(
        &got, expected,
        "case {case:?}: GetTrafficPolicy must report version {} as it was created, got {got:?}; \
         body {}",
        expected.version, read.body
    );
}

// ---------------------------------------------------------------------------
// A delete flags the one version it names
// ---------------------------------------------------------------------------

/// One row of the delete table: the versions a policy is built with, and the
/// order its versions are deleted in.
struct DeleteRow {
    case: &'static str,
    /// The record type of each version, in version order.
    version_types: &'static [&'static str],
    /// The version numbers to delete, in the order this row deletes them.
    delete_order: &'static [i32],
}

const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "the only version of a one-version policy",
        version_types: &["A"],
        delete_order: &[1],
    },
    DeleteRow {
        case: "the latest version of a two-version policy",
        version_types: &["A", "AAAA"],
        delete_order: &[2],
    },
    DeleteRow {
        case: "the first version of a two-version policy",
        version_types: &["CNAME", "TXT"],
        delete_order: &[1],
    },
    DeleteRow {
        case: "the middle version of a three-version policy",
        version_types: &["A", "MX", "SRV"],
        delete_order: &[2],
    },
    DeleteRow {
        case: "the latest version of a three-version policy, then the first",
        version_types: &["TXT", "A", "AAAA"],
        delete_order: &[3, 1],
    },
    DeleteRow {
        case: "every version of a four-version policy, out of numerical order",
        version_types: &["A", "AAAA", "CNAME", "TXT"],
        delete_order: &[2, 4, 1, 3],
    },
];

#[tokio::test]
async fn each_deleted_version_stops_being_one_the_account_holds() {
    for row in DELETE_ROWS {
        let case = row.case;
        let router = router();

        // A policy nothing in this row deletes, so every assertion below also
        // proves the delete reaches only the pair the request names.
        let bystander = policy_with_versions(&router, case, "bystander", &["A", "SRV"]).await;
        let under_test = policy_with_versions(&router, case, "under-test", row.version_types).await;
        let id = under_test[0].id.clone();

        let highest = i32::try_from(row.version_types.len()).expect("few versions");
        let mut live: Vec<i32> = (1..=highest).collect();

        for &version in row.delete_order {
            let deleted = delete_policy(&router, &id, &version.to_string()).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected 200 from DeleteTrafficPolicy for version {version}, \
                 got {} with body {}",
                deleted.status, deleted.body
            );
            assert!(
                deleted.error_code().is_empty(),
                "case {case:?}: a served delete must carry no error, got {:?}; body {}",
                deleted.error_code(),
                deleted.body
            );
            live.retain(|held| *held != version);

            // The record is RETAINED: every version of the policy, flagged or
            // not, still reports what it was created with.
            for view in &under_test {
                assert_reads_back(&router, case, view).await;
            }

            // The version listing reports the versions the account still
            // holds, in numerical order — and a policy with none of them left
            // lists an EMPTY collection: the policy record is still there, so
            // the listing is not a missing resource.
            let expected: Vec<String> = live.iter().map(|version| version.to_string()).collect();
            let got = listed_versions(&router, case, &id).await;
            assert_eq!(
                got, expected,
                "case {case:?}: after deleting version {version} the listing must report \
                 {expected:?}, got {got:?}"
            );

            // The account listing reports the policy only while it holds a
            // version, summarised by its highest LIVE one.
            let mut expected_summaries = vec![(
                bystander[0].id.clone(),
                "2".to_string(),
                "2".to_string(),
            )];
            if let Some(latest) = live.iter().max() {
                expected_summaries.push((
                    id.clone(),
                    latest.to_string(),
                    live.len().to_string(),
                ));
            }
            let summaries = listed_policies(&router, case).await;
            assert_eq!(
                summaries, expected_summaries,
                "case {case:?}: after deleting version {version} the account listing must \
                 report {expected_summaries:?}, got {summaries:?}"
            );

            // The account's traffic policy quota counts the policies it holds,
            // so a policy every version of which is flagged releases its slot.
            let expected_count = if live.is_empty() { "1" } else { "2" };
            let count = policy_count(&router).await;
            assert_eq!(
                count, expected_count,
                "case {case:?}: after deleting version {version} the account must hold \
                 {expected_count} traffic policy(ies), got Count {count:?}"
            );

            // Deleting the same pair again has nothing left to delete, and
            // neither has an update of its comment.
            let again = delete_policy(&router, &id, &version.to_string()).await;
            assert_delete_refused(case, &again);
            let updated = update_comment(&router, &id, &version.to_string(), "revived?").await;
            assert_no_such_policy(case, &updated);
            assert_reads_back(&router, case, &under_test[(version - 1) as usize]).await;

            // The bystander policy is untouched, whichever version went.
            let bystander_versions = listed_versions(&router, case, &bystander[0].id).await;
            assert_eq!(
                bystander_versions,
                vec!["1".to_string(), "2".to_string()],
                "case {case:?}: deleting version {version} of one policy must leave another \
                 policy's versions alone, got {bystander_versions:?}"
            );
            for view in &bystander {
                assert_reads_back(&router, case, view).await;
            }
        }

        // A policy with no version left is one the account no longer LISTS,
        // while its record — and so its id — survives: it still takes a further
        // version, numbered one past the highest it ever reached. And because a
        // `Name` is a label rather than an identity, the name it carries is
        // still available to a new policy — which is a NEW policy, with its own
        // id and its own version 1.
        if live.is_empty() {
            let revived = create_version(
                &router,
                &id,
                &document_with_start_endpoint("A", "192.0.2.200"),
                None,
            )
            .await;
            assert_eq!(
                revived.status, 201,
                "case {case:?}: a policy whose versions were all deleted must still take a \
                 further version, got {} with body {}",
                revived.status, revived.body
            );
            let revived = policy_of(&revived, case);
            let expected_version = (under_test.len() + 1).to_string();
            assert_eq!(
                revived.version, expected_version,
                "case {case:?}: the version after every version was deleted must be \
                 {expected_version}, got {:?}",
                revived.version
            );
            assert_eq!(
                (revived.id.as_str(), revived.name.as_str()),
                (id.as_str(), "under-test"),
                "case {case:?}: the further version must be a version OF the policy its id \
                 names, got {revived:?}"
            );

            let reborn = create_policy(
                &router,
                "under-test",
                &document_with_start_endpoint("A", "192.0.2.201"),
                None,
            )
            .await;
            assert_eq!(
                reborn.status, 201,
                "case {case:?}: a name another policy carries must not refuse a create, got {} \
                 with body {}",
                reborn.status, reborn.body
            );
            let reborn = policy_of(&reborn, case);
            assert_ne!(
                reborn.id, id,
                "case {case:?}: the policy taking the shared name must be a new policy, got the \
                 earlier policy's id {id:?} again"
            );
            assert_eq!(
                reborn.version, "1",
                "case {case:?}: a new policy's first version is 1, got {:?}",
                reborn.version
            );

            // And every flagged version of the old policy still reads back, by
            // its retained id.
            for view in &under_test {
                assert_reads_back(&router, case, view).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The version number stays taken
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_version_number_is_not_handed_out_again() {
    let case = "a policy whose highest and first versions were deleted";
    let router = router();
    let created = policy_with_versions(&router, case, "numbering", &["A", "AAAA", "CNAME"]).await;
    let id = created[0].id.clone();

    for version in ["3", "1"] {
        let deleted = delete_policy(&router, &id, version).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 deleting version {version}, got {} with body {}",
            deleted.status, deleted.body
        );
    }

    // The next version counts from the highest the policy EVER reached, so it
    // is 4: neither the flagged 3 nor the flagged 1 is handed out again.
    let next = create_version(
        &router,
        &id,
        &document_with_start_endpoint("TXT", "version=4"),
        Some("the version after the deletes"),
    )
    .await;
    assert_eq!(
        next.status, 201,
        "case {case:?}: expected 201 from CreateTrafficPolicyVersion, got {} with body {}",
        next.status, next.body
    );
    let next = policy_of(&next, case);
    assert_eq!(
        next.version, "4",
        "case {case:?}: expected the version after the highest the policy reached, got {:?}",
        next.version
    );

    // The flagged versions still report what they were created with — the new
    // version replaced neither of them — and only the live ones are listed.
    for view in &created {
        assert_reads_back(&router, case, view).await;
    }
    let listed = listed_versions(&router, case, &id).await;
    assert_eq!(
        listed,
        vec!["2".to_string(), "4".to_string()],
        "case {case:?}: only the versions the account holds are listed, got {listed:?}"
    );
    let summaries = listed_policies(&router, case).await;
    assert_eq!(
        summaries,
        vec![(id.clone(), "4".to_string(), "2".to_string())],
        "case {case:?}: the summary reports the highest live version and how many are live, \
         got {summaries:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_or_version_the_account_does_not_hold_is_refused() {
    let case_router = router();
    let held = policy_with_versions(
        &case_router,
        "refusals",
        "the-only-policy",
        &["A", "AAAA"],
    )
    .await;
    let id = held[0].id.clone();

    // An id one character off the held one, so a lookup that matched on a
    // prefix could not pass.
    let mut near_miss = id.clone();
    near_miss.pop();
    near_miss.push(if id.ends_with('0') { '1' } else { '0' });

    let rows: Vec<(&str, String, String)> = vec![
        (
            "an id in the minted form that no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "1".to_string(),
        ),
        (
            "an id one character off the held one",
            near_miss,
            "1".to_string(),
        ),
        (
            "an id that is not in the minted form at all",
            "x".to_string(),
            "1".to_string(),
        ),
        (
            "a version the policy has never reached",
            id.clone(),
            "3".to_string(),
        ),
        (
            "the highest version a policy may carry, which this one does not",
            id.clone(),
            "1000".to_string(),
        ),
    ];

    for (case, requested_id, version) in &rows {
        let reply = delete_policy(&case_router, requested_id, version).await;
        assert_delete_refused(case, &reply);
    }

    // Nothing the refusals touched: both versions still read back and both are
    // still listed.
    for view in &held {
        assert_reads_back(&case_router, "after the refusals", view).await;
    }
    let listed = listed_versions(&case_router, "after the refusals", &id).await;
    assert_eq!(
        listed,
        vec!["1".to_string(), "2".to_string()],
        "a refused delete must leave the policy's versions as they were, got {listed:?}"
    );
}

#[tokio::test]
async fn a_version_outside_the_modelled_range_is_refused_before_the_handler() {
    // `TrafficPolicyVersion` is `range 1..1000`
    // (smithy_query(query="shape",shape="TrafficPolicyVersion")): a version
    // outside it is refused by the constraint rather than by this operation, so
    // it never reaches the store — the published bounds themselves are served
    // (as the `NoSuchTrafficPolicy` above) and only what is past them is
    // rejected here.
    let router = router();
    let held = policy_with_versions(&router, "range", "bounded", &["A"]).await;
    let id = held[0].id.clone();

    for version in ["0", "-1", "1001"] {
        let reply = delete_policy(&router, &id, version).await;
        assert_eq!(
            reply.status, 400,
            "case {version:?}: expected HTTP 400 for a version outside the modelled range, \
             got {} with body {}",
            reply.status, reply.body
        );
    }

    assert_reads_back(&router, "after the range refusals", &held[0]).await;
    let listed = listed_versions(&router, "after the range refusals", &id).await;
    assert_eq!(
        listed,
        vec!["1".to_string()],
        "a refused delete must leave the policy's versions as they were, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A version instances were created from
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_version_traffic_policy_instances_were_created_from_cannot_be_deleted() {
    let case = "a version with one traffic policy instance built on it";
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "traffic-policy-deletion").await;
    let created = policy_with_versions(&router, case, "in-use", &["A", "AAAA"]).await;
    let id = created[0].id.clone();

    let instance = create_instance(&router, &zone, "in-use.example.com", 60, &id, 1).await;
    assert_eq!(
        instance.status, 201,
        "case {case:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
        instance.status, instance.body
    );
    let instance_id = instance
        .first("TrafficPolicyInstance/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create must report the instance id; body {}",
                instance.body
            )
        })
        .to_string();

    // The version the instance was built on cannot go while the instance is
    // there, and the refusal changes nothing: the version still reads back and
    // is still listed.
    let refused = delete_policy(&router, &id, "1").await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 for a version instances were created from, got {} \
         with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TrafficPolicyInUse",
        "case {case:?}: expected TrafficPolicyInUse, got {:?}; body {}",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.first("Error/Message"),
        Some(IN_USE_MESSAGE),
        "case {case:?}: expected the message {IN_USE_MESSAGE:?}, got {:?}; body {}",
        refused.first("Error/Message"),
        refused.body
    );
    assert_reads_back(&router, case, &created[0]).await;
    let listed = listed_versions(&router, case, &id).await;
    assert_eq!(
        listed,
        vec!["1".to_string(), "2".to_string()],
        "case {case:?}: a refused delete must leave the policy's versions as they were, \
         got {listed:?}"
    );

    // The instance was built on ONE version, so the OTHER version of the same
    // policy carries no instance and is deletable.
    let other = delete_policy(&router, &id, "2").await;
    assert_eq!(
        other.status, 200,
        "case {case:?}: a version no instance was created from must be deletable, got {} with \
         body {}",
        other.status, other.body
    );

    // Deleting the last instance built on version 1 makes it deletable again:
    // the refusal is counted off the instances the account holds.
    let gone = delete_instance(&router, &instance_id).await;
    assert_eq!(
        gone.status, 200,
        "case {case:?}: expected 200 from DeleteTrafficPolicyInstance, got {} with body {}",
        gone.status, gone.body
    );
    let deleted = delete_policy(&router, &id, "1").await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected 200 deleting the version once its instance is gone, got {} \
         with body {}",
        deleted.status, deleted.body
    );

    // Both versions are flagged now, so the policy is gone from the account's
    // listing — while its id still resolves, to an empty version listing, and
    // both versions still read back by that retained id.
    let listing = list_versions(&router, &id).await;
    assert_eq!(
        listing.status, 200,
        "case {case:?}: expected 200 listing the versions of a policy whose versions are all \
         deleted, got {} with body {}",
        listing.status, listing.body
    );
    let listed = listing.all("TrafficPolicy/Version");
    assert!(
        listed.is_empty(),
        "case {case:?}: a policy with no version left must list none, got {listed:?}; body {}",
        listing.body
    );
    let summaries = listed_policies(&router, case).await;
    assert!(
        summaries.is_empty(),
        "case {case:?}: a policy every version of which is deleted must not be listed, \
         got {summaries:?}"
    );
    for view in &created {
        assert_reads_back(&router, case, view).await;
    }
}

// ---------------------------------------------------------------------------
// A deleted version is not a version anything new is built on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_traffic_policy_instance_is_created_from_a_deleted_version() {
    let case = "an instance over a version that was deleted";
    let router = router();
    let zone = create_zone_ok(&router, "example.net", "traffic-policy-deletion-instances").await;
    let created = policy_with_versions(&router, case, "applied-later", &["A", "AAAA"]).await;
    let id = created[0].id.clone();

    let deleted = delete_policy(&router, &id, "1").await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected 200 from DeleteTrafficPolicy, got {} with body {}",
        deleted.status, deleted.body
    );

    let refused = create_instance(&router, &zone, "one.example.net", 60, &id, 1).await;
    assert_no_such_policy(case, &refused);

    // The version that was NOT deleted still serves an instance, so the refusal
    // above is about the flagged version and not about the policy.
    let served = create_instance(&router, &zone, "two.example.net", 60, &id, 2).await;
    assert_eq!(
        served.status, 201,
        "case {case:?}: a version the account holds must still serve an instance, got {} with \
         body {}",
        served.status, served.body
    );
}
