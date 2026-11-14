// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for taking a package out of the account: `DeletePackage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`] and [`super::package_dissociation`]; this suite adds
//! no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a package no domain holds is deleted and answered through the same
//!   `PackageDetails` projection every other package path reports it through, over a
//!   table spanning every `PackageType` value, the description present and absent, and
//!   content imported once and updated twice;
//! * the record is left in a transient status and the clock owes its removal: every row
//!   is `DELETING` the instant the call returns and reported by nothing once the span
//!   has passed, without any call having asked for the move;
//! * a package a domain still holds cannot be deleted — the link being put in place,
//!   in place, being taken off, and held by two domains at once — and the delete is
//!   accepted once the package is off every domain;
//! * an id that names no package of the account is refused, over the forms such an id
//!   takes, leaving the stored set exactly as it was;
//! * the slot a package counted against the store's bound is released by deleting it.
//!
//! The stored set is read back over the wire through `DescribePackages`, and the other
//! read paths of one package (`GetPackageVersionHistory`, `ListDomainsForPackage`)
//! through their own routes, so a case asserts that no read reports a package the
//! removal has taken out — the by-id read refusing the id and the listing of its
//! domains reporting the empty set.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PACKAGE_NOT_FOUND, PACKAGE_QUOTA, PACKAGE_STATUS_AVAILABLE, PACKAGE_STATUS_DELETING,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{member, PACKAGE_TYPES, SERVICE_PACKAGE_ID, UNKNOWN_PACKAGE_ID};
use super::package_dissociation::{associate, dissociate_many, listed_status, with_associations};

/// `DeletePackage`: `DELETE /2021-01-01/packages/{PackageID}`.
async fn delete(emulator: &Emulator, package_id: &str) -> Answer {
    emulator
        .call("DELETE", &format!("{PREFIX}/packages/{package_id}"), None)
        .await
}

/// `CreatePackage`: `POST /2021-01-01/packages`. The table varies the description,
/// which the sibling suites' fixed-shape helper does not carry, so this suite posts the
/// body it builds.
async fn create(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages"), Some(body))
        .await
}

/// `UpdatePackage`: `POST /2021-01-01/packages/update` — the path that imports another
/// version of a package's content, so a row can delete a package with a longer version
/// history than the create left it with.
async fn update(emulator: &Emulator, package_id: &str, case: &str) {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/update"),
            Some(json!({
                "PackageID": package_id,
                "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "next.txt" },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the update to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The packages `DescribePackages` reports for THIS ACCOUNT, which is its stored set.
///
/// The listing reports "all packages available to Amazon OpenSearch Service" — the
/// plugin packages the SERVICE publishes beside the ones the account created — and a
/// published package is owned by its publisher rather than by the account, so the
/// stored set is the listing narrowed by `PackageOwner`.
async fn described(emulator: &Emulator, case: &str) -> Vec<Value> {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/describe"),
            Some(json!({ "MaxResults": 100 })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageDetailsList, got {:?}",
                answer.body
            )
        })
        .iter()
        .filter(|details| {
            details["PackageOwner"].as_str() == Some(::engine::emulator_account_id())
        })
        .cloned()
        .collect()
}

/// The `PackageStatus` the listing reports for `package_id`, or `None` where the
/// listing no longer reports the package at all.
async fn described_status(emulator: &Emulator, package_id: &str, case: &str) -> Option<String> {
    described(emulator, case)
        .await
        .into_iter()
        .find(|details| details["PackageID"].as_str() == Some(package_id))
        .and_then(|details| details["PackageStatus"].as_str().map(str::to_owned))
}

/// Assert `answer` is the refusal of a `PackageID` that resolves to no package of this
/// account: the service treats it as a bad input value, so `ValidationException` at the
/// HTTP 400 this service's model declares it at, carrying the fixed sentence for that
/// condition and no id.
fn assert_not_found(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case}: expected the modelled validation refusal, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(PACKAGE_NOT_FOUND),
        "case {case}: expected the fixed id-free sentence, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the modelled refusal of a delete the account cannot serve while
/// the package is in use: `ValidationException` at HTTP 400.
fn assert_in_use(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case}: expected the modelled validation refusal, got {:?}",
        answer.body
    );
}

/// One package to create and delete: its name, type, description and how many further
/// versions of its content are imported before the delete.
struct Row {
    case: &'static str,
    name: &'static str,
    package_type: &'static str,
    description: Option<&'static str>,
    updates: usize,
}

