// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW a paginated report refuses a `Marker` it cannot place: the
//! sentence it answers with.
//!
//! RULE: the invalid-marker refusal sentence belongs to the REPORT that rejected the token,
//! not to the pagination layer that cut the page. Every report of this front door answers the
//! fixed sentence, which carries no variable part and reflects nothing of the request back;
//! only the PARAMETER reports — the ones whose `@paginated` items are a parameter list
//! (`DescribeDBParameters`, `DescribeDBClusterParameters` and the two engine-default reports)
//! — word it with the rejected token echoed after a colon. Both sentences carry the Query
//! API's own `InvalidParameterValue` code and HTTP 400, which is the refusal for a token this
//! service's codec could not have produced at all (these reports bind no error of their own:
//! `smithy_query(query="operation",operation="DescribeDBEngineVersions")` reports
//! `errors: []`).
//!
//! The tables span what the rule generalises over:
//!
//! * the report, over both classes: published reference data (engine versions, major engine
//!   versions, CA certificates, orderable options), records a caller created or this service
//!   provides (instances, snapshots, option groups, parameter groups, subnet groups,
//!   clusters), a group's own stored parameters, a cluster group's parameters and the two
//!   published engine-default parameter catalogs — including reports no issue cited;
//! * the shape of the rejected token: punctuation only, a word, a single character, encoded
//!   bytes that decode to no key, characters the XML envelope has to escape, a plausible key
//!   of another report, and the longest token a `Marker` member admits;
//! * whether the token is reflected: the fixed sentence carries no part of it, the parameter
//!   reports' sentence carries it verbatim.
//!
//! A second test shows the refusals are about the TOKEN rather than about the member: on one
//! report of each class, a `Marker` a prior page DID issue resumes the report beyond the
//! records that page served.
//!
//! Every call goes over the wire through the harness `db_instances.rs` owns, so each
//! assertion reads the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    invalid_marker_message, invalid_parameter_marker_message,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_parameter_groups::create_group as create_cluster_parameter_group;
use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text};
use crate::db_parameters::create_group as create_db_parameter_group;

/// The Query API's own code for a member value this service will not serve, which a token it
/// cannot place is one of.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The longest `Marker` a member of this front door admits: `len 1..340`
/// (`smithy_query(query="shape",shape="DescribeDBMajorEngineVersionsRequest")`).
const MAX_MARKER_LEN: usize = 340;

/// The smallest page `MaxRecords` admits: "Constraints: Minimum 20, maximum 100."
const SMALLEST_PAGE: usize = 20;

/// How many parameters the stored-parameter report below holds: more than one page, so that
/// report issues a `Marker` of its own.
const STORED_PARAMETERS: usize = 25;

/// Which invalid-marker sentence a report answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wording {
    /// The sentence with no variable part, which reflects nothing the request carried.
    Fixed,
    /// The parameter reports' sentence, which carries the rejected token verbatim.
    EchoesTheToken,
}

impl Wording {
    /// The whole sentence this wording answers the token `token` with.
    fn sentence(self, token: &str) -> String {
        match self {
            Self::Fixed => invalid_marker_message(),
            Self::EchoesTheToken => invalid_parameter_marker_message(token),
        }
    }
}

/// One report under test: how to ask for it, and the wording its class refuses a token in.
struct Report {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    wording: Wording,
}

