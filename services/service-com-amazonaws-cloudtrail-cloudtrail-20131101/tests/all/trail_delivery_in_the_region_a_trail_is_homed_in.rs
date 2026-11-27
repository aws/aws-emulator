// SPDX-License-Identifier: CC0-1.0

//! What a logging trail delivers in the Region it is HOMED in, whichever Region that is —
//! and what it delivers for the calls served at once.
//!
//! RULE: a trail delivers the records of the calls of the Region it covers, under the key
//! layout of THAT Region (`<key-prefix>/AWSLogs/<account>/CloudTrail/<region>/…`), and which
//! records it delivers is decided by its own event selectors alone: a MANAGEMENT call of the
//! Region reaches a trail whose selectors log management events, a DATA call reaches a trail
//! only while one of its advanced event selectors admits that very record (its category, the
//! TYPE of the resource the call acted on and — where the selector names one — that
//! resource's ARN), and a data event reaches event history never. Nothing about it is a
//! property of one Region: the partition's leader Region is where GLOBAL services record,
//! which is a fact about the events of those services and not about the delivery, so a trail
//! homed anywhere delivers the calls of its own Region exactly as one homed there does.
//!
//! The rows span the Region the whole account is served in — the leader Region, and two
//! Regions that are not it — because that is what the rule generalises over; each row runs on
//! an emulator of its own, so what one Region delivered is never read as another's. Within a
//! row the contrast is between TWO trails, each delivering under its own `S3KeyPrefix` into
//! one bucket: one that logs management events, and one whose only selector admits the data
//! events of ONE named identity pool. So an absence under a prefix is the selector in force
//! withholding the record, and not a record that landed somewhere else.
//!
//! The second suite is the same rule under CONCURRENCY: many calls served at once on a
//! multi-threaded runtime, each of which must leave exactly one delivered record — the record
//! of a call is that call's own, so serving a second call at the same time may not lose it.
//!
//! Three services are mounted on ONE engine, because the behaviour spans them: CloudTrail
//! holds the trails and decides every delivery, Amazon Cognito Identity serves the identity
//! data plane the data events belong to, and the object store is where the log files land and
//! is read back through its own API, so a delivered log file counts only if the object store
//! itself answers with it.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::emulator::{cloudtrail_bucket_policy, TARGET_PREFIX};

/// Amazon Cognito Identity's `X-Amz-Target` prefix (its model declares the service shape
/// `AWSCognitoIdentityService`) and the `eventSource` CloudTrail stamps on its records
/// (`smithy_query(query="service_info")` on that service -> `endpointPrefix:
/// cognito-identity`).
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityService.";
const COGNITO_EVENT_SOURCE: &str = "cognito-identity.amazonaws.com";

/// The `eventCategory` of a data event and of a management one, as a selector pins them and
/// as a record carries them.
const DATA_CATEGORY: &str = "Data";
const MANAGEMENT_CATEGORY: &str = "Management";

/// The type an identity pool is named by in a record's `resources` and in an advanced event
/// selector's `resources.type` field.
const IDENTITY_POOL_TYPE: &str = "AWS::Cognito::IdentityPool";

/// What CloudTrail writes in place of a value it withholds
/// (`service_common::management_events::HIDDEN_SENSITIVE_VALUE`), spelled out here so a wrong
/// constant in the implementation cannot make this suite pass.
const HIDDEN: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// The public login provider the fixture pools declare, and the token a call presents under
/// it. The token is a SECRET by the model (`LoginsMap`'s value targets the `@sensitive`
/// `IdentityProviderToken`), so no delivered byte may carry it.
const LOGIN_PROVIDER: &str = "graph.facebook.com";
const LOGIN_TOKEN: &str = "provider-token-that-no-log-file-may-carry";

/// The segments of a log-file key, and what such a file's name ends with.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The member a log file's document holds its records in.
const RECORDS_MEMBER: &str = "Records";

/// The key prefix each of the two trails of a row delivers under.
const MANAGEMENT_PREFIX: &str = "mgmt";
const DATA_PREFIX: &str = "data";

/// How many calls the concurrency suite serves at once.
const CONCURRENT_CALLS: usize = 24;

/// CloudTrail, Amazon Cognito Identity and the object store over one shared engine, with the
/// in-process callback a delivery reaches the object store through.
#[derive(Clone)]
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
    region: String,
}

