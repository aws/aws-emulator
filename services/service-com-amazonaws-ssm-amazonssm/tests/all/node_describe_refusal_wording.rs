// SPDX-License-Identifier: CC0-1.0

//! How the managed-node describe words the three malformed request classes it refuses:
//! `DescribeInstanceInformation`'s page-size range, its two filter dialects named together,
//! and a filter value outside the set its key publishes.
//!
//! An HTTP status, an error code and an error MESSAGE are each part of the answer, so each
//! rule below is pinned over a TABLE of inputs rather than over the one request a recorded
//! call happened to send:
//!
//! 1. A `MaxResults` outside the modelled range (`smithy_query(query="shape",
//!    shape="MaxResultsEC2Compatible")`: `range 5..50`) is refused with the member path and
//!    the bound the model declares, and with nothing of the request in it — this operation
//!    states no value in a boundary clause. The bound is a property of the SHAPE, so the
//!    same clause answers a page size one past the bound and one far past it, and the
//!    published bounds themselves are served. A request that is wrong both ways — a filter
//!    key the operation does not document BESIDE a page size outside the range — is refused
//!    with that same single clause, because the modelled constraints of the input are
//!    checked before any filter of it is read.
//! 2. Naming BOTH filter dialects (the legacy `InstanceInformationFilterList` beside
//!    `Filters`) is refused with one fixed sentence that names neither member and carries
//!    no part of either list.
//! 3. A filter value outside the set its key publishes is the operation's own modelled
//!    `InvalidInstanceInformationFilterValue`, naming the KEY and the values that key
//!    accepts in the order the API publishes them — the same sentence in both dialects, with
//!    no value echoed, and the comparison is exact, so another case or another suffix of a
//!    published value is refused too.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.

use serde_json::{json, Map, Value};

use service_com_amazonaws_ssm_amazonssm::state::node_page_size;

use crate::parameter_store::{call, error_code, error_message, router};

/// The one operation every case here calls.
const OPERATION: &str = "DescribeInstanceInformation";

/// The member a successful report lists its nodes in.
const REPORT_MEMBER: &str = "InstanceInformationList";

/// The modelled bounds of `DescribeInstanceInformation$MaxResults`
/// (`smithy_query(query="shape", shape="MaxResultsEC2Compatible")`: `range 5..50`), written
/// out here so the test pins them rather than echoing whatever the service holds.
const MIN_PAGE_SIZE: i64 = 5;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 50;

/// The envelope a refusal of ONE broken constraint carries.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The whole message a page size outside the modelled range is refused with: the member
/// path and the bound the model declares, and no value from the request.
fn page_size_refusal(bound: i64) -> String {
    let requirement = if bound == MIN_PAGE_SIZE {
        format!("Member must have value greater than or equal to {MIN_PAGE_SIZE}")
    } else {
        format!("Member must have value less than or equal to {MAX_PAGE_SIZE}")
    };
    one_violation(&format!(
        "Value at 'maxResults' failed to satisfy constraint: {requirement}"
    ))
}

/// One filter of a request: the key and the values it carries.
type Filter<'a> = (&'a str, &'a [&'a str]);

