// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the S3-Table-Integration source family:
//! `AssociateSourceToS3TableIntegration`, the read it is proved through
//! (`ListSourcesForS3TableIntegration`) and `DisassociateSourceFromS3TableIntegration`.
//!
//! The rule under test: an accepted associate persists ONE association of the data source
//! the request names with the integration its `integrationArn` names, hands back the unique
//! identifier that association is stored under, and `ListSourcesForS3TableIntegration`
//! reports exactly the associations of the integration it is asked about — each with the
//! data source as the associate named it, the status it is in and the instant it was
//! created — in the order they were associated, paged by `maxResults`. Associating a pair
//! that already has query access names the association that has it rather than a second
//! one. A disassociate takes query access away and leaves the data deletion running, so the
//! association is reported `DATA_SOURCE_DELETE_IN_PROGRESS` until the CLOCK — never a
//! call — has finished it, after which it is gone. An `integrationArn` that is not an ARN
//! and an identifier no association is stored under persist nothing and are refused.
//!
//! Every `integrationArn` here names an S3 Table Integration of the account and Region the
//! calls are signed for, and every data source is named in the letters, digits and
//! underscores a source name is spelled in; the refusals for the other forms are the
//! subject of `s3_table_source_validation`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: four integrations
//! of the account, their ids both plain names and in the UUID form the owning service mints;
//! data sources named from one character
//! to 256, two data sources differing only in their type, and several data sources of one
//! integration;
//! several associations per integration and several integrations per account; `maxResults`
//! at both published bounds of `ListSourcesForS3TableIntegrationMaxResults`' `@range(1..100)`
//! and one past each (source:
//! `smithy_query(query="operation",operation="ListSourcesForS3TableIntegration")`), and
//! both unset; and deletion instants pinned on either side of the span the clock owes. The
//! store cap is exercised at the cap, one past it, and after room is freed again.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::DataSource;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    mint_s3_table_integration_source_identifier, ComAmazonawsCloudwatchlogsLogs20140328State,
    S3TableIntegrationSourceRecord, DEFAULT_LIST_LIMIT, MAX_S3_TABLE_INTEGRATION_SOURCES,
    S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS, S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE,
    S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operations under test.
const ASSOCIATE: &str = "AssociateSourceToS3TableIntegration";
const LIST: &str = "ListSourcesForS3TableIntegration";
const DISASSOCIATE: &str = "DisassociateSourceFromS3TableIntegration";

/// The code this family answers for a parameter it will not accept ("One of the parameters
/// for the request is not valid").
const VALIDATION: &str = "ValidationException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a request would take the store past a cap ("The request was
/// throttled because of quota limits").
const THROTTLING: &str = "ThrottlingException";

/// The widest identifier the model admits (`S3TableIntegrationSourceIdentifier`,
/// `@length(min: 1, max: 2048)`), which every identifier the associate mints must fit.
const IDENTIFIER_MAX_LEN: usize = 2048;

/// How CloudWatch Logs words the refusal of an identifier that names no association: the
/// service's own sentence, carrying the identifier the request asked for.
fn not_found_message(identifier: &str) -> String {
    format!("Association not found for identifier: {identifier}")
}

/// The ARN that is the handle on the S3 Table Integration `id`, which is what every
/// `integrationArn` of this family names: the one the service that OWNS an S3 Table
/// Integration mints, in the account and Region the calls are signed for.
fn arn_of(id: &str) -> String {
    crate::harness::s3_table_integration_arn(REGION, id)
}

/// Stand up every integration this suite's rows name, so each `integrationArn` below names
/// one the account HOLDS. They are the owning service's records; see
/// [`crate::harness::seed_s3_table_integration`] for why they are seeded rather than created.
fn seed_integrations(logs: &Logs) {
    for id in [
        "logs-lake",
        "s3-tables-lake",
        "6f5a0b2c-1d3e-4f50-8a9b-0c1d2e3f4a5b",
        "analytics-lake",
        "empty-lake",
        "other-lake",
    ] {
        crate::harness::seed_s3_table_integration(logs, REGION, id);
    }
}

/// One integration of the account, named by a plain id.
fn table_arn() -> String {
    arn_of("logs-lake")
}

/// A SECOND integration of the account, so a row proves each integration reports its own
/// associations.
fn logs_integration_arn() -> String {
    arn_of("s3-tables-lake")
}

