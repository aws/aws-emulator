// SPDX-License-Identifier: CC0-1.0

//! What `GetResources` ACCEPTS in `ResourceTypeFilters`, and the wording of the
//! request-level refusals that guard it.
//!
//! The rules under test:
//!
//! * a `ResourceTypeFilters` entry is `service[:resourceType]` in the spelling
//!   the service embeds in its ARNs, so an entry whose service token is not a
//!   supported service — a display spelling, an upper-case one, a misspelling, a
//!   token carrying stray whitespace — is refused HTTP 400
//!   `InvalidParameterException` with "Unsupported service=<token> in
//!   ResourceTypeFilter - <entry>", while an entry naming a supported service is
//!   served, whether or not anything of that type is tagged;
//! * every list member of the request selects a SET, so a list naming one value
//!   twice is refused with "<value> is a duplicate, please input unique
//!   resources.";
//! * the resource-type half is NOT validated: it selects within a supported
//!   service, so a type that names nothing is an empty page rather than an error;
//! * `ResourceARNList` combined with any filtering or pagination member is
//!   refused with the service's one fixed sentence, which names no member;
//! * a `TagFilters` element carrying no `Key` is refused by the field it left out.
//!
//! The tables span what each rule generalises over: which spelling mistake the
//! token makes, where in the array the bad entry sits, which supported service
//! the entry names and whether it is bare or qualified, and which member
//! `ResourceARNList` was combined with. The `Lambda:Function` case the issue
//! cited is one row, and the catalog is also exercised on `auditmanager` — the
//! entry the member's own documentation gives as its worked example.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization`
    // header below is present because it is what routes an awsJson request to
    // the `tagging` signing name, not to be verified.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation of this API and return its HTTP status and parsed body.