/// A request member, owned so a row can carry a created resource's name.
fn member(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The key of every record `body` reports: one entry per `record_element` subtree, keyed by
/// that record's own `key_elements` joined.
///
/// The record element is what makes "the key of this record" well defined: an engine-version
/// entry carries its upgrade targets and each target carries its own `EngineVersion`, and one
/// version string is published by several engines, so the key is read INSIDE the record and
/// spans every element the record's identity needs.
fn keys(body: &str, record_element: &str, key_elements: &[&str]) -> Vec<String> {
    subtrees(body, record_element)
        .into_iter()
        .map(|record| {
            key_elements
                .iter()
                .map(|element| text(&record, element))
                .collect::<Vec<String>>()
                .join("/")
        })
        .collect()
}

/// The names the stored-parameter report holds, in the order it serves them.
fn stored_parameter_names() -> Vec<String> {
    (0..STORED_PARAMETERS)
        .map(|index| format!("p_{index:03}"))
        .collect()
}

/// Write `names` into the DB parameter group `group`, in modifies no longer than the
/// documented twenty parameters per request.
async fn fill_group(router: &MultiServiceRouter, group: &str, names: &[String]) {
    for chunk in names.chunks(20) {
        let mut params: Vec<(String, String)> = vec![member("DBParameterGroupName", group)];
        for (index, name) in chunk.iter().enumerate() {
            let position = index + 1;
            params.push((
                format!("Parameters.Parameter.{position}.ParameterName"),
                name.clone(),
            ));
            params.push((
                format!("Parameters.Parameter.{position}.ParameterValue"),
                "1".to_string(),
            ));
        }
        expect_ok(
            "filling the group",
            call(router, "ModifyDBParameterGroup", &params).await,
        );
    }
}

/// An emulator holding what the reports below page over, and those reports.
async fn seeded() -> (MultiServiceRouter, Vec<Report>) {
    let router = emulator();
    // A family this service publishes no instance-parameter catalog for, so the group's own
    // report holds exactly what a caller wrote into it.
    let group = create_db_parameter_group(&router, "a group to page", "emu-marker-pg", "db2-se")
        .await;
    let cluster_group = create_cluster_parameter_group(
        &router,
        "a cluster group to page",
        "emu-marker-cpg",
        "docdb5.0",
    )
    .await;
    // Records for the stored-record reports, so a row of those is refused for its token
    // rather than answered off an empty store.
    expect_ok(
        "an option group to page",
        call(
            &router,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "emu-marker-og"),
                ("EngineName", "mysql"),
                ("MajorEngineVersion", "8.0"),
                ("OptionGroupDescription", "a group to page"),
            ],
        )
        .await,
    );

    let reports = vec![
        Report {
            case: "the published engine versions",
            action: "DescribeDBEngineVersions",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the published major engine versions",
            action: "DescribeDBMajorEngineVersions",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the published CA certificates",
            action: "DescribeCertificates",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the orderable instance options",
            action: "DescribeOrderableDBInstanceOptions",
            params: vec![member("Engine", "mysql")],
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's DB instances",
            action: "DescribeDBInstances",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's DB snapshots",
            action: "DescribeDBSnapshots",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's DB clusters",
            action: "DescribeDBClusters",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's option groups",
            action: "DescribeOptionGroups",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's DB parameter groups",
            action: "DescribeDBParameterGroups",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "the account's DB subnet groups",
            action: "DescribeDBSubnetGroups",
            params: Vec::new(),
            wording: Wording::Fixed,
        },
        Report {
            case: "a group's own stored parameters",
            action: "DescribeDBParameters",
            params: vec![member("DBParameterGroupName", &group)],
            wording: Wording::EchoesTheToken,
        },
        Report {
            case: "a cluster group's own parameters",
            action: "DescribeDBClusterParameters",
            params: vec![member("DBClusterParameterGroupName", &cluster_group)],
            wording: Wording::EchoesTheToken,
        },
        Report {
            case: "a family's published engine defaults",
            action: "DescribeEngineDefaultParameters",
            params: vec![member("DBParameterGroupFamily", "mysql8.0")],
            wording: Wording::EchoesTheToken,
        },
        Report {
            case: "a family's published cluster engine defaults",
            action: "DescribeEngineDefaultClusterParameters",
            params: vec![member("DBParameterGroupFamily", "docdb5.0")],
            wording: Wording::EchoesTheToken,
        },
    ];
    (router, reports)
}

/// The tokens the table drives, each one no page of any report can have issued.
fn unplaceable_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("punctuation only", "!!!!".to_string()),
        ("a word", "not-a-marker".to_string()),
        ("one character", "m".to_string()),
        ("encoded bytes that are no key", "/////w==".to_string()),
        (
            "characters the XML envelope has to escape",
            "a&b<c>\"d'".to_string(),
        ),
        ("a plausible key of another report", "mysql#8.0.0".to_string()),
        (
            "the longest token the member admits",
            "m".repeat(MAX_MARKER_LEN),
        ),
    ]
}