/// A third integration, whose id is in the UUID form the owning service mints.
fn telemetry_integration_arn() -> String {
    arn_of("6f5a0b2c-1d3e-4f50-8a9b-0c1d2e3f4a5b")
}

/// A fourth integration, so a row proves the association is kept against the ARN the request
/// gave rather than one rebuilt from the call's own scope.
fn gov_arn() -> String {
    arn_of("analytics-lake")
}

/// One association to make: the integration it is made against and the data source it gives
/// query access to.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    integration_arn: String,
    name: String,
    source_type: Option<&'static str>,
}

/// The `dataSource` member of an associate, as the request carries it.
fn data_source(name: &str, source_type: Option<&str>) -> Value {
    match source_type {
        Some(source_type) => json!({"name": name, "type": source_type}),
        None => json!({"name": name}),
    }
}

/// Associate `row`'s data source with its integration, returning the identifier handed back.
async fn associate(logs: &Logs, row: &Row) -> String {
    let body = json!({
        "integrationArn": row.integration_arn,
        "dataSource": data_source(&row.name, row.source_type),
    });
    let response = logs.ok(ASSOCIATE, &body).await;
    response
        .get("identifier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: an accepted associate carries an identifier, got {response}",
                row.case
            )
        })
        .to_string()
}

/// Every association the service reports for `integration_arn`, walked to the end through
/// `maxResults`-sized pages when one is given.
async fn sources(logs: &Logs, integration_arn: &str, max_results: Option<i64>) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per association plus the empty tail, which bounds the walk whatever the
    // service answers.
    for _ in 0..64 {
        let mut body = json!({"integrationArn": integration_arn});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok(LIST, &body).await;
        let listed = page
            .get("sources")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {integration_arn}: a page carries sources, got {page}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                i64::try_from(listed.len()).unwrap_or(i64::MAX) <= max_results,
                "case {integration_arn}: expected at most {max_results} per page, got {}",
                listed.len()
            );
        }
        collected.extend(listed);
        match page.get("nextToken").and_then(Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!("case {integration_arn}: the pagination walk must terminate");
}

/// One member of a reported association.
fn member<'a>(source: &'a Value, key: &str) -> Option<&'a Value> {
    source.get(key).filter(|value| !value.is_null())
}

/// The `identifier` of a reported association.
fn identifier_of(source: &Value) -> String {
    member(source, "identifier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a reported association carries an identifier, got {source}"))
        .to_string()
}

/// Move the instant the clock owes `identifier`'s deletion to `due_ms`, in the store.
///
/// Nothing a client can send chooses when the data deletion a disassociate leaves running
/// finishes, so a suite whose subject is what a reader reports on either side of that
/// instant pins it here and asserts the rest over the wire.
fn pin_removal_due(logs: &Logs, identifier: &str, due_ms: i64) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let association = store
        .s3_table_integration_sources
        .get_mut(identifier)
        .unwrap_or_else(|| panic!("the association under {identifier} is stored"));
    association.removal_due_ms = Some(due_ms);
}

