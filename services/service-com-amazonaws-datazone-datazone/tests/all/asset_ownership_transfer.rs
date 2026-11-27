// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAssetOwner`: the transfer of an asset's ownership to
//! another project of the same domain.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request: a
//! transfer is recorded `IN_PROGRESS` against the asset and lands asynchronously, and
//! when it lands the ownership MOVES — the asset is owned by the target project, and the
//! catalog record of that asset (its listing, and so every subscription resolved through
//! that listing) reports the project that now owns it. While a transfer is in progress the
//! asset admits no second transfer and no change to itself or to its listing, and a
//! replay of the request that started it answers with that same transfer.
//!
//! The rows differ along what the rule generalises over: whether the asset is published
//! into the catalog and whether anything is subscribed to it (the records a landing
//! transfer has to move), which change a running transfer refuses, and which refusal each
//! unservable pair of projects earns.
//!
//! Every assertion is made on the HTTP answer only. ONE fixture seam reaches into a store
//! — the DataZone user a project belongs to, because a project is attributed to the user
//! that created it and no wire call records one another user owns
//! ([`crate::harness::attribute_project_to`]). Nothing is ASSERTED from a store.

use crate::harness::{attribute_project_to, Answer, Emulator};
use serde_json::{json, Value};
// The enum members and the refusal wording as the service states them, so a drift between
// these and what goes on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    transfer_in_progress_message, OWNERSHIP_TRANSFER_STATUS_FAILED,
    OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS, OWNERSHIP_TRANSFER_STATUS_SUCCEEDED,
};
use std::time::{Duration, Instant};

/// A GENEROUS deadline for a transfer the clock owes, far longer than the span the service
/// gives one, so a loaded host still reaches it. The wait is for the CONDITION; this only
/// bounds how long it is looked for.
const TRANSFER_DEADLINE: Duration = Duration::from_secs(60);

/// How long to wait between two looks at a condition the clock owes.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A `UserProfileId`-shaped id of some OTHER DataZone user (hex groups 8-4-4-4-12,
/// smithy_query shape="UserProfileId"): who a project the caller does not own belongs to.
const ANOTHER_USER: &str = "b7c8d9e0-1a2b-4c3d-8e4f-5a6b7c8d9e0f";

/// The DataZone user this emulator authenticates a caller presenting the account's own
/// credential as, read from the service's own constant: who a project is attributed back
/// to once a case has handed it to another user.
const CALLER: &str = service_com_amazonaws_datazone_datazone::state::CALLER_USER_PROFILE_ID;

