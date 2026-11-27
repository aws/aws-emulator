// SPDX-License-Identifier: CC0-1.0

//! The SageMaker Feature Store hand-off for a feature group whose OFFLINE STORE cannot be
//! created: the control plane REFUSES the create, and neither plane holds the group.
//!
//! RULE: a feature group's offline store is set up and written AS the `RoleArn` its create
//! supplied ("the IAM execution role used to persist data into the OfflineStore"), so
//! SageMaker probes that location with that role before it admits the group: where the
//! permissions in force do not allow the write the create is REFUSED at the call site — HTTP
//! 400 `ValidationException` naming the Amazon S3 access-denied cause — and nothing is stored,
//! so the control plane answers `ResourceNotFound` for the name and the FeatureStore Runtime
//! data plane resolves no group under it either. The same create made with the write permitted
//! is admitted, settles `Created`, and its records are written and read.
//!
//! The four services the rule spans are mounted on ONE shared engine, exactly as
//! `core/router/src/lib.rs` mounts them: the SageMaker control plane that creates the group,
//! IAM which holds the role and answers what it may do, Amazon S3 which holds the destination
//! bucket, and the FeatureStore Runtime data plane that serves the records. The permission is
//! never guessed here — it is whatever this emulator's own IAM answers about the role the
//! create named.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite's own form encoding would drift from every other one here: `query_wire` holds
// the tree's one `application/x-www-form-urlencoded` component encoder.
use crate::query_wire::form_encode;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The service principal a SageMaker execution role's trust document admits.
const SERVICE_PRINCIPAL: &str = "sagemaker.amazonaws.com";

/// The IAM action an object write is authorized under.
const WRITE_ACTION: &str = "s3:PutObject";

/// The `FeatureGroupStatus` a group whose offline store could be created settles in.
const CREATED: &str = "Created";

/// The error code a refusal neither of `CreateFeatureGroup`'s two modelled errors states
/// travels as.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code the control plane reports a feature group this account does not hold with.
const NOT_FOUND: &str = "ResourceNotFound";

/// How long a case waits for the terminal status the clock already owes a created group.
///
/// GENEROUS on purpose: what is under test is WHICH status the group settles in, never how
/// fast it gets there, so each case polls the reader for the condition it needs and this is
/// only the deadline that stops a broken emulator from hanging the suite.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long a case waits between two reads while polling for a settled status.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential: this
/// suite turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The four services on one shared engine, as the production router mounts them.
struct Aws {
    router: MultiServiceRouter,
}