/// The associations each integration holds are the ones associated with it, reported with
/// the data source the associate named, in the order they were associated — and an
/// integration nothing was associated with holds none.
#[tokio::test]
async fn an_associated_source_is_reported_for_its_own_integration() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let long_name = "a".repeat(256);
    let rows = [
        Row {
            case: "an S3 Tables table, a typed data source",
            integration_arn: table_arn(),
            name: "amazon_vpc".to_string(),
            source_type: Some("vpc_flow_logs"),
        },
        Row {
            case: "the same integration, the same name under another type",
            integration_arn: table_arn(),
            name: "amazon_vpc".to_string(),
            source_type: Some("ocsf_vpc_flow_logs"),
        },
        Row {
            case: "the same integration, another data source",
            integration_arn: table_arn(),
            name: "aws_lambda_orders".to_string(),
            source_type: Some("lambda_logs"),
        },
        Row {
            case: "a second integration, a one-character name",
            integration_arn: logs_integration_arn(),
            name: "w".to_string(),
            source_type: Some("waf_logs"),
        },
        Row {
            case: "another integration, a 256-character name",
            integration_arn: gov_arn(),
            name: long_name.clone(),
            source_type: Some("cloudtrail"),
        },
        Row {
            case: "an integration whose id is a UUID",
            integration_arn: telemetry_integration_arn(),
            name: "amazon_logs".to_string(),
            source_type: Some("cloudwatch_logs"),
        },
    ];

    let before = now_ms();
    let mut identifiers: Vec<String> = Vec::new();
    for row in &rows {
        identifiers.push(associate(&logs, row).await);
    }
    let after = now_ms();

    // Every identifier is the model's own shape and names ONE association: no two of the
    // five share one.
    let unique: BTreeSet<&String> = identifiers.iter().collect();
    assert_eq!(
        unique.len(),
        identifiers.len(),
        "case every row: expected {} distinct identifiers, got {identifiers:?}",
        identifiers.len()
    );
    for (row, identifier) in rows.iter().zip(&identifiers) {
        assert!(
            (1..=IDENTIFIER_MAX_LEN).contains(&identifier.chars().count()),
            "case {:?}: expected an identifier of 1..={IDENTIFIER_MAX_LEN} characters, got {identifier:?}",
            row.case
        );
    }

    for integration_arn in [
        table_arn(),
        logs_integration_arn(),
        gov_arn(),
        telemetry_integration_arn(),
    ] {
        let listed = sources(&logs, &integration_arn, None).await;
        let expected: Vec<(&Row, &String)> = rows
            .iter()
            .zip(&identifiers)
            .filter(|(row, _)| row.integration_arn == integration_arn)
            .collect();
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {integration_arn}: expected {} association(s), got {listed:?}",
            expected.len()
        );
        for (source, (row, identifier)) in listed.iter().zip(&expected) {
            let case = row.case;
            assert_eq!(
                &identifier_of(source),
                *identifier,
                "case {case:?}: expected the association {identifier}, got {source}"
            );
            let reported = member(source, "dataSource").unwrap_or_else(|| {
                panic!("case {case:?}: an association carries its dataSource, got {source}")
            });
            assert_eq!(
                reported,
                &data_source(&row.name, row.source_type),
                "case {case:?}: expected the dataSource {}, got {reported}",
                data_source(&row.name, row.source_type)
            );
            let status = member(source, "status").and_then(Value::as_str);
            assert_eq!(
                status,
                Some(S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE),
                "case {case:?}: expected the status {S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE}, got {status:?}"
            );
            let created = member(source, "createdTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an association carries createdTimeStamp, got {source}")
                });
            assert!(
                (before..=after).contains(&created),
                "case {case:?}: expected a createdTimeStamp in {before}..={after}, got {created}"
            );
            assert!(
                member(source, "parentSourceIdentifier").is_none(),
                "case {case:?}: an association a request made has no parent, got {source}"
            );
        }
    }

    // An integration nothing was associated with is read successfully and holds nothing.
    let untouched = arn_of("empty-lake");
    let listed = sources(&logs, &untouched, None).await;
    assert!(
        listed.is_empty(),
        "case an integration nothing was associated with: expected no association, got {listed:?}"
    );
}

/// An association IS one data source's query access through one integration, so associating
/// the same pair again names the association that already has it: the same identifier, and
/// still one association.
#[tokio::test]
async fn associating_a_pair_that_already_has_access_names_the_same_association() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let rows = [
        Row {
            case: "a typed data source",
            integration_arn: table_arn(),
            name: "amazon_vpc".to_string(),
            source_type: Some("vpc_flow_logs"),
        },
        Row {
            case: "another data source",
            integration_arn: logs_integration_arn(),
            name: "aws_lambda_orders".to_string(),
            source_type: Some("lambda_logs"),
        },
    ];

    for row in &rows {
        let case = row.case;
        let first = associate(&logs, row).await;
        let again = associate(&logs, row).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the stored identifier {first}, got {again}"
        );
        let listed = sources(&logs, &row.integration_arn, None).await;
        let identifiers: Vec<String> = listed.iter().map(identifier_of).collect();
        assert_eq!(
            identifiers,
            vec![first.clone()],
            "case {case:?}: expected the one association {first}, got {identifiers:?}"
        );
    }
}

