// SPDX-License-Identifier: CC0-1.0

//! Cover for the ARN FIELD an ARN-addressed operation's refusal names, driven over the
//! wire the router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>`
//! on awsJson1_1.
//!
//! The rule under test: **an operation that addresses a resource by ARN reads the ARN's
//! fields in order (`arn:<partition>:<service>:<region>:<account>:<resource>`) and
//! refuses the FIRST field that is not well formed, naming that field and quoting its
//! value out of the request — so an ARN whose region field cannot be a Region name is
//! refused with `InvalidInputException` and `Invalid ARN: region <that field> is
//! invalid`, and the resource behind it is never examined.**
//!
//! The rows span the ways a region field fails to be a Region name — a bare word, an
//! uppercase one, one with no number, one with no hyphen, an empty group, an Availability
//! Zone, the empty field — against every operation of this service whose input is an ARN,
//! so one rule is asserted for all of them rather than for the one operation a recording
//! happened to exercise. The controls are the other side of it: a WELL-FORMED Region
//! field (including partitioned and recently added Regions this emulator serves nothing
//! in) is never refused on the region, and the refusals those calls do carry are the
//! resource-kind and not-found ones each operation already states.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// One router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDCODEBUILDARN/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The `__type` an error body carries.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The `message` a body carries, or the empty string when it carries none.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One operation of this service whose input is a resource ARN: how a request of it is
/// written, and the resource field an ARN of the kind it addresses carries.
struct ArnOperation {
    /// The operation's name, as `X-Amz-Target` names it.
    name: &'static str,
    /// The resource field of an ARN this operation addresses a resource by — the kind of
    /// resource it acts on, so the region field is the only part of the value at fault.
    resource: &'static str,
    /// A request of this operation carrying `arn`.
    request: fn(&str) -> Value,
}

/// Every operation of this service whose only resource identifier is an ARN, with a
/// request that is well formed apart from the ARN under test.
///
/// `BatchGetReportGroups` and the other batch reads that answer a value they hold nothing
/// for in their not-found half are NOT here: they state no ARN refusal at all, so no
/// field of their input is ever named.
fn arn_operations() -> Vec<ArnOperation> {
    vec![
        ArnOperation {
            name: "DeleteSourceCredentials",
            resource: "token/github",
            request: |arn| json!({"arn": arn}),
        },
        ArnOperation {
            name: "DeleteReportGroup",
            resource: "report-group/held-group",
            request: |arn| json!({"arn": arn}),
        },
        ArnOperation {
            name: "UpdateReportGroup",
            resource: "report-group/held-group",
            request: |arn| json!({"arn": arn}),
        },
        ArnOperation {
            name: "GetReportGroupTrend",
            resource: "report-group/held-group",
            // `trendField` is @required and targets `ReportGroupTrendFieldType`
            // (`smithy_query(query="shape", shape="ReportGroupTrendFieldType")`).
            request: |arn| json!({"reportGroupArn": arn, "trendField": "DURATION"}),
        },
        ArnOperation {
            name: "BatchGetReports",
            resource: "report/held-group:0abc1234",
            request: |arn| json!({"reportArns": [arn]}),
        },
        ArnOperation {
            name: "DeleteFleet",
            resource: "fleet/held-fleet",
            request: |arn| json!({"arn": arn}),
        },
        ArnOperation {
            name: "UpdateProjectVisibility",
            resource: "project/held-project",
            // `projectVisibility` is @required and targets `ProjectVisibilityType`.
            request: |arn| json!({"projectArn": arn, "projectVisibility": "PRIVATE"}),
        },
        ArnOperation {
            name: "GetResourcePolicy",
            resource: "project/held-project",
            request: |arn| json!({"resourceArn": arn}),
        },
        ArnOperation {
            name: "PutResourcePolicy",
            resource: "project/held-project",
            request: |arn| {
                json!({
                    "resourceArn": arn,
                    "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                })
            },
        },
        ArnOperation {
            name: "DeleteResourcePolicy",
            resource: "project/held-project",
            request: |arn| json!({"resourceArn": arn}),
        },
    ]
}

/// Region fields that are not a Region name, each with what is wrong with it.
///
/// The first row is the shape a client sends when it substituted nothing for the
/// placeholder in a documented ARN template; the rest vary the ways the form is missed.
fn malformed_regions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a bare word", "region"),
        ("the word Region in upper case", "REGION"),
        ("an area and a direction with no number", "us-west"),
        ("a Region name with no hyphen at all", "uswest2"),
        ("underscores instead of hyphens", "us_west_2"),
        ("an empty group", "us--west-2"),
        ("an Availability Zone rather than a Region", "us-west-2a"),
        ("a trailing hyphen", "us-west-"),
        ("a number carrying a letter", "eu-west-1x"),
        ("an area alone", "us"),
        ("no region field at all", ""),
    ]
}

