// SPDX-License-Identifier: CC0-1.0

//! A stack's resources in the services that OWN them, end to end.
//!
//! RULE: CloudFormation does not only RECORD the resources a template declares — it
//! provisions them ("CloudFormation then makes the underlying service calls to provision
//! your resources"), so once a stack reports a resource of a provisionable type as
//! created, that resource exists in the service that owns it, and the physical resource id
//! the stack reports for it — the same value its `Ref` output publishes — names THAT
//! resource. Deleting the stack deletes it again ("deleting a stack deletes the resources
//! in it"), and a resource that could not be provisioned leaves the stack in the
//! `*_FAILED` status with the reason, never in `*_COMPLETE`.
//!
//! The provisionable type here is `AWS::S3::Bucket`, whose documented `Ref` return value
//! is the bucket name; `crate::state::provisioning` is the layer under test, and the
//! bucket names a template does not supply are CloudFormation's generated
//! `<stack>-<logical id>-<unique id>`, which for a bucket must satisfy Amazon S3's own
//! naming rules ("can consist only of lowercase letters, numbers, dots (.), and hyphens
//! (-)").
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the S3 calls as a client makes them — so what is asserted is what a client that
//! provisions a bucket by template and then writes into it observes.

use crate::hotswapped_function_code::package;
use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_text, call,
    member, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

/// What an S3 request answered: the status, the bound response headers, and the body for the
/// cases that read one.
pub(crate) struct S3Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: String,
}

impl S3Answer {
    /// One bound response header's value — what an object's encryption is reported in.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// Drive one S3 request through the same router, signed for `s3` the way a client's SDK
/// signs it (the harness verifies no signature, exactly as the emulator runs by default).
///
/// This crate's ONE S3 caller: a stack's buckets are read back as a client reads them.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> S3Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed S3 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    S3Answer {
        status,
        headers,
        body,
    }
}

/// Stage every Amazon S3 object a template names as the deployment package of a function or
/// the content of a layer, the way `sam package` and `cdk deploy` upload them before the
/// stack is submitted.
///
/// AWS Lambda READS the package out of the object the expanded `Code`/`Content` names, so the
/// object has to be there — and has to be a readable .zip archive — or the create is refused
/// and the stack rolls back. The locations are read off the template TEXT: every
/// `s3://<bucket>/<key>` it carries whose key names a .zip, which is the form `CodeUri` and
/// `ContentUri` take, so a suite stages exactly what its own template names and nothing else.
pub(crate) async fn stage_code_objects(router: &MultiServiceRouter, case: &str, template: &str) {
    for (bucket, key) in staged_locations(template) {
        let created = s3(router, "PUT", &format!("/{bucket}"), b"").await;
        assert!(
            (200..300).contains(&created.status),
            "case {case:?}: the bucket {bucket} must be created first, got {} {}",
            created.status,
            created.body
        );
        let archive = package("index.py", "def handler(event, context):\n    return {}\n");
        let staged = s3(router, "PUT", &format!("/{bucket}/{key}"), &archive).await;
        assert!(
            (200..300).contains(&staged.status),
            "case {case:?}: the archive {key} must be staged first, got {} {}",
            staged.status,
            staged.body
        );
    }
}

/// The `(bucket, key)` pairs [`stage_code_objects`] stages: every `s3://<bucket>/<key>` the
/// template text carries whose key names a .zip archive.
fn staged_locations(template: &str) -> Vec<(String, String)> {
    let mut staged: Vec<(String, String)> = Vec::new();
    for rest in template.split("s3://").skip(1) {
        let location: String = rest
            .chars()
            .take_while(|character| {
                !matches!(character, '"' | '\'' | ' ' | ',' | '}' | '\\' | '\n' | '\r')
            })
            .collect();
        let Some((bucket, key)) = location.split_once('/') else {
            continue;
        };
        if !key.ends_with(".zip") {
            continue;
        }
        let pair = (bucket.to_string(), key.to_string());
        if !staged.contains(&pair) {
            staged.push(pair);
        }
    }
    staged
}

/// `HEAD /<bucket>` — does S3 hold this bucket?
async fn head_bucket(router: &MultiServiceRouter, bucket: &str) -> u16 {
    s3(router, "HEAD", &format!("/{bucket}"), b"").await.status
}

/// The `OutputValue` the stack reports for `key`, or the empty string where it reports
/// none.
///
/// The one reader is `stack_lifecycle::output_value`, which takes the value INSIDE the
/// `Outputs` entry carrying that key; this is its total form, for the suites whose every
/// output resolves and which compare the value rather than its presence.
pub(crate) fn output_value(xml: &str, key: &str) -> String {
    crate::stack_lifecycle::output_value(xml, key).unwrap_or_default()
}

/// The `PhysicalResourceId` the stack reports for the resource `logical_id`.
///
/// Shared with the suites that read a provisioned resource back through its owning service by
/// the handle the stack publishes for it, so the "ask the stack, then call that service" step
/// is spelled once.
pub(crate) async fn physical_id(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack), member("LogicalResourceId", logical_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// One row of the provisioning table: the case, the stack name, the template, and the
/// `(logical id, output key)` pairs whose bucket must exist in S3 afterwards.
struct ProvisionCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    buckets: &'a [(&'a str, &'a str)],
}

