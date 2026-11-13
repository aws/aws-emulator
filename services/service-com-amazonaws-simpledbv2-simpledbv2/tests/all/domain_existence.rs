// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DOMAIN PRECONDITION of the SimpleDBv2 export
//! operations.
//!
//! The rule under test: an export operation that names a SimpleDB domain resolves that
//! name against the account's domain registry before it mints or enumerates anything,
//! and a name the registry does not hold is the modelled `NoSuchDomainException`
//! (client error, HTTP 400, `x-amzn-errortype: NoSuchDomainException`, the sentence
//! this service states for a resource that does not exist) rather than a success. That
//! covers
//! `StartDomainExport`'s required `domainName` and `ListExports`' optional `domainName`
//! filter; `GetExport` names no domain (it is keyed by `exportArn`) and models only
//! `NoSuchExportException`, so it is not part of the rule.
//!
//! The registry holds the domains the operator declared (`SIMPLEDB_EXISTING_DOMAINS`,
//! reached in-process by `create_domain`) plus the domain of every stored export,
//! because no operation of this service creates a SimpleDB domain — that is the
//! SimpleDB 2009-04-15 `CreateDomain`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_simpledbv2_simpledbv2::state::refusal_message::{
    no_such_resource, DOMAIN_RESOURCE,
};
use service_com_amazonaws_simpledbv2_simpledbv2::state::{
    parse_existing_domains, ComAmazonawsSimpledbv2Simpledbv2State, StoredExport, IN_PROGRESS_SPAN,
    MAX_EXPORT_ARN_LEN, STATUS_PENDING, SUCCEEDED_SPAN,
};

use crate::domain_export_lifecycle::{
    call, call_reporting_error_type, emulator, emulator_over, emulator_with_domains, error_code,
    member, ok_call, summary_arns, ACCOUNT, REGION,
};

/// The bucket every start below exports to — a name the modelled `S3BucketName`
/// pattern `^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$` admits, so no row is refused on it.
const BUCKET: &str = "domain-existence-bucket";

/// The domain names this rule is asserted over, one row per FORM of name.
///
/// `DomainName` carries only `@length(min: 1)` and is documented as "the domain name
/// that uniquely identifies a SimpleDB domain within your account" (source:
/// `smithy_query(query="shape",shape="DomainName")`), so the rows differ along size,
/// case, punctuation and script rather than along a published alphabet. Each is a name
/// the minted `exportArn` can carry, so what decides every case below is the domain
/// precondition and nothing else.
fn domain_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "a one-character name, the modelled minimum length",
            "d".to_string(),
        ),
        ("a plain lower-case name", "orders".to_string()),
        (
            "mixed case with the punctuation a name may carry",
            "Mixed-Case.Domain_1".to_string(),
        ),
        ("a name with a space in it", "quarterly reports".to_string()),
        ("a non-ASCII name", "日本語ドメイン".to_string()),
        (
            "a long name well inside the exportArn's length bound",
            "l".repeat(MAX_EXPORT_ARN_LEN - 150),
        ),
    ]
}

/// The error shape the `x-amzn-errortype` header names.
///
/// Smithy allows that header to carry the shape name alone, a namespaced shape id, or
/// the shape name followed by `:` and a URI (which is the form AWS answers with), so
/// the SHAPE NAME is what a case compares.
fn header_error_shape(header: Option<&str>) -> String {
    header
        .unwrap_or("<no x-amzn-errortype header>")
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `(status, header, body)` is the modelled `NoSuchDomainException`.
fn assert_no_such_domain(case: &str, answered: (u16, Option<String>, Value)) {
    let (status, error_type, body) = answered;
    let documented = no_such_resource(DOMAIN_RESOURCE);
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        header_error_shape(error_type.as_deref()),
        "NoSuchDomainException",
        "case {case}: expected x-amzn-errortype to name NoSuchDomainException, got \
         {error_type:?}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchDomainException",
        "case {case}: expected the NoSuchDomainException shape, got {body:?}"
    );
    assert_eq!(
        member(&body, "message").as_deref(),
        Some(documented.as_str()),
        "case {case}: expected the sentence the service states for a domain that does not \
         exist ({documented:?}), got {body:?}"
    );
}

