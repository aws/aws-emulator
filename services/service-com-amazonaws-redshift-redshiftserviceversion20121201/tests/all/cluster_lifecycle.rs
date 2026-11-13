// SPDX-License-Identifier: CC0-1.0

//! The provisioned-cluster lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize. The suite asserts
//! RULES over tables of inputs, not one recorded request: identifier syntax, the
//! documented `MaxRecords` window, the cluster-type/node-count agreement, the
//! retention-period ranges, and the state a write leaves for a later read.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Snapshot;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for, so the derived endpoint
/// hostnames and ARNs are predictable.
///
/// `pub(crate)` so a sibling suite that builds an ARN for the same endpoint reads
/// the region from here rather than repeating the literal.
pub(crate) const REGION: &str = "us-west-2";

/// How long a poll loop of this harness is given before it calls the condition it
/// waits on unobservable — generous next to the span a clock-owed transition is
/// due after, so a loaded runner cannot fail a suite, and far short of a client's
/// patience.
///
/// `pub(crate)` with [`POLL_INTERVAL`] and [`Emulator::poll_until`] so a suite
/// waits for the CONDITION through this one copy of the loop rather than sleeping
/// for a fixed time, which is what makes a test pass or fail by how fast the host
/// runs.
pub(crate) const POLL_DEADLINE: Duration = Duration::from_secs(10);

/// The gap between two polls of one loop.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How many FRESH resources a case is allowed to start before it calls a
/// transient state unobservable.
///
/// A resource is transient for a whole span, and two reads of it take
/// microseconds, so only an attempt that happens to straddle the due instant
/// misses the window — and each attempt starts its own resource, so a loaded
/// runner costs a case more attempts rather than failing it. An implementation
/// where reading settles the resource leaves no attempt a window at all.
const TRANSIENT_ATTEMPTS: usize = 64;

/// The IAM Identity Center instances the ACCOUNT holds in every suite of this
/// crate.
///
/// An IAM Identity Center application is a resource of its instance, so a create
/// that registers one can only name an instance the account holds
/// (`src/state/identity_center.rs`) — a suite that registers an application names
/// one of these, seeded into the Identity Center store the way that service's own
/// `CreateInstance` writes them, which is where this crate's gate reads them. An
/// ARN outside this list names no instance of the account, which is the refusal
/// [`crate::identity_center_dependency`] pins.
///
/// The forms differ along what the owning service's `InstanceArn` pattern admits
/// (`^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`): both spellings of
/// the instance id and the punctuation it allows.
pub(crate) const IDENTITY_CENTER_INSTANCES: [&str; 4] = [
    "arn:aws:sso:::instance/ssoins-1111111111111111",
    "arn:aws:sso:::instance/ssoins-2222222222222222",
    "arn:aws:sso:::instance/ins-abcdef0123456789",
    "arn:aws:sso:::instance/ssoins-a.b-c.d-e.f-g1h2",
];

/// Put the account's IAM Identity Center instances into the Identity Center store
/// in `engine`, exactly where that service's own `CreateInstance` writes them.
///
/// Seeded at mount, before any request, because the managed application a Redshift
/// IAM Identity Center application is backed by is created in an instance the
/// ACCOUNT holds and a create naming any other instance is refused — so a suite
/// about those applications needs the instances it names to exist, the same way it
/// needs a cluster to exist before it modifies one. `ambient_instance_enabled` is
/// set with them: the account's enablement is then decided, so the instances it
/// holds are exactly these.
fn seed_identity_center_instances(engine: &Arc<engine::InMemoryEngine>) {
    use service_com_amazonaws_ssoadmin_swbexternalservice::state as identity_center;

    let state = engine.state::<identity_center::ComAmazonawsSsoadminSwbexternalserviceState>();
    let mut guard = state.lock();
    for instance_arn in IDENTITY_CENTER_INSTANCES {
        let instance_id = instance_arn
            .rsplit('/')
            .next()
            .expect("the seeded instance ARN names an instance");
        let instance = identity_center::Instance::active(
            instance_id,
            identity_center::SEED_IDENTITY_STORE_ID,
            identity_center::SEED_INSTANCE_TYPE,
            Some(identity_center::SEED_INSTANCE_NAME.to_string()),
            ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            REGION,
        );
        guard
            .instances
            .insert(instance.instance_arn.clone(), instance);
    }
    guard.ambient_instance_enabled = true;
}

/// One emulator: a single shared engine behind a router mounting Redshift and
/// Amazon EC2 — the service that owns the subnets a cluster subnet group is made
/// of — and optionally the KMS service a handler resolves a key through.
///
/// `pub(crate)` so a sibling suite under `tests/all/` drives requests through the
/// same harness instead of writing a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the router handed the service, so a suite can seed a
    /// record no implemented operation mints.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

/// A served response, reduced to what the assertions read.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// What ONE awsJson service of this emulator is addressed and signed as: the four
/// values the request builder [`Emulator::call_json`] differs on per service.
#[derive(Debug, Clone, Copy)]
struct JsonService {
    /// The first label of the endpoint host the call is addressed to.
    endpoint_prefix: &'static str,
    /// The service name in the SigV4 credential scope, which is what routes the
    /// call to that service.
    signing_name: &'static str,
    /// The `content-type` of the awsJson version it speaks.
    content_type: &'static str,
    /// The prefix the operation name travels behind in `x-amz-target`.
    target_prefix: &'static str,
}

impl Answer {
    /// The `<Code>` of an awsQuery `<ErrorResponse>`, or `None` on a success.
    pub(crate) fn error_code(&self) -> Option<String> {
        first_text(&self.body, "Code")
    }

    /// The text of the first element with this local name, anywhere in the body.
    pub(crate) fn text(&self, tag: &str) -> Option<String> {
        first_text(&self.body, tag)
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::build(false)
    }

    /// An emulator with KMS mounted beside Redshift on the SAME engine, exactly as
    /// the assembled emulator mounts them.
    ///
    /// `CreateSnapshotCopyGrant` resolves the `KmsKeyId` it is given by calling KMS
    /// in process, so a suite that is about that resolution needs a KMS to reach;
    /// [`Emulator::new`] deliberately leaves it unmounted, which is the "KMS said
    /// nothing" case.
    pub(crate) fn with_kms() -> Self {
        Self::build(true)
    }

