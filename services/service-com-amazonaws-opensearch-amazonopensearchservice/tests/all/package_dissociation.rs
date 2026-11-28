// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for taking a package OFF a domain: `DissociatePackages` and
//! `DissociatePackage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the package helpers from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a dissociation removes the link the call names and answers with the stored
//!   record, over a table of calls spanning every `PackageType` value, batches of
//!   one, two and four packages, and BOTH wire forms (the one that unlinks several
//!   packages and the one that unlinks a single package);
//! * the link is left in a transient status and the clock owes it the removal that
//!   takes the package off the domain: every row is `DISSOCIATING` the instant the
//!   call returns and gone once the span has passed, without any call having asked
//!   for the move;
//! * a dissociation is refused while another change to one of the domain's
//!   associations has not landed yet — the association being put in place, and the
//!   removal of any other package — and the change in flight still lands;
//! * a request that cannot be served dissociates NOTHING, over the ways it cannot be
//!   served — including a `PackageList` longer than the packages the account holds —
//!   each with the status and error code the model declares.
//!
//! The domain's associations are read back over the wire: `ListPackagesForDomain` and
//! `ListDomainsForPackage` must both stop reporting a link once the clock has taken it
//! off, and a second dissociation of a link that is gone is refused — with the store
//! (reached exactly as a handler reaches it) used only to assert what no accepted call
//! has yet made observable.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DOMAIN_PACKAGE_STATUS_ACTIVE, DOMAIN_PACKAGE_STATUS_ASSOCIATING,
    DOMAIN_PACKAGE_STATUS_DISSOCIATING, PACKAGE_QUOTA, PACKAGE_TYPE_TXT_DICTIONARY,
};

use super::domain_lifecycle::{create_baseline, error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::{
    available_packages, member, stored_status, PACKAGE_TYPES, UNKNOWN_PACKAGE_ID,
};

/// `AssociatePackages`: `POST /2021-01-01/packages/associateMultiple`. A sibling suite
/// drives the same path, rather than growing a second copy of it.
pub(crate) async fn associate(
    emulator: &Emulator,
    domain: &str,
    package_ids: &[String],
) -> Answer {
    let package_list: Vec<Value> = package_ids
        .iter()
        .map(|package_id| json!({ "PackageID": package_id }))
        .collect();
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/associateMultiple"),
            Some(json!({ "DomainName": domain, "PackageList": package_list })),
        )
        .await
}

/// `DissociatePackages`: `POST /2021-01-01/packages/dissociateMultiple`.
pub(crate) async fn dissociate_many(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/dissociateMultiple"),
            Some(body),
        )
        .await
}

/// `DissociatePackage`: `POST /2021-01-01/packages/dissociate/{PackageID}/{DomainName}`.
pub(crate) async fn dissociate_one(emulator: &Emulator, domain: &str, package_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/dissociate/{package_id}/{domain}"),
            None,
        )
        .await
}