impl Emulator {
    /// An emulator whose calls are all served in `region`.
    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region a call is served in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            callback,
            region: region.to_string(),
        }
    }

    /// One `X-Amz-Target` RPC call signed for this emulator's Region: its status and body.
    async fn rpc(&self, signing_name: &str, target: &str, body: &Value) -> (u16, Value) {
        let region = &self.region;
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", format!("{signing_name}.{region}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One call that has to be served.
    async fn served(&self, case: &str, signing_name: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc(signing_name, target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }

    /// One CloudTrail call that has to be served.
    async fn cloudtrail(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.served(
            case,
            "cloudtrail",
            &format!("{TARGET_PREFIX}{operation}"),
            body,
        )
        .await
    }

    /// One Amazon Cognito Identity call that has to be served.
    async fn cognito(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.served(
            case,
            "cognito-identity",
            &format!("{COGNITO_TARGET_PREFIX}{operation}"),
            body,
        )
        .await
    }

    /// The Amazon S3 client, over the same in-process callback the delivery uses and in the
    /// same Region, so the test's transport is the delivery's by construction.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, &self.region)
    }

    /// Create the one log bucket of a row, in this emulator's Region, with the policy that
    /// lets CloudTrail deliver into it.
    async fn log_bucket(&self, bucket: &str) {
        let mut create = self.s3().create_bucket().bucket(bucket);
        // Outside the leader Region a create must name the Region it is made in.
        if self.region != ::service_common::management_events::PARTITION_LEADER_REGION {
            create = create.create_bucket_configuration(
                ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(
                        self.region.as_str(),
                    ))
                    .build(),
            );
        }
        create.send().await.unwrap_or_else(|error| {
            panic!(
                "case {:?}: creating {bucket} must succeed, got {}",
                self.region,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        });
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(cloudtrail_bucket_policy(bucket))
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {:?}: policying {bucket} must succeed, got {}",
                    self.region,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
    }

    /// Every object key the bucket holds under `prefix`, as Amazon S3 lists them.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "listing {bucket}/{prefix} must succeed, got {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        keys
    }

    /// One delivered object's bytes, as the object store answers with them.
    async fn object(&self, bucket: &str, key: &str) -> Vec<u8> {
        let object = self
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "reading {key} must succeed, got {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
        object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key}'s body must succeed, got {error}"))
            .into_bytes()
            .to_vec()
    }

    /// Every record delivered under `prefix`, each with the whole file's bytes, so an
    /// assertion about what no delivered BYTE may carry reads the object and not a rendering
    /// of it.
    async fn delivered(&self, bucket: &str, prefix: &str) -> Vec<(String, Vec<u8>, Value)> {
        let mut delivered = Vec::new();
        for key in self.keys_under(bucket, prefix).await {
            let bytes = self.object(bucket, &key).await;
            let mut document = String::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_string(&mut document)
                .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
            let parsed: Value = serde_json::from_str(&document)
                .unwrap_or_else(|error| panic!("{key} must gunzip to JSON, got {error}"));
            let records = parsed
                .get(RECORDS_MEMBER)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("{key} must hold {RECORDS_MEMBER}, got {parsed}"));
            for record in records {
                delivered.push((key.clone(), bytes.clone(), record));
            }
        }
        delivered
    }
}

/// The account every bucket, pool, trail and record below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The whole delivery area of a trail that carries `key_prefix`, for the Region `region`:
/// where a consumer of that trail looks for the files of that Region's calls.
fn delivery_prefix(key_prefix: &str, region: &str) -> String {
    format!(
        "{key_prefix}/{LOG_FILE_ROOT_SEGMENT}/{}/{LOG_FILE_SERVICE_SEGMENT}/{region}/",
        account()
    )
}

/// The ARN of the identity pool `pool_id` in `region`, which is how a record names the
/// resource an identity-pool call acted on.
fn pool_arn(region: &str, pool_id: &str) -> String {
    format!(
        "arn:aws:cognito-identity:{region}:{}:identitypool/{pool_id}",
        account()
    )
}

/// The text a record holds at `member`, or `""` for one that holds none.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The names of the events delivered under one prefix, as `<eventName>/<eventCategory>`.
fn named(delivered: &[(String, Vec<u8>, Value)]) -> Vec<String> {
    let mut names: Vec<String> = delivered
        .iter()
        .map(|(_, _, record)| {
            format!(
                "{}/{}",
                text(record, "eventName"),
                text(record, "eventCategory")
            )
        })
        .collect();
    names.sort();
    names
}

