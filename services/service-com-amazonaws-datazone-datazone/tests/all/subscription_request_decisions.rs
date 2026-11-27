// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DECISION side of a DataZone subscription request: the
//! status a created request carries, `AcceptSubscriptionRequest` /
//! `RejectSubscriptionRequest` taken on it, and the subscription an accept materialises.
//!
//! Every assertion is made on the HTTP answer only — a handler that answered from a
//! hardcoded value instead of from the store fails here. A FIXTURE reaches into the store
//! for exactly three things: a subscription target, because the operation that would record
//! one on the wire is still a scaffold (`CreateSubscriptionTarget` is addressed by an
//! `environmentIdentifier`, and no environment resource exists yet); the DataZone user its
//! PRODUCING projects belong to, because a project is attributed to the user that created
//! it and this emulator authenticates every caller as one user, so no wire call creates a
//! project somebody else owns ([`hand_to_another_owner`]); and the ids of the subscriptions
//! a domain holds, which keeps the cases here about the DECISION alone. Nothing is ASSERTED
//! from the store.
//!
//! The rules under test, each asserted over a TABLE of inputs:
//!
//! 1. A subscription request an asker that does NOT own the producing project makes is an
//!    ASK, so it is `PENDING`: it awaits a decision, with no reviewer, no decision comment
//!    and no existing subscription. Whether the domain holds a subscription target for the
//!    listing does not enter into it — a target is what a GRANT is fulfilled through, not a
//!    decision. (The other side of that rule, an OWNER's own ask resolved when it is made,
//!    is [`crate::subscription_request_owner_asks`].)
//! 2. A decision may be taken only on a request still awaiting one: `Accept` moves it to
//!    `ACCEPTED` and `Reject` to `REJECTED`, each recording the caller as `reviewerId` and
//!    `updatedBy`, the comment it carried, and a later `updatedAt` —
//!    `GetSubscriptionRequestDetails` reads back exactly that.
//! 3. Accepting a request GRANTS the access, so it materialises exactly one subscription:
//!    `APPROVED`, naming the request it came from and the principal and listing revision
//!    the ask recorded, and readable by `GetSubscription` from that call on. Rejecting
//!    materialises none.
//! 4. A request that is NOT awaiting a decision refuses either decision with 400
//!    `ValidationException`, leaves its stored record untouched and materialises no
//!    further subscription.
//! 5. A decision addressing a domain this store serves none for is denied, and one
//!    addressing a request the domain does not hold is a miss.
//! 6. The access itself is REPORTED: `ListSubscriptions` answers the subscriptions the
//!    domain holds and only those every filter the request names holds for — the request
//!    that materialised one, the listing it is on, the principal that holds it (one arm of
//!    the union per `owning*` member), the project that approves access to the listing,
//!    and the status, which defaults to `APPROVED` when the request names none.
//! 7. The access is given UP: `RevokeSubscription` moves a subscription that still grants
//!    it to `REVOKED`, recording the `retainPermissions` the call carried and the caller
//!    as `updatedBy`; `GetSubscription` and the listing report exactly that, a repeat
//!    revoke leaves the record where the first one left it, and an access given up some
//!    OTHER way conflicts with the status it carries.
//! 8. The WINDOW the listing reports through is the model's, not the caller's: the page
//!    sizes it admits are exactly the modelled `maxResults` range, the order is the one
//!    the model states whatever `sortBy` names, a later page resumes after the
//!    subscription the page before it ended on even when the store changed in between,
//!    and a domain holding no access at all still reports the required `items`.

use crate::harness::Emulator;
// Percent-encoding a query-parameter value: the one encoder this test target shares, so
// nothing here carries a second copy of it.
use crate::domain_project_lifecycle::urlencode;
use serde_json::{json, Value};
// The messages and enum members as the service states them, so a drift between these and
// what goes on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    cannot_reject_message, cannot_revoke_message, service_state, SubscriptionTarget,
    SUBSCRIPTION_ALREADY_DECIDED_MESSAGE, SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
    SUBSCRIPTION_REQUEST_STATUS_PENDING, SUBSCRIPTION_REQUEST_STATUS_REJECTED,
    SUBSCRIPTION_STATUS_APPROVED, SUBSCRIPTION_STATUS_CANCELLED, SUBSCRIPTION_STATUS_REVOKED,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The DataZone user every call here is attributed to, and so the reviewer of every
/// decision taken here: the user this emulator authenticates every caller as, read from
/// the service's own constant so a drift between it and the value on the wire fails here.
const CALLER: &str = service_com_amazonaws_datazone_datazone::state::CALLER_USER_PROFILE_ID;

/// An IAM principal a subscription request can be made for: a role ARN of this account,
/// which is what `IamPrincipalArn`'s modelled pattern admits. DataZone resolves nothing
/// for an IAM principal, so no record has to exist for it.
const CONSUMER_ROLE: &str = "arn:aws:iam::000000000000:role/datazone-consumer";

/// A domain with a producer project, a published asset of a named type and a consumer
/// project that asks for access to it — the fixture every case here needs.
struct Catalog {
    domain: String,
    consumer: String,
    /// The asset type the published asset is OF: what a subscription target's applicable
    /// asset types can be made to include.
    asset_type: String,
    /// The published asset itself — what an `assetScopes` or `assetPermissions` entry on
    /// a request against this listing may name.
    asset: String,
    listing: String,
    producer: String,
}

/// The DataZone user the fixture's PRODUCING projects are handed to: a `UserProfileId` of
/// the form the model admits (hex groups 8-4-4-4-12, smithy_query shape="UserProfileId")
/// and not the one this emulator authenticates the caller as, so the caller owns no project
/// that published a listing here.
const ANOTHER_OWNER: &str = "b17c5a90-3d4e-4f21-9ab8-0c1d2e3f4a5b";

/// Build the fixture: a settled domain, two projects, a custom asset type with the empty
/// `formsInput` the docs allow, an asset of that type and the listing publishing it. The
/// producing project is owned by somebody else ([`hand_to_another_owner`]), so every ask
/// made here is one that awaits a decision.
async fn catalog(
    dz: &Emulator,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    asset_type: &str,
) -> Catalog {
    let domain = dz.available_domain(name).await;
    let producer = dz.project(&domain, "producer").await;
    let consumer = dz.project(&domain, "consumer").await;
    let (asset, listing) = publish_asset_listing(dz, &domain, &producer, asset_type).await;
    hand_to_another_owner(engine, &domain, &producer);

    Catalog {
        domain,
        consumer,
        asset_type: asset_type.to_string(),
        asset,
        listing,
        producer,
    }
}

/// Publish an asset of its own custom type `asset_type`, owned by `producer`, and answer
/// the asset and the listing that records it.
///
/// One helper rather than one copy per fixture, so every case here subscribes to a listing
/// built the same way — and a second PRODUCING project, here or in
/// [`crate::subscription_request_owner_asks`], gets its own listing through it.
pub(crate) async fn publish_asset_listing(
    dz: &Emulator,
    domain: &str,
    producer: &str,
    asset_type: &str,
) -> (String, String) {
    let created_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(json!({
                "name": asset_type,
                "owningProjectIdentifier": producer,
                "formsInput": {},
            })),
        )
        .await;
    assert_eq!(
        created_type.status, 201,
        "creating asset type {asset_type:?}: expected 201, got {} {:?}",
        created_type.status, created_type.body
    );

    let created_asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": format!("{asset_type}-asset"),
                "typeIdentifier": asset_type,
                "owningProjectIdentifier": producer,
            })),
        )
        .await;
    assert_eq!(
        created_asset.status, 201,
        "creating the asset of {asset_type:?}: expected 201, got {} {:?}",
        created_asset.status, created_asset.body
    );

    let published = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/listings/change-set"),
            Some(json!({
                "entityIdentifier": created_asset.s("id"),
                "entityType": "ASSET",
                "action": "PUBLISH",
            })),
        )
        .await;
    assert_eq!(
        published.status, 200,
        "publishing the asset of {asset_type:?}: expected 200, got {} {:?}",
        published.status, published.body
    );

    (created_asset.s("id"), published.s("listingId"))
}

