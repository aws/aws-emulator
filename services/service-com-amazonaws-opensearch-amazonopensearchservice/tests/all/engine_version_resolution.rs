// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the catalog operations resolve the engine version they
//! are asked about: `DescribeInstanceTypeLimits` and `ListInstanceTypeDetails`,
//! against the set `ListVersions` advertises.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test: an operation that reports what the service offers FOR a named
//! engine release resolves that release against the catalog of releases the service
//! publishes — the same set `ListVersions` advertises — and refuses a value the
//! catalog does not hold with HTTP 400 `ValidationException`, naming the value asked
//! for and the catalog it was resolved against. `VersionString`'s own pattern
//! (`^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$`, from
//! `smithy_query(query="operation",operation="DescribeInstanceTypeLimits")`) admits
//! every `Elasticsearch_X.Y` and `OpenSearch_X.Y` value, so satisfying it is not
//! resolving: the two must not disagree about which engines exist.
//!
//! So the assertions span SETS of inputs: every version the emulator itself
//! advertises is answered by both operations, and a table of well-formed versions
//! outside the catalog — a far-future major, a minor its line skips, a minor of each
//! engine line that was never published, a two-digit major — is refused by both, each
//! refusal carrying exactly the advertised catalog.

use std::collections::BTreeSet;

use serde_json::Value;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// The instance type every catalog call below asks about: a previous-generation type the
/// catalog offers on every release it publishes, so the only thing a row can turn on is
/// the engine version.
const INSTANCE_TYPE: &str = "m4.large.search";

/// `ListVersions`: `GET /2021-01-01/opensearch/versions`, read to the end of its
/// pagination, so what it returns is the WHOLE advertised catalog however it is paged.
async fn advertised_versions(emulator: &Emulator) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let uri = match token.as_deref() {
            None => format!("{PREFIX}/opensearch/versions"),
            Some(next) => format!("{PREFIX}/opensearch/versions?nextToken={next}"),
        };
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case ListVersions(token={token:?}): expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let page = answer.body["Versions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case ListVersions(token={token:?}): expected a Versions list, got {:?}",
                    answer.body
                )
            })
            .iter()
            .filter_map(|version| version.as_str().map(str::to_owned))
            .collect::<Vec<String>>();
        versions.extend(page);
        token = answer.body["NextToken"].as_str().map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    assert!(
        !versions.is_empty(),
        "case ListVersions: the service must advertise at least one engine version"
    );
    versions
}

/// `DescribeInstanceTypeLimits`:
/// `GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}`.
async fn describe_limits(emulator: &Emulator, engine_version: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/instanceTypeLimits/{engine_version}/{INSTANCE_TYPE}"),
            None,
        )
        .await
}

/// `ListInstanceTypeDetails`:
/// `GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}`.
async fn list_details(emulator: &Emulator, engine_version: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}"),
            None,
        )
        .await
}

/// Every operation the rule covers, named for the case it is reported under, with the
/// output member a successful answer carries.
const OPERATIONS: [(&str, &str); 2] = [
    ("DescribeInstanceTypeLimits", "LimitsByRole"),
    ("ListInstanceTypeDetails", "InstanceTypeDetails"),
];

/// One of [`OPERATIONS`] asked about `engine_version`.
async fn call(emulator: &Emulator, operation: &str, engine_version: &str) -> Answer {
    match operation {
        "DescribeInstanceTypeLimits" => describe_limits(emulator, engine_version).await,
        "ListInstanceTypeDetails" => list_details(emulator, engine_version).await,
        other => panic!("case {other}: the suite drives only the catalog operations"),
    }
}

/// The error message of a refused answer, with the case named on failure.
fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned()
}

