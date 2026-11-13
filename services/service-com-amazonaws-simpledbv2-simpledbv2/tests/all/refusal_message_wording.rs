// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the WORDING of every refusal this service answers.
//!
//! The `message` member is the only part of a client error that says WHICH parameter
//! or resource the service faulted, so its wording is part of the contract. Three
//! rules are asserted here, each over a TABLE of inputs rather than the one request a
//! recording happened to carry:
//!
//! 1. a refusal that faults ONE PARAMETER names it in the casing the API documentation
//!    uses and states the requirement — `Found invalid value for parameter
//!    <Parameter>. <Parameter> must <requirement>.` for a value the parameter does not
//!    admit, `Parameter <Parameter> must be provided only when <condition>.` for one
//!    supplied without the value it is only expected alongside — never the value that
//!    missed it;
//! 2. a refusal about a RESOURCE the account does not hold names the KIND of resource
//!    (`The specified <resource> does not exist.`), whichever identifier named it, and
//!    an error with neither a parameter nor a resource carries the sentence the service
//!    publishes for it (source for each: the `smithy_query` named beside it);
//! 3. a refusal the centralized constraint walk raises quotes a `@pattern` requirement
//!    as the expression a value has to satisfy, WITHOUT the `^` / `$` pair the model
//!    wraps it in to spell a whole-value match. Every other half of that envelope —
//!    its count, its member path, its echoed value and its other requirement clauses —
//!    is unchanged.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_simpledbv2_simpledbv2::state::refusal_message::api_parameter;
use service_com_amazonaws_simpledbv2_simpledbv2::state::{
    ComAmazonawsSimpledbv2Simpledbv2State, StoredExport, IN_PROGRESS_SPAN, MAX_EXPORTS,
    MAX_EXPORT_ARN_LEN, STATUS_PENDING, SUCCEEDED_SPAN,
};

use crate::domain_export_lifecycle::{
    call, emulator, emulator_over, emulator_with_domains, error_code, member, ok_call, ACCOUNT,
    REGION,
};

/// A bucket name the modelled `S3BucketName` pattern admits, so a case about something
/// else is never refused on its bucket.
const BUCKET: &str = "wording-bucket";

/// The sentence this service states for a parameter whose value it refused, rendered
/// HERE rather than taken from the crate under test: `Found invalid value for parameter
/// <Parameter>. <Parameter> must <requirement>.`
///
/// The recorded instance of this form is `GetExport`'s `exportArn` — "Found invalid
/// value for parameter ExportArn. ExportArn must be a valid AWS SimpleDB Domain Export
/// ARN." — and every row below is the same form with its own parameter and requirement.
fn invalid_parameter_sentence(parameter: &str, requirement: &str) -> String {
    format!("Found invalid value for parameter {parameter}. {parameter} must {requirement}.")
}

/// The requirement the minted `exportArn` imposes on the characters of a `domainName`.
const DOMAIN_CHARACTER_REQUIREMENT: &str =
    "be free of '/', ':' and ASCII control characters, each of which would make the \
     exportArn this service mints name a different export resource";

/// The requirement an `exportArn` has to meet, as the recording words it.
const EXPORT_ARN_REQUIREMENT: &str = "be a valid AWS SimpleDB Domain Export ARN";

/// The `message` a failed call reported.
fn message_of(body: &Value) -> String {
    member(body, "message")
        .unwrap_or_else(|| panic!("every modelled error of this service requires a message: {body:?}"))
}

/// Call `operation`, assert it answered `status_expected` with the `error` shape, and
/// return its `message`.
async fn refusal_message(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    error: &str,
    status_expected: u16,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, status_expected,
        "case {case}: expected HTTP {status_expected}, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        error,
        "case {case}: expected the {error} shape, got {body:?}"
    );
    message_of(&body)
}