    /// An emulator with the SNS service mounted beside Redshift on the SAME
    /// engine, exactly as the assembled emulator mounts them.
    ///
    /// A control-plane write publishes the Redshift event it raises onto the
    /// topic each subscription scoped to that event names, by calling Amazon SNS
    /// in process, so a suite that is about those notifications needs an SNS to
    /// reach.
    pub(crate) fn with_sns() -> Self {
        let mut emulator = Self::build(false);
        emulator.router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// An emulator with the Redshift Serverless control plane mounted beside
    /// Redshift on the SAME engine, exactly as the assembled emulator mounts
    /// them.
    ///
    /// The serverless namespace and workgroup a namespace registration change
    /// names are resources of THAT service, so a suite about resolving them has
    /// to have it to reach; [`Emulator::new`] deliberately leaves it unmounted,
    /// which is the "Redshift Serverless said nothing" case.
    pub(crate) fn with_serverless() -> Self {
        let mut emulator = Self::build(false);
        emulator.router.register_service(
            service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// An emulator with Amazon DynamoDB, Amazon S3 AND the Redshift Serverless
    /// control plane mounted beside Redshift on the SAME engine, exactly as the
    /// assembled emulator mounts them.
    ///
    /// Those three own the resources the two ARNs of a zero-ETL integration name:
    /// the table or bucket a `SourceArn` names is DynamoDB's or Amazon S3's and
    /// the namespace a `TargetArn` names is the serverless control plane's, so a
    /// suite about resolving either has to have them all to reach.
    pub(crate) fn with_integration_owners() -> Self {
        let mut emulator = Self::with_serverless();
        emulator.router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &emulator.engine,
            )),
        );
        emulator
            .router
            .register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ));
        emulator
    }

    /// POST one Amazon DynamoDB operation with this JSON body, signed for
    /// [`REGION`], and answer its status and parsed body.
    ///
    /// That service speaks awsJson1_0, so the operation travels in `x-amz-target`
    /// rather than in a form field. Panics unless
    /// [`Emulator::with_integration_owners`] built this emulator — there is
    /// no DynamoDB to call otherwise.
    pub(crate) async fn call_dynamodb(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        self.call_json(
            JsonService {
                endpoint_prefix: "dynamodb",
                signing_name: "dynamodb",
                content_type: "application/x-amz-json-1.0",
                target_prefix: "DynamoDB_20120810",
            },
            operation,
            body,
        )
        .await
    }

    /// An emulator with the Amazon CloudWatch service mounted beside Redshift on
    /// the SAME engine, exactly as the assembled emulator mounts them.
    ///
    /// The health of a cluster is published as a CloudWatch metric, and a metric
    /// has ONE home: a suite that asserts what a cluster publishes reads it where
    /// a client reads it, through that service's own `GetMetricStatistics`.
    pub(crate) fn with_cloudwatch() -> Self {
        let mut emulator = Self::build(false);
        emulator.router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// POST one Amazon CloudWatch operation with this JSON body, signed for
    /// [`REGION`], and answer its status and parsed body.
    ///
    /// That service speaks awsJson1_0, so the operation travels in `x-amz-target`
    /// rather than in a form field, and the credential scope names `monitoring`,
    /// which is what routes the call to it. Panics unless
    /// [`Emulator::with_cloudwatch`] built this emulator — there is no CloudWatch
    /// to call otherwise.
    pub(crate) async fn call_cloudwatch(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        self.call_json(
            JsonService {
                endpoint_prefix: "monitoring",
                signing_name: "monitoring",
                content_type: "application/x-amz-json-1.0",
                target_prefix: "GraniteServiceVersion20100801",
            },
            operation,
            body,
        )
        .await
    }

    /// An emulator with the Redshift Data API mounted beside Redshift on the
    /// SAME engine, exactly as the assembled emulator mounts them.
    ///
    /// A cluster's DATABASES live in the SQL engine that service owns, so a suite
    /// about what a snapshot of a cluster holds — and what a restore from it
    /// serves — has to have it to run statements through.
    pub(crate) fn with_data_api() -> Self {
        let mut emulator = Self::build(false);
        emulator.router.register_service(
            service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService::new(Arc::clone(
                &emulator.engine,
            )),
        );
        emulator
    }

    /// POST one Redshift Data API operation with this JSON body, signed for
    /// [`REGION`], and answer its status and parsed body.
    ///
    /// That service speaks awsJson1_1, so the operation travels in `x-amz-target`
    /// rather than in a form field, and the credential scope names
    /// `redshift-data`, which is what routes the call to it. Panics unless
    /// [`Emulator::with_data_api`] built this emulator — there is no Data API to
    /// call otherwise.
    pub(crate) async fn call_data_api(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        self.call_json(
            JsonService {
                endpoint_prefix: "redshift-data",
                signing_name: "redshift-data",
                content_type: "application/x-amz-json-1.1",
                target_prefix: "RedshiftData",
            },
            operation,
            body,
        )
        .await
    }

    /// POST one Amazon Redshift Serverless operation with this JSON body,
    /// signed for [`REGION`], and answer its status and parsed body.
    ///
    /// That service speaks awsJson1_1, so the operation travels in `x-amz-target`
    /// rather than in a form field. Panics unless [`Emulator::with_serverless`]
    /// built this emulator — there is no serverless control plane to call
    /// otherwise.
    pub(crate) async fn call_serverless(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        self.call_json(
            JsonService {
                endpoint_prefix: "redshift-serverless",
                signing_name: "redshift-serverless",
                content_type: "application/x-amz-json-1.1",
                target_prefix: "RedshiftServerless",
            },
            operation,
            body,
        )
        .await
    }

    /// POST one awsJson operation of `service` with this JSON body, signed for
    /// [`REGION`], and answer its status and parsed body.
    ///
    /// The ONE request builder the three awsJson services this suite reaches are
    /// driven through: they differ only in the four values [`JsonService`] carries,
    /// so a new one is a row rather than another copy of this function.
    async fn call_json(
        &self,
        service: JsonService,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://{}.{REGION}.amazonaws.com/",
                service.endpoint_prefix
            ))
            .header("content-type", service.content_type)
            .header(
                "x-amz-target",
                format!("{}.{operation}", service.target_prefix),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{}/aws4_request, SignedHeaders=host;x-amz-target, Signature=0",
                    service.signing_name
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the test request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    /// A handle to this emulator for driving it through an AWS SDK client: the
    /// same in-process seam a handler reaches another mounted service through.
    pub(crate) fn callback(&self) -> smithy_server_runtime::EmbeddedEmulator {
        smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone())
    }

    /// An emulator with the Amazon S3 service mounted beside Redshift on the
    /// SAME engine, exactly as the assembled emulator mounts them.
    ///
    /// The bucket an `EnableLogging` names is a resource of THAT service, and
    /// both facts this service needs about it — that the account has it, and
    /// that its policy admits the delivery — are resolved by calling it in
    /// process, so a suite about those preconditions has to have one to reach;
    /// [`Emulator::new`] deliberately leaves it unmounted, which is the "Amazon
    /// S3 said nothing" case.
    pub(crate) fn with_s3() -> Self {
        let mut emulator = Self::build(false);
        emulator
            .router
            .register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&emulator.engine),
            ));
        emulator
    }

    /// An Amazon S3 client for [`REGION`] over this emulator's own in-process
    /// callback — the very seam the Redshift handlers resolve a log bucket
    /// through.
    ///
    /// Path style for the same reason the handler's client is: the emulator
    /// serves one endpoint for every service, so the bucket travels in the path.
    pub(crate) fn s3(&self) -> ::aws_sdk_s3::Client {
        let callback = self.callback();
        let config = ::aws_sdk_s3::Config::builder()
            .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
            .region(::aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .force_path_style(true)
            .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_s3::Client::from_conf(config)
    }

    /// An emulator with Redshift ALONE: the service that owns subnets is not
    /// mounted, so a handler that resolves a subnet identifier gets no answer at
    /// all.
    ///
    /// That is the "EC2 said nothing" deployment, and what a cluster subnet group
    /// does with it is part of the contract (`services/common/src/network.rs`),
    /// so a suite about that case needs an emulator without it.
    pub(crate) fn without_subnet_owner() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        seed_identity_center_instances(&engine);
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    fn build(with_kms: bool) -> Self {
        // The one shared engine, exactly as `router/src/lib.rs` builds it: every
        // request in this suite observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        seed_identity_center_instances(&engine);
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // Amazon EC2 beside Redshift on the SAME engine, exactly as the assembled
        // emulator mounts them: a subnet, the VPC that contains it and the zone it
        // lies in are EC2's facts, and a cluster subnet group resolves the subnets
        // a request names against that service — so a suite that creates one needs
        // it to reach.
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        if with_kms {
            router.register_service(
                service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        Self { router, engine }
    }

    /// An EC2 client for [`REGION`] over this emulator's own in-process callback —
    /// the very seam the Redshift handlers resolve a subnet through.
    pub(crate) fn ec2(&self) -> ::aws_sdk_ec2::Client {
        let callback = self.callback();
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// The subnets the ACCOUNT has in [`REGION`], as EC2 reports them: the default
    /// VPC's own subnets, in zone order.
    ///
    /// ASKED, never assumed — every id is one EC2 minted, which is what a Redshift
    /// subnet group resolves a `SubnetIds` entry against, so no id a case names can
    /// pass itself off as one the account holds.
    pub(crate) async fn account_subnets(&self) -> Vec<String> {
        let described = self
            .ec2()
            .describe_subnets()
            .send()
            .await
            .expect("EC2 must report the account's subnets");
        let mut reported: Vec<(String, String)> = described
            .subnets()
            .iter()
            .filter_map(|subnet| {
                Some((
                    subnet.availability_zone()?.to_string(),
                    subnet.subnet_id()?.to_string(),
                ))
            })
            .filter(|(zone, _)| zone.starts_with(REGION))
            .collect();
        reported.sort();
        reported.into_iter().map(|(_, id)| id).collect()
    }

    /// `count` subnet ids of the account, minted by EC2 in a VPC of their OWN.
    ///
    /// A cluster subnet group covers subnets of one VPC, so each call mints its
    /// own `/16` and carves a `/24` per subnet out of it: two groups built from
    /// two calls are in two different VPCs, exactly as a client that builds two
    /// networks sees them. The zones are the Region's, assigned round-robin — a
    /// Region has several and AWS admits more than one subnet per zone.
    pub(crate) async fn subnet_ids(&self, case: &str, count: usize) -> Vec<String> {
        // One RFC 1918 `/16` per call, numbered from a counter shared by the whole
        // test binary so two calls never ask for the same block.
        static NEXT_BLOCK: AtomicUsize = AtomicUsize::new(0);
        let block = NEXT_BLOCK.fetch_add(1, Ordering::Relaxed);
        assert!(
            block < 256 && count <= 256,
            "case {case:?}: the suite's RFC 1918 range holds 256 VPCs of 256 subnets"
        );
        let ec2 = self.ec2();
        let zones: Vec<String> = ec2
            .describe_availability_zones()
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: EC2 must report the Region's zones: {error:?}")
            })
            .availability_zones()
            .iter()
            .filter_map(|zone| zone.zone_name().map(str::to_string))
            .collect();
        assert!(
            !zones.is_empty(),
            "case {case:?}: EC2 must report at least one Availability Zone in {REGION}"
        );
        let vpc = ec2
            .create_vpc()
            .cidr_block(format!("10.{block}.0.0/16"))
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: EC2 must create the VPC: {error:?}"))
            .vpc()
            .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
            .unwrap_or_else(|| panic!("case {case:?}: CreateVpc must report the VPC it minted"))
            .to_string();
        let mut minted = Vec::with_capacity(count);
        for index in 0..count {
            let created = ec2
                .create_subnet()
                .vpc_id(&vpc)
                .cidr_block(format!("10.{block}.{index}.0/24"))
                .availability_zone(&zones[index % zones.len()])
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: EC2 must create subnet {index}: {error:?}")
                });
            minted.push(
                created
                    .subnet()
                    .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: CreateSubnet must report the subnet it minted")
                    })
                    .to_string(),
            );
        }
        minted
    }

    /// Mint a KMS key in `region` through the mounted KMS service and answer its
    /// ARN, so a `KmsKeyId` a case means to be REAL is one KMS can resolve.
    ///
    /// Panics unless [`Emulator::with_kms`] built this emulator — there is no KMS to
    /// create a key in otherwise.
    pub(crate) async fn create_kms_key(&self, region: &str) -> String {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://kms.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", "TrentService.CreateKey")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kms/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from_static(b"{}"))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).into_owned();
        assert_eq!(
            status, 200,
            "case \"mint a real KMS key\": expected status 200, got {status} with body {body}"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("CreateKey answers a JSON body");
        parsed["KeyMetadata"]["Arn"]
            .as_str()
            .expect("CreateKey reports the new key's ARN")
            .to_string()
    }

    /// Seed a snapshot Redshift took on its OWN schedule.
    ///
    /// Nothing in this slice mints one — `CreateClusterSnapshot` is documented to
    /// produce `manual` — so an `automated` record is the only way to make the
    /// `SOURCE_TYPE` ordering observable at all. Written straight into the same
    /// shared store the handlers read, so the request path is unchanged.
    fn seed_automated_snapshot(&self, snapshot_identifier: &str, cluster_identifier: &str) {
        let store = state::service_state(&*self.engine).expect("the Redshift store");
        let mut store = store.lock();
        store.snapshots.push(state::StoredSnapshot {
            snapshot: Snapshot {
                snapshot_identifier: Some(snapshot_identifier.to_string()),
                cluster_identifier: Some(cluster_identifier.to_string()),
                snapshot_create_time: Some(aws_smithy_types::DateTime::from(
                    std::time::SystemTime::now(),
                )),
                status: Some("available".to_string()),
                snapshot_type: Some("automated".to_string()),
                ..Default::default()
            },
            // Already past, so the record needs no clock-owed transition.
            available_at: std::time::Instant::now(),
            // A seeded record was taken of no cluster this store holds, so it
            // carries no source configuration for a restore to rebuild from,
            // and no copy of any cluster's databases either.
            source: None,
            data: Default::default(),
        });
    }

    /// POST one awsQuery action with these form parameters, signed for [`REGION`].
    pub(crate) async fn call(&self, action: &str, params: &[(&str, &str)]) -> Answer {
        self.call_in_region(REGION, action, params).await
    }

    /// POST one awsQuery action with these form parameters, signed for `region`.
    ///
    /// The Region reaches a handler only through the request's SigV4 credential
    /// scope, so a suite asserting a Region-derived value drives the call through
    /// here rather than building its own request.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> Answer {
        let mut body = format!("Action={action}&Version=2012-12-01");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://redshift.{region}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/redshift/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: String::from_utf8_lossy(response.body()).into_owned(),
        }
    }

    /// Call this REPORTING action until `settled` holds of the answer it serves,
    /// answering that answer and how many polls it took; panics quoting the last
    /// answer when [`POLL_DEADLINE`] passes first.
    ///
    /// The loop makes no mutating call, so only a transition the clock already
    /// owed can end it — and it waits for that CONDITION rather than for a fixed
    /// time, so a slow or loaded host only costs it more polls.
    pub(crate) async fn poll_until(
        &self,
        case: &str,
        action: &str,
        params: &[(&str, &str)],
        settled: impl Fn(&Answer) -> bool,
    ) -> (Answer, usize) {
        let deadline = Instant::now() + POLL_DEADLINE;
        let mut polls = 0usize;
        loop {
            let read = self.call(action, params).await;
            polls += 1;
            assert_eq!(
                read.status, 200,
                "case {case}: poll {polls} expected {action} 200, got {} with body {}",
                read.status, read.body
            );
            if settled(&read) {
                return (read, polls);
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: {action} never reported the condition within \
                 {POLL_DEADLINE:?} ({polls} polls); last answer: {}",
                read.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Create a cluster with just the three required members.
    pub(crate) async fn create_minimal(&self, identifier: &str) -> Answer {
        self.call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
        )
        .await
    }
}

/// Percent-encode one form value, escaping everything outside the RFC 3986
/// unreserved characters so a new case cannot silently send a malformed body.
///
/// Hand-written rather than taken from `percent-encoding`, for the same reason
/// as [`all_text`]: this suite must not add an entry to the workspace lockfile.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The text of every `<tag>…</tag>` element in an awsQuery response body, in
/// document order.
///
/// Deliberately a scanner over this one wire format rather than a general XML
/// parser from crates.io: adding a crate would add an entry to the workspace
/// `Cargo.lock`, which mainline also rewrites continuously, and the gate's
/// squash merge cannot resolve a lockfile both sides changed. The emulator's
/// awsQuery envelope is element-only, carries no attributes on the members this
/// suite reads, and is produced by the framework's own escaping serializer, so
/// matching one element's open/close pair is exact for this input. Element names
/// are matched whole (`<Port>` cannot match `<PortRange>`).
pub(crate) fn all_text(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let inner = &rest[start + open.len()..];
        let Some(end) = inner.find(&close) else { break };
        found.push(unescape(&inner[..end]));
        rest = &inner[end + close.len()..];
    }
    found
}

