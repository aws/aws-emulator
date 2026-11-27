// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the S3-Table-Integration source family REFUSES about the two
//! members it resolves a request from: the `dataSource` an associate names, and the
//! `integrationArn` both the associate and the reader name.
//!
//! The rules under test, one per group below:
//!
//! 1. A data source's NAME is spelled in letters, digits and underscores. A name carrying
//!    anything else names no data source: the associate is refused HTTP 400
//!    `ValidationException` with the sentence stating that rule, and nothing is associated —
//!    so the reader still reports what it reported before.
//! 2. An `integrationArn` has to name an S3 Table Integration the ACCOUNT HOLDS, which is
//!    the ARN the service that OWNS one mints, in the partition, Region and account that
//!    integration lives in. Both operations state the rung it failed — a resource that is not
//!    an integration's, another account's integration, an ARN of another service, partition
//!    or Region, and a handle nothing is held under — rather than answering an integration
//!    that is not there as one with no associations. The CLASS each refusal travels under is
//!    RECORDED PER OPERATION: the reader answers every rung of its ladder the
//!    parameter-validity class (`ValidationException`), while the associate answers a handle
//!    of the right form the account holds nothing under as a resource that does not exist
//!    (`ResourceNotFoundException`).
//! 3. An ARN-typed member of this family is held FIRST to the length an ARN takes: a value
//!    shorter than that is refused with the standard one-violation constraint clause naming
//!    the member and the bound, and only a value long enough to be an ARN is read as one.
//! 4. The integration an ARN resolves to is the one the OWNING service holds: an integration
//!    created through that service's own API is one a source can be associated with, and the
//!    association is reported for it.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed by the service cannot pass.
//!
//! The tables span what each rule generalises over rather than one request: names carrying
//! each punctuation class a caller plausibly sends and names over every character class the
//! rule admits, at one character and at 256; integration ids of three forms, against ARNs
//! wrong in each field they can be wrong in — the partition, the service, the Region, the
//! account, the resource type, the id and whether the account holds it; and lengths at the
//! bound, one short of it and far below it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{seed_s3_table_integration, Logs, REGION};

/// The operations under test.
const ASSOCIATE: &str = "AssociateSourceToS3TableIntegration";
const LIST: &str = "ListSourcesForS3TableIntegration";

/// The code this family answers for a parameter it will not accept.
const VALIDATION: &str = "ValidationException";

/// The code the associate answers for an `integrationArn` that IS the handle of an integration
/// this caller could hold and names one the account does not hold: a resource that does not
/// exist (smithy_query: operations AssociateSourceToS3TableIntegration and
/// ListSourcesForS3TableIntegration both model `ResourceNotFoundException`, "the specified
/// resource does not exist", httpError 400). The reader records the parameter-validity class
/// for that rung instead.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The fixed sentence a source name outside the character set is refused with. It states the
/// rule and carries no part of the submitted name.
const SOURCE_NAME_MESSAGE: &str =
    "Source name must only contain letters, numbers, and underscores";

/// The fixed sentence a source TYPE that is not spelled the way a data source type is spelled
/// is refused with — one sentence for either fault, a character outside the set and a letter
/// in the wrong case. Stated here independently of the crate under test.
const SOURCE_TYPE_MESSAGE: &str =
    "Source type must only contain letters, numbers, and underscores";

/// The ARN that is the handle on the S3 Table Integration `id` in the Region the calls are
/// signed for: the one the service that owns one mints, which is what an `integrationArn`
/// names.
fn integration_arn(id: &str) -> String {
    crate::harness::s3_table_integration_arn(REGION, id)
}

/// The refusal both operations answer an `integrationArn` that is not the handle of any
/// integration this caller could hold: the fixed text and the ARN it could not resolve.
fn unreachable_arn_refusal(arn: &str) -> String {
    format!("Invalid integration ARN: {arn}")
}

/// The refusal both operations answer an `integrationArn` whose RESOURCE is not an
/// integration's at all: the resource form they expect, with that ARN after it.
fn arn_form_refusal(arn: &str) -> String {
    format!(
        "Invalid S3 table integration ARN format. Expected: s3tableintegration/\
         {{integration-id}}: {arn}"
    )
}