/// Attribute `project` of `domain` to [`ANOTHER_OWNER`], so the calling identity does not
/// own it.
///
/// The second fixture seam this suite documents at the top, through the shared helper every
/// suite that needs a project the caller does not own uses
/// ([`crate::harness::attribute_project_to`]). What the cases here need from it is only
/// that the asker is not the producing project's owner.
fn hand_to_another_owner(engine: &Arc<engine::InMemoryEngine>, domain: &str, project: &str) {
    crate::harness::attribute_project_to(engine, domain, project, ANOTHER_OWNER);
}

/// Put a subscription target into `catalog`'s domain, applicable to the asset types
/// `applicable_asset_types`.
///
/// The fixture seam this suite documents at the top: `CreateSubscriptionTarget` is still a
/// scaffold, so there is no wire call that records one. The record is exactly what that
/// handler will write.
fn put_subscription_target(
    engine: &Arc<engine::InMemoryEngine>,
    catalog: &Catalog,
    id: &str,
    applicable_asset_types: &[&str],
) {
    let state = service_state(&**engine).expect("the DataZone store is reachable");
    let mut store = state.lock();
    let domain = store
        .domains
        .get_mut(&catalog.domain)
        .expect("the fixture's domain is stored");
    domain.subscription_targets.insert(
        id.to_string(),
        SubscriptionTarget {
            id: id.to_string(),
            domain_id: catalog.domain.clone(),
            environment_id: format!("{id}-environment"),
            project_id: catalog.producer.clone(),
            applicable_asset_types: applicable_asset_types
                .iter()
                .map(|asset_type| (*asset_type).to_string())
                .collect(),
        },
    );
}

/// The ids of the subscriptions `domain` holds.
///
/// The third fixture seam: a case has to learn the id to address `GetSubscription` with,
/// and reading it from the store keeps these cases about the DECISION rather than about
/// how a listing filters and pages (which `ListSubscriptions` has its own suite for). Only
/// the IDS come from here — every assertion about a subscription below is made on the HTTP
/// answer `GetSubscription` gives for it.
fn stored_subscription_ids(engine: &Arc<engine::InMemoryEngine>, domain: &str) -> Vec<String> {
    let state = service_state(&**engine).expect("the DataZone store is reachable");
    let store = state.lock();
    store
        .domains
        .get(domain)
        .map(|domain| domain.subscriptions.keys().cloned().collect())
        .unwrap_or_default()
}