/// A disassociate takes query access away and leaves the data deletion running: the
/// association is reported `DATA_SOURCE_DELETE_IN_PROGRESS` until the instant the clock owes
/// its deletion, and gone after it — and an association already gone is a missing resource.
#[tokio::test]
async fn a_disassociated_source_is_deleting_until_the_clock_has_removed_it() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let rows = [
        Row {
            case: "a typed data source",
            integration_arn: table_arn(),
            name: "amazon_vpc".to_string(),
            source_type: Some("vpc_flow_logs"),
        },
        Row {
            case: "another data source",
            integration_arn: table_arn(),
            name: "aws_lambda_orders".to_string(),
            source_type: Some("lambda_logs"),
        },
        Row {
            case: "another integration",
            integration_arn: gov_arn(),
            name: "cloudtrail".to_string(),
            source_type: Some("cloudtrail"),
        },
    ];
    let mut identifiers: Vec<String> = Vec::new();
    for row in &rows {
        identifiers.push(associate(&logs, row).await);
    }

    for (row, identifier) in rows.iter().zip(&identifiers) {
        let case = row.case;
        let response = logs
            .ok(DISASSOCIATE, &json!({"identifier": identifier}))
            .await;
        let echoed = member(&response, "identifier").and_then(Value::as_str);
        assert_eq!(
            echoed,
            Some(identifier.as_str()),
            "case {case:?}: expected the removed association {identifier}, got {response}"
        );
    }

    // Straight after the call the deletion is still running, on every one of them.
    for (row, identifier) in rows.iter().zip(&identifiers) {
        let case = row.case;
        let listed = sources(&logs, &row.integration_arn, None).await;
        let reported = listed
            .iter()
            .find(|source| &identifier_of(source) == identifier)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: an association whose data is still being deleted is \
                     reported, got {listed:?}"
                )
            });
        let status = member(reported, "status").and_then(Value::as_str);
        assert_eq!(
            status,
            Some(S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS),
            "case {case:?}: expected the status \
             {S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS}, got {status:?}"
        );
    }

    // (case, how the instant the clock owes the deletion sits relative to now, reported)
    //
    // The near-side rows are pinned far enough ahead of the due instant for the read that
    // follows to be taken while it is still ahead on a loaded machine — a row pinned a
    // millisecond ahead races the read itself, which says nothing about the rule. The
    // tight boundary is kept on the far side, where elapsed time cannot flip the answer.
    let ages: [(&str, i64, bool); 3] = [
        (
            "half a deletion span before it is due",
            S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS / 2,
            true,
        ),
        (
            "a whole deletion span before it is due",
            S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS,
            true,
        ),
        ("one millisecond after it was due", -1, false),
    ];
    let (row, identifier) = (&rows[0], &identifiers[0]);
    for (case, offset, reported) in ages {
        pin_removal_due(&logs, identifier, now_ms() + offset);
        let listed = sources(&logs, &row.integration_arn, None).await;
        let found = listed
            .iter()
            .any(|source| &identifier_of(source) == identifier);
        assert_eq!(
            found, reported,
            "case {case:?}: expected reported={reported}, got {listed:?}"
        );
        if !reported {
            // Gone is gone: the identifier names no association any more, which is what a
            // second disassociate of it is refused for, naming the identifier it asked for.
            let (code, message) = logs
                .refusal(DISASSOCIATE, &json!({"identifier": identifier}))
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {code} {message}"
            );
            assert_eq!(
                message,
                not_found_message(identifier),
                "case {case:?}: expected the message {:?}, got {message:?}",
                not_found_message(identifier)
            );
        }
    }
}

/// A read reports what the clock has made due and changes nothing: two reads of one
/// integration's associations report the same thing, and no read associates or removes one.
#[tokio::test]
async fn reading_an_integrations_sources_changes_nothing() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let row = Row {
        case: "one association, read twice",
        integration_arn: table_arn(),
        name: "amazon_vpc".to_string(),
        source_type: Some("vpc_flow_logs"),
    };
    let identifier = associate(&logs, &row).await;
    // Pinned well past any wall-clock drift between the two reads, so the only thing that
    // could differ is a read that wrote.
    pin_removal_due(
        &logs,
        &identifier,
        now_ms() + 10 * S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS,
    );

    let first = sources(&logs, &row.integration_arn, None).await;
    let second = sources(&logs, &row.integration_arn, None).await;
    assert_eq!(
        first, second,
        "case {:?}: expected the second read to report {first:?}, got {second:?}",
        row.case
    );
}

