// SPDX-License-Identifier: CC0-1.0

//! A bucket's ACCESS CONTROL as a stack declares it — its Object Ownership and its Block
//! Public Access settings — read back through Amazon S3 and applied to the ACL calls they
//! decide.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create. For an
//! `AWS::S3::Bucket` the whole `OwnershipControls` is ONE `PutBucketOwnershipControls` and the
//! whole `PublicAccessBlockConfiguration` ONE `PutPublicAccessBlock`, so a bucket a template
//! declares them for behaves exactly like one configured through those calls rather than like
//! one left on the settings every bucket is CREATED with.
//!
//! What makes that observable is the ACL decision the two settings take between them:
//!
//! * "Access control lists (ACLs) are disabled and no longer affect permissions … requests to
//!   set ACLs or update ACLs fail and return the `AccessControlListNotSupported` error code"
//!   (`PutObjectAcl`'s own documentation) — for `BucketOwnerEnforced` alone, which is also
//!   the setting a bucket created without any has, so a declaration of `ObjectWriter` or
//!   `BucketOwnerPreferred` that never reached S3 is indistinguishable from one.
//! * With ACLs ENABLED, the guardrail is the one Block Public Access states: "PUT Bucket ACL
//!   and PUT Object ACL calls fail if the specified ACL is public"
//!   (`PublicAccessBlockConfiguration$BlockPublicAcls`), which is `AccessDenied` (HTTP 403)
//!   naming the caller, the action, the resource and the setting — and nothing at all once the
//!   declaration turns that switch off.
//!
//! So each row puts BOTH a public and a private canned ACL on an object of its bucket: the
//! private one is the probe of what the ownership controls decide, and the public one of what
//! the Block Public Access switches do.
//!
//! The rows differ along what the rule generalises over: WHICH `ObjectOwnership` the
//! declaration names (each of the three the API publishes,
//! `smithy_query(query="shape",shape="ObjectOwnership")`, plus a bucket declaring none),
//! which Block Public Access switches it declares (all four on, two of the four on, one
//! switch alone, and none declared at all), and the template SPELLING (JSON and YAML).
//!
//! Every expected value is computed from what the row declares, never read back out of the
//! answer under test.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{create_stack, describe_stack, router, xml_text, REGION};
use crate::stack_resource_provisioning::output_value;
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The two canned ACLs every row puts on its object: `public-read` "grants the `AllUsers`
/// group READ access", which is what makes an ACL PUBLIC ("The Meaning of Public", Amazon S3
/// User Guide), and `private` grants "no one else has access rights" — the same ACL every
/// object already carries, so it is public-access-block's business not at all and only the
/// bucket's Object Ownership decides it.
const PUBLIC_CANNED_ACL: &str = "public-read";
const PRIVATE_CANNED_ACL: &str = "private";

/// The longest this suite waits for a stack to reach its terminal status, and how often it
/// looks. Generous: the wait is for a CONDITION the emulator settles on its own clock, so the
/// deadline only has to outlast a loaded machine, and nothing here asserts on elapsed time.
const DEPLOY_DEADLINE: Duration = Duration::from_secs(60);
const DEPLOY_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What one Amazon S3 request answered.
struct S3Answer {
    status: u16,
    body: String,
}

impl S3Answer {
    /// The `Code` of the error envelope the answer carries, or the empty string for an answer
    /// that is no refusal.
    fn error_code(&self) -> String {
        xml_text(&self.body, "Code").unwrap_or_default()
    }
}

/// One Amazon S3 request through the same router, carrying `headers` beside the signature.
///
/// The crate's shared S3 caller ([`crate::stack_resource_provisioning::s3`]) carries none, and
/// a canned ACL arrives in one (`x-amz-acl`), so this suite sends its own requests — the same
/// dispatch with the header list the case names.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> S3Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed S3 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    S3Answer { status, body }
}

/// Deploy a template and answer with the `DescribeStacks` body it completed with, looking
/// again until the stack reports a terminal status or [`DEPLOY_DEADLINE`] passes.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (http_status, created) = create_stack(router, stack_name, template, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
    );

    let started = Instant::now();
    let mut described = String::new();
    while started.elapsed() < DEPLOY_DEADLINE {
        let (http_status, body) = describe_stack(router, stack_name).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: DescribeStacks must answer 200, got {http_status} {body}"
        );
        described = body;
        if xml_text(&described, "StackStatus").as_deref() != Some(status::CREATE_IN_PROGRESS) {
            break;
        }
        tokio::time::sleep(DEPLOY_POLL_INTERVAL).await;
    }
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    described
}