/// Ask for access to `catalog`'s listing for `principal`.
async fn ask_for(dz: &Emulator, catalog: &Catalog, principal: Value, reason: &str) -> Value {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/subscription-requests", catalog.domain),
            Some(json!({
                "subscribedPrincipals": [principal],
                "subscribedListings": [{ "identifier": catalog.listing }],
                "requestReason": reason,
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "asking for access ({reason:?}): expected 200, got {} {:?}",
        created.status, created.body
    );
    created.body
}

/// Ask for access to `catalog`'s listing for its consumer PROJECT.
async fn ask(dz: &Emulator, catalog: &Catalog, reason: &str) -> Value {
    let principal = json!({ "project": { "identifier": catalog.consumer } });
    ask_for(dz, catalog, principal, reason).await
}

/// `GetSubscriptionRequestDetails` on one stored request.
async fn read_back(dz: &Emulator, domain: &str, id: &str) -> Value {
    let got = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/subscription-requests/{id}"),
            None,
        )
        .await;
    assert_eq!(
        got.status, 200,
        "reading request {id}: expected 200, got {} {:?}",
        got.status, got.body
    );
    got.body
}

/// A member of a response body as a string, or `"<absent>"` so a message says which
/// member was missing.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

#[tokio::test]
async fn a_created_request_awaits_a_decision_whatever_targets_the_domain_holds() {
    // The rows differ along what the rule is independent of: whether the domain holds no
    // subscription target at all, one applicable to a DIFFERENT asset type, one
    // applicable to the published type, or several of each.
    let rows: Vec<(&str, Vec<Vec<&str>>)> = vec![
        ("a domain holding no subscription target", vec![]),
        (
            "a target applicable to another asset type only",
            vec![vec!["amazon.datazone.RelationalTable"]],
        ),
        (
            "a target applicable to the published type",
            vec![vec!["PlainCustomType"]],
        ),
        (
            "several targets, one of them applicable",
            vec![
                vec!["amazon.datazone.RelationalTable", "OtherCustomType"],
                vec!["PlainCustomType"],
                vec![],
            ],
        ),
    ];

    for (index, (label, targets)) in rows.into_iter().enumerate() {
        let (dz, engine) = Emulator::with_engine();
        let catalog = catalog(
            &dz,
            &engine,
            &format!("awaits-decision-{index}"),
            "PlainCustomType",
        )
        .await;
        for (slot, applicable) in targets.iter().enumerate() {
            put_subscription_target(&engine, &catalog, &format!("target-{slot}"), applicable);
        }

        let created = ask(&dz, &catalog, "a decision is awaited").await;
        assert_eq!(
            member(&created, "status"),
            SUBSCRIPTION_REQUEST_STATUS_PENDING,
            "case {label:?}: a created request awaits a decision, got {created:?}"
        );
        assert_eq!(
            created.get("reviewerId"),
            None,
            "case {label:?}: nobody has reviewed the request, got {created:?}"
        );
        assert_eq!(
            created.get("decisionComment"),
            None,
            "case {label:?}: no decision carried a comment, got {created:?}"
        );
        assert_eq!(
            created.get("existingSubscriptionId"),
            None,
            "case {label:?}: no subscription is materialised by the ask, got {created:?}"
        );
        assert_eq!(
            stored_subscription_ids(&engine, &catalog.domain),
            Vec::<String>::new(),
            "case {label:?}: the ask granted nothing, so the domain holds no subscription"
        );

        let read = read_back(&dz, &catalog.domain, &member(&created, "id")).await;
        assert_eq!(
            member(&read, "status"),
            SUBSCRIPTION_REQUEST_STATUS_PENDING,
            "case {label:?}: the reader reports the stored status, got {read:?}"
        );
        // The target is a grant's fulfilment path, not a decision: the request that one
        // applies to still takes a decision, and that is what moves it.
        let decided = dz
            .call(
                "PUT",
                &format!(
                    "/v2/domains/{}/subscription-requests/{}/accept",
                    catalog.domain,
                    member(&created, "id")
                ),
                None,
            )
            .await;
        assert_eq!(
            (decided.status, member(&decided.body, "status")),
            (200, SUBSCRIPTION_REQUEST_STATUS_ACCEPTED.to_string()),
            "case {label:?}: the awaited decision is taken, got {} {:?}",
            decided.status,
            decided.body
        );
        assert_eq!(
            catalog.asset_type, "PlainCustomType",
            "case {label:?}: the rows' targets are written against the published type"
        );
    }
}

#[tokio::test]
async fn a_decision_on_a_request_that_awaits_one_is_recorded() {
    // Both decisions, over the same rule: the decision moves the request to its own
    // terminal status and records who took it; the rows differ along which decision is
    // taken and whether it carried a comment.
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "an accept with a comment",
            "accept",
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            Some("approved for the quarterly report"),
        ),
        (
            "an accept with no comment",
            "accept",
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            None,
        ),
        (
            "a reject with a comment",
            "reject",
            SUBSCRIPTION_REQUEST_STATUS_REJECTED,
            Some("the consumer project is out of scope"),
        ),
        (
            "a reject with no comment",
            "reject",
            SUBSCRIPTION_REQUEST_STATUS_REJECTED,
            None,
        ),
    ];

    for (index, (label, decision, expect_status, comment)) in rows.into_iter().enumerate() {
        let (dz, engine) = Emulator::with_engine();
        let catalog = catalog(&dz, &engine, &format!("decision-{index}"), "Fulfillable").await;
        let created = ask(&dz, &catalog, "please grant access").await;
        let id = member(&created, "id");
        assert_eq!(
            member(&created, "status"),
            SUBSCRIPTION_REQUEST_STATUS_PENDING,
            "case {label:?}: the fixture's request must await a decision, got {created:?}"
        );

        let body = comment.map(|comment| json!({ "decisionComment": comment }));
        let decided = dz
            .call(
                "PUT",
                &format!(
                    "/v2/domains/{}/subscription-requests/{id}/{decision}",
                    catalog.domain
                ),
                body,
            )
            .await;
        assert_eq!(
            decided.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            decided.status, decided.body
        );
        assert_eq!(
            member(&decided.body, "status"),
            expect_status,
            "case {label:?}: the decision moves the request, got {:?}",
            decided.body
        );
        assert_eq!(
            member(&decided.body, "reviewerId"),
            CALLER,
            "case {label:?}: the reviewer is the caller that decided, got {:?}",
            decided.body
        );
        assert_eq!(
            member(&decided.body, "updatedBy"),
            CALLER,
            "case {label:?}: the decision is what updated the request, got {:?}",
            decided.body
        );
        assert_eq!(
            decided.body.get("decisionComment").and_then(Value::as_str),
            comment,
            "case {label:?}: the comment the decision carried, got {:?}",
            decided.body
        );
        assert_eq!(
            member(&decided.body, "id"),
            id,
            "case {label:?}: the decision is reported on the request it addressed, got {:?}",
            decided.body
        );
        assert_eq!(
            member(&decided.body, "requestReason"),
            "please grant access",
            "case {label:?}: the ask itself is unchanged, got {:?}",
            decided.body
        );

        // The round trip: the reader reports the decided record, not the ask.
        let read = read_back(&dz, &catalog.domain, &id).await;
        assert_eq!(
            member(&read, "status"),
            expect_status,
            "case {label:?}: the reader reports the decided status, got {read:?}"
        );
        assert_eq!(
            read.get("decisionComment").and_then(Value::as_str),
            comment,
            "case {label:?}: the reader reports the stored comment, got {read:?}"
        );
        assert_eq!(
            member(&read, "reviewerId"),
            CALLER,
            "case {label:?}: the reader reports the stored reviewer, got {read:?}"
        );
        assert_eq!(
            read.get("createdAt"),
            created.get("createdAt"),
            "case {label:?}: a decision does not move createdAt, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_decision_materialises_a_subscription_only_when_it_accepts() {
    // The rows span the class the rule generalises over: which decision is taken, which
    // kind of principal the access was asked for, and whether the accept narrowed the
    // permissions it grants. Every accept materialises exactly one APPROVED subscription;
    // a reject materialises none.
    enum Principal {
        /// The ask names the consumer PROJECT, a DataZone resource of the domain.
        Project,
        /// The ask names an IAM principal, a resource of another service.
        Iam,
    }
    let rows: Vec<(&str, &str, Principal, bool)> = vec![
        ("an accept for a project", "accept", Principal::Project, false),
        ("an accept for an IAM principal", "accept", Principal::Iam, false),
        (
            "an accept narrowing the asset permissions",
            "accept",
            Principal::Project,
            true,
        ),
        ("a reject", "reject", Principal::Project, false),
    ];

    for (index, (label, decision, principal, narrow)) in rows.into_iter().enumerate() {
        let (dz, engine) = Emulator::with_engine();
        let catalog = catalog(&dz, &engine, &format!("materialise-{index}"), "Fulfillable").await;
        let asked_for = match principal {
            Principal::Project => json!({ "project": { "identifier": catalog.consumer } }),
            Principal::Iam => json!({ "iam": { "identifier": CONSUMER_ROLE } }),
        };
        let created = ask_for(&dz, &catalog, asked_for, "access for the report").await;
        let request_id = member(&created, "id");

        let body = narrow.then(|| {
            json!({
                "assetPermissions": [{
                    "assetId": catalog.asset,
                    // `Permissions` is a union with one `s3` arm of S3Permission
                    // (smithy_query shape="Permissions,S3Permission": READ | WRITE).
                    "permissions": { "s3": ["READ"] },
                }],
            })
        });
        let decided = dz
            .call(
                "PUT",
                &format!(
                    "/v2/domains/{}/subscription-requests/{request_id}/{decision}",
                    catalog.domain
                ),
                body,
            )
            .await;
        assert_eq!(
            decided.status, 200,
            "case {label:?}: the decision: expected 200, got {} {:?}",
            decided.status, decided.body
        );

        let ids = stored_subscription_ids(&engine, &catalog.domain);
        if decision == "reject" {
            assert_eq!(
                ids,
                Vec::<String>::new(),
                "case {label:?}: a refused ask grants nothing, so no subscription exists"
            );
            // Nothing holds the request's own id as a subscription either, so a read of
            // it is a miss rather than an invented access.
            let missed = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{}/subscriptions/{request_id}", catalog.domain),
                    None,
                )
                .await;
            assert_eq!(
                (missed.status, missed.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?}: reading a subscription nothing created: got {} {:?}",
                missed.status,
                missed.body
            );
            continue;
        }

        assert_eq!(
            ids.len(),
            1,
            "case {label:?}: an accept materialises exactly one subscription, got {ids:?}"
        );
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{}/subscriptions/{}", catalog.domain, ids[0]),
                None,
            )
            .await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: reading the subscription back: got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            member(&got.body, "status"),
            SUBSCRIPTION_STATUS_APPROVED,
            "case {label:?}: the access the accept granted is approved, got {:?}",
            got.body
        );
        assert_eq!(
            member(&got.body, "id"),
            ids[0],
            "case {label:?}: the subscription reports its own id, got {:?}",
            got.body
        );
        assert_eq!(
            member(&got.body, "subscriptionRequestId"),
            request_id,
            "case {label:?}: it names the request it came from, got {:?}",
            got.body
        );
        assert_eq!(
            member(&got.body, "domainId"),
            catalog.domain,
            "case {label:?}: it is reported in its own domain, got {:?}",
            got.body
        );
        assert_eq!(
            member(&got.body, "createdBy"),
            CALLER,
            "case {label:?}: the caller that decided created the access, got {:?}",
            got.body
        );
        assert_eq!(
            got.body.get("updatedBy"),
            None,
            "case {label:?}: nothing has updated the subscription yet, got {:?}",
            got.body
        );
        assert_eq!(
            got.body.get("createdAt"),
            got.body.get("updatedAt"),
            "case {label:?}: an untouched subscription was last changed when it was made, \
             got {:?}",
            got.body
        );

        // The listing the access is ON: the catalog record the ask was made against.
        let listing = got
            .body
            .get("subscribedListing")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&listing, "id"),
            catalog.listing,
            "case {label:?}: the subscription is on the subscribed listing, got {listing:?}"
        );
        assert_eq!(
            member(&listing, "ownerProjectId"),
            catalog.producer,
            "case {label:?}: the listing is reported with its owning project, got {listing:?}"
        );
        assert_eq!(
            listing
                .pointer("/item/assetListing/entityId")
                .and_then(Value::as_str),
            Some(catalog.asset.as_str()),
            "case {label:?}: the published entity is the asset, got {listing:?}"
        );
        let granted = listing.pointer("/item/assetListing/permissions/s3");
        if narrow {
            assert_eq!(
                granted,
                Some(&json!(["READ"])),
                "case {label:?}: the subscription reports what the decision narrowed the \
                 access to, got {listing:?}"
            );
        } else {
            assert_eq!(
                granted, None,
                "case {label:?}: no decision narrowed the access, got {listing:?}"
            );
        }

        // The principal the access is HELD BY, in the arm the ask named.
        let subscribed = got
            .body
            .get("subscribedPrincipal")
            .cloned()
            .unwrap_or(Value::Null);
        match principal {
            Principal::Project => assert_eq!(
                subscribed.pointer("/project/id").and_then(Value::as_str),
                Some(catalog.consumer.as_str()),
                "case {label:?}: the project that asked holds the access, got {subscribed:?}"
            ),
            Principal::Iam => assert_eq!(
                subscribed.pointer("/iam/principalArn").and_then(Value::as_str),
                Some(CONSUMER_ROLE),
                "case {label:?}: the IAM principal that asked holds the access, got \
                 {subscribed:?}"
            ),
        }

        // The decided request still reports itself as accepted, and reading the
        // subscription again answers the same record — a read changes nothing.
        let read = read_back(&dz, &catalog.domain, &request_id).await;
        assert_eq!(
            member(&read, "status"),
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            "case {label:?}: the accepted request reads back accepted, got {read:?}"
        );
        let again = dz
            .call(
                "GET",
                &format!("/v2/domains/{}/subscriptions/{}", catalog.domain, ids[0]),
                None,
            )
            .await;
        assert_eq!(
            (again.status, again.body),
            (got.status, got.body),
            "case {label:?}: a second read answers the same subscription"
        );
        assert_eq!(
            stored_subscription_ids(&engine, &catalog.domain),
            ids,
            "case {label:?}: reading created no further subscription"
        );
    }
}