/// One integration's associations are walked exactly once, whatever the page size: every
/// published bound of `maxResults`, and unset, reports the same associations in the same
/// order — and a token is handed back only where a page follows.
#[tokio::test]
async fn pagination_walks_each_association_once() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let integration_arn = table_arn();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..5 {
        let row = Row {
            case: "one of five associations",
            integration_arn: integration_arn.clone(),
            name: format!("aws_vended_{index}"),
            source_type: Some(if index % 2 == 0 {
                "vended_logs"
            } else {
                "cloudtrail"
            }),
        };
        expected.push(associate(&logs, &row).await);
    }

    // The published bounds of `ListSourcesForS3TableIntegrationMaxResults`' `@range(1..100)`,
    // a size between them, and no maximum at all.
    for max_results in [Some(1), Some(2), Some(100), None] {
        let listed = sources(&logs, &integration_arn, max_results).await;
        let identifiers: Vec<String> = listed.iter().map(identifier_of).collect();
        assert_eq!(
            identifiers, expected,
            "case maxResults={max_results:?}: expected {expected:?}, got {identifiers:?}"
        );
    }

    // The last page of a walk carries no token: with no `maxResults` the page size is this
    // service's list default ([`DEFAULT_LIST_LIMIT`]), which is wider than the associations
    // this account holds, so the first page already holds them all.
    let whole = logs
        .ok(LIST, &json!({"integrationArn": integration_arn}))
        .await;
    let identifiers: Vec<String> = whole
        .get("sources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case one unpaged page: a page carries sources, got {whole}"))
        .iter()
        .map(identifier_of)
        .collect();
    assert_eq!(
        identifiers, expected,
        "case one unpaged page: expected {expected:?}, got {identifiers:?}"
    );
    assert!(
        identifiers.len() < usize::try_from(DEFAULT_LIST_LIMIT).unwrap_or(usize::MAX),
        "case one unpaged page: this case needs fewer than {DEFAULT_LIST_LIMIT} associations \
         to fit in the default page, got {}",
        identifiers.len()
    );
    assert!(
        member(&whole, "nextToken").is_none(),
        "case a page that ends the walk: expected no nextToken, got {whole}"
    );
}

