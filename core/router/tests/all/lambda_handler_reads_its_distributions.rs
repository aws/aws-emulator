// SPDX-License-Identifier: CC0-1.0

//! A Lambda handler that RESOLVES the CloudFront distribution in front of its origin out of
//! CloudFront's own records before refreshing it, end to end over the assembled router.
//!
//! RULE: the AWS SDK a managed runtime supplies binds the calls of a service a handler
//! reaches, so a handler's read of the distribution surface — the account's distributions,
//! one page of them, one distribution by id — is MADE under the function's execution role and
//! answered with the service's own records; an operation the seam binds no request for is
//! reported as a call that was NOT MADE, naming it, and a resource the service does not hold
//! is answered with the service's own refusal.
//!
//! Only a cross-service suite can pin this: the distributions are CloudFront's records, the
//! handler and the SDK seam it calls through are Lambda's, the role it runs as is IAM's and
//! the origins behind the distributions are Amazon S3's. The Lambda crate's own suites pin
//! what a handler computes, and the CloudFront crate's pin what the distribution surface
//! answers; neither can state that a RUNNING handler can read it at all. The INVALIDATION
//! half of the same seam is the suite in
//! `core/router/tests/all/lambda_handler_invalidates_its_distribution.rs`, which owns what a
//! submitted batch does to a distribution the handler was handed by id; this suite owns
//! finding the distribution in the first place, and invalidates only the one it found.
//!
//! The rows differ along what the rule generalises over for a distribution the handler finds:
//! the number of origins it serves from (one, two), whether an origin carries a path, whether
//! the distribution is enabled, and the price class it is served under — and then, over the
//! surface itself, along the FORM of the read: the whole listing, a bounded page and the
//! resume after it, a read by id, a read of an id no distribution holds, and an operation the
//! seam binds no request for.
//!
//! Every read runs in ONE invocation, an invocation being the dearest thing this suite does,
//! and the handler ends by invalidating the distribution it FOUND — the event names no
//! distribution id at all — which is then read back through CloudFront's own API, where the
//! invalidation has to exist.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode, opt_element, outputs_of};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version every route is under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The buckets the distributions below serve from, and the object behind the paths the
/// handler invalidates.
const BUCKET_A: &str = "edge-reads-origin-a";
const BUCKET_B: &str = "edge-reads-origin-b";
const KEY: &str = "assets/app.js";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The status a distribution reaches once CloudFront has propagated it, which is the one
/// terminal status its reads then keep reporting.
const DEPLOYED: &str = "Deployed";

/// How long work the clock owes is waited for, and how often it is looked at. Generous: the
/// deadline is not what any row measures.
const WAIT: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The role the function runs as, and the function's own name.
const ROLE: &str = "edge-reader-role";
const FUNCTION: &str = "edge-reader";

/// An id of the shape CloudFront mints a distribution id in (`E` and 13 more characters of
/// its id alphabet) that no distribution in this suite holds — the row for a read of a
/// resource the service does not hold.
const ABSENT_DISTRIBUTION: &str = "EABSENT0000000";

/// An operation of CloudFront the runtime's SDK seam binds no request for, which a handler
/// naming it is told was not made.
const UNBOUND_OPERATION: &str = "ListStreamingDistributions";

/// The paths the handler invalidates on the distribution it found.
const PATHS: &[&str] = &["/assets/app.js", "/assets/*"];

/// The handler the stack deploys: it reads the distribution surface through the AWS SDK the
/// runtime supplies and reports what CloudFront answered each read, then invalidates the
/// distribution it FOUND — the enabled one serving the origin the event names.
const HANDLER: &str = r#"
import boto3
import botocore.exceptions


