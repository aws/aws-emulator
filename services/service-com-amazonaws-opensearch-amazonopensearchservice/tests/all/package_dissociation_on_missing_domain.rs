// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the package-association calls answer a `DomainName` the
//! account holds no domain under — and, in particular, that each of them answers it
//! with ITS OWN sentence.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. This suite adds no second copy of anything
//! the package suites already have: the harness comes from [`super::domain_lifecycle`],
//! the package helpers from [`super::package_associations`], the dissociate request
//! helpers from [`super::package_dissociation`], the associate and listing helpers from
//! [`super::package_listing_page_bounds`], and the table of names no domain is held
//! under from [`super::package_listing_for_missing_domain`].
//!
//! The rules asserted here are:
//!
//! * a refusal's WORDING belongs to the operation that answers, not to the decision
//!   the operations share: for a `DomainName` that resolves to no domain, the call that
//!   unlinks a single package is refused with the service's fixed sentence about a
//!   domain that is not on the latest software version, the calls that link a package
//!   are refused with the sentence naming the domain that does not exist, and the call
//!   that unlinks SEVERAL answers about the link it was asked to remove — a domain this
//!   account does not hold holds no link either — over the table of such names and over
//!   a table of `PackageID`s the pattern admits, including one of a package the account
//!   holds;
//! * that sentence is bound to the name that could not be resolved, not to the
//!   operation: on a domain that EXISTS, the same call keeps the service's own sentence
//!   for a package that resolves to nothing and for a link the domain is not holding,
//!   and serves the link it is holding;
//! * every one of these refusals leaves the domain's links exactly as they were.

use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_NOT_ON_LATEST_SOFTWARE_VERSION, DOMAIN_PACKAGE_STATUS_DISSOCIATING, PACKAGE_NOT_FOUND,
    PACKAGE_NOT_ASSOCIATED,
};
use serde_json::json;

use super::domain_lifecycle::{error_code, Answer, Emulator};
use super::package_associations::{available_packages, member, UNKNOWN_PACKAGE_ID};
use super::package_dissociation::{dissociate_many, dissociate_one, with_associations};
use super::package_listing_for_missing_domain::missing_names;
use super::package_listing_page_bounds::{associate_many, associate_one, details, list_for_domain};

/// The ids every table below carries, each one `PackageID` admits (`@pattern`
/// `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`, from
/// `smithy_query(query="operation",operation="DissociatePackage")`) that resolves to no
/// package of this account: the shortest of each alternative the pattern allows, the
/// service-minted form, and the hex form a create of this account mints.
const UNRESOLVABLE_IDS: [(&str, &str); 4] = [
    ("an id no create of this account minted", UNKNOWN_PACKAGE_ID),
    ("the shortest id the first alternative admits", "G7"),
    ("a service-minted id", "F123456"),
    ("the shortest id the second alternative admits", "pkg-a"),
];

/// The status, error code and message of a refused answer, with the case named on
/// failure: the three things a client branches on.
fn refusal(answer: &Answer, case: &str) -> (u16, String, String) {
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned();
    (answer.status, error_code(answer), message)
}

/// Every `PackageID` a case drives, paired with what it is: the ids that resolve to no
/// package, plus the id of a package this account holds — so a refusal about the domain
/// cannot be the package lookup answering instead.
fn package_ids(held: &str) -> Vec<(String, String)> {
    let mut ids: Vec<(String, String)> = UNRESOLVABLE_IDS
        .iter()
        .map(|(case, id)| ((*case).to_owned(), (*id).to_owned()))
        .collect();
    ids.push((
        "an id of a package this account holds".to_owned(),
        held.to_owned(),
    ));
    ids
}