async fn tagging(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/tagging/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The error code an awsJson error body carries, under either spelling of the
/// member.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .or(body["code"].as_str())
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message an awsJson error body carries, under either spelling.
fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or(body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The ARNs of a `GetResources` answer, in the order the service returned them.
fn page_arns(body: &Value) -> Vec<String> {
    body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .map(|mapping| {
            mapping["ResourceARN"]
                .as_str()
                .unwrap_or_else(|| panic!("a mapping with no ResourceARN: {mapping}"))
                .to_string()
        })
        .collect()
}

/// Tag `arn`, asserting the write landed with no failed resource.
async fn tag(router: &MultiServiceRouter, arn: &str, key: &str, value: &str) {
    let (status, body) = tagging(
        router,
        "TagResources",
        json!({ "ResourceARNList": [arn], "Tags": { key: value } }),
    )
    .await;
    assert_eq!(status, 200, "TagResources({arn}): body {body}");
    assert_eq!(
        body["FailedResourcesMap"],
        json!({}),
        "TagResources({arn}): expected an empty FailedResourcesMap, got {body}"
    );
}

/// Rows of the refusal table: the `ResourceTypeFilters` array as sent, the
/// service token the refusal must name, and the whole entry it must echo.
///
/// The spellings cover a display-cased service with a type (the cited case), a
/// bare upper-case service, an all-caps qualified one, a misspelling of a real
/// service, a token carrying stray whitespace, and a bad entry sitting AFTER a
/// good one so every entry is seen to be checked.
const UNSUPPORTED: &[(&[&str], &str, &str)] = &[
    (&["Lambda:Function"], "Lambda", "Lambda:Function"),
    (&["EC2"], "EC2", "EC2"),
    (&["LAMBDA:FUNCTION"], "LAMBDA", "LAMBDA:FUNCTION"),
    (&["lambdas:function"], "lambdas", "lambdas:function"),
    (&["Sqs"], "Sqs", "Sqs"),
    (&[" lambda"], " lambda", " lambda"),
    (&["lambda:function", "Glue:Table"], "Glue", "Glue:Table"),
    (
        &["s3:bucket", "dynamodb:table", "DynamoDB:Table"],
        "DynamoDB",
        "DynamoDB:Table",
    ),
];

#[tokio::test]
async fn an_unsupported_service_token_in_a_resource_type_filter_is_refused() {
    let router = emulator();
    for (filters, service, entry) in UNSUPPORTED {
        let case = format!("ResourceTypeFilters={filters:?}");
        let (status, body) = tagging(
            &router,
            "GetResources",
            json!({ "ResourceTypeFilters": filters }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Unsupported service={service} in ResourceTypeFilter - {entry}"),
            "case {case}: expected the unsupported-service sentence for {entry}, got {body}"
        );
    }
}

/// Rows of the accepted table: a `ResourceTypeFilters` entry naming a supported
/// service, and the ARNs it must select out of the four tagged below.
///
/// They cover a bare service token (which selects every resource of that
/// service), the qualified form of the same one, qualified entries of two more
/// services whose ARNs spell their resource type the other two ways (`type/id`
/// and `type:id`), the documentation's own `auditmanager` example, a supported
/// service nothing is tagged under, and a supported service with a resource type
/// nothing answers to.
type AcceptedRow = (&'static [&'static str], &'static [usize]);

/// The four tagged resources every accepted row is matched against. Each ARN is
/// in a namespace no owning service crate resolves, so this API's own store is
/// their tag home and no second service has to be standing; between them they
/// spell their resource type the three ways an ARN can (`type/id`, `type:id`,
/// and the bare id whose type `crate::state::implicit_resource_type` names).
const TAGGED: &[&str] = &[
    "arn:aws:auditmanager:us-east-1:ACCOUNT:assessment/spelling-assessment",
    "arn:aws:dynamodb:us-east-1:ACCOUNT:table/spelling-table",
    "arn:aws:rds:us-east-1:ACCOUNT:db:spelling-db",
    "arn:aws:s3:::spelling-bucket",
];

const ACCEPTED: &[AcceptedRow] = &[
    // A bare service token selects every resource of that service.
    (&["s3"], &[3]),
    // The qualified form of the same resource, whose type comes from the table
    // of implicit resource types rather than from the ARN.
    (&["s3:bucket"], &[3]),
    // An ARN whose resource field carries `type:id`.
    (&["rds:db"], &[2]),
    // An ARN whose resource field carries `type/id`.
    (&["dynamodb:table"], &[1]),
    // The worked example from the member's own documentation.
    (&["auditmanager"], &[0]),
    (&["auditmanager:assessment"], &[0]),
    // Several entries select the union of what each selects.
    (&["rds:db", "s3"], &[2, 3]),
    // A supported service nothing is tagged under: an empty answer, not an error.
    (&["ec2:instance"], &[]),
    (&["kms"], &[]),
    // A supported service with a resource type nothing answers to — including a
    // mis-cased type, which is NOT a request-level fault because only the
    // service half is validated.
    (&["s3:Bucket"], &[]),
    (&["s3:accesspoint"], &[]),
];

#[tokio::test]
async fn a_supported_service_token_is_served_and_narrows_the_answer() {
    let router = emulator();
    let tagged: Vec<String> = TAGGED
        .iter()
        .map(|arn| arn.replace("ACCOUNT", ACCOUNT_ID))
        .collect();
    for arn in &tagged {
        tag(&router, arn, "SpellingProbe", "sized").await;
    }

    for (filters, selected) in ACCEPTED {
        let case = format!("ResourceTypeFilters={filters:?}");
        let (status, body) = tagging(
            &router,
            "GetResources",
            json!({ "ResourceTypeFilters": filters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        let expected: Vec<String> = selected.iter().map(|at| tagged[*at].clone()).collect();
        assert_eq!(
            page_arns(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
    }
}

/// Rows of the duplicate table: the member the repeated value arrived in, the
/// list as sent, and the value the refusal must name.
///
/// They cover each list member of this request, a value repeated immediately and
/// one repeated after an intervening entry, a bare service token and a qualified
/// one, and — as the control — two entries where one SUBSUMES the other without
/// being equal to it, which is not a duplicate.
#[tokio::test]
async fn a_list_member_naming_one_value_twice_is_refused() {
    let router = emulator();
    let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:duplicate-db");
    let other = format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:other-db");
    let rows: &[(&str, Value, Option<&str>)] = &[
        (
            "ResourceTypeFilters",
            json!(["lambda:function", "lambda:function"]),
            Some("lambda:function"),
        ),
        ("ResourceTypeFilters", json!(["s3", "ec2", "s3"]), Some("s3")),
        ("ResourceARNList", json!([arn, arn]), Some(arn.as_str())),
        (
            "ResourceARNList",
            json!([arn, other, arn]),
            Some(arn.as_str()),
        ),
        // Distinct entries, including one that selects a superset of another.
        (
            "ResourceTypeFilters",
            json!(["lambda", "lambda:function"]),
            None,
        ),
        ("ResourceARNList", json!([arn, other]), None),
    ];
    for (member, values, duplicate) in rows {
        let case = format!("{member}={values}");
        let mut request = json!({});
        request[*member] = values.clone();
        let (status, body) = tagging(&router, "GetResources", request).await;
        match duplicate {
            None => assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            ),
            Some(value) => {
                assert_eq!(
                    status, 400,
                    "case {case}: expected HTTP 400, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterException",
                    "case {case}: got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("{value} is a duplicate, please input unique resources."),
                    "case {case}: expected the duplicate sentence for {value}, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn resource_arn_list_with_a_filter_or_a_pagination_member_is_refused_by_one_sentence() {
    let router = emulator();
    let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:exclusive-db");
    let rows: &[(&str, Value)] = &[
        ("TagFilters", json!({ "Key": "Env", "Values": ["prod"] })),
        ("ResourceTypeFilters", json!("lambda:function")),
        ("ResourcesPerPage", json!(1)),
        ("TagsPerPage", json!(100)),
        ("PaginationToken", json!("ZGVhZGJlZWY=")),
    ];
    for (member, value) in rows {
        let mut request = json!({ "ResourceARNList": [arn] });
        // `TagFilters` and `ResourceTypeFilters` are lists; the two page sizes
        // and the token are scalars.
        request[*member] = match *member {
            "TagFilters" | "ResourceTypeFilters" => json!([value]),
            _ => value.clone(),
        };
        let (status, body) = tagging(&router, "GetResources", request).await;
        assert_eq!(
            status, 400,
            "case ResourceARNList+{member}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case ResourceARNList+{member}: got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Filters is not allowed when providing a resource list.",
            "case ResourceARNList+{member}: expected the one fixed sentence, got {body}"
        );
    }
}

#[tokio::test]
async fn a_tag_filter_element_without_a_key_is_refused_by_the_field_it_left_out() {
    let router = emulator();
    let rows: &[(&str, Value)] = &[
        ("an only element with no Key", json!([{ "Values": ["prod"] }])),
        ("an empty element", json!([{}])),
        (
            "a second element with no Key",
            json!([{ "Key": "Env" }, { "Values": ["prod"] }]),
        ),
    ];
    for (case, filters) in rows {
        let (status, body) = tagging(&router, "GetResources", json!({ "TagFilters": filters })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case}: got {body}"
        );
        assert_eq!(
            error_message(&body),
            "An element of TagFilters is missing required field Key",
            "case {case}: expected the missing-field sentence, got {body}"
        );
    }
}

/// The token a paginated answer hands out is OPAQUE: one unstructured run of
/// base64 characters, carrying neither the ARN it resumes after nor the instant
/// it was minted in clear — and it still resumes exactly that scan.
#[tokio::test]
async fn the_pagination_token_is_an_opaque_run_of_characters() {
    let router = emulator();
    let arns = [
        format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:opaque-db-a"),
        format!("arn:aws:rds:{REGION}:{ACCOUNT_ID}:db:opaque-db-b"),
    ];
    for arn in &arns {
        tag(&router, arn, "OpaqueProbe", "sized").await;
    }

    let (status, first) = tagging(
        &router,
        "GetResources",
        json!({ "TagFilters": [{ "Key": "OpaqueProbe" }], "ResourcesPerPage": 1 }),
    )
    .await;
    assert_eq!(status, 200, "case the first page: body {first}");
    assert_eq!(
        page_arns(&first),
        vec![arns[0].clone()],
        "case the first page: expected the first ARN alone, got {first}"
    );
    let token = first["PaginationToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case the first page: a page filled to the cap must carry a token, got {first}"
    );
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
        "case the token's characters: expected base64 only, got {token:?}"
    );
    assert!(
        !token.contains("arn:") && !token.contains(ACCOUNT_ID),
        "case the token's characters: expected no resource ARN in clear, got {token:?}"
    );
    assert!(
        token.len() <= 2048,
        "case the token's length: expected at most the modelled 2048 characters, got {}",
        token.len()
    );

    let (status, second) = tagging(
        &router,
        "GetResources",
        json!({
            "TagFilters": [{ "Key": "OpaqueProbe" }],
            "ResourcesPerPage": 1,
            "PaginationToken": token,
        }),
    )
    .await;
    assert_eq!(status, 200, "case the resumed page: body {second}");
    assert_eq!(
        page_arns(&second),
        vec![arns[1].clone()],
        "case the resumed page: expected the second ARN alone, got {second}"
    );
}