/// The five XML entities the framework's serializer emits.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text of the first `<tag>` element in the body.
fn first_text(xml: &str, tag: &str) -> Option<String> {
    all_text(xml, tag).into_iter().next()
}

/// The inner XML of every `<name>…</name>` element in a body, in document order.
///
/// Needed beside [`all_text`] for a STRUCTURE rather than a scalar, and written
/// with two allowances an exact `<name>` match cannot make: the framework's
/// serializer writes the service's XML namespace onto the FIRST member of a
/// list, so that member's open tag carries an attribute, and a list's wrapper
/// shares its entries' element-name prefix (`<ReservedNodes>` around
/// `<ReservedNode>`) — so only a `>` or a space may follow the name.
///
/// `pub(crate)` for the same reason as the harness above: a sibling suite under
/// `tests/all/` reads a served structure through this one scanner rather than
/// keeping a second copy of it.
pub(crate) fn element_blocks(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let attributes = &after[..open_end];
        if !(attributes.is_empty() || attributes.starts_with(' ')) {
            // A longer element name that merely starts the same way.
            rest = &after[open_end..];
            continue;
        }
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else {
            break;
        };
        blocks.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    blocks
}

/// The `<ClusterIdentifier>` of every cluster a `DescribeClusters` body carries,
/// in the order the page lists them.
fn listed_clusters(xml: &str) -> Vec<String> {
    all_text(xml, "ClusterIdentifier")
}