/// `ListPackagesForDomain`: `GET /2021-01-01/domain/{DomainName}/packages` — the wire
/// read path for a domain's associations, so a case asserts what a client sees rather
/// than only what the store holds. A sibling suite reads the same listing through it.
pub(crate) async fn packages_of_domain(
    emulator: &Emulator,
    domain: &str,
    case: &str,
) -> Vec<Value> {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/domain/{domain}/packages"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainPackageDetailsList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// `ListDomainsForPackage`: `GET /2021-01-01/packages/{PackageID}/domains` — the other
/// wire read path for the same records, so a case asserts the two cannot disagree.
async fn domains_of_package(emulator: &Emulator, package_id: &str, case: &str) -> Vec<Value> {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/packages/{package_id}/domains"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainPackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DomainPackageDetailsList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `DomainPackageStatus` the domain's listing reports for `package_id`, or `None`
/// where that listing no longer reports the package at all.
pub(crate) async fn listed_status(
    emulator: &Emulator,
    domain: &str,
    package_id: &str,
    case: &str,
) -> Option<String> {
    packages_of_domain(emulator, domain, case)
        .await
        .into_iter()
        .find(|details| details["PackageID"].as_str() == Some(package_id))
        .and_then(|details| {
            details["DomainPackageStatus"]
                .as_str()
                .map(str::to_owned)
        })
}

/// The `DomainPackageStatus` the package's listing reports for `domain`, or `None`
/// where that listing no longer reports the domain at all.
async fn listed_domain_status(
    emulator: &Emulator,
    package_id: &str,
    domain: &str,
    case: &str,
) -> Option<String> {
    domains_of_package(emulator, package_id, case)
        .await
        .into_iter()
        .find(|details| details["DomainName"].as_str() == Some(domain))
        .and_then(|details| details["DomainPackageStatus"].as_str().map(str::to_owned))
}

/// Which wire form a row uses to dissociate: the call that unlinks several packages
/// at once, or the one that unlinks a single package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Many,
    One,
}

/// Dissociate `package_ids` from `domain` through `form`, and answer with the
/// `DomainPackageDetails` records the call reported — one per package it named,
/// whichever form answered, so a rule is asserted over both.
async fn dissociate(
    emulator: &Emulator,
    form: Form,
    domain: &str,
    package_ids: &[String],
) -> (Answer, Vec<Value>) {
    match form {
        Form::Many => {
            let answer =
                dissociate_many(emulator, json!({ "DomainName": domain, "PackageList": package_ids }))
                    .await;
            let reported = answer.body["DomainPackageDetailsList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            (answer, reported)
        }
        Form::One => {
            let package_id = package_ids
                .first()
                .expect("the single-package form names one package");
            let answer = dissociate_one(emulator, domain, package_id).await;
            let reported = answer.body["DomainPackageDetails"]
                .as_object()
                .map(|_| vec![answer.body["DomainPackageDetails"].clone()])
                .unwrap_or_default();
            (answer, reported)
        }
    }
}

/// Both wire forms, so every rule below is asserted over each.
const FORMS: [Form; 2] = [Form::Many, Form::One];

/// One dissociation to make: the domain it names and the created packages it unlinks,
/// in request order, through one wire form.
///
/// The package indexes are [`PACKAGE_TYPES`] indexes, whose set
/// `super::package_associations::every_package_type_is_covered` pins against the
/// model's own `PackageType` values.
struct Row {
    case: &'static str,
    domain: &'static str,
    packages: Vec<usize>,
    form: Form,
}

/// The dissociation table: one row per way an accepted call differs from another — a
/// single package through each wire form, a batch of two, a batch spanning every
/// package type, and a package of a type with no reference path.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one dictionary package, several-package form",
            domain: "dissoc-single",
            packages: vec![0],
            form: Form::Many,
        },
        Row {
            case: "one dictionary package, single-package form",
            domain: "dissoc-one-dict",
            packages: vec![0],
            form: Form::One,
        },
        Row {
            case: "one plugin package, single-package form",
            domain: "dissoc-one-plugin",
            packages: vec![1],
            form: Form::One,
        },
        Row {
            case: "a batch of two",
            domain: "dissoc-batch-two",
            packages: vec![1, 2],
            form: Form::Many,
        },
        Row {
            case: "a batch spanning every package type",
            domain: "dissoc-every-type",
            packages: vec![0, 1, 2, 3],
            form: Form::Many,
        },
    ]
}

/// A domain holding a settled (`ACTIVE`) association with each of `packages`, so a
/// dissociation of it is judged with no other change in flight.
pub(crate) async fn with_associations(
    emulator: &Emulator,
    domain: &str,
    ids: &[String],
    packages: &[usize],
) {
    create_baseline(emulator, domain).await;
    let named: Vec<String> = packages.iter().map(|index| ids[*index].clone()).collect();
    let answer = associate(emulator, domain, &named).await;
    assert_eq!(
        answer.status, 200,
        "case AssociatePackages({domain:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    for (index, package_id) in packages.iter().zip(&named) {
        assert_eq!(
            stored_status(emulator, domain, package_id).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
            "case AssociatePackages({domain:?}): package {index} must be in place before it is \
             taken off"
        );
    }
}