/// The stack whose buckets declare each form of access control, in JSON.
const JSON_TEMPLATE: &str = r#"{"Resources":{
     "WriterBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-acl-writer",
       "OwnershipControls":{"Rules":[{"ObjectOwnership":"ObjectWriter"}]},
       "PublicAccessBlockConfiguration":{"BlockPublicAcls":true,"BlockPublicPolicy":true,
         "IgnorePublicAcls":true,"RestrictPublicBuckets":true}}},
     "PreferredBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-acl-preferred",
       "OwnershipControls":{"Rules":[{"ObjectOwnership":"BucketOwnerPreferred"}]},
       "PublicAccessBlockConfiguration":{"BlockPublicAcls":false,"BlockPublicPolicy":true,
         "IgnorePublicAcls":false,"RestrictPublicBuckets":true}}},
     "EnforcedBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-acl-enforced",
       "OwnershipControls":{"Rules":[{"ObjectOwnership":"BucketOwnerEnforced"}]}}},
     "PlainBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-acl-plain"}}},
   "Outputs":{
     "WriterBucketName":{"Value":{"Ref":"WriterBucket"}},
     "PreferredBucketName":{"Value":{"Ref":"PreferredBucket"}},
     "EnforcedBucketName":{"Value":{"Ref":"EnforcedBucket"}},
     "PlainBucketName":{"Value":{"Ref":"PlainBucket"}}}}"#;

/// The stack whose bucket declares the same ownership the first row does, in YAML, with the
/// ACL switch off: the template SPELLING is one of the dimensions the rule generalises over.
const YAML_TEMPLATE: &str = concat!(
    "Resources:\n",
    "  ArchiveBucket:\n",
    "    Type: AWS::S3::Bucket\n",
    "    Properties:\n",
    "      BucketName: stack-acl-archive\n",
    "      OwnershipControls:\n",
    "        Rules:\n",
    "          - ObjectOwnership: ObjectWriter\n",
    "      PublicAccessBlockConfiguration:\n",
    "        BlockPublicAcls: false\n",
    "Outputs:\n",
    "  ArchiveBucketName:\n",
    "    Value: !Ref ArchiveBucket\n",
);