/// A string of `len` characters starting with a letter, for a length-bound case.
fn identifier_of_length(len: usize) -> String {
    std::iter::once('c')
        .chain(std::iter::repeat_n('x', len - 1))
        .collect()
}

/// One row of the cluster-type / node-count table: the case name, the
/// `ClusterType` and `NumberOfNodes` the request carries, and the node count the
/// created cluster must report (`None` when the pair must be refused).
type ShapeCase = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of a filter table: the case name, the request's filter parameters,
/// and the resource ids that must come back.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<&'static str>,
);

/// One row of a refusal table: the case name, the request's parameters, and
/// whether the request must be accepted.
type RefusalCase = (&'static str, Vec<(&'static str, &'static str)>, bool);

#[tokio::test]
async fn the_cluster_lifecycle_is_consistent_across_calls() {
    let emulator = Emulator::new();

    // CREATE — the record is minted in `creating`, with the creation time, no
    // endpoint yet, and the password nowhere on the wire.
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "demo-cluster"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "Passw0rd!"),
                ("ClusterType", "single-node"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case create demo-cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.text("ClusterIdentifier").as_deref(),
        Some("demo-cluster"),
        "case create demo-cluster: expected the identifier back, got {:?}",
        created.text("ClusterIdentifier")
    );
    assert_eq!(
        created.text("ClusterStatus").as_deref(),
        Some("creating"),
        "case create demo-cluster: expected ClusterStatus=creating, got {:?}",
        created.text("ClusterStatus")
    );
    // A cluster still being provisioned publishes NO endpoint: its address and
    // port appear only once it is serving, which is what stops a client that
    // polls for one from dialling a cluster that is not up.
    assert_eq!(
        created.text("Port"),
        None,
        "case create demo-cluster: expected no endpoint port while creating, got {:?}",
        created.text("Port")
    );
    assert_eq!(
        created.text("Address"),
        None,
        "case create demo-cluster: expected no endpoint address while creating, got {:?}",
        created.text("Address")
    );
    assert_eq!(
        created.text("DBName").as_deref(),
        Some("dev"),
        "case create demo-cluster: expected the default DBName dev, got {:?}",
        created.text("DBName")
    );
    assert!(
        !created.body.contains("Passw0rd"),
        "case create demo-cluster: the admin password must never be echoed, got body {}",
        created.body
    );
    assert!(
        created.text("ClusterCreateTime").is_some(),
        "case create demo-cluster: expected ClusterCreateTime to be set, got body {}",
        created.body
    );

    // A second create with the same identifier is refused.
    let duplicate = emulator.create_minimal("demo-cluster").await;
    assert_eq!(
        (duplicate.status, duplicate.error_code()),
        (400, Some("ClusterAlreadyExists".to_string())),
        "case duplicate create: expected 400 ClusterAlreadyExists, got {} / {:?}",
        duplicate.status,
        duplicate.error_code()
    );

    // DESCRIBE — the stored record comes back with the values create persisted.
    // Past the provisioning span, so the cluster is serving and publishes the
    // endpoint a client connects to.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let described = emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", "demo-cluster")],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case describe demo-cluster: expected 200, got {} with body {}",
        described.status, described.body
    );
    for (member, expected) in [
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
        ("DBName", "dev"),
        ("ClusterVersion", "1.0"),
        ("NumberOfNodes", "1"),
    ] {
        assert_eq!(
            described.text(member).as_deref(),
            Some(expected),
            "case describe demo-cluster: expected {member}={expected}, got {:?}",
            described.text(member)
        );
    }
    let address = described
        .text("Address")
        .expect("case describe demo-cluster: an available cluster publishes its endpoint");
    assert!(
        address.starts_with("demo-cluster.") && address.ends_with(".redshift.amazonaws.com"),
        "case describe demo-cluster: expected a Redshift endpoint host for the identifier, got {address:?}"
    );
    assert_eq!(
        described.text("Port").as_deref(),
        Some("5439"),
        "case describe demo-cluster: expected the default port 5439, got {:?}",
        described.text("Port")
    );

    // MODIFY — the mutation persists and a later describe shows it.
    let modified = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "demo-cluster"),
                ("AutomatedSnapshotRetentionPeriod", "7"),
            ],
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case modify retention: expected 200, got {} with body {}",
        modified.status, modified.body
    );
    let after_modify = emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", "demo-cluster")],
        )
        .await;
    assert_eq!(
        after_modify
            .text("AutomatedSnapshotRetentionPeriod")
            .as_deref(),
        Some("7"),
        "case modify retention: expected the new retention period to persist, got {:?}",
        after_modify.text("AutomatedSnapshotRetentionPeriod")
    );

    // SNAPSHOT — the identifying attributes are copied off the stored cluster.
    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "demo-snap"),
                ("ClusterIdentifier", "demo-cluster"),
            ],
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "case snapshot demo-snap: expected 200, got {} with body {}",
        snapshot.status, snapshot.body
    );
    for (member, expected) in [
        ("SnapshotIdentifier", "demo-snap"),
        ("ClusterIdentifier", "demo-cluster"),
        // The model states CreateClusterSnapshot "returns status as creating".
        ("Status", "creating"),
        ("SnapshotType", "manual"),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
        ("DBName", "dev"),
        ("Port", "5439"),
    ] {
        assert_eq!(
            snapshot.text(member).as_deref(),
            Some(expected),
            "case snapshot demo-snap: expected {member}={expected}, got {:?}",
            snapshot.text(member)
        );
    }

    let snapshots = emulator
        .call(
            "DescribeClusterSnapshots",
            &[("ClusterIdentifier", "demo-cluster")],
        )
        .await;
    assert_eq!(
        all_text(&snapshots.body, "SnapshotIdentifier"),
        vec!["demo-snap".to_string()],
        "case list snapshots of demo-cluster: expected exactly demo-snap, got body {}",
        snapshots.body
    );

    // DELETE — the cluster leaves the store and its final snapshot stays.
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "demo-cluster"),
                ("FinalClusterSnapshotIdentifier", "demo-final"),
            ],
        )
        .await;
    assert_eq!(
        (deleted.status, deleted.text("ClusterStatus")),
        (200, Some("deleting".to_string())),
        "case delete demo-cluster: expected 200 with ClusterStatus=deleting, got {} / {:?}",
        deleted.status,
        deleted.text("ClusterStatus")
    );

    let gone = emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", "demo-cluster")],
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case describe after delete: expected 404 ClusterNotFound, got {} / {:?}",
        gone.status,
        gone.error_code()
    );

    let final_snapshots = emulator.call("DescribeClusterSnapshots", &[]).await;
    let mut listed = all_text(&final_snapshots.body, "SnapshotIdentifier");
    listed.sort();
    assert_eq!(
        listed,
        vec!["demo-final".to_string(), "demo-snap".to_string()],
        "case list snapshots after delete: expected both snapshots to survive, got body {}",
        final_snapshots.body
    );
    assert_eq!(
        all_text(&final_snapshots.body, "SnapshotType")
            .iter()
            .filter(|kind| kind.as_str() == "manual")
            .count(),
        2,
        "case list snapshots after delete: both snapshots are manual, got body {}",
        final_snapshots.body
    );
}