#[tokio::test]
async fn a_decision_on_a_request_that_is_already_decided_is_refused() {
    // The rows span the statuses a decided request can carry — one an accept took, one a
    // reject took — and both decision operations against each. Each refusal names its own
    // message, the stored record is untouched afterwards, and no further subscription is
    // materialised.
    let already: Vec<(&str, &str, &str, Option<&str>, usize)> = vec![
        (
            "a request an accept decided",
            "accept",
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            Some("already approved"),
            1,
        ),
        (
            "a request a reject decided",
            "reject",
            SUBSCRIPTION_REQUEST_STATUS_REJECTED,
            Some("already refused"),
            0,
        ),
    ];

    for (index, (label, decided_by, expect_status, comment, subscriptions)) in
        already.into_iter().enumerate()
    {
        let (dz, engine) = Emulator::with_engine();
        let catalog = catalog(
            &dz,
            &engine,
            &format!("already-decided-{index}"),
            "Fulfillable",
        )
        .await;
        let created = ask(&dz, &catalog, "the original ask").await;
        let id = member(&created, "id");
        let decided = dz
            .call(
                "PUT",
                &format!(
                    "/v2/domains/{}/subscription-requests/{id}/{decided_by}",
                    catalog.domain
                ),
                comment.map(|comment| json!({ "decisionComment": comment })),
            )
            .await;
        assert_eq!(
            decided.status, 200,
            "case {label:?}: the fixture's {decided_by}: got {} {:?}",
            decided.status, decided.body
        );
        let held = stored_subscription_ids(&engine, &catalog.domain);
        assert_eq!(
            held.len(),
            subscriptions,
            "case {label:?}: the first decision's own effect on the access, got {held:?}"
        );

        let refusals: Vec<(&str, String)> = vec![
            ("accept", SUBSCRIPTION_ALREADY_DECIDED_MESSAGE.to_string()),
            ("reject", cannot_reject_message(expect_status)),
        ];
        for (decision, expect_message) in refusals {
            let refused = dz
                .call(
                    "PUT",
                    &format!(
                        "/v2/domains/{}/subscription-requests/{id}/{decision}",
                        catalog.domain
                    ),
                    Some(json!({ "decisionComment": "a second opinion" })),
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (400, "ValidationException"),
                "case {label:?}: a {decision} on it: got {} {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                member(&refused.body, "message"),
                expect_message,
                "case {label:?}: the {decision}'s refusal message, got {:?}",
                refused.body
            );

            // The refusal wrote nothing: the status, the comment and the access granted
            // are as they were.
            let read = read_back(&dz, &catalog.domain, &id).await;
            assert_eq!(
                member(&read, "status"),
                expect_status,
                "case {label:?}: the refused {decision} left the status alone, got {read:?}"
            );
            assert_eq!(
                read.get("decisionComment").and_then(Value::as_str),
                comment,
                "case {label:?}: the refused {decision} left the comment alone, got {read:?}"
            );
            assert_eq!(
                stored_subscription_ids(&engine, &catalog.domain),
                held,
                "case {label:?}: the refused {decision} materialised no subscription"
            );
        }
    }
}

#[tokio::test]
async fn a_decision_addressing_nothing_this_store_holds_is_refused() {
    let (dz, engine) = Emulator::with_engine();
    let catalog = catalog(&dz, &engine, "decision-refusals", "Fulfillable").await;
    let pending = member(&ask(&dz, &catalog, "a pending ask").await, "id");

    // (label, domain, request id, expected status, expected error code)
    let rows: Vec<(&str, String, String, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an identifier
            // the store serves no domain for: authorization decides the call before any
            // lookup.
            "a domain nothing created",
            "dzd_doesnotexist".to_string(),
            pending.clone(),
            403,
            "AccessDeniedException",
        ),
        (
            "a request id nothing recorded",
            catalog.domain.clone(),
            "nosuchrequest".to_string(),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (label, domain, id, status, code) in rows {
        for decision in ["accept", "reject"] {
            let refused = dz
                .call(
                    "PUT",
                    &format!("/v2/domains/{domain}/subscription-requests/{id}/{decision}"),
                    None,
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (status, code),
                "case {label:?} on {decision}: got {} {:?}",
                refused.status,
                refused.body
            );
        }
        // A subscription is read the same way: the denial and the miss are decided
        // before any access is resolved, and neither reports one.
        let read = dz
            .call("GET", &format!("/v2/domains/{domain}/subscriptions/{id}"), None)
            .await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (status, code),
            "case {label:?} on a subscription read: got {} {:?}",
            read.status,
            read.body
        );
    }

    // Every refusal left the request alone: it still awaits a decision, and still takes
    // one — which is what materialises the access.
    let read = read_back(&dz, &catalog.domain, &pending).await;
    assert_eq!(
        member(&read, "status"),
        SUBSCRIPTION_REQUEST_STATUS_PENDING,
        "the refusals left the request awaiting a decision, got {read:?}"
    );
    assert_eq!(
        stored_subscription_ids(&engine, &catalog.domain),
        Vec::<String>::new(),
        "no refused call granted any access"
    );
    let decided = dz
        .call(
            "PUT",
            &format!(
                "/v2/domains/{}/subscription-requests/{pending}/accept",
                catalog.domain
            ),
            None,
        )
        .await;
    assert_eq!(
        decided.status, 200,
        "an accept after the refusals: got {} {:?}",
        decided.status, decided.body
    );
    let ids = stored_subscription_ids(&engine, &catalog.domain);
    assert_eq!(
        ids.len(),
        1,
        "the accept after the refusals granted the access, got {ids:?}"
    );
}

/// A `UserProfileId`-shaped identifier no principal in these fixtures carries, used to show
/// that an owning filter addressing ANOTHER arm of the principal union keeps nothing. The
/// shape is the one the model declares for the member (smithy_query query="operation"
/// operation="ListSubscriptions").
const UNHELD_USER_ID: &str = "4f1a2b3c-1111-2222-3333-444455556666";

/// A `GroupProfileId`-shaped identifier no principal in these fixtures carries, for the same
/// reason.
const UNHELD_GROUP_ID: &str = "5e2b3c4d-9999-8888-7777-666655554444";

/// `ListSubscriptions` with the query string `query` (empty for none).
async fn list_subscriptions(dz: &Emulator, domain: &str, query: &str) -> crate::harness::Answer {
    let uri = if query.is_empty() {
        format!("/v2/domains/{domain}/subscriptions")
    } else {
        format!("/v2/domains/{domain}/subscriptions?{query}")
    };
    dz.call("GET", &uri, None).await
}