/// One row of the table: the case, the stack output naming the bucket, the `ObjectOwnership`
/// its `GetBucketOwnershipControls` must report, the four Block Public Access switches
/// `GetPublicAccessBlock` must report, and how each of the two canned ACLs on one of its
/// objects is answered — the HTTP status and the error code, `None` for an ACL the bucket
/// takes.
struct Row {
    case: &'static str,
    output: &'static str,
    ownership: &'static str,
    switches: [bool; 4],
    public_acl: Option<(u16, &'static str)>,
    private_acl: Option<(u16, &'static str)>,
}

/// The four switches, in the order [`Row::switches`] states them, as
/// `PublicAccessBlockConfiguration` publishes them.
const SWITCHES: [&str; 4] = [
    "BlockPublicAcls",
    "BlockPublicPolicy",
    "IgnorePublicAcls",
    "RestrictPublicBuckets",
];

/// The key the one object of each bucket below is written under.
const SEEDED_KEY: &str = "report.csv";

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_access_control_it_declared() {
    let router = router();
    let json = deploy(
        &router,
        "four buckets, in JSON: each form of declared access control",
        "stack-acl-json",
        JSON_TEMPLATE,
    )
    .await;
    let yaml = deploy(
        &router,
        "one bucket, in YAML: ObjectWriter with the ACL switch off",
        "stack-acl-yaml",
        YAML_TEMPLATE,
    )
    .await;

    let rows: [(Row, &str); 5] = [
        (
            Row {
                case: "ObjectWriter with all four switches on",
                output: "WriterBucketName",
                ownership: "ObjectWriter",
                switches: [true, true, true, true],
                // ACLs are ENABLED, so the public ACL is refused for being public and the
                // private one is taken.
                public_acl: Some((403, "AccessDenied")),
                private_acl: None,
            },
            &json,
        ),
        (
            Row {
                case: "BucketOwnerPreferred with the ACL switches off",
                output: "PreferredBucketName",
                ownership: "BucketOwnerPreferred",
                switches: [false, true, false, true],
                public_acl: None,
                private_acl: None,
            },
            &json,
        ),
        (
            Row {
                case: "BucketOwnerEnforced, which disables ACLs",
                output: "EnforcedBucketName",
                ownership: "BucketOwnerEnforced",
                // No `PublicAccessBlockConfiguration` declared, so the bucket keeps the one
                // every bucket is created with: "Amazon S3 automatically enables S3 Block
                // Public Access … for new buckets".
                switches: [true, true, true, true],
                // Every ACL is refused on a bucket that allows none; the public one meets
                // the Block Public Access guardrail first, which this emulator judges ahead
                // of the bucket's own precondition.
                public_acl: Some((403, "AccessDenied")),
                private_acl: Some((400, "AccessControlListNotSupported")),
            },
            &json,
        ),
        (
            Row {
                case: "no access control declared at all",
                output: "PlainBucketName",
                // The setting a bucket is CREATED with: "by default, `ObjectOwnership` is
                // set to `BucketOwnerEnforced` and ACLs are disabled" — which is why a
                // declaration that never reaches S3 cannot be told from one.
                ownership: "BucketOwnerEnforced",
                switches: [true, true, true, true],
                public_acl: Some((403, "AccessDenied")),
                private_acl: Some((400, "AccessControlListNotSupported")),
            },
            &json,
        ),
        (
            Row {
                case: "ObjectWriter declared in YAML, with the ACL switch off",
                output: "ArchiveBucketName",
                ownership: "ObjectWriter",
                // `PutPublicAccessBlock` "creates or modifies the PublicAccessBlock
                // configuration" from the complete configuration it carries, so a
                // declaration stating ONE switch replaces the whole of what the bucket had:
                // the three it leaves out are not stated and so are not on.
                switches: [false, false, false, false],
                public_acl: None,
                private_acl: None,
            },
            &yaml,
        ),
    ];

    for (row, described) in &rows {
        let Row {
            case,
            output,
            ownership,
            switches,
            public_acl,
            private_acl,
        } = row;
        let bucket = output_value(described, output);
        assert!(
            !bucket.is_empty(),
            "case {case:?}: the stack must publish {output}, got {described}"
        );

        // The Object Ownership the stack installed, read through the call S3 publishes for it.
        let reported = s3(
            &router,
            "GET",
            &format!("/{bucket}?ownershipControls"),
            &[],
            b"",
        )
        .await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: GetBucketOwnershipControls({bucket:?}) must answer 200, got {} {}",
            reported.status, reported.body
        );
        assert_eq!(
            xml_text(&reported.body, "ObjectOwnership").as_deref(),
            Some(*ownership),
            "case {case:?}: {bucket:?} must report {ownership:?}, got {}",
            reported.body
        );

        // And the Block Public Access switches, read through the call S3 publishes for them.
        let blocked = s3(
            &router,
            "GET",
            &format!("/{bucket}?publicAccessBlock"),
            &[],
            b"",
        )
        .await;
        assert_eq!(
            blocked.status, 200,
            "case {case:?}: GetPublicAccessBlock({bucket:?}) must answer 200, got {} {}",
            blocked.status, blocked.body
        );
        for (switch, expected) in SWITCHES.iter().zip(switches) {
            assert_eq!(
                xml_text(&blocked.body, switch).as_deref(),
                Some(expected.to_string().as_str()),
                "case {case:?}: {bucket:?} must report {switch} {expected}, got {}",
                blocked.body
            );
        }

        // One object to put an ACL on, which the settings above decide.
        let written = s3(
            &router,
            "PUT",
            &format!("/{bucket}/{SEEDED_KEY}"),
            &[],
            b"region,total\nus-west-2,41\n",
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the seed write into {bucket:?} must answer 200, got {} {}",
            written.status, written.body
        );
        for (canned, expected) in [
            (PUBLIC_CANNED_ACL, public_acl),
            (PRIVATE_CANNED_ACL, private_acl),
        ] {
            let acl = s3(
                &router,
                "PUT",
                &format!("/{bucket}/{SEEDED_KEY}?acl"),
                &[("x-amz-acl", canned)],
                b"",
            )
            .await;
            match expected {
                Some((expected_status, expected_code)) => {
                    assert_eq!(
                        acl.status, *expected_status,
                        "case {case:?}/{canned}: the ACL on {bucket:?} must be refused with \
                         HTTP {expected_status}, got {} {}",
                        acl.status, acl.body
                    );
                    assert_eq!(
                        acl.error_code(),
                        *expected_code,
                        "case {case:?}/{canned}: expected {expected_code:?}, got {:?} ({})",
                        acl.error_code(),
                        acl.body
                    );
                }
                None => assert_eq!(
                    acl.status, 200,
                    "case {case:?}/{canned}: the ACL on {bucket:?} must be taken, got {} {}",
                    acl.status, acl.body
                ),
            }
        }
    }
}