/// RULE 1: every parameter-value refusal this service raises names the parameter as the
/// API documents it, states the requirement, and withholds the value that missed it.
///
/// The rows span both members a handler of this service faults — `GetExport`'s
/// `exportArn` (seven forms of value, from "no ARN at all" to an ARN whose export id is
/// empty) and `StartDomainExport`'s `domainName` (each character class the minted ARN
/// cannot carry, and a name one past the ARN's length bound).
#[tokio::test]
async fn a_parameter_value_refusal_names_the_parameter_and_withholds_the_value() {
    // Only this domain is held, so a row refused below is refused on its own value,
    // before any domain is resolved.
    let router = emulator_with_domains(&["held-domain"]);

    // Every `exportArn` row is at least 20 characters, so the modelled
    // `@length(20, 2048)` passes and the handler's own rule is what answers.
    let arn_rows: Vec<(&str, String)> = vec![
        ("no ARN prefix at all", "this-is-not-an-arn-at-all".to_string()),
        (
            "the recorded malformed value",
            "not-a-valid-simpledb-export-arn-at-all".to_string(),
        ),
        (
            "another service's namespace",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:export/abc123"),
        ),
        (
            "an empty partition field",
            format!("arn::sdb:{REGION}:{ACCOUNT}:export/abc123"),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:sdb:{REGION}:export/abc123xyz"),
        ),
        (
            "no export path element",
            format!("arn:aws:sdb:{REGION}:{ACCOUNT}:domain/held-domain"),
        ),
        (
            "a path element merely ending in 'export'",
            format!("arn:aws:sdb:{REGION}:{ACCOUNT}:notexport/abc123"),
        ),
        (
            "an empty export id",
            format!("arn:aws:sdb:{REGION}:{ACCOUNT}:domain/held-domain/export/"),
        ),
    ];

    let long_name = "d".repeat(MAX_EXPORT_ARN_LEN + 1);
    let length_requirement = format!(
        "be short enough for the export ARN naming it to stay within the \
         {MAX_EXPORT_ARN_LEN} characters an exportArn may carry"
    );
    let domain_rows: Vec<(&str, String, &str)> = vec![
        (
            "a name carrying the ARN's path separator",
            "two/segments".to_string(),
            DOMAIN_CHARACTER_REQUIREMENT,
        ),
        (
            "a name carrying the ARN's field separator",
            "two:fields".to_string(),
            DOMAIN_CHARACTER_REQUIREMENT,
        ),
        (
            "a name carrying a line feed",
            "lf\ninjected".to_string(),
            DOMAIN_CHARACTER_REQUIREMENT,
        ),
        (
            "a name carrying a NUL",
            "nul\0injected".to_string(),
            DOMAIN_CHARACTER_REQUIREMENT,
        ),
        (
            "a name one past the exportArn's length bound",
            long_name.clone(),
            length_requirement.as_str(),
        ),
    ];

    for (case, arn) in arn_rows {
        let stated = refusal_message(
            &router,
            case,
            "GetExport",
            &json!({ "exportArn": arn.clone() }),
            "InvalidParameterValueException",
            400,
        )
        .await;
        let expected = invalid_parameter_sentence("ExportArn", EXPORT_ARN_REQUIREMENT);
        assert_eq!(
            stated, expected,
            "case {case}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(arn.as_str()),
            "case {case}: the refusal must not echo the submitted value {arn:?}, got {stated:?}"
        );
    }

    for (case, domain, requirement) in domain_rows {
        let stated = refusal_message(
            &router,
            case,
            "StartDomainExport",
            &json!({ "domainName": domain.clone(), "s3Bucket": BUCKET }),
            "InvalidParameterValueException",
            400,
        )
        .await;
        let expected = invalid_parameter_sentence("DomainName", requirement);
        assert_eq!(
            stated, expected,
            "case {case}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(domain.as_str()),
            "case {case}: the refusal must not echo the submitted value, got {stated:?}"
        );
    }

    // The held domain itself is served, so nothing above was refused for being this
    // service's only answer.
    let started = ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "held-domain", "s3Bucket": BUCKET }),
    )
    .await;
    assert!(
        member(&started, "exportArn").is_some(),
        "case the held domain: a start of it must mint an exportArn, got {started:?}"
    );
}