/// A request carrying `filters` in the current `Filters` member.
fn filters_body(filters: &[Filter<'_>]) -> Value {
    json!({
        "Filters": filters
            .iter()
            .map(|(key, values)| json!({ "Key": key, "Values": values }))
            .collect::<Vec<Value>>(),
    })
}

/// A request carrying `filters` in the legacy `InstanceInformationFilterList` member,
/// whose entries are the lowercase `key` / `valueSet` pair the model declares
/// (`smithy_query(query="shape", shape="InstanceInformationFilter")`).
fn legacy_body(filters: &[Filter<'_>]) -> Value {
    json!({
        "InstanceInformationFilterList": filters
            .iter()
            .map(|(key, values)| json!({ "key": key, "valueSet": values }))
            .collect::<Vec<Value>>(),
    })
}

/// RULE 1: a page size outside the modelled range is refused with the member and the bound,
/// and with no part of the request.
///
/// The rows differ along the direction the bound was broken, how far past it the value sat,
/// and its sign — the clause is the same for all of them, because it reports the CONSTRAINT.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_states_the_bound_and_no_value() {
    let router = router();

    // (case, the MaxResults the request carries, the bound it broke)
    let cases: &[(&str, i64, i64)] = &[
        ("one below the minimum", MIN_PAGE_SIZE - 1, MIN_PAGE_SIZE),
        ("one above the maximum", MAX_PAGE_SIZE + 1, MAX_PAGE_SIZE),
        ("zero", 0, MIN_PAGE_SIZE),
        ("negative", -1, MIN_PAGE_SIZE),
        ("far below the minimum", -4_096, MIN_PAGE_SIZE),
        ("twice the maximum", MAX_PAGE_SIZE * 2, MAX_PAGE_SIZE),
        ("far above the maximum", 100_000, MAX_PAGE_SIZE),
        (
            "the largest number the member can hold",
            i64::from(i32::MAX),
            MAX_PAGE_SIZE,
        ),
    ];

    for (case, max_results, bound) in cases {
        let body = json!({ "MaxResults": max_results });
        let (status, response) = call(&router, OPERATION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({max_results}): a page size outside the range is refused, got \
             {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?} ({max_results}): the refusal is a ValidationException, got {response}"
        );
        let expected = page_size_refusal(*bound);
        let stated = error_message(&response);
        assert_eq!(
            stated, expected,
            "case {case:?} ({max_results}): expected {expected:?}, got {stated:?}"
        );
        let quoted = format!("Value '{max_results}'");
        assert!(
            !stated.contains(&quoted),
            "case {case:?} ({max_results}): the clause quotes no value, got {stated:?}"
        );
    }
}

/// The published bounds themselves, and every page size between them, are SERVED: the
/// clause above is about the range and not about naming the member.
#[tokio::test]
async fn every_page_size_the_range_admits_is_served() {
    let router = router();

    // (case, the body the request carries)
    let cases: &[(&str, Value)] = &[
        ("the published minimum", json!({ "MaxResults": MIN_PAGE_SIZE })),
        ("the published maximum", json!({ "MaxResults": MAX_PAGE_SIZE })),
        ("one inside the minimum", json!({ "MaxResults": MIN_PAGE_SIZE + 1 })),
        ("one inside the maximum", json!({ "MaxResults": MAX_PAGE_SIZE - 1 })),
        ("the documented default", json!({ "MaxResults": 10 })),
        ("no page size at all", json!({})),
    ];

    for (case, body) in cases {
        let (status, response) = call(&router, OPERATION, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page size the range admits is served, got {status} {response}"
        );
        assert!(
            response[REPORT_MEMBER].is_array(),
            "case {case:?}: expected a report in {REPORT_MEMBER}, got {response}"
        );
    }
}

/// RULE 1, continued: a request that is wrong BOTH ways — an undocumented filter key beside
/// a page size outside the range — is refused with the page-size clause alone, so the input's
/// modelled constraints are reported before any filter of it is read.
///
/// The rows differ along the key the request made up, which dialect carried it, how many
/// filters it carried, and which end of the range the page size broke.
#[tokio::test]
async fn the_page_size_clause_precedes_the_filter_key_refusal() {
    let router = router();

    // (case, the filters member and its filters, the MaxResults, the bound it broke)
    let cases: &[(&str, Value, i64, i64)] = &[
        (
            "a key that names nothing, above the maximum",
            filters_body(&[("PlatformName", &["Windows"])]),
            MAX_PAGE_SIZE + 1,
            MAX_PAGE_SIZE,
        ),
        (
            "a documented key in another case, above the maximum",
            filters_body(&[("platformtypes", &["Windows"])]),
            MAX_PAGE_SIZE * 3,
            MAX_PAGE_SIZE,
        ),
        (
            "a tag prefix naming no tag, below the minimum",
            filters_body(&[("tag:", &["Test"])]),
            MIN_PAGE_SIZE - 1,
            MIN_PAGE_SIZE,
        ),
        (
            "a documented key beside an undocumented one",
            filters_body(&[("PlatformTypes", &["Windows"]), ("NotAKey", &["x"])]),
            0,
            MIN_PAGE_SIZE,
        ),
        (
            "an undocumented key the legacy dialect carried",
            legacy_body(&[("PlatformTypes", &["Windows"])]),
            MAX_PAGE_SIZE + 7,
            MAX_PAGE_SIZE,
        ),
    ];

    for (case, filters, max_results, bound) in cases {
        let mut body = filters
            .as_object()
            .expect("a filter body is an object")
            .clone();
        body.insert("MaxResults".to_string(), json!(max_results));
        let body = Value::Object(body);
        let (status, response) = call(&router, OPERATION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request is refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: the refusal is a ValidationException, got {response}"
        );
        let expected = page_size_refusal(*bound);
        let stated = error_message(&response);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the page-size clause {expected:?}, got {stated:?}"
        );
    }
}