def summary(item):
    return {
        "arn": item["ARN"],
        "status": item["Status"],
        "comment": item["Comment"],
        "domain_name": item["DomainName"],
        "enabled": item["Enabled"],
        "price_class": item["PriceClass"],
        # The SDK hands a member the model types as a timestamp over as a `datetime`, so what
        # this report carries is the instant it stands for.
        "last_modified": item["LastModifiedTime"].timestamp(),
        "aliases": item["Aliases"].get("Items", []),
        "alias_quantity": item["Aliases"]["Quantity"],
        "origin_quantity": item["Origins"]["Quantity"],
        "origins": [origin["DomainName"] for origin in item["Origins"].get("Items", [])],
        "origin_paths": [
            origin.get("OriginPath", "") for origin in item["Origins"].get("Items", [])
        ],
        "target_origin_id": item["DefaultCacheBehavior"]["TargetOriginId"],
    }


def lambda_handler(event, context):
    cloudfront = boto3.client("cloudfront")
    report = {}

    listed = cloudfront.list_distributions()["DistributionList"]
    report["listed"] = {
        "quantity": listed["Quantity"],
        "is_truncated": listed["IsTruncated"],
        "marker": listed["Marker"],
        "items": {item["Id"]: summary(item) for item in listed.get("Items", [])},
    }

    reads = {}
    for held in event["distributions"]:
        answered = cloudfront.get_distribution(Id=held)
        distribution = answered["Distribution"]
        config = distribution["DistributionConfig"]
        reads[held] = {
            "id": distribution["Id"],
            "arn": distribution["ARN"],
            "status": distribution["Status"],
            "domain_name": distribution["DomainName"],
            "in_progress": distribution["InProgressInvalidationBatches"],
            "etag": answered.get("ETag", ""),
            "comment": config["Comment"],
            "caller_reference": config["CallerReference"],
            "enabled": config["Enabled"],
            "origins": [origin["DomainName"] for origin in config["Origins"]["Items"]],
        }
    report["read"] = reads

    page = cloudfront.list_distributions(MaxItems=1)["DistributionList"]
    report["page"] = {
        "max_items": page["MaxItems"],
        "quantity": page["Quantity"],
        "is_truncated": page["IsTruncated"],
        "next_marker": page.get("NextMarker", ""),
        "ids": [item["Id"] for item in page.get("Items", [])],
    }
    if page.get("NextMarker"):
        resumed = cloudfront.list_distributions(Marker=page["NextMarker"], MaxItems=1)[
            "DistributionList"
        ]
        report["resumed"] = {
            "marker": resumed["Marker"],
            "ids": [item["Id"] for item in resumed.get("Items", [])],
        }

    try:
        cloudfront.get_distribution(Id=event["absent"])
    except botocore.exceptions.ClientError as error:
        report["absent"] = {
            "code": error.response["Error"]["Code"],
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        }

    try:
        getattr(cloudfront, event["unbound"])()
    except botocore.exceptions.SdkCallNotMade as error:
        report["unbound"] = str(error)

    # The point of the reads: the distribution to refresh is the ENABLED one serving the
    # origin just written to, which the event names by its origin and not by its id.
    chosen = next(
        held
        for held, found in report["listed"]["items"].items()
        if event["origin"] in found["origins"] and found["enabled"]
    )
    submitted = cloudfront.create_invalidation(
        DistributionId=chosen,
        InvalidationBatch={
            "Paths": {"Items": event["paths"]},
            "CallerReference": event["reference"],
        },
    )
    report["invalidated"] = {
        "distribution": chosen,
        "id": submitted["Invalidation"]["Id"],
        "paths": submitted["Invalidation"]["InvalidationBatch"]["Paths"]["Items"],
    }
    return report
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the distributions it reads are the ones this suite created.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: each setup call carries a credential
    // scope so the router can name the target service, without a computed signature.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One response: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<name>…</name>` element of the body, or the empty string.
    fn element(&self, name: &str) -> String {
        opt_element(&self.body, name).unwrap_or_default()
    }
}

/// One request against a service's own host, with `signing_name` in the credential scope.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name))
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// One Amazon S3 REST call, path-style as the emulator serves it.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    call(
        router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        method,
        path,
        body,
    )
    .await
}