/// The deletion table: one row per way a deleted package differs from another — every
/// `PackageType` value ([`PACKAGE_TYPES`], pinned against the model's own value set by
/// `super::package_associations::every_package_type_is_covered`), a description present
/// and absent, and a package whose content was imported once against one updated twice.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a dictionary package, no description",
            name: "delete-dictionary",
            package_type: PACKAGE_TYPES[0],
            description: None,
            updates: 0,
        },
        Row {
            case: "a plugin package with a description",
            name: "delete-plugin",
            package_type: PACKAGE_TYPES[1],
            description: Some("the plugin this account no longer needs"),
            updates: 0,
        },
        Row {
            case: "a configuration package",
            name: "delete-config",
            package_type: PACKAGE_TYPES[2],
            description: None,
            updates: 0,
        },
        Row {
            case: "a license package",
            name: "delete-license",
            package_type: PACKAGE_TYPES[3],
            description: Some("a licence of its own"),
            updates: 0,
        },
        Row {
            case: "a dictionary package whose content was updated twice",
            name: "delete-updated",
            package_type: PACKAGE_TYPES[0],
            description: None,
            updates: 2,
        },
    ]
}

/// Create the row's package and leave it `AVAILABLE` with every version of its content
/// in place, answering with the `PackageID` the create minted.
async fn available_package(emulator: &Emulator, row: &Row) -> String {
    let mut body = json!({
        "PackageName": row.name,
        "PackageType": row.package_type,
        "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "first.txt" },
    });
    if let Some(description) = row.description {
        body["PackageDescription"] = json!(description);
    }
    let answer = create(emulator, body).await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected the create to be accepted, got {} with {:?}",
        row.case, answer.status, answer.body
    );
    let package_id = answer.body["PackageDetails"]["PackageID"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {}: expected a PackageID, got {:?}",
                row.case, answer.body
            )
        })
        .to_string();
    tokio::time::sleep(SETTLE_WAIT).await;
    for _ in 0..row.updates {
        update(emulator, &package_id, row.case).await;
        tokio::time::sleep(SETTLE_WAIT).await;
    }
    package_id
}

#[tokio::test]
async fn a_package_no_domain_holds_is_deleted_and_reported_from_the_stored_record() {
    let emulator = Emulator::new();

    for row in rows() {
        let package_id = available_package(&emulator, &row).await;
        let answer = delete(&emulator, &package_id).await;

        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let details = &answer.body["PackageDetails"];
        assert_eq!(
            member(details, "PackageID", row.case),
            package_id,
            "case {}: the answer names the package it deleted, got {details:?}",
            row.case
        );
        assert_eq!(
            member(details, "PackageName", row.case),
            row.name,
            "case {}: the answer reports the stored name, got {details:?}",
            row.case
        );
        assert_eq!(
            member(details, "PackageType", row.case),
            row.package_type,
            "case {}: the answer reports the stored type, got {details:?}",
            row.case
        );
        assert_eq!(
            details["PackageDescription"].as_str(),
            row.description,
            "case {}: the answer reports the stored description, got {details:?}",
            row.case
        );
        assert_eq!(
            member(details, "PackageStatus", row.case),
            PACKAGE_STATUS_DELETING,
            "case {}: the package is being deleted the instant the call returns, got \
             {details:?}",
            row.case
        );

        // Nothing has asked for the removal, so the record is still the account's — in
        // the status the delete left it in, reported by the listing exactly as the call
        // answered it.
        assert_eq!(
            described_status(&emulator, &package_id, row.case)
                .await
                .as_deref(),
            Some(PACKAGE_STATUS_DELETING),
            "case {}: the listing reports the package being deleted until its removal \
             falls due",
            row.case
        );

        tokio::time::sleep(SETTLE_WAIT).await;

        assert_eq!(
            described_status(&emulator, &package_id, row.case).await,
            None,
            "case {}: the clock owed the removal, so the listing reports the package no \
             more",
            row.case
        );

        // Every read path of ONE package answers for a package the account no longer
        // holds the same way: there is nothing left to report. The listing of the
        // DOMAINS the id is associated with reports a SET, so it answers that set empty
        // rather than refusing the id.
        let history = emulator
            .call(
                "GET",
                &format!("{PREFIX}/packages/{package_id}/history"),
                None,
            )
            .await;
        assert_not_found(&history, row.case);
        let domains = emulator
            .call(
                "GET",
                &format!("{PREFIX}/packages/{package_id}/domains"),
                None,
            )
            .await;
        assert_eq!(
            domains.status, 200,
            "case {}: expected the listing to be served, got {} with {:?}",
            row.case, domains.status, domains.body
        );
        assert_eq!(
            domains.body["DomainPackageDetailsList"].as_array(),
            Some(&Vec::<Value>::new()),
            "case {}: expected an empty DomainPackageDetailsList once the package is gone, \
             got {:?}",
            row.case,
            domains.body
        );
        let again = delete(&emulator, &package_id).await;
        assert_not_found(&again, row.case);
    }
}