/// RULE 2: naming both filter dialects is refused with one fixed sentence — it names
/// neither member and carries no part of either list, so it is the same answer whatever the
/// two lists held.
///
/// The rows differ along the keys each dialect names, how many filters each holds, and how
/// many values a filter carries.
#[tokio::test]
async fn naming_both_dialects_states_one_fixed_sentence() {
    let router = router();

    /// The sentence the refusal carries, written out here independently of the service.
    const BOTH_DIALECTS: &str = "Cannot use both legacy and new filter simultaneously.";

    // (case, the filters the current member carries, the filters the legacy member carries)
    let cases: &[(&str, &[Filter<'_>], &[Filter<'_>])] = &[
        (
            "one filter in each, on different keys",
            &[("PingStatus", &["Online"])],
            &[("ResourceType", &["EC2Instance"])],
        ),
        (
            "the same key in both dialects",
            &[("PlatformTypes", &["Linux"])],
            &[("PlatformTypes", &["Linux"])],
        ),
        (
            "two filters beside one",
            &[("PingStatus", &["Online"]), ("ResourceType", &["EC2Instance"])],
            &[("AgentVersion", &["3.0.1124.0"])],
        ),
        (
            "a filter carrying several values",
            &[("PlatformTypes", &["Windows", "Linux", "MacOS"])],
            &[("PingStatus", &["Online", "Inactive"])],
        ),
        (
            "a tag filter the legacy dialect cannot express",
            &[("tag:Purpose", &["Test"])],
            &[("InstanceIds", &["i-0123456789abcdef0"])],
        ),
    ];

    for (case, current, legacy) in cases {
        let mut body = Map::new();
        body.insert(
            "Filters".to_string(),
            filters_body(current)["Filters"].clone(),
        );
        body.insert(
            "InstanceInformationFilterList".to_string(),
            legacy_body(legacy)["InstanceInformationFilterList"].clone(),
        );
        let body = Value::Object(body);
        let (status, response) = call(&router, OPERATION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request is refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: the refusal is a ValidationException, got {response}"
        );
        let stated = error_message(&response);
        assert_eq!(
            stated, BOTH_DIALECTS,
            "case {case:?}: expected {BOTH_DIALECTS:?}, got {stated:?}"
        );
        for named in ["Filters", "InstanceInformationFilterList"] {
            assert!(
                !stated.contains(named),
                "case {case:?}: the sentence names no member, got {stated:?}"
            );
        }
    }
}

/// RULE 3: a filter value outside the set its key publishes is the modelled
/// `InvalidInstanceInformationFilterValue`, naming the key and the values it accepts in the
/// order the API publishes them — the same sentence in both dialects.
///
/// The published sets are the `InstanceInformationStringFilter$Key` doc's lists
/// (`smithy_query(query="doc", shape="InstanceInformationStringFilter", member="Key")`),
/// except `AssociationStatus`, which is the modelled `AssociationStatusName` enum's own
/// declaration (`smithy_query(query="shape", shape="AssociationStatusName")`). They are
/// written out here so the test pins them rather than echoing the service, and the rows
/// cover keys the issue this suite answers did not cite.
#[tokio::test]
async fn a_filter_value_outside_its_published_set_names_the_key_and_the_set() {
    let router = router();

    for (key, set, in_legacy_dialect) in PUBLISHED_SETS {
        let published: Vec<&str> = set.split(", ").collect();
        let first = published[0];
        let last = published[published.len() - 1];
        // (case, the value the filter carries) — the same domain, spelled every way that is
        // not one of its values.
        let refused: Vec<(String, String)> = vec![
            ("a value no node can hold".to_string(), "NotAPublishedValue".to_string()),
            ("the first published value lowercased".to_string(), first.to_lowercase()),
            ("the first published value uppercased".to_string(), first.to_uppercase()),
            ("the last published value lowercased".to_string(), last.to_lowercase()),
            ("a published value with a suffix".to_string(), format!("{first}2022")),
            ("a published value with a prefix".to_string(), format!("my{first}")),
            ("a prefix of a published value".to_string(), first[..1].to_string()),
            ("a published value with a trailing space".to_string(), format!("{first} ")),
        ];

        for (case, value) in &refused {
            let mut dialects: Vec<(&str, Value)> =
                vec![("Filters", filters_body(&[(key, &[value.as_str()])]))];
            if *in_legacy_dialect {
                dialects.push((
                    "InstanceInformationFilterList",
                    legacy_body(&[(key, &[value.as_str()])]),
                ));
            }
            for (dialect, body) in &dialects {
                let case = format!("{key} {case} ({value:?}) in {dialect}");
                let (status, response) = call(&router, OPERATION, body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: the filter value is refused, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "InvalidInstanceInformationFilterValue",
                    "case {case:?}: the refusal is the modelled filter-value error, got \
                     {response}"
                );
                let expected = format!("{key} filter is invalid, valid values are {set}");
                let stated = error_message(&response);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected {expected:?}, got {stated:?}"
                );
                assert!(
                    !stated.contains(&format!("'{value}'")),
                    "case {case:?}: the sentence quotes no rejected value, got {stated:?}"
                );
            }
        }
    }
}