#[tokio::test]
async fn each_package_call_refuses_a_name_no_domain_holds_with_its_own_sentence() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let names = missing_names(&emulator).await;

    for (name_case, name) in &names {
        for (id_case, package_id) in package_ids(&ids[0]) {
            // The call that unlinks ONE package: the service's fixed sentence about a
            // domain that cannot serve the feature, which carries no part of the
            // request.
            let case = format!("DissociatePackage({name:?}, {name_case} / {id_case})");
            let answer = dissociate_one(&emulator, name, &package_id).await;
            assert_eq!(
                refusal(&answer, &case),
                (
                    400,
                    "ValidationException".to_owned(),
                    DOMAIN_NOT_ON_LATEST_SOFTWARE_VERSION.to_owned()
                ),
                "case {case}: expected the service's software-version refusal, got {} with {:?}",
                answer.status,
                answer.body
            );
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(name.as_str()) && !message.contains(package_id.as_str()),
                "case {case}: the sentence has no variable part, got {message:?}"
            );

            // The calls that LINK a package, for the very same name and id: the
            // sentence that names the domain that does not exist.
            for (call, answer) in [
                (
                    "AssociatePackage",
                    associate_one(&emulator, name, &package_id).await,
                ),
                (
                    "AssociatePackages",
                    associate_many(&emulator, name, std::slice::from_ref(&package_id)).await,
                ),
            ] {
                let case = format!("{call}({name:?}, {name_case} / {id_case})");
                assert_eq!(
                    refusal(&answer, &case),
                    (
                        400,
                        "ValidationException".to_owned(),
                        format!("The domain {name} does not exist.")
                    ),
                    "case {case}: expected the refusal naming the domain, got {} with {:?}",
                    answer.status,
                    answer.body
                );
            }

            // And the batch that unlinks packages, which answers such a name as the LINK
            // it names not being there — it discloses nothing about the domain — whatever
            // its `PackageList` carries.
            let case = format!("DissociatePackages({name:?}, {name_case} / {id_case})");
            let answer = dissociate_many(
                &emulator,
                json!({ "DomainName": name, "PackageList": [package_id] }),
            )
            .await;
            assert_eq!(
                refusal(&answer, &case),
                (
                    400,
                    "ValidationException".to_owned(),
                    PACKAGE_NOT_ASSOCIATED.to_owned()
                ),
                "case {case}: expected the service's not-associated sentence, got {} with {:?}",
                answer.status,
                answer.body
            );
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(name.as_str()),
                "case {case}: the sentence discloses nothing about the domain, got {message:?}"
            );
        }

        // None of the refused calls linked or unlinked anything: the listing of the
        // same name still reports the empty set.
        let case = format!("ListPackagesForDomain({name:?}, {name_case}, after the refusals)");
        let listed = details(&list_for_domain(&emulator, name, "").await, &case);
        assert!(
            listed.is_empty(),
            "case {case}: expected the empty set, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_software_version_refusal_belongs_to_the_name_that_resolved_to_no_domain() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "dissoc-gate-held";
    // The domain holds a settled link with the FIRST package only, so the second is one
    // the account holds and this domain does not.
    with_associations(&emulator, domain, &ids, &[0]).await;

    // An id that resolves to no package: the service's fixed sentence for that, not the
    // one about the domain.
    for (id_case, package_id) in UNRESOLVABLE_IDS {
        let case = format!("DissociatePackage({domain:?}, {id_case})");
        let answer = dissociate_one(&emulator, domain, package_id).await;
        assert_eq!(
            refusal(&answer, &case),
            (
                400,
                "ValidationException".to_owned(),
                PACKAGE_NOT_FOUND.to_owned()
            ),
            "case {case}: expected the service's package-not-found sentence, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // A package the account holds and this domain is not holding: the service's fixed
    // sentence for a link that is not there.
    let case = format!("DissociatePackage({domain:?}, a package the domain is not holding)");
    let answer = dissociate_one(&emulator, domain, &ids[1]).await;
    assert_eq!(
        refusal(&answer, &case),
        (
            400,
            "ValidationException".to_owned(),
            PACKAGE_NOT_ASSOCIATED.to_owned()
        ),
        "case {case}: expected the service's not-associated sentence, got {} with {:?}",
        answer.status,
        answer.body
    );

    // And the link the domain IS holding is taken off, so none of the refusals above is
    // this operation refusing the domain.
    let case = format!("DissociatePackage({domain:?}, the package the domain holds)");
    let answer = dissociate_one(&emulator, domain, &ids[0]).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        member(&answer.body["DomainPackageDetails"], "DomainPackageStatus", &case),
        DOMAIN_PACKAGE_STATUS_DISSOCIATING,
        "case {case}: expected the association in its removal status, got {:?}",
        answer.body
    );
}
