// SPDX-License-Identifier: CC0-1.0

//! An object written to a bucket that publishes to EventBridge drives a matched rule's
//! PERMITTED Lambda target, whose `CreateInvalidation` becomes CloudFront's own invalidation
//! and refreshes what the distribution's edge serves — end to end over the assembled router.
//!
//! RULE: a write to a bucket whose notification configuration carries an
//! `EventBridgeConfiguration` ("enables delivery of events to Amazon EventBridge") publishes
//! an `aws.s3` "Object Created" event naming the bucket and the key; the default bus hands
//! that event to EVERY target of each ENABLED rule whose pattern matches it, and a Lambda
//! function target is handed it only where that FUNCTION's own resource policy admits the
//! `events.amazonaws.com` principal `lambda:InvokeFunction` on it — so of two targets of one
//! rule, exactly the permitted one runs. Whatever the running handler then does is the called
//! service's own state from that moment: a `CreateInvalidation` it submits is listed by
//! `ListInvalidations` and reported by `GetInvalidation` under the same `Id`, with the batch's
//! own `Paths`, and with a `Status` that is `Completed` only once the batch has finished
//! ("when the invalidation batch is finished, the status is `Completed`",
//! `smithy_query(query="shape",shape="Invalidation")` on `Status`) — after which the next
//! viewer request for a path the batch names is re-read from the origin.
//!
//! None of that depends on how the rule, the target, the grant or the function came to
//! exist: a stack's `AWS::Events::Rule`, `AWS::Lambda::Function` and
//! `AWS::Lambda::Permission` leave behind the same rule, target and statement a client's own
//! `PutRule`/`PutTargets`/`AddPermission` do.
//!
//! Only a cross-service suite can state this: the event is Amazon S3's, the match and the
//! handover EventBridge's, the invocation and the handler Lambda's, the execution role IAM's,
//! the stack CloudFormation's, and the invalidation and the edge CloudFront's.
//! [`super::cloudfront_edge_cache_refresh`] pins what an invalidation a CLIENT submits does
//! to the edge, [`super::lambda_handler_invalidates_its_distribution`] pins that a running
//! handler can submit one at all, and
//! [`super::cloudformation_events_rule_lambda_target_permission_boundary`] pins the resource
//! -policy boundary of a rule's Lambda target — none of them states that an ORIGIN WRITE is
//! what drives the invalidation.
//!
//! The rows of the first test differ along what the rule generalises over: the form of the
//! object's key (a bare name at the bucket root, a key under a prefix, a deeply nested key
//! carrying dots and dashes), which is what the handler derives the invalidated path from.
//! Each row carries both controls the boundary needs — the rule's UN-permitted sibling
//! target, whose own handler would submit a distinguishable batch, and a second
//! EventBridge-enabled bucket the rule's pattern does not name.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudfront_oac_s3_origin_edge::{
    create_bucket, create_distribution as create_private_distribution,
    create_origin_access_control, emulator, policy, put_bucket_policy, put_object, view,
    CLOUDFRONT_PRINCIPAL,
};
use crate::query_wire::{element, elements, form_encode, outputs_of};

/// The Region every setup call is signed for.
const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version every route of it is under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The S3 document namespace a notification configuration is written in.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `StackStatus` every deployment here waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The service principal EventBridge hands a Lambda target over as, and the action a grant
/// for it has to name.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The two `Status` values an invalidation is reported under: the terminal one the model's
/// own documentation names ("when the invalidation batch is finished, the status is
/// `Completed`") and the one a batch that has not finished carries.
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// The AWS-managed cache policy `Managed-CachingOptimized`, whose `DefaultTTL` is a day: a
/// behaviour under it HOLDS what it fetched, so only an invalidation can refresh it.
const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// `X-Cache` for an answer the edge cache held, and for one the origin produced.
const HIT: &str = "Hit from cloudfront";
const MISS: &str = "Miss from cloudfront";

/// How long a chain's observable end state is waited for before the suite gives up on it,
/// and how often it is looked at. Generous: no row measures the deadline, only the outcome.
const DELIVERY_BUDGET: Duration = Duration::from_secs(90);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How long the half that must observe NOTHING keeps looking before concluding nothing came.
const QUIET_BUDGET: Duration = Duration::from_secs(5);

