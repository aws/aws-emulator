// SPDX-License-Identifier: CC0-1.0

//! An S3 Batch Operations job SETTLES on the assembled, authenticating emulator, and what
//! it did to the objects it was given is there to read afterwards.
//!
//! The RULE under test: **a job `CreateJob` accepted with no confirmation required runs on
//! its own and reaches a TERMINAL `JobStatus` a caller polling `DescribeJob` observes —
//! `Complete` once its action has been carried out, AS the role the create named, against
//! every object its manifest lists, or `Failed` with a `FailureReasons` entry naming that
//! role when the account's IAM withholds `sts:AssumeRole` from
//! `batchoperations.s3.amazonaws.com`, in which case no object of its list is touched.** A
//! job asked for a completion report has it under `<Prefix>/job-<JobId>/` by the time it
//! reports that status. Sources: `JobDescriptor$RoleArn` ("the Identity and Access
//! Management (IAM) role that Batch Operations uses to run this job's operation on every
//! object in the manifest"), `JobDescriptor$FailureReasons` ("if the specified job failed,
//! this field contains information describing the failure") and
//! `smithy_query(query="shape", shape="com.amazonaws.s3control#JobStatus")` for the statuses
//! a job takes.
//!
//! Only the assembled router can pin this end of it: the job's own run reads its manifest
//! and writes its tags and its report through Amazon S3's own operations over the emulator's
//! in-process callback, the role it runs as is IAM's, and every call below — the client's and
//! the job's — is made against a router that VERIFIES request signatures, which is how the
//! emulator serves traffic. The s3control crate's own
//! `batch_job_terminal_status_and_report.rs` pins the same rule over a wider table of
//! manifests, prefixes and task outcomes on three hand-picked services with authentication
//! off; what only this suite reaches is the whole-router, authentication-on axis, which is
//! what a client of the served emulator actually drives.
//!
//! The rows span what the rule generalises over: WHAT the job's role trusts — the
//! batch-operations principal, that principal among several, a wildcard action covering the
//! assume, another service's principal alone, and an explicit deny beside the grant — which
//! is the one difference between a job that runs and one that cannot.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_emulator_router::{build_router, InMemoryEngine};
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_stack_resources::xml_texts;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account the S3 Control calls name, in `x-amz-account-id`: the caller's own, which is
/// the only account S3 Control authorizes a request for.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The XML namespace S3's own model declares, which a client's payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The XML namespace S3 Control's model declares.
const XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The service principal S3 Batch Operations assumes a job's role as, spelled out here
/// rather than imported from the service crate.
const BATCH_OPERATIONS_PRINCIPAL: &str = "batchoperations.s3.amazonaws.com";

/// The fixed text the reason of a job whose role could not be assumed opens with; the role
/// the create named is its variable part.
const ASSUME_ROLE_FAILURE_TEXT: &str = "Could not assume the IAM role";

/// The bucket every object of this suite lives in.
const BUCKET: &str = "xsvc-batch-operations";

/// The tag the job's action puts on every object its manifest lists.
const TAG_KEY: &str = "sim2real";
const TAG_VALUE: &str = "tagged";

/// How long a row waits for its job to settle: far above the span the service holds a job
/// before its run begins, plus room for the tasks themselves. Driven off the STATUS the job
/// reports rather than off a fixed sleep, so a slow host delays a row instead of failing it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);

/// One poll of `DescribeJob` inside [`SETTLE_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What one call answered.
struct Answer {
    status: u16,
    body: String,
    /// The `ETag` response header, for the write whose entity tag a manifest names.
    etag: String,
}

impl Answer {
    /// The text of the first `<name>` element of the body, or the empty string.
    fn text(&self, name: &str) -> String {
        xml_texts(&self.body, name).into_iter().next().unwrap_or_default()
    }
}

/// The emulator this suite drives, and the credential every call it makes is signed with:
/// the pair this process publishes once request authentication is on.
struct Emulator {
    router: MultiServiceRouter,
    key_id: String,
    secret: String,
}

