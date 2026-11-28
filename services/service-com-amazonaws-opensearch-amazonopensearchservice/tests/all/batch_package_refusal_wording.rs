// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW the package operations that act on SEVERAL packages at once
//! word what they refuse.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`],
//! the package helpers from [`super::package_associations`] and the two dissociate request
//! helpers from [`super::package_dissociation`]; this suite adds no second copy of any of
//! them.
//!
//! The rules under test, each over a SET of inputs rather than one recorded call:
//!
//! * a `PackageID` that resolves to no package visible to this account is refused by a
//!   batch call with the service's batch sentence, which NAMES that id — so a caller that
//!   listed several packages is told which of them the account has no package for — and the
//!   id in the sentence is the one the request sent, at whichever position in the list it
//!   sat, and whether it is the package to link or one of its prerequisites. The call that
//!   acts on ONE package answers the same condition with the sentence that names nothing of
//!   the request, which is what makes the batch wording a property of the CALL rather than
//!   of the id;
//! * `DissociatePackages` answers about the LINK it was asked to remove and never about the
//!   domain: a `DomainName` that names no domain of this account holds no link for the
//!   package either, so it is refused with the fixed not-associated sentence, exactly as a
//!   domain that exists and is not holding that package is — over a table of such names and
//!   over ids that do and do not resolve to a package;
//! * every one of these refusals leaves the stored links exactly as they were.

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PACKAGE_NOT_ASSOCIATED, PACKAGE_NOT_FOUND,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX};
use super::package_associations::UNKNOWN_PACKAGE_ID;
use super::package_dissociation::{dissociate_many, dissociate_one};

/// The domain the rows that name an existing one act on.
const DOMAIN: &str = "batch-pkg-wording";

/// The `DomainName`s no domain of this account is held under: a name nothing created, and
/// one whose every character class the member admits is exercised.
const ABSENT_DOMAINS: [&str; 3] = ["batch-pkg-absent", "no-such-domain-7", "abc"];

/// The ids every table carries that resolve to no package of this account — both forms
/// `PackageID`'s `@pattern` `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$` admits, at more than one
/// length.
const UNRESOLVABLE_IDS: [&str; 4] = [UNKNOWN_PACKAGE_ID, "pkg-abcdef0123456789", "G7", "F1234567"];

/// Two `PackageID`s this account can associate, read through `DescribePackages`: the
/// packages the SERVICE publishes are visible to every account and their content is in
/// place, so a row needs neither a create nor a wait for one to land.
async fn visible_packages(emulator: &Emulator) -> Vec<String> {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/describe"),
            Some(json!({ "MaxResults": 100 })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "reading the visible packages: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let ids: Vec<String> = answer.body["PackageDetailsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|details| details["PackageStatus"] == json!("AVAILABLE"))
        .filter_map(|details| details["PackageID"].as_str().map(str::to_owned))
        .collect();
    assert!(
        ids.len() >= 2,
        "the rows need two packages this account can associate, got {ids:?}"
    );
    ids
}

/// How the batch calls word a `PackageID` that resolves to no package: fixed text around
/// the id the REQUEST named. Spelled out here rather than taken from the crate, so a change
/// to either part fails this suite.
fn batch_package_not_found(package_id: &str) -> String {
    format!("Operation not allowed. Package with id {package_id} not found.")
}

/// `AssociatePackages` with the `PackageList` a row names, as it answered.
async fn associate_many(emulator: &Emulator, domain: &str, package_list: serde_json::Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(json!({ "DomainName": domain, "PackageList": package_list })),
        )
        .await
}

/// `AssociatePackage`, the call that links ONE package.
async fn associate_one(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associate/{package_id}/{domain}"),
            Some(json!({})),
        )
        .await
}

/// The links `ListPackagesForDomain` reports for `domain`, as `PackageID`s.
async fn linked(emulator: &Emulator, domain: &str) -> Vec<String> {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/domain/{domain}/packages"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "reading the links of {domain:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|details| details["PackageID"].as_str().map(str::to_owned))
        .collect()
}

/// The status, error code and message of a refused answer: the three things a client
/// branches on.
fn refusal(answer: &Answer, case: &str) -> (u16, String, String) {
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned();
    (answer.status, error_code(answer), message)
}