/// The path prefix the PERMITTED target's handler invalidates the written key under — the
/// one a client's own invalidation Lambda would use — and the one the UN-permitted sibling
/// would use, which is what makes a handover that must not happen observable if it did.
const PERMITTED_PREFIX: &str = "/";
const BLOCKED_PREFIX: &str = "/blocked/";

/// The handler both declared functions run: it reads the object's key out of the event it was
/// handed and submits an invalidation for the path its own prefix makes of it, through the AWS
/// SDK the managed runtime supplies. The distribution, the prefix and the label are written
/// into the source, because a template declares a function's code before anything invokes it.
const INVALIDATING_HANDLER: &str = r#"
import boto3

DISTRIBUTION = "__DISTRIBUTION__"
PREFIX = "__PREFIX__"
LABEL = "__LABEL__"


def lambda_handler(event, context):
    key = event["detail"]["object"]["key"]
    client = boto3.client("cloudfront")
    submitted = client.create_invalidation(
        DistributionId=DISTRIBUTION,
        InvalidationBatch={
            "Paths": {"Items": [PREFIX + key]},
            "CallerReference": event["id"] + LABEL,
        },
    )
    return {"invalidation": submitted["Invalidation"]["Id"]}
"#;

/// [`INVALIDATING_HANDLER`] with this function's own distribution, prefix and label in it.
fn handler_source(distribution: &str, prefix: &str, label: &str) -> String {
    INVALIDATING_HANDLER
        .replace("__DISTRIBUTION__", distribution)
        .replace("__PREFIX__", prefix)
        .replace("__LABEL__", label)
}

/// The emulator as it is ASSEMBLED, on `engine`: a handler's own SDK call is dispatched back
/// into this same router, so the distribution it invalidates and the bucket that published the
/// event are the ones this suite created.
fn setup_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = aws_emulator_router::build_router(Arc::clone(engine));
    // The suite exercises the services, not SigV4: each setup call carries a credential scope
    // so the router can name the target service, without a computed signature.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked on the setup plane.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// What one request answered: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

/// One request against `host`, with `signing_name` in the credential scope.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    content_type: &str,
    body: &str,
) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// One CloudFront control-plane call.
async fn cloudfront(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        method,
        path,
        "application/xml",
        body,
    )
    .await
}

/// One CloudFormation awsQuery call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answered = call(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        &body,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: CloudFormation {action} must answer 200, got {} with {}",
        answered.status, answered.body
    );
    answered.body
}

/// One Lambda `restJson1` GET, as its parsed body.
async fn lambda_get(router: &MultiServiceRouter, uri: &str) -> Value {
    let answered = call(
        router,
        "lambda",
        &format!("lambda.{REGION}.amazonaws.com"),
        "GET",
        uri,
        "application/json",
        "",
    )
    .await;
    serde_json::from_str(&answered.body).unwrap_or(Value::Null)
}

/// `PUT /<bucket>?notification` with an `EventBridgeConfiguration` and nothing else, which is
/// what "enables delivery of events to Amazon EventBridge" for that bucket.
async fn enable_event_bridge(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let body = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\">\
         <EventBridgeConfiguration></EventBridgeConfiguration>\
         </NotificationConfiguration>"
    );
    let stored = call(
        router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        "PUT",
        &format!("/{bucket}?notification"),
        "application/xml",
        &body,
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "case {case:?}: enabling EventBridge notifications on {bucket:?} must succeed, got {} \
         with {}",
        stored.status, stored.body
    );
}