/// The refusal both operations answer an `integrationArn` naming another ACCOUNT's
/// integration.
fn cross_account_refusal(arn: &str) -> String {
    format!("Cross account access not supported for S3 Table Integrations: {arn}")
}

/// The refusal both operations answer a handle the account holds no integration under.
fn not_found_refusal(arn: &str) -> String {
    format!("Integration not found: {arn}")
}

/// The refusal a value shorter than an ARN must answer, on the member it arrived at.
fn short_arn_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have length greater than or equal to 20"
    )
}

/// The body one call of `operation` sends for `integration_arn`, carrying the `dataSource`
/// the associate requires.
fn body(operation: &str, integration_arn: &str) -> Value {
    let mut body = json!({"integrationArn": integration_arn});
    if operation == ASSOCIATE {
        body["dataSource"] = json!({"name": "amazon_vpc", "type": "vended_logs"});
    }
    body
}

/// Associate `name` with `integration_arn`, returning the status and the parsed body.
async fn associate(logs: &Logs, integration_arn: &str, name: &str) -> (u16, Value) {
    logs.call(
        ASSOCIATE,
        &json!({
            "integrationArn": integration_arn,
            "dataSource": {"name": name, "type": "vended_logs"},
        }),
    )
    .await
}

/// The identifiers the reader reports for `integration_arn`, which must be a success.
async fn listed(logs: &Logs, integration_arn: &str) -> Vec<String> {
    let page = logs
        .ok(LIST, &json!({"integrationArn": integration_arn}))
        .await;
    page.get("sources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a listing carries sources, got {page}"))
        .iter()
        .filter_map(|source| {
            source
                .get("identifier")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// RULE 1: a data source whose name carries anything but letters, digits and underscores is
/// refused with the sentence stating that rule and associates nothing; a name inside that
/// set is associated, however long it is and whichever of those characters it uses.
#[tokio::test]
async fn a_source_name_outside_the_character_set_is_refused_and_associates_nothing() {
    let logs = Logs::new();
    let integration_arn = seed_s3_table_integration(&logs, REGION, "logs-lake");

    // One association the refusals must not disturb.
    let (status, accepted) = associate(&logs, &integration_arn, "amazon_vpc").await;
    assert_eq!(
        status, 200,
        "case \"the association the refusals must not disturb\": expected HTTP 200, got \
         {status} {accepted}"
    );
    let held = listed(&logs, &integration_arn).await;
    assert_eq!(
        held.len(),
        1,
        "case \"the association the refusals must not disturb\": expected one association, \
         got {held:?}"
    );

    // (case, the name) — one row per punctuation class a caller plausibly sends, plus a
    // space, a name that is only a separator, and a non-ASCII letter.
    let refused = [
        ("a hyphen", "amazon-vpc"),
        ("a dot", "orders.fn"),
        ("a slash", "aws/lambda/orders"),
        ("a colon pair", "AWS::EC2::VPCFlowLogs"),
        ("a space", "amazon vpc"),
        ("a plus", "amazon+vpc"),
        ("an at sign", "logs@amazon"),
        ("a hash", "amazon#vpc"),
        ("nothing but a separator", "-"),
        ("a non-ASCII letter", "amazon_vpcé"),
    ];
    for (case, name) in refused {
        let (status, answered) = associate(&logs, &integration_arn, name).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({name:?}): expected HTTP 400, got {status} {answered}"
        );
        let code = crate::harness::error_code(&answered);
        assert_eq!(
            code, VALIDATION,
            "case {case:?} ({name:?}): expected {VALIDATION}, got {code} / {answered}"
        );
        let message = answered.get("message").and_then(Value::as_str);
        assert_eq!(
            message,
            Some(SOURCE_NAME_MESSAGE),
            "case {case:?} ({name:?}): expected {SOURCE_NAME_MESSAGE:?}, got {answered}"
        );
    }

    // Nothing above was associated: the reader still reports the one association.
    let after = listed(&logs, &integration_arn).await;
    assert_eq!(
        after, held,
        "case \"after every refusal\": expected the associations {held:?}, got {after:?}"
    );

    // (case, the name) — every character class the rule admits, at the ends of the lengths
    // a name is seen in.
    let longest = "n".repeat(256);
    let accepted_names = [
        ("one lower-case letter", "w"),
        ("one digit", "7"),
        ("one underscore", "_"),
        ("upper and lower case", "AmazonVPC"),
        ("digits and underscores", "aws_cloudtrail_2"),
        ("a leading underscore", "_amazon_vpc"),
        ("a trailing underscore", "amazon_vpc_"),
        ("a 256-character name", longest.as_str()),
    ];
    for (case, name) in accepted_names {
        let (status, answered) = associate(&logs, &integration_arn, name).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({name:?}): expected HTTP 200, got {status} {answered}"
        );
        let identifier = answered.get("identifier").and_then(Value::as_str);
        assert!(
            identifier.is_some_and(|identifier| !identifier.is_empty()),
            "case {case:?} ({name:?}): an accepted associate carries an identifier, got \
             {answered}"
        );
        let reported = listed(&logs, &integration_arn).await;
        assert!(
            identifier.is_some_and(|identifier| reported.iter().any(|held| held == identifier)),
            "case {case:?} ({name:?}): expected {identifier:?} among {reported:?}"
        );
    }
}

