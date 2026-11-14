// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PAGE a `MaxRecords`-limited engine-version sweep serves and
//! the continuation token it carries, and for the catalogue facts a page of one engine
//! family rests on.
//!
//! Every call goes over the wire through the router the production emulator mounts
//! (`crate::db_instances::emulator`): a real awsQuery form POST in, the XML the emulator
//! answered with out.
//!
//! The rules under test, each stated over a TABLE spanning its class rather than over one
//! recorded request:
//!
//! 1. A `Marker` is an OPAQUE token the service issues and accepts: it is handed out
//!    exactly while records remain, it is not the plaintext key of the record it resumes
//!    at (that plaintext is refused, like any token no page issued), re-presenting it
//!    answers the same page, and walking it visits the report once whatever the page size.
//!    Spanned over paginated reports of this front door — one published catalogue and one
//!    report of stored records — and over the published `MaxRecords` bounds.
//! 2. A `MaxRecords` page is the report's OWN first page: the entries the walked report
//!    serves, in its order, and the token resumes exactly where the page stopped. Spanned
//!    over several engines and page sizes.
//! 3. A major version past its community end of life is published through its dated
//!    Amazon RDS Extended Support builds, and the builds the service still offers are
//!    `available` — so they are what a default (no `IncludeAll`) report of that family
//!    serves, and a family's offered builds can outnumber one smallest page. Spanned over
//!    both families this service publishes such a build line for.
//! 4. The CA authorities an entry publishes are its ENGINE's: the still-valid authorities
//!    of the one certificate catalogue, the elliptic-curve one only where the engine is
//!    offered it, ordered as the service publishes them. Spanned over the Aurora engines,
//!    the DB instance engines and the engines of the sibling control planes.
//! 5. Each `UpgradeTarget` reports the TARGET version's own members, cross-checked against
//!    that target's own entry in the same report.

use std::collections::BTreeSet;
use std::time::SystemTime;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    invalid_marker_message, CertificateKey, CA_CERTIFICATES, MAX_RECORDS_RANGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::{versions, walk_report, Version};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
};

/// The operation whose page this suite is about.
const ACTION: &str = "DescribeDBEngineVersions";

/// The engines rule 2 is spanned over: the engine the paged sweep this suite was written
/// for asks about, the other family with an Extended Support build line, a cluster engine,
/// and two engines whose versions are published in another form (the Oracle release update
/// and the Microsoft SQL Server build).
const ENGINES: [&str; 5] = [
    "mysql",
    "postgres",
    "aurora-mysql",
    "oracle-ee",
    "sqlserver-ee",
];

/// The (engine, major-version family, community release) rows rule 3 is spanned over.
///
/// Source: "Using Amazon RDS Extended Support" (Amazon RDS User Guide) names RDS for
/// MySQL 5.7 and RDS for PostgreSQL 11 as the families RDS keeps releasing builds of after
/// the community stopped, each build named `<version>-rds.YYYYMMDD`.
const EXTENDED_SUPPORT: [(&str, &str, &str); 2] =
    [("mysql", "5.7", "5.7.44"), ("postgres", "11", "11.22")];

/// How many DB parameter groups the stored-record report of rule 1 is given, so that its
/// first page at the smallest `MaxRecords` leaves records unserved.
const STORED_GROUPS: usize = 25;