/// Create one trail of a row: delivering into `bucket` under `key_prefix`, homed in the
/// emulator's own Region, carrying `selectors` when a row states them, and logging.
///
/// A trail that states no selector keeps the ones a create gives it — "trails created without
/// specific event selectors will be configured to log all read and write management events,
/// and no data events" — which is what makes the management trail of a row a management trail.
async fn logging_trail(
    emulator: &Emulator,
    trail: &str,
    bucket: &str,
    key_prefix: &str,
    selectors: Option<Value>,
) -> String {
    let created = emulator
        .cloudtrail(
            trail,
            "CreateTrail",
            &json!({
                "Name": trail,
                "S3BucketName": bucket,
                "S3KeyPrefix": key_prefix,
                "IsMultiRegionTrail": false,
                "IncludeGlobalServiceEvents": false,
            }),
        )
        .await;
    let trail_arn = created["TrailARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {trail:?}: CreateTrail must answer a TrailARN, got {created}")
        })
        .to_string();
    if let Some(selectors) = selectors {
        let configured = emulator
            .cloudtrail(
                trail,
                "PutEventSelectors",
                &json!({"TrailName": trail, "AdvancedEventSelectors": &selectors}),
            )
            .await;
        assert_eq!(
            configured.get("AdvancedEventSelectors"),
            Some(&selectors),
            "case {trail:?}: expected the selectors echoed back unchanged, got {configured}"
        );
    }
    emulator
        .cloudtrail(trail, "StartLogging", &json!({"Name": trail}))
        .await;
    let status = emulator
        .cloudtrail(trail, "GetTrailStatus", &json!({"Name": trail}))
        .await;
    assert_eq!(
        status.get("IsLogging"),
        Some(&json!(true)),
        "case {trail:?}: expected the trail to report itself logging, got {status}"
    );
    trail_arn
}

/// The advanced event selector that admits the data events of ONE identity pool: the `Data`
/// category, the identity-pool resource type, and that pool's own ARN.
fn one_pools_data_selector(pool_arn: &str) -> Value {
    json!([{
        "Name": "one-identity-pool",
        "FieldSelectors": [
            {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
            {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
            {"Field": "resources.ARN", "Equals": [pool_arn]},
        ],
    }])
}

/// One fixture identity pool, declaring the public provider a federated login is presented
/// under.
async fn fixture_pool(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .cognito(
            name,
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "SupportedLoginProviders": {LOGIN_PROVIDER: "declared-app-id"},
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {name:?}: CreateIdentityPool must answer an id, got {created}")
        })
        .to_string()
}

/// The two calls of the identity flow against one pool, each presenting the federated login:
/// mint the identity, then mint its OpenID token.
async fn identity_flow(emulator: &Emulator, case: &str, pool_id: &str) {
    let minted = emulator
        .cognito(
            case,
            "GetId",
            &json!({
                "IdentityPoolId": pool_id,
                "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
            }),
        )
        .await;
    let identity_id = minted["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetId must answer an IdentityId, got {minted}"))
        .to_string();
    emulator
        .cognito(
            case,
            "GetOpenIdToken",
            &json!({
                "IdentityId": identity_id,
                "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
            }),
        )
        .await;
}

/// What a delivered identity-pool DATA record has to carry, whichever of the flow's calls it
/// is the record of.
fn assert_identity_pool_data_record(case: &str, region: &str, pool_id: &str, record: &Value) {
    for (member, expected) in [
        ("eventSource", COGNITO_EVENT_SOURCE.to_string()),
        ("eventCategory", DATA_CATEGORY.to_string()),
        ("awsRegion", region.to_string()),
    ] {
        let actual = text(record, member);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {member} {expected:?}, got {actual:?} in {record}"
        );
    }
    assert_eq!(
        record.get("managementEvent"),
        Some(&json!(false)),
        "case {case:?}: a data event is no management event, got {record}"
    );
    let resources = record
        .get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        resources,
        vec![json!({
            "accountId": account(),
            "type": IDENTITY_POOL_TYPE,
            "ARN": pool_arn(region, pool_id),
        })],
        "case {case:?}: expected the pool as the event's one resource, got {record}"
    );
    assert_eq!(
        record.pointer("/requestParameters/logins"),
        Some(&json!({LOGIN_PROVIDER: HIDDEN})),
        "case {case:?}: expected the provider keyed to the redaction literal, got {record}"
    );
}

