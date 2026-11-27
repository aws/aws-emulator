// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the package/domain **association** surface —
//! `AssociatePackage` and the two reads of it, `ListPackagesForDomain` and
//! `ListDomainsForPackage` — driven as real restJson1 requests through the
//! router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded call:
//!
//! 1. A domain takes a package only once it is on the latest service software,
//!    which a domain still provisioning a change is not: that associate is
//!    refused with the modelled 400 `ValidationException` and writes nothing, so
//!    both lists still report an empty `DomainPackageDetailsList`.
//! 2. An accepted associate PERSISTS the association and answers with the stored
//!    record's own `DomainPackageDetails`; both lists answer from that same
//!    record, so the three cannot disagree.
//! 3. The association is `ASSOCIATING` for a non-zero span and then settles on
//!    `ACTIVE`, and no read moves it.
//! 4. Each list reports only the associations of the resource it names, in a
//!    stable order, and `MaxResults`/`NextToken` page that set.
//! 5. A package or a domain the Region does not hold is not found.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Longer than the emulator's provisioning span, so a transition is certainly
/// due.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// The refusal AWS gives an associate against a domain that is not yet on the
/// latest service software. The sentence pair is the whole message, trailing
/// space included; nothing is interpolated into it.
const NOT_ON_LATEST_SOFTWARE: &str =
    "The domain is not on the latest software version. Upgrade the domain to start using this \
     feature. ";

async fn create_domain(router: &MultiServiceRouter, name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({
            "DomainName": name,
            "ElasticsearchVersion": "7.10",
            "ElasticsearchClusterConfig": {
                "InstanceType": "t2.small.elasticsearch",
                "InstanceCount": 1
            },
            "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 10}
        })),
    )
    .await
}

/// A created domain's name, asserted to have been stored.
async fn domain(router: &MultiServiceRouter, name: &str) -> String {
    let created = create_domain(router, name).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the domain create must answer 200, got {} {}",
        created.status, created.body
    );
    name.to_string()
}

/// A created package's minted id.
async fn package(router: &MultiServiceRouter, name: &str) -> String {
    let created = call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/packages",
        Some(&json!({
            "PackageName": name,
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the package create must answer 200, got {} {}",
        created.status, created.body
    );
    created.text(name, "/PackageDetails/PackageID")
}

async fn associate(router: &MultiServiceRouter, package_id: &str, domain_name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        &format!("/2015-01-01/packages/associate/{package_id}/{domain_name}"),
        None,
    )
    .await
}

async fn packages_for_domain(
    router: &MultiServiceRouter,
    domain_name: &str,
    query: &str,
) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/domain/{domain_name}/packages{query}"),
        None,
    )
    .await
}

async fn domains_for_package(router: &MultiServiceRouter, package_id: &str, query: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/packages/{package_id}/domains{query}"),
        None,
    )
    .await
}

/// The `DomainPackageDetails` entries a list reported, in the order it gave
/// them.
fn listed(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a list must answer 200, got {} {}",
        answer.status, answer.body
    );
    answer
        .at(case, "/DomainPackageDetailsList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: DomainPackageDetailsList is not a list, got {body}",
                body = answer.body
            )
        })
        .clone()
}