#[tokio::test]
async fn a_bucket_a_stack_declares_exists_in_s3_once_the_stack_reports_it_created() {
    // The rows differ along what the rule generalises over: whether the template NAMES the
    // bucket or leaves CloudFormation to generate the name, the template spelling (JSON
    // and YAML, long- and short-form intrinsics), how MANY buckets one stack declares, and
    // whether the stack's own name carries characters an S3 bucket name may not (upper
    // case), which the generated name has to be held to.
    let cases: &[ProvisionCase<'_>] = &[
        ProvisionCase {
            // The issue's own shape: an unnamed `DataBucket` published as `!Ref`.
            case: "an unnamed bucket published by Ref, in YAML",
            stack_name: "sagemaker-data",
            template: concat!(
                "Resources:\n",
                "  DataBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref DataBucket\n",
            ),
            buckets: &[("DataBucket", "BucketName")],
        },
        ProvisionCase {
            case: "a template that names the bucket, in JSON",
            stack_name: "named-bucket",
            template: r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket",
                 "Properties":{"BucketName":"declared-training-data"}}},
                 "Outputs":{"BucketName":{"Value":{"Ref":"Data"}}}}"#,
            buckets: &[("Data", "BucketName")],
        },
        ProvisionCase {
            case: "two buckets in one stack, one named and one generated",
            stack_name: "two-buckets",
            template: concat!(
                "Resources:\n",
                "  Input:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: declared-input-data\n",
                "  Output:\n",
                "    Type: AWS::S3::Bucket\n",
                "Outputs:\n",
                "  InputBucket:\n",
                "    Value: !Ref Input\n",
                "  OutputBucket:\n",
                "    Value: !Ref Output\n",
            ),
            buckets: &[("Input", "InputBucket"), ("Output", "OutputBucket")],
        },
        ProvisionCase {
            // A stack name with upper case: a generated bucket name derived from it still
            // has to be a name S3 accepts, so it is lowercase.
            case: "a stack whose name carries upper case",
            stack_name: "MixedCaseStack",
            template: concat!(
                "Resources:\n",
                "  ModelArtifacts:\n",
                "    Type: AWS::S3::Bucket\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref ModelArtifacts\n",
            ),
            buckets: &[("ModelArtifacts", "BucketName")],
        },
    ];

    for case in cases {
        let router = router();
        let (status, created) = create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStacks must answer 200, got {status} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        for (logical_id, output_key) in case.buckets {
            let reported = physical_id(&router, case.stack_name, logical_id).await;
            let published = output_value(&described, output_key);
            assert_eq!(
                published, reported,
                "case {:?}: the {output_key} output must publish the physical id of \
                 {logical_id}, got {published:?} against {reported:?}",
                case.case
            );
            assert!(
                is_bucket_name(&published),
                "case {:?}: {published:?} must be a name Amazon S3 accepts",
                case.case
            );

            // The whole of the rule: the name the stack hands back is a bucket S3 HOLDS,
            // and a client can write into it and read it back — which is the workflow the
            // template exists for.
            assert_eq!(
                head_bucket(&router, &published).await,
                200,
                "case {:?}: S3 must hold the bucket {published:?} the stack reports",
                case.case
            );
            let written = s3(
                &router,
                "PUT",
                &format!("/{published}/train/train.csv"),
                b"sepal,petal\n1,2\n",
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {:?}: PutObject into {published:?} must answer 200, got {} {}",
                case.case, written.status, written.body
            );
            let read = s3(&router, "GET", &format!("/{published}/train/train.csv"), b"").await;
            assert_eq!(
                read.status, 200,
                "case {:?}: GetObject from {published:?} must answer 200, got {} {}",
                case.case, read.status, read.body
            );
            assert_eq!(
                read.body, "sepal,petal\n1,2\n",
                "case {:?}: the object written into {published:?} must read back, got {:?}",
                case.case, read.body
            );
        }
    }
}