/// RULE: a trail homed in a Region delivers that Region's calls under that Region's own key
/// layout — the management calls of a trail that logs management events, and a data call only
/// while one of a trail's advanced selectors admits that record — whichever Region the whole
/// account is served in.
#[tokio::test]
async fn a_logging_trail_delivers_the_calls_of_the_region_it_is_homed_in() {
    // The rows differ along the Region the account is served in: the partition's leader
    // Region, and two Regions that are not it.
    for region in [
        ::service_common::management_events::PARTITION_LEADER_REGION,
        "us-west-2",
        "eu-central-1",
    ] {
        let emulator = Emulator::in_region(region);
        let bucket = format!("region-delivery-{region}");
        emulator.log_bucket(&bucket).await;

        // The pool the data selector will name, and the pool it will not.
        let selected = fixture_pool(&emulator, "region-delivery-selected").await;
        let withheld = fixture_pool(&emulator, "region-delivery-withheld").await;

        // The two trails of the row, each under its own key prefix: the one that keeps a
        // create's own selectors, and the one whose only selector admits the data events of
        // the SELECTED pool.
        logging_trail(
            &emulator,
            "region-delivery-management",
            &bucket,
            MANAGEMENT_PREFIX,
            None,
        )
        .await;
        let data_trail = logging_trail(
            &emulator,
            "region-delivery-data",
            &bucket,
            DATA_PREFIX,
            Some(one_pools_data_selector(&pool_arn(region, &selected))),
        )
        .await;

        // A management call of the account, and the identity flow of each pool.
        emulator
            .cognito(
                region,
                "TagResource",
                &json!({
                    "ResourceArn": pool_arn(region, &selected),
                    "Tags": {"delivery": "management-marker"},
                }),
            )
            .await;
        identity_flow(&emulator, "the pool no selector names", &withheld).await;
        identity_flow(&emulator, "the pool the selector names", &selected).await;

        // What the MANAGEMENT trail holds: the marker, under the Region's own prefix, and no
        // data event at all.
        let management = emulator
            .delivered(&bucket, &delivery_prefix(MANAGEMENT_PREFIX, region))
            .await;
        let marker: Vec<&(String, Vec<u8>, Value)> = management
            .iter()
            .filter(|(_, _, record)| text(record, "eventName") == "TagResource")
            .collect();
        assert_eq!(
            marker.len(),
            1,
            "case {region:?}: expected exactly one delivered TagResource record, got {:?}",
            named(&management)
        );
        let (_, _, marker_record) = marker[0];
        for (member, expected) in [
            ("eventSource", COGNITO_EVENT_SOURCE.to_string()),
            ("eventCategory", MANAGEMENT_CATEGORY.to_string()),
            ("awsRegion", region.to_string()),
        ] {
            let actual = text(marker_record, member);
            assert_eq!(
                actual, expected,
                "case {region:?}: expected the marker's {member} {expected:?}, got {actual:?} \
                 in {marker_record}"
            );
        }
        assert_eq!(
            marker_record.get("managementEvent"),
            Some(&json!(true)),
            "case {region:?}: a control-plane call is a management event, got {marker_record}"
        );
        let data_under_management: Vec<String> = named(&management)
            .into_iter()
            .filter(|name| name.ends_with(DATA_CATEGORY))
            .collect();
        assert!(
            data_under_management.is_empty(),
            "case {region:?}: a trail with no data selector delivers no data event, got \
             {data_under_management:?}"
        );

        // What the DATA trail holds: one record per call of the SELECTED pool's flow, and
        // nothing of the other pool's identical calls.
        let data = emulator
            .delivered(&bucket, &delivery_prefix(DATA_PREFIX, region))
            .await;
        let mut delivered_names = named(&data);
        delivered_names.dedup();
        assert_eq!(
            delivered_names,
            vec![
                format!("GetId/{DATA_CATEGORY}"),
                format!("GetOpenIdToken/{DATA_CATEGORY}"),
            ],
            "case {region:?}: expected the selected pool's two calls and nothing else, got {:?}",
            named(&data)
        );
        for (key, bytes, record) in &data {
            let case = format!("{region}: the delivered {}", text(record, "eventName"));
            assert!(
                key.starts_with(&delivery_prefix(DATA_PREFIX, region))
                    && key.ends_with(LOG_FILE_SUFFIX),
                "case {case:?}: expected a log-file key under the Region's own prefix, got {key:?}"
            );
            assert_identity_pool_data_record(&case, region, &selected, record);
            assert!(
                !String::from_utf8_lossy(bytes).contains(LOGIN_TOKEN),
                "case {case:?}: the presented token may appear in no delivered byte of {key:?}"
            );
        }

        // Event history: the management marker is reported there and no data event is.
        let looked_up = emulator
            .cloudtrail(region, "LookupEvents", &json!({"MaxResults": 50}))
            .await;
        let history: Vec<String> = looked_up["Events"]
            .as_array()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event["EventName"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            history.iter().any(|name| name == "TagResource"),
            "case {region:?}: event history must report the management call, got {history:?}"
        );
        for withheld_name in ["GetId", "GetOpenIdToken"] {
            assert!(
                !history.iter().any(|name| name == withheld_name),
                "case {region:?}: event history serves no data event, got {withheld_name} in \
                 {history:?}"
            );
        }

        // The trail that delivered reports WHEN it last delivered, at or past the instant of
        // the last record it delivered, and reports no delivery error.
        let status = emulator
            .cloudtrail(region, "GetTrailStatus", &json!({"Name": &data_trail}))
            .await;
        let reported = status["LatestDeliveryTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {region:?}: a trail that delivered must report a LatestDeliveryTime, got \
                 {status}"
            )
        });
        let last_record = data
            .iter()
            .filter_map(|(_, _, record)| {
                ::aws_smithy_types::DateTime::from_str(
                    &text(record, "eventTime"),
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .ok()
                .map(|at| at.secs() as f64)
            })
            .fold(f64::MIN, f64::max);
        assert!(
            reported >= last_record,
            "case {region:?}: expected a LatestDeliveryTime at or past the last record's \
             {last_record}, got {reported}"
        );
        assert_eq!(
            status.get("LatestDeliveryError"),
            None,
            "case {region:?}: a trail that delivered reports no delivery error, got {status}"
        );
    }
}