/// The `items` of a listing answer.
fn items(body: &Value) -> Vec<Value> {
    body.get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `id` of every item of a listing answer, in the order they were reported.
fn item_ids(body: &Value) -> Vec<String> {
    items(body).iter().map(|item| member(item, "id")).collect()
}

/// Ids as a SET: the filter rows are about WHICH subscriptions a filter keeps, not about
/// their order, which has its own case.
fn id_set(ids: &[String]) -> BTreeSet<String> {
    ids.iter().cloned().collect()
}

/// The subscription the accept of `request` materialised, found through the listing filtered
/// by that request — the wire call that names it.
async fn subscription_of(dz: &Emulator, domain: &str, request: &str) -> String {
    let listed = list_subscriptions(
        dz,
        domain,
        &format!("subscriptionRequestIdentifier={request}"),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "listing the subscriptions of request {request}: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let ids = item_ids(&listed.body);
    assert_eq!(
        ids.len(),
        1,
        "an accepted request materialises exactly one subscription, got {ids:?}"
    );
    ids[0].clone()
}

/// `GetSubscription` on one stored subscription.
async fn read_subscription(dz: &Emulator, domain: &str, id: &str) -> crate::harness::Answer {
    dz.call(
        "GET",
        &format!("/v2/domains/{domain}/subscriptions/{id}"),
        None,
    )
    .await
}

/// Ask for access to `listing` of `domain` for `principal` and accept it, answering the
/// request's id and the subscription the accept materialised.
async fn granted_access(
    dz: &Emulator,
    domain: &str,
    listing: &str,
    principal: Value,
    reason: &str,
) -> (String, String) {
    let asked = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/subscription-requests"),
            Some(json!({
                "subscribedPrincipals": [principal],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": reason,
            })),
        )
        .await;
    assert_eq!(
        asked.status, 200,
        "asking for access ({reason:?}): expected 200, got {} {:?}",
        asked.status, asked.body
    );
    let request = asked.s("id");
    let decided = dz
        .call(
            "PUT",
            &format!("/v2/domains/{domain}/subscription-requests/{request}/accept"),
            None,
        )
        .await;
    assert_eq!(
        decided.status, 200,
        "accepting the ask ({reason:?}): expected 200, got {} {:?}",
        decided.status, decided.body
    );
    let subscription = subscription_of(dz, domain, &request).await;
    (request, subscription)
}

/// The fixture the access cases read: a domain holding TWO producing projects, each with a
/// listing of its own, and four granted accesses plus one ask a reject turned down.
struct Granted {
    domain: String,
    producer_a: String,
    producer_b: String,
    consumer: String,
    listing_a: String,
    listing_b: String,
    /// The request whose accept materialised [`Self::project_on_a`].
    request_project_on_a: String,
    /// The request a reject turned down, so it materialised nothing.
    request_rejected: String,
    /// The consumer PROJECT's access to producer A's listing.
    project_on_a: String,
    /// An IAM principal's access to the same listing.
    iam_on_a: String,
    /// The consumer project's access to producer B's listing.
    project_on_b: String,
    /// The `UserProfileId` of a user profile this domain holds, which holds
    /// [`Self::user_on_b`]. A GROUP principal has no counterpart here: a group profile is
    /// created by `CreateGroupProfile`, which is still a scaffold, so no domain holds one
    /// and only the negative direction of `owningGroupId` is reachable.
    user: String,
    /// That user profile's access to producer B's listing.
    user_on_b: String,
}

async fn granted(dz: &Emulator, engine: &Arc<engine::InMemoryEngine>, name: &str) -> Granted {
    let catalog = catalog(dz, engine, name, "AccessTypeA").await;
    let producer_b = dz.project(&catalog.domain, "producer-b").await;
    let (_, listing_b) =
        publish_asset_listing(dz, &catalog.domain, &producer_b, "AccessTypeB").await;
    // Both producing projects belong to somebody else, so every ask below awaits the
    // decision these cases then take on it.
    hand_to_another_owner(engine, &catalog.domain, &producer_b);

    let as_project = json!({ "project": { "identifier": catalog.consumer } });
    let as_iam = json!({ "iam": { "identifier": CONSUMER_ROLE } });

    let (request_project_on_a, project_on_a) = granted_access(
        dz,
        &catalog.domain,
        &catalog.listing,
        as_project.clone(),
        "the project on A",
    )
    .await;
    let (_, iam_on_a) = granted_access(
        dz,
        &catalog.domain,
        &catalog.listing,
        as_iam,
        "the role on A",
    )
    .await;
    let (_, project_on_b) = granted_access(
        dz,
        &catalog.domain,
        &listing_b,
        as_project.clone(),
        "the project on B",
    )
    .await;

    // A USER principal: a profile of this domain, addressed by the `UserProfileId` the
    // create reports, so the `owningUserId` arm has a subscription to keep as well as one
    // to leave out.
    let profile = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/user-profiles", catalog.domain),
            Some(json!({ "userIdentifier": "access.consumer", "userType": "IAM_USER" })),
        )
        .await;
    assert_eq!(
        profile.status, 201,
        "creating the consuming user profile: expected 201, got {} {:?}",
        profile.status, profile.body
    );
    let user = profile.s("id");
    let (_, user_on_b) = granted_access(
        dz,
        &catalog.domain,
        &listing_b,
        json!({ "user": { "identifier": user } }),
        "the user on B",
    )
    .await;

    let request_rejected = member(
        &ask_for(dz, &catalog, as_project, "the ask that is turned down").await,
        "id",
    );
    let rejected = dz
        .call(
            "PUT",
            &format!(
                "/v2/domains/{}/subscription-requests/{request_rejected}/reject",
                catalog.domain
            ),
            None,
        )
        .await;
    assert_eq!(
        rejected.status, 200,
        "rejecting the fixture's ask: expected 200, got {} {:?}",
        rejected.status, rejected.body
    );

    Granted {
        domain: catalog.domain,
        producer_a: catalog.producer,
        producer_b,
        consumer: catalog.consumer,
        listing_a: catalog.listing,
        listing_b,
        request_project_on_a,
        request_rejected,
        project_on_a,
        iam_on_a,
        project_on_b,
        user,
        user_on_b,
    }
}

#[tokio::test]
async fn a_listing_reports_the_subscriptions_every_named_filter_keeps() {
    let (dz, engine) = Emulator::with_engine();
    let held = granted(&dz, &engine, "access-listing-filters").await;
    let every = vec![
        held.project_on_a.clone(),
        held.iam_on_a.clone(),
        held.project_on_b.clone(),
        held.user_on_b.clone(),
    ];

    // The rows differ along which filter the request names, which arm of the principal
    // union it addresses, whether it names one filter or two at once, and whether what it
    // names resolves to a subscription at all.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter at all, so the default status",
            String::new(),
            every.clone(),
        ),
        (
            "the request that materialised one subscription",
            format!(
                "subscriptionRequestIdentifier={}",
                held.request_project_on_a
            ),
            vec![held.project_on_a.clone()],
        ),
        (
            "a request a reject turned down, which granted nothing",
            format!("subscriptionRequestIdentifier={}", held.request_rejected),
            Vec::new(),
        ),
        (
            "the listing two principals hold access to",
            format!("subscribedListingId={}", held.listing_a),
            vec![held.project_on_a.clone(), held.iam_on_a.clone()],
        ),
        (
            "the other producer's listing",
            format!("subscribedListingId={}", held.listing_b),
            vec![held.project_on_b.clone(), held.user_on_b.clone()],
        ),
        (
            "the project that holds two of the accesses",
            format!("owningProjectId={}", held.consumer),
            vec![held.project_on_a.clone(), held.project_on_b.clone()],
        ),
        (
            "a project that holds none of them",
            format!("owningProjectId={}", held.producer_a),
            Vec::new(),
        ),
        (
            "the IAM principal that holds one",
            format!("owningIamPrincipalArn={}", urlencode(CONSUMER_ROLE)),
            vec![held.iam_on_a.clone()],
        ),
        (
            "the user profile that holds one",
            format!("owningUserId={}", urlencode(&held.user)),
            vec![held.user_on_b.clone()],
        ),
        (
            "a user profile arm no principal here is",
            format!("owningUserId={UNHELD_USER_ID}"),
            Vec::new(),
        ),
        (
            "a group profile arm no principal here is",
            format!("owningGroupId={UNHELD_GROUP_ID}"),
            Vec::new(),
        ),
        (
            "the project that published the listing two accesses are on",
            format!("approverProjectId={}", held.producer_a),
            vec![held.project_on_a.clone(), held.iam_on_a.clone()],
        ),
        (
            "the project that published the other listing",
            format!("approverProjectId={}", held.producer_b),
            vec![held.project_on_b.clone(), held.user_on_b.clone()],
        ),
        (
            "a project that published nothing subscribed to",
            format!("approverProjectId={}", held.consumer),
            Vec::new(),
        ),
        (
            "the status every access carries",
            format!("status={SUBSCRIPTION_STATUS_APPROVED}"),
            every.clone(),
        ),
        (
            "a status no access carries yet",
            format!("status={SUBSCRIPTION_STATUS_REVOKED}"),
            Vec::new(),
        ),
        (
            "the status a cancel would leave",
            format!("status={SUBSCRIPTION_STATUS_CANCELLED}"),
            Vec::new(),
        ),
        (
            "two filters at once, which keeps their intersection",
            format!(
                "owningProjectId={}&subscribedListingId={}",
                held.consumer, held.listing_a
            ),
            vec![held.project_on_a.clone()],
        ),
        (
            "two filters at once whose intersection is empty",
            format!(
                "owningProjectId={}&subscribedListingId={}",
                held.producer_a, held.listing_a
            ),
            Vec::new(),
        ),
    ];

    for (label, query, expected) in rows {
        let listed = list_subscriptions(&dz, &held.domain, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            id_set(&item_ids(&listed.body)),
            id_set(&expected),
            "case {label:?}: expected {expected:?}, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body.get("nextToken"),
            None,
            "case {label:?}: one page holds every kept subscription, got {:?}",
            listed.body
        );
    }
}