impl Emulator {
    /// The whole emulator over one engine, serving as it serves traffic — with request
    /// authentication ON, so every call below, and every call the job's own run makes over
    /// the in-process callback, is verified.
    fn start() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(true);
        let published = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning authentication on publishes this process's keys");
        Self {
            router,
            key_id: published.access_key_id.clone(),
            secret: published.secret_access_key.clone(),
        }
    }

    /// Sign `request` for `signing_name` in [`REGION`], in the `Authorization` header an SDK
    /// request carries.
    fn signed(
        &self,
        mut request: http::Request<Vec<u8>>,
        signing_name: &str,
    ) -> http::Request<Vec<u8>> {
        let identity = Credentials::new(&self.key_id, &self.secret, None, None, "test").into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(REGION)
            .name(signing_name)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("the signing parameters are complete")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().expect("a text header").to_string(),
                )
            })
            .collect();
        let signable = SignableRequest::new(
            &method,
            &uri,
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(request.body()),
        )
        .expect("the request is signable");
        let (instructions, _) = sign(signable, &params)
            .expect("the request signs")
            .into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }

    /// Dispatch one signed request as it arrives on the wire.
    async fn send(
        &self,
        method: &str,
        host: &str,
        signing: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Answer {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let built = builder
            .body(body.as_bytes().to_vec())
            .expect("a well-formed request");
        let (parts, payload) = self.signed(built, signing).into_parts();
        let response = self
            .router
            .dispatch(http::Request::from_parts(
                parts,
                bytes::Bytes::from(payload),
            ))
            .await;
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("etag")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
            .unwrap_or_default();
        Answer {
            status,
            body: String::from_utf8_lossy(response.body()).into_owned(),
            etag,
        }
    }

    /// One Amazon S3 REST call, path-style as the emulator serves it.
    async fn s3(&self, method: &str, path: &str, body: &str) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        self.send(method, &host, "s3", path, &[], body).await
    }

    /// One S3 Control call for this account.
    async fn s3control(&self, method: &str, path: &str, body: &str) -> Answer {
        let host = format!("{ACCOUNT}.s3-control.{REGION}.amazonaws.com");
        self.send(
            method,
            &host,
            "s3",
            path,
            &[
                ("x-amz-account-id", ACCOUNT),
                ("content-type", "application/xml"),
            ],
            body,
        )
        .await
    }

    /// One IAM `awsQuery` call.
    async fn iam(&self, body: &str) -> Answer {
        self.send(
            "POST",
            "iam.amazonaws.com",
            "iam",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body,
        )
        .await
    }

    /// Poll `DescribeJob` until the job reports a terminal status, exactly as a client waits
    /// for one, and answer that answer's body.
    ///
    /// Waits for the CONDITION rather than for a span, under a deadline far above the span
    /// the service holds a job for.
    async fn await_terminal(&self, case: &str, job_id: &str) -> String {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = self
                .s3control("GET", &format!("/v20180820/jobs/{job_id}"), "")
                .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribeJob must answer 200, got {} with {}",
                described.status, described.body
            );
            let status = described.text("Status");
            if status == "Complete" || status == "Failed" || status == "Cancelled" {
                return described.body;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the job never reached a terminal status; last status \
                 {status:?} in {}",
                described.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One job the rule is asserted over.
struct Row {
    case: &'static str,
    /// The role's name, and the label its own object and manifest are keyed under.
    label: &'static str,
    /// The `Statement` list of the role's trust policy — the one thing the rows vary. It
    /// carries none of the characters that are significant in an
    /// `application/x-www-form-urlencoded` body, so the document travels as written.
    trust_statements: &'static str,
    /// Whether that document lets S3 Batch Operations assume the role, and so whether the
    /// job runs at all.
    runs: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the batch-operations principal",
            label: "granted",
            trust_statements: concat!(
                r#"{"Effect":"Allow","Principal":"#,
                r#"{"Service":"batchoperations.s3.amazonaws.com"},"Action":"sts:AssumeRole"}"#
            ),
            runs: true,
        },
        Row {
            case: "that principal among several",
            label: "granted-among-several",
            trust_statements: concat!(
                r#"{"Effect":"Allow","Principal":{"Service":"#,
                r#"["ec2.amazonaws.com","batchoperations.s3.amazonaws.com"]},"#,
                r#""Action":["sts:TagSession","sts:AssumeRole"]}"#
            ),
            runs: true,
        },
        Row {
            case: "a wildcard action covering the assume",
            label: "granted-by-wildcard",
            trust_statements: concat!(
                r#"{"Effect":"Allow","Principal":"#,
                r#"{"Service":"batchoperations.s3.amazonaws.com"},"Action":"sts:*"}"#
            ),
            runs: true,
        },
        Row {
            case: "another service's principal alone",
            label: "withheld",
            trust_statements: concat!(
                r#"{"Effect":"Allow","Principal":"#,
                r#"{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}"#
            ),
            runs: false,
        },
        Row {
            case: "an explicit deny beside the grant",
            label: "denied",
            trust_statements: concat!(
                r#"{"Effect":"Allow","Principal":"#,
                r#"{"Service":"batchoperations.s3.amazonaws.com"},"Action":"sts:AssumeRole"},"#,
                r#"{"Effect":"Deny","Principal":"#,
                r#"{"Service":"batchoperations.s3.amazonaws.com"},"Action":"sts:AssumeRole"}"#
            ),
            runs: false,
        },
    ]
}