/// Every value a key publishes is SERVED in both dialects, so the refusal above is about
/// the value set and the exactness of the comparison rather than about the key.
#[tokio::test]
async fn every_published_filter_value_is_served() {
    let router = router();

    for (key, set, in_legacy_dialect) in PUBLISHED_SETS {
        for value in set.split(", ") {
            let mut dialects: Vec<(&str, Value)> =
                vec![("Filters", filters_body(&[(key, &[value])]))];
            if *in_legacy_dialect {
                dialects.push((
                    "InstanceInformationFilterList",
                    legacy_body(&[(key, &[value])]),
                ));
            }
            for (dialect, body) in &dialects {
                let case = format!("{key}={value:?} in {dialect}");
                let (status, response) = call(&router, OPERATION, body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a published value is served, got {status} {response}"
                );
                assert!(
                    response[REPORT_MEMBER].is_array(),
                    "case {case:?}: expected a report in {REPORT_MEMBER}, got {response}"
                );
            }
        }
    }
}

/// The same refusal from the handler's own backstop: the page size is checked against the
/// modelled range inside the operation too ([`node_page_size`]), for a request the boundary
/// walk did not catch, and that path cannot word the refusal differently from the walk's.
#[test]
fn the_page_size_backstop_words_the_same_refusal() {
    // (case, the MaxResults the request carries, the bound it broke)
    let outside: &[(&str, i32, i64)] = &[
        ("one below the minimum", 4, MIN_PAGE_SIZE),
        ("one above the maximum", 51, MAX_PAGE_SIZE),
        ("zero", 0, MIN_PAGE_SIZE),
        ("negative", -1, MIN_PAGE_SIZE),
        ("far above the maximum", 100_000, MAX_PAGE_SIZE),
        ("the largest number the member can hold", i32::MAX, MAX_PAGE_SIZE),
    ];
    for (case, max_results, bound) in outside {
        let expected = page_size_refusal(*bound);
        let stated = node_page_size(Some(*max_results))
            .expect_err(&format!("case {case:?}: {max_results} is outside the range"));
        assert_eq!(
            stated, expected,
            "case {case:?} ({max_results}): expected {expected:?}, got {stated:?}"
        );
    }

    // Inside the range the backstop reports the page size itself, bounds included.
    for (case, max_results, served) in [
        ("the published minimum", Some(5), 5usize),
        ("the published maximum", Some(50), 50),
        ("no page size at all", None, 10),
    ] {
        let page = node_page_size(max_results)
            .unwrap_or_else(|why| panic!("case {case:?}: expected a page size, got {why:?}"));
        assert_eq!(
            page, served,
            "case {case:?}: expected a page of {served}, got {page}"
        );
    }
}

/// Every filter key whose values this operation publishes a set for: the key, the set in
/// the order the API publishes it, and whether the LEGACY dialect accepts that key — its
/// `key` member targets the modelled `InstanceInformationFilterKey` enum
/// (`smithy_query(query="shape", shape="InstanceInformationFilterKey")`), which carries no
/// `SourceTypes`, so a request naming it there is refused for the key and not for the value.
const PUBLISHED_SETS: &[(&str, &str, bool)] = &[
    ("PlatformTypes", "Windows, Linux, MacOS", true),
    ("PingStatus", "Online, ConnectionLost, Inactive", true),
    ("ResourceType", "EC2Instance, ManagedInstance", true),
    ("AssociationStatus", "Pending, Success, Failed", true),
    (
        "SourceTypes",
        "AWS::EC2::Instance, AWS::SSM::ManagedInstance, AWS::IoT::Thing, \
         Microsoft.Compute/virtualMachines, compute.googleapis.com/Instance",
        false,
    ),
];
