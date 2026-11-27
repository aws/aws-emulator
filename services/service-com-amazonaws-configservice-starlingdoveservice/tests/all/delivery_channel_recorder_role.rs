// SPDX-License-Identifier: CC0-1.0

//! What the CONFIGURATION RECORDER'S IAM ROLE decides about a delivery channel.
//!
//! The rule under test: **Config makes a delivery as its own service principal under the
//! configuration recorder's IAM role, so `PutDeliveryChannel` takes a channel only while
//! IAM shows that role can be assumed by `config.amazonaws.com`. Where IAM answers that it
//! cannot — this account holds no role under the name the ARN carries, or the trust
//! document the role carries does not admit that principal — the call is refused with HTTP
//! 400 `InsufficientDeliveryPolicyException`, worded after that assume and naming the
//! bucket and the role, and NOTHING is persisted: the channel-existence reads
//! (`DescribeDeliveryChannels`, `DescribeDeliveryChannelStatus`, `DeleteDeliveryChannel`)
//! then answer 400 `NoSuchDeliveryChannelException` quoting the name the refused put
//! named. A role nothing can be shown about is refused nothing.**
//!
//! Config and IAM are mounted on ONE engine, because the assume is put to IAM itself over
//! the emulator's own in-process callback: that is why every other suite in this crate,
//! which mounts Config alone, still puts delivery channels whose recorder role no IAM
//! record backs.
//!
//! The second rule under test: **Config makes those S3 calls as its own SERVICE PRINCIPAL,
//! which holds no identity-based policies, so the policy in force on the bucket is the only
//! thing that can permit them: a call that document does not allow the Config principal is
//! refused however much the recorder's role is allowed, a bucket carrying no policy at all
//! withholds the first of them, and the same document decides the write a performed delivery
//! makes, read at the moment of that write.**
//!
//! The rows differ along what the rules generalise over: what the role's trust document
//! says (which principal, which action, which effect, how many statements, whether it is
//! readable as a policy at all, whether the role carries one), whether this account holds
//! the role, whose account the ARN names, the path IAM holds the role under, the channel and
//! bucket names the refusal has to quote back, and — for the second rule — which statements
//! the bucket's policy carries, what the role is granted beside it, which of the two calls
//! each grant covers, and whether the channel delivers under a key prefix.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    DEFAULT_PATH,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The Config service principal a role has to admit for Config to deliver under it.
const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The error code a delivery Config cannot make is refused with, and the one every
/// existence read answers for a channel this account does not hold.
const INSUFFICIENT_POLICY: &str = "InsufficientDeliveryPolicyException";
/// See [`INSUFFICIENT_POLICY`].
const NO_SUCH_CHANNEL: &str = "NoSuchDeliveryChannelException";

/// Config, IAM, and optionally Amazon S3, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    callback: EmbeddedEmulator,
}

impl Emulator {
    /// Config and IAM alone: with no S3 to ask, no bucket policy is established for any
    /// bucket, so the ROLE is the only thing that can decide a put.
    fn new() -> Self {
        Self::build(false)
    }

    /// The same, with S3 mounted: a bucket S3 holds answers about its own policy, so both
    /// halves of the decision are live and their ORDER is observable.
    fn with_s3() -> Self {
        Self::build(true)
    }

    fn build(with_s3: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        if with_s3 {
            router.register_service(
                service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// One Config call (`awsJson1_1`), answering the HTTP status and the parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/config/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("config.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A Config call that must answer 200.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An S3 client over the in-process callback, the way Config's own reads of the delivery
    /// bucket are made.
    fn s3(&self) -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    /// An IAM client over the same callback, so a role the rows provision is provisioned
    /// through IAM's OWN API — `CreateRole`, `PutRolePolicy`, `AttachRolePolicy` — the way a
    /// client sets a configuration recorder's role up.
    fn iam(&self) -> aws_sdk_iam::Client {
        service_common::iam_client::iam_client_through(&self.callback, REGION)
    }

    /// Create `bucket` through S3's own API, carrying no policy at all.
    async fn create_bucket(&self, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
    }

    /// Put `document` as `bucket`'s own policy.
    async fn put_bucket_policy(&self, bucket: &str, document: String) {
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(document)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("writing {bucket}'s policy must succeed, got {error:?}")
            });
    }

    /// Every key `bucket` holds.
    async fn keys(&self, bucket: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("listing {bucket} must succeed, got {error:?}"));
        listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect()
    }