/// A bucket the caller ALREADY OWNS, under the name a template declares: "for legacy
/// compatibility, if you re-create an existing bucket that you already own in the
/// us-east-1 Region, Amazon S3 returns 200 OK" (`CreateBucket`), which is the Region these
/// cases are signed for — so the create is not a refusal there, and what the rule forbids
/// is the deployment re-creating or clearing what is already in that bucket.
///
/// Read with the pre-deployment validations OFF, since that answer is Amazon S3's: with them
/// on, the existence check fails a create whose declared name the owning service already
/// holds before that service is asked at all (`state::early_validation`).
#[tokio::test]
async fn a_bucket_name_the_caller_already_owns_is_neither_re_created_nor_cleared() {
    let router = router();
    // A bucket the stack did not create, under the name its template declares.
    let taken = s3(&router, "PUT", "/already-taken-bucket", b"").await;
    assert_eq!(
        taken.status, 200,
        "the bucket must be created outside the stack first, got {} {}",
        taken.status, taken.body
    );
    let seeded = s3(
        &router,
        "PUT",
        "/already-taken-bucket/someone-elses.csv",
        b"a,b\n",
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "the object in it must be stored first, got {} {}",
        seeded.status, seeded.body
    );

    let template = r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket",
         "Properties":{"BucketName":"already-taken-bucket"}}}}"#;
    let (status, created) = create_stack(
        &router,
        "collides",
        template,
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateStack still answers 200 — the creation starts, and the stack reports the \
         outcome — got {status} {created}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "collides").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "a bucket the caller already owns is what S3 answers 200 for in this Region, so \
         the stack completes, got {described}"
    );

    // And the bucket that WAS there is untouched: the deployment created nothing over it
    // and cleared nothing out of it.
    assert_eq!(
        head_bucket(&router, "already-taken-bucket").await,
        200,
        "the bucket that was already there must still be there"
    );
    let survived = s3(
        &router,
        "GET",
        "/already-taken-bucket/someone-elses.csv",
        b"",
    )
    .await;
    assert_eq!(
        survived.status, 200,
        "the object in the bucket must be untouched, got {} {}",
        survived.status, survived.body
    );
    assert_eq!(
        survived.body, "a,b\n",
        "the object in the bucket must still carry its own bytes, got {:?}",
        survived.body
    );
}

#[tokio::test]
async fn deleting_a_stack_deletes_the_buckets_it_created() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Data:\n",
        "    Type: AWS::S3::Bucket\n",
        "Outputs:\n",
        "  BucketName:\n",
        "    Value: !Ref Data\n",
    );
    create_stack(&router, "deleted-stack", template, &[]).await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "deleted-stack").await;
    let bucket = output_value(&described, "BucketName");
    assert_eq!(
        head_bucket(&router, &bucket).await,
        200,
        "the stack's bucket must exist before the delete"
    );

    let (status, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "deleted-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteStack must answer 200, got {status} {deleted}"
    );
    wait_for_the_transition().await;

    assert_eq!(
        head_bucket(&router, &bucket).await,
        404,
        "deleting the stack must delete the bucket it created"
    );

    // A stack whose bucket still holds an object cannot be deleted — S3 refuses to delete
    // a bucket that is not empty — so the stack reports DELETE_FAILED and stays, rather
    // than reporting a deletion that did not happen.
    create_stack(&router, "stuck-stack", template, &[]).await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "stuck-stack").await;
    let bucket = output_value(&described, "BucketName");
    let written = s3(&router, "PUT", &format!("/{bucket}/train.csv"), b"a,b\n").await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} {}",
        written.status, written.body
    );

    call(
        &router,
        "DeleteStack",
        &[member("StackName", "stuck-stack")],
    )
    .await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "stuck-stack").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_FAILED),
        "a bucket that could not be deleted must leave the stack DELETE_FAILED, got \
         {described}"
    );
    assert_eq!(
        head_bucket(&router, &bucket).await,
        200,
        "the bucket the delete could not remove must still be there"
    );
}

#[tokio::test]
async fn an_update_provisions_what_the_new_template_adds_and_releases_what_it_drops() {
    let router = router();
    let before = concat!(
        "Resources:\n",
        "  Kept:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: update-kept-bucket\n",
        "  Dropped:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: update-dropped-bucket\n",
    );
    // The same stack, one bucket removed and one added.
    let after = concat!(
        "Resources:\n",
        "  Kept:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: update-kept-bucket\n",
        "  Added:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: update-added-bucket\n",
    );

    create_stack(&router, "updated-stack", before, &[]).await;
    wait_for_the_transition().await;
    for bucket in ["update-kept-bucket", "update-dropped-bucket"] {
        assert_eq!(
            head_bucket(&router, bucket).await,
            200,
            "the create must have provisioned {bucket}"
        );
    }

    let (status, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated-stack"),
            member("TemplateBody", after),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateStack must answer 200, got {status} {updated}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "updated-stack").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "the update must complete, got {described}"
    );

    // (case, bucket, the status S3 must answer for it)
    for (case, bucket, expected) in [
        ("the bucket the update added", "update-added-bucket", 200),
        ("the bucket both templates declare", "update-kept-bucket", 200),
        (
            "the bucket the new template no longer declares",
            "update-dropped-bucket",
            404,
        ),
    ] {
        assert_eq!(
            head_bucket(&router, bucket).await,
            expected,
            "case {case:?}: expected S3 to answer {expected} for {bucket}"
        );
    }
}

/// Whether `name` satisfies Amazon S3's bucket naming rules for the characters a
/// CloudFormation-generated name can produce: "Bucket names can consist only of lowercase
/// letters, numbers, dots (.), and hyphens (-)", "must begin and end with a letter or
/// number" and "must be between 3 (min) and 63 (max) characters long".
fn is_bucket_name(name: &str) -> bool {
    let shaped = (3..=63).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    let ends = |c: Option<char>| c.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    shaped && ends(name.chars().next()) && ends(name.chars().next_back())
}