/// `cloudfront:CreateDistribution` over the bucket `bucket` as a plain S3 origin, answering
/// the distribution's id. The first test never fetches through this distribution: what it
/// reads is the INVALIDATION state the chain leaves on it.
async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    bucket: &str,
) -> String {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>origin-bucket</Id>\
         <DomainName>{bucket}.s3.{REGION}.amazonaws.com</DomainName>\
         <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <ForwardedValues><QueryString>false</QueryString>\
         <Cookies><Forward>none</Forward></Cookies></ForwardedValues>\
         <MinTTL>0</MinTTL>\
         </DefaultCacheBehavior>\
         <Comment>the distribution an object write invalidates</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let created = cloudfront(router, "POST", &format!("/{API_VERSION}/distribution"), &body).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateDistribution must succeed, got {} with {}",
        created.status, created.body
    );
    let id = element(&created.body, "Id");
    assert!(
        !id.is_empty(),
        "case {case:?}: the created distribution reports no Id: {}",
        created.body
    );
    id
}

/// The stack one row deploys: the execution role the handlers call CloudFront under, the two
/// function targets, the rule that matches this bucket's "Object Created" events with BOTH of
/// them as targets, and the `AWS::Lambda::Permission` for exactly ONE of them.
fn chain_stack(stack: &str, bucket: &str, distribution: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "InvalidatorRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{stack}-exec"),
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "invalidate-the-distribution",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "cloudfront:CreateInvalidation",
                                    "cloudfront:GetInvalidation",
                                    "cloudfront:ListInvalidations",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Permitted": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-permitted"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": { "Fn::GetAtt": ["InvalidatorRole", "Arn"] },
                    "Timeout": 30,
                    "Code": {
                        "ZipFile": handler_source(distribution, PERMITTED_PREFIX, "-permitted"),
                    },
                },
            },
            "Blocked": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-blocked"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": { "Fn::GetAtt": ["InvalidatorRole", "Arn"] },
                    "Timeout": 30,
                    "Code": {
                        "ZipFile": handler_source(distribution, BLOCKED_PREFIX, "-blocked"),
                    },
                },
            },
            "ObjectCreated": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": format!("{stack}-rule"),
                    "State": "ENABLED",
                    "EventPattern": {
                        "source": ["aws.s3"],
                        "detail-type": ["Object Created"],
                        "detail": { "bucket": { "name": [bucket] } },
                    },
                    "Targets": [
                        { "Id": "permitted", "Arn": { "Fn::GetAtt": ["Permitted", "Arn"] } },
                        { "Id": "blocked", "Arn": { "Fn::GetAtt": ["Blocked", "Arn"] } },
                    ],
                },
            },
            "InvokePermitted": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Permitted", "Arn"] },
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                    "SourceArn": { "Fn::GetAtt": ["ObjectCreated", "Arn"] },
                },
            },
        },
        "Outputs": {
            "PermittedFunction": { "Value": { "Ref": "Permitted" } },
            "BlockedFunction": { "Value": { "Ref": "Blocked" } },
        },
    })
    .to_string()
}

/// Deploy `stack` and wait for `CREATE_COMPLETE`, answering the names of the two functions it
/// declares.
async fn deploy_chain(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    bucket: &str,
    distribution: &str,
) -> (String, String) {
    cloudformation(
        router,
        case,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &chain_stack(stack, bucket, distribution)),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {case:?}: expected the stack {stack:?} to deploy, got {status} with {described}"
    );
    let outputs = outputs_of(&described);
    let name = |output: &str| {
        outputs.get(output).cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish {output}, got {described}")
        })
    };
    let (permitted, blocked) = (name("PermittedFunction"), name("BlockedFunction"));
    // "During this time, you can't invoke or modify the function" (`CreateFunction`): a
    // handover withheld before either is ready would be the lifecycle's refusal, not the
    // policy's.
    for function in [&permitted, &blocked] {
        wait_until_active(router, case, function).await;
    }
    (permitted, blocked)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports `name` `Active` — "the function