    /// Create `role_name` through IAM's own API, trusting the Config service principal the
    /// way a configuration recorder's role has to, and answer the ARN IAM minted for it.
    async fn create_config_role(&self, role_name: &str) -> String {
        self.iam()
            .create_role()
            .role_name(role_name)
            .assume_role_policy_document(config_trust())
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {role_name} must succeed, got {error:?}"))
            .role
            .expect("CreateRole answers the role")
            .arn
    }

    /// Embed `document` in `role_name` as the inline role policy `INLINE_POLICY_NAME`: a
    /// second call under the same name REPLACES it, which is how a row takes a grant away
    /// again.
    async fn put_role_policy(&self, role_name: &str, document: String) {
        self.iam()
            .put_role_policy()
            .role_name(role_name)
            .policy_name(INLINE_POLICY_NAME)
            .policy_document(document)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("embedding a policy in {role_name} must succeed, got {error:?}")
            });
    }

    /// Attach the managed policy `policy_arn` to `role_name`.
    async fn attach_role_policy(&self, role_name: &str, policy_arn: &str) {
        self.iam()
            .attach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("attaching {policy_arn} to {role_name} must succeed, got {error:?}")
            });
    }

    /// Seed `role_name` into the shared engine's IAM store under `path`, carrying `trust`
    /// as its `AssumeRolePolicyDocument` — the record `CreateRole` writes — and answer the
    /// ARN IAM holds it under.
    fn seed_role(&self, path: &str, role_name: &str, trust: Option<String>) -> String {
        let state = self.engine.state::<IamState>();
        let mut store = state.lock();
        let arn = role_arn(path, role_name);
        store.roles.insert(
            name_key(role_name),
            Role {
                path: path.to_string(),
                role_name: role_name.to_string(),
                role_id: format!("AROA{}", role_name.to_uppercase()),
                arn: arn.clone(),
                create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
                assume_role_policy_document: trust,
                description: None,
                max_session_duration: None,
                permissions_boundary: None,
                tags: None,
                role_last_used: None,
                source_role_template: None,
            },
        );
        arn
    }
}

/// `body`'s error code, as the awsJson envelope carries it.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// `body`'s error message.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal must carry a message, got {body}"))
        .to_string()
}

/// A trust document with one statement: `effect` for the service `principal` performing
/// `action`.
fn trust(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// The trust document a role Config can assume carries.
fn config_trust() -> String {
    trust("Allow", CONFIG_PRINCIPAL, "sts:AssumeRole")
}

/// The sentence a delivery the recorder's role withholds is refused with.
fn unable_to_assume(bucket: &str, role: &str) -> String {
    format!("Insufficient delivery policy to s3 bucket: {bucket}, unable to assume role: {role}.")
}

/// The sentence every existence read answers for a channel this account does not hold.
fn cannot_find_channel(channel: &str) -> String {
    format!("Cannot find delivery channel with specified name '{channel}'.")
}

/// Put the customer managed configuration recorder a delivery channel needs, running as
/// `role`.
async fn with_recorder(emulator: &Emulator, role: &str) {
    emulator
        .ok(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": "the-recorder",
                    "roleARN": role,
                    "recordingGroup": { "allSupported": true },
                }
            }),
        )
        .await;
}

/// The `PutDeliveryChannel` body for `channel` delivering into `bucket`.
fn channel_request(channel: &str, bucket: &str) -> Value {
    json!({ "DeliveryChannel": { "name": channel, "s3BucketName": bucket } })
}

/// Assert that this account holds NO delivery channel under `channel`: every read that
/// names it answers the modelled 400, quoting the name the request carried.
async fn assert_no_such_channel(emulator: &Emulator, case: &str, channel: &str) {
    let expected = cannot_find_channel(channel);
    let reads: [(&str, Value); 3] = [
        (
            "DescribeDeliveryChannels",
            json!({ "DeliveryChannelNames": [channel] }),
        ),
        (
            "DescribeDeliveryChannelStatus",
            json!({ "DeliveryChannelNames": [channel] }),
        ),
        (
            "DeleteDeliveryChannel",
            json!({ "DeliveryChannelName": channel }),
        ),
    ];
    for (operation, body) in reads {
        let (status, answered) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answered), error_message(&answered)),
            (400, NO_SUCH_CHANNEL.to_string(), expected.clone()),
            "case {case:?} ({operation}): expected HTTP 400 {NO_SUCH_CHANNEL} and \
             {expected:?}, got {status} with {answered}"
        );
    }
    // The unnamed read reports the whole account, which holds nothing either.
    let listed = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
    assert_eq!(
        listed["DeliveryChannels"],
        json!([]),
        "case {case:?}: a refused put persists no channel, got {listed}"
    );
}