/// One CloudFront control-plane call.
async fn cloudfront(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    call(router, "cloudfront", CLOUDFRONT_HOST, method, path, body).await
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// One Lambda `restJson1` call, answered with the status and the body as JSON.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(body) => Bytes::from(body.to_string()),
        None => Bytes::new(),
    };
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The buckets the distributions serve, each holding the object the paths name.
async fn create_origins(router: &MultiServiceRouter) {
    for bucket in [BUCKET_A, BUCKET_B] {
        let created = s3(router, "PUT", &format!("/{bucket}"), "").await;
        assert_eq!(
            created.status, 200,
            "setup: CreateBucket({bucket}) must succeed, got {} with {}",
            created.status, created.body
        );
        let written = s3(router, "PUT", &format!("/{bucket}/{KEY}"), "the asset").await;
        assert_eq!(
            written.status, 200,
            "setup: PutObject({bucket}) must succeed, got {} with {}",
            written.status, written.body
        );
    }
}

/// One row: a distribution the handler has to find, and what CloudFront must report about it.
struct Row {
    case: &'static str,
    /// The comment the configuration carries, which is also what tells the rows apart.
    comment: &'static str,
    /// The buckets this distribution serves from, in order, each with the origin path it
    /// carries (the empty string for an origin that carries none).
    origins: &'static [(&'static str, &'static str)],
    /// Whether the distribution accepts viewer requests.
    enabled: bool,
    /// The price class the configuration states, or `None` to state none at all — in which
    /// case CloudFront reports the one an absent member means.
    price_class: Option<&'static str>,
    /// The price class the reads must then report.
    reported_price_class: &'static str,
}

/// The rows, differing along the number of origins, whether an origin carries a path,
/// whether the distribution is enabled, and the price class it is served under.
///
/// `PriceClass_100` and `PriceClass_All` are `PriceClass` values of this service's model
/// (`smithy_query(query="shape",shape="PriceClass")`); a configuration that states none at
/// all is served under `PriceClass_All`, the value CloudFront reports for it.
const ROWS: &[Row] = &[
    Row {
        case: "one origin, enabled, with no price class of its own",
        comment: "the first distribution a handler finds",
        origins: &[(BUCKET_A, "")],
        enabled: true,
        price_class: None,
        reported_price_class: "PriceClass_All",
    },
    Row {
        case: "two origins, one of them with an origin path, enabled",
        comment: "the distribution in front of the object just written",
        origins: &[(BUCKET_A, ""), (BUCKET_B, "/static")],
        enabled: true,
        price_class: Some("PriceClass_100"),
        reported_price_class: "PriceClass_100",
    },
    Row {
        case: "one origin, disabled",
        comment: "the distribution that accepts no viewer requests",
        origins: &[(BUCKET_B, "")],
        enabled: false,
        price_class: Some("PriceClass_200"),
        reported_price_class: "PriceClass_200",
    },
];

/// The domain name an Amazon S3 bucket is addressed as an origin by.
fn origin_domain(bucket: &str) -> String {
    format!("{bucket}.s3.{REGION}.amazonaws.com")
}

/// What one distribution's create answered: the identity CloudFront minted for it.
struct Created {
    id: String,
    arn: String,
    domain_name: String,
}

/// `cloudfront:CreateDistribution` for one row, answering the identity it was given.
async fn create_distribution(
    router: &MultiServiceRouter,
    row: &Row,
    caller_reference: &str,
) -> Created {
    let mut origins = String::new();
    for (bucket, path) in row.origins {
        origins.push_str(&format!(
            "<Origin><Id>{bucket}</Id><DomainName>{}</DomainName>{}\
             <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
             </Origin>",
            origin_domain(bucket),
            if path.is_empty() {
                String::new()
            } else {
                format!("<OriginPath>{path}</OriginPath>")
            },
        ));
    }
    let price_class = match row.price_class {
        Some(class) => format!("<PriceClass>{class}</PriceClass>"),
        None => String::new(),
    };
    let target = row.origins[0].0;
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>{}</Quantity><Items>{origins}</Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>{target}</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <ForwardedValues><QueryString>false</QueryString>\
         <Cookies><Forward>none</Forward></Cookies></ForwardedValues>\
         <MinTTL>0</MinTTL>\
         </DefaultCacheBehavior>\
         <Comment>{}</Comment>{price_class}\
         <Enabled>{}</Enabled>\
         </DistributionConfig>",
        row.origins.len(),
        row.comment,
        row.enabled,
    );
    let created = cloudfront(router, "POST", &format!("/{API_VERSION}/distribution"), &body).await;
    assert_eq!(
        created.status, 201,
        "setup {:?}: CreateDistribution must succeed, got {} with {}",
        row.case, created.status, created.body
    );
    let held = Created {
        id: created.element("Id"),
        arn: created.element("ARN"),
        domain_name: created.element("DomainName"),
    };
    assert!(
        !held.id.is_empty() && !held.arn.is_empty() && !held.domain_name.is_empty(),
        "setup {:?}: the created distribution must report an identity, got {}",
        row.case,
        created.body
    );
    held
}