#[tokio::test]
async fn a_batch_names_the_package_id_it_could_not_resolve() {
    let emulator = Emulator::new();
    create_baseline(&emulator, DOMAIN).await;
    let held = visible_packages(&emulator).await;

    for unresolvable in UNRESOLVABLE_IDS {
        // Rows differ in how many packages the batch names and where the unresolvable id
        // sits: alone, before and after a package the account HOLDS, and as a prerequisite
        // of one it holds — the other place the call resolves an id.
        let rows = vec![
            ("alone", json!([{ "PackageID": unresolvable }])),
            (
                "after a package the account holds",
                json!([{ "PackageID": held[0] }, { "PackageID": unresolvable }]),
            ),
            (
                "before a package the account holds",
                json!([{ "PackageID": unresolvable }, { "PackageID": held[1] }]),
            ),
            (
                "as a prerequisite of a package the account holds",
                json!([{
                    "PackageID": held[0],
                    "PrerequisitePackageIDList": [unresolvable],
                }]),
            ),
        ];
        for (where_it_sits, package_list) in rows {
            let case = format!("AssociatePackages({unresolvable}, {where_it_sits})");
            let answer = associate_many(&emulator, DOMAIN, package_list).await;
            assert_eq!(
                refusal(&answer, &case),
                (
                    400,
                    "ValidationException".to_owned(),
                    batch_package_not_found(unresolvable)
                ),
                "case {case}: expected the batch sentence naming the id, got {} with {:?}",
                answer.status,
                answer.body
            );
        }

        // The batch that UNLINKS packages words the same condition the same way, on a
        // domain that exists so the id is what the call cannot resolve.
        let case = format!("DissociatePackages({unresolvable})");
        let answer = dissociate_many(
            &emulator,
            json!({ "DomainName": DOMAIN, "PackageList": [unresolvable] }),
        )
        .await;
        assert_eq!(
            refusal(&answer, &case),
            (
                400,
                "ValidationException".to_owned(),
                batch_package_not_found(unresolvable)
            ),
            "case {case}: expected the batch sentence naming the id, got {} with {:?}",
            answer.status,
            answer.body
        );

        // ... while the calls that act on ONE package answer that same condition with the
        // sentence that names nothing of the request: the wording belongs to the CALL.
        for (call, answer) in [
            (
                "AssociatePackage",
                associate_one(&emulator, DOMAIN, unresolvable).await,
            ),
            (
                "DissociatePackage",
                dissociate_one(&emulator, DOMAIN, unresolvable).await,
            ),
        ] {
            let case = format!("{call}({unresolvable})");
            assert_eq!(
                refusal(&answer, &case),
                (
                    400,
                    "ValidationException".to_owned(),
                    PACKAGE_NOT_FOUND.to_owned()
                ),
                "case {case}: expected the single-package sentence, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }

    // Not one of the refusals above linked anything.
    let links = linked(&emulator, DOMAIN).await;
    assert!(
        links.is_empty(),
        "case the refused calls: expected no link to have been made, got {links:?}"
    );
}

#[tokio::test]
async fn the_batch_dissociation_answers_about_the_link_and_not_about_the_domain() {
    let emulator = Emulator::new();
    create_baseline(&emulator, DOMAIN).await;
    let held = visible_packages(&emulator).await;

    // The link the account's domain is NOT holding, on the domain that exists: the
    // reference answer the rows below have to match.
    let case = format!("DissociatePackages({DOMAIN}, a package the domain is not holding)");
    let answer = dissociate_many(
        &emulator,
        json!({ "DomainName": DOMAIN, "PackageList": [held[0]] }),
    )
    .await;
    assert_eq!(
        refusal(&answer, &case),
        (
            400,
            "ValidationException".to_owned(),
            PACKAGE_NOT_ASSOCIATED.to_owned()
        ),
        "case {case}: expected the not-associated sentence, got {} with {:?}",
        answer.status,
        answer.body
    );

    // ... and a `DomainName` this account holds no domain under is answered identically,
    // whichever id the call names: the refusal is about the link, and it discloses nothing
    // about the domain.
    for absent in ABSENT_DOMAINS {
        for package_id in [held[0].as_str(), UNKNOWN_PACKAGE_ID, "G7"] {
            let case = format!("DissociatePackages({absent}, {package_id})");
            let answer = dissociate_many(
                &emulator,
                json!({ "DomainName": absent, "PackageList": [package_id] }),
            )
            .await;
            assert_eq!(
                refusal(&answer, &case),
                (
                    400,
                    "ValidationException".to_owned(),
                    PACKAGE_NOT_ASSOCIATED.to_owned()
                ),
                "case {case}: expected the not-associated sentence, got {} with {:?}",
                answer.status,
                answer.body
            );
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(absent) && !message.contains(package_id),
                "case {case}: the sentence has no variable part, got {message:?}"
            );
        }
    }

    // The domain that exists still holds nothing: every call above removed nothing.
    let links = linked(&emulator, DOMAIN).await;
    assert!(
        links.is_empty(),
        "case the refused calls: expected the links to be untouched, got {links:?}"
    );
}