/// A domain the account does not hold is refused by BOTH operations that name one, for
/// every form of name, and nothing is stored for the refused start.
#[tokio::test]
async fn a_domain_the_account_does_not_hold_is_refused() {
    // An account that holds no domain at all: nothing was declared and nothing was
    // exported, which is what a fresh emulator comes up as.
    let router = emulator();

    for (case, domain) in domain_rows() {
        assert_no_such_domain(
            &format!("StartDomainExport naming {case}"),
            call_reporting_error_type(
                &router,
                "StartDomainExport",
                &json!({ "domainName": domain, "s3Bucket": BUCKET }),
            )
            .await,
        );
        assert_no_such_domain(
            &format!("ListExports filtered on {case}"),
            call_reporting_error_type(&router, "ListExports", &json!({ "domainName": domain }))
                .await,
        );
    }

    // A refused start stores nothing, so the unfiltered enumeration is still empty.
    let listed = ok_call(&router, "ListExports", &json!({})).await;
    assert_eq!(
        summary_arns(&listed),
        Vec::<String>::new(),
        "case after every refusal: a refused start must store no export, got {listed:?}"
    );
}

/// A domain the account DOES hold is exported, reported back by `GetExport`, and listed
/// under its own filter — for every form of name.
#[tokio::test]
async fn a_domain_the_account_holds_is_exported_and_listed() {
    for (case, domain) in domain_rows() {
        let router = emulator_with_domains(&[domain.as_str()]);

        let started = ok_call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": BUCKET }),
        )
        .await;
        let export_arn = member(&started, "exportArn")
            .unwrap_or_else(|| panic!("case {case}: no exportArn in {started:?}"));

        // The round trip: the stored record reports the very domain that was resolved.
        let fetched = ok_call(
            &router,
            "GetExport",
            &json!({ "exportArn": export_arn.clone() }),
        )
        .await;
        assert_eq!(
            member(&fetched, "domainName").as_deref(),
            Some(domain.as_str()),
            "case {case}: GetExport must report the exported domain, got {fetched:?}"
        );

        let listed = ok_call(&router, "ListExports", &json!({ "domainName": domain })).await;
        assert_eq!(
            summary_arns(&listed),
            vec![export_arn.clone()],
            "case {case}: the filter on a held domain must report its export, got {listed:?}"
        );
    }
}

/// Resolution is by the WHOLE name: a domain the account holds makes no neighbouring
/// name exist, so each near miss is still the modelled refusal while the declared name
/// itself is served.
#[tokio::test]
async fn only_the_declared_name_itself_resolves() {
    let router = emulator_with_domains(&["orders"]);

    let near_misses = [
        ("a prefix of the held name", "order"),
        ("the held name with a suffix", "orders2"),
        ("the held name capitalised", "Orders"),
        ("the held name upper-cased", "ORDERS"),
        ("the held name with leading space", " orders"),
        ("the held name with trailing space", "orders "),
        ("a different name entirely", "customers"),
    ];
    for (case, domain) in near_misses {
        assert_no_such_domain(
            &format!("StartDomainExport naming {case}"),
            call_reporting_error_type(
                &router,
                "StartDomainExport",
                &json!({ "domainName": domain, "s3Bucket": BUCKET }),
            )
            .await,
        );
        assert_no_such_domain(
            &format!("ListExports filtered on {case}"),
            call_reporting_error_type(&router, "ListExports", &json!({ "domainName": domain }))
                .await,
        );
    }

    let (status, body) = call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "orders", "s3Bucket": BUCKET }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the held name itself: expected HTTP 200, got {status} with {body:?}"
    );
}

/// `domainName` is OPTIONAL on `ListExports` ("if not provided, exports for all the
/// domains will be listed"), so a request that names no domain resolves none: it is a
/// page, never this error — even on an account that holds no domain at all.
#[tokio::test]
async fn an_unfiltered_list_resolves_no_domain() {
    let router = emulator();
    for (case, request) in [
        ("no members at all", json!({})),
        ("a page size but no filter", json!({ "maxResults": 1 })),
    ] {
        let listed = ok_call(&router, "ListExports", &request).await;
        assert_eq!(
            summary_arns(&listed),
            Vec::<String>::new(),
            "case {case}: an account with no domains lists no exports, got {listed:?}"
        );
        assert!(
            listed.get("nextToken").is_none(),
            "case {case}: an empty enumeration carries no nextToken, got {listed:?}"
        );
    }
}

/// A domain the account holds that no export names resolves, and answers the required
/// member as an EMPTY list: "has no exports" and "does not exist" are different answers.
#[tokio::test]
async fn a_held_domain_with_no_exports_lists_empty() {
    let router = emulator_with_domains(&["declared-but-unexported", "exported"]);
    let started = ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "exported", "s3Bucket": BUCKET }),
    )
    .await;
    let export_arn = member(&started, "exportArn").expect("the start minted an ARN");

    let listed = ok_call(
        &router,
        "ListExports",
        &json!({ "domainName": "declared-but-unexported" }),
    )
    .await;
    assert_eq!(
        summary_arns(&listed),
        Vec::<String>::new(),
        "case a held domain with no exports: expected an empty exportSummaries, got {listed:?}"
    );
    assert_eq!(
        summary_arns(&ok_call(&router, "ListExports", &json!({ "domainName": "exported" })).await),
        vec![export_arn],
        "case the domain that was exported: expected its own export back"
    );
}