/// One answer: the status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code: restJson1 and awsJson1_1 both carry it as the body's `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// The error message an answer carries, whichever casing the envelope spells it with.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request through the whole router, signed for `service`.
    async fn send(
        &self,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Bytes>,
        content_type: &str,
    ) -> Answer {
        let host = if service == "iam" {
            "iam.amazonaws.com".to_owned()
        } else {
            format!("{service}.{REGION}.amazonaws.com")
        };
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", content_type)
            .header("authorization", authorization(service));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(body.unwrap_or_default())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null),
        }
    }

    /// One SageMaker control-plane call, in its awsJson1_1 RPC form.
    async fn sagemaker(&self, target: &str, body: &Value) -> Answer {
        self.send(
            "sagemaker",
            "POST",
            "/",
            &[("x-amz-target", &format!("SageMaker.{target}"))],
            Some(Bytes::from(body.to_string())),
            "application/x-amz-json-1.1",
        )
        .await
    }

    /// One SageMaker control-plane call that must be served.
    async fn sagemaker_ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let answer = self.sagemaker(target, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {target} to be served, got {} with {}",
            answer.status, answer.body
        );
        answer.body
    }

    /// One IAM call (`awsQuery`) that must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let answer = self
            .send(
                "iam",
                "POST",
                "/",
                &[],
                Some(Bytes::from(body)),
                "application/x-www-form-urlencoded",
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected IAM {action} to be served, got {} with {}",
            answer.status, answer.body
        );
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    async fn given_bucket(&self, case: &str, bucket: &str) {
        let answer = self
            .send("s3", "PUT", &format!("/{bucket}"), &[], None, "application/xml")
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the bucket {bucket} to be created, got {}",
            answer.status
        );
    }

    /// Hold `role_name` in IAM, trusted by the SageMaker service principal, and answer its
    /// ARN.
    async fn given_role(&self, case: &str, role_name: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", role_name.to_owned()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        format!(
            "arn:aws:iam::{}:role/{role_name}",
            engine::emulator_account_id()
        )
    }

    /// Grant `role_name` the object write on `resource`, as its one inline policy.
    async fn given_write_grant(&self, case: &str, role_name: &str, resource: &str) {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": WRITE_ACTION,
                "Resource": resource,
            }],
        })
        .to_string();
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_owned()),
                ("PolicyName", "offline-store".to_owned()),
                ("PolicyDocument", document),
            ],
        )
        .await;
    }

    /// `CreateFeatureGroup` with an offline store at `s3_uri`, written as `role_arn`.
    async fn create_feature_group(&self, group: &str, s3_uri: &str, role_arn: &str) -> Answer {
        self.sagemaker(
            "CreateFeatureGroup",
            &json!({
                "FeatureGroupName": group,
                "RecordIdentifierFeatureName": "record_id",
                "EventTimeFeatureName": "event_time",
                "FeatureDefinitions": [
                    { "FeatureName": "record_id", "FeatureType": "String" },
                    { "FeatureName": "event_time", "FeatureType": "String" },
                    { "FeatureName": "feature_a", "FeatureType": "String" },
                ],
                "OnlineStoreConfig": { "EnableOnlineStore": true },
                // `DisableGlueTableCreation`: this suite manages no Glue table.
                "OfflineStoreConfig": {
                    "S3StorageConfig": { "S3Uri": s3_uri },
                    "DisableGlueTableCreation": true,
                },
                "RoleArn": role_arn,
            }),
        )
        .await
    }

    /// `DescribeFeatureGroup` as it answers, whether or not the group is held.
    async fn describe_feature_group(&self, group: &str) -> Answer {
        self.sagemaker(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": group }),
        )
        .await
    }

    /// `DescribeFeatureGroup`, polled until the group's `FeatureGroupStatus` is one the clock
    /// no longer owes a transition from.
    async fn settled(&self, case: &str, group: &str) -> Value {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = self
                .sagemaker_ok(
                    case,
                    "DescribeFeatureGroup",
                    &json!({ "FeatureGroupName": group }),
                )
                .await;
            let status = described
                .get("FeatureGroupStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if status == CREATED {
                return described;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the group never reached {CREATED}, last read {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `PutRecord` on the data plane.
    async fn put_record(&self, group: &str) -> Answer {
        self.send(
            "sagemaker",
            "PUT",
            &format!("/FeatureGroup/{group}"),
            &[],
            Some(Bytes::from(
                json!({
                    "Record": [
                        { "FeatureName": "record_id", "ValueAsString": "R1" },
                        { "FeatureName": "event_time", "ValueAsString": "2026-06-01T07:08:09Z" },
                        { "FeatureName": "feature_a", "ValueAsString": "first" },
                    ],
                })
                .to_string(),
            )),
            "application/json",
        )
        .await
    }

    /// `GetRecord` on the data plane.
    async fn get_record(&self, group: &str) -> Answer {
        self.send(
            "sagemaker",
            "GET",
            &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString=R1"),
            &[],
            None,
            "application/json",
        )
        .await
    }
}

/// RULE: the create is REFUSED exactly when the permissions in force do not let its role
/// write the offline location — HTTP 400 `ValidationException` naming the access-denied cause,
/// with neither plane left holding the group — while the same create made with that write
/// granted is admitted, settles `Created`, and its records are written and read.
#[tokio::test]
async fn a_feature_group_whose_offline_store_cannot_be_created_is_refused_in_both_planes() {
    // (case, whether the role is granted the object write on the group's own bucket, whether
    //  the create must be refused)
    let rows = [
        ("a role granted the write on the offline store", true, false),
        ("a role carrying no policy at all", false, true),
    ];

    for (case, granted, must_refuse) in rows {
        let aws = Aws::new();
        let bucket = if granted {
            "offline-store-granted"
        } else {
            "offline-store-ungranted"
        };
        let group = if granted {
            "offlinegranted"
        } else {
            "offlineungranted"
        };
        aws.given_bucket(case, bucket).await;
        let role_name = format!("{group}-role");
        let role_arn = aws.given_role(case, &role_name).await;
        if granted {
            aws.given_write_grant(case, &role_name, &format!("arn:aws:s3:::{bucket}/*"))
                .await;
        }

        let s3_uri = format!("s3://{bucket}/offline/");
        let created = aws.create_feature_group(group, &s3_uri, &role_arn).await;

        if must_refuse {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (400, VALIDATION_EXCEPTION),
                "case {case:?}: expected the create refused, got {} with {}",
                created.status,
                created.body
            );
            let refusal = created.message();
            for expected in ["AccessDenied", role_arn.as_str(), WRITE_ACTION, s3_uri.as_str()] {
                assert!(
                    refusal.contains(expected),
                    "case {case:?}: expected the refusal to name {expected:?}, got {refusal:?}"
                );
            }
            // NEITHER plane holds the group: the control plane never stored it, so the name is
            // free, and the data plane — which resolves a group through the directory that
            // create registers — resolves nothing under it.
            let absent = aws.describe_feature_group(group).await;
            assert_eq!(
                (absent.status, absent.error_code().as_str()),
                (400, NOT_FOUND),
                "case {case:?}: a refused create stores nothing, got {} with {}",
                absent.status,
                absent.body
            );
            for (call, answer) in [
                ("PutRecord", aws.put_record(group).await),
                ("GetRecord", aws.get_record(group).await),
            ] {
                let refusal = answer.message();
                assert!(
                    refusal.contains("can't find a FeatureGroup"),
                    "case {case:?}: expected {call} to resolve no group under {group:?}, got {} \
                     {:?} with {}",
                    answer.status,
                    answer.error_code(),
                    answer.body
                );
            }
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create admitted, got {} with {}",
            created.status, created.body
        );
        let described = aws.settled(case, group).await;
        assert_eq!(
            described.get("FailureReason"),
            None,
            "case {case:?}: expected no FailureReason, got {described}"
        );
        let put = aws.put_record(group).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutRecord to be served, got {} with {}",
            put.status, put.body
        );
        let got = aws.get_record(group).await;
        let served = got
            .body
            .get("Record")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            (got.status, served),
            (200, 3),
            "case {case:?}: expected GetRecord to serve the three features written, got {} \
             with {}",
            got.status,
            got.body
        );
    }
}