/// The key of the object one row's job tags.
fn target_key(label: &str) -> String {
    format!("data/{label}.txt")
}

/// The key of the manifest one row's job reads.
fn manifest_key(label: &str) -> String {
    format!("manifests/{label}.csv")
}

#[test]
fn a_submitted_batch_job_settles_as_its_role_allows() {
    std::thread::Builder::new()
        .name("xsvc-batch-operations".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::start();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .build()
                .expect("a multi-thread runtime");

            runtime.block_on(async {
                let created = emulator
                    .s3(
                        "PUT",
                        &format!("/{BUCKET}"),
                        &format!(
                            "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
                             <LocationConstraint>{REGION}</LocationConstraint>\
                             </CreateBucketConfiguration>"
                        ),
                    )
                    .await;
                assert_eq!(
                    created.status, 200,
                    "setup: the bucket must be created, got {} with {}",
                    created.status, created.body
                );

                for row in rows() {
                    let case = row.case;
                    let label = row.label;

                    // The object the job's action is meant to reach, and the manifest that
                    // lists it: both written through Amazon S3 itself.
                    let target = emulator
                        .s3(
                            "PUT",
                            &format!("/{BUCKET}/{}", target_key(label)),
                            "the payload of the object this job tags",
                        )
                        .await;
                    assert_eq!(
                        target.status, 200,
                        "setup {case:?}: the target object must be written, got {} with {}",
                        target.status, target.body
                    );
                    let manifest = emulator
                        .s3(
                            "PUT",
                            &format!("/{BUCKET}/{}", manifest_key(label)),
                            &format!("{BUCKET},{}\n", target_key(label)),
                        )
                        .await;
                    assert_eq!(
                        manifest.status, 200,
                        "setup {case:?}: the manifest must be written, got {} with {}",
                        manifest.status, manifest.body
                    );

                    // The role the job runs as, created through IAM's own CreateRole: the
                    // rows differ in this document and in nothing else.
                    let role = emulator
                        .iam(&format!(
                            "Action=CreateRole&Version=2010-05-08&RoleName=batch-{label}\
                             &AssumeRolePolicyDocument=\
                             {{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
                            row.trust_statements
                        ))
                        .await;
                    assert_eq!(
                        role.status, 200,
                        "setup {case:?}: the role must be created, got {} with {}",
                        role.status, role.body
                    );
                    let role_arn = role.text("Arn");
                    assert!(
                        !role_arn.is_empty(),
                        "setup {case:?}: CreateRole must answer the role's ARN, got {}",
                        role.body
                    );

                    let submitted = emulator
                        .s3control(
                            "POST",
                            "/v20180820/jobs",
                            &format!(
                                "<CreateJobRequest xmlns=\"{XMLNS}\">\
                                 <ClientRequestToken>token-{label}</ClientRequestToken>\
                                 <ConfirmationRequired>false</ConfirmationRequired>\
                                 <Operation><S3PutObjectTagging><TagSet>\
                                 <member><Key>{TAG_KEY}</Key><Value>{TAG_VALUE}</Value>\
                                 </member></TagSet></S3PutObjectTagging></Operation>\
                                 <Report><Bucket>arn:aws:s3:::{BUCKET}</Bucket>\
                                 <Format>Report_CSV_20180820</Format><Enabled>true</Enabled>\
                                 <Prefix>reports/{label}</Prefix>\
                                 <ReportScope>AllTasks</ReportScope></Report>\
                                 <Manifest><Spec>\
                                 <Format>S3BatchOperations_CSV_20180820</Format>\
                                 <Fields><member>Bucket</member><member>Key</member></Fields>\
                                 </Spec><Location>\
                                 <ObjectArn>arn:aws:s3:::{BUCKET}/{}</ObjectArn>\
                                 <ETag>{}</ETag></Location></Manifest>\
                                 <Priority>1</Priority><RoleArn>{role_arn}</RoleArn>\
                                 </CreateJobRequest>",
                                manifest_key(label),
                                manifest.etag
                            ),
                        )
                        .await;
                    assert_eq!(
                        submitted.status, 200,
                        "case {case:?}: CreateJob must answer 200, got {} with {}",
                        submitted.status, submitted.body
                    );
                    let job_id = submitted.text("JobId");
                    assert!(
                        !job_id.is_empty(),
                        "case {case:?}: CreateJob must answer a JobId, got {}",
                        submitted.body
                    );

                    // Nothing else is called: the job runs on its own and the poll observes
                    // the status it settled in.
                    let settled = emulator.await_terminal(case, &job_id).await;
                    let status = xml_texts(&settled, "Status")
                        .into_iter()
                        .next()
                        .unwrap_or_default();
                    let expected = if row.runs { "Complete" } else { "Failed" };
                    assert_eq!(
                        status, expected,
                        "case {case:?}: expected the job to settle {expected}, got {settled}"
                    );

                    // What the job's run left behind in Amazon S3: the tag set of the object
                    // its manifest listed.
                    let tags = emulator
                        .s3(
                            "GET",
                            &format!("/{BUCKET}/{}?tagging", target_key(label)),
                            "",
                        )
                        .await;
                    assert_eq!(
                        tags.status, 200,
                        "case {case:?}: GetObjectTagging must answer 200, got {} with {}",
                        tags.status, tags.body
                    );
                    let tagged = xml_texts(&tags.body, "Key").contains(&TAG_KEY.to_string())
                        && xml_texts(&tags.body, "Value").contains(&TAG_VALUE.to_string());
                    assert_eq!(
                        tagged, row.runs,
                        "case {case:?}: expected the target to be tagged={}, got {}",
                        row.runs, tags.body
                    );

                    if row.runs {
                        // A job that ran accounts for every row of its object list, and the
                        // report it was asked for is an object under its own prefix.
                        assert!(
                            settled.contains("<TotalNumberOfTasks>1</TotalNumberOfTasks>")
                                && settled.contains(
                                    "<NumberOfTasksSucceeded>1</NumberOfTasksSucceeded>"
                                ),
                            "case {case:?}: the job must account for its one task, got {settled}"
                        );
                        let listed = emulator
                            .s3(
                                "GET",
                                &format!("/{BUCKET}?list-type=2&prefix=reports/{label}"),
                                "",
                            )
                            .await;
                        assert!(
                            listed.body.contains(&format!("reports/{label}/job-{job_id}/")),
                            "case {case:?}: the completion report must be written under the \
                             job's own prefix, got {}",
                            listed.body
                        );
                    } else {
                        // A job that could not be run says so, naming the role it could not
                        // assume — the one variable part of that reason.
                        let reason = xml_texts(&settled, "FailureReason")
                            .into_iter()
                            .next()
                            .unwrap_or_default();
                        assert!(
                            reason.contains(ASSUME_ROLE_FAILURE_TEXT) && reason.contains(&role_arn),
                            "case {case:?}: expected a reason carrying \
                             {ASSUME_ROLE_FAILURE_TEXT:?} and the role {role_arn:?}, got \
                             {settled}"
                        );
                        assert!(
                            reason.contains(BATCH_OPERATIONS_PRINCIPAL),
                            "case {case:?}: the reason must name the principal that could not \
                             assume the role, got {settled}"
                        );
                        assert!(
                            !settled.contains("<NumberOfTasksSucceeded>1<"),
                            "case {case:?}: a job that could not be run carried out no task, \
                             got {settled}"
                        );
                    }
                }
            });
        })
        .expect("spawn the batch-operations thread")
        .join()
        .expect("a submitted batch job settles as its role allows");
}