/// One row of the paginated-report table: the action, the parameters that ask for the
/// whole report, and the element an entry's key is read from.
type ReportRow = (&'static str, &'static [(&'static str, &'static str)], &'static str);

/// The smallest page a caller may ask for: "Constraints: Minimum 20, maximum 100".
fn smallest_page() -> usize {
    *MAX_RECORDS_RANGE.start() as usize
}

/// The largest page this operation's `MaxRecords` admits.
fn largest_page() -> usize {
    *MAX_RECORDS_RANGE.end() as usize
}

/// One call of `action`, paged at `page_size` and resumed at `marker`: the keys its entries
/// carried (read from `key_element`) and the `Marker` it answered with.
async fn page_of(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    key_element: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut request: Vec<(&str, String)> = vec![("MaxRecords", page_size.to_string())];
    for (name, value) in params {
        request.push((name, (*value).to_string()));
    }
    if let Some(marker) = marker {
        request.push(("Marker", marker.to_string()));
    }
    let body = expect_ok(case, call(router, action, &request).await);
    // An engine-version entry carries its upgrade targets, and each target carries its own
    // `EngineVersion`; stripping the lists first is what makes "the key of this entry"
    // well defined. Harmless for a report that carries no such list.
    let mut flat = body.clone();
    for subtree in subtrees(&body, "ValidUpgradeTarget") {
        flat = flat.replace(&subtree, "");
    }
    (texts(&flat, key_element), optional_text(&flat, "Marker"))
}

/// The whole report `action` serves, walked one page of `page_size` at a time.
async fn walk_keys(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    key_element: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..100 {
        let (keys, next) = page_of(
            router,
            case,
            action,
            params,
            key_element,
            page_size,
            marker.as_deref(),
        )
        .await;
        assert!(
            keys.len() <= page_size,
            "case {case:?}: a page holds at most MaxRecords={page_size} entries, got {}",
            keys.len()
        );
        seen.extend(keys);
        match next {
            Some(next) => marker = Some(next),
            None => return seen,
        }
    }
    panic!("case {case:?}: walking the report did not terminate")
}

/// One page of `engine`'s engine-version report, deprecated entries included.
async fn engine_page(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    page_size: usize,
    marker: Option<&str>,
) -> (Vec<Version>, Option<String>) {
    let mut request: Vec<(&str, String)> = vec![
        ("Engine", engine.to_string()),
        ("IncludeAll", "true".to_string()),
        ("MaxRecords", page_size.to_string()),
    ];
    if let Some(marker) = marker {
        request.push(("Marker", marker.to_string()));
    }
    let body = expect_ok(case, call(router, ACTION, &request).await);
    (versions(&body), optional_text(&body, "Marker"))
}

/// The `(engine, version)` identity of each reported entry, as two pages are compared by.
fn keys(entries: &[Version]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("{}/{}", entry.engine, entry.engine_version))
        .collect()
}

/// Every entry of one engine's report, deprecated ones included, walked across its pages.
async fn whole_report(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    walk_report(
        router,
        engine,
        largest_page(),
        &[
            ("Engine", engine.to_string()),
            ("IncludeAll", "true".to_string()),
        ],
    )
    .await
}

/// Whether a published certificate validity has passed.
fn has_expired(valid_till: &str) -> bool {
    let till = ::aws_smithy_types::DateTime::from_str(
        valid_till,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|_| panic!("the catalogue's {valid_till:?} is an ISO-8601 instant"));
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    till.secs() <= now
}