/// RULE 1, the parameter NAME half: a parameter is named as the API documents it — the
/// modelled member name with its first character upper-cased — for every member of this
/// service's three inputs, not only the two a handler can fault.
///
/// Source of the member names: `smithy_query(query="operation",
/// operation="GetExport,ListExports,StartDomainExport")`.
#[test]
fn a_parameter_is_named_as_the_api_documents_it() {
    let rows = [
        ("exportArn", "ExportArn"),
        ("nextToken", "NextToken"),
        ("maxResults", "MaxResults"),
        ("domainName", "DomainName"),
        ("clientToken", "ClientToken"),
        ("s3Bucket", "S3Bucket"),
        ("s3BucketOwner", "S3BucketOwner"),
        ("s3KeyPrefix", "S3KeyPrefix"),
        ("s3SseAlgorithm", "S3SseAlgorithm"),
        ("s3SseKmsKeyId", "S3SseKmsKeyId"),
    ];
    for (member_name, documented) in rows {
        let stated = api_parameter(member_name);
        assert_eq!(
            stated, documented,
            "case {member_name}: expected the documented spelling {documented:?}, got {stated:?}"
        );
    }
}

/// RULE 2: a refusal about a resource the account does not hold names the KIND of
/// resource, and an error about neither a parameter nor a resource carries the sentence
/// the service publishes for it — whatever request reached it, and with nothing the
/// caller sent interpolated into it.
#[tokio::test]
async fn an_error_states_the_services_own_sentence() {
    // Source of the not-found sentences: the recorded refusals of this service, which
    // name the kind of resource (`GetExport` on an ARN naming nothing, and both
    // operations that resolve a domain).
    let no_such_export = "The specified export does not exist.";
    let no_such_domain = "The specified domain does not exist.";
    // Source: `smithy_query(query="shape",shape="InvalidNextTokenException")`.
    let invalid_next_token = "The specified next token is not valid.";
    // The recorded combination refusal: the parameter, named as the API documents it,
    // and the condition it may be provided under.
    let bad_combination =
        "Parameter S3SseKmsKeyId must be provided only when S3-SSE Algorithm is KMS.";

    let router = emulator_with_domains(&["documented"]);

    // `NoSuchExportException`: well-formed export ARNs that name nothing. Both resource
    // forms the model's own documents use, and a foreign partition, account and region.
    let unknown_arns = [
        (
            "the plain export/<id> form",
            "arn:aws:sdb:us-east-1:123456789012:export/abc123".to_string(),
        ),
        (
            "the domain/<name>/export/<id> form",
            "arn:aws:sdb:us-east-1:123456789012:domain/testDomain/export/\
             3eb4eaed-872b-4e08-b4b6-ff6999a83e01"
                .to_string(),
        ),
        (
            "this emulator's own region and account",
            format!("arn:aws:sdb:{REGION}:{ACCOUNT}:domain/documented/export/not-a-stored-id"),
        ),
        (
            "another partition",
            "arn:aws-cn:sdb:cn-north-1:000000000000:export/abc123".to_string(),
        ),
    ];
    for (case, arn) in unknown_arns {
        let stated = refusal_message(
            &router,
            case,
            "GetExport",
            &json!({ "exportArn": arn.clone() }),
            "NoSuchExportException",
            400,
        )
        .await;
        assert_eq!(
            stated, no_such_export,
            "case {case}: expected {no_such_export:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(arn.as_str()),
            "case {case}: the refusal must not echo the submitted ARN, got {stated:?}"
        );
    }

    // `InvalidNextTokenException`: tokens this service never issued, each malformed in
    // its own way, plus one it DID issue for a different enumeration.
    let forged_tokens = [
        ("text that is not hex at all", "not-a-pagination-token".to_string()),
        ("hex carrying no offset", hex::encode("no-dot-here")),
        ("hex naming an offset the store does not have", hex::encode("99.deadbeef")),
        ("hex naming the zero offset", hex::encode("0.deadbeef")),
        (
            "hex carrying a digest and an offset no store of this size has",
            hex::encode("1.00000000"),
        ),
    ];
    for (case, token) in forged_tokens {
        let stated = refusal_message(
            &router,
            case,
            "ListExports",
            &json!({ "nextToken": token.clone() }),
            "InvalidNextTokenException",
            400,
        )
        .await;
        assert_eq!(
            stated, invalid_next_token,
            "case {case}: expected {invalid_next_token:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(token.as_str()),
            "case {case}: the refusal must not echo the submitted token, got {stated:?}"
        );
    }

    // A token this service issued for the UNFILTERED enumeration, replayed under a
    // filter: the same sentence, because it continues a stream this request never
    // started. Two exports, so a page of one leaves a token.
    for seeded in ["the first export", "the second export"] {
        let (status, body) = call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": "documented", "s3Bucket": BUCKET }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case seeding {seeded}: expected HTTP 200, got {status} with {body:?}"
        );
    }
    let first_page = ok_call(&router, "ListExports", &json!({ "maxResults": 1 })).await;
    let issued = member(&first_page, "nextToken")
        .expect("two stored exports and a page size of one leave a nextToken");
    let stated = refusal_message(
        &router,
        "a token issued for the unfiltered enumeration, replayed under a filter",
        "ListExports",
        &json!({ "maxResults": 1, "nextToken": issued.clone(), "domainName": "documented" }),
        "InvalidNextTokenException",
        400,
    )
    .await;
    assert_eq!(
        stated, invalid_next_token,
        "case a token from another enumeration: expected {invalid_next_token:?}, got {stated:?}"
    );
    assert!(
        !stated.contains(issued.as_str()),
        "case a token from another enumeration: the refusal must not echo it, got {stated:?}"
    );

    // `NoSuchDomainException`: both operations that name a domain, for a name the
    // account does not hold.
    for (case, operation, request) in [
        (
            "StartDomainExport naming a domain nobody holds",
            "StartDomainExport",
            json!({ "domainName": "no-such-domain-here", "s3Bucket": BUCKET }),
        ),
        (
            "ListExports filtered on a domain nobody holds",
            "ListExports",
            json!({ "domainName": "no-such-domain-here" }),
        ),
    ] {
        let stated = refusal_message(
            &router,
            case,
            operation,
            &request,
            "NoSuchDomainException",
            400,
        )
        .await;
        assert_eq!(
            stated, no_such_domain,
            "case {case}: expected {no_such_domain:?}, got {stated:?}"
        );
        assert!(
            !stated.contains("no-such-domain-here"),
            "case {case}: the refusal must not echo the submitted name, got {stated:?}"
        );
    }

    // `InvalidParameterCombinationException`: `s3SseKmsKeyId` is only expected with the
    // KMS algorithm, so each other pairing is this error.
    for (case, request) in [
        (
            "a KMS key with SSE-S3",
            json!({
                "domainName": "documented",
                "s3Bucket": BUCKET,
                "s3SseAlgorithm": "AES256",
                "s3SseKmsKeyId": "alias/combination-key",
            }),
        ),
        (
            "a KMS key with no algorithm at all",
            json!({
                "domainName": "documented",
                "s3Bucket": BUCKET,
                "s3SseKmsKeyId": "alias/combination-key",
            }),
        ),
    ] {
        let stated = refusal_message(
            &router,
            case,
            "StartDomainExport",
            &request,
            "InvalidParameterCombinationException",
            400,
        )
        .await;
        assert_eq!(
            stated, bad_combination,
            "case {case}: expected {bad_combination:?}, got {stated:?}"
        );
        assert!(
            !stated.contains("alias/combination-key"),
            "case {case}: the refusal must not echo the submitted key, got {stated:?}"
        );
    }
}