/// is ready to be invoked" (`FunctionConfiguration$State`). The span a created function spends
/// coming up is that service's own, so nothing here sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut reported = Value::Null;
    while Instant::now() < deadline {
        reported = lambda_get(
            router,
            &format!("/2015-03-31/functions/{name}/configuration"),
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: Lambda never reported {name:?} Active, last read {reported}");
}

/// Poll `GetDistribution` until CloudFront first reports `id` `Deployed`. The span is that
/// service's own.
async fn wait_until_deployed(router: &MultiServiceRouter, case: &str, id: &str) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut last = String::new();
    while Instant::now() < deadline {
        let read = cloudfront(
            router,
            "GET",
            &format!("/{API_VERSION}/distribution/{id}"),
            "",
        )
        .await;
        last = element(&read.body, "Status");
        if last == "Deployed" {
            return;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: the distribution {id:?} never reached Deployed, last read {last:?}");
}

/// One invalidation as a client reads it: the id its SUMMARY carries in the list, and the id,
/// status and batch paths `GetInvalidation` reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invalidation {
    listed_id: String,
    read_id: String,
    status: String,
    paths: Vec<String>,
}

/// Every invalidation CloudFront holds for `distribution`, read the way a client finds one:
/// `ListInvalidations` for the ids, then `GetInvalidation` for each — the list carries no
/// paths at all (`InvalidationSummary` is `Id`, `CreateTime` and `Status`).
async fn invalidations(
    router: &MultiServiceRouter,
    case: &str,
    distribution: &str,
) -> Vec<Invalidation> {
    let listed = cloudfront(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution}/invalidation"),
        "",
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListInvalidations must answer, got {} with {}",
        listed.status, listed.body
    );
    let mut held = Vec::new();
    for listed_id in elements(&listed.body, "Id") {
        let read = cloudfront(
            router,
            "GET",
            &format!("/{API_VERSION}/distribution/{distribution}/invalidation/{listed_id}"),
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetInvalidation({listed_id}) must answer, got {} with {}",
            read.status, read.body
        );
        held.push(Invalidation {
            listed_id,
            read_id: element(&read.body, "Id"),
            status: element(&read.body, "Status"),
            paths: elements(&read.body, "Path"),
        });
    }
    held
}