#[tokio::test]
async fn an_identifier_is_accepted_exactly_when_it_satisfies_the_documented_syntax() {
    // The rule, from `smithy_query(query="doc", shape="CreateClusterMessage",
    // member="ClusterIdentifier")` and the identical sentences on
    // `ModifyClusterMessage$NewClusterIdentifier` /
    // `CreateClusterSnapshotMessage$SnapshotIdentifier`: 1 to N alphanumeric
    // characters or hyphens, first character a letter, no trailing hyphen and no
    // two consecutive hyphens; a CLUSTER identifier is additionally lowercase
    // only, a SNAPSHOT identifier is not, and N is 63 for a cluster and 255 for
    // a snapshot.
    let emulator = Emulator::new();

    let max = state::CLUSTER_IDENTIFIER_RULE.max_len;
    let cluster_cases: Vec<(&str, String, bool)> = vec![
        ("one letter", "c".to_string(), true),
        ("letters, digits and hyphens", "my-cluster-01".to_string(), true),
        ("at the published length bound", identifier_of_length(max), true),
        ("one past the length bound", identifier_of_length(max + 1), false),
        ("uppercase letter", "Demo-cluster".to_string(), false),
        ("leading digit", "1cluster".to_string(), false),
        ("trailing hyphen", "cluster-".to_string(), false),
        ("two consecutive hyphens", "my--cluster".to_string(), false),
        ("underscore", "my_cluster".to_string(), false),
        ("dot", "my.cluster".to_string(), false),
    ];
    for (case, identifier, accepted) in &cluster_cases {
        let answer = emulator.create_minimal(identifier).await;
        let ok = answer.status == 200;
        assert_eq!(
            ok, *accepted,
            "case ClusterIdentifier {case} ({identifier:?}): expected accepted={accepted}, got status {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if !accepted {
            assert_eq!(
                answer.error_code().as_deref(),
                Some("ValidationException"),
                "case ClusterIdentifier {case} ({identifier:?}): a refusal names the member and its constraint, got body {}",
                answer.body
            );
            assert!(
                answer.body.contains("ClusterIdentifier"),
                "case ClusterIdentifier {case} ({identifier:?}): the refusal must name the member, got body {}",
                answer.body
            );
        }
    }

    // `NewClusterIdentifier` is checked by the same rule.
    let host = emulator.create_minimal("rename-host").await;
    assert_eq!(host.status, 200, "case rename host: {}", host.body);
    for (case, identifier, accepted) in [
        ("uppercase", "Renamed", false),
        ("two consecutive hyphens", "re--named", false),
        ("valid", "renamed-host", true),
    ] {
        let answer = emulator
            .call(
                "ModifyCluster",
                &[
                    ("ClusterIdentifier", "rename-host"),
                    ("NewClusterIdentifier", identifier),
                ],
            )
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case NewClusterIdentifier {case} ({identifier:?}): expected accepted={accepted}, got status {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }

    // A snapshot identifier shares the rule but allows uppercase and runs to 255.
    let snap_max = state::SNAPSHOT_IDENTIFIER_RULE.max_len;
    assert_eq!(
        snap_max, 255,
        "the snapshot identifier bound comes from the model doc, got {snap_max}"
    );
    for (case, identifier, accepted) in [
        ("uppercase is allowed", "MySnapshot".to_string(), true),
        ("at the published length bound", identifier_of_length(snap_max), true),
        ("one past the length bound", identifier_of_length(snap_max + 1), false),
        ("trailing hyphen", "snap-".to_string(), false),
    ] {
        let answer = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", &identifier),
                    ("ClusterIdentifier", "renamed-host"),
                ],
            )
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case SnapshotIdentifier {case}: expected accepted={accepted}, got status {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

#[tokio::test]
async fn the_cluster_type_and_the_node_count_must_describe_one_another() {
    // From `smithy_query(query="doc", shape="CreateClusterMessage",
    // member="ClusterType")`: "Valid Values: multi-node | single-node", and
    // "When cluster type is specified as single-node, the NumberOfNodes
    // parameter is not required. multi-node, the NumberOfNodes parameter is
    // required". `NumberOfNodes` defaults to 1.
    let emulator = Emulator::new();

    let cases: [ShapeCase; 7] = [
        ("neither given", None, None, Some("1")),
        ("single-node alone", Some("single-node"), None, Some("1")),
        ("single-node with 1 node", Some("single-node"), Some("1"), Some("1")),
        ("single-node with 2 nodes", Some("single-node"), Some("2"), None),
        ("multi-node with 4 nodes", Some("multi-node"), Some("4"), Some("4")),
        ("multi-node with 1 node", Some("multi-node"), Some("1"), None),
        ("an unmodelled cluster type", Some("mega-node"), Some("2"), None),
    ];
    for (index, (case, cluster_type, nodes, expected_nodes)) in cases.into_iter().enumerate() {
        let identifier = format!("shape-{index}");
        let mut params: Vec<(&str, &str)> = vec![
            ("ClusterIdentifier", &identifier),
            ("NodeType", "dc2.large"),
            ("MasterUsername", "admin"),
        ];
        if let Some(kind) = cluster_type {
            params.push(("ClusterType", kind));
        }
        if let Some(count) = nodes {
            params.push(("NumberOfNodes", count));
        }
        let answer = emulator.call("CreateCluster", &params).await;
        match expected_nodes {
            Some(expected) => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the create to be accepted, got {} / {:?}",
                    answer.status,
                    answer.error_code()
                );
                assert_eq!(
                    answer.text("NumberOfNodes").as_deref(),
                    Some(expected),
                    "case {case}: expected NumberOfNodes={expected}, got {:?}",
                    answer.text("NumberOfNodes")
                );
            }
            None => assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case}: expected a 400 refusal, got {} / {:?} with body {}",
                answer.status,
                answer.error_code(),
                answer.body
            ),
        }
    }

    // The same rule governs a MODIFY: switching to `single-node` without naming
    // a count is the one-node shape ("the NumberOfNodes parameter is not
    // required"), while switching to `multi-node` must say how many.
    let host = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "shape-host"),
                ("NodeType", "dc2.large"),
                ("MasterUsername", "admin"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "4"),
            ],
        )
        .await;
    assert_eq!(host.status, 200, "case seed a 4-node cluster: {}", host.body);

    let to_multi_without_count = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "shape-host"),
                ("ClusterType", "single-node"),
            ],
        )
        .await;
    assert_eq!(
        to_multi_without_count.status, 200,
        "case modify to single-node without a count: expected 200, got {} / {:?}",
        to_multi_without_count.status,
        to_multi_without_count.error_code()
    );
    assert_eq!(
        to_multi_without_count.text("NumberOfNodes").as_deref(),
        Some("1"),
        "case modify to single-node without a count: expected NumberOfNodes=1, got {:?}",
        to_multi_without_count.text("NumberOfNodes")
    );

    let back_to_multi = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "shape-host"),
                ("ClusterType", "multi-node"),
            ],
        )
        .await;
    assert_eq!(
        (back_to_multi.status, back_to_multi.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case modify to multi-node without a count: expected a 400 refusal, got {} / {:?}",
        back_to_multi.status,
        back_to_multi.error_code()
    );
}

#[tokio::test]
async fn a_retention_period_is_accepted_exactly_inside_its_published_range() {
    // `AutomatedSnapshotRetentionPeriod`: "Must be a value from 0 to 35."
    // `ManualSnapshotRetentionPeriod`: "either -1 or an integer between 1 and
    // 3,653". Both from the `CreateClusterMessage` member docs.
    let emulator = Emulator::new();

    let (auto_lo, auto_hi) = state::AUTOMATED_RETENTION_BOUNDS;
    let (manual_lo, manual_hi) = state::MANUAL_RETENTION_BOUNDS;
    let cases: Vec<(String, &str, String, bool)> = vec![
        ("automated at the low bound".into(), "AutomatedSnapshotRetentionPeriod", auto_lo.to_string(), true),
        ("automated mid-range".into(), "AutomatedSnapshotRetentionPeriod", "7".into(), true),
        ("automated at the high bound".into(), "AutomatedSnapshotRetentionPeriod", auto_hi.to_string(), true),
        ("automated one below".into(), "AutomatedSnapshotRetentionPeriod", (auto_lo - 1).to_string(), false),
        ("automated one above".into(), "AutomatedSnapshotRetentionPeriod", (auto_hi + 1).to_string(), false),
        ("manual indefinite".into(), "ManualSnapshotRetentionPeriod", state::MANUAL_RETENTION_INDEFINITE.to_string(), true),
        ("manual at the low bound".into(), "ManualSnapshotRetentionPeriod", manual_lo.to_string(), true),
        ("manual at the high bound".into(), "ManualSnapshotRetentionPeriod", manual_hi.to_string(), true),
        ("manual at zero".into(), "ManualSnapshotRetentionPeriod", "0".into(), false),
        ("manual one above".into(), "ManualSnapshotRetentionPeriod", (manual_hi + 1).to_string(), false),
    ];
    for (index, (case, member, value, accepted)) in cases.into_iter().enumerate() {
        let identifier = format!("retention-{index}");
        let answer = emulator
            .call(
                "CreateCluster",
                &[
                    ("ClusterIdentifier", &identifier),
                    ("NodeType", "dc2.large"),
                    ("MasterUsername", "admin"),
                    (member, &value),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case} ({member}={value}): expected 200, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
            assert_eq!(
                answer.text(member).as_deref(),
                Some(value.as_str()),
                "case {case}: expected {member} to be reported back as {value}, got {:?}",
                answer.text(member)
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidRetentionPeriodFault".to_string())),
                "case {case} ({member}={value}): expected 400 InvalidRetentionPeriodFault, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The same range governs the final snapshot a delete takes.
    let host = emulator.create_minimal("retention-host").await;
    assert_eq!(host.status, 200, "case delete host: {}", host.body);
    let refused = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "retention-host"),
                ("FinalClusterSnapshotIdentifier", "retention-final"),
                ("FinalClusterSnapshotRetentionPeriod", "0"),
            ],
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("InvalidRetentionPeriodFault".to_string())),
        "case final snapshot retention 0: expected 400 InvalidRetentionPeriodFault, got {} / {:?}",
        refused.status,
        refused.error_code()
    );
}