#[tokio::test]
async fn a_marker_is_an_opaque_token_the_service_issued_and_accepts_back() {
    let router = emulator();

    // The stored-record report of the table below needs records to page over; the
    // published catalogue needs none.
    for index in 0..STORED_GROUPS {
        let name = format!("emu-token-group-{index:03}");
        expect_ok(
            &name,
            call(
                &router,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", name.clone()),
                    ("DBParameterGroupFamily", "mysql8.0".to_string()),
                    ("Description", "paging tokens".to_string()),
                ],
            )
            .await,
        );
    }

    // (action, the parameters that ask for the whole report, the element an entry's key is
    // read from): one report of published reference data and one of stored records, so the
    // rule is not read as a property of either source.
    let reports: [ReportRow; 2] = [
        (ACTION, &[("IncludeAll", "true")], "EngineVersion"),
        ("DescribeDBParameterGroups", &[], "DBParameterGroupName"),
    ];

    for (action, params, key_element) in reports {
        let case = format!("{action} paged at {}", smallest_page());
        let (first, marker) = page_of(
            &router,
            &case,
            action,
            params,
            key_element,
            smallest_page(),
            None,
        )
        .await;
        assert_eq!(
            first.len(),
            smallest_page(),
            "case {case:?}: a report longer than one page serves a full page, got {}",
            first.len()
        );
        let marker = marker.unwrap_or_else(|| {
            panic!("case {case:?}: a cut that leaves entries unserved issues a Marker")
        });

        let (resumed, _) = page_of(
            &router,
            &format!("{case}, resumed"),
            action,
            params,
            key_element,
            smallest_page(),
            Some(&marker),
        )
        .await;
        let resumes_at = resumed
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the resumed page must carry entries"))
            .clone();

        // Opaque on the wire: the token is not the plaintext key of the record it resumes
        // at, it is drawn from the encoded alphabet, and that plaintext key is refused
        // exactly like any other token no page of this report issued.
        assert_ne!(
            marker, resumes_at,
            "case {case:?}: the Marker must be a token rather than the plaintext key of \
             the entry it resumes at, got {marker:?}"
        );
        assert!(
            marker
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)),
            "case {case:?}: the token must be the encoded form the wire carries, got \
             {marker:?}"
        );
        for unissued in [resumes_at.as_str(), "not-a-token-this-service-issued"] {
            let refused = format!("{case}, resumed from {unissued:?}");
            let mut request: Vec<(&str, String)> = vec![("Marker", unissued.to_string())];
            for (name, value) in params {
                request.push((name, (*value).to_string()));
            }
            let body = expect_error(
                &refused,
                call(&router, action, &request).await,
                400,
                "InvalidParameterValue",
            );
            assert_eq!(
                text(&body, "Message"),
                invalid_marker_message(),
                "case {refused:?}: a token no page issued is refused in the service's own \
                 wording"
            );
        }

        // A cursor, not a one-shot token: the same token answers the same page.
        let (again, _) = page_of(
            &router,
            &format!("{case}, resumed twice"),
            action,
            params,
            key_element,
            smallest_page(),
            Some(&marker),
        )
        .await;
        assert_eq!(
            again, resumed,
            "case {case:?}: re-presenting a Marker must answer the same page"
        );

        // Whatever the page size, walking the tokens visits the report's own order
        // exactly once and the page that serves the tail issues no token.
        let walked_small = walk_keys(
            &router,
            &format!("{case}, walked at {}", smallest_page()),
            action,
            params,
            key_element,
            smallest_page(),
        )
        .await;
        let walked_large = walk_keys(
            &router,
            &format!("{case}, walked at {}", largest_page()),
            action,
            params,
            key_element,
            largest_page(),
        )
        .await;
        assert_eq!(
            walked_small, walked_large,
            "case {case:?}: the page size must not change the report the walk visits"
        );
        assert_eq!(
            walked_small[..first.len()],
            first[..],
            "case {case:?}: the walk must begin with the page the first call served"
        );
        assert!(
            walked_small.len() > smallest_page(),
            "case {case:?}: the walk must cross a page boundary, got {} entries",
            walked_small.len()
        );
    }
}