/// Region fields that ARE Region names, from the AWS General Reference's Region table:
/// the signed Region, another commercial one, a partitioned one, one of the recently
/// added commercial Regions and a two-group area.
fn region_names() -> Vec<&'static str> {
    vec![
        REGION,
        "eu-west-1",
        "us-gov-west-1",
        "ap-southeast-4",
        "cn-north-1",
        "us-isob-east-1",
        "il-central-1",
    ]
}

/// An ARN of this service whose region field is `region`.
fn arn_in(region: &str, resource: &str) -> String {
    format!(
        "arn:aws:codebuild:{region}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The sentence AWS states for an ARN whose region field is not a Region: fixed text
/// around the offending field, copied out of the request.
fn expected_message(region: &str) -> String {
    format!("Invalid ARN: region {region} is invalid")
}

#[tokio::test]
async fn an_arn_whose_region_field_is_no_region_is_refused_on_that_field() {
    let router = emulator();

    for operation in arn_operations() {
        for (why, region) in malformed_regions() {
            let arn = arn_in(region, operation.resource);
            let case = format!("{} with {why} ({region:?})", operation.name);
            let (status, body) = call(&router, operation.name, (operation.request)(&arn)).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {:?}",
                error_type(&body)
            );
            assert_eq!(
                message(&body),
                expected_message(region),
                "case {case:?}: expected AWS's invalid-region sentence, got {:?}",
                message(&body)
            );
        }
    }
}

#[tokio::test]
async fn a_well_formed_region_field_is_never_the_field_refused() {
    let router = emulator();

    for operation in arn_operations() {
        for region in region_names() {
            let arn = arn_in(region, operation.resource);
            let case = format!("{} in {region}", operation.name);
            let (status, body) = call(&router, operation.name, (operation.request)(&arn)).await;

            assert!(
                !message(&body).starts_with("Invalid ARN: region"),
                "case {case:?}: a Region name must not be the field at fault, got {status} \
                 with {body}"
            );
            assert_ne!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: an ARN of the kind the operation addresses is well formed, \
                 so the answer is the resource's own, got {status} with {body}"
            );
        }
    }
}

/// The region field is read BEFORE the resource field: an ARN that misses both is
/// refused on the region, and the resource-kind sentences the operations state are
/// reached only once the region reads.
#[tokio::test]
async fn the_region_field_is_refused_before_the_resource_field() {
    let router = emulator();

    for operation in arn_operations() {
        // A resource field of a kind this operation does not address, so BOTH fields are
        // at fault at once.
        let wrong_resource = "not-a-resource-kind/held";
        let bad_region = "region";

        let (status, body) = call(
            &router,
            operation.name,
            (operation.request)(&arn_in(bad_region, wrong_resource)),
        )
        .await;
        assert_eq!(
            message(&body),
            expected_message(bad_region),
            "case {:?}: the region field is read first, got {status} with {body}",
            operation.name
        );

        // The same value with the region repaired is refused on the resource instead:
        // a different sentence, and not the region one.
        let (status, body) = call(
            &router,
            operation.name,
            (operation.request)(&arn_in(REGION, wrong_resource)),
        )
        .await;
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {:?}: a resource field of another kind is still invalid input, got \
             {status} with {body}",
            operation.name
        );
        assert_ne!(
            message(&body),
            expected_message(REGION),
            "case {:?}: a Region name is not the field at fault, got {status} with {body}",
            operation.name
        );
        assert!(
            !message(&body).starts_with("Invalid ARN: region"),
            "case {:?}: the refusal must name the resource field, got {status} with {body}",
            operation.name
        );
    }
}

/// A value that is not an ARN at all, or that has fewer than an ARN's six fields, has no
/// single field at fault: it keeps each operation's own invalid-input sentence rather
/// than being reported as a region problem.
#[tokio::test]
async fn a_value_that_is_no_arn_at_all_is_not_refused_on_a_field() {
    let router = emulator();

    for operation in arn_operations() {
        let not_arns = [
            ("not an ARN at all", "held-group".to_string()),
            ("the resource field alone", operation.resource.to_string()),
            (
                "an ARN missing the resource field",
                format!("arn:aws:codebuild:region:{}", engine::EMULATOR_ACCOUNT_ID),
            ),
            (
                "an ARN of another service",
                format!(
                    "arn:aws:codepipeline:region:{}:{}",
                    engine::EMULATOR_ACCOUNT_ID, operation.resource
                ),
            ),
            (
                "an ARN with an empty partition",
                format!(
                    "arn::codebuild:region:{}:{}",
                    engine::EMULATOR_ACCOUNT_ID, operation.resource
                ),
            ),
        ];

        for (why, value) in not_arns {
            let case = format!("{} with {why} ({value:?})", operation.name);
            let (status, body) = call(&router, operation.name, (operation.request)(&value)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {status} with {body}"
            );
            assert!(
                !message(&body).starts_with("Invalid ARN: region"),
                "case {case:?}: a value with no region field must not be refused on one, \
                 got {status} with {body}"
            );
        }
    }
}