#[tokio::test]
async fn pagination_walks_the_whole_collection_through_the_marker() {
    // "Default: 100. Constraints: minimum 20, maximum 100." — the `MaxRecords`
    // doc shared by `DescribeClustersMessage` and
    // `DescribeClusterSnapshotsMessage`.
    let emulator = Emulator::new();

    let total = 25usize;
    for index in 0..total {
        let identifier = format!("page-{index:02}");
        let answer = emulator.create_minimal(&identifier).await;
        assert_eq!(
            answer.status, 200,
            "case seed {identifier}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    for (case, max_records, accepted) in [
        ("one below the minimum", (state::MAX_RECORDS_MIN - 1).to_string(), false),
        ("at the minimum", state::MAX_RECORDS_MIN.to_string(), true),
        ("at the maximum", state::MAX_RECORDS_MAX.to_string(), true),
        ("one above the maximum", (state::MAX_RECORDS_MAX + 1).to_string(), true),
        ("far above the maximum", (state::MAX_RECORDS_MAX * 10).to_string(), true),
        ("zero", "0".to_string(), false),
    ] {
        let answer = emulator
            .call("DescribeClusters", &[("MaxRecords", &max_records)])
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({max_records}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        let snapshots = emulator
            .call("DescribeClusterSnapshots", &[("MaxRecords", &max_records)])
            .await;
        assert_eq!(
            snapshots.status == 200,
            accepted,
            "case MaxRecords {case} ({max_records}) on snapshots: expected accepted={accepted}, got {} / {:?}",
            snapshots.status,
            snapshots.error_code()
        );
    }

    // A truncated page carries a marker; the last page does not.
    let page_size = state::MAX_RECORDS_MIN as usize;
    let first = emulator
        .call(
            "DescribeClusters",
            &[("MaxRecords", &state::MAX_RECORDS_MIN.to_string())],
        )
        .await;
    let first_page = listed_clusters(&first.body);
    assert_eq!(
        first_page.len(),
        page_size,
        "case first page: expected {page_size} clusters, got {:?}",
        first_page
    );
    let marker = first
        .text("Marker")
        .expect("case first page: a truncated page returns a Marker");

    let second = emulator
        .call(
            "DescribeClusters",
            &[
                ("MaxRecords", &state::MAX_RECORDS_MIN.to_string()),
                ("Marker", &marker),
            ],
        )
        .await;
    let second_page = listed_clusters(&second.body);
    assert_eq!(
        second_page.len(),
        total - page_size,
        "case second page: expected the remaining {} clusters, got {:?}",
        total - page_size,
        second_page
    );
    assert_eq!(
        second.text("Marker"),
        None,
        "case second page: the last page returns no Marker, got {:?}",
        second.text("Marker")
    );

    let walked: Vec<String> = first_page.into_iter().chain(second_page).collect();
    let expected: Vec<String> = (0..total).map(|index| format!("page-{index:02}")).collect();
    assert_eq!(
        walked, expected,
        "case walk every page: expected creation order with no gap or repeat, got {walked:?}"
    );

    // A marker nobody handed out is refused rather than silently reset — a
    // decimal offset the caller invented included, since the cursor is not just
    // a number.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "7"),
        ("an offset with a forged tag", "20.00000000"),
    ] {
        let bogus = emulator.call("DescribeClusters", &[("Marker", cursor)]).await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }

    // "You can specify either the ClusterIdentifier parameter or the Marker
    // parameter, but not both."
    let both = emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", "page-00"), ("Marker", &marker)],
        )
        .await;
    assert_eq!(
        (both.status, both.error_code()),
        (400, Some("InvalidParameterCombination".to_string())),
        "case identifier and marker together: expected a 400 refusal, got {} / {:?}",
        both.status,
        both.error_code()
    );
}