#[tokio::test]
async fn every_named_package_is_taken_off_the_domain_and_reported_from_the_stored_record() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    for row in rows() {
        with_associations(&emulator, row.domain, &ids, &row.packages).await;
        let named: Vec<String> = row.packages.iter().map(|index| ids[*index].clone()).collect();
        let (answer, reported) = dissociate(&emulator, row.form, row.domain, &named).await;

        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            reported.len(),
            row.packages.len(),
            "case {}: one record per package the call named, got {reported:?}",
            row.case
        );

        for (index, details) in row.packages.iter().zip(&reported) {
            let case = format!("{} / package {index}", row.case);
            let package_id = &ids[*index];
            let package_type = PACKAGE_TYPES[*index];

            assert_eq!(
                member(details, "PackageID", &case),
                *package_id,
                "case {case}: the record names the package it unlinked, got {details:?}"
            );
            assert_eq!(
                member(details, "PackageName", &case),
                format!("package-{index}"),
                "case {case}: the record reports the package's own name, got {details:?}"
            );
            assert_eq!(
                member(details, "PackageType", &case),
                package_type,
                "case {case}: the record reports the package's own type, got {details:?}"
            );
            assert_eq!(
                member(details, "DomainName", &case),
                row.domain,
                "case {case}: the record names the domain it was taken off, got {details:?}"
            );
            assert_eq!(
                member(details, "DomainPackageStatus", &case),
                DOMAIN_PACKAGE_STATUS_DISSOCIATING,
                "case {case}: the package is being taken off the domain, got {details:?}"
            );
            // The packages were created once and never updated, so the version the
            // association carried is the first one, and a removal reports the record
            // it is removing rather than inventing one.
            assert_eq!(
                member(details, "PackageVersion", &case),
                "v1",
                "case {case}: the record carries the version the association had, got {details:?}"
            );
            assert!(
                details["LastUpdated"].is_number(),
                "case {case}: expected a LastUpdated timestamp, got {details:?}"
            );
            assert!(
                details["ErrorDetails"].is_null(),
                "case {case}: a removal that is not in an error state reports no ErrorDetails, \
                 got {details:?}"
            );
            if package_type == PACKAGE_TYPE_TXT_DICTIONARY {
                assert_eq!(
                    member(details, "ReferencePath", &case),
                    format!("analyzers/{package_id}"),
                    "case {case}: a dictionary package is referenced by path, got {details:?}"
                );
            } else {
                assert!(
                    details["ReferencePath"].is_null(),
                    "case {case}: content loaded onto the nodes has no reference path, \
                     got {details:?}"
                );
            }

            assert_eq!(
                stored_status(&emulator, row.domain, package_id).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_DISSOCIATING),
                "case {case}: the record the call answered with is the one persisted"
            );
        }
    }
}

#[tokio::test]
async fn an_association_disappears_from_the_domain_on_the_clock() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    for form in FORMS {
        let domain = match form {
            Form::Many => "dissoc-clock-many",
            Form::One => "dissoc-clock-one",
        };
        let case = format!("{form:?} / {domain}");
        with_associations(&emulator, domain, &ids, &[0]).await;
        let (answer, _) = dissociate(&emulator, form, domain, &ids[0..1]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        // The package is still on the domain the instant the call returns, in the
        // status that says it is being taken off — as BOTH wire read paths for the
        // record report it.
        assert_eq!(
            listed_status(&emulator, domain, &ids[0], &case)
                .await
                .as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_DISSOCIATING),
            "case {case}: the domain's listing reports the removal in flight"
        );
        assert_eq!(
            listed_domain_status(&emulator, &ids[0], domain, &case)
                .await
                .as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_DISSOCIATING),
            "case {case}: the package's listing reports the same removal in flight"
        );

        // Nothing below asks for the removal: the span the call recorded falls due on
        // its own, and the next read materialises it.
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_eq!(
            listed_status(&emulator, domain, &ids[0], &case).await,
            None,
            "case {case}: the domain's listing no longer reports the package"
        );
        assert_eq!(
            listed_domain_status(&emulator, &ids[0], domain, &case).await,
            None,
            "case {case}: the package's listing no longer reports the domain"
        );
        assert_eq!(
            stored_status(&emulator, domain, &ids[0]),
            None,
            "case {case}: the package the clock owed a removal is no longer on the domain"
        );

        // Read back over the wire: the link is gone, so a second dissociation of the
        // same pair has nothing to remove.
        let (again, _) = dissociate(&emulator, form, domain, &ids[0..1]).await;
        assert_eq!(
            again.status, 400,
            "case {case}(again): expected 400, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ValidationException",
            "case {case}(again): expected the modelled validation refusal, got {:?}",
            again.body
        );

        // The package itself is untouched by the removal of a link to it, so it can be
        // put back on the domain.
        let reassociated = associate(&emulator, domain, &ids[0..1]).await;
        assert_eq!(
            reassociated.status, 200,
            "case {case}(reassociate): expected 200, got {} with {:?}",
            reassociated.status, reassociated.body
        );
        assert_eq!(
            stored_status(&emulator, domain, &ids[0]).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ASSOCIATING),
            "case {case}(reassociate): the package is being put back on the domain"
        );
    }
}