/// RULE 2 for `ConflictException`: a `clientToken` reused inside its window with
/// changed parameters is refused with the sentence the service publishes for that
/// error, and the token the caller chose is not echoed back.
#[tokio::test]
async fn a_client_token_reused_with_changed_parameters_states_the_published_sentence() {
    // Source: `smithy_query(query="shape",shape="ConflictException")`.
    let conflict = "Indicates a conflict with one or more parameters of the request.";
    let router = emulator_with_domains(&["conflicting", "conflicting-other"]);
    let token = "caller-chosen-idempotency-token";

    let started = ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "conflicting", "s3Bucket": BUCKET, "clientToken": token }),
    )
    .await;
    assert_eq!(
        member(&started, "clientToken").as_deref(),
        Some(token),
        "case the first start: the token the caller chose is what comes back, got {started:?}"
    );

    // One changed parameter per row, so the rule holds for each member the repeat can
    // differ in rather than for one of them.
    let changed = [
        (
            "a different bucket",
            json!({ "domainName": "conflicting", "s3Bucket": "other-bucket", "clientToken": token }),
        ),
        (
            "a different domain",
            json!({ "domainName": "conflicting-other", "s3Bucket": BUCKET, "clientToken": token }),
        ),
        (
            "an added key prefix",
            json!({
                "domainName": "conflicting",
                "s3Bucket": BUCKET,
                "s3KeyPrefix": "exports/",
                "clientToken": token,
            }),
        ),
        (
            "an added encryption algorithm",
            json!({
                "domainName": "conflicting",
                "s3Bucket": BUCKET,
                "s3SseAlgorithm": "AES256",
                "clientToken": token,
            }),
        ),
    ];
    for (case, request) in changed {
        let stated = refusal_message(
            &router,
            case,
            "StartDomainExport",
            &request,
            "ConflictException",
            400,
        )
        .await;
        assert_eq!(
            stated, conflict,
            "case {case}: expected {conflict:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(token),
            "case {case}: the refusal must not echo the caller's token, got {stated:?}"
        );
    }
}