/// Every member a reported summary carries is the stored subscription's own, and the reader
/// that did not write it answers the same.
#[tokio::test]
async fn a_reported_summary_is_the_stored_subscription() {
    let (dz, engine) = Emulator::with_engine();
    let held = granted(&dz, &engine, "access-listing-members").await;
    let listed = list_subscriptions(&dz, &held.domain, "").await;

    let rows: Vec<(&str, String, String, Value)> = vec![
        (
            "the project's access to producer A's listing",
            held.project_on_a.clone(),
            held.listing_a.clone(),
            json!({ "project": { "id": held.consumer, "name": "consumer" } }),
        ),
        (
            "the role's access to the same listing",
            held.iam_on_a.clone(),
            held.listing_a.clone(),
            json!({ "iam": { "principalArn": CONSUMER_ROLE } }),
        ),
        (
            "the project's access to producer B's listing",
            held.project_on_b.clone(),
            held.listing_b.clone(),
            json!({ "project": { "id": held.consumer, "name": "consumer" } }),
        ),
    ];

    for (label, id, listing, owner) in rows {
        let item = items(&listed.body)
            .into_iter()
            .find(|item| member(item, "id") == id)
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the listing reports {id}, got {:?}",
                    listed.body
                )
            });
        assert_eq!(
            member(&item, "status"),
            SUBSCRIPTION_STATUS_APPROVED,
            "case {label:?}: an accepted ask grants the access, got {item:?}"
        );
        assert_eq!(
            item.get("subscribedListing")
                .map(|listing| member(listing, "id")),
            Some(listing),
            "case {label:?}: the access is on the listing the ask recorded, got {item:?}"
        );
        assert_eq!(
            item.get("subscribedPrincipal"),
            Some(&owner),
            "case {label:?}: the access is held by the principal the ask named, got {item:?}"
        );
        assert_eq!(
            item.get("retainPermissions"),
            None,
            "case {label:?}: nothing has given the access up, got {item:?}"
        );
        assert_eq!(
            item.get("updatedBy"),
            None,
            "case {label:?}: nothing has updated the access, got {item:?}"
        );

        // The reader that did not write it answers the same record, member for member.
        let read = read_subscription(&dz, &held.domain, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: reading {id}: expected 200, got {} {:?}",
            read.status, read.body
        );
        for name in [
            "id",
            "status",
            "createdBy",
            "createdAt",
            "updatedAt",
            "domainId",
            "subscribedListing",
            "subscribedPrincipal",
            "subscriptionRequestId",
        ] {
            assert_eq!(
                read.body.get(name),
                item.get(name),
                "case {label:?}: the listing and the read agree about {name}, got {:?}",
                read.body
            );
        }
    }
}

/// Revoking an access that still stands moves it to `REVOKED` and records what the call
/// carried; a repeat leaves the record exactly where the first one left it.
#[tokio::test]
async fn a_revocation_takes_the_access_away_and_is_idempotent() {
    // The rows differ along the `retainPermissions` the call carries — the one member a
    // revoke takes — including omitting it altogether.
    let rows: Vec<(&str, Option<bool>)> = vec![
        ("a revoke retaining the permissions", Some(true)),
        ("a revoke not retaining them", Some(false)),
        ("a revoke naming no retainPermissions at all", None),
    ];

    for (index, (label, retain)) in rows.into_iter().enumerate() {
        let (dz, engine) = Emulator::with_engine();
        let held = granted(&dz, &engine, &format!("access-revocation-{index}")).await;
        let id = held.project_on_b.clone();
        let before = read_subscription(&dz, &held.domain, &id).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the access stands before the revoke, got {} {:?}",
            before.status, before.body
        );

        let revoked = dz
            .call(
                "PUT",
                &format!("/v2/domains/{}/subscriptions/{id}/revoke", held.domain),
                retain.map(|retain| json!({ "retainPermissions": retain })),
            )
            .await;
        assert_eq!(
            revoked.status, 200,
            "case {label:?}: the revoke: expected 200, got {} {:?}",
            revoked.status, revoked.body
        );
        assert_eq!(
            revoked.s("status"),
            SUBSCRIPTION_STATUS_REVOKED,
            "case {label:?}: the access is given up, got {:?}",
            revoked.body
        );
        assert_eq!(
            revoked
                .body
                .get("retainPermissions")
                .and_then(Value::as_bool),
            retain,
            "case {label:?}: the revoke records what it carried, got {:?}",
            revoked.body
        );
        assert_eq!(
            revoked.s("updatedBy"),
            CALLER,
            "case {label:?}: whoever revoked it is who updated it, got {:?}",
            revoked.body
        );
        assert_eq!(
            revoked.body.get("createdAt"),
            before.body.get("createdAt"),
            "case {label:?}: a revoke does not move createdAt, got {:?}",
            revoked.body
        );
        assert_eq!(
            revoked.body.get("subscriptionRequestId"),
            before.body.get("subscriptionRequestId"),
            "case {label:?}: the access still names the request it came from, got {:?}",
            revoked.body
        );

        // The reader that did not write it reports exactly the revoked record.
        let read = read_subscription(&dz, &held.domain, &id).await;
        assert_eq!(
            (read.status, read.body),
            (200, revoked.body.clone()),
            "case {label:?}: the read answers the stored revocation"
        );

        // The listing follows the status: the default listing no longer holds it, and the
        // one naming REVOKED holds exactly it.
        let standing = list_subscriptions(&dz, &held.domain, "").await;
        assert_eq!(
            id_set(&item_ids(&standing.body)),
            id_set(&[
                held.project_on_a.clone(),
                held.iam_on_a.clone(),
                held.user_on_b.clone(),
            ]),
            "case {label:?}: the default listing reports only what still stands, got {:?}",
            standing.body
        );
        let given_up = list_subscriptions(
            &dz,
            &held.domain,
            &format!("status={SUBSCRIPTION_STATUS_REVOKED}"),
        )
        .await;
        assert_eq!(
            item_ids(&given_up.body),
            vec![id.clone()],
            "case {label:?}: the revoked listing reports exactly the revoked access, got {:?}",
            given_up.body
        );

        // A repeat revoke, carrying the opposite member, leaves the record where the first
        // one left it: the operation is idempotent.
        let again = dz
            .call(
                "PUT",
                &format!("/v2/domains/{}/subscriptions/{id}/revoke", held.domain),
                Some(json!({ "retainPermissions": !retain.unwrap_or(false) })),
            )
            .await;
        assert_eq!(
            (again.status, again.body),
            (200, revoked.body),
            "case {label:?}: a second revoke answers the same record"
        );
    }
}