/// Wait for the invalidation whose batch names exactly `paths` to appear on `distribution`,
/// answering it and every invalidation seen alongside it.
async fn wait_for_invalidation(
    router: &MultiServiceRouter,
    case: &str,
    distribution: &str,
    paths: &[String],
) -> (Option<Invalidation>, Vec<Invalidation>) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let held = invalidations(router, case, distribution).await;
        if let Some(found) = held.iter().find(|entry| entry.paths == paths) {
            return (Some(found.clone()), held);
        }
        if Instant::now() >= deadline {
            return (None, held);
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Keep looking for an invalidation whose batch names exactly `paths` for as long as
/// [`QUIET_BUDGET`], answering the first one seen — `None` when none ever is.
async fn stayed_absent(
    router: &MultiServiceRouter,
    case: &str,
    distribution: &str,
    paths: &[String],
) -> Option<Invalidation> {
    let deadline = Instant::now() + QUIET_BUDGET;
    while Instant::now() < deadline {
        let held = invalidations(router, case, distribution).await;
        if let Some(found) = held.into_iter().find(|entry| entry.paths == paths) {
            return Some(found);
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    None
}

/// Poll `GetInvalidation` until the batch `id` reports `Completed`, answering the last status
/// read.
async fn wait_until_completed(router: &MultiServiceRouter, distribution: &str, id: &str) -> String {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut last = String::new();
    while Instant::now() < deadline {
        let read = cloudfront(
            router,
            "GET",
            &format!("/{API_VERSION}/distribution/{distribution}/invalidation/{id}"),
            "",
        )
        .await;
        last = element(&read.body, "Status");
        if last == COMPLETED {
            return last;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    last
}

/// One row: the object the chain is driven by.
struct Row {
    case: &'static str,
    /// The stack, the bucket and the key, each this row's own so one row's rule cannot match
    /// another's write and one row's invalidation cannot be mistaken for another's.
    stack: &'static str,
    bucket: &'static str,
    key: &'static str,
    bytes: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a bare key at the bucket root",
        stack: "xsvc-object-created-invalidation-bare",
        bucket: "object-created-invalidation-bare",
        key: "index.html",
        bytes: "<p>the first revision</p>",
    },
    Row {
        case: "a key under a prefix",
        stack: "xsvc-object-created-invalidation-nested",
        bucket: "object-created-invalidation-nested",
        key: "assets/site/app.js",
        bytes: "export const revision = 1;",
    },
    Row {
        case: "a deeply nested key carrying dots and dashes",
        stack: "xsvc-object-created-invalidation-deep",
        bucket: "object-created-invalidation-deep",
        key: "releases/2026-02/bundle-v3.min.js",
        bytes: "//# revision 3",
    },
];

#[tokio::test]
async fn an_object_created_event_drives_the_permitted_targets_invalidation() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = setup_router(&engine);

    for row in ROWS {
        let case = row.case;
        // The bucket the rule's pattern names, and a second EventBridge-enabled bucket it
        // does not — the control for the MATCH, beside the permission control the stack's
        // un-permitted sibling target is.
        let elsewhere = format!("{}-elsewhere", row.bucket);
        create_bucket(&router, case, row.bucket).await;
        create_bucket(&router, case, &elsewhere).await;

        let distribution =
            create_distribution(&router, case, &format!("{}-reference", row.stack), row.bucket)
                .await;
        let (permitted, blocked) =
            deploy_chain(&router, case, row.stack, row.bucket, &distribution).await;

        enable_event_bridge(&router, case, row.bucket).await;
        enable_event_bridge(&router, case, &elsewhere).await;

        // Written FIRST, so the write the rule's pattern does not name has every chance to
        // produce an invalidation it must not produce.
        let control_key = format!("control/{}", row.key);
        put_object(&router, case, &elsewhere, &control_key, row.bytes, &[]).await;
        put_object(&router, case, row.bucket, row.key, row.bytes, &[]).await;

        // The permitted target's own work product: the invalidation its handler submitted for
        // the path it derived from the event's key.
        let expected = vec![format!("{PERMITTED_PREFIX}{}", row.key)];
        let (found, held) = wait_for_invalidation(&router, case, &distribution, &expected).await;
        let found = found.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the write of {:?} to drive the permitted target \
                 {permitted:?} to invalidate {expected:?}, got the invalidations {held:?}",
                row.key
            )
        });
        assert_eq!(
            found.read_id, found.listed_id,
            "case {case:?}: expected GetInvalidation to report the Id ListInvalidations \
             listed, got {found:?}"
        );
        assert!(
            found.status == IN_PROGRESS || found.status == COMPLETED,
            "case {case:?}: expected the invalidation's Status to be {IN_PROGRESS:?} or \
             {COMPLETED:?}, got {:?}",
            found.status
        );

        // The permission control: the sibling target of the SAME rule, which no statement
        // admits, never ran — so the batch its own handler would have submitted is nowhere.
        let withheld = vec![format!("{BLOCKED_PREFIX}{}", row.key)];
        let leaked = stayed_absent(&router, case, &distribution, &withheld).await;
        assert!(
            leaked.is_none(),
            "case {case:?}: no statement admits {blocked:?}, so expected no invalidation of \
             {withheld:?}, got {leaked:?}"
        );

        // The match control: the write to the bucket the rule's pattern does not name reached
        // no target at all.
        let unmatched = vec![format!("{PERMITTED_PREFIX}{control_key}")];
        let matched_anyway = stayed_absent(&router, case, &distribution, &unmatched).await;
        assert!(
            matched_anyway.is_none(),
            "case {case:?}: the rule's pattern does not name {elsewhere:?}, so expected no \
             invalidation of {unmatched:?}, got {matched_anyway:?}"
        );
    }
}

/// The bucket, key and the three revisions the edge leg writes.
const REFRESH_BUCKET: &str = "object-created-invalidation-edge";
const REFRESH_KEY: &str = "assets/app.js";
const FIRST: &str = "export const revision = 1;";
const SECOND: &str = "export const revision = 2;";
const THIRD: &str = "export const revision = 3;";