/// RULE 2 for `NumberExportsLimitExceeded`: a store at its ceiling refuses another
/// start with the sentence the service publishes for that error, which names no figure
/// — the service publishes none, and this emulator's ceiling is not that quota.
#[tokio::test]
async fn a_start_past_the_ceiling_states_the_published_sentence() {
    // Source: `smithy_query(query="shape",shape="NumberExportsLimitExceeded")` — no
    // final stop, as the model writes it.
    let over_quota = "Cannot start export as export quota limit was exceeded";

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsSimpledbv2Simpledbv2State>();
        let mut store = state.lock();
        store.create_domain("at-the-ceiling");
        let now = SystemTime::now();
        for index in 0..MAX_EXPORTS {
            let export_id = format!("seeded-{index}");
            let stored = store.insert_export(StoredExport {
                export_arn: format!(
                    "arn:aws:sdb:{REGION}:{ACCOUNT}:domain/seeded/export/{export_id}"
                ),
                export_id,
                client_token: format!("seeded-token-{index}"),
                domain_name: "seeded".to_string(),
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
                "case seeding record {index}: the store must accept every record up to its \
                 {MAX_EXPORTS} ceiling"
            );
        }
    }
    let router = emulator_over(engine);

    let stated = refusal_message(
        &router,
        "a store already holding its ceiling",
        "StartDomainExport",
        &json!({ "domainName": "at-the-ceiling", "s3Bucket": BUCKET }),
        "NumberExportsLimitExceeded",
        409,
    )
    .await;
    assert_eq!(
        stated, over_quota,
        "case a full store: expected {over_quota:?}, got {stated:?}"
    );
    assert!(
        !stated.contains(&MAX_EXPORTS.to_string()),
        "case a full store: the sentence must name no figure for the quota, got {stated:?}"
    );
}

/// The modelled `@pattern` of `S3BucketName`, as the model writes it — source:
/// `smithy_query(query="shape",shape="S3BucketName")`.
const BUCKET_PATTERN_AS_MODELLED: &str = r"^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$";

/// The modelled `@pattern` of `AwsAccountId`, which `s3BucketOwner` targets — source:
/// `smithy_query(query="shape",shape="AwsAccountId")`.
const ACCOUNT_PATTERN_AS_MODELLED: &str = r"^[0-9]{12}$";

/// The expression a refusal quotes for a model `@pattern`: the same expression without
/// the outer anchor pair the model wraps it in.
fn quoted_pattern(modelled: &str) -> &str {
    modelled
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        .unwrap_or(modelled)
}

/// The clause the constraint envelope carries for a value outside its member's
/// `@pattern`.
fn pattern_clause(value: &str, member_path: &str, modelled: &str) -> String {
    format!(
        "Value '{value}' at '{member_path}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {}",
        quoted_pattern(modelled)
    )
}