/// Wait until CloudFront reports `id` propagated, which is the terminal status its reads then
/// keep reporting. The span a created distribution spends propagating is that service's own,
/// so nothing here sleeps a fixed span on its behalf.
async fn wait_until_deployed(router: &MultiServiceRouter, id: &str) {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut status = String::new();
    for _ in 0..looks {
        let read = cloudfront(
            router,
            "GET",
            &format!("/{API_VERSION}/distribution/{id}"),
            "",
        )
        .await;
        status = read.element("Status");
        if status == DEPLOYED {
            return;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("setup: the distribution {id} must reach {DEPLOYED}, last reported {status:?}");
}

/// The stack that declares the execution role and the function: the role trusts Lambda and is
/// granted the distribution reads and the invalidation, which is what the handler's calls are
/// judged against.
fn handler_stack() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ReaderRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "read-and-invalidate-the-distributions",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "cloudfront:ListDistributions",
                                    "cloudfront:GetDistribution",
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
            "Reader": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["ReaderRole", "Arn"]},
                    "Timeout": 30,
                    "Code": {"ZipFile": HANDLER},
                },
            },
        },
        "Outputs": {
            "FunctionName": {"Value": {"Ref": "Reader"}},
        },
    })
    .to_string()
}

/// What `DescribeStacks` reports for `stack`: its status and the whole body.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (_status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStacks",
        &[("StackName", stack)],
    )
    .await;
    (element(&body, "StackStatus"), body)
}