/// The other side of that rule: an access given up some OTHER way has no revocation left
/// to take either, and the ask conflicts with the status it carries.
#[tokio::test]
async fn a_revoke_of_an_access_given_up_another_way_is_refused() {
    let (dz, engine) = Emulator::with_engine();
    let held = granted(&dz, &engine, "access-revoke-conflict").await;
    let id = held.project_on_b.clone();

    // A third fixture seam, for the same reason as the other two: `CancelSubscription` is
    // still a scaffold, so no wire call leaves a subscription CANCELLED and the status has
    // to be put there for the conflict this operation models to be reachable at all.
    {
        let state = service_state(&*engine).expect("the DataZone store is reachable");
        let mut store = state.lock();
        store
            .domains
            .get_mut(&held.domain)
            .expect("the fixture's domain is stored")
            .subscriptions
            .get_mut(&id)
            .expect("the fixture's subscription is stored")
            .status = SUBSCRIPTION_STATUS_CANCELLED.to_string();
    }

    let refused = dz
        .call(
            "PUT",
            &format!("/v2/domains/{}/subscriptions/{id}/revoke", held.domain),
            Some(json!({ "retainPermissions": true })),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "revoking a cancelled access: expected 409 ConflictException, got {} {:?} {:?}",
        refused.status,
        refused.error_code,
        refused.body
    );
    assert_eq!(
        refused.s("message"),
        cannot_revoke_message(&id, SUBSCRIPTION_STATUS_CANCELLED),
        "the refusal names the subscription and the status it carries, got {:?}",
        refused.body
    );

    // The refused revoke changed nothing about the access it addressed.
    let read = read_subscription(&dz, &held.domain, &id).await;
    assert_eq!(
        (read.status, read.s("status")),
        (200, SUBSCRIPTION_STATUS_CANCELLED.to_string()),
        "the refused revoke left the status alone, got {:?}",
        read.body
    );
    assert_eq!(
        read.body.get("retainPermissions"),
        None,
        "the refused revoke recorded nothing, got {:?}",
        read.body
    );
    assert_eq!(
        read.body.get("updatedBy"),
        None,
        "the refused revoke updated nobody, got {:?}",
        read.body
    );
}

/// The order a listing reports, and the pages `maxResults` / `nextToken` cut it into.
#[tokio::test]
async fn a_listing_pages_over_one_order() {
    let (dz, engine) = Emulator::with_engine();
    let catalog = catalog(&dz, &engine, "access-listing-pages", "PagedType").await;

    // Five accesses on the one listing, so the order and the pages have something to cut.
    let mut granted_ids = Vec::new();
    for index in 0..5u32 {
        let (_, id) = granted_access(
            &dz,
            &catalog.domain,
            &catalog.listing,
            json!({ "project": { "identifier": catalog.consumer } }),
            &format!("ask {index}"),
        )
        .await;
        granted_ids.push(id);
    }

    let ascending = list_subscriptions(&dz, &catalog.domain, "").await;
    let ordered = item_ids(&ascending.body);
    assert_eq!(
        id_set(&ordered),
        id_set(&granted_ids),
        "the listing reports every access the accepts granted, got {ordered:?}"
    );
    // The order is by `updatedAt`, so the reported instants never go backwards.
    let instants: Vec<f64> = items(&ascending.body)
        .iter()
        .map(|item| item.get("updatedAt").and_then(Value::as_f64).unwrap_or(0.0))
        .collect();
    assert!(
        instants.windows(2).all(|pair| pair[0] <= pair[1]),
        "the ascending order never goes backwards in updatedAt, got {instants:?}"
    );

    let descending = list_subscriptions(&dz, &catalog.domain, "sortOrder=DESCENDING").await;
    let mut reversed = ordered.clone();
    reversed.reverse();
    assert_eq!(
        item_ids(&descending.body),
        reversed,
        "DESCENDING walks the same order backwards, got {:?}",
        descending.body
    );

    // Each page size cuts the SAME order into pages that concatenate back to it, and the
    // last page carries no token.
    for page_size in [1usize, 2, 5, 50] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let page = list_subscriptions(&dz, &catalog.domain, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let ids = item_ids(&page.body);
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page holds at most that many, got {ids:?}"
            );
            collected.extend(ids);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: the pages end, got {collected:?}"
            );
            match page.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, ordered,
            "case page size {page_size}: the pages concatenate to the one order"
        );
    }
}

/// Every refusal the two access operations owe: a domain this store serves none for is
/// denied before any lookup, a subscription the domain does not hold is a miss, and a token
/// this service never issued is invalid input.
#[tokio::test]
async fn an_access_call_addressing_nothing_this_store_holds_is_refused() {
    let (dz, engine) = Emulator::with_engine();
    let held = granted(&dz, &engine, "access-refusals").await;

    // (label, method, uri, expected status, expected error code)
    let rows: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "a listing of a domain nothing created",
            "GET",
            "/v2/domains/dzd_doesnotexist/subscriptions".to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "a revoke in a domain nothing created",
            "PUT",
            format!(
                "/v2/domains/dzd_doesnotexist/subscriptions/{}/revoke",
                held.project_on_a
            ),
            403,
            "AccessDeniedException",
        ),
        (
            "a revoke of a subscription the domain does not hold",
            "PUT",
            format!(
                "/v2/domains/{}/subscriptions/notasubscription/revoke",
                held.domain
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a revoke addressing the REQUEST's id rather than the subscription's",
            "PUT",
            format!(
                "/v2/domains/{}/subscriptions/{}/revoke",
                held.domain, held.request_project_on_a
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a listing resuming from a token this service never issued",
            "GET",
            format!(
                "/v2/domains/{}/subscriptions?nextToken=notatoken",
                held.domain
            ),
            400,
            "ValidationException",
        ),
    ];

    for (label, method, uri, status, code) in rows {
        let refused = dz.call(method, &uri, None).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
    }

    // Every refusal left the accesses alone: each still stands.
    let listed = list_subscriptions(&dz, &held.domain, "").await;
    assert_eq!(
        id_set(&item_ids(&listed.body)),
        id_set(&[
            held.project_on_a.clone(),
            held.iam_on_a.clone(),
            held.project_on_b.clone(),
            held.user_on_b.clone(),
        ]),
        "the refusals granted and revoked nothing, got {:?}",
        listed.body
    );
}