/// The engine versions a refusal lists as allowed: the contents of the one bracketed
/// list its message carries, split on the separator between entries.
fn allowed_versions(message: &str, case: &str) -> Vec<String> {
    let opened = message.find('[').unwrap_or_else(|| {
        panic!("case {case}: expected the allowed versions in brackets, got {message:?}")
    });
    let closed = message.rfind(']').unwrap_or_else(|| {
        panic!("case {case}: expected the allowed versions in brackets, got {message:?}")
    });
    assert!(
        opened < closed,
        "case {case}: expected a bracketed list of allowed versions, got {message:?}"
    );
    message[opened + 1..closed]
        .split(',')
        .map(|entry| entry.trim().to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[tokio::test]
async fn every_advertised_version_is_answered_by_every_catalog_operation() {
    let emulator = Emulator::new();

    // The positive side of the rule, over the WHOLE advertised catalog rather than one
    // version: what `ListVersions` offers is what the catalog operations serve.
    for version in advertised_versions(&emulator).await {
        for (operation, member) in OPERATIONS {
            let case = format!("{operation}({version})");
            let answer = call(&emulator, operation, &version).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: an advertised version must be answered, got {} {:?}",
                answer.status, answer.body
            );
            assert!(
                !answer.body[member].is_null(),
                "case {case}: expected a {member} in the answer, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_version_outside_the_advertised_catalog_is_refused_with_that_catalog() {
    let emulator = Emulator::new();
    let advertised: BTreeSet<String> = advertised_versions(&emulator).await.into_iter().collect();

    // Rows differ along HOW a well-formed version can fail to name a release the
    // service has: a far-future major of each engine line, a minor its line skips, a
    // minor never published, and a two-digit major the pattern allows. Each satisfies
    // `VersionString`'s length (14..18) and pattern, so the pattern alone refuses none
    // of them.
    let outside = [
        "OpenSearch_99.99",
        "OpenSearch_10.1",
        "OpenSearch_1.9",
        "OpenSearch_2.4",
        "Elasticsearch_9.9",
        "Elasticsearch_6.6",
    ];

    for version in outside {
        assert!(
            !advertised.contains(version),
            "case {version}: the row must name a version the service does not advertise, \
             but the catalog holds it"
        );
        for (operation, _) in OPERATIONS {
            let case = format!("{operation}({version})");
            let answer = call(&emulator, operation, version).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled validation refusal, got {:?}",
                answer.body
            );

            // The refusal names the version asked for, verbatim, and the catalog it was
            // resolved against — which is exactly what `ListVersions` advertises, so a
            // client told "unsupported" can see what IS supported.
            let stated = message(&answer, &case);
            assert!(
                stated.starts_with(&format!("Unsupported Version: {version}, ")),
                "case {case}: expected the refusal to name the version asked for, got \
                 {stated:?}"
            );
            let listed: BTreeSet<String> = allowed_versions(&stated, &case).into_iter().collect();
            assert_eq!(
                listed, advertised,
                "case {case}: expected the refusal to list the advertised catalog, got \
                 {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_version_that_names_no_release_at_all_is_told_the_format_not_the_catalog() {
    let emulator = Emulator::new();

    // A value that is not a version at all fails for a DIFFERENT reason than one that
    // is a version the service does not have, so the two refusals say different things:
    // this one is about the form of the member.
    for malformed in [
        "Opensearch_2.11",
        "OpenSearch_abc",
        "Elasticsearch_x.y",
        "AmazonSearch_2.11",
    ] {
        for (operation, _) in OPERATIONS {
            let case = format!("{operation}({malformed})");
            let answer = call(&emulator, operation, malformed).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected the modelled validation refusal, got {:?}",
                answer.body
            );
            let stated = message(&answer, &case);
            assert!(
                !stated.starts_with("Unsupported Version:"),
                "case {case}: a value that is no version at all is not an unsupported \
                 release, got {stated:?}"
            );
            assert!(
                stated.to_ascii_lowercase().contains("engineversion"),
                "case {case}: expected the refusal to name the member, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn resolving_the_version_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let advertised = advertised_versions(&emulator).await;
    let sampled = advertised
        .first()
        .cloned()
        .expect("the catalog advertises at least one version");

    // A refused call writes nothing: the catalog a later caller reads is the catalog
    // that was there before, and the answer for an advertised version is unchanged.
    let before: Value = describe_limits(&emulator, &sampled).await.body;
    for version in ["OpenSearch_99.99", "Elasticsearch_9.9"] {
        let refused = describe_limits(&emulator, version).await;
        assert_eq!(
            refused.status, 400,
            "case DescribeInstanceTypeLimits({version}): expected HTTP 400, got {} {:?}",
            refused.status, refused.body
        );
        let refused = list_details(&emulator, version).await;
        assert_eq!(
            refused.status, 400,
            "case ListInstanceTypeDetails({version}): expected HTTP 400, got {} {:?}",
            refused.status, refused.body
        );
    }
    let after: Value = describe_limits(&emulator, &sampled).await.body;
    assert_eq!(
        after, before,
        "case DescribeInstanceTypeLimits({sampled}): a refused call must leave the catalog \
         exactly as it was: {before:?} then {after:?}"
    );
    assert_eq!(
        advertised_versions(&emulator).await,
        advertised,
        "case ListVersions: a refused catalog call must not change what is advertised"
    );
}