/// Create the stack, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
) -> BTreeMap<String, String> {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// Wait until Lambda reports the function `Active` — the state an invocation needs. The span
/// a created function spends coming up is that service's own, so nothing here sleeps a fixed
/// span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut state = String::new();
    for _ in 0..looks {
        let (_status, body) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        state = body["State"].as_str().unwrap_or_default().to_string();
        if state == "Active" {
            return;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("setup: the function {name:?} must reach Active, last reported {state:?}");
}

/// The text values of a JSON array, in order.
fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_handler_reads_the_distribution_surface_then_invalidates_what_it_found() {
    let router = build_router();
    create_origins(&router).await;

    let mut created: Vec<Created> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        created.push(create_distribution(&router, row, &format!("edge-reads-{index}")).await);
    }
    for held in &created {
        wait_until_deployed(&router, &held.id).await;
    }

    let outputs = deployed(&router, "xsvc-edge-reads-stack", &handler_stack()).await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| FUNCTION.to_string());
    wait_until_active(&router, &function).await;

    let ids: Vec<String> = created.iter().map(|held| held.id.clone()).collect();
    let (status, report) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{function}/invocations"),
        Some(json!({
            "distributions": ids,
            "absent": ABSENT_DISTRIBUTION,
            "unbound": "list_streaming_distributions",
            // The distribution to refresh is named by its ORIGIN, so the handler has to
            // resolve the id out of CloudFront's own records.
            "origin": origin_domain(BUCKET_B),
            "paths": PATHS,
            "reference": "edge-reads-invalidation",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the invocation must be served, got {status} with {report}"
    );
    assert!(
        report.get("errorMessage").is_none() && report.get("errorType").is_none(),
        "the handler must not fault, got {report}"
    );

    // Every distribution this suite created is on the listing the handler read, reported with
    // the configuration it was created under.
    let listed = &report["listed"]["items"];
    assert_eq!(
        report["listed"]["quantity"],
        json!(ROWS.len()),
        "expected the listing to report every distribution created, got {}",
        report["listed"]
    );
    assert_eq!(
        report["listed"]["is_truncated"],
        json!(false),
        "expected the whole listing to be untruncated, got {}",
        report["listed"]
    );
    for (row, held) in ROWS.iter().zip(&created) {
        let case = row.case;
        let found = &listed[&held.id];
        assert!(
            found.is_object(),
            "case {case:?}: expected the listing to hold {}, got {listed}",
            held.id
        );
        assert_eq!(
            found["arn"],
            json!(held.arn),
            "case {case:?}: expected the ARN the create answered, got {found}"
        );
        assert_eq!(
            found["domain_name"],
            json!(held.domain_name),
            "case {case:?}: expected the domain name the create answered, got {found}"
        );
        assert_eq!(
            found["status"],
            json!(DEPLOYED),
            "case {case:?}: expected the propagated status, got {found}"
        );
        assert_eq!(
            found["comment"],
            json!(row.comment),
            "case {case:?}: expected the configuration's own comment, got {found}"
        );
        assert_eq!(
            found["enabled"],
            json!(row.enabled),
            "case {case:?}: expected the configuration's own Enabled, got {found}"
        );
        assert_eq!(
            found["price_class"],
            json!(row.reported_price_class),
            "case {case:?}: expected the price class the configuration is served under, got \
             {found}"
        );
        assert_eq!(
            found["origin_quantity"],
            json!(row.origins.len()),
            "case {case:?}: expected as many origins as the configuration declared, got {found}"
        );
        assert_eq!(
            texts(&found["origins"]),
            row.origins
                .iter()
                .map(|(bucket, _path)| origin_domain(bucket))
                .collect::<Vec<String>>(),
            "case {case:?}: expected the origins' own domain names, got {found}"
        );
        assert_eq!(
            texts(&found["origin_paths"]),
            row.origins
                .iter()
                .map(|(_bucket, path)| (*path).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected each origin's own path, got {found}"
        );
        assert_eq!(
            found["target_origin_id"],
            json!(row.origins[0].0),
            "case {case:?}: expected the default behaviour's own target origin, got {found}"
        );
        // A distribution created with no alternate domain names carries the empty wrapper:
        // `Aliases$Quantity` is 0 and the listing carries no `Items` for it at all.
        assert_eq!(
            found["alias_quantity"],
            json!(0),
            "case {case:?}: expected no alternate domain names, got {found}"
        );
        assert!(
            texts(&found["aliases"]).is_empty(),
            "case {case:?}: expected no alternate domain names, got {found}"
        );
        assert!(
            found["last_modified"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected the time the distribution was last modified, got {found}"
        );

        // The read BY ID answers the same record, with the configuration the distribution is
        // served under and the version token the surface addresses it by.
        let read = &report["read"][&held.id];
        assert_eq!(
            read["id"],
            json!(held.id),
            "case {case:?}: expected the distribution read by id, got {read}"
        );
        assert_eq!(
            read["arn"],
            json!(held.arn),
            "case {case:?}: expected the same ARN the listing reported, got {read}"
        );
        assert_eq!(
            read["domain_name"],
            json!(held.domain_name),
            "case {case:?}: expected the same domain name the listing reported, got {read}"
        );
        assert_eq!(
            read["status"],
            json!(DEPLOYED),
            "case {case:?}: expected the propagated status, got {read}"
        );
        assert_eq!(
            read["comment"],
            json!(row.comment),
            "case {case:?}: expected the configuration's own comment, got {read}"
        );
        assert_eq!(
            read["enabled"],
            json!(row.enabled),
            "case {case:?}: expected the configuration's own Enabled, got {read}"
        );
        assert_eq!(
            texts(&read["origins"]),
            row.origins
                .iter()
                .map(|(bucket, _path)| origin_domain(bucket))
                .collect::<Vec<String>>(),
            "case {case:?}: expected the configuration's own origins, got {read}"
        );
        assert!(
            read["caller_reference"]
                .as_str()
                .is_some_and(|reference| reference.starts_with("edge-reads-")),
            "case {case:?}: expected the reference the create was made under, got {read}"
        );
        assert!(
            read["etag"].as_str().is_some_and(|tag| !tag.is_empty()),
            "case {case:?}: expected the version of the distribution's information, got {read}"
        );
        assert_eq!(
            read["in_progress"],
            json!(0),
            "case {case:?}: nothing had been invalidated when the read was made, got {read}"
        );
    }

    // A BOUNDED page serves one distribution and says that more remain, and the resume point
    // it hands back continues the listing where it left off — the store being ordered by id.
    let mut ordered = ids.clone();
    ordered.sort();
    let page = &report["page"];
    assert_eq!(
        page["max_items"],
        json!(1),
        "case \"a bounded page\": expected the MaxItems the call asked for, got {page}"
    );
    assert_eq!(
        page["quantity"],
        json!(1),
        "case \"a bounded page\": expected one distribution on the page, got {page}"
    );
    assert_eq!(
        page["is_truncated"],
        json!(true),
        "case \"a bounded page\": expected the page to report that more remain, got {page}"
    );
    assert_eq!(
        texts(&page["ids"]),
        vec![ordered[0].clone()],
        "case \"a bounded page\": expected the first distribution by id, got {page}"
    );
    assert_eq!(
        page["next_marker"],
        json!(ordered[0]),
        "case \"a bounded page\": expected the id the page ended at as the resume point, got \
         {page}"
    );
    let resumed = &report["resumed"];
    assert_eq!(
        resumed["marker"],
        json!(ordered[0]),
        "case \"the resume after a bounded page\": expected the marker the call carried, got \
         {resumed}"
    );
    assert_eq!(
        texts(&resumed["ids"]),
        vec![ordered[1].clone()],
        "case \"the resume after a bounded page\": expected the next distribution by id, got \
         {resumed}"
    );

    // A read of a distribution the service does not hold is the SERVICE's own refusal, raised
    // as the SDK's client error with CloudFront's own code and status.
    assert_eq!(
        report["absent"]["code"],
        json!("NoSuchDistribution"),
        "case \"an id no distribution holds\": expected CloudFront's own refusal, got {}",
        report["absent"]
    );
    assert_eq!(
        report["absent"]["status"],
        json!(404),
        "case \"an id no distribution holds\": expected the status CloudFront answers it with, \
         got {}",
        report["absent"]
    );

    // An operation the seam binds no request for is reported as a call that was NOT MADE,
    // naming it and the operations that are bound — not sent with something guessed at.
    let unbound = report["unbound"].as_str().unwrap_or_default();
    assert!(
        unbound.contains(UNBOUND_OPERATION)
            && unbound.contains("ListDistributions")
            && unbound.contains("GetDistribution"),
        "case \"an operation the seam binds no request for\": expected a call that was not \
         made, naming it and the bound operations, got {unbound:?}"
    );

    // What the reads were FOR: the handler invalidated the one enabled distribution serving
    // the origin the event named, and the invalidation is CloudFront's own state from then on.
    let invalidated = &report["invalidated"];
    let expected = &created[1].id;
    assert_eq!(
        invalidated["distribution"],
        json!(expected),
        "case \"the distribution resolved from the listing\": expected the enabled \
         distribution serving that origin, got {invalidated}"
    );
    assert_eq!(
        texts(&invalidated["paths"]),
        PATHS
            .iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<String>>(),
        "case \"the invalidation of the distribution found\": expected the batch's own paths, \
         got {invalidated}"
    );
    let id = invalidated["id"].as_str().unwrap_or_default();
    assert!(
        !id.is_empty(),
        "case \"the invalidation of the distribution found\": expected an Id, got {invalidated}"
    );
    let read = cloudfront(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{expected}/invalidation/{id}"),
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case \"the invalidation of the distribution found\": GetInvalidation({id}) must \
         answer, got {} with {}",
        read.status, read.body
    );
    for path in PATHS {
        assert!(
            read.body.contains(&format!("<Path>{path}</Path>")),
            "case \"the invalidation of the distribution found\": CloudFront must hold the \
             path {path:?}, got {}",
            read.body
        );
    }
}