#[tokio::test]
async fn a_dissociation_is_refused_while_another_change_to_the_domains_associations_is_in_flight() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    for form in FORMS {
        let domain = match form {
            Form::Many => "dissoc-race-many",
            Form::One => "dissoc-race-one",
        };
        let case = format!("{form:?} / {domain}");
        create_baseline(&emulator, domain).await;

        // The association is still being put in place, so a removal of that same pair
        // is refused rather than overtaking it or half-applying.
        let opened = associate(&emulator, domain, &ids[0..1]).await;
        assert_eq!(
            opened.status, 200,
            "case {case}: expected the association to open, got {} with {:?}",
            opened.status, opened.body
        );
        let (racing, _) = dissociate(&emulator, form, domain, &ids[0..1]).await;
        assert_eq!(
            racing.status, 409,
            "case {case}(associating): expected 409, got {} with {:?}",
            racing.status, racing.body
        );
        assert_eq!(
            error_code(&racing),
            "ConflictException",
            "case {case}(associating): expected the modelled conflict refusal, got {:?}",
            racing.body
        );

        // The refused removal left the change in flight alone: it lands on its own
        // span, and the domain holds exactly ONE association for the pair.
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_eq!(
            stored_status(&emulator, domain, &ids[0]).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
            "case {case}(settled): the association the refused call raced is in place"
        );
        let held = packages_of_domain(&emulator, domain, &case).await;
        assert_eq!(
            held.len(),
            1,
            "case {case}(settled): the domain's listing reports exactly one association, \
             got {held:?}"
        );

        // The same removal, once nothing is in flight, is served.
        let (accepted, reported) = dissociate(&emulator, form, domain, &ids[0..1]).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}(solo): expected 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            reported.len(),
            1,
            "case {case}(solo): the call unlinked the one package it named, got {reported:?}"
        );

        // A removal is itself a change to the domain's associations: while it has not
        // landed, neither a second removal nor an association of ANOTHER package
        // overtakes it.
        let (overtaking, _) = dissociate(&emulator, form, domain, &ids[0..1]).await;
        assert_eq!(
            error_code(&overtaking),
            "ConflictException",
            "case {case}(dissociating): expected the modelled conflict refusal, got {:?}",
            overtaking.body
        );
        assert_eq!(
            overtaking.status, 409,
            "case {case}(dissociating): expected 409, got {} with {:?}",
            overtaking.status, overtaking.body
        );
        let other = associate(&emulator, domain, &ids[1..2]).await;
        assert_eq!(
            error_code(&other),
            "ConflictException",
            "case {case}(associate during removal): expected the modelled conflict refusal, \
             got {:?}",
            other.body
        );
        assert_eq!(
            stored_status(&emulator, domain, &ids[1]),
            None,
            "case {case}(associate during removal): a refused request associates nothing"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_dissociates_nothing() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;
    let domain = "dissoc-refusals";
    let sibling = "dissoc-sibling";
    with_associations(&emulator, domain, &ids, &[0, 1]).await;
    with_associations(&emulator, sibling, &ids, &[2]).await;

    // One row per way a request cannot be served, each with the status and error code
    // the model declares for it: `ValidationException` at HTTP 400. A `DomainName` that
    // resolves to no domain and a `PackageID` that resolves to no package are read from
    // the request like any other value, so each is the validation refusal — the same one
    // the associate path answers; a link the domain does not hold is a relationship the
    // request names and the service cannot dissolve, so it is the same refusal. A list
    // longer than the packages the account can hold is refused by its first entry that
    // resolves to nothing, which is what keeps a call's work bounded by the store rather
    // than by the request.
    let more_than_the_account_holds: Vec<String> = (0..=PACKAGE_QUOTA)
        .map(|index| format!("pkg-{index:032x}"))
        .collect();
    let table: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a domain nothing created",
            json!({ "DomainName": "dissoc-no-such-domain", "PackageList": [ids[0]] }),
            400,
            "ValidationException",
        ),
        (
            "a package nothing created",
            json!({ "DomainName": domain, "PackageList": [UNKNOWN_PACKAGE_ID] }),
            400,
            "ValidationException",
        ),
        (
            "a stored package the domain never had",
            json!({ "DomainName": domain, "PackageList": [ids[3]] }),
            400,
            "ValidationException",
        ),
        (
            "a package associated with another domain",
            json!({ "DomainName": domain, "PackageList": [ids[2]] }),
            400,
            "ValidationException",
        ),
        (
            "a batch whose last package the domain never had",
            json!({ "DomainName": domain, "PackageList": [ids[0], ids[1], ids[3]] }),
            400,
            "ValidationException",
        ),
        (
            "the same package named twice",
            json!({ "DomainName": domain, "PackageList": [ids[0], ids[0]] }),
            400,
            "ValidationException",
        ),
        (
            "more packages than the account holds",
            json!({ "DomainName": domain, "PackageList": more_than_the_account_holds }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, status, error) in table {
        let answer = dissociate_many(&emulator, body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            error,
            "case {case}: expected {error}, got {:?}",
            answer.body
        );
        // A refused request dissociates nothing: every association the domains held is
        // still in place, in the status it was in.
        for index in [0_usize, 1] {
            assert_eq!(
                stored_status(&emulator, domain, &ids[index]).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
                "case {case}: the association with package {index} is as it was"
            );
        }
        assert_eq!(
            stored_status(&emulator, sibling, &ids[2]).as_deref(),
            Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
            "case {case}: the sibling domain's association is as it was"
        );
    }

    // The single-package form refuses the same request shapes the same way: a domain
    // nothing created, a package nothing created, a stored package the domain never
    // had, and one associated with another domain.
    let unknown = UNKNOWN_PACKAGE_ID.to_string();
    for (case, name, package_id, status, error) in [
        (
            "a domain nothing created",
            "dissoc-no-such-domain",
            &ids[0],
            400,
            "ValidationException",
        ),
        (
            "a package nothing created",
            domain,
            &unknown,
            400,
            "ValidationException",
        ),
        (
            "a stored package the domain never had",
            domain,
            &ids[3],
            400,
            "ValidationException",
        ),
        (
            "a package associated with another domain",
            domain,
            &ids[2],
            400,
            "ValidationException",
        ),
    ] {
        let answer = dissociate_one(&emulator, name, package_id).await;
        assert_eq!(
            answer.status, status,
            "case single-package form / {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            error,
            "case single-package form / {case}: expected {error}, got {:?}",
            answer.body
        );
        for index in [0_usize, 1] {
            assert_eq!(
                stored_status(&emulator, domain, &ids[index]).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
                "case single-package form / {case}: the association with package {index} is as \
                 it was"
            );
        }
    }

    // A request that names NO package at all is served rather than refused, because an
    // empty `PackageList` asks for no removal: the call reports the empty set and the
    // domain's associations are exactly what they were — the same answer the associate
    // path gives an empty list, so the two forms of the batch call agree. It holds for a
    // `DomainName` the account holds no domain under too: this call answers about the
    // LINKS it was asked to remove, and a request that names none asks for nothing, so
    // there is nothing to refuse and nothing is disclosed about the domain.
    for (case, name) in [
        ("a call naming no package", domain),
        (
            "a call naming no package on a domain nothing created",
            "dissoc-no-such-domain",
        ),
    ] {
        let empty = dissociate_many(
            &emulator,
            json!({ "DomainName": name, "PackageList": [] }),
        )
        .await;
        assert_eq!(
            empty.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            empty.status, empty.body
        );
        assert_eq!(
            empty.body["DomainPackageDetailsList"],
            json!([]),
            "case {case}: expected no records, got {:?}",
            empty.body
        );
        for index in [0_usize, 1] {
            assert_eq!(
                stored_status(&emulator, domain, &ids[index]).as_deref(),
                Some(DOMAIN_PACKAGE_STATUS_ACTIVE),
                "case {case}: the association with package {index} is as it was"
            );
        }
    }
}