/// RULE: the record of a call is that call's own — serving many calls at once loses none of
/// them, so a trail delivers exactly one record per call however many are in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_call_served_at_once_leaves_its_own_delivered_record() {
    let region = "us-west-2";
    let emulator = Emulator::in_region(region);
    let bucket = "concurrent-delivery-logs";
    emulator.log_bucket(bucket).await;
    let pool = fixture_pool(&emulator, "concurrent-delivery-pool").await;
    // ONE trail, carrying both an advanced selector for the pool's data events and one for
    // the account's management events, so both classes land under the same prefix.
    logging_trail(
        &emulator,
        "concurrent-delivery",
        bucket,
        DATA_PREFIX,
        Some(json!([
            {
                "Name": "identity-pool-data",
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": [DATA_CATEGORY]},
                    {"Field": "resources.type", "Equals": [IDENTITY_POOL_TYPE]},
                ],
            },
            {
                "Name": "management",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": [MANAGEMENT_CATEGORY]}],
            },
        ])),
    )
    .await;

    let mut tasks = Vec::new();
    for index in 0..CONCURRENT_CALLS {
        let emulator = emulator.clone();
        let pool_arn = pool_arn(region, &pool);
        let pool_id = pool.clone();
        tasks.push(tokio::spawn(async move {
            emulator
                .cognito(
                    "a management call served at once",
                    "TagResource",
                    &json!({
                        "ResourceArn": pool_arn,
                        "Tags": {format!("concurrent{index}"): "marker"},
                    }),
                )
                .await;
            emulator
                .cognito(
                    "a data call served at once",
                    "GetId",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "Logins": {LOGIN_PROVIDER: LOGIN_TOKEN},
                    }),
                )
                .await;
        }));
    }
    for task in tasks {
        task.await.expect("every concurrent call is served");
    }

    let delivered = emulator
        .delivered(bucket, &delivery_prefix(DATA_PREFIX, region))
        .await;
    for (event_name, category) in [
        ("TagResource", MANAGEMENT_CATEGORY),
        ("GetId", DATA_CATEGORY),
    ] {
        let count = delivered
            .iter()
            .filter(|(_, _, record)| text(record, "eventName") == event_name)
            .count();
        assert_eq!(
            count, CONCURRENT_CALLS,
            "case {event_name:?}: expected one delivered {category} record per call, got {count} \
             of {CONCURRENT_CALLS} among {:?}",
            named(&delivered)
        );
    }
    // Every record names the request its own caller was answered under, so no two records of
    // the concurrent calls are records of one call.
    let mut request_ids: Vec<String> = delivered
        .iter()
        .map(|(_, _, record)| text(record, "requestID"))
        .collect();
    request_ids.sort();
    let answered = request_ids.len();
    request_ids.dedup();
    assert_eq!(
        request_ids.len(),
        answered,
        "expected {answered} records of {answered} distinct calls, got {} distinct requestIDs",
        request_ids.len()
    );
}