/// Grant `count` accesses to `catalog`'s consumer project on `catalog`'s listing, and answer
/// the one order the listing reports them in.
///
/// The order comes from the WIRE rather than from the sequence the accesses were granted in:
/// the key the listing pages over is the instant a subscription was last changed and then its
/// own id, so accesses granted inside one millisecond are ordered by id, which is not the
/// order they were created in.
async fn accesses_in_reported_order(
    dz: &Emulator,
    catalog: &Catalog,
    count: u32,
) -> Vec<String> {
    for index in 0..count {
        granted_access(
            dz,
            &catalog.domain,
            &catalog.listing,
            json!({ "project": { "identifier": catalog.consumer } }),
            &format!("windowed ask {index}"),
        )
        .await;
    }
    let listed = list_subscriptions(dz, &catalog.domain, "").await;
    assert_eq!(
        listed.status, 200,
        "listing the granted accesses: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let order = item_ids(&listed.body);
    assert_eq!(
        order.len(),
        count as usize,
        "every one of the {count} accepts granted an access the listing reports, got {order:?}"
    );
    order
}

/// The page sizes a listing admits are exactly the modelled `maxResults` range (`@range` min
/// 1, max 50 on the member — smithy_query query="operation" operation="ListSubscriptions"): a
/// size inside it is served a page of at most that many, with a token exactly while something
/// is left over, and a size one past either published bound is refused as invalid input with
/// no page reported at all.
#[tokio::test]
async fn a_listing_admits_exactly_the_modelled_page_sizes() {
    let (dz, engine) = Emulator::with_engine();
    let catalog = catalog(&dz, &engine, "access-page-size-bounds", "BoundedType").await;
    // Three accesses, so the bottom of the range leaves something over while the top of it
    // holds every one of them.
    let order = accesses_in_reported_order(&dz, &catalog, 3).await;

    // (label, the page size asked for, how many the page reports, whether a token follows)
    let served: Vec<(&str, u32, usize, bool)> = vec![
        ("the bottom of the published range", 1, 1, true),
        ("a size inside the range", 2, 2, true),
        ("the top of the published range", 50, 3, false),
    ];
    for (label, size, reported, token) in served {
        let page = list_subscriptions(&dz, &catalog.domain, &format!("maxResults={size}")).await;
        assert_eq!(
            page.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            page.status, page.body
        );
        assert_eq!(
            item_ids(&page.body),
            order[..reported].to_vec(),
            "case {label:?}: expected the first {reported} of the one order, got {:?}",
            page.body
        );
        assert_eq!(
            page.body.get("nextToken").is_some(),
            token,
            "case {label:?}: a token follows exactly while something is left over, got {:?}",
            page.body
        );
    }

    // One past each published bound of that same range.
    let refused: Vec<(&str, u32)> = vec![
        ("one below the bottom of the range", 0),
        ("one above the top of the range", 51),
    ];
    for (label, size) in refused {
        let answer = list_subscriptions(&dz, &catalog.domain, &format!("maxResults={size}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected 400 ValidationException, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
        assert_eq!(
            answer.body.get("items"),
            None,
            "case {label:?}: a refused page size reports no page at all, got {:?}",
            answer.body
        );
    }

    // The refused sizes consumed nothing: the whole order is still there to report.
    let after = list_subscriptions(&dz, &catalog.domain, "").await;
    assert_eq!(
        item_ids(&after.body),
        order,
        "the refused page sizes left the one order exactly as it was, got {:?}",
        after.body
    );
}

/// The order is not the caller's to pick: `sortBy` is `@deprecated` with "Results are always
/// sorted by updatedAt" (smithy_query query="operation" operation="ListSubscriptions"), so
/// every member of it answers the one order, and only `sortOrder` turns that order round.
#[tokio::test]
async fn a_listing_reports_one_order_whatever_sort_by_names() {
    let (dz, engine) = Emulator::with_engine();
    let catalog = catalog(&dz, &engine, "access-listing-sort-key", "SortedType").await;
    // Three accesses, so an order has two steps to get wrong.
    let ascending = accesses_in_reported_order(&dz, &catalog, 3).await;
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    // The rows span the class: each member of `SortKey`, alone and with each member of
    // `SortOrder` named beside it.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no sort key at all",
            String::new(),
            ascending.clone(),
        ),
        (
            "the sort key naming the instant of creation",
            "sortBy=CREATED_AT".to_string(),
            ascending.clone(),
        ),
        (
            "the sort key naming the instant of the last change",
            "sortBy=UPDATED_AT".to_string(),
            ascending.clone(),
        ),
        (
            "a sort key with the ascending order named beside it",
            "sortBy=CREATED_AT&sortOrder=ASCENDING".to_string(),
            ascending.clone(),
        ),
        (
            "a sort key with the descending order named beside it",
            "sortBy=CREATED_AT&sortOrder=DESCENDING".to_string(),
            descending.clone(),
        ),
        (
            "the other sort key with the descending order",
            "sortBy=UPDATED_AT&sortOrder=DESCENDING".to_string(),
            descending.clone(),
        ),
    ];
    for (label, query, expected) in rows {
        let listed = list_subscriptions(&dz, &catalog.domain, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            item_ids(&listed.body),
            expected,
            "case {label:?}: expected {expected:?}, got {:?}",
            listed.body
        );
    }

    // The pages a sort key is named on cut that same one order, so naming it changes
    // nothing about which subscription a page holds either.
    let first = list_subscriptions(&dz, &catalog.domain, "sortBy=CREATED_AT&maxResults=1").await;
    assert_eq!(
        item_ids(&first.body),
        ascending[..1].to_vec(),
        "a page of one under a sort key holds the first of the one order, got {:?}",
        first.body
    );
}

/// A page resumes after the subscription the page before it ENDED ON, located by where that
/// subscription sits in the order rather than by how many were already reported — so a
/// change to the stored accesses between two pages cannot make the next page skip or repeat
/// a different one.
#[tokio::test]
async fn a_later_page_resumes_after_the_access_the_page_before_it_ended_on() {
    let (dz, engine) = Emulator::with_engine();
    let catalog = catalog(&dz, &engine, "access-page-resume", "ResumedType").await;
    // Five accesses, so a page of two leaves three over and the resume has somewhere to go
    // wrong.
    let order = accesses_in_reported_order(&dz, &catalog, 5).await;

    let first = list_subscriptions(&dz, &catalog.domain, "maxResults=2").await;
    assert_eq!(
        item_ids(&first.body),
        order[..2].to_vec(),
        "the first page holds the first two of the one order, got {:?}",
        first.body
    );
    let mut token = first
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    assert!(
        token.is_some(),
        "the first page of five leaves something over, got {:?}",
        first.body
    );

    // The store CHANGES between the two pages: the access the first page ended on is
    // revoked, which both moves it out of the default listing and moves the instant the
    // order is on.
    let revoked = dz
        .call(
            "PUT",
            &format!(
                "/v2/domains/{}/subscriptions/{}/revoke",
                catalog.domain, order[1]
            ),
            None,
        )
        .await;
    assert_eq!(
        revoked.status, 200,
        "revoking the access the first page ended on: expected 200, got {} {:?}",
        revoked.status, revoked.body
    );
    assert_eq!(
        revoked.s("status"),
        SUBSCRIPTION_STATUS_REVOKED,
        "the change between the pages really gave the access up, got {:?}",
        revoked.body
    );

    // The pages that follow still resume after it: the three that were left over, in the
    // same order, neither skipped nor repeated.
    let mut collected = order[..2].to_vec();
    let mut pages = 1;
    while let Some(resume) = token.take() {
        let page = list_subscriptions(
            &dz,
            &catalog.domain,
            &format!("maxResults=2&nextToken={resume}"),
        )
        .await;
        assert_eq!(
            page.status, 200,
            "the page resuming from {resume:?}: expected 200, got {} {:?}",
            page.status, page.body
        );
        collected.extend(item_ids(&page.body));
        pages += 1;
        assert!(
            pages <= 10,
            "the pages end rather than resuming for ever, got {collected:?}"
        );
        token = page
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    assert_eq!(
        collected, order,
        "the pages across the change concatenate to the one order"
    );

    // The access the change gave up is reported by the listing that names the status it
    // now carries, and by no other.
    let given_up = list_subscriptions(
        &dz,
        &catalog.domain,
        &format!("status={SUBSCRIPTION_STATUS_REVOKED}"),
    )
    .await;
    assert_eq!(
        item_ids(&given_up.body),
        vec![order[1].clone()],
        "the given-up access is reported under its own status, got {:?}",
        given_up.body
    );
    let standing = list_subscriptions(&dz, &catalog.domain, "").await;
    assert_eq!(
        item_ids(&standing.body),
        vec![
            order[0].clone(),
            order[2].clone(),
            order[3].clone(),
            order[4].clone(),
        ],
        "the default listing reports only the accesses that still stand, got {:?}",
        standing.body
    );
}

/// A domain that holds no access at all still reports `items`, which the model makes a
/// required output member, as an empty list — and no token, because there is no further page.
#[tokio::test]
async fn a_domain_holding_no_access_reports_an_empty_page() {
    let dz = Emulator::new();
    let domain = dz.available_domain("access-listing-empty").await;

    // The rows differ along what the request asks for of a store that holds nothing.
    let rows: Vec<(&str, String)> = vec![
        ("no filter at all", String::new()),
        ("a page of one", "maxResults=1".to_string()),
        (
            "the status a granted access carries",
            format!("status={SUBSCRIPTION_STATUS_APPROVED}"),
        ),
        (
            "the status a revoke leaves",
            format!("status={SUBSCRIPTION_STATUS_REVOKED}"),
        ),
        (
            "a filter naming a listing the domain never published",
            "subscribedListingId=nosuchlisting".to_string(),
        ),
    ];
    for (label, query) in rows {
        let listed = list_subscriptions(&dz, &domain, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body.get("items").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {label:?}: the required items member is an empty list, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body.get("nextToken"),
            None,
            "case {label:?}: an empty page is the last one, got {:?}",
            listed.body
        );
    }
}