/// RULE 3: a `@pattern` requirement is quoted without the anchor pair the model wraps
/// it in, for every patterned member of this service's inputs and every form of
/// offending value — and the rest of the envelope is untouched.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_anchors() {
    // No domain is held: a constraint refusal is raised before any handler runs, so
    // every row below is answered by the walk rather than by the domain precondition.
    let router = emulator();

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the recorded bucket name",
            json!({ "domainName": "any-domain", "s3Bucket": "Invalid_Bucket_Name!" }),
            pattern_clause(
                "Invalid_Bucket_Name!",
                "s3Bucket",
                BUCKET_PATTERN_AS_MODELLED,
            ),
        ),
        (
            "a bucket name ending in the separator",
            json!({ "domainName": "any-domain", "s3Bucket": "bucket-" }),
            pattern_clause("bucket-", "s3Bucket", BUCKET_PATTERN_AS_MODELLED),
        ),
        (
            "a bucket name starting with a dot",
            json!({ "domainName": "any-domain", "s3Bucket": ".bucket" }),
            pattern_clause(".bucket", "s3Bucket", BUCKET_PATTERN_AS_MODELLED),
        ),
        (
            "a bucket name carrying a space",
            json!({ "domainName": "any-domain", "s3Bucket": "my bucket" }),
            pattern_clause("my bucket", "s3Bucket", BUCKET_PATTERN_AS_MODELLED),
        ),
        (
            "an account id of the wrong length",
            json!({ "domainName": "any-domain", "s3Bucket": BUCKET, "s3BucketOwner": "12345" }),
            pattern_clause("12345", "s3BucketOwner", ACCOUNT_PATTERN_AS_MODELLED),
        ),
        (
            "an account id that is not digits at all",
            json!({
                "domainName": "any-domain",
                "s3Bucket": BUCKET,
                "s3BucketOwner": "not-an-acct1",
            }),
            pattern_clause("not-an-acct1", "s3BucketOwner", ACCOUNT_PATTERN_AS_MODELLED),
        ),
    ];

    for (case, request, clause) in rows {
        let stated = refusal_message(
            &router,
            case,
            "StartDomainExport",
            &request,
            "ValidationException",
            400,
        )
        .await;
        let expected = format!("1 validation error detected: {clause}");
        assert_eq!(
            stated, expected,
            "case {case}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains('^') && !stated.contains('$'),
            "case {case}: the quoted expression must carry no anchor, got {stated:?}"
        );
    }

    // Two patterned members broken by one request: the count and both clauses are the
    // envelope's own, and each expression is quoted the same way.
    let both = refusal_message(
        &router,
        "a bad bucket name and a bad account id together",
        "StartDomainExport",
        &json!({ "domainName": "any-domain", "s3Bucket": "my bucket", "s3BucketOwner": "12345" }),
        "ValidationException",
        400,
    )
    .await;
    assert!(
        both.starts_with("2 validation errors detected: "),
        "case two broken patterns: expected the two-error envelope, got {both:?}"
    );
    for clause in [
        pattern_clause("my bucket", "s3Bucket", BUCKET_PATTERN_AS_MODELLED),
        pattern_clause("12345", "s3BucketOwner", ACCOUNT_PATTERN_AS_MODELLED),
    ] {
        assert!(
            both.contains(&clause),
            "case two broken patterns: expected the clause {clause:?}, got {both:?}"
        );
    }
    assert!(
        !both.contains('^') && !both.contains('$'),
        "case two broken patterns: no quoted expression may carry an anchor, got {both:?}"
    );

    // A requirement that is not a `@pattern` keeps the framework's own clause, so what
    // changed is the pattern rendering and nothing else: a bucket name below
    // `S3BucketName`'s `@length` minimum, and an absent `@required` member.
    let too_short = refusal_message(
        &router,
        "a bucket name below the modelled minimum length",
        "StartDomainExport",
        &json!({ "domainName": "any-domain", "s3Bucket": "ab" }),
        "ValidationException",
        400,
    )
    .await;
    assert_eq!(
        too_short,
        "1 validation error detected: Value 'ab' at 's3Bucket' failed to satisfy constraint: \
         Member must have length greater than or equal to 3",
        "case a bucket name below the minimum: expected the framework's length clause, got \
         {too_short:?}"
    );

    let absent = refusal_message(
        &router,
        "no s3Bucket at all",
        "StartDomainExport",
        &json!({ "domainName": "any-domain" }),
        "ValidationException",
        400,
    )
    .await;
    assert_eq!(
        absent,
        "1 validation error detected: Value null at 's3Bucket' failed to satisfy constraint: \
         Member must not be null",
        "case an absent required member: expected the framework's clause, got {absent:?}"
    );

    // A bucket name the pattern DOES admit is not refused on it: the check still runs
    // the model's expression as written, anchors included.
    let router = emulator_with_domains(&["pattern-ok"]);
    for admitted in ["abc", "my.bucket-1_x", BUCKET] {
        let started = ok_call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": "pattern-ok", "s3Bucket": admitted }),
        )
        .await;
        assert!(
            member(&started, "exportArn").is_some(),
            "case the admitted bucket name {admitted:?}: expected a minted exportArn, got \
             {started:?}"
        );
    }
}