/// Provisioning settles on the CLOCK alone, and a read materialises only what
/// the clock already owed: it neither moves the resource on by itself nor jumps
/// the span.
///
/// Both halves are decided by a CONDITION, never by how fast the host runs. The
/// transient half retries on a FRESH cluster until one attempt's two
/// back-to-back reads both land inside the span: a read never advances a
/// resource, so such an attempt reports the transient status twice, while an
/// attempt whose reads straddle the due instant observed no transient window at
/// all and is retried rather than asserted on. An implementation where a read
/// settles the resource — before or after reporting it — produces no such
/// attempt for any cluster, so [`TRANSIENT_ATTEMPTS`] attempts all failing to
/// see it is what fails the test; nothing here is bounded by the clock. The
/// settled half then polls each reporting operation until the clock-owed
/// transition is materialised, making no mutating call while it waits.
#[tokio::test]
async fn provisioning_settles_on_the_clock_and_a_read_changes_nothing_else() {
    let emulator = Emulator::new();

    let mut observed_transient = None;
    for attempt in 1..=TRANSIENT_ATTEMPTS {
        let cluster = format!("clock-cluster-{attempt}");
        let snapshot_id = format!("clock-snap-{attempt}");

        let created = emulator.create_minimal(&cluster).await;
        assert_eq!(
            created.text("ClusterStatus").as_deref(),
            Some("creating"),
            "case create {cluster}: the span is never zero, so the caller sees the transient \
             status, got {:?}",
            created.text("ClusterStatus")
        );
        let snapshot = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot_id.as_str()),
                    ("ClusterIdentifier", cluster.as_str()),
                ],
            )
            .await;
        assert_eq!(
            snapshot.text("Status").as_deref(),
            Some("creating"),
            "case snapshot {snapshot_id}: expected the transient status, got {:?}",
            snapshot.text("Status")
        );

        // Two reads back to back. Both reporting the transient status is the
        // claim: a read never moves the resource on by itself.
        let first = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &cluster)])
            .await;
        let second = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &cluster)])
            .await;
        let read_statuses = (first.text("ClusterStatus"), second.text("ClusterStatus"));
        let observed = (read_statuses.0.as_deref(), read_statuses.1.as_deref());
        if observed == (Some(state::STATUS_CREATING), Some(state::STATUS_CREATING)) {
            observed_transient = Some((cluster, snapshot_id));
            break;
        }
        // Neither read may ever REGRESS, whichever side of the due instant it
        // fell on: the only transition this cluster owes is creating ->
        // available.
        for (which, status) in [("first", observed.0), ("second", observed.1)] {
            assert!(
                status == Some(state::STATUS_CREATING) || status == Some(state::STATUS_AVAILABLE),
                "case repeated read of {cluster}: expected the {which} read to report creating \
                 or available, got {status:?}"
            );
        }
        assert!(
            observed.0 != Some(state::STATUS_AVAILABLE)
                || observed.1 != Some(state::STATUS_CREATING),
            "case repeated read of {cluster}: expected no read to regress, got {:?} then {:?}",
            observed.0,
            observed.1
        );
    }
    let Some((cluster, snapshot_id)) = observed_transient else {
        panic!(
            "case repeated read: none of {TRANSIENT_ATTEMPTS} fresh clusters reported the \
             transient status to two reads in a row — a read that settles the resource it \
             reports leaves no transient window for any of them"
        );
    };

    // Past the due instant the clock-owed transition is materialised — reached
    // by polling the read alone, so nothing but the clock can have moved it.
    let (settled, _) = emulator
        .poll_until(
            "settled cluster",
            "DescribeClusters",
            &[("ClusterIdentifier", &cluster)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
    assert_eq!(
        settled.text("ClusterStatus").as_deref(),
        Some("available"),
        "case settled cluster: expected available once the span has passed, got {:?}",
        settled.text("ClusterStatus")
    );
    assert_eq!(
        settled.text("ClusterAvailabilityStatus").as_deref(),
        Some("Available"),
        "case settled cluster: expected the availability status to follow, got {:?}",
        settled.text("ClusterAvailabilityStatus")
    );
    // And a read of the settled cluster still changes nothing: the next one
    // reports the same thing.
    let again = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", &cluster)])
        .await;
    assert_eq!(
        (
            again.text("ClusterStatus"),
            again.text("ClusterAvailabilityStatus")
        ),
        (
            settled.text("ClusterStatus"),
            settled.text("ClusterAvailabilityStatus")
        ),
        "case settled cluster read twice: expected the second read to agree, got {:?} / {:?}",
        again.text("ClusterStatus"),
        again.text("ClusterAvailabilityStatus")
    );

    let (settled_snapshots, _) = emulator
        .poll_until(
            "settled snapshot",
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", &snapshot_id)],
            |answer| answer.text("Status").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
    assert_eq!(
        settled_snapshots.text("Status").as_deref(),
        Some("available"),
        "case settled snapshot: expected available once the span has passed, got {:?}",
        settled_snapshots.text("Status")
    );
}

#[tokio::test]
async fn snapshots_are_filtered_by_cluster_type_and_existence() {
    let emulator = Emulator::new();

    for identifier in ["filter-a", "filter-b"] {
        let answer = emulator.create_minimal(identifier).await;
        assert_eq!(answer.status, 200, "case seed {identifier}: {}", answer.body);
    }
    for (snapshot, cluster) in [("snap-a1", "filter-a"), ("snap-a2", "filter-a"), ("snap-b1", "filter-b")] {
        let answer = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot),
                    ("ClusterIdentifier", cluster),
                ],
            )
            .await;
        assert_eq!(answer.status, 200, "case seed {snapshot}: {}", answer.body);
    }

    let cases: [FilterCase; 4] = [
        (
            "by cluster",
            vec![("ClusterIdentifier", "filter-a")],
            vec!["snap-a1", "snap-a2"],
        ),
        (
            "by snapshot id",
            vec![("SnapshotIdentifier", "snap-b1")],
            vec!["snap-b1"],
        ),
        (
            "by the manual type",
            vec![("SnapshotType", "manual")],
            vec!["snap-a1", "snap-a2", "snap-b1"],
        ),
        (
            "by the automated type",
            vec![("SnapshotType", "automated")],
            vec![],
        ),
    ];
    for (case, params, expected) in cases {
        let answer = emulator.call("DescribeClusterSnapshots", &params).await;
        assert_eq!(
            answer.status, 200,
            "case filter {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let mut listed = all_text(&answer.body, "SnapshotIdentifier");
        listed.sort();
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case filter {case}: unexpected snapshot set, got body {}",
            answer.body
        );
    }

    // An unmodelled snapshot type is refused, not quietly matched.
    let bad_type = emulator
        .call("DescribeClusterSnapshots", &[("SnapshotType", "hourly")])
        .await;
    assert_eq!(
        (bad_type.status, bad_type.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case an unmodelled snapshot type: expected a 400 refusal, got {} / {:?}",
        bad_type.status,
        bad_type.error_code()
    );

    // `ClusterExists` splits the snapshots whose cluster is still there from
    // those whose cluster has gone. "If ClusterExists is set to true,
    // ClusterIdentifier is required"; a `false` needs none, and then reports
    // every orphaned snapshot.
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "filter-b"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(deleted.status, 200, "case delete filter-b: {}", deleted.body);

    let missing_identifier = emulator
        .call("DescribeClusterSnapshots", &[("ClusterExists", "true")])
        .await;
    assert_eq!(
        (missing_identifier.status, missing_identifier.error_code()),
        (400, Some("InvalidParameterCombination".to_string())),
        "case ClusterExists=true without ClusterIdentifier: expected a 400 refusal, got {} / {:?}",
        missing_identifier.status,
        missing_identifier.error_code()
    );

    let cases: [FilterCase; 3] = [
        (
            "an existing cluster",
            vec![("ClusterExists", "true"), ("ClusterIdentifier", "filter-a")],
            vec!["snap-a1", "snap-a2"],
        ),
        (
            "every orphaned snapshot",
            vec![("ClusterExists", "false")],
            vec!["snap-b1"],
        ),
        (
            "an existing cluster asked for as orphaned",
            vec![("ClusterExists", "false"), ("ClusterIdentifier", "filter-a")],
            vec![],
        ),
    ];
    for (case, params, expected) in cases {
        let answer = emulator.call("DescribeClusterSnapshots", &params).await;
        assert_eq!(
            answer.status, 200,
            "case ClusterExists {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let mut listed = all_text(&answer.body, "SnapshotIdentifier");
        listed.sort();
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case ClusterExists {case}: unexpected snapshot set, got body {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn snapshots_are_sorted_by_the_entities_the_request_names() {
    // `SnapshotAttributeToSortBy` declares SOURCE_TYPE | TOTAL_SIZE |
    // CREATE_TIME and `SortByOrder` declares ASC | DESC.
    let emulator = Emulator::new();

    let host = emulator.create_minimal("sort-host").await;
    assert_eq!(host.status, 200, "case seed cluster: {}", host.body);
    for snapshot in ["sort-1", "sort-2", "sort-3"] {
        let answer = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot),
                    ("ClusterIdentifier", "sort-host"),
                ],
            )
            .await;
        assert_eq!(answer.status, 200, "case seed {snapshot}: {}", answer.body);
    }

    // CREATE_TIME: every snapshot was taken at a distinct instant, so the ASC
    // page is the creation order and the DESC page its exact reverse. Asserting
    // the identifier SEQUENCE (not a monotone timestamp window) is what makes a
    // wrong comparator fail.
    for (case, order, expected) in [
        ("ascending", "ASC", ["sort-1", "sort-2", "sort-3"]),
        ("descending", "DESC", ["sort-3", "sort-2", "sort-1"]),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterSnapshots",
                &[
                    (
                        "SortingEntities.SnapshotSortingEntity.1.Attribute",
                        "CREATE_TIME",
                    ),
                    ("SortingEntities.SnapshotSortingEntity.1.SortOrder", order),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case sort by CREATE_TIME {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            all_text(&answer.body, "SnapshotIdentifier"),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case sort by CREATE_TIME {case}: unexpected order, got body {}",
            answer.body
        );
    }

    // SOURCE_TYPE orders on the snapshot's own type, so it is only observable
    // over a set that carries more than one: seed the `automated` snapshot
    // Redshift would have taken on its own schedule. `automated` sorts before
    // `manual`, and the three manual snapshots keep their creation order because
    // the sort is stable.
    emulator.seed_automated_snapshot("sort-auto", "sort-host");
    for (case, order, expected) in [
        (
            "ascending",
            "ASC",
            ["sort-auto", "sort-1", "sort-2", "sort-3"],
        ),
        (
            "descending",
            "DESC",
            ["sort-1", "sort-2", "sort-3", "sort-auto"],
        ),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterSnapshots",
                &[
                    (
                        "SortingEntities.SnapshotSortingEntity.1.Attribute",
                        "SOURCE_TYPE",
                    ),
                    ("SortingEntities.SnapshotSortingEntity.1.SortOrder", order),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case sort by SOURCE_TYPE {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            all_text(&answer.body, "SnapshotIdentifier"),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case sort by SOURCE_TYPE {case}: unexpected order, got body {}",
            answer.body
        );
    }

    // An unmodelled SortOrder is refused rather than quietly read as ascending:
    // `SortByOrder` is a modelled enum, so the boundary validator answers it
    // before the handler runs.
    let bad_order = emulator
        .call(
            "DescribeClusterSnapshots",
            &[
                (
                    "SortingEntities.SnapshotSortingEntity.1.Attribute",
                    "CREATE_TIME",
                ),
                (
                    "SortingEntities.SnapshotSortingEntity.1.SortOrder",
                    "DESCENDING",
                ),
            ],
        )
        .await;
    assert_eq!(
        (bad_order.status, bad_order.error_code()),
        (400, Some("ValidationError".to_string())),
        "case an unmodelled SortOrder: expected a 400 refusal, got {} / {:?}",
        bad_order.status,
        bad_order.error_code()
    );

    // An attribute this emulator cannot order by honestly is refused with the
    // operation's own modelled fault, not served in an arbitrary order.
    let unsupported = emulator
        .call(
            "DescribeClusterSnapshots",
            &[(
                "SortingEntities.SnapshotSortingEntity.1.Attribute",
                "TOTAL_SIZE",
            )],
        )
        .await;
    assert_eq!(
        (unsupported.status, unsupported.error_code()),
        (400, Some("UnsupportedOperation".to_string())),
        "case sort by TOTAL_SIZE: expected 400 UnsupportedOperation, got {} / {:?}",
        unsupported.status,
        unsupported.error_code()
    );

    // More sorting entities than there are sortable attributes is refused, so
    // the per-request sort work stays bounded.
    let mut params: Vec<(String, String)> = Vec::new();
    for index in 1..=state::MAX_SORTING_ENTITIES + 1 {
        params.push((
            format!("SortingEntities.SnapshotSortingEntity.{index}.Attribute"),
            "CREATE_TIME".to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let too_many = emulator.call("DescribeClusterSnapshots", &borrowed).await;
    assert_eq!(
        (too_many.status, too_many.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case {} sorting entities: expected a 400 refusal, got {} / {:?}",
        state::MAX_SORTING_ENTITIES + 1,
        too_many.status,
        too_many.error_code()
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_quota() {
    // The store is bounded, so no run of requests can grow it without end. The
    // bound is the emulator's own (`CLUSTER_QUOTA`), reported through the
    // operation's modelled `ClusterQuotaExceededFault`.
    let emulator = Emulator::new();

    for index in 0..state::CLUSTER_QUOTA {
        let identifier = format!("quota-{index:04}");
        let answer = emulator.create_minimal(&identifier).await;
        assert_eq!(
            answer.status, 200,
            "case fill {identifier}: expected 200 below the quota, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
    let over = emulator.create_minimal("quota-over").await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, Some("ClusterQuotaExceeded".to_string())),
        "case one past the quota: expected 400 ClusterQuotaExceeded, got {} / {:?}",
        over.status,
        over.error_code()
    );

    // Identity before capacity: at the quota, a repeat create of an existing
    // cluster still answers the more specific fault.
    let repeat = emulator.create_minimal("quota-0000").await;
    assert_eq!(
        (repeat.status, repeat.error_code()),
        (400, Some("ClusterAlreadyExists".to_string())),
        "case repeat create at the quota: expected 400 ClusterAlreadyExists, got {} / {:?}",
        repeat.status,
        repeat.error_code()
    );

    // Releasing a record makes room again: the bound is on what is held, not on
    // how many requests have ever been served.
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "quota-0000"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(deleted.status, 200, "case release one: {}", deleted.body);
    let again = emulator.create_minimal("quota-over").await;
    assert_eq!(
        again.status, 200,
        "case create after releasing one: expected 200, got {} / {:?}",
        again.status,
        again.error_code()
    );
}

#[tokio::test]
async fn a_rename_re_keys_the_cluster_and_keeps_its_namespace() {
    let emulator = Emulator::new();

    let created = emulator.create_minimal("rename-me").await;
    assert_eq!(created.status, 200, "case create: {}", created.body);
    let namespace = created
        .text("ClusterNamespaceArn")
        .expect("case create: the cluster reports a namespace ARN");
    // The endpoint is published once the cluster is serving, so the hostname a
    // rename moves is read past the provisioning span.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let serving = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "rename-me")])
        .await;
    let original_address = serving
        .text("Address")
        .expect("case create: an available cluster reports an endpoint address");

    let taken = emulator.create_minimal("already-there").await;
    assert_eq!(taken.status, 200, "case create collision target: {}", taken.body);
    let collision = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "rename-me"),
                ("NewClusterIdentifier", "already-there"),
            ],
        )
        .await;
    assert_eq!(
        (collision.status, collision.error_code()),
        (400, Some("ClusterAlreadyExists".to_string())),
        "case rename onto an existing identifier: expected 400 ClusterAlreadyExists, got {} / {:?}",
        collision.status,
        collision.error_code()
    );

    let renamed = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", "rename-me"),
                ("NewClusterIdentifier", "renamed"),
                ("Port", "5555"),
            ],
        )
        .await;
    assert_eq!(renamed.status, 200, "case rename: {}", renamed.body);

    let old = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "rename-me")])
        .await;
    assert_eq!(
        (old.status, old.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case describe the old identifier: expected 404 ClusterNotFound, got {} / {:?}",
        old.status,
        old.error_code()
    );

    let now = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "renamed")])
        .await;
    assert_eq!(
        now.text("ClusterNamespaceArn").as_deref(),
        Some(namespace.as_str()),
        "case rename: the namespace ARN identifies the cluster and must survive, got {:?}",
        now.text("ClusterNamespaceArn")
    );
    assert_eq!(
        now.text("Port").as_deref(),
        Some("5555"),
        "case rename: the new port must persist, got {:?}",
        now.text("Port")
    );
    let new_address = now
        .text("Address")
        .expect("case rename: the renamed cluster reports an endpoint address");
    assert!(
        new_address.starts_with("renamed.") && new_address != original_address,
        "case rename: the endpoint hostname carries the new name, got {new_address:?} (was {original_address:?})"
    );

    let unknown = emulator
        .call(
            "ModifyCluster",
            &[("ClusterIdentifier", "never-existed")],
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case modify an unknown cluster: expected 404 ClusterNotFound, got {} / {:?}",
        unknown.status,
        unknown.error_code()
    );
}