/// The value of `member` in every entry a list reported.
fn member_of(entries: &[Value], member: &str) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .pointer(&format!("/{member}"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE 1: a domain that has not finished provisioning is not on the latest
/// service software, so the associate is refused and nothing is written.
#[tokio::test]
async fn an_associate_on_a_domain_that_is_still_provisioning_is_refused_and_writes_nothing() {
    // Rows differ along WHICH change left the domain provisioning: its own
    // create, and a later config update that starts a fresh span.
    for case in ["while the create is provisioning", "while an update is provisioning"] {
        let router = router();
        let package_id = package(&router, "assoc-gate-dict").await;
        let domain_name = domain(&router, "assoc-gate-domain").await;

        if case == "while an update is provisioning" {
            // Let the create settle, then start another change: the gate is
            // about the domain being mid-change, not about it being new.
            tokio::time::sleep(PAST_PROVISIONING).await;
            let updated = call_in(
                &router,
                REGION,
                "POST",
                &format!("/2015-01-01/es/domain/{domain_name}/config"),
                Some(&json!({
                    "ElasticsearchClusterConfig": {
                        "InstanceType": "t2.small.elasticsearch",
                        "InstanceCount": 2
                    }
                })),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: the config update must answer 200, got {} {}",
                updated.status, updated.body
            );
        }

        let refused = associate(&router, &package_id, &domain_name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the associate must be refused with 400, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.body.pointer("/message").and_then(Value::as_str),
            Some(NOT_ON_LATEST_SOFTWARE),
            "case {case:?}: expected the latest-software sentence pair, got {:?}",
            refused.body.pointer("/message")
        );

        // The refused call is a no-op on BOTH sides of the association.
        let on_domain = packages_for_domain(&router, &domain_name, "").await;
        let on_package = domains_for_package(&router, &package_id, "").await;
        assert!(
            listed(&on_domain, case).is_empty(),
            "case {case:?}: the domain must report no associated package, got {}",
            on_domain.body
        );
        assert!(
            listed(&on_package, case).is_empty(),
            "case {case:?}: the package must report no associated domain, got {}",
            on_package.body
        );
    }
}

/// RULES 2 and 3: an accepted associate persists the association, both lists
/// answer from it, and the clock alone moves it from ASSOCIATING to ACTIVE.
#[tokio::test]
async fn an_accepted_associate_persists_and_both_lists_answer_from_the_store() {
    let router = router();
    let package_id = package(&router, "assoc-live-dict").await;
    let domain_name = domain(&router, "assoc-live-domain").await;
    // Both the domain's create and the package's copy are due by now.
    tokio::time::sleep(PAST_PROVISIONING).await;

    let associated = associate(&router, &package_id, &domain_name).await;
    assert_eq!(
        associated.status, 200,
        "the associate must answer 200, got {} {}",
        associated.status, associated.body
    );
    let details = "/DomainPackageDetails";
    for (member, value) in [
        ("PackageID", package_id.clone()),
        ("PackageName", "assoc-live-dict".to_string()),
        ("PackageType", "TXT-DICTIONARY".to_string()),
        ("DomainName", domain_name.clone()),
        ("DomainPackageStatus", "ASSOCIATING".to_string()),
        ("PackageVersion", "v1".to_string()),
        ("ReferencePath", format!("analyzers/{package_id}")),
    ] {
        assert_eq!(
            associated.at("the accepted associate", &format!("{details}/{member}")),
            &json!(value),
            "case {member:?}: expected {value:?}, got {:?}",
            associated.body.pointer(&format!("{details}/{member}"))
        );
    }
    assert!(
        !associated.missing(&format!("{details}/LastUpdated")),
        "LastUpdated must be reported, got {}",
        associated.body
    );

    // Two reads inside the install span agree: a read must not move the status.
    for case in ["first read while associating", "second read while associating"] {
        for entries in [
            listed(&packages_for_domain(&router, &domain_name, "").await, case),
            listed(&domains_for_package(&router, &package_id, "").await, case),
        ] {
            assert_eq!(
                member_of(&entries, "DomainPackageStatus"),
                vec!["ASSOCIATING".to_string()],
                "case {case:?}: the association must still be ASSOCIATING, got {entries:?}"
            );
            assert_eq!(
                member_of(&entries, "PackageID"),
                vec![package_id.clone()],
                "case {case:?}: the entry must name the associated package, got {entries:?}"
            );
            assert_eq!(
                member_of(&entries, "DomainName"),
                vec![domain_name.clone()],
                "case {case:?}: the entry must name the associated domain, got {entries:?}"
            );
        }
    }

    // The transition is due by the clock alone; nothing polls for it.
    tokio::time::sleep(PAST_PROVISIONING).await;
    for case in ["first read once installed", "second read once installed"] {
        for entries in [
            listed(&packages_for_domain(&router, &domain_name, "").await, case),
            listed(&domains_for_package(&router, &package_id, "").await, case),
        ] {
            assert_eq!(
                member_of(&entries, "DomainPackageStatus"),
                vec!["ACTIVE".to_string()],
                "case {case:?}: the install is due, so the association must be ACTIVE, got \
                 {entries:?}"
            );
        }
    }

    // Associating the same pair again reports the association as it stands and
    // adds no second entry.
    let again = associate(&router, &package_id, &domain_name).await;
    assert_eq!(
        again.status, 200,
        "a repeated associate must answer 200, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.at("the repeated associate", &format!("{details}/DomainPackageStatus")),
        &json!("ACTIVE"),
        "a repeated associate must report the association as it stands, got {:?}",
        again.body.pointer(&format!("{details}/DomainPackageStatus"))
    );
    assert_eq!(
        listed(
            &packages_for_domain(&router, &domain_name, "").await,
            "after the repeat"
        )
        .len(),
        1,
        "a repeated associate must not add a second entry"
    );
}

/// RULE 4: each list reports the associations of the resource it names, in a
/// stable order, and pages them.
#[tokio::test]
async fn each_list_reports_its_own_resources_associations_and_pages_them() {
    let router = router();
    // Three packages and two domains, associated in a cross pattern: every
    // package on the first domain, and only the first package on the second.
    let mut package_ids = Vec::new();
    for name in ["assoc-dict-one", "assoc-dict-two", "assoc-dict-three"] {
        package_ids.push(package(&router, name).await);
    }
    let first = domain(&router, "assoc-domain-first").await;
    let second = domain(&router, "assoc-domain-second").await;
    tokio::time::sleep(PAST_PROVISIONING).await;

    for package_id in &package_ids {
        let answer = associate(&router, package_id, &first).await;
        assert_eq!(
            answer.status, 200,
            "associating {package_id} with {first} must answer 200, got {} {}",
            answer.status, answer.body
        );
    }
    let answer = associate(&router, &package_ids[0], &second).await;
    assert_eq!(
        answer.status, 200,
        "associating the shared package with {second} must answer 200, got {} {}",
        answer.status, answer.body
    );

    // Rows: each resource, with the set it must report.
    let on_first = listed(&packages_for_domain(&router, &first, "").await, "the first domain");
    let mut ids_on_first = member_of(&on_first, "PackageID");
    let mut expected_ids = package_ids.clone();
    ids_on_first.sort();
    expected_ids.sort();
    assert_eq!(
        ids_on_first, expected_ids,
        "the first domain must report every package associated with it"
    );

    let on_second = listed(
        &packages_for_domain(&router, &second, "").await,
        "the second domain",
    );
    assert_eq!(
        member_of(&on_second, "PackageID"),
        vec![package_ids[0].clone()],
        "the second domain must report only its own association"
    );

    let shared = listed(
        &domains_for_package(&router, &package_ids[0], "").await,
        "the shared package",
    );
    let mut domains_of_shared = member_of(&shared, "DomainName");
    domains_of_shared.sort();
    assert_eq!(
        domains_of_shared,
        vec![first.clone(), second.clone()],
        "the shared package must report both domains"
    );

    let lone = listed(
        &domains_for_package(&router, &package_ids[1], "").await,
        "a package on one domain",
    );
    assert_eq!(
        member_of(&lone, "DomainName"),
        vec![first.clone()],
        "a package associated once must report that one domain"
    );

    // A list reports the same order twice, which is what lets a token name a
    // place in the set.
    let repeated = listed(
        &packages_for_domain(&router, &first, "").await,
        "the same request twice",
    );
    assert_eq!(
        member_of(&repeated, "PackageID"),
        member_of(&on_first, "PackageID"),
        "two identical lists must report the same order"
    );

    // Rows differ along the page size, including one past the set's own size.
    for page_size in [1_usize, 2, 3, 5] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = packages_for_domain(&router, &first, &query).await;
            let entries = listed(&page, &case);
            assert!(
                entries.len() <= page_size,
                "case {case:?}: a page must not exceed the page size, got {} entries",
                entries.len()
            );
            walked.extend(entries);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: paging must terminate, walked {pages} pages"
            );
            token = page
                .body
                .pointer("/NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            member_of(&walked, "PackageID"),
            member_of(&on_first, "PackageID"),
            "case {case:?}: paging must walk the whole set in order"
        );
    }

    for (case, query) in [
        (
            "a token this service never minted",
            "?nextToken=pkg-not-a-real-token",
        ),
        ("an empty token", "?nextToken="),
    ] {
        let refused = packages_for_domain(&router, &first, query).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token naming no entry must be refused with 400, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            refused.error_type,
            refused.body
        );
    }
}