#[tokio::test]
async fn a_max_records_page_is_the_reports_own_page_and_the_marker_resumes_it() {
    let router = emulator();

    // Each published `MaxRecords` bound and a size between them that divides neither the
    // catalogue nor an engine's run of entries.
    for engine in ENGINES {
        let whole = whole_report(&router, engine).await;
        assert!(
            !whole.is_empty(),
            "case {engine:?}: this service publishes versions for this engine, got none"
        );
        for page_size in [smallest_page(), 37, largest_page()] {
            let case = format!("{engine} paged at {page_size}");
            let (first, marker) = engine_page(&router, &case, engine, page_size, None).await;
            let served = page_size.min(whole.len());
            assert_eq!(
                keys(&first),
                keys(&whole[..served]),
                "case {case:?}: the first page must be the report's own first {served} \
                 entries"
            );
            match marker {
                None => assert!(
                    whole.len() <= page_size,
                    "case {case:?}: {} entries remain unserved, so a Marker is owed",
                    whole.len().saturating_sub(served)
                ),
                Some(marker) => {
                    let (second, _) =
                        engine_page(&router, &case, engine, page_size, Some(&marker)).await;
                    let end = (served + page_size).min(whole.len());
                    assert_eq!(
                        keys(&second),
                        keys(&whole[served..end]),
                        "case {case:?}: the token must resume the report at entry {served}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn an_extended_support_family_publishes_the_builds_it_still_offers() {
    let router = emulator();

    for (engine, family, community_release) in EXTENDED_SUPPORT {
        let case = format!("{engine} {family}");
        // The default report: no `IncludeAll`, so it carries the versions a caller can
        // still use ("the default is to list only available engine versions").
        let offered = walk_report(
            &router,
            &case,
            largest_page(),
            &[("Engine", engine.to_string())],
        )
        .await;
        let offered_in_family: Vec<&Version> = offered
            .iter()
            .filter(|entry| entry.major_engine_version == family)
            .collect();
        assert!(
            !offered_in_family.is_empty(),
            "case {case:?}: a family under Extended Support is still offered, so the \
             default report must carry builds of it, got {:?}",
            offered
                .iter()
                .map(|entry| entry.engine_version.clone())
                .collect::<Vec<_>>()
        );
        for build in &offered_in_family {
            assert_eq!(
                build.status, "available",
                "case {case:?}: a build the default report serves is available, got {:?} \
                 for {:?}",
                build.status, build.engine_version
            );
            // Every offered build of such a family is one of its DATED builds: the
            // community release the line continues is not offered any more.
            let dated = build
                .engine_version
                .strip_prefix(&format!("{community_release}-rds."))
                .unwrap_or_default();
            assert!(
                dated.len() == 8 && dated.bytes().all(|byte| byte.is_ascii_digit()),
                "case {case:?}: an offered build of this family is published as \
                 <version>-rds.YYYYMMDD, got {:?}",
                build.engine_version
            );
        }

        // The whole family, the builds it has moved past included: the community release
        // is published and deprecated, and the offered builds are the LATER ones.
        let every_build = whole_report(&router, engine).await;
        let community = every_build
            .iter()
            .find(|entry| entry.engine_version == community_release)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the community release the line continues is published")
            });
        assert_eq!(
            community.status, "deprecated",
            "case {case:?}: the community release past its end of life is deprecated, got \
             {:?}",
            community.status
        );
        let family_order: Vec<String> = every_build
            .iter()
            .filter(|entry| entry.major_engine_version == family)
            .map(|entry| entry.engine_version.clone())
            .collect();
        let first_offered = offered_in_family[0].engine_version.clone();
        let offered_at = family_order
            .iter()
            .position(|version| *version == first_offered)
            .unwrap_or_default();
        assert!(
            offered_at > 0,
            "case {case:?}: the offered builds follow the ones the family has moved past, \
             got {family_order:?}"
        );
        // …and the first row the default report serves for this engine is that family's
        // first offered build: the report is grouped by family, earliest family first.
        assert_eq!(
            offered.first().map(|entry| entry.engine_version.clone()),
            Some(first_offered),
            "case {case:?}: the first row of the engine's report is the earliest build it \
             still offers"
        );
    }

    // The page the rule above is read through: an engine whose offered builds outnumber
    // the smallest page a caller may ask for is served a truncated page and a token, so a
    // client enumerating that engine's versions has to walk it.
    let offered = walk_report(
        &router,
        "mysql offered builds",
        largest_page(),
        &[("Engine", "mysql".to_string())],
    )
    .await;
    assert!(
        offered.len() > smallest_page(),
        "case \"mysql offered builds\": this engine's offered builds must outnumber one \
         page of {}, got {}",
        smallest_page(),
        offered.len()
    );
    let body = expect_ok(
        "the mysql page",
        call(
            &router,
            ACTION,
            &[
                ("Engine", "mysql".to_string()),
                ("MaxRecords", smallest_page().to_string()),
            ],
        )
        .await,
    );
    let page = versions(&body);
    assert_eq!(
        keys(&page),
        keys(&offered[..smallest_page()]),
        "case \"the mysql page\": the page is the report's own first {} offered builds",
        smallest_page()
    );
    assert!(
        optional_text(&body, "Marker").is_some(),
        "case \"the mysql page\": builds remain unserved, so the page carries a token: \
         {body}"
    );
}

#[tokio::test]
async fn the_authorities_an_entry_publishes_are_its_engines() {
    let router = emulator();

    // The rule's source is the ONE certificate catalogue `DescribeCertificates` reports:
    // an entry publishes the authorities in it that have not expired AND that its engine
    // is offered — the elliptic-curve authority for an Aurora engine, the RSA ones for
    // every engine — ordered as the service publishes them: the elliptic-curve authority
    // first, then the RSA ones from the longest key to the shortest.
    let published = expect_ok(
        "the certificate catalogue",
        call(
            &router,
            "DescribeCertificates",
            &[("MaxRecords", smallest_page().to_string())],
        )
        .await,
    );
    let catalogue: Vec<String> = texts(&published, "CertificateIdentifier");
    let mut ranked: Vec<(u8, u32, String)> = CA_CERTIFICATES
        .iter()
        .filter(|certificate| !has_expired(certificate.valid_till))
        .map(|certificate| match certificate.key {
            CertificateKey::EllipticCurve(bits) => (0, bits, certificate.identifier.to_string()),
            CertificateKey::Rsa(bits) => (1, bits, certificate.identifier.to_string()),
        })
        .collect();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
    assert!(
        ranked.iter().any(|(kind, _, _)| *kind == 0)
            && ranked.iter().filter(|(kind, _, _)| *kind == 1).count() > 1,
        "the catalogue must publish an elliptic-curve authority and more than one RSA \
         authority, or this rule tests nothing: got {ranked:?}"
    );
    for (_, _, identifier) in &ranked {
        assert!(
            catalogue.contains(identifier),
            "the authorities an entry can publish are the catalogue's own, and \
             {identifier:?} is not in {catalogue:?}"
        );
    }

    // Every engine this front door publishes versions for: the Aurora engines, the DB
    // instance engines, and the engines it reports for another control plane.
    let engines = [
        "aurora-mysql",
        "aurora-postgresql",
        "mysql",
        "mariadb",
        "postgres",
        "oracle-ee",
        "sqlserver-ee",
        "db2-ae",
        "docdb",
        "neptune",
    ];
    for engine in engines {
        let aurora = engine.starts_with("aurora");
        let expected: Vec<String> = ranked
            .iter()
            .filter(|(kind, _, _)| *kind == 1 || aurora)
            .map(|(_, _, identifier)| identifier.clone())
            .collect();
        for entry in whole_report(&router, engine).await {
            let case = format!("{engine} {}", entry.engine_version);
            let wrapped = subtrees(&entry.scalars, "SupportedCACertificateIdentifiers");
            assert_eq!(
                wrapped.len(),
                1,
                "case {case:?}: one entry reports one authority list, got {}",
                entry.scalars
            );
            let listed = texts(&wrapped[0], "member");
            assert_eq!(
                listed, expected,
                "case {case:?}: an entry publishes the authorities its engine is offered, \
                 got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_upgrade_target_reports_the_target_versions_own_members() {
    let router = emulator();

    for engine in ENGINES {
        let report = whole_report(&router, engine).await;
        let published: BTreeSet<String> = report
            .iter()
            .map(|entry| entry.engine_version.clone())
            .collect();
        let mut targets_seen = 0usize;
        for entry in &report {
            let family = entry.major_engine_version.clone();
            for target in subtrees(&entry.xml, "UpgradeTarget") {
                targets_seen += 1;
                let version = text(&target, "EngineVersion");
                let case = format!("{engine} {} -> {version}", entry.engine_version);
                assert_eq!(
                    text(&target, "Engine"),
                    entry.engine,
                    "case {case:?}: an upgrade target is a version of the same engine, got \
                     {target}"
                );
                assert!(
                    published.contains(&version),
                    "case {case:?}: an upgrade target must be a version this report \
                     publishes, got {version:?}"
                );
                // The target's own entry decides every member the target reports about it.
                let own = report
                    .iter()
                    .find(|candidate| candidate.engine_version == version)
                    .unwrap_or_else(|| panic!("case {case:?}: the target's own entry"));
                for (member, expected) in [
                    ("MajorEngineVersion", own.major_engine_version.clone()),
                    (
                        "MinorEngineVersion",
                        text(&own.scalars, "MinorEngineVersion"),
                    ),
                    (
                        "Description",
                        text(&own.scalars, "DBEngineVersionDescription"),
                    ),
                ] {
                    assert_eq!(
                        text(&target, member),
                        expected,
                        "case {case:?}: {member} must be the target version's own, got \
                         {target}"
                    );
                }
                let major_upgrade = own.major_engine_version != family;
                assert_eq!(
                    text(&target, "IsMajorVersionUpgrade"),
                    major_upgrade.to_string(),
                    "case {case:?}: a target in another major-version family is a major \
                     version upgrade, got {target}"
                );
                // A snapshot is upgraded straight into the target only where that upgrade
                // moves it between major versions; restoring into a later build of its own
                // family needs no snapshot upgrade.
                assert_eq!(
                    text(&target, "SupportsSnapshotUpgrade"),
                    major_upgrade.to_string(),
                    "case {case:?}: SupportsSnapshotUpgrade must answer for the target \
                     kind, got {target}"
                );
                if text(&target, "AutoUpgrade") == "true" {
                    assert!(
                        !major_upgrade,
                        "case {case:?}: an automatic upgrade never crosses a major \
                         version, got {target}"
                    );
                }
            }
        }
        assert!(
            targets_seen > 0,
            "case {engine:?}: this engine's report must carry upgrade targets to check"
        );
    }
}