#[tokio::test]
async fn a_token_no_page_issued_is_refused_in_its_own_reports_wording() {
    let (router, reports) = seeded().await;

    for (token_case, token) in unplaceable_tokens() {
        for report in &reports {
            let case = format!("{} resumed from {token_case}", report.case);
            let mut params = report.params.clone();
            params.push(member("Marker", &token));
            let body = expect_error(
                &case,
                call(&router, report.action, &params).await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = text(&body, "Message");
            let expected = report.wording.sentence(&token);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // What the two wordings differ in: the fixed sentence carries no part of the
            // token the request sent, the parameter reports' sentence carries it verbatim.
            // A token whose characters the fixed sentence already holds (a single letter)
            // cannot show that, so it is read only on the echoing class.
            let incidental = invalid_marker_message().contains(token.as_str());
            match report.wording {
                Wording::Fixed if !incidental => assert!(
                    !message.contains(token.as_str()),
                    "case {case:?}: the fixed sentence must reflect no part of the rejected \
                     token {token:?}, got {message:?}"
                ),
                Wording::Fixed => {}
                Wording::EchoesTheToken => assert!(
                    message.contains(token.as_str()),
                    "case {case:?}: this report's sentence must carry the rejected token \
                     {token:?}, got {message:?}"
                ),
            }
        }
    }
}

/// One row of the resume table: the case, the report, the members asking for it, the element
/// one record is reported in, and the elements that record's identity is read from.
struct Resumed {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    record_element: &'static str,
    key_elements: &'static [&'static str],
}

#[tokio::test]
async fn a_token_a_prior_page_issued_resumes_the_report_in_both_wordings() {
    let (router, _) = seeded().await;
    let group = create_db_parameter_group(&router, "a group to resume", "emu-resume-pg", "db2-se")
        .await;
    let names = stored_parameter_names();
    fill_group(&router, &group, &names).await;

    // One report of each wording class: a published catalogue that answers the fixed
    // sentence, and a group's own parameters, which echo the token.
    let rows = [
        Resumed {
            case: "the published engine versions",
            action: "DescribeDBEngineVersions",
            params: Vec::new(),
            record_element: "DBEngineVersion",
            key_elements: &["Engine", "EngineVersion"],
        },
        Resumed {
            case: "a group's own stored parameters",
            action: "DescribeDBParameters",
            params: vec![member("DBParameterGroupName", &group)],
            record_element: "Parameter",
            key_elements: &["ParameterName"],
        },
    ];

    for Resumed {
        case,
        action,
        params,
        record_element,
        key_elements,
    } in rows
    {
        let mut first_request = params.clone();
        first_request.push(member("MaxRecords", &SMALLEST_PAGE.to_string()));
        let first = expect_ok(case, call(&router, action, &first_request).await);
        let served = keys(&first, record_element, key_elements);
        assert_eq!(
            served.len(),
            SMALLEST_PAGE,
            "case {case:?}: a report longer than one page serves a full page, got {}",
            served.len()
        );
        let marker = text(&first, "Marker");
        assert!(
            !marker.is_empty(),
            "case {case:?}: a cut that leaves records unserved issues a Marker, got {first}"
        );

        let mut resumed_request = params.clone();
        resumed_request.push(member("MaxRecords", &SMALLEST_PAGE.to_string()));
        resumed_request.push(member("Marker", &marker));
        let resumed = expect_ok(case, call(&router, action, &resumed_request).await);
        let beyond = keys(&resumed, record_element, key_elements);
        assert!(
            !beyond.is_empty(),
            "case {case:?}: the resumed page must carry the records beyond the token, got \
             {resumed}"
        );
        assert!(
            beyond.iter().all(|key| !served.contains(key)),
            "case {case:?}: the resumed page serves only records the first page did not, got \
             {beyond:?} after {served:?}"
        );
    }
}