/// RULE 2 (lifetime): an association belongs to the domain it is on, so
/// deleting the domain takes the association with it.
#[tokio::test]
async fn deleting_a_domain_takes_its_associations_with_it() {
    let router = router();
    let package_id = package(&router, "assoc-deleted-dict").await;
    let kept = domain(&router, "assoc-kept-domain").await;
    let doomed = domain(&router, "assoc-doomed-domain").await;
    tokio::time::sleep(PAST_PROVISIONING).await;

    for domain_name in [&kept, &doomed] {
        let answer = associate(&router, &package_id, domain_name).await;
        assert_eq!(
            answer.status, 200,
            "associating with {domain_name} must answer 200, got {} {}",
            answer.status, answer.body
        );
    }
    let before = listed(
        &domains_for_package(&router, &package_id, "").await,
        "before the delete",
    );
    assert_eq!(
        before.len(),
        2,
        "the package must report both domains first, got {before:?}"
    );

    let deleted = call_in(
        &router,
        REGION,
        "DELETE",
        &format!("/2015-01-01/es/domain/{doomed}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must answer 200, got {} {}",
        deleted.status, deleted.body
    );

    let after = listed(
        &domains_for_package(&router, &package_id, "").await,
        "after the delete",
    );
    assert_eq!(
        member_of(&after, "DomainName"),
        vec![kept.clone()],
        "the package must report only the domain that still exists, got {after:?}"
    );
}

/// RULE 5: a package or domain this Region does not hold is not found, and the
/// refusal writes nothing.
#[tokio::test]
async fn a_resource_the_region_does_not_hold_is_not_found() {
    let router = router();
    let package_id = package(&router, "assoc-missing-dict").await;
    let domain_name = domain(&router, "assoc-missing-dom").await;
    tokio::time::sleep(PAST_PROVISIONING).await;

    // Rows differ along WHICH half of the pair is absent, and in one row the
    // pair exists but in another Region.
    for (case, id, name, region) in [
        (
            "a package id nothing minted",
            "pkg-0000000000000000".to_string(),
            domain_name.clone(),
            REGION,
        ),
        (
            "a domain that was never created",
            package_id.clone(),
            "assoc-absent-dom".to_string(),
            REGION,
        ),
        (
            "both halves in a Region that holds neither",
            package_id.clone(),
            domain_name.clone(),
            "eu-west-1",
        ),
    ] {
        let refused = call_in(
            &router,
            region,
            "POST",
            &format!("/2015-01-01/packages/associate/{id}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: the associate must be refused with 404, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            refused.error_type,
            refused.body
        );
    }

    for (case, answer) in [
        (
            "a domain that was never created",
            packages_for_domain(&router, "assoc-absent-dom", "").await,
        ),
        (
            "a package id nothing minted",
            domains_for_package(&router, "pkg-0000000000000000", "").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: the list must be refused with 404, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
    }

    let after = packages_for_domain(&router, &domain_name, "").await;
    assert!(
        listed(&after, "after every refusal").is_empty(),
        "a refused associate must write nothing, got {}",
        after.body
    );
}