/// RULE 2: both operations resolve an integration through one ladder, and state the rung an
/// ARN failed rather than answering an integration that is not there.
#[tokio::test]
async fn an_arn_naming_no_integration_of_this_account_is_refused_by_both_operations() {
    let logs = Logs::new();

    // (case, the integration id) — the id forms an integration's ARN is seen carrying.
    let reachable = [
        ("a plain name", "logs-lake"),
        (
            "the UUID form the owning service mints",
            "6f5a0b2c-1d3e-4f50-8a9b-0c1d2e3f4a5b",
        ),
        ("a hexadecimal token", "9f0a1b2c3d4e5f60718293a4b5c6d7e8"),
        ("one character", "a"),
    ];
    for (case, id) in reachable {
        let arn = seed_s3_table_integration(&logs, REGION, id);
        let (status, answered) = logs.call(ASSOCIATE, &body(ASSOCIATE, &arn)).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({arn}): expected the associate to be served, got {status} \
             {answered}"
        );
        let reported = listed(&logs, &arn).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?} ({arn}): expected the one association of that integration, got \
             {reported:?}"
        );
    }

    // (case, the ARN, the refusal each operation must answer) — wrong in each field an
    // integration's ARN can be wrong in, including a handle of the right form the account
    // holds nothing under. The two operations do not always answer alike: the ASSOCIATE's
    // ladder has a rung of its own for an integration of THIS service, and takes the
    // NOT-FOUND class for a handle the account holds nothing under, while the READER resolves
    // one family and answers every rung under the one class it records.
    let another_account = format!("arn:aws:observabilityadmin:{REGION}:123456789012:s3tableintegration/x");
    let unheld = integration_arn("held-by-nothing");
    let unreachable: [UnreachableArn; 10] = [
        UnreachableArn::alike(
            "another partition",
            format!("arn:aws-us-gov:observabilityadmin:{REGION}:{ACCOUNT}:s3tableintegration/x"),
            VALIDATION,
            unreachable_arn_refusal(&format!(
                "arn:aws-us-gov:observabilityadmin:{REGION}:{ACCOUNT}:s3tableintegration/x"
            )),
        ),
        UnreachableArn::alike(
            "another service",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:s3tableintegration/x"),
            VALIDATION,
            unreachable_arn_refusal(&format!(
                "arn:aws:logs:{REGION}:{ACCOUNT}:s3tableintegration/x"
            )),
        ),
        UnreachableArn::alike(
            "another Region",
            format!("arn:aws:observabilityadmin:us-west-2:{ACCOUNT}:s3tableintegration/x"),
            VALIDATION,
            unreachable_arn_refusal(&format!(
                "arn:aws:observabilityadmin:us-west-2:{ACCOUNT}:s3tableintegration/x"
            )),
        ),
        UnreachableArn::alike(
            "another account",
            another_account.clone(),
            VALIDATION,
            cross_account_refusal(&another_account),
        ),
        UnreachableArn {
            case: "a handle the account holds no integration under",
            arn: unheld.clone(),
            associate: (NOT_FOUND, not_found_refusal(&unheld)),
            list: (VALIDATION, not_found_refusal(&unheld)),
        },
        UnreachableArn {
            case: "an integration of this service instead",
            arn: format!("arn:aws:logs:{REGION}:{ACCOUNT}:integration:opensearch"),
            associate: (
                NOT_FOUND,
                not_found_refusal(&format!(
                    "arn:aws:logs:{REGION}:{ACCOUNT}:integration:opensearch"
                )),
            ),
            list: (
                VALIDATION,
                arn_form_refusal(&format!(
                    "arn:aws:logs:{REGION}:{ACCOUNT}:integration:opensearch"
                )),
            ),
        },
        UnreachableArn::alike(
            "an S3 Tables table",
            format!("arn:aws:s3tables:{REGION}:{ACCOUNT}:bucket/logs-lake/table/cloudwatch"),
            VALIDATION,
            arn_form_refusal(&format!(
                "arn:aws:s3tables:{REGION}:{ACCOUNT}:bucket/logs-lake/table/cloudwatch"
            )),
        ),
        UnreachableArn::alike(
            "the resource type spelled with hyphens",
            format!("arn:aws:observabilityadmin:{REGION}:{ACCOUNT}:s3-table-integration/x"),
            VALIDATION,
            arn_form_refusal(&format!(
                "arn:aws:observabilityadmin:{REGION}:{ACCOUNT}:s3-table-integration/x"
            )),
        ),
        UnreachableArn::alike(
            "the resource prefix with no integration id",
            format!("arn:aws:observabilityadmin:{REGION}:{ACCOUNT}:s3tableintegration/"),
            VALIDATION,
            arn_form_refusal(&format!(
                "arn:aws:observabilityadmin:{REGION}:{ACCOUNT}:s3tableintegration/"
            )),
        ),
        UnreachableArn::alike(
            "a log group",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/lambda/orders"),
            VALIDATION,
            arn_form_refusal(&format!(
                "arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/lambda/orders"
            )),
        ),
    ];
    for row in &unreachable {
        for (operation, (expected_code, expected)) in
            [(ASSOCIATE, &row.associate), (LIST, &row.list)]
        {
            let case = row.case;
            let arn = &row.arn;
            let (status, answered) = logs.call(operation, &body(operation, arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({arn}) on {operation}: expected HTTP 400, got {status} \
                 {answered}"
            );
            let code = crate::harness::error_code(&answered);
            assert_eq!(
                code, *expected_code,
                "case {case:?} ({arn}) on {operation}: expected {expected_code}, got {code} / \
                 {answered}"
            );
            assert_eq!(
                answered.get("message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?} ({arn}) on {operation}: expected {expected:?}, got {answered}"
            );
        }
    }
}

/// One row of RULE 2's table: an `integrationArn` that resolves to no S3 Table Integration,
/// and the `(code, message)` each operation of the family refuses it with.
struct UnreachableArn {
    case: &'static str,
    arn: String,
    /// What `AssociateSourceToS3TableIntegration` answers.
    associate: (&'static str, String),
    /// What `ListSourcesForS3TableIntegration` answers.
    list: (&'static str, String),
}

impl UnreachableArn {
    /// A row both operations answer identically, which is every ARN that could name no
    /// integration of this account at all.
    fn alike(case: &'static str, arn: String, code: &'static str, message: String) -> Self {
        Self {
            case,
            arn,
            associate: (code, message.clone()),
            list: (code, message),
        }
    }
}

/// RULE 2 (the integration families): WHICH integrations an operation of this family resolves
/// is the operation's own. The ASSOCIATE resolves an integration of THIS service's own family
/// too, so an ARN naming one the account does not hold is the NOT-FOUND class — the request
/// addresses a resource that does not exist — and one the account DOES hold is a real resource
/// of the wrong KIND, told the resource form an S3 Table Integration's ARN carries. The READER
/// resolves the S3 Table Integrations alone, so either way that ARN is simply not an S3 Table
/// Integration's and is told that form, under the one class this operation records.
///
/// The ONE ARN is sent around that state: before this service holds an integration of that
/// name, and after it does — so what decides is the integration the account holds and nothing
/// about the request.
#[tokio::test]
async fn an_integration_of_this_service_is_not_found_until_the_account_holds_it() {
    let logs = Logs::new();
    let name = "opensearch-held";
    let arn = format!("arn:aws:logs:{REGION}:{ACCOUNT}:integration:{name}");

    // (the operation, the code it answers, the sentence it answers) before the account holds
    // an integration of that name.
    for (operation, code_expected, expected) in [
        (ASSOCIATE, NOT_FOUND, not_found_refusal(&arn)),
        (LIST, VALIDATION, arn_form_refusal(&arn)),
    ] {
        let (status, answered) = logs.call(operation, &body(operation, &arn)).await;
        let code = crate::harness::error_code(&answered);
        assert_eq!(
            (status, code.as_str(), answered.get("message").and_then(Value::as_str)),
            (400, code_expected, Some(expected.as_str())),
            "case \"an integration of this service the account does not hold\" on {operation}: \
             expected 400 {code_expected} {expected:?}, got {status} {answered}"
        );
    }

    // The integration of this service's own family, seeded into the store its own reads serve
    // from, so everything asserted after it goes over the wire like any client's call.
    seed_logs_integration(&logs, name);

    for operation in [ASSOCIATE, LIST] {
        let (status, answered) = logs.call(operation, &body(operation, &arn)).await;
        let code = crate::harness::error_code(&answered);
        let expected = arn_form_refusal(&arn);
        assert_eq!(
            (status, code.as_str(), answered.get("message").and_then(Value::as_str)),
            (400, VALIDATION, Some(expected.as_str())),
            "case \"an integration of this service the account holds\" on {operation}: expected \
             400 {VALIDATION} {expected:?}, got {status} {answered}"
        );
    }
}

/// One integration of THIS service's own family, under the name `name`.
///
/// Written through this service's own record type on the shared engine — the same store
/// `GetIntegration` reads — because what the case needs is only that the account HOLDS one,
/// and the create's own OpenSearch setup is another suite's subject.
fn seed_logs_integration(logs: &Logs, name: &str) {
    use service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs_state;

    let config = service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::OpenSearchResourceConfig {
        kms_key_arn: None,
        data_source_role_arn: format!("arn:aws:iam::{ACCOUNT}:role/CloudWatchOpenSearch"),
        dashboard_viewer_principals: vec![format!("arn:aws:iam::{ACCOUNT}:role/Viewer")],
        application_arn: None,
        retention_days: 7,
    };
    let state = logs
        .engine()
        .state::<logs_state::ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let seq = store.next_integration_seq;
    store.next_integration_seq += 1;
    store.integrations.insert(
        name.to_string(),
        logs_state::IntegrationRecord::new(
            REGION,
            name,
            logs_state::INTEGRATION_TYPE_OPENSEARCH,
            &config,
            crate::harness::now_ms(),
            seq,
            // Seeded straight into the store, so no request's own endpoint stands behind it.
            None,
        ),
    );
}

/// RULE 2 (the type): a data source names a TYPE, so an associate that names none is
/// refused at the position the service validates it at and associates nothing.
#[tokio::test]
async fn an_associate_that_names_no_source_type_is_refused() {
    let logs = Logs::new();
    let arn = seed_s3_table_integration(&logs, REGION, "logs-lake");

    let expected = "1 validation error detected: Value null at 'sourceType' failed to satisfy \
                    constraint: Member must not be null";
    // (case, the `dataSource` the request carries)
    let refused = [
        ("the member absent", json!({"name": "amazon_vpc"})),
        ("another legal name, still no type", json!({"name": "w"})),
    ];
    for (case, data_source) in refused {
        let (status, answered) = logs
            .call(
                ASSOCIATE,
                &json!({"integrationArn": arn, "dataSource": data_source}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answered}"
        );
        let code = crate::harness::error_code(&answered);
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code} / {answered}"
        );
        assert_eq!(
            answered.get("message").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {answered}"
        );
    }
    let reported = listed(&logs, &arn).await;
    assert!(
        reported.is_empty(),
        "case \"after every refusal\": nothing is associated, got {reported:?}"
    );

    // A type names the kind of logs the source carries, whichever kind it is — spelled the way
    // a data source type is spelled.
    for source_type in ["vended_logs", "cloudtrail", "a", "flow", "ocsf_2", "_"] {
        let (status, answered) = logs
            .call(
                ASSOCIATE,
                &json!({
                    "integrationArn": arn,
                    "dataSource": {"name": "amazon_vpc", "type": source_type},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {source_type:?}: expected HTTP 200, got {status} {answered}"
        );
    }

    // The same words spelled in capitals are no data source type — that spelling belongs to the
    // vended-log `logType` catalogue — so each is refused with the sentence stating the rule,
    // and the associations already held are left alone.
    let held_before = listed(&logs, &arn).await;
    for source_type in ["VENDED_LOGS", "CLOUDTRAIL", "A", "Flow", "application_Logs"] {
        let (status, answered) = logs
            .call(
                ASSOCIATE,
                &json!({
                    "integrationArn": arn,
                    "dataSource": {"name": "amazon_vpc", "type": source_type},
                }),
            )
            .await;
        let code = crate::harness::error_code(&answered);
        assert_eq!(
            (status, code.as_str(), answered.get("message").and_then(Value::as_str)),
            (400, VALIDATION, Some(SOURCE_TYPE_MESSAGE)),
            "case {source_type:?}: expected 400 {VALIDATION} {SOURCE_TYPE_MESSAGE:?}, got \
             {status} {answered}"
        );
    }
    let held_after = listed(&logs, &arn).await;
    assert_eq!(
        held_after, held_before,
        "case \"after every type refusal\": expected the associations {held_before:?}, got \
         {held_after:?}"
    );
}

/// RULE 3: an ARN-typed member of this family is held to the minimum length an ARN takes
/// before anything is read out of it, on every operation of the family that carries one.
#[tokio::test]
async fn an_arn_shorter_than_an_arn_is_refused_on_that_bound() {
    let logs = Logs::new();

    // The bound is the 20 characters an ARN takes, so the row at exactly 20 is read as an
    // ARN and held to naming an integration instead.
    let one_short = "arn:aws:logs:::aaaa";
    assert_eq!(
        one_short.len(),
        19,
        "the row below must be one character short of the bound"
    );
    let at_the_bound = "arn:aws:logs:::aaaaa";
    assert_eq!(
        at_the_bound.len(),
        20,
        "the row below must be exactly at the bound"
    );

    // (case, the value, the whole message it must be refused with — `None` where each
    // operation words that refusal its own way)
    let rows: [(&str, &str, Option<String>); 5] = [
        (
            "an empty value",
            "",
            Some(short_arn_message("integrationArn")),
        ),
        (
            "one character",
            "a",
            Some(short_arn_message("integrationArn")),
        ),
        (
            "one character short of the bound",
            one_short,
            Some(short_arn_message("integrationArn")),
        ),
        (
            "at the bound, and read as an ARN",
            at_the_bound,
            // Long enough to be read as an ARN, so it is held to naming an integration
            // instead: its resource is not an integration's.
            Some(arn_form_refusal(at_the_bound)),
        ),
        (
            "long enough, but not an ARN at all",
            "not-an-arn-but-long-enough",
            Some(
                "1 validation error detected: Unable to parse integrationArn \
                 not-an-arn-but-long-enough"
                    .to_string(),
            ),
        ),
    ];
    for (case, value, expected) in rows {
        for operation in [ASSOCIATE, LIST] {
            let (status, answered) = logs.call(operation, &body(operation, value)).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {answered}"
            );
            let expected = expected
                .clone()
                .unwrap_or_else(|| unreachable_arn_refusal(value));
            assert_eq!(
                answered.get("message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?} on {operation}: expected {expected:?}, got {answered}"
            );
        }
    }
}

/// CloudWatch Logs and the service that OWNS an S3 Table Integration, on one shared engine,
/// reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_observabilityadmin_observabilityadmin::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One awsJson1_1 CloudWatch Logs call: its status and parsed body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        self.answered(request).await
    }

    /// One `restJson1` call to the service that owns an S3 Table Integration, mounted beside
    /// CloudWatch Logs: a POST of the operation's own URI through the SAME router, so both
    /// services read one engine.
    async fn owner(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("content-type", "application/json")
            .header("host", format!("observabilityadmin.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                     observabilityadmin/aws4_request, SignedHeaders=host, \
                     Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        self.answered(request).await
    }

    /// The status and parsed body of one dispatched request.
    async fn answered(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// RULE 4: the integration an `integrationArn` resolves to is one the OWNING service holds,
/// so an integration created through that service's own API is one a source can be
/// associated with — and one the owning service does not hold is not, whatever its ARN
/// looks like.
#[tokio::test]
async fn an_integration_the_owning_service_created_is_one_a_source_can_be_associated_with() {
    let emulator = Emulator::new();

    // The integration, created through the API of the service that owns it. Its ARN is that
    // service's own; nothing of it is built here.
    let (status, created) = emulator
        .owner(
            "CreateS3TableIntegration",
            &json!({
                "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/S3TablesIntegration"),
                "Encryption": {"SseAlgorithm": "AES256"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the owning service's create\": expected HTTP 200, got {status} {created}"
    );
    let arn = created
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the create answers the integration's ARN, got {created}"))
        .to_string();

    // A source associated through that ARN is associated, and reported for that integration.
    let (status, associated) = emulator
        .logs(
            ASSOCIATE,
            &json!({
                "integrationArn": arn,
                "dataSource": {"name": "amazon_vpc", "type": "vended_logs"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an integration the owner holds\": expected HTTP 200, got {status} {associated}"
    );
    let identifier = associated
        .get("identifier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an accepted associate carries an identifier, got {associated}"))
        .to_string();
    let (status, page) = emulator.logs(LIST, &json!({"integrationArn": arn})).await;
    assert_eq!(
        status, 200,
        "case \"the reader of that integration\": expected HTTP 200, got {status} {page}"
    );
    let reported: Vec<&str> = page
        .get("sources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a listing carries sources, got {page}"))
        .iter()
        .filter_map(|source| source.get("identifier").and_then(Value::as_str))
        .collect();
    assert_eq!(
        reported,
        vec![identifier.as_str()],
        "case \"the reader of that integration\": expected the association {identifier}, got \
         {reported:?}"
    );

    // The same integration DELETED through the owning service is one no source can be
    // associated with any more: the ARN resolves to nothing, and both operations say so —
    // each under the class it records for that rung.
    let (status, deleted) = emulator
        .owner("DeleteS3TableIntegration", &json!({"Arn": arn}))
        .await;
    assert!(
        (200..300).contains(&status),
        "case \"the owning service's delete\": expected a success, got {status} {deleted}"
    );
    for (operation, code_expected) in [(ASSOCIATE, NOT_FOUND), (LIST, VALIDATION)] {
        let (status, answered) = emulator
            .logs(
                operation,
                &json!({
                    "integrationArn": arn,
                    "dataSource": {"name": "amazon_vpc", "type": "vended_logs"},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case \"an integration the owner no longer holds\" on {operation}: expected HTTP \
             400, got {status} {answered}"
        );
        let expected = not_found_refusal(&arn);
        let code = crate::harness::error_code(&answered);
        assert_eq!(
            code, code_expected,
            "case \"an integration the owner no longer holds\" on {operation}: expected \
             {code_expected}, got {code} / {answered}"
        );
        assert_eq!(
            answered.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case \"an integration the owner no longer holds\" on {operation}: expected \
             {expected:?}, got {answered}"
        );
    }
}