/// A request this family will not accept is refused, and a refused request persists
/// nothing: an `integrationArn` that is not an ARN, a `maxResults` outside the range the
/// model publishes, a token no walk of this integration issued, and an identifier no
/// association is stored under.
#[tokio::test]
async fn a_request_the_family_cannot_accept_is_refused_and_stores_nothing() {
    let logs = Logs::new();
    seed_integrations(&logs);

    let integration_arn = table_arn();
    let stored = associate(
        &logs,
        &Row {
            case: "the one association the refusals must not disturb",
            integration_arn: integration_arn.clone(),
            name: "amazon_vpc".to_string(),
            source_type: Some("vpc_flow_logs"),
        },
    )
    .await;

    // (case, the value handed to `integrationArn`, the refusal it must answer) — a value
    // shorter than the length an ARN takes is refused on that bound, and a longer one that
    // is still not an ARN on being unparsable.
    let short_arn = "1 validation error detected: Value at 'integrationArn' failed to satisfy \
                     constraint: Member must have length greater than or equal to 20";
    let not_arns: [(&str, &str, &str); 4] = [
        ("an empty value", "", short_arn),
        ("a bare name", "logs-lake", short_arn),
        ("a prefix with no segments", "arn:", short_arn),
        (
            "long enough, but too few segments",
            "arn:aws:s3tables:eu-west-2",
            "1 validation error detected: Unable to parse integrationArn \
             arn:aws:s3tables:eu-west-2",
        ),
    ];
    for (case, value, expected) in not_arns {
        for operation in [ASSOCIATE, LIST] {
            let mut body = json!({"integrationArn": value});
            if operation == ASSOCIATE {
                body["dataSource"] = data_source("amazon_vpc", Some("vpc_flow_logs"));
            }
            let (code, message) = logs.refusal(operation, &body).await;
            assert_eq!(
                code, VALIDATION,
                "case {operation} with {case}: expected {VALIDATION}, got {code} {message}"
            );
            assert_eq!(
                message, expected,
                "case {operation} with {case}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // One past each published bound of `maxResults`' `@range(1..100)`.
    for max_results in [0, 101] {
        let code = logs
            .err(
                LIST,
                &json!({"integrationArn": integration_arn, "maxResults": max_results}),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case maxResults={max_results}: expected {VALIDATION}, got {code}"
        );
    }

    // A token this integration's walk never issued, and one issued for another integration.
    let other = arn_of("other-lake");
    associate(
        &logs,
        &Row {
            case: "an association of another integration",
            integration_arn: other.clone(),
            name: "cloudtrail".to_string(),
            source_type: Some("cloudtrail"),
        },
    )
    .await;
    let other_page = logs
        .ok(LIST, &json!({"integrationArn": other, "maxResults": 1}))
        .await;
    let foreign = member(&other_page, "nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut tokens: Vec<(&str, String)> = vec![("a token nothing issued", "not-a-token".to_string())];
    if let Some(foreign) = foreign {
        tokens.push(("a token issued for another integration", foreign));
    }
    for (case, token) in tokens {
        let (code, message) = logs
            .refusal(
                LIST,
                &json!({"integrationArn": integration_arn, "nextToken": token}),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case}: expected {VALIDATION}, got {code} {message}"
        );
    }

    // An identifier no association is stored under.
    for (case, identifier) in [
        ("an identifier in no form this service mints", "association-1"),
        (
            "an identifier of the form it mints that no associate handed out",
            "00000000-0000-4000-8000-ffffffffffff",
        ),
    ] {
        let (code, message) = logs
            .refusal(DISASSOCIATE, &json!({"identifier": identifier}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: expected {NOT_FOUND}, got {code} {message}"
        );
        assert_eq!(
            message,
            not_found_message(identifier),
            "case {case}: expected the message {:?}, got {message:?}",
            not_found_message(identifier)
        );
    }

    // Nothing above disturbed the one association the account holds.
    let listed = sources(&logs, &integration_arn, None).await;
    let identifiers: Vec<String> = listed.iter().map(identifier_of).collect();
    assert_eq!(
        identifiers,
        vec![stored.clone()],
        "case after every refusal: expected the one association {stored}, got {identifiers:?}"
    );
    let status = listed
        .first()
        .and_then(|source| member(source, "status"))
        .and_then(Value::as_str);
    assert_eq!(
        status,
        Some(S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE),
        "case after every refusal: expected the status \
         {S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE}, got {status:?}"
    );
}

/// The store holds only so many associations: an associate that would take it past the cap
/// is refused and retains nothing, and room freed by a deletion the clock has finished is
/// room an associate can use again.
#[tokio::test]
async fn an_associate_past_the_store_cap_retains_nothing() {
    let logs = Logs::new();
    seed_integrations(&logs);
    let integration_arn = table_arn();

    // Fill the store to its cap. Nothing a client sends chooses how many associations the
    // store already holds, so the rows that are not the subject of this case are seeded
    // here and the cap itself is asserted over the wire.
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for _ in 0..MAX_S3_TABLE_INTEGRATION_SOURCES {
            let seq = store.take_s3_table_integration_source_seq();
            let identifier = mint_s3_table_integration_source_identifier(seq);
            store.s3_table_integration_sources.insert(
                identifier.clone(),
                S3TableIntegrationSourceRecord::new(
                    &identifier,
                    &integration_arn,
                    &DataSource {
                        name: format!("aws_seeded_{seq}"),
                        r#type: Some("vended_logs".to_string()),
                    },
                    now_ms(),
                ),
            );
        }
    }

    let body = json!({
        "integrationArn": integration_arn,
        "dataSource": data_source("one_too_many", Some("vended_logs")),
    });
    let (code, message) = logs.refusal(ASSOCIATE, &body).await;
    assert_eq!(
        code, THROTTLING,
        "case one past the cap: expected {THROTTLING}, got {code} {message}"
    );
    assert!(
        message.contains(&MAX_S3_TABLE_INTEGRATION_SOURCES.to_string()),
        "case one past the cap: expected a message naming the cap \
         {MAX_S3_TABLE_INTEGRATION_SOURCES}, got {message:?}"
    );

    let held = {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        store.s3_table_integration_sources.len()
    };
    assert_eq!(
        held, MAX_S3_TABLE_INTEGRATION_SOURCES,
        "case one past the cap: expected the store to still hold \
         {MAX_S3_TABLE_INTEGRATION_SOURCES} association(s), got {held}"
    );

    // One association deleted — by the clock, after a disassociate — is room for one more.
    let first = {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let store = state.lock();
        store
            .s3_table_integration_sources
            .keys()
            .next()
            .cloned()
            .expect("the seeded store holds an association")
    };
    logs.ok(DISASSOCIATE, &json!({"identifier": first})).await;
    pin_removal_due(&logs, &first, now_ms() - 1);
    let accepted = logs.ok(ASSOCIATE, &body).await;
    assert!(
        member(&accepted, "identifier")
            .and_then(Value::as_str)
            .is_some_and(|identifier| !identifier.is_empty()),
        "case the cap after a deletion the clock finished: expected an identifier, got {accepted}"
    );
}