/// Which state the link between the domain and the package is in when the delete is
/// made.
#[derive(Debug, Clone, Copy)]
enum Held {
    /// The association is being put in place and has not landed yet.
    BeingPutInPlace,
    /// The association is in place.
    InPlace,
    /// The association is being taken off and the removal has not landed yet.
    BeingTakenOff,
}

/// One held package: the domains holding it and the state their links are in.
struct HoldRow {
    case: &'static str,
    domains: Vec<&'static str>,
    held: Held,
}

/// The table of ways a package is still held: a link in flight, a settled link, a
/// removal in flight, and one package held by two domains at once.
fn hold_rows() -> Vec<HoldRow> {
    vec![
        HoldRow {
            case: "the association is being put in place",
            domains: vec!["del-associating"],
            held: Held::BeingPutInPlace,
        },
        HoldRow {
            case: "the association is in place",
            domains: vec!["del-active"],
            held: Held::InPlace,
        },
        HoldRow {
            case: "the association is being taken off",
            domains: vec!["del-dissociating"],
            held: Held::BeingTakenOff,
        },
        HoldRow {
            case: "two domains hold the package",
            domains: vec!["del-two-first", "del-two-second"],
            held: Held::InPlace,
        },
    ]
}

/// Put `package_id` on every domain of `row`, leaving the links in the state the row
/// names.
async fn hold(emulator: &Emulator, row: &HoldRow, package_id: &str) {
    for domain in &row.domains {
        match row.held {
            Held::BeingPutInPlace => {
                create_baseline(emulator, domain).await;
                let answer = associate(emulator, domain, &[package_id.to_string()]).await;
                assert_eq!(
                    answer.status, 200,
                    "case {}: expected the association to be accepted, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
            }
            Held::InPlace => {
                with_associations(emulator, domain, &[package_id.to_string()], &[0]).await;
            }
            Held::BeingTakenOff => {
                with_associations(emulator, domain, &[package_id.to_string()], &[0]).await;
                let answer = dissociate_many(
                    emulator,
                    json!({ "DomainName": domain, "PackageList": [package_id] }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {}: expected the dissociation to be accepted, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_package_a_domain_holds_cannot_be_deleted_until_it_is_off_every_domain() {
    for row in hold_rows() {
        // One emulator per row, so the package a row holds is the one its delete
        // addresses and no other row's domains can hold it.
        let emulator = Emulator::new();
        let package_id = available_package(
            &emulator,
            &Row {
                case: row.case,
                name: "held-package",
                package_type: PACKAGE_TYPES[0],
                description: None,
                updates: 0,
            },
        )
        .await;
        hold(&emulator, &row, &package_id).await;

        let answer = delete(&emulator, &package_id).await;
        assert_in_use(&answer, row.case);
        assert_eq!(
            described_status(&emulator, &package_id, row.case)
                .await
                .as_deref(),
            Some(PACKAGE_STATUS_AVAILABLE),
            "case {}: the refused delete left the package exactly as it was",
            row.case
        );
        for domain in &row.domains {
            assert!(
                listed_status(&emulator, domain, &package_id, row.case)
                    .await
                    .is_some(),
                "case {}: the refused delete left the link on {domain:?}",
                row.case
            );
        }

        // Taking the package off every domain is what makes the delete servable: once
        // nothing holds it, the same call is accepted.
        tokio::time::sleep(SETTLE_WAIT).await;
        for domain in &row.domains {
            if listed_status(&emulator, domain, &package_id, row.case)
                .await
                .is_none()
            {
                continue;
            }
            let answer = dissociate_many(
                &emulator,
                json!({ "DomainName": domain, "PackageList": [&package_id] }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {}: expected the dissociation to be accepted, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            tokio::time::sleep(SETTLE_WAIT).await;
        }

        let answer = delete(&emulator, &package_id).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected the delete to be accepted once nothing holds the package, \
             got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            member(&answer.body["PackageDetails"], "PackageStatus", row.case),
            PACKAGE_STATUS_DELETING,
            "case {}: the accepted delete leaves the package being deleted, got {:?}",
            row.case, answer.body
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_package_of_the_account_is_refused() {
    let emulator = Emulator::new();
    let row = Row {
        case: "a package the account keeps",
        name: "kept-package",
        package_type: PACKAGE_TYPES[0],
        description: None,
        updates: 0,
    };
    let kept = available_package(&emulator, &row).await;

    // The id of a package whose deletion the clock has already performed names no
    // package of the account either, so it is one of the rows.
    let gone = available_package(
        &emulator,
        &Row {
            case: "a package already deleted",
            name: "gone-package",
            package_type: PACKAGE_TYPES[1],
            description: None,
            updates: 0,
        },
    )
    .await;
    let answer = delete(&emulator, &gone).await;
    assert_eq!(
        answer.status, 200,
        "case the first delete of {gone:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let cases: [(&str, &str); 3] = [
        ("an id no create of this account minted", UNKNOWN_PACKAGE_ID),
        (
            "an id of the form the service's own packages take",
            SERVICE_PACKAGE_ID,
        ),
        ("the id of a package already deleted", gone.as_str()),
    ];
    for (case, package_id) in cases {
        let answer = delete(&emulator, package_id).await;
        assert_not_found(&answer, case);
    }

    let stored = described(&emulator, "the refused deletes").await;
    let ids: Vec<&str> = stored
        .iter()
        .filter_map(|details| details["PackageID"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![kept.as_str()],
        "case the refused deletes: the stored set is the one package nothing deleted"
    );
}

#[tokio::test]
async fn a_repeated_delete_within_the_removal_span_keeps_the_record_on_its_way_out() {
    let emulator = Emulator::new();
    let package_id = available_package(
        &emulator,
        &Row {
            case: "a package deleted twice",
            name: "twice-deleted",
            package_type: PACKAGE_TYPES[0],
            description: None,
            updates: 0,
        },
    )
    .await;

    for attempt in 0..2 {
        let case = format!("delete attempt {attempt}");
        let answer = delete(&emulator, &package_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            member(&answer.body["PackageDetails"], "PackageStatus", &case),
            PACKAGE_STATUS_DELETING,
            "case {case}: the repeat reports the record on its way out, got {:?}",
            answer.body
        );
    }

    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        described_status(&emulator, &package_id, "after the removal landed").await,
        None,
        "case after the removal landed: the repeat did not push the removal back"
    );
}

#[tokio::test]
async fn deleting_a_package_releases_the_slot_it_counted_against_the_store_bound() {
    let emulator = Emulator::new();
    let mut first = String::new();

    for index in 0..PACKAGE_QUOTA {
        let case = format!("CreatePackage(bound-{index})");
        let answer = create(
            &emulator,
            json!({
                "PackageName": format!("bound-package-{index}"),
                "PackageType": PACKAGE_TYPES[0],
                "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "p.txt" },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        if index == 0 {
            first = member(&answer.body["PackageDetails"], "PackageID", &case);
        }
    }

    let past_the_bound = json!({
        "PackageName": "bound-package-past-the-end",
        "PackageType": PACKAGE_TYPES[0],
        "PackageSource": { "S3BucketName": "my-package-bucket", "S3Key": "p.txt" },
    });
    let answer = create(&emulator, past_the_bound.clone()).await;
    assert_eq!(
        answer.status, 409,
        "case at the bound: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "LimitExceededException",
        "case at the bound: expected the modelled limit refusal, got {:?}",
        answer.body
    );

    let answer = delete(&emulator, &first).await;
    assert_eq!(
        answer.status, 200,
        "case the delete that frees a slot: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let answer = create(&emulator, past_the_bound).await;
    assert_eq!(
        answer.status, 200,
        "case under the bound again: expected the create to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    let stored = described(&emulator, "under the bound again").await;
    assert_eq!(
        stored.len(),
        PACKAGE_QUOTA,
        "case under the bound again: the stored set is back at the bound, got {stored:?}"
    );
    assert!(
        stored
            .iter()
            .all(|details| details["PackageID"].as_str() != Some(first.as_str())),
        "case under the bound again: the deleted package is not one of the stored set"
    );
}
