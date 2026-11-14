// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how EVERY operation that resolves a `PackageID` answers an id
//! that resolves to no package of the account.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The rule under test, which holds for every operation that CHANGES what a
//! `PackageID` resolves to rather than for the ones a report cited: an id the account
//! holds no package under is a bad input VALUE, so the call is refused with
//! `ValidationException` at the HTTP 400 the model declares it at, carrying the
//! service's fixed sentence for that condition and no part of the request — and nothing
//! is written. The listing of the DOMAINS an id is associated with is the other side of
//! that boundary: it reports a set, so it is served with that set empty (see
//! [`super::absent_scope_listings`]).
//!
//! So the assertions span a SET of inputs: every operation of the family crossed with
//! the ways an id resolves to nothing (never minted, in either form the `PackageID`
//! pattern admits, and a package this account created and then deleted), plus the
//! prerequisite list, which resolves ids of its own. A live package serving the same
//! operations is the positive control: the refusals are about the ID, not about the
//! operation or an empty store.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PACKAGE_NOT_FOUND, PACKAGE_STATUS_COPYING,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{
    create_package, PACKAGE_TYPES, SERVICE_PACKAGE_ID, UNKNOWN_PACKAGE_ID,
};

/// The domain every row that names one acts on.
const DOMAIN: &str = "unresolvable-pkg";

/// Which operation of the package family a row drives. Every operation whose input
/// carries a `PackageID` — the batch and single forms of associating and dissociating,
/// the delete, the two updates, and the by-id reads — plus the prerequisite list,
/// which resolves ids of its own inside an otherwise serviceable request.
#[derive(Debug, Clone, Copy)]
enum Op {
    AssociatePackages,
    AssociatePackagesPrerequisite,
    AssociatePackage,
    DissociatePackages,
    DissociatePackage,
    DeletePackage,
    UpdatePackage,
    UpdatePackageScope,
    GetPackageVersionHistory,
    ListDomainsForPackage,
}

/// Every operation the refusal rule covers, so a row is asserted over the whole family
/// rather than over the operations a report happened to name.
/// `Op::ListDomainsForPackage` is NOT one of them: it reports the set of domains an id
/// is associated with, and an unresolvable id names an empty set rather than a refusal,
/// which [`every_operation_that_resolves_a_package_id_refuses_one_that_names_no_package`]
/// asserts beside each refusal.
const OPS: [Op; 9] = [
    Op::AssociatePackages,
    Op::AssociatePackagesPrerequisite,
    Op::AssociatePackage,
    Op::DissociatePackages,
    Op::DissociatePackage,
    Op::DeletePackage,
    Op::UpdatePackage,
    Op::UpdatePackageScope,
    Op::GetPackageVersionHistory,
];

/// Drive `op` for `package_id`. `live` is a package the account DOES hold, which the
/// prerequisite row associates while naming `package_id` as its prerequisite — so that
/// row's only unresolvable value is the one under test.
async fn drive(emulator: &Emulator, op: Op, package_id: &str, live: &str) -> Answer {
    match op {
        Op::AssociatePackages => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/associateMultiple"),
                    Some(json!({
                        "DomainName": DOMAIN,
                        "PackageList": [{ "PackageID": package_id }],
                    })),
                )
                .await
        }
        Op::AssociatePackagesPrerequisite => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/associateMultiple"),
                    Some(json!({
                        "DomainName": DOMAIN,
                        "PackageList": [{
                            "PackageID": live,
                            "PrerequisitePackageIDList": [package_id],
                        }],
                    })),
                )
                .await
        }
        Op::AssociatePackage => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/associate/{package_id}/{DOMAIN}"),
                    Some(json!({})),
                )
                .await
        }
        Op::DissociatePackages => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/dissociateMultiple"),
                    Some(json!({ "DomainName": DOMAIN, "PackageList": [package_id] })),
                )
                .await
        }
        Op::DissociatePackage => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/dissociate/{package_id}/{DOMAIN}"),
                    None,
                )
                .await
        }
        Op::DeletePackage => {
            emulator
                .call("DELETE", &format!("{PREFIX}/packages/{package_id}"), None)
                .await
        }
        Op::UpdatePackage => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/update"),
                    Some(json!({
                        "PackageID": package_id,
                        "PackageSource": {
                            "S3BucketName": "my-package-bucket",
                            "S3Key": "packages/next.txt",
                        },
                        "CommitMessage": "after the id stopped resolving",
                    })),
                )
                .await
        }
        Op::UpdatePackageScope => {
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/packages/updateScope"),
                    Some(json!({
                        "PackageID": package_id,
                        "Operation": "ADD",
                        "PackageUserList": ["123456789012"],
                    })),
                )
                .await
        }
        Op::GetPackageVersionHistory => {
            emulator
                .call(
                    "GET",
                    &format!("{PREFIX}/packages/{package_id}/history"),
                    None,
                )
                .await
        }
        Op::ListDomainsForPackage => {
            emulator
                .call(
                    "GET",
                    &format!("{PREFIX}/packages/{package_id}/domains"),
                    None,
                )
                .await
        }
    }
}

/// The `PackageID`s `DescribePackages` reports for THIS ACCOUNT, which is its stored
/// set.
///
/// The listing reports "all packages available to Amazon OpenSearch Service" — the
/// plugin packages the SERVICE publishes beside the ones the account created — and a
/// published package is owned by its publisher rather than by the account, so the
/// stored set is the listing narrowed by `PackageOwner`.
async fn stored_ids(emulator: &Emulator, case: &str) -> Vec<String> {
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
        .filter_map(|details| details["PackageID"].as_str().map(str::to_owned))
        .collect()
}

