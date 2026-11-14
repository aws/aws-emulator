// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model registry: `CreateModelPackage` and the
//! `DescribeModelPackage` that replays what it stored.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for both kinds of model package in every region and name shape; the
//! create/describe round trip for every optional member; version numbering inside a
//! group; the refusals (a name already in use, a group that does not exist, the two
//! mutually exclusive kind members, an identifier no package answers to); the
//! idempotency-token record; the `Pending` -> `Completed` transition the clock owes; and
//! the model-card sanitization `IncludedData=MetadataOnly` asks for.
//!
//! A versioned package needs its group to exist first, so the versioned cases create it
//! through `CreateModelPackageGroup` over the same wire.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::service_resolved_members::{
    materialised_additional_inference_specifications, materialised_inference_specification,
};

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How long a poll below gives the clock to settle a registration: far longer than
/// `state::MODEL_PACKAGE_COMPLETED_AFTER`, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often such a poll looks again. Only the loop's condition decides when it stops.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A container the boundary validator accepts for an `InferenceSpecification`.
pub(crate) fn container(image: &str) -> Value {
    json!({"Image": image, "ModelDataUrl": "s3://models/artifact.tar.gz"})
}

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Create the model package group a versioned registration joins, through the API
    /// that owns it, and return its ARN.
    pub(crate) async fn create_group(&self, region: &str, name: &str) -> String {
        let (status, created) = self
            .call_in(
                region,
                "CreateModelPackageGroup",
                &json!({"ModelPackageGroupName": name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from CreateModelPackageGroup, got {status} \
             with {created}"
        );
        str_field(&created, "ModelPackageGroupArn")
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in(state::DEFAULT_REGION, target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    pub(crate) async fn ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }

    /// Register a model package and return its ARN.
    pub(crate) async fn create(&self, case: &str, body: &Value) -> String {
        let output = self.ok(case, "CreateModelPackage", body).await;
        str_field(&output, "ModelPackageArn")
    }

    /// Describe `identifier` (a name, a `name/version` pair or an ARN).
    pub(crate) async fn describe(&self, case: &str, identifier: &str) -> Value {
        self.ok(
            case,
            "DescribeModelPackage",
            &json!({"ModelPackageName": identifier}),
        )
        .await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {member} in {body}"))
        .to_string()
}

/// The ARN form every model package carries:
/// `arn:aws:sagemaker:<region>:<account>:model-package/<name>[/<version>]`.
pub(crate) fn expected_arn(region: &str, resource: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-package/{resource}")
}

/// `ModelPackageStatusDetails.ValidationStatuses`, treating an omitted list as empty.
fn validation_statuses(describe: &Value) -> Vec<Value> {
    describe
        .get("ModelPackageStatusDetails")
        .and_then(|details| details.get("ValidationStatuses"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A `ValidationSpecification` with `profiles` profiles, so a case can vary how many
/// per-profile validation items the status details must carry.
pub(crate) fn validation_specification(profiles: usize) -> Value {
    let profiles: Vec<Value> = (0..profiles)
        .map(|index| {
            json!({
                "ProfileName": format!("profile{index}"),
                "TransformJobDefinition": {
                    "TransformInput": {
                        "DataSource": {
                            "S3DataSource": {
                                "S3DataType": "S3Prefix",
                                "S3Uri": "s3://validation/input/"
                            }
                        }
                    },
                    "TransformOutput": {"S3OutputPath": "s3://validation/output/"},
                    "TransformResources": {
                        "InstanceType": "ml.m5.large",
                        "InstanceCount": 1
                    }
                }
            })
        })
        .collect();
    json!({
        "ValidationRole": "arn:aws:iam::000000000000:role/SageMakerValidation",
        "ValidationProfiles": profiles,
    })
}

/// The `ModelPackageStatus` a registration carrying `body` is in the moment
/// `CreateModelPackage` answers: `Pending` while the batch transform jobs its
/// `ValidationSpecification` names are still to run, and `Completed` for a registration
/// with no validation to run at all — either because it named no profile or because
/// `SkipModelValidation` is `All`.
///
/// The oracle the suites below read a freshly registered package against, so a row varying
/// the validation it asks for carries its own expectation.
pub(crate) fn status_at_registration(body: &Value) -> &'static str {
    let profiles = body
        .pointer("/ValidationSpecification/ValidationProfiles")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let validation_skipped =
        body.get("SkipModelValidation").and_then(Value::as_str) == Some("All");
    if profiles > 0 && !validation_skipped {
        "Pending"
    } else {
        "Completed"
    }
}

/// A create body: `members` plus whatever the row adds.
fn body_with(members: &Value, extras: &Value) -> Value {
    let mut body = members.as_object().cloned().unwrap_or_default();
    for (key, value) in extras.as_object().cloned().unwrap_or_default() {
        body.insert(key, value);
    }
    Value::Object(body)
}

/// One round-trip row: the optional members the request carries, and the members
/// `DescribeModelPackage` does NOT replay (so the echo check skips them).
struct RoundTrip {
    case: &'static str,
    /// `None` signs no region, so the ARN takes the emulator default.
    region: Option<&'static str>,
    name: &'static str,
    extras: Value,
    not_echoed: &'static [&'static str],
}

/// RULE: an unversioned model package is stored under the `ModelPackageName` the request
/// named, answers with `arn:aws:sagemaker:<region>:<account>:model-package/<name>` built
/// from the region the caller signed for, and `DescribeModelPackage` replays every
/// member the request carried, unchanged, for every member and name shape.
#[tokio::test]
async fn unversioned_create_describe_round_trip_over_optional_members_and_regions() {
    let rows = vec![
        RoundTrip {
            case: "minimal, no optional member",
            region: Some("us-east-1"),
            name: "a",
            extras: json!({}),
            not_echoed: &[],
        },
        RoundTrip {
            case: "description, domain and task, non-ASCII description",
            region: Some("us-west-2"),
            name: "detector-v2",
            extras: json!({
                "ModelPackageDescription": "Détection ✓ (naïve) — 1024",
                "Domain": "COMPUTER_VISION",
                "Task": "IMAGE_CLASSIFICATION",
            }),
            not_echoed: &[],
        },
        RoundTrip {
            case: "inference specification with two containers",
            region: Some("eu-central-1"),
            name: "pipeline-model",
            extras: json!({
                "InferenceSpecification": {
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/a:1"),
                                   container("123.dkr.ecr.us-east-1.amazonaws.com/b@sha256:0")],
                    "SupportedTransformInstanceTypes": ["ml.m5.large", "ml.c5.xlarge"],
                    "SupportedRealtimeInferenceInstanceTypes": ["ml.m5.large"],
                    "SupportedContentTypes": ["application/json", "text/csv"],
                    "SupportedResponseMIMETypes": ["application/json"],
                },
                "CertifyForMarketplace": true,
            }),
            not_echoed: &[],
        },
        RoundTrip {
            case: "tags and customer metadata; tags are replayed by ListTags, not here",
            region: Some("ap-southeast-2"),
            name: "tagged-package",
            extras: json!({
                "Tags": [{"Key": "team", "Value": "ml"}, {"Key": "cost-centre", "Value": ""}],
                "CustomerMetadataProperties": {"framework": "pytorch", "commit": "abc123"},
                "MetadataProperties": {"CommitId": "abc123", "Repository": "models"},
            }),
            not_echoed: &["Tags"],
        },
        RoundTrip {
            case: "metrics, drift baselines and additional inference specifications",
            region: Some("us-east-2"),
            name: "metrics-package",
            extras: json!({
                "ModelMetrics": {
                    "ModelQuality": {
                        "Statistics": {"ContentType": "application/json",
                                       "S3Uri": "s3://metrics/stats.json"}
                    }
                },
                "DriftCheckBaselines": {
                    "ModelDataQuality": {
                        "Constraints": {"ContentType": "application/json",
                                        "S3Uri": "s3://metrics/constraints.json"}
                    }
                },
                "AdditionalInferenceSpecifications": [{
                    "Name": "neo-compiled",
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/neo:1")],
                }],
            }),
            not_echoed: &[],
        },
        RoundTrip {
            case: "registration type, storage type, skip validation and life cycle",
            region: Some("eu-west-1"),
            name: "lifecycle-package",
            extras: json!({
                "ModelPackageRegistrationType": "Registered",
                "ManagedStorageType": "Restricted",
                "SkipModelValidation": "All",
                "SourceUri": "s3://source/model.tar.gz",
                "SamplePayloadUrl": "s3://payloads/sample.tar.gz",
                "SecurityConfig": {"KmsKeyId": "alias/models"},
                "ModelLifeCycle": {"Stage": "Development", "StageStatus": "InProgress",
                                   "StageDescription": "smoke tested"},
            }),
            not_echoed: &[],
        },
        RoundTrip {
            case: "validation specification with one profile",
            region: None,
            name: "validated-package",
            extras: json!({"ValidationSpecification": validation_specification(1)}),
            not_echoed: &[],
        },
        RoundTrip {
            case: "63-character name, the longest EntityName allows",
            region: None,
            name: "n0123456789012345678901234567890123456789012345678901234567890a",
            extras: json!({}),
            not_echoed: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let body = body_with(&json!({"ModelPackageName": row.name}), &row.extras);
        let (status, created) = match row.region {
            Some(region) => emulator.call_in(region, "CreateModelPackage", &body).await,
            None => emulator.call_unsigned("CreateModelPackage", &body).await,
        };
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateModelPackage, got {status} \
             with {created}"
        );

        let region = row.region.unwrap_or(state::DEFAULT_REGION);
        let arn = str_field(&created, "ModelPackageArn");
        assert_eq!(
            arn,
            expected_arn(region, row.name),
            "case {case:?}: expected the model-package ARN for region {region}, got {arn:?}"
        );

        let described = emulator.describe(case, row.name).await;
        assert_eq!(
            str_field(&described, "ModelPackageName"),
            row.name,
            "case {case:?}: expected the stored name, got {described}"
        );
        assert_eq!(
            str_field(&described, "ModelPackageArn"),
            arn,
            "case {case:?}: expected the created ARN replayed, got {described}"
        );
        // An unversioned package belongs to no group and carries no version.
        for absent in ["ModelPackageGroupName", "ModelPackageVersion"] {
            assert!(
                described.get(absent).is_none_or(Value::is_null),
                "case {case:?}: an unversioned package must report no {absent}, got {described}"
            );
        }
        assert!(
            described.get("CreationTime").is_some(),
            "case {case:?}: expected a CreationTime, got {described}"
        );
        // A registration with validation jobs still to run is reported in the
        // non-terminal status; one with no validation to run is registered complete.
        let expected_status = status_at_registration(&body);
        assert_eq!(
            str_field(&described, "ModelPackageStatus"),
            expected_status,
            "case {case:?}: expected a freshly registered package to be \
             {expected_status}, got {described}"
        );
        // `CreatedBy` names the IAM identity the registering request signed with; an
        // unsigned request presents no access-key id, so `extract_akid`'s fallback
        // principal is recorded instead.
        let principal = if row.region.is_some() { "AKIDEXAMPLE" } else { "default" };
        assert_eq!(
            described
                .get("CreatedBy")
                .and_then(|created_by| created_by.get("IamIdentity"))
                .and_then(|identity| identity.get("Arn"))
                .and_then(Value::as_str),
            Some(format!("arn:aws:iam::{ACCOUNT}:user/{principal}").as_str()),
            "case {case:?}: expected CreatedBy to name the signing identity, got {described}"
        );
        assert!(
            described.get("LastModifiedBy").is_none(),
            "case {case:?}: nothing has modified the package, so no modifying identity \
             is reported, got {described}"
        );

        // Every member the request carried that `DescribeModelPackage` reports comes
        // back byte-for-byte.
        for (member, sent) in row.extras.as_object().cloned().unwrap_or(Map::new()) {
            if row.not_echoed.contains(&member.as_str()) {
                assert!(
                    described.get(&member).is_none(),
                    "case {case:?}: {member} is not a DescribeModelPackage member, \
                     got {described}"
                );
                continue;
            }
            // An inference specification comes back with the members the SERVICE resolves
            // for its containers (their image digests, their checkpoint flag) laid over
            // what the request sent; every other member is replayed byte-for-byte.
            let expected = match member.as_str() {
                "InferenceSpecification" => materialised_inference_specification(&sent),
                "AdditionalInferenceSpecifications" => {
                    materialised_additional_inference_specifications(&sent)
                }
                _ => sent.clone(),
            };
            let replayed = described.get(&member);
            assert_eq!(
                replayed,
                Some(&expected),
                "case {case:?}: expected {member} replayed as {expected}, got {replayed:?}"
            );
        }

        // `ModelApprovalStatus` does not apply to unversioned packages, and nothing
        // defaults it in.
        assert!(
            described.get("ModelApprovalStatus").is_none(),
            "case {case:?}: an unversioned package gets no ModelApprovalStatus, \
             got {described}"
        );
    }
}

/// RULE: a model package identifier is in use once it is registered, so a second
/// registration under the same identifier is refused as a bad request — 400
/// `ValidationException`, the class AWS answers for an identifier already registered,
/// not the retryable conflict — and the package already stored is left exactly as it was.
#[tokio::test]
async fn a_name_already_in_use_is_refused_with_validation_exception() {
    for name in ["a", "duplicate-package", "Mixed-Case-9"] {
        let case = format!("duplicate {name:?}");
        let emulator = Emulator::new();
        let first = emulator
            .create(
                &case,
                &json!({"ModelPackageName": name, "ModelPackageDescription": "first"}),
            )
            .await;

        let (status, refused) = emulator
            .call(
                "CreateModelPackage",
                &json!({"ModelPackageName": name, "ModelPackageDescription": "second"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a name already in use, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert!(
            message(&refused).contains(name),
            "case {case:?}: expected the message to name {name:?}, got {:?}",
            message(&refused)
        );

        let described = emulator.describe(&case, name).await;
        assert_eq!(
            str_field(&described, "ModelPackageArn"),
            first,
            "case {case:?}: the refused create must not have replaced the package, \
             got {described}"
        );
        assert_eq!(
            str_field(&described, "ModelPackageDescription"),
            "first",
            "case {case:?}: the refused create must not have overwritten the package, \
             got {described}"
        );
    }
}

/// RULE: the refusal of a registration under an identifier already registered names the
/// ALREADY-EXISTING package by its ARN — the one the earlier create minted and the registry
/// still holds — so a client that parses the message can address the colliding resource.
///
/// The rows differ along what the rule generalises over: the shape of the name (one
/// character, a hyphenated name, mixed case, the modelled 63-character maximum) and the
/// Region the package was registered in, which is the Region its ARN carries.
#[tokio::test]
async fn a_duplicate_registration_is_refused_by_the_existing_packages_arn() {
    let longest = "m".repeat(63);
    let rows: Vec<(&str, &str)> = vec![
        ("a", "us-east-1"),
        ("duplicate-package", "us-east-1"),
        ("Mixed-Case-9", "eu-west-2"),
        (longest.as_str(), "ap-southeast-1"),
    ];
    for (name, region) in rows {
        let case = format!("duplicate {name:?} in {region}");
        let emulator = Emulator::new();
        let (status, first) = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({"ModelPackageName": name, "ModelPackageDescription": "first"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first registration must be accepted, got {status} with {first}"
        );
        let existing = str_field(&first, "ModelPackageArn");

        let (status, refused) = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({"ModelPackageName": name, "ModelPackageDescription": "second"}),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a duplicate must be refused 400 ValidationException, got {status} \
             with {refused}"
        );
        assert_eq!(
            message(&refused),
            format!("ModelPackage already exists: {existing}"),
            "case {case:?}: the refusal must carry the existing package's ARN, got {:?}",
            message(&refused)
        );
        // The ARN in the message is the one the registry answers for that identifier, so it
        // addresses the colliding resource.
        let described = emulator
            .ok(
                &case,
                "DescribeModelPackage",
                &json!({"ModelPackageName": existing.clone()}),
            )
            .await;
        assert_eq!(
            (
                str_field(&described, "ModelPackageArn"),
                str_field(&described, "ModelPackageDescription"),
            ),
            (existing.clone(), "first".to_string()),
            "case {case:?}: the ARN the refusal named must describe the stored package, got \
             {described}"
        );

        // A DIFFERENT name is not in use, so it still gets its own ARN. Built by replacing
        // the name's last character, so it stays inside the modelled 63-character maximum
        // however long the row's own name is.
        let other = format!("{}z", &name[..name.len() - 1]);
        let (status, accepted) = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({"ModelPackageName": other}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a different name must still be registrable, got {status} with \
             {accepted}"
        );
        assert_ne!(
            str_field(&accepted, "ModelPackageArn"),
            existing,
            "case {case:?}: a different name gets its own ARN, got {accepted}"
        );
    }
}

/// RULE: exactly one of the two kind-selecting members names the package to create —
/// `ModelPackageName` for an unversioned one, `ModelPackageGroupName` for a version —
/// so a request carrying neither or both is refused 400 `ValidationException`.
#[tokio::test]
async fn neither_or_both_kind_members_is_refused() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "group").await;
    let rows = [
        ("neither member", json!({})),
        (
            "neither member, other members present",
            json!({"ModelPackageDescription": "no name at all", "Task": "OTHER"}),
        ),
        (
            "both members",
            json!({"ModelPackageName": "pkg", "ModelPackageGroupName": "group"}),
        ),
        (
            "both members, group given as an ARN",
            json!({
                "ModelPackageName": "pkg",
                "ModelPackageGroupName":
                    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/group"),
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, refused) = emulator.call("CreateModelPackage", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        for member in ["ModelPackageName", "ModelPackageGroupName"] {
            assert!(
                message(&refused).contains(member),
                "case {case:?}: expected the message to name {member}, got {:?}",
                message(&refused)
            );
        }
    }
}

/// RULE: a registration into a model package group is given the next version past the
/// highest the group holds, counting from 1; the version's ARN and store key are
/// `<group>/<version>`, it reports the group as its name and group, and it answers to a
/// describe by `<group>/<version>` and by ARN alike.
#[tokio::test]
async fn versioned_registration_numbers_versions_from_one_inside_each_group() {
    // Rows differ in how many versions the group gets and in the region signed for.
    let rows = [("g", "us-east-1", 1), ("model-group-9", "us-west-2", 3), ("A0", "eu-west-1", 5)];

    for (group, region, versions) in rows {
        let emulator = Emulator::new();
        let group_arn = emulator.create_group(region, group).await;
        // A second group in the same store: versions are numbered per group, so this
        // one's numbering must not shift.
        let other = format!("{group}-other");
        emulator.create_group(region, &other).await;

        for expected_version in 1..=versions {
            let case = format!("{group:?} version {expected_version}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateModelPackage",
                    &json!({
                        "ModelPackageGroupName": group,
                        "ModelPackageDescription": format!("version {expected_version}"),
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {created}"
            );
            let arn = str_field(&created, "ModelPackageArn");
            assert_eq!(
                arn,
                expected_arn(region, &format!("{group}/{expected_version}")),
                "case {case:?}: expected the versioned model-package ARN, got {arn:?}"
            );

            // The same package, by `<group>/<version>` and by its ARN.
            for identifier in [format!("{group}/{expected_version}"), arn.clone()] {
                let described = emulator
                    .ok(
                        &case,
                        "DescribeModelPackage",
                        &json!({"ModelPackageName": identifier}),
                    )
                    .await;
                assert_eq!(
                    described.get("ModelPackageVersion").and_then(Value::as_i64),
                    Some(i64::from(expected_version)),
                    "case {case:?}: describing {identifier:?} must report version \
                     {expected_version}, got {described}"
                );
                assert_eq!(
                    str_field(&described, "ModelPackageGroupName"),
                    group,
                    "case {case:?}: describing {identifier:?} must report its group, \
                     got {described}"
                );
                assert_eq!(
                    str_field(&described, "ModelPackageName"),
                    group,
                    "case {case:?}: a version is named by its group, got {described}"
                );
                assert_eq!(
                    str_field(&described, "ModelPackageDescription"),
                    format!("version {expected_version}"),
                    "case {case:?}: expected this version's own description, got {described}"
                );
                // "This parameter is optional for versioned models": a version the
                // request left unset awaits manual approval.
                assert_eq!(
                    str_field(&described, "ModelApprovalStatus"),
                    "PendingManualApproval",
                    "case {case:?}: expected the default approval status, got {described}"
                );
            }
        }

        // The neighbouring group starts its own numbering at 1.
        let neighbour = emulator
            .create(
                &format!("{other:?} first version"),
                &json!({"ModelPackageGroupName": other}),
            )
            .await;
        assert_eq!(
            neighbour,
            expected_arn(state::DEFAULT_REGION, &format!("{other}/1")),
            "case {other:?}: each group numbers its versions independently, got {neighbour:?}"
        );

        // `ModelPackageGroupName` is an `ArnOrName`, so the group's ARN names the same
        // group and the registration continues that group's numbering.
        let case = format!("{group:?} named by its ARN");
        let (status, created) = emulator
            .call_in(
                region,
                "CreateModelPackage",
                &json!({"ModelPackageGroupName": group_arn}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {created}"
        );
        assert_eq!(
            str_field(&created, "ModelPackageArn"),
            expected_arn(region, &format!("{group}/{}", versions + 1)),
            "case {case:?}: expected version {} of {group:?}, got {created}",
            versions + 1
        );
    }
}

/// RULE: `ModelApprovalStatus` is kept as the request set it for a VERSIONED package, for
/// every value the `ModelApprovalStatus` enum allows (`smithy_query(query="shape",
/// shape="ModelApprovalStatus")`: Approved, Rejected, PendingManualApproval), and is not
/// recorded at all for an unversioned one, where the member "does not apply".
#[tokio::test]
async fn a_requested_approval_status_is_kept_for_every_modelled_value() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "approvals").await;
    for (index, requested) in ["Approved", "Rejected", "PendingManualApproval"]
        .into_iter()
        .enumerate()
    {
        let case = format!("approval status {requested:?}");
        emulator
            .create(
                &case,
                &json!({"ModelPackageGroupName": "approvals", "ModelApprovalStatus": requested}),
            )
            .await;
        let described = emulator.describe(&case, &format!("approvals/{}", index + 1)).await;
        assert_eq!(
            str_field(&described, "ModelApprovalStatus"),
            requested,
            "case {case:?}: expected the requested approval status, got {described}"
        );
        assert!(
            described.get("ApprovalDescription").is_none(),
            "case {case:?}: a registration records no approval description, got {described}"
        );
    }

    // "does not apply to unversioned models": an unversioned package records no approval
    // status, whatever the request named, for every value of the enum.
    for requested in ["Approved", "Rejected", "PendingManualApproval"] {
        let case = format!("unversioned package asking for {requested:?}");
        let name = format!("unversioned{requested}");
        emulator
            .create(
                &case,
                &json!({"ModelPackageName": name, "ModelApprovalStatus": requested}),
            )
            .await;
        let described = emulator.describe(&case, &name).await;
        assert!(
            described.get("ModelApprovalStatus").is_none(),
            "case {case:?}: an unversioned package records no approval status, got {described}"
        );
    }
}

/// RULE: a version takes the tags of the model package group it joins — "If you supply
/// ModelPackageGroupName, your model package belongs to the model group you specify and
/// uses the tags associated with the model group. In this case, you cannot supply a tag
/// argument." — so a versioned registration that names tags of its own is refused 400
/// `ValidationException`, while an unversioned one takes the tags it names.
#[tokio::test]
async fn tags_cannot_be_supplied_with_a_model_package_group_name() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "tagged").await;
    let rows = [
        ("one tag", json!([{"Key": "team", "Value": "ml"}])),
        (
            "two tags",
            json!([{"Key": "team", "Value": "ml"}, {"Key": "env", "Value": "test"}]),
        ),
    ];

    for (case, tags) in rows {
        let (status, refused) = emulator
            .call(
                "CreateModelPackage",
                &json!({"ModelPackageGroupName": "tagged", "Tags": tags}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert!(
            message(&refused).contains("Tags"),
            "case {case:?}: expected the message to name Tags, got {:?}",
            message(&refused)
        );
    }

    // An empty list supplies no tag, so it is no obstacle, and the version it registers
    // is the group's first.
    let versioned = emulator
        .create(
            "an empty tag list",
            &json!({"ModelPackageGroupName": "tagged", "Tags": []}),
        )
        .await;
    assert_eq!(
        versioned,
        expected_arn(state::DEFAULT_REGION, "tagged/1"),
        "case \"an empty tag list\": expected the group's first version, got {versioned:?}"
    );

    // The same tags on the unversioned path are accepted: there is no group to take them
    // from.
    let unversioned = emulator
        .create(
            "tags on an unversioned package",
            &json!({
                "ModelPackageName": "tagged-unversioned",
                "Tags": [{"Key": "team", "Value": "ml"}],
            }),
        )
        .await;
    assert_eq!(
        unversioned,
        expected_arn(state::DEFAULT_REGION, "tagged-unversioned"),
        "case \"tags on an unversioned package\": expected the package ARN, got {unversioned:?}"
    );
}

/// RULE: `ModelPackageGroupName` references a model package group the account holds, by
/// name or by ARN; a reference to a group that does not exist is refused 400
/// `ValidationException` quoting the identifier the request passed, and nothing is
/// registered.
#[tokio::test]
async fn registration_into_a_group_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "exists").await;
    let rows = [
        ("a never-created group, by name", "missing".to_string()),
        ("a group name that differs in case", "Exists".to_string()),
        (
            "a never-created group, by ARN",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/missing"),
        ),
    ];

    for (case, group) in rows {
        let (status, refused) = emulator
            .call("CreateModelPackage", &json!({"ModelPackageGroupName": group}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert!(
            message(&refused).contains(&group),
            "case {case:?}: expected the message to quote {group:?}, got {:?}",
            message(&refused)
        );
    }

    // Nothing was registered: the group that does exist is still empty, so its first
    // version is 1.
    let first = emulator
        .create("after the refusals", &json!({"ModelPackageGroupName": "exists"}))
        .await;
    assert_eq!(
        first,
        expected_arn(state::DEFAULT_REGION, "exists/1"),
        "case \"after the refusals\": a refused registration must consume no version, \
         got {first:?}"
    );
}

/// RULE: a `ClientToken` is SPENT by the registration that presented it — a repeat
/// presenting the same token is refused with the modelled `ConflictException`, naming the
/// model package that token registered, and registers neither a second package nor a second
/// version, while a different token registers a new one.
///
/// (The whole rule, with every form of retry, is driven by `model_package_admission`; these
/// rows are the registry's own view of it.)
#[tokio::test]
async fn a_repeated_client_token_registers_nothing_a_second_time() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "idempotent").await;
    let rows = [
        (
            "unversioned, token repeated three times",
            json!({"ModelPackageName": "token-package"}),
            "token-package".to_string(),
        ),
        (
            "versioned, token repeated three times",
            json!({"ModelPackageGroupName": "idempotent"}),
            "idempotent/1".to_string(),
        ),
    ];

    for (case, members, expected_resource) in rows {
        let token = format!("token-{}", expected_resource.replace('/', "-"));
        let body = body_with(&members, &json!({"ClientToken": token}));
        let first = emulator.create(case, &body).await;
        assert_eq!(
            first,
            expected_arn(state::DEFAULT_REGION, &expected_resource),
            "case {case:?}: expected the ARN for {expected_resource:?}, got {first:?}"
        );
        for repeat in 1..=2 {
            let (status, refused) = emulator.call("CreateModelPackage", &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ConflictException"),
                "case {case:?}: repeat {repeat} must be refused, got {status} with {refused}"
            );
            assert!(
                message(&refused).contains(&format!("Name:{expected_resource}"))
                    || message(&refused).contains(&format!(
                        "Group:{}",
                        expected_resource.split('/').next().unwrap_or_default()
                    )),
                "case {case:?}: repeat {repeat} must name the registration the token was \
                 spent on, got {:?}",
                message(&refused)
            );
        }
    }

    // A different token is a different call: it registers the next version.
    let second = emulator
        .create(
            "a fresh token registers a new version",
            &json!({"ModelPackageGroupName": "idempotent", "ClientToken": "token-other"}),
        )
        .await;
    assert_eq!(
        second,
        expected_arn(state::DEFAULT_REGION, "idempotent/2"),
        "case \"a fresh token registers a new version\": expected version 2, got {second:?}"
    );

    // A name already in use is refused whether a token is presented or not.
    let (status, refused) = emulator
        .call("CreateModelPackage", &json!({"ModelPackageName": "token-package"}))
        .await;
    assert_eq!(
        status, 400,
        "case \"same name, no token\": expected HTTP 400, got {status} with {refused}"
    );

    // The record is scoped to the package the retry asks for: the same token naming a
    // DIFFERENT package registers that package rather than being refused about the first.
    let rows = [
        (
            "a used token on another unversioned name",
            json!({"ModelPackageName": "token-elsewhere", "ClientToken": "token-token-package"}),
            "token-elsewhere".to_string(),
        ),
        (
            "a used token on a group",
            json!({"ModelPackageGroupName": "idempotent", "ClientToken": "token-token-package"}),
            "idempotent/3".to_string(),
        ),
    ];
    for (case, body, expected_resource) in rows {
        let created = emulator.create(case, &body).await;
        assert_eq!(
            created,
            expected_arn(state::DEFAULT_REGION, &expected_resource),
            "case {case:?}: expected the ARN for {expected_resource:?}, got {created:?}"
        );
    }
}

/// RULE: a registration is asynchronous exactly while it has validation work to run — a
/// package whose request names a validation profile is `Pending`, with one `NotStarted`
/// validation item per profile, until the instant the clock owes it `Completed`, while a
/// package the request asked no validation for is registered `Completed` outright — and
/// the settled `LastModifiedTime` is stable across later reads.
#[tokio::test]
async fn status_settles_from_pending_to_completed_when_the_clock_owes_it() {
    // `ModelPackageValidationSpecification$ValidationProfiles` is modelled `len 1..1`, so
    // a request carries either no validation specification or exactly one profile.
    for profiles in [0_usize, 1] {
        let case = format!("{profiles} validation profiles");
        let emulator = Emulator::new();
        let name = format!("settling{profiles}");
        let mut body = json!({"ModelPackageName": name});
        if profiles > 0 {
            body = body_with(
                &body,
                &json!({"ValidationSpecification": validation_specification(profiles)}),
            );
        }
        emulator.create(&case, &body).await;

        // Straight after the call: a registration with validation jobs outstanding is
        // reported in the non-terminal status, one with nothing to run is already
        // complete.
        let registered = emulator.describe(&case, &name).await;
        let expected_status = status_at_registration(&body);
        assert_eq!(
            str_field(&registered, "ModelPackageStatus"),
            expected_status,
            "case {case:?}: expected {expected_status} as registered, got {registered}"
        );
        assert!(
            registered.get("LastModifiedTime").is_none(),
            "case {case:?}: nothing has modified the package yet, got {registered}"
        );
        let items = validation_statuses(&registered);
        assert_eq!(
            items.len(),
            profiles,
            "case {case:?}: expected one validation item per profile, got {registered}"
        );
        for item in &items {
            assert_eq!(
                str_field(item, "Status"),
                "NotStarted",
                "case {case:?}: expected NotStarted while Pending, got {item}"
            );
            assert!(
                str_field(item, "Name").starts_with("profile"),
                "case {case:?}: expected the item to name its profile, got {item}"
            );
        }

        // Look again until the registration has settled, however long the host takes:
        // what is under test is that the transition happens, not when.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        let completed = loop {
            let read = emulator.describe(&case, &name).await;
            if str_field(&read, "ModelPackageStatus") == "Completed" {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected Completed within {SETTLE_DEADLINE:?}, got {read}"
            );
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        };
        assert_eq!(
            str_field(&completed, "ModelPackageStatus"),
            "Completed",
            "case {case:?}: expected Completed once the transition is due, got {completed}"
        );
        // `LastModifiedTime` is reported only once something has CHANGED the package: the
        // settling of an outstanding registration is such a change, while a package that
        // was registered complete has had nothing modify it.
        let settled_at = completed.get("LastModifiedTime").cloned();
        assert_eq!(
            settled_at.is_some(),
            expected_status == "Pending",
            "case {case:?}: expected a LastModifiedTime only for a registration that \
             settled, got {completed}"
        );
        for item in validation_statuses(&completed) {
            assert_eq!(
                str_field(&item, "Status"),
                "Completed",
                "case {case:?}: expected Completed validation items, got {item}"
            );
        }

        // A read does not move the resource on: the settled timestamps hold.
        let again = emulator.describe(&case, &name).await;
        assert_eq!(
            again.get("LastModifiedTime").cloned(),
            settled_at,
            "case {case:?}: LastModifiedTime must be stable across reads, got {again}"
        );
        assert_eq!(
            again.get("CreationTime").cloned(),
            completed.get("CreationTime").cloned(),
            "case {case:?}: CreationTime must be stable across reads, got {again}"
        );
    }
}

/// RULE: `IncludedData=MetadataOnly` keeps `ModelCardStatus` and reduces
/// `ModelCardContent` to the JSON paths the model documents for it
/// (`smithy_query(query="doc",shape="DescribeModelPackageOutput",member="ModelCard")`:
/// `model_overview.model_id`, `model_overview.model_name`, `intended_uses.risk_rating`,
/// `model_package_details.model_package_group_name`,
/// `model_package_details.model_package_arn`), while `AllData` — the default when the
/// member is absent — replays the card verbatim.
#[tokio::test]
async fn metadata_only_sanitizes_the_model_card_content_to_the_documented_paths() {
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "every documented path, plus fields that are not",
            json!({
                "model_overview": {
                    "model_id": "m-1",
                    "model_name": "detector",
                    "model_creator": "team-ml",
                    "model_artifact": ["s3://models/artifact.tar.gz"],
                },
                "intended_uses": {"risk_rating": "Low", "purpose_of_model": "secret"},
                "model_package_details": {
                    "model_package_group_name": "group",
                    "model_package_arn": "arn:aws:sagemaker:us-east-1:000000000000:model-package/group/1",
                    "model_package_description": "secret",
                },
                "training_details": {"objective_function": "secret"},
            }),
            json!({
                "model_overview": {"model_id": "m-1", "model_name": "detector"},
                "intended_uses": {"risk_rating": "Low"},
                "model_package_details": {
                    "model_package_group_name": "group",
                    "model_package_arn": "arn:aws:sagemaker:us-east-1:000000000000:model-package/group/1",
                },
            }),
        ),
        (
            "only one documented path present",
            json!({"intended_uses": {"risk_rating": "High"}, "additional_information": {"x": 1}}),
            json!({"intended_uses": {"risk_rating": "High"}}),
        ),
        (
            "no documented path present",
            json!({"model_overview": {"model_creator": "team-ml"}}),
            json!({}),
        ),
        (
            "a card the schema allows but which carries nothing at all",
            json!({}),
            json!({}),
        ),
    ];

    for (case, content, sanitized) in rows {
        let emulator = Emulator::new();
        let name = "carded";
        let content = content.to_string();
        emulator
            .create(
                case,
                &json!({
                    "ModelPackageName": name,
                    "ModelCard": {"ModelCardContent": content, "ModelCardStatus": "Draft"},
                }),
            )
            .await;

        // The default, and an explicit `AllData`, replay the stored card unchanged.
        for included in [None, Some("AllData")] {
            let mut request = json!({"ModelPackageName": name});
            if let Some(included) = included {
                request = body_with(&request, &json!({"IncludedData": included}));
            }
            let described = emulator.ok(case, "DescribeModelPackage", &request).await;
            let card = described
                .get("ModelCard")
                .unwrap_or_else(|| panic!("case {case:?}: expected a ModelCard in {described}"));
            assert_eq!(
                str_field(card, "ModelCardContent"),
                content,
                "case {case:?}: IncludedData={included:?} must replay the card verbatim, \
                 got {card}"
            );
            assert_eq!(
                str_field(card, "ModelCardStatus"),
                "Draft",
                "case {case:?}: IncludedData={included:?} must keep the card status, got {card}"
            );
        }

        let described = emulator
            .ok(
                case,
                "DescribeModelPackage",
                &json!({"ModelPackageName": name, "IncludedData": "MetadataOnly"}),
            )
            .await;
        let card = described
            .get("ModelCard")
            .unwrap_or_else(|| panic!("case {case:?}: expected a ModelCard in {described}"));
        let returned: Value = serde_json::from_str(&str_field(card, "ModelCardContent"))
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the sanitized content must be JSON: {error}")
            });
        assert_eq!(
            returned, sanitized,
            "case {case:?}: expected the sanitized content {sanitized}, got {returned}"
        );
        assert_eq!(
            str_field(card, "ModelCardStatus"),
            "Draft",
            "case {case:?}: MetadataOnly preserves ModelCardStatus, got {card}"
        );
    }
}

/// RULE: `MetadataOnly` content that is not a JSON object carries none of the documented
/// paths, so nothing of it is preserved.
#[tokio::test]
async fn metadata_only_preserves_nothing_from_content_that_is_not_a_json_object() {
    let emulator = Emulator::new();
    for (index, content) in ["not json at all", "[1, 2, 3]", "\"a string\"", ""]
        .into_iter()
        .enumerate()
    {
        let case = format!("non-object content {content:?}");
        let name = format!("opaque{index}");
        emulator
            .create(
                &case,
                &json!({
                    "ModelPackageName": name,
                    "ModelCard": {"ModelCardContent": content, "ModelCardStatus": "Draft"},
                }),
            )
            .await;
        let described = emulator
            .ok(
                &case,
                "DescribeModelPackage",
                &json!({"ModelPackageName": name, "IncludedData": "MetadataOnly"}),
            )
            .await;
        let card = described
            .get("ModelCard")
            .unwrap_or_else(|| panic!("case {case:?}: expected a ModelCard in {described}"));
        assert_eq!(
            str_field(card, "ModelCardContent"),
            "{}",
            "case {case:?}: expected nothing preserved, got {card}"
        );
        // `AllData` still replays exactly what was stored.
        let all_data = emulator.describe(&case, &name).await;
        assert_eq!(
            str_field(
                all_data
                    .get("ModelCard")
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ModelCard")),
                "ModelCardContent"
            ),
            content,
            "case {case:?}: AllData must replay the stored content, got {all_data}"
        );
    }
}

/// RULE: `DescribeModelPackage` answers only for an identifier some stored package
/// carries; anything else is refused 400 `ValidationException` naming the package ARN the
/// identifier resolves to.
#[tokio::test]
async fn describing_an_unknown_model_package_is_refused() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "known").await;
    emulator
        .create("a package to miss", &json!({"ModelPackageGroupName": "known"}))
        .await;
    emulator
        .create("an unversioned package", &json!({"ModelPackageName": "unversioned"}))
        .await;

    // Rows: (case, the identifier the request carries, the package key the refusal must
    // report the ARN of).
    let rows = [
        ("a name never registered", "absent".to_string(), "absent"),
        ("a group with no such version", "known/2".to_string(), "known/2"),
        ("a versioned package by its group alone", "known".to_string(), "known"),
        (
            "an unversioned package asked for by version",
            "unversioned/1".to_string(),
            "unversioned/1",
        ),
        (
            "an ARN for a package that does not exist",
            expected_arn(state::DEFAULT_REGION, "absent"),
            "absent",
        ),
    ];

    for (case, identifier, key) in rows {
        let (status, refused) = emulator
            .call("DescribeModelPackage", &json!({"ModelPackageName": identifier}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "ModelPackage {} does not exist.",
                expected_arn(state::DEFAULT_REGION, key)
            ),
            "case {case:?}: expected the message to name the resolved package ARN, got {:?}",
            message(&refused)
        );
    }
}