#[tokio::test]
async fn the_chain_refreshes_what_the_distributions_edge_serves() {
    let case = "an object overwritten on an EventBridge-enabled origin";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The setup plane, which every control-plane call and the object writes go through; and
    // the VIEWER plane, an authenticating router over the SAME engine, which is what makes the
    // signature CloudFront puts on its origin request the thing Amazon S3 serves the fetch
    // for.
    let router = setup_router(&engine);
    let viewers = emulator(&engine, true);

    create_bucket(&router, case, REFRESH_BUCKET).await;
    put_object(&router, case, REFRESH_BUCKET, REFRESH_KEY, FIRST, &[]).await;

    let control =
        create_origin_access_control(&router, case, "object-created-invalidation-oac", "always")
            .await;
    let (distribution, domain, arn) = create_private_distribution(
        &router,
        case,
        "object-created-invalidation-edge-reference",
        REFRESH_BUCKET,
        &control,
        CACHING_OPTIMIZED,
    )
    .await;
    // Written AFTER the distribution exists, because it is scoped to that distribution's own
    // ARN — the order a client has to follow.
    put_bucket_policy(
        &router,
        case,
        REFRESH_BUCKET,
        &policy(
            CLOUDFRONT_PRINCIPAL,
            "s3:GetObject",
            &format!("arn:aws:s3:::{REFRESH_BUCKET}/*"),
            Some(&format!(
                "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
            )),
        ),
    )
    .await;
    wait_until_deployed(&router, case, &distribution).await;

    let path = format!("/{REFRESH_KEY}");

    // The first viewer request is the ORIGIN's answer, the second the CACHE's.
    let fetched = view(&viewers, &domain, &path).await;
    assert_eq!(
        (fetched.status, fetched.text().as_str(), fetched.header("x-cache").as_str()),
        (200, FIRST, MISS),
        "case {case:?}: expected the origin's object through the edge, got {} with {:?} and \
         X-Cache {:?}",
        fetched.status,
        fetched.text(),
        fetched.header("x-cache")
    );
    let cached = view(&viewers, &domain, &path).await;
    assert_eq!(
        (cached.text().as_str(), cached.header("x-cache").as_str()),
        (FIRST, HIT),
        "case {case:?}: expected the cached object, got {:?} with X-Cache {:?}",
        cached.text(),
        cached.header("x-cache")
    );

    // An ORIGIN WRITE alone changes nothing a viewer sees. Nothing is wired yet, so no
    // invalidation can exist at all: the stale answer here is the rule's own baseline.
    put_object(&router, case, REFRESH_BUCKET, REFRESH_KEY, SECOND, &[]).await;
    let stale = view(&viewers, &domain, &path).await;
    assert_eq!(
        (stale.text().as_str(), stale.header("x-cache").as_str()),
        (FIRST, HIT),
        "case {case:?}: an origin write alone must not change what the edge serves, got {:?} \
         with X-Cache {:?}",
        stale.text(),
        stale.header("x-cache")
    );

    // Now the chain: the rule, its permitted Lambda target and the bucket's EventBridge
    // notifications.
    let (permitted, _blocked) = deploy_chain(
        &router,
        case,
        "xsvc-object-created-invalidation-edge",
        REFRESH_BUCKET,
        &distribution,
    )
    .await;
    enable_event_bridge(&router, case, REFRESH_BUCKET).await;

    put_object(&router, case, REFRESH_BUCKET, REFRESH_KEY, THIRD, &[]).await;
    let expected = vec![path.clone()];
    let (found, held) = wait_for_invalidation(&router, case, &distribution, &expected).await;
    let found = found.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the write to drive {permitted:?} to invalidate \
             {expected:?}, got the invalidations {held:?}"
        )
    });
    let status = wait_until_completed(&router, &distribution, &found.read_id).await;
    assert_eq!(
        status, COMPLETED,
        "case {case:?}: the chain's invalidation {:?} never reached {COMPLETED}, last read \
         {status:?}",
        found.read_id
    );

    // Only once the batch the chain submitted has finished is the object re-read from the
    // origin — and what the origin now holds is the bytes the write that drove the chain put
    // there.
    let refreshed = view(&viewers, &domain, &path).await;
    assert_eq!(
        (refreshed.text().as_str(), refreshed.header("x-cache").as_str()),
        (THIRD, MISS),
        "case {case:?}: after the chain's invalidation completed, expected the newly written \
         bytes from the origin, got {:?} with X-Cache {:?}",
        refreshed.text(),
        refreshed.header("x-cache")
    );
}