#[tokio::test]
async fn a_final_snapshot_is_required_unless_it_is_explicitly_skipped() {
    let emulator = Emulator::new();

    for identifier in ["final-a", "final-b", "final-c"] {
        let answer = emulator.create_minimal(identifier).await;
        assert_eq!(answer.status, 200, "case seed {identifier}: {}", answer.body);
    }

    // "The FinalClusterSnapshotIdentifier parameter must be specified if
    // SkipFinalClusterSnapshot is false. Default: false"
    let cases: [RefusalCase; 4] = [
        (
            "neither member given",
            vec![("ClusterIdentifier", "final-a")],
            false,
        ),
        (
            "skip explicitly false and no identifier",
            vec![
                ("ClusterIdentifier", "final-a"),
                ("SkipFinalClusterSnapshot", "false"),
            ],
            false,
        ),
        (
            "skip true with an identifier",
            vec![
                ("ClusterIdentifier", "final-a"),
                ("SkipFinalClusterSnapshot", "true"),
                ("FinalClusterSnapshotIdentifier", "final-a-snap"),
            ],
            false,
        ),
        (
            "an identifier and no skip",
            vec![
                ("ClusterIdentifier", "final-a"),
                ("FinalClusterSnapshotIdentifier", "final-a-snap"),
            ],
            true,
        ),
    ];
    for (case, params, accepted) in cases {
        let answer = emulator.call("DeleteCluster", &params).await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case {case}: expected accepted={accepted}, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // Skipping leaves nothing behind.
    let skipped = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "final-b"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(skipped.status, 200, "case skip the final snapshot: {}", skipped.body);

    // A final snapshot name already in use is refused and the cluster stays.
    let clash = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "final-c"),
                ("FinalClusterSnapshotIdentifier", "final-a-snap"),
            ],
        )
        .await;
    assert_eq!(
        (clash.status, clash.error_code()),
        (400, Some("ClusterSnapshotAlreadyExists".to_string())),
        "case a final snapshot name already in use: expected 400 ClusterSnapshotAlreadyExists, got {} / {:?}",
        clash.status,
        clash.error_code()
    );
    let survivors = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        listed_clusters(&survivors.body),
        vec!["final-c".to_string()],
        "case a refused delete: the cluster must stay in the store, got body {}",
        survivors.body
    );
    let snapshots = emulator.call("DescribeClusterSnapshots", &[]).await;
    assert_eq!(
        all_text(&snapshots.body, "SnapshotIdentifier"),
        vec!["final-a-snap".to_string()],
        "case skipped and refused deletes: only the one final snapshot exists, got body {}",
        snapshots.body
    );
}
