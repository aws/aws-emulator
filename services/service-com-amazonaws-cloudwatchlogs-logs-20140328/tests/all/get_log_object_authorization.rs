// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH refusal `GetLogObject` answers a caller holding no permissions,
//! and why that is not the authorization refusal every other operation of this service answers.
//!
//! The rule under test: **a request whose target this service cannot resolve at all is refused
//! for THAT, before the caller's identity policies are consulted — because there is no resource
//! for an identity decision to be about.** `GetLogObject` is the one operation of this service
//! in that position: it carries no `logGroupName` and no `logGroupIdentifier`, so the only
//! thing naming the resource it reaches is the `logObjectPointer` the service itself issued,
//! and a pointer this service did not issue decodes to nothing. Every operation whose request
//! DOES name its resource is still decided against the caller's policies first, which is what
//! the second test pins.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine (the
//! harness `crate::delivery_identity_authorization` assembles), so the documents this service
//! reads are the ones those services wrote, and every case drives the service the way a client
//! does — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`.
//!
//! The rows span what the rule generalises over: the pointer forms a caller can present (one it
//! built itself, one this service mints for another purpose, one at the modelled length bounds)
//! and the policies the caller may hold (none at all, an explicit deny of the action, a grant of
//! it).

use serde_json::{json, Value};

use crate::delivery_identity_authorization::{assumed_role, statement, Emulator, Principal};

/// The error code `GetLogObject` refuses an unresolvable pointer with, the one its model
/// declares for a parameter it will not take.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The error code the authorization layer in front of every other operation refuses with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The wire error code of an answer, or `None` for a body that is no error at all.
fn reported_code(body: &Value) -> Option<String> {
    let raw = body.get("__type").and_then(Value::as_str)?;
    Some(raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string())
}

/// A principal holding exactly the documents given, under a role name of its own.
async fn principal(aws: &Emulator, role: &str, policies: &[String]) -> Principal {
    assumed_role(aws, role, policies).await
}

/// `GetLogObject` refuses the pointer, not the caller: whatever the caller may do, a pointer
/// this service did not issue is answered with the operation's own parameter refusal.
#[tokio::test]
async fn an_unissued_pointer_is_refused_whatever_the_caller_may_do() {
    let aws = Emulator::new();
    // (case, the policies the caller holds)
    let callers: Vec<(&str, Vec<String>)> = vec![
        ("a caller holding no policy at all", Vec::new()),
        (
            "a caller whose policy denies the action",
            vec![statement("Deny", "logs:GetLogObject", "*")],
        ),
        (
            "a caller granted every action of this service",
            vec![statement("Allow", "logs:*", "*")],
        ),
        (
            "a caller granted another service's actions only",
            vec![statement("Allow", "s3:GetObject", "*")],
        ),
    ];
    // The pointer forms a caller can present: one it built itself, one this service mints for
    // another purpose (a query row's `@ptr`), and values at the modelled `length 1..512`
    // bounds.
    let pointers: Vec<(&str, String)> = vec![
        ("a pointer the caller built itself", "not-a-pointer".to_string()),
        (
            "a pointer of the shape a query row's @ptr carries",
            "CnsKPAo4".to_string(),
        ),
        ("the shortest pointer the model admits", "x".to_string()),
        ("the longest pointer the model admits", "p".repeat(512)),
    ];

    for (index, (caller_case, policies)) in callers.into_iter().enumerate() {
        let caller = principal(&aws, &format!("log-object-{index}"), &policies).await;
        for (pointer_case, pointer) in &pointers {
            let case = format!("{caller_case} / {pointer_case}");
            let (status, parsed) = aws
                .call_as(
                    &caller.access_key_id,
                    "GetLogObject",
                    &json!({"logObjectPointer": pointer}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the pointer to be refused, got {status} {parsed}"
            );
            assert_eq!(
                reported_code(&parsed).as_deref(),
                Some(INVALID_PARAMETER),
                "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
            );
        }
    }
}

/// The authorization layer is still in force for the operations whose request DOES name the
/// resource they act on: the same caller that reads a parameter refusal out of `GetLogObject`
/// is refused `AccessDeniedException` by a listing and by a read of a log group it was granted
/// nothing on.
#[tokio::test]
async fn an_operation_that_names_its_resource_is_still_decided_against_the_caller() {
    let aws = Emulator::new();
    let group = "/identity/log-objects";
    aws.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let caller = principal(&aws, "log-object-denied", &[]).await;

    for (case, operation, body) in [
        (
            "a listing that names no log group",
            "DescribeLogGroups",
            json!({}),
        ),
        (
            "a read of one log group",
            "DescribeLogStreams",
            json!({"logGroupName": group}),
        ),
        (
            "a write on one log group",
            "PutRetentionPolicy",
            json!({"logGroupName": group, "retentionInDays": 7}),
        ),
    ] {
        let (status, parsed) = aws.call_as(&caller.access_key_id, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to be refused, got {status} {parsed}"
        );
        assert_eq!(
            reported_code(&parsed).as_deref(),
            Some(ACCESS_DENIED),
            "case {case:?}: expected {ACCESS_DENIED} from {operation}, got {parsed}"
        );
    }
}