/// One row of the table: what IAM holds under the row's role name, and whether the rule
/// says Config must take the channel.
struct Row {
    case: &'static str,
    /// The trust document the account's role carries, or `None` for a row whose role the
    /// account does not hold at all.
    held: Option<Option<String>>,
    /// Whether the put must be admitted.
    admitted: bool,
}

/// A delivery channel is taken only while IAM shows Config can assume the configuration
/// recorder's role — and a refusal names the bucket and that role, and persists nothing.
#[tokio::test]
async fn a_channel_is_taken_only_while_config_can_assume_the_recorders_role() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a trust document admitting the Config principal",
            held: Some(Some(config_trust())),
            admitted: true,
        },
        Row {
            case: "a service wildcard in the action",
            held: Some(Some(trust("Allow", CONFIG_PRINCIPAL, "sts:*"))),
            admitted: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            held: Some(Some(trust("Allow", "CONFIG.amazonaws.com", "STS:AssumeRole"))),
            admitted: true,
        },
        Row {
            case: "a document whose second statement admits the Config principal",
            held: Some(Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": CONFIG_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
                .to_string(),
            )),
            admitted: true,
        },
        Row {
            case: "a trust document admitting another service only",
            held: Some(Some(trust(
                "Allow",
                "cloudtrail.amazonaws.com",
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document naming another action",
            held: Some(Some(trust("Allow", CONFIG_PRINCIPAL, "s3:GetObject"))),
            admitted: false,
        },
        Row {
            case: "a trust document explicitly denying the Config principal",
            held: Some(Some(trust("Deny", CONFIG_PRINCIPAL, "sts:AssumeRole"))),
            admitted: false,
        },
        Row {
            case: "a trust document carrying no statement",
            held: Some(Some(json!({ "Version": "2012-10-17" }).to_string())),
            admitted: false,
        },
        Row {
            case: "a trust document that is not a policy at all",
            held: Some(Some("this is not a policy".to_string())),
            admitted: false,
        },
        Row {
            case: "a role carrying no trust document",
            held: Some(None),
            admitted: false,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            admitted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Each row stands its own emulator up, and names its own channel and bucket, so
        // the refusal's variable parts are the ones THAT row sent.
        let emulator = Emulator::new();
        let role_name = format!("delivery-role-{index}");
        let channel = format!("delivery-channel-{index}");
        let bucket = format!("delivery-bucket-{index}");
        let role = match &row.held {
            Some(trust) => emulator.seed_role(DEFAULT_PATH, &role_name, trust.clone()),
            // Not seeded, so IAM answers that this account holds no such role. The ARN is
            // the one IAM would hold it under.
            None => role_arn(DEFAULT_PATH, &role_name),
        };
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        let case = row.case;
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be admitted, got {status} with {body}"
            );
            // Round trip: the channel the assume admitted is readable, and its status is
            // reported, under the name it was put with.
            let read = emulator
                .ok(
                    "DescribeDeliveryChannels",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            assert_eq!(
                read["DeliveryChannels"][0]["s3BucketName"],
                json!(bucket),
                "case {case:?}: expected the held channel to name the bucket it was put \
                 with, got {read}"
            );
            let reported = emulator
                .ok(
                    "DescribeDeliveryChannelStatus",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            assert_eq!(
                reported["DeliveryChannelsStatus"][0]["name"],
                json!(channel),
                "case {case:?}: expected the status of the channel that was put, got \
                 {reported}"
            );
            continue;
        }

        let expected = unable_to_assume(&bucket, &role);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The same decision over the forms the recorder's `roleARN` may name a role by: a role
/// under a path is held by IAM under its NAME, so it is judged like one at the default
/// path — while an ARN of ANOTHER account names a role this emulator's IAM cannot be asked
/// about, so nothing is shown and nothing is refused.
#[tokio::test]
async fn the_recorders_role_is_resolved_whatever_arn_names_it() {
    // (case, how the row's role ARN is built from the role it seeds, whether admitted)
    let rows: [(&str, bool, Option<&str>, bool); 5] = [
        ("this account's role, trusting Config", true, None, true),
        (
            "the same role under a service-role path, which IAM holds by name",
            true,
            Some("/service-role/"),
            true,
        ),
        (
            "this account's role, trusting another service",
            false,
            None,
            false,
        ),
        (
            "that same role under a service-role path",
            false,
            Some("/service-role/"),
            false,
        ),
        // Another account's IAM is not this emulator's to ask, so nothing is shown about
        // the role and the put goes ahead.
        (
            "another account's role ARN, which this IAM cannot be asked about",
            false,
            Some("other-account"),
            true,
        ),
    ];

    for (index, (case, trusts_config, path, admitted)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let role_name = format!("role-form-{index}");
        let channel = format!("channel-form-{index}");
        let bucket = format!("bucket-form-{index}");
        let document = if *trusts_config {
            config_trust()
        } else {
            trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")
        };
        let role = match path {
            Some("other-account") => {
                format!("arn:aws:iam::123456789012:role/{role_name}")
            }
            Some(path) => emulator.seed_role(path, &role_name, Some(document)),
            None => emulator.seed_role(DEFAULT_PATH, &role_name, Some(document)),
        };
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be admitted, got {status} with {body}"
            );
            continue;
        }
        let expected = unable_to_assume(&bucket, &role);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The assume is made BEFORE anything of the bucket is read, so a put that fails both
/// halves at once reports the ROLE: with S3 mounted and a bucket carrying no policy — which
/// on its own denies the first S3 call Config makes — the sentence names the assume while
/// the role withholds it, and the bucket call once the role admits it.
///
/// The rows differ along whether the role is assumable, which is what makes the first row
/// about precedence and not about the bucket check having gone away.
#[tokio::test]
async fn the_recorders_role_is_judged_before_the_buckets_policy() {
    // (case, the trust document the seeded role carries, the `unable to …` clause)
    let rows: [(&str, bool, &str); 2] = [
        (
            "a role Config cannot assume, pointed at a bucket with no policy",
            false,
            "assume role",
        ),
        (
            "a role Config can assume, pointed at the same bucket",
            true,
            "get s3 bucket acl",
        ),
    ];

    for (index, (case, assumable, clause)) in rows.iter().enumerate() {
        let emulator = Emulator::with_s3();
        let role_name = format!("ordering-role-{index}");
        let channel = format!("ordering-channel-{index}");
        let bucket = format!("ordering-bucket-{index}");
        emulator.create_bucket(&bucket).await;
        let document = if *assumable {
            config_trust()
        } else {
            trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")
        };
        let role = emulator.seed_role(DEFAULT_PATH, &role_name, Some(document));
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        let expected = if *assumable {
            format!("Insufficient delivery policy to s3 bucket: {bucket}, unable to {clause}.")
        } else {
            unable_to_assume(&bucket, &role)
        };
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The name the rows below embed their inline role policy under. One name, so a row that
/// embeds a second document REPLACES the first.
const INLINE_POLICY_NAME: &str = "config-delivery";

/// The managed policy AWS publishes for a configuration recorder's role, which a client
/// ATTACHES instead of writing the bucket-level grants itself. Among its grants is
/// `s3:GetBucketAcl` on every resource — which the rows attach to show that what the ROLE is
/// allowed admits no channel the bucket's own policy withholds.
const CONFIG_ROLE_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/service-role/AWS_ConfigRole";

/// The canned ACL Config writes a delivered object with, which the delivery grant AWS
/// documents conditions on (`s3:x-amz-acl`).
const DELIVERY_OBJECT_ACL: &str = "bucket-owner-full-control";

/// The account every row's bucket and role belong to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of `bucket`, which a statement's `Resource` names.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The delivery path Config writes this account's objects under, behind `prefix` where a
/// channel configures one: `[<prefix>/]AWSLogs/<account-id>`, the root the writability check
/// Config makes before it takes a channel sits below (AWS Config Developer Guide,
/// *Permissions for the Amazon S3 bucket*).
fn delivery_path(prefix: Option<&str>) -> String {
    let root = format!("AWSLogs/{}", account());
    match prefix {
        Some(prefix) => format!("{prefix}/{root}"),
        None => root,
    }
}

/// One `Allow` statement of an IDENTITY policy: `actions` on `resources`, carrying
/// `condition` where the row states one. Such a statement names no principal — the role the
/// policy is in force on IS the principal.
fn identity_allow(actions: &[&str], resources: &[String], condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Action": actions,
        "Resource": resources,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A policy document carrying `statements`.
fn document(statements: Vec<Value>) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// The condition the documented delivery grant carries: the object is written with the canned
/// ACL `acl`.
fn acl_condition(acl: &str) -> Value {
    json!({ "StringLike": { "s3:x-amz-acl": acl } })
}

/// The grant of the ACL read on the bucket itself, which Config makes before it will take a
/// channel.
fn acl_read(bucket: &str) -> Value {
    identity_allow(&["s3:GetBucketAcl"], &[bucket_arn(bucket)], None)
}

/// The grant of the object write on `objects`, conditioned on the canned ACL `acl`.
fn object_write(objects: String, acl: Option<&str>) -> Value {
    identity_allow(
        &["s3:PutObject", "s3:PutObjectAcl"],
        std::slice::from_ref(&objects),
        acl.map(acl_condition),
    )
}

/// The inline role policy AWS's own Config setup embeds in the recorder's role: the ACL read
/// on the bucket, and the object write on this account's delivery path under the channel's
/// prefix, conditioned on the canned ACL Config writes with.
fn documented_role_policy(bucket: &str, prefix: Option<&str>) -> String {
    document(vec![
        acl_read(bucket),
        object_write(
            format!("{}/{}/*", bucket_arn(bucket), delivery_path(prefix)),
            Some(DELIVERY_OBJECT_ACL),
        ),
    ])
}

/// The bucket policy AWS documents for a delivery channel, as a client writes it on the
/// bucket ITSELF: each statement an `Allow` for the Config service principal, which is what a
/// resource-based grant names.
fn documented_bucket_policy(bucket: &str) -> String {
    bucket_policy_without(bucket, &[])
}

/// The same, less the statements granting `without`: the rows that drop one of the documented
/// grants from the BUCKET build their policy with it.
fn bucket_policy_without(bucket: &str, without: &[&str]) -> String {
    let arn = bucket_arn(bucket);
    let statements = [
        ("s3:GetBucketAcl", arn.clone()),
        ("s3:ListBucket", arn.clone()),
        ("s3:PutObject", format!("{arn}/*")),
    ]
    .into_iter()
    .filter(|(action, _)| !without.contains(action))
    .map(|(action, resource)| {
        json!({
            "Effect": "Allow",
            "Principal": { "Service": CONFIG_PRINCIPAL },
            "Action": action,
            "Resource": resource,
        })
    })
    .collect();
    document(statements)
}

/// The documented bucket policy without its `AWSConfigBucketPermissionsCheck` statement —
/// the one grant of `s3:GetBucketAcl` to the Config service principal.
fn bucket_policy_without_the_acl_read(bucket: &str) -> String {
    bucket_policy_without(bucket, &["s3:GetBucketAcl"])
}

/// The documented bucket policy without its `AWSConfigBucketDelivery` statement.
fn bucket_policy_without_the_delivery(bucket: &str) -> String {
    bucket_policy_without(bucket, &["s3:PutObject"])
}

/// The documented bucket policy without its `AWSConfigBucketExistenceCheck` statement, which
/// is not among the calls Config's own refusals name.
fn bucket_policy_without_the_existence_check(bucket: &str) -> String {
    bucket_policy_without(bucket, &["s3:ListBucket"])
}

/// A bucket policy that DENIES Config the ACL read outright.
fn bucket_policy_denying_the_acl_read(bucket: &str) -> String {
    document(vec![json!({
        "Effect": "Deny",
        "Principal": { "Service": CONFIG_PRINCIPAL },
        "Action": "s3:GetBucketAcl",
        "Resource": bucket_arn(bucket),
    })])
}

/// Which S3 call a refusal names, and the `unable to …` clause it carries for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denied {
    /// The ACL read Config makes on the bucket.
    BucketAcl,
    /// The write of the object Config puts in the bucket to check it can deliver there.
    ObjectWrite,
}

impl Denied {
    /// The sentence the refusal carries, which names the bucket and this call — the object
    /// write naming the key prefix the channel asked for with it.
    fn message(self, bucket: &str, prefix: Option<&str>) -> String {
        let clause = match self {
            Self::BucketAcl => "get s3 bucket acl".to_string(),
            Self::ObjectWrite => format!(
                "write to bucket, provided s3 key prefix is '{}'",
                prefix.unwrap_or("null")
            ),
        };
        format!("Insufficient delivery policy to s3 bucket: {bucket}, unable to {clause}.")
    }
}

/// One row: what each source of the permission states, and what the rule says of the put.
struct SourceRow {
    case: &'static str,
    /// The inline policy embedded in the recorder's role, built from the row's bucket and the
    /// prefix its channel configures; `None` for a role carrying none.
    inline: Option<RolePolicy>,
    /// Whether the row attaches the managed policy AWS publishes for the role.
    managed: bool,
    /// The policy in force on the delivery bucket, built from the row's bucket; `None` for a
    /// bucket carrying none.
    bucket_policy: Option<fn(&str) -> String>,
    /// The `s3KeyPrefix` the channel configures.
    prefix: Option<&'static str>,
    /// The call the refusal has to name, or `None` for a put the rule says must be admitted.
    denied: Option<Denied>,
}

/// The `PutDeliveryChannel` body for `channel` delivering into `bucket` under `prefix`.
fn prefixed_channel_request(channel: &str, bucket: &str, prefix: Option<&str>) -> Value {
    let mut body = channel_request(channel, bucket);
    if let Some(prefix) = prefix {
        body["DeliveryChannel"]["s3KeyPrefix"] = json!(prefix);
    }
    body
}

/// The S3 calls a delivery channel needs are made as the Config SERVICE PRINCIPAL, so the
/// policy in force on the bucket is the only source of that permission: a call that document
/// does not allow Config is refused and named however much the configuration recorder's role
/// is allowed, and a bucket carrying no policy withholds the first of them.
///
/// The rows differ along which statements the bucket's policy carries (all three, each of
/// them dropped in turn, a `Deny`, none at all), what the role is granted beside it (its own
/// inline policy, the managed policy AWS publishes, both, neither), which of the two calls
/// each grant covers, and whether the channel delivers under a key prefix.
#[tokio::test]
async fn only_the_buckets_own_policy_permits_the_calls_a_channel_needs() {
    let rows: Vec<SourceRow> = vec![
        SourceRow {
            case: "the bucket's documented policy, the role granted nothing beside it",
            inline: None,
            managed: false,
            bucket_policy: Some(documented_bucket_policy),
            prefix: None,
            denied: None,
        },
        SourceRow {
            case: "the role's own policy grants both calls, the bucket carrying no policy",
            inline: Some(documented_role_policy),
            managed: false,
            bucket_policy: None,
            prefix: None,
            denied: Some(Denied::BucketAcl),
        },
        SourceRow {
            case: "the same, with the managed policy AWS publishes attached as well",
            inline: Some(documented_role_policy),
            managed: true,
            bucket_policy: None,
            prefix: None,
            denied: Some(Denied::BucketAcl),
        },
        SourceRow {
            case: "the bucket's policy without its permissions check, the role granted the \
                   ACL read by the managed policy",
            inline: None,
            managed: true,
            bucket_policy: Some(bucket_policy_without_the_acl_read),
            prefix: None,
            denied: Some(Denied::BucketAcl),
        },
        SourceRow {
            case: "the bucket's policy without its delivery statement, the role granted the \
                   write by its own policy",
            inline: Some(documented_role_policy),
            managed: false,
            bucket_policy: Some(bucket_policy_without_the_delivery),
            prefix: None,
            denied: Some(Denied::ObjectWrite),
        },
        SourceRow {
            case: "the bucket's policy without its existence check, which Config's own calls \
                   do not turn on",
            inline: None,
            managed: false,
            bucket_policy: Some(bucket_policy_without_the_existence_check),
            prefix: None,
            denied: None,
        },
        SourceRow {
            case: "neither the bucket's policy nor the role states anything",
            inline: None,
            managed: false,
            bucket_policy: None,
            prefix: None,
            denied: Some(Denied::BucketAcl),
        },
        SourceRow {
            case: "the bucket's policy denies the ACL read the role grants",
            inline: Some(documented_role_policy),
            managed: false,
            bucket_policy: Some(bucket_policy_denying_the_acl_read),
            prefix: None,
            denied: Some(Denied::BucketAcl),
        },
        SourceRow {
            case: "a channel under a key prefix, the bucket's delivery statement covering \
                   every object",
            inline: None,
            managed: false,
            bucket_policy: Some(documented_bucket_policy),
            prefix: Some("audit"),
            denied: None,
        },
        SourceRow {
            case: "the same channel, the write granted by the role alone",
            inline: Some(documented_role_policy),
            managed: false,
            bucket_policy: Some(bucket_policy_without_the_delivery),
            prefix: Some("audit"),
            denied: Some(Denied::ObjectWrite),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Each row stands its own emulator up and names its own bucket, role and channel, so
        // the refusal's variable parts are the ones THAT row sent.
        let emulator = Emulator::with_s3();
        let bucket = format!("source-bucket-{index}");
        let role_name = format!("source-role-{index}");
        let channel = format!("source-channel-{index}");
        emulator.create_bucket(&bucket).await;
        if let Some(document) = row.bucket_policy {
            emulator.put_bucket_policy(&bucket, document(&bucket)).await;
        }
        let role = emulator.create_config_role(&role_name).await;
        if let Some(inline) = row.inline {
            emulator
                .put_role_policy(&role_name, inline(&bucket, row.prefix))
                .await;
        }
        if row.managed {
            emulator
                .attach_role_policy(&role_name, CONFIG_ROLE_MANAGED_POLICY)
                .await;
        }
        with_recorder(&emulator, &role).await;

        let case = row.case;
        let (status, body) = emulator
            .call(
                "PutDeliveryChannel",
                &prefixed_channel_request(&channel, &bucket, row.prefix),
            )
            .await;
        let Some(denied) = row.denied else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be admitted, got {status} with {body}"
            );
            // Round trip: the channel the permission admitted is readable, naming the bucket
            // and the prefix it was put with.
            let read = emulator
                .ok(
                    "DescribeDeliveryChannels",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            let held = &read["DeliveryChannels"][0];
            assert_eq!(
                (held["s3BucketName"].as_str(), held["s3KeyPrefix"].as_str()),
                (Some(bucket.as_str()), row.prefix),
                "case {case:?}: expected the held channel to carry the bucket and prefix it \
                 was put with, got {read}"
            );
            continue;
        };
        let expected = denied.message(&bucket, row.prefix);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The ONE statement of the bucket's policy decides, and nothing else: the identical request
/// is taken while the bucket's policy carries the grant of `s3:GetBucketAcl` to the Config
/// service principal, refused as soon as that statement alone is dropped — with nothing
/// persisted, so the name does not resolve — and taken again the moment it is restored, the
/// recorder's role carrying the managed policy that grants it that same read throughout.
///
/// The rows differ along which of the two named calls the dropped statement grants, since the
/// rule holds for each call Config makes on the bucket and words the refusal after it.
#[tokio::test]
async fn dropping_one_statement_refuses_the_identical_put_until_it_is_restored() {
    // (case, the policy left on the bucket while the put must be refused, the denied call)
    let rows: [(&str, BucketPolicy, Denied); 2] = [
        (
            "the permissions check",
            bucket_policy_without_the_acl_read,
            Denied::BucketAcl,
        ),
        (
            "the delivery statement",
            bucket_policy_without_the_delivery,
            Denied::ObjectWrite,
        ),
    ];

    for (index, (case, without, denied)) in rows.iter().enumerate() {
        let emulator = Emulator::with_s3();
        let bucket = format!("statement-bucket-{index}");
        let role_name = format!("statement-role-{index}");
        let channel = format!("statement-channel-{index}");
        emulator.create_bucket(&bucket).await;
        emulator
            .put_bucket_policy(&bucket, documented_bucket_policy(&bucket))
            .await;
        let role = emulator.create_config_role(&role_name).await;
        // The role is granted what AWS publishes for a configuration recorder, so whatever it
        // is allowed is in force for every leg below and cannot be what differs.
        emulator
            .attach_role_policy(&role_name, CONFIG_ROLE_MANAGED_POLICY)
            .await;
        with_recorder(&emulator, &role).await;

        // The control leg: the documented policy admits the channel, which reads back.
        let request = channel_request(&channel, &bucket);
        emulator.ok("PutDeliveryChannel", &request).await;
        let read = emulator
            .ok(
                "DescribeDeliveryChannels",
                &json!({ "DeliveryChannelNames": [&channel] }),
            )
            .await;
        assert_eq!(
            read["DeliveryChannels"][0]["s3BucketName"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected the admitted channel to name its bucket, got {read}"
        );
        emulator
            .ok(
                "DeleteDeliveryChannel",
                &json!({ "DeliveryChannelName": &channel }),
            )
            .await;

        // The same request, against the same bucket with that one statement dropped.
        emulator.put_bucket_policy(&bucket, without(&bucket)).await;
        let (status, body) = emulator.call("PutDeliveryChannel", &request).await;
        let expected = denied.message(&bucket, None);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;

        // The statement restored, the identical request is taken again.
        emulator
            .put_bucket_policy(&bucket, documented_bucket_policy(&bucket))
            .await;
        emulator.ok("PutDeliveryChannel", &request).await;
        let listed = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
        assert_eq!(
            listed["DeliveryChannels"][0]["name"].as_str(),
            Some(channel.as_str()),
            "case {case:?}: expected the restored statement to admit the channel again, got \
             {listed}"
        );
    }
}

/// How long a poll waits for the delivery the clock owes, which falls due a fraction of a
/// second after the call that scheduled it: many passes' worth of slack, not a span the
/// behaviour depends on.
const DELIVERY_BUDGET: Duration = Duration::from_secs(15);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `lastStatus` a delivery that wrote its object reports, and the one a delivery that did
/// not: the members of `com.amazonaws.configservice#DeliveryStatus`, upper-cased.
const SUCCESS: &str = "SUCCESS";
/// See [`SUCCESS`].
const FAILURE: &str = "FAILURE";

/// How a row builds the policy document a role carries, from the row's bucket and the prefix
/// its channel configures.
type RolePolicy = fn(&str, Option<&str>) -> String;

/// How a row builds the policy document the delivery bucket carries, from the row's bucket.
type BucketPolicy = fn(&str) -> String;

/// The same permission decides the write a PERFORMED delivery makes, read at the moment of
/// the write: a snapshot delivery the bucket's policy still allows writes its object, and one
/// whose delivery statement was taken away after the channel was taken writes nothing and is
/// reported as the failed delivery it is — the recorder's role stating the write grant
/// throughout either way, which permits none of it.
///
/// The rows differ along whether the bucket's own write grant is still in force when the
/// delivery runs, which is what the rule turns on.
#[tokio::test]
async fn a_performed_delivery_is_judged_by_the_bucket_policy_in_force_at_the_write() {
    // (case, the policy left in force on the bucket when the delivery runs, whether written)
    let rows: [(&str, BucketPolicy, bool); 2] = [
        (
            "the bucket's policy still grants the write",
            documented_bucket_policy,
            true,
        ),
        (
            "the bucket's delivery statement taken away after the channel was taken",
            bucket_policy_without_the_delivery,
            false,
        ),
    ];

    for (index, (case, left_in_force, written)) in rows.iter().enumerate() {
        let emulator = Emulator::with_s3();
        let bucket = format!("performed-bucket-{index}");
        let role_name = format!("performed-role-{index}");
        let channel = format!("performed-channel-{index}");
        emulator.create_bucket(&bucket).await;
        emulator
            .put_bucket_policy(&bucket, documented_bucket_policy(&bucket))
            .await;
        let role = emulator.create_config_role(&role_name).await;
        // The role states the whole delivery grant for every row, so what differs at the
        // write is what the BUCKET's policy says and nothing else.
        emulator
            .put_role_policy(&role_name, documented_role_policy(&bucket, None))
            .await;
        with_recorder(&emulator, &role).await;
        emulator
            .ok("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        emulator
            .ok(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": "the-recorder" }),
            )
            .await;
        // The channel is taken under the full policy — the row then replaces the bucket's
        // policy with the document IT leaves in force, so what differs at the write is the
        // permission and not the channel.
        emulator
            .put_bucket_policy(&bucket, left_in_force(&bucket))
            .await;

        let scheduled = emulator
            .ok(
                "DeliverConfigSnapshot",
                &json!({ "deliveryChannelName": &channel }),
            )
            .await;
        assert!(
            scheduled["configSnapshotId"].is_string(),
            "case {case:?}: expected a scheduled snapshot id, got {scheduled}"
        );

        // Wait for the delivery to reach a terminal status rather than for any span of time.
        let deadline = Instant::now() + DELIVERY_BUDGET;
        let info = loop {
            let reported = emulator
                .ok(
                    "DescribeDeliveryChannelStatus",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            let info = reported["DeliveryChannelsStatus"][0]["configSnapshotDeliveryInfo"].clone();
            if info["lastStatus"].is_string() {
                break info;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the delivery to settle, got {info}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };

        let keys = emulator.keys(&bucket).await;
        if *written {
            assert_eq!(
                info["lastStatus"],
                json!(SUCCESS),
                "case {case:?}: expected the delivery to succeed, got {info}"
            );
            assert_eq!(
                keys.len(),
                1,
                "case {case:?}: expected one delivered object, got {keys:?}"
            );
            assert!(
                keys[0].starts_with(&format!("{}/Config/", delivery_path(None))),
                "case {case:?}: expected the object under this account's delivery path, got \
                 {keys:?}"
            );
            continue;
        }
        let expected = Denied::ObjectWrite.message(&bucket, None);
        assert_eq!(
            (
                info["lastStatus"].as_str(),
                info["lastErrorCode"].as_str(),
                info["lastErrorMessage"].as_str(),
            ),
            (
                Some(FAILURE),
                Some(INSUFFICIENT_POLICY),
                Some(expected.as_str()),
            ),
            "case {case:?}: expected the delivery reported as failed with {expected:?}, got \
             {info}"
        );
        assert!(
            keys.is_empty(),
            "case {case:?}: expected no delivered object, got {keys:?}"
        );
    }
}