/// Create an asset owned by `owner`, answering the whole response so a case can read its
/// id and revision back off it.
async fn asset(dz: &Emulator, domain: &str, owner: &str, name: &str) -> Value {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": name,
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": owner,
                "formsInput": [{
                    "formName": "OwnershipForm",
                    "content": "{\"owner\":\"data-eng\"}",
                }],
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the asset {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.body
}

/// Publish an asset into the domain's catalog and wait until the listing is `ACTIVE`,
/// answering the `ListingId` the change set recorded it under.
async fn published_listing(dz: &Emulator, domain: &str, asset_id: &str) -> String {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/listings/change-set"),
            Some(json!({
                "entityIdentifier": asset_id,
                "entityType": "ASSET",
                "action": "PUBLISH",
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "publishing asset {asset_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let listing = answer.s("listingId");
    dz.await_listing_status(domain, &listing, "ACTIVE").await;
    listing
}

/// Start a transfer of `asset_id`'s ownership to `target`, answering what the service
/// said.
async fn transfer(
    dz: &Emulator,
    domain: &str,
    asset_id: &str,
    target: &str,
    client_token: Option<&str>,
) -> Answer {
    let mut body = json!({ "targetProjectIdentifier": target });
    if let Some(token) = client_token {
        body["clientToken"] = json!(token);
    }
    dz.call(
        "PATCH",
        &format!("/v2/domains/{domain}/assets/{asset_id}"),
        Some(body),
    )
    .await
}

/// Look again until `GetAsset` reports the asset's ownership transfer in `status`, or a
/// generous deadline passes, and answer the asset as it then stands. Waits for the
/// CONDITION the clock owes, never for a fixed time.
async fn await_transfer_status(
    dz: &Emulator,
    domain: &str,
    asset_id: &str,
    status: &str,
) -> Value {
    let started = Instant::now();
    loop {
        let read = dz
            .call("GET", &format!("/v2/domains/{domain}/assets/{asset_id}"), None)
            .await;
        let reported = read
            .body
            .get("ownershipTransfer")
            .and_then(|transfer| transfer.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if read.status == 200 && reported == status {
            return read.body;
        }
        assert!(
            started.elapsed() < TRANSFER_DEADLINE,
            "asset {asset_id:?}: the transfer never reached {status:?}: last answer {} {:?}",
            read.status,
            read.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The member of a response body at `path`, or `Value::Null` so an assertion message says
/// which member was missing.
fn at(body: &Value, path: &[&str]) -> Value {
    let mut cursor = body;
    for step in path {
        match cursor.get(step) {
            Some(next) => cursor = next,
            None => return Value::Null,
        }
    }
    cursor.clone()
}

/// One asset whose ownership is transferred: which records of it the move has to carry
/// along.
struct MoveCase {
    label: &'static str,
    /// Whether the asset is published into the catalog before the transfer.
    published: bool,
    /// Whether a subscription request is made against that listing, whose reported
    /// listing owner has to follow the asset.
    subscribed: bool,
}

fn move_cases() -> Vec<MoveCase> {
    vec![
        MoveCase {
            label: "an asset nobody published",
            published: false,
            subscribed: false,
        },
        MoveCase {
            label: "a published asset",
            published: true,
            subscribed: false,
        },
        MoveCase {
            label: "a published asset with a subscription on it",
            published: true,
            subscribed: true,
        },
    ]
}

#[tokio::test]
async fn a_landed_transfer_moves_the_asset_its_listing_and_what_is_subscribed_to_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-transfer-domain").await;
    let source = dz.project(&domain, "transfer source").await;
    let target_name = "transfer target";
    let target = dz.project(&domain, target_name).await;
    let subscriber = dz.project(&domain, "transfer subscriber").await;

    for case in move_cases() {
        let created = asset(&dz, &domain, &source, &format!("asset: {}", case.label)).await;
        let asset_id = created.get("id").and_then(Value::as_str).unwrap_or_default();
        let listing = if case.published {
            Some(published_listing(&dz, &domain, asset_id).await)
        } else {
            None
        };
        let request = match (&listing, case.subscribed) {
            (Some(listing), true) => {
                let made = dz
                    .call(
                        "POST",
                        &format!("/v2/domains/{domain}/subscription-requests"),
                        Some(json!({
                            "subscribedPrincipals": [{ "project": { "identifier": subscriber } }],
                            "subscribedListings": [{ "identifier": listing }],
                            "requestReason": "the quarterly report needs this asset",
                        })),
                    )
                    .await;
                assert_eq!(
                    made.status, 200,
                    "case {:?}: the subscription request expected 200, got {} {:?}",
                    case.label, made.status, made.body
                );
                Some(made.s("id"))
            }
            _ => None,
        };

        // An asset that has never had a transfer reports none at all.
        let before = dz
            .call("GET", &format!("/v2/domains/{domain}/assets/{asset_id}"), None)
            .await;
        assert_eq!(
            before.body.get("ownershipTransfer"),
            None,
            "case {:?}: an asset with no transfer reports none, got {:?}",
            case.label,
            before.body
        );

        let started = transfer(&dz, &domain, asset_id, &target, None).await;
        assert_eq!(
            started.status, 202,
            "case {:?}: UpdateAssetOwner expected 202, got {} {:?}",
            case.label, started.status, started.body
        );
        assert_eq!(
            started.s("id"),
            asset_id,
            "case {:?}: the answer names the asset asked for, got {:?}",
            case.label,
            started.body
        );
        // The transfer records a revision of the asset, so the asset created at revision
        // 1 is at revision 2 when its first transfer starts.
        assert_eq!(
            started.s("revision"),
            "2",
            "case {:?}: the transfer records the next revision of the asset, got {:?}",
            case.label,
            started.body
        );
        assert_eq!(
            at(&started.body, &["ownershipTransfer", "status"]),
            json!(OWNERSHIP_TRANSFER_STATUS_IN_PROGRESS),
            "case {:?}: a transfer is in progress when the call answers, got {:?}",
            case.label,
            started.body
        );
        assert_eq!(
            at(&started.body, &["ownershipTransfer", "sourceProjectId"]),
            json!(source),
            "case {:?}: the source is the project that owned the asset, got {:?}",
            case.label,
            started.body
        );
        assert_eq!(
            at(&started.body, &["ownershipTransfer", "targetProjectId"]),
            json!(target),
            "case {:?}: the target is the project asked for, got {:?}",
            case.label,
            started.body
        );
        for stamped in ["startedAt", "updatedAt"] {
            assert_ne!(
                at(&started.body, &["ownershipTransfer", stamped]),
                Value::Null,
                "case {:?}: a started transfer reports {stamped}, got {:?}",
                case.label,
                started.body
            );
        }

        // The move lands on the clock, and only then is the asset the target's.
        let settled =
            await_transfer_status(&dz, &domain, asset_id, OWNERSHIP_TRANSFER_STATUS_SUCCEEDED)
                .await;
        assert_eq!(
            settled.get("owningProjectId"),
            Some(&json!(target)),
            "case {:?}: the target project owns the asset once the transfer lands, got {settled:?}",
            case.label
        );
        assert_eq!(
            at(&settled, &["ownershipTransfer", "sourceProjectId"]),
            json!(source),
            "case {:?}: a landed transfer still reports where the asset came from, got \
             {settled:?}",
            case.label
        );
        assert_eq!(
            settled.get("revision"),
            Some(&json!("2")),
            "case {:?}: the asset is at the revision the transfer recorded, got {settled:?}",
            case.label
        );

        // The catalog record of the asset moved with it.
        if let Some(listing) = &listing {
            let read = dz
                .call("GET", &format!("/v2/domains/{domain}/listings/{listing}"), None)
                .await;
            assert_eq!(
                at(&read.body, &["item", "assetListing", "owningProjectId"]),
                json!(target),
                "case {:?}: the listing is owned by the project the asset moved to, got {:?}",
                case.label,
                read.body
            );
        }
        // And so does what is subscribed to it: a subscription names the catalog record,
        // which this store resolves on every read.
        if let Some(request) = &request {
            let read = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/subscription-requests/{request}"),
                    None,
                )
                .await;
            let subscribed = read
                .body
                .get("subscribedListings")
                .and_then(Value::as_array)
                .and_then(|listings| listings.first().cloned())
                .unwrap_or(Value::Null);
            assert_eq!(
                subscribed.get("ownerProjectId"),
                Some(&json!(target)),
                "case {:?}: the subscribed listing is owned by the project the asset moved \
                 to, got {subscribed:?}",
                case.label
            );
            assert_eq!(
                subscribed.get("ownerProjectName"),
                Some(&json!(target_name)),
                "case {:?}: and names that project, got {subscribed:?}",
                case.label
            );
        }
    }
}

/// One change refused while a transfer of the asset is running: the request, and the
/// wording the refusal carries.
struct RefusedChange {
    label: &'static str,
    /// The HTTP method and the URI, built from the domain and the asset.
    request: fn(&str, &str) -> (&'static str, String),
    /// The body the request carries, built from the asset and the project it would be
    /// transferred to.
    body: fn(&str, &str) -> Value,
    /// What the refusal says the request would have done.
    change: &'static str,
    /// Whether the asset has to be published for the request to reach the refusal.
    needs_a_listing: bool,
}

fn refused_changes() -> Vec<RefusedChange> {
    vec![
        RefusedChange {
            label: "a revision of the asset",
            request: |domain, asset_id| {
                ("POST", format!("/v2/domains/{domain}/assets/{asset_id}/revisions"))
            },
            body: |_, _| json!({ "name": "revised while transferring" }),
            change: "revised",
            needs_a_listing: false,
        },
        RefusedChange {
            label: "a delete of the asset",
            request: |domain, asset_id| {
                ("DELETE", format!("/v2/domains/{domain}/assets/{asset_id}"))
            },
            body: |_, _| Value::Null,
            change: "deleted",
            needs_a_listing: false,
        },
        RefusedChange {
            label: "a publish of the asset",
            request: |domain, _| ("POST", format!("/v2/domains/{domain}/listings/change-set")),
            body: |asset_id, _| {
                json!({
                    "entityIdentifier": asset_id,
                    "entityType": "ASSET",
                    "action": "PUBLISH",
                })
            },
            change: "published into the catalog or withdrawn from it",
            needs_a_listing: false,
        },
        RefusedChange {
            label: "an unpublish of the asset's listing",
            request: |domain, _| ("POST", format!("/v2/domains/{domain}/listings/change-set")),
            body: |asset_id, _| {
                json!({
                    "entityIdentifier": asset_id,
                    "entityType": "ASSET",
                    "action": "UNPUBLISH",
                })
            },
            change: "published into the catalog or withdrawn from it",
            needs_a_listing: true,
        },
        RefusedChange {
            label: "a second transfer of the asset",
            request: |domain, asset_id| ("PATCH", format!("/v2/domains/{domain}/assets/{asset_id}")),
            body: |_, target| json!({ "targetProjectIdentifier": target }),
            change: "transferred to another project",
            needs_a_listing: false,
        },
    ]
}

#[tokio::test]
async fn a_running_transfer_refuses_every_change_to_the_asset_and_its_listing() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-transfer-conflict-domain").await;
    let source = dz.project(&domain, "conflict source").await;
    let target = dz.project(&domain, "conflict target").await;

    for case in refused_changes() {
        // A fresh asset per row, so each refusal is asked of a transfer this row just
        // started rather than of one an earlier row left behind.
        let created = asset(&dz, &domain, &source, &format!("conflict: {}", case.label)).await;
        let asset_id = created
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if case.needs_a_listing {
            published_listing(&dz, &domain, &asset_id).await;
        }
        let started = transfer(&dz, &domain, &asset_id, &target, None).await;
        assert_eq!(
            started.status, 202,
            "case {:?}: UpdateAssetOwner expected 202, got {} {:?}",
            case.label, started.status, started.body
        );

        let (method, uri) = (case.request)(&domain, &asset_id);
        let body = (case.body)(&asset_id, &target);
        let refused = dz
            .call(
                method,
                &uri,
                (!body.is_null()).then_some(body),
            )
            .await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected 409 while the transfer runs, got {} {:?}",
            case.label, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "ConflictException",
            "case {:?}: expected a ConflictException, got {:?} {:?}",
            case.label, refused.error_code, refused.body
        );
        assert_eq!(
            refused.s("message"),
            transfer_in_progress_message(&asset_id, case.change),
            "case {:?}: the refusal names the asset and the change, got {:?}",
            case.label,
            refused.body
        );

        // Once the transfer has landed the asset takes a change again, so the refusal was
        // the transfer's and not the asset's.
        await_transfer_status(&dz, &domain, &asset_id, OWNERSHIP_TRANSFER_STATUS_SUCCEEDED).await;
        let served = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/assets/{asset_id}/revisions"),
                Some(json!({ "name": "revised once the transfer landed" })),
            )
            .await;
        assert_eq!(
            served.status, 200,
            "case {:?}: a landed transfer leaves the asset changeable, got {} {:?}",
            case.label, served.status, served.body
        );
    }
}

/// One `UpdateAssetOwner` the service cannot serve: the pair of projects it names, and
/// the refusal it earns.
struct RefusedTransfer {
    label: &'static str,
    /// The domain identifier the request addresses, when it is not the stored one.
    domain: Option<&'static str>,
    /// The asset identifier the request addresses, when it is not the stored one.
    asset: Option<&'static str>,
    /// The target the request names: `None` for the project that already owns the asset.
    target: Option<&'static str>,
    /// Whether the project that owns the asset belongs to another DataZone user.
    owned_by_another: bool,
    status: u16,
    error_code: &'static str,
}

fn refused_transfers() -> Vec<RefusedTransfer> {
    vec![
        RefusedTransfer {
            label: "a domain this store serves no record of",
            domain: Some("dzd_doesnotexist"),
            asset: None,
            target: Some("someproject"),
            owned_by_another: false,
            status: 403,
            error_code: "AccessDeniedException",
        },
        RefusedTransfer {
            label: "an asset the domain holds no record of",
            domain: None,
            asset: Some("nosuchasset"),
            target: Some("someproject"),
            owned_by_another: false,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusedTransfer {
            label: "a target project the domain holds no record of",
            domain: None,
            asset: None,
            target: Some("nosuchproject"),
            owned_by_another: false,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusedTransfer {
            label: "the project that already owns the asset",
            domain: None,
            asset: None,
            target: None,
            owned_by_another: false,
            status: 400,
            error_code: "ValidationException",
        },
        RefusedTransfer {
            label: "an asset owned by a project the caller does not own",
            domain: None,
            asset: None,
            target: Some("the target"),
            owned_by_another: true,
            status: 403,
            error_code: "AccessDeniedException",
        },
    ]
}

#[tokio::test]
async fn a_transfer_the_service_cannot_serve_is_refused_and_records_nothing() {
    let (dz, engine) = Emulator::with_engine();
    let domain = dz.available_domain("asset-transfer-refusal-domain").await;
    let source = dz.project(&domain, "refusal source").await;
    let target = dz.project(&domain, "refusal target").await;

    for case in refused_transfers() {
        let created = asset(&dz, &domain, &source, &format!("refused: {}", case.label)).await;
        let asset_id = created
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if case.owned_by_another {
            attribute_project_to(&engine, &domain, &source, ANOTHER_USER);
        }
        let addressed_domain = case.domain.unwrap_or(&domain).to_string();
        let addressed_asset = case.asset.unwrap_or(&asset_id).to_string();
        let named_target = match case.target {
            // A target the case does not name is the project that already owns the asset.
            None => source.clone(),
            Some("the target") => target.clone(),
            Some(named) => named.to_string(),
        };

        let refused = transfer(
            &dz,
            &addressed_domain,
            &addressed_asset,
            &named_target,
            None,
        )
        .await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected {}, got {} {:?}",
            case.label, case.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, case.error_code,
            "case {:?}: expected {}, got {:?} {:?}",
            case.label, case.error_code, refused.error_code, refused.body
        );

        // A refused transfer records nothing: the asset is still owned by the project
        // that owned it, at the revision it was created with, with no transfer at all.
        if case.owned_by_another {
            attribute_project_to(&engine, &domain, &source, CALLER);
        }
        let read = dz
            .call("GET", &format!("/v2/domains/{domain}/assets/{asset_id}"), None)
            .await;
        assert_eq!(
            read.body.get("owningProjectId"),
            Some(&json!(source)),
            "case {:?}: a refused transfer leaves the owner alone, got {:?}",
            case.label,
            read.body
        );
        assert_eq!(
            read.body.get("revision"),
            Some(&json!("1")),
            "case {:?}: a refused transfer records no revision, got {:?}",
            case.label,
            read.body
        );
        assert_eq!(
            read.body.get("ownershipTransfer"),
            None,
            "case {:?}: a refused transfer records no transfer, got {:?}",
            case.label,
            read.body
        );
    }
}

#[tokio::test]
async fn a_client_token_replays_the_transfer_it_started_while_that_transfer_runs() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-transfer-token-domain").await;
    let source = dz.project(&domain, "token source").await;
    let target = dz.project(&domain, "token target").await;

    // 126 of the 128 characters a `ClientToken` admits, from the printable-ASCII class
    // its pattern names (len 1..128, pattern ^[\x21-\x7E]+$, smithy_query
    // shape="UpdateAssetOwnerInput" member="clientToken").
    let longest = "T~9".repeat(42);
    // The rows differ along the token itself: the shortest the model admits, the longest,
    // and one in between.
    for token in ["t", longest.as_str(), "transfer-token-1"] {
        let created = asset(&dz, &domain, &source, &format!("token asset: {token}")).await;
        let asset_id = created
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let first = transfer(&dz, &domain, &asset_id, &target, Some(token)).await;
        assert_eq!(
            first.status, 202,
            "case {token:?}: UpdateAssetOwner expected 202, got {} {:?}",
            first.status, first.body
        );
        let replay = transfer(&dz, &domain, &asset_id, &target, Some(token)).await;
        assert_eq!(
            replay.status, 202,
            "case {token:?}: a replay expected 202, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay answers with the transfer the first call started, \
             got {:?}",
            replay.body
        );

        // Once the transfer has landed the same token is a NEW request: the asset is now
        // the target's, so it can be transferred back, and the revision the second
        // transfer records is the next one again.
        await_transfer_status(&dz, &domain, &asset_id, OWNERSHIP_TRANSFER_STATUS_SUCCEEDED).await;
        let again = transfer(&dz, &domain, &asset_id, &source, Some(token)).await;
        assert_eq!(
            again.status, 202,
            "case {token:?}: a transfer back expected 202, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.s("revision"),
            "3",
            "case {token:?}: the second transfer records the next revision, got {:?}",
            again.body
        );
        assert_eq!(
            at(&again.body, &["ownershipTransfer", "sourceProjectId"]),
            json!(target),
            "case {token:?}: the asset now comes FROM the project it was moved to, got {:?}",
            again.body
        );
        await_transfer_status(&dz, &domain, &asset_id, OWNERSHIP_TRANSFER_STATUS_SUCCEEDED).await;
    }
}

#[tokio::test]
async fn a_transfer_whose_target_is_gone_when_it_lands_fails_and_the_source_keeps_the_asset() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-transfer-failure-domain").await;
    let source = dz.project(&domain, "failing source").await;
    let target = dz.project(&domain, "failing target").await;
    let created = asset(&dz, &domain, &source, "asset whose transfer fails").await;
    let asset_id = created
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let started = transfer(&dz, &domain, &asset_id, &target, None).await;
    assert_eq!(
        started.status, 202,
        "UpdateAssetOwner expected 202, got {} {:?}",
        started.status, started.body
    );
    // The target is taken out of the domain while the transfer is still running, so there
    // is no project left to give the asset to when it lands.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}/projects/{target}"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "deleting the target project expected 204, got {} {:?}",
        deleted.status, deleted.body
    );

    let settled =
        await_transfer_status(&dz, &domain, &asset_id, OWNERSHIP_TRANSFER_STATUS_FAILED).await;
    assert_eq!(
        settled.get("owningProjectId"),
        Some(&json!(source)),
        "a transfer that could not complete leaves the asset with the source project, got \
         {settled:?}"
    );
    assert_eq!(
        at(&settled, &["ownershipTransfer", "targetProjectId"]),
        json!(target),
        "the failed transfer still reports the project it tried to move the asset to, got \
         {settled:?}"
    );

    // Nothing is in progress any more, so the asset takes a change again.
    let revised = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets/{asset_id}/revisions"),
            Some(json!({ "name": "revised after a failed transfer" })),
        )
        .await;
    assert_eq!(
        revised.status, 200,
        "a failed transfer leaves the asset changeable, got {} {:?}",
        revised.status, revised.body
    );
}