/// A STORED EXPORT is evidence of its domain: an export could only have been started
/// for a domain that existed, so a record seeded straight into the store makes its
/// domain resolvable — a filter on it reports summaries instead of refusing, and a
/// further export of it is accepted.
#[tokio::test]
async fn a_stored_export_is_evidence_of_its_domain() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let seeded_arn =
        format!("arn:aws:sdb:{REGION}:{ACCOUNT}:domain/undeclared/export/seeded-export-id");
    {
        let state = engine.state::<ComAmazonawsSimpledbv2Simpledbv2State>();
        let mut store = state.lock();
        let now = SystemTime::now();
        let stored = store.insert_export(StoredExport {
            export_arn: seeded_arn.clone(),
            export_id: "seeded-export-id".to_string(),
            client_token: "seeded-token".to_string(),
            // A domain NOBODY declared: the record itself is what makes it exist.
            domain_name: "undeclared".to_string(),
            export_status: STATUS_PENDING.to_string(),
            requested_at: now,
            s3_bucket: BUCKET.to_string(),
            s3_key_prefix: None,
            s3_sse_algorithm: None,
            s3_sse_kms_key_id: None,
            s3_bucket_owner: None,
            export_manifest: None,
            items_count: None,
            export_data_cutoff_time: None,
            failure_code: None,
            failure_message: None,
            in_progress_due_at: now + IN_PROGRESS_SPAN,
            succeeded_due_at: now + SUCCEEDED_SPAN,
        });
        assert!(
            stored,
            "case seeding one record: an empty store must accept it"
        );
    }
    let router = emulator_over(engine);

    let listed = ok_call(
        &router,
        "ListExports",
        &json!({ "domainName": "undeclared" }),
    )
    .await;
    assert_eq!(
        summary_arns(&listed),
        vec![seeded_arn],
        "case a filter on the domain of a stored export: expected that export, got {listed:?}"
    );

    let (status, body) = call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "undeclared", "s3Bucket": BUCKET }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a further export of the domain of a stored export: expected HTTP 200, got \
         {status} with {body:?}"
    );
}

/// The operator's declaration admits exactly the names this service could export, and
/// skips anything else rather than guessing at it.
#[tokio::test]
async fn the_operator_declaration_admits_only_exportable_names() {
    // A name whose export ARN would be longer than the modelled `ExportArn` bound
    // (`@length(20, 2048)`), which is one past what `domain_name_rejection` admits.
    let past_the_arn_bound = "x".repeat(MAX_EXPORT_ARN_LEN + 1);
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "two names",
            "orders,customers".to_string(),
            vec!["orders", "customers"],
        ),
        (
            "surrounding space",
            "  orders ,\tcustomers  ".to_string(),
            vec!["orders", "customers"],
        ),
        (
            "an empty entry between two names",
            "orders,,customers".to_string(),
            vec!["orders", "customers"],
        ),
        ("nothing at all", String::new(), vec![]),
        (
            "a name carrying the ARN's path separator",
            "two/segments".to_string(),
            vec![],
        ),
        (
            "a name carrying the ARN's field separator",
            "two:fields".to_string(),
            vec![],
        ),
        (
            "a name carrying a control character",
            "line\nfeed".to_string(),
            vec![],
        ),
        (
            "a name past the exportArn's length bound",
            past_the_arn_bound.clone(),
            vec![],
        ),
        (
            "one good name beside one that is not exportable",
            format!("orders,{past_the_arn_bound}"),
            vec!["orders"],
        ),
    ];

    for (case, setting, expected) in rows {
        let parsed = parse_existing_domains(&setting);
        assert_eq!(
            parsed, expected,
            "case {case}: expected {expected:?}, got {parsed:?}"
        );
    }

    // And a declared name is a domain the export operations resolve: the declaration
    // and the in-process seam reach the same registry.
    let declared = parse_existing_domains("orders, customers");
    let names: Vec<&str> = declared.iter().map(String::as_str).collect();
    let router = emulator_with_domains(&names);
    for domain in &declared {
        let (status, body) = call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": BUCKET }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case the declared domain {domain}: expected HTTP 200, got {status} with {body:?}"
        );
    }
}