/// A live AVAILABLE package and a second one created, deleted and waited out — so the
/// id of the deleted one resolves to nothing while the account still holds a package.
async fn with_a_live_and_a_deleted_package(emulator: &Emulator) -> (String, String) {
    let live = create_package(emulator, "unresolvable-live", PACKAGE_TYPES[0]).await;
    let gone = create_package(emulator, "unresolvable-gone", PACKAGE_TYPES[1]).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/packages/{gone}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the delete that makes an id stop resolving: expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let stored = stored_ids(emulator, "after the delete").await;
    assert_eq!(
        stored,
        vec![live.clone()],
        "case after the delete: expected only the live package to be reported, got {stored:?}"
    );
    (live, gone)
}

#[tokio::test]
async fn every_operation_that_resolves_a_package_id_refuses_one_that_names_no_package() {
    let emulator = Emulator::new();
    create_baseline(&emulator, DOMAIN).await;
    let (live, gone) = with_a_live_and_a_deleted_package(&emulator).await;

    // Rows differ along the ways an id resolves to no package: both forms the model's
    // `PackageID` pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$` admits — the one a
    // caller's own package takes and the one addressing a package the service publishes
    // — at more than one length, and an id this account did hold until it deleted the
    // package.
    let ids: [(&str, &str); 6] = [
        ("an id no create of this account minted", UNKNOWN_PACKAGE_ID),
        ("a shorter id of the same form", "pkg-abcdef0123456789"),
        ("the service-published form", SERVICE_PACKAGE_ID),
        ("the shortest service-published id", "G7"),
        ("an id whose package was deleted", gone.as_str()),
        ("the deleted id again", gone.as_str()),
    ];

    for (row, package_id) in ids {
        assert_ne!(
            package_id, live,
            "case {row}: the id under test is not the live package's"
        );
        for op in OPS {
            let case = format!("{op:?}({package_id}) / {row}");
            let answer = drive(&emulator, op, package_id, &live).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(PACKAGE_NOT_FOUND),
                "case {case}: expected the fixed sentence, got {:?}",
                answer.body
            );
            assert!(
                !answer.body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(package_id),
                "case {case}: the refusal carries no part of the request, got {:?}",
                answer.body
            );
        }

        // The other side of the boundary: the listing of the domains this id is
        // associated with reports that empty set rather than refusing the id.
        let listed = drive(&emulator, Op::ListDomainsForPackage, package_id, &live).await;
        let case = format!("ListDomainsForPackage({package_id}) / {row}");
        assert_eq!(
            listed.status, 200,
            "case {case}: expected the listing to be served, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body["DomainPackageDetailsList"].as_array(),
            Some(&Vec::<Value>::new()),
            "case {case}: expected an empty DomainPackageDetailsList, got {:?}",
            listed.body
        );
    }

    // A refused call writes nothing: no association was made on the domain, and the
    // stored set is still the one live package.
    let listed = emulator
        .call("GET", &format!("{PREFIX}/domain/{DOMAIN}/packages"), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "case the domain's listing: expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["DomainPackageDetailsList"].as_array(),
        Some(&Vec::<Value>::new()),
        "case the domain's listing: expected no association to have been made, got {:?}",
        listed.body
    );
    let stored = stored_ids(&emulator, "the refused calls").await;
    assert_eq!(
        stored,
        vec![live.clone()],
        "case the refused calls: expected the stored set to be untouched, got {stored:?}"
    );
}

#[tokio::test]
async fn a_package_the_account_holds_is_served_by_the_same_operations() {
    let emulator = Emulator::new();
    create_baseline(&emulator, DOMAIN).await;
    let (live, _gone) = with_a_live_and_a_deleted_package(&emulator).await;

    // The positive control for the refusals above: every by-id path answers the package
    // the account DOES hold, so the refusal of an unresolvable id is about that id and
    // not about a broken read or a store with nothing in it.
    for (case, method, uri) in [
        (
            "GetPackageVersionHistory",
            "GET",
            format!("{PREFIX}/packages/{live}/history"),
        ),
        (
            "ListDomainsForPackage",
            "GET",
            format!("{PREFIX}/packages/{live}/domains"),
        ),
    ] {
        let answer = emulator.call(method, &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}(the live package): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // The mutating members answer it too, and the update records another version of its
    // content — which is what tells apart "this id names nothing" from "this request can
    // never be served".
    let updated = drive(&emulator, Op::UpdatePackage, &live, &live).await;
    assert_eq!(
        updated.status, 200,
        "case UpdatePackage(the live package): expected 200, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.body["PackageDetails"]["PackageStatus"].as_str(),
        Some(PACKAGE_STATUS_COPYING),
        "case UpdatePackage(the live package): the new content is being copied, got {:?}",
        updated.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let history = emulator
        .call("GET", &format!("{PREFIX}/packages/{live}/history"), None)
        .await;
    let versions: Vec<String> = history.body["PackageVersionHistoryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case GetPackageVersionHistory(after the update): expected a history, got {:?}",
                history.body
            )
        })
        .iter()
        .filter_map(|version| version["PackageVersion"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        versions,
        vec!["2".to_string(), "1".to_string()],
        "case GetPackageVersionHistory(after the update): expected the update to have recorded a \
         version, got {versions:?}"
    );
}
