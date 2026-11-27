// SPDX-License-Identifier: CC0-1.0

//! The emulator every suite in this test target drives, and the helpers they share.
//!
//! One harness rather than one per suite: a suite that needs the router or a request takes
//! it from here (`use crate::emulator::{..}`) so the way this crate is driven over the wire
//! is described once. The suites that still carry their own copy are welcome to adopt it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a request is signed for when a suite does not care which one it is. A
/// DynamoDB table name is unique within ONE region, which is the scope the store keys by.
pub const REGION: &str = "us-east-1";

/// One emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_and_engine().0
}

/// The same emulator, together with the shared engine it was built on — for a suite that
/// must reach this service's STORED state (what a handler reaches through `self.engine`)
/// and not only its wire surface.
pub fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests, so signature verification is off; the
    // `Authorization` header below is still what names the region, exactly as a signed
    // client's would.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    // KINESIS is mounted beside it, on the same engine, because a table's streaming
    // destination IS a Kinesis data stream: enabling one resolves its `StreamArn` against
    // that service's own registry, so a suite that enables a destination has to be able to
    // create the stream it names ([`create_kinesis_stream`]).
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One emulator holding only this service, with signature VERIFICATION ON — the
/// configuration the emulator serves traffic in (`--auth on`) — together with the access key
/// id it published for this run, which is the one credential a request can have RESOLVE.
///
/// For the suites whose rule is about the SigV4 layer rather than about an operation
/// (`authentication_refusal_wording`, `signature_presentation_refusals`): everything else
/// drives unsigned requests, so it takes [`emulator`] instead.
pub fn verifying_emulator() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    let access_key_id = match router.vended_credentials().into_iter().next() {
        Some(emulator_server::VendedCredentials::AwsAccessKey { access_key_id, .. }) => {
            access_key_id
        }
        other => panic!("with verification on the emulator must vend a key pair, got {other:?}"),
    };
    router.register_service(EmulatedService::new(engine));
    (router, access_key_id)
}

/// The instant a suite signs a request at, in SigV4's own `YYYYMMDDTHHMMSSZ` form, and the
/// scope date it falls on.
///
/// Read from the CLOCK rather than written down, because a request signed at a fixed instant
/// is refused for the difference between that instant and the current time once the clock has
/// moved past the window SigV4 admits — and what the suites that sign one are about is the
/// CREDENTIAL and the PRESENTATION. Both halves come from one reading, so the scope date and
/// the signing instant can never name different days.
pub fn signed_at() -> (String, String) {
    let rendered = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the current instant renders in RFC 3339");
    let (date, time) = rendered
        .split_once('T')
        .expect("an RFC 3339 instant separates its date and time with 'T'");
    let date: String = date.chars().filter(|held| held.is_ascii_digit()).collect();
    let time: String = time
        .chars()
        .filter(|held| held.is_ascii_digit())
        .take(6)
        .collect();
    (format!("{date}T{time}Z"), date)
}

/// The WHOLE `__type` of a refusal document, namespace included — what a client reads off the
/// wire, where [`error_code`] answers the shape name alone.
pub fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Make sure the Kinesis data stream `name` exists in [`REGION`], creating it over the
/// wire if it does not, and answer the ARN THAT SERVICE reports for it — the ARN a table's
/// destination then names.
///
/// A single shard, which is all a destination that only has to RESOLVE the stream needs; a
/// suite whose rule is about the shard MAP names its own count
/// ([`ensure_kinesis_stream_with_shards`]). Idempotent, so a suite may call it from the
/// helper that enables a destination however many times that helper runs: a stream the
/// region already holds answers `ResourceInUseException`, which is this call's success too.
/// The ARN is read back off that service's own description rather than built here, so a
/// suite does not depend on how the Kinesis crate mints one.
pub async fn ensure_kinesis_stream(router: &MultiServiceRouter, name: &str) -> String {
    ensure_kinesis_stream_with_shards(router, name, 1).await
}

/// The same, over `shards` shards — for a suite that asserts WHERE in a stream's shard map
/// a record landed, which a one-shard stream cannot show.
pub async fn ensure_kinesis_stream_with_shards(
    router: &MultiServiceRouter,
    name: &str,
    shards: u32,
) -> String {
    let (status, body) = dispatch_as(
        router,
        REGION,
        "kinesis",
        "Kinesis_20131202.CreateStream",
        json!({"StreamName": name, "ShardCount": shards}),
    )
    .await;
    assert!(
        status == 200 || error_code(&body) == "ResourceInUseException",
        "case ensure the Kinesis stream {name:?}: expected it created or already there, got \
         {status} with {body}"
    );
    let (status, described) = dispatch_as(
        router,
        REGION,
        "kinesis",
        "Kinesis_20131202.DescribeStreamSummary",
        json!({"StreamName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ensure the Kinesis stream {name:?}: expected its description, got {status} with \
         {described}"
    );
    described["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Send one KINESIS operation in [`REGION`], signed for that service — the surface a
/// table's streaming destination delivers its records into, so a suite that asserts what
/// was delivered reads it where Kinesis holds it.
pub async fn call_kinesis(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch_as(
        router,
        REGION,
        "kinesis",
        &format!("Kinesis_20131202.{operation}"),
        body,
    )
    .await
}

/// [`call_kinesis`] for an operation that must succeed, returning its parsed body.
pub async fn ok_kinesis(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call_kinesis(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case kinesis {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One emulator holding this service AND DynamoDB Streams, over ONE shared engine.
///
/// A stream is created by a DynamoDB call and read by a streams call, so a suite
/// about streams needs both services mounted on the same engine — which is also how
/// the router mounts them. They share the `dynamodb` signing name and are told apart
/// by `X-Amz-Target`, which is what [`call_streams`] sets.
pub fn emulator_with_streams() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One emulator holding this service AND Amazon S3, over ONE shared engine.
///
/// The objects a table export writes and the ones a table import reads are Amazon S3's
/// resources: they exist only if that service serves them — so a suite about either mounts
/// S3 beside DynamoDB, exactly as `core/router` assembles them.
pub fn emulator_with_s3() -> MultiServiceRouter {
    emulator_with_s3_and_engine().0
}

/// The same emulator, together with the shared engine it was built on — for a suite about
/// the export or import surface that must also reach this service's STORED state.
pub fn emulator_with_s3_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam an export
/// writes its objects through and an import reads them from, so what a suite writes here
/// is what the import really reads.
pub fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Create one bucket through Amazon S3's own API, which is what gives an import a source
/// and an export a destination.
pub async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    s3(router)
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("S3 creates the bucket {bucket:?}: {error:?}"));
}

/// Store one object through Amazon S3's own API — a source object an import then reads.
pub async fn put_object(router: &MultiServiceRouter, bucket: &str, key: &str, body: Vec<u8>) {
    s3(router)
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
        .unwrap_or_else(|error| panic!("S3 stores {key:?} in {bucket:?}: {error:?}"));
}

/// One emulator holding this service AND KMS, over ONE shared engine.
///
/// A table's encryption key is a KMS resource: the key ARN a table reports, and the key
/// STATE its data plane is held to, are that service's answers — so a suite about either
/// mounts KMS beside DynamoDB, exactly as `core/router` assembles them.
pub fn emulator_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// One emulator holding this service AND CloudWatch, over ONE shared engine.
///
/// Contributor Insights for DynamoDB IS a set of CloudWatch Contributor Insights rules: the
/// rules an enable mints live in that service's store, so a suite that asserts them mounts
/// CloudWatch beside DynamoDB, exactly as `core/router` assembles them.
pub fn emulator_with_cloudwatch() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one CLOUDWATCH operation in [`REGION`], signed for that service (`monitoring`) and
/// named under its own `X-Amz-Target` prefix.
pub async fn call_cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch_as(
        router,
        REGION,
        "monitoring",
        &format!("GraniteServiceVersion20100801.{operation}"),
        body,
    )
    .await
}

/// [`call_cloudwatch`] for an operation that must succeed, returning its parsed body.
pub async fn ok_cloudwatch(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call_cloudwatch(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case cloudwatch {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Send one KMS operation in `region`, signed for that service. KMS names its operations
/// under its own `X-Amz-Target` prefix, which is what tells it apart here.
pub async fn call_kms_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch_as(
        router,
        region,
        "kms",
        &format!("TrentService.{operation}"),
        body,
    )
    .await
}

/// [`call_kms_in`] for a KMS operation that must succeed in `region`, returning its parsed
/// body — for a suite whose keys live in more than one Region, since a KMS key is a
/// REGIONAL resource.
pub async fn ok_kms_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_kms_in(router, region, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case kms {operation} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// [`ok_kms_in`] in [`REGION`], for a suite that is not about Regions.
pub async fn ok_kms(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    ok_kms_in(router, REGION, operation, body).await
}

/// Send one operation in `region` and return its HTTP status and parsed body.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        region,
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await
}

/// Send one DynamoDB STREAMS operation in [`REGION`]. Both services sign as
/// `dynamodb`, so the `X-Amz-Target` prefix is the whole discriminator.
pub async fn call_streams(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        REGION,
        &format!("DynamoDBStreams_20120810.{operation}"),
        body,
    )
    .await
}

/// Send one request to `target` (the whole `X-Amz-Target` value) in `region`.
async fn dispatch(
    router: &MultiServiceRouter,
    region: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    dispatch_as(router, region, "dynamodb", target, body).await
}

/// The same, signed for the service `signing_name` — which is what the router claims a
/// request by, so a call to another service mounted on the same engine names its own.
pub async fn dispatch_as(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/{signing_name}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`call_in`] in [`REGION`], for a suite that is not about regions.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// [`call_in`] for an operation that must succeed in `region`, returning its parsed body
/// — for a suite whose calls are signed for more than one region.
pub async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_in(router, region, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// [`ok_in`] in [`REGION`], for a suite that is not about regions.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    ok_in(router, REGION, operation, body).await
}

/// How long a poll for the documented `CREATING` -> `ACTIVE` transition may take before
/// a suite calls it a hang. Generous slack over the emulator's own transition span: a
/// loaded machine is not a defect, and the point of the bound is that the wait ENDS,
/// which is what a client's `table_exists` waiter needs.
pub const ACTIVE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long [`describe_when_active`] sleeps between `DescribeTable` calls.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Poll `DescribeTable` until the table reports `ACTIVE`, answering the `Table`
/// description it reported then — the waiter loop a client runs after a `CreateTable`,
/// shared by every suite that needs a table ready before it drives one.
///
/// Every poll must answer 200: "You can use the DescribeTable action to check the table
/// status", so a table an accepted create registered is never missing. The wait is
/// bounded, and a table that never settles panics naming the last description it gave
/// rather than hanging. `case` names the row the assertions belong to.
pub async fn describe_when_active(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    table_when(router, case, REGION, name, |table| {
        table["TableStatus"] == json!("ACTIVE")
    })
    .await
}

/// The same waiter over any Region and any condition: poll `DescribeTable` in `region`
/// until the `Table` description satisfies `settled`, and answer that description.
///
/// The shared waiter for whatever the CLOCK owes a table or its replicas — a table reaching
/// `ACTIVE`, a replica settling, a per-replica setting coming into force — so a suite about
/// a global table waits for the CONDITION in the Region that answers it rather than
/// spelling the loop again ([`describe_when_active`] is this loop over [`REGION`] and the
/// table's own status). The wait is bounded by [`ACTIVE_DEADLINE`], and a condition that
/// never holds panics naming the last description rather than hanging.
pub async fn table_when(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < ACTIVE_DEADLINE {
        let (status, body) = call_in(router, region, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTable in {region} must resolve a table the create \
             accepted, got {status} with {body}"
        );
        if settled(&body["Table"]) {
            return body["Table"].clone();
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {name} in {region} never settled within {ACTIVE_DEADLINE:?}, last {last}"
    )
}

/// The attribute names the `SearchVectors` result at `at` carries, sorted — the reading every
/// vector suite does of `SearchResultItem$Item`, shared so two suites cannot disagree about
/// what "the attributes a result carries" means.
///
/// An absent result and one carrying no `Item` both read as no attributes, which is what a
/// suite asserting an empty projection expects.
pub fn search_result_attributes(answered: &Value, at: usize) -> Vec<String> {
    let mut names: Vec<String> = answered["SearchResults"][at]["Item"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// Poll `DescribeContributorInsights` until the resource's insights report `status`,
/// answering the description it reported then — the waiter a client runs after an
/// `UpdateContributorInsights`, which is asynchronous in either direction ("the status only
/// becomes the settled one once the rules behind it are the ones the action asked for").
///
/// `index` names the global secondary index the setting belongs to, or `None` for the
/// table's own. Every poll must answer 200: the resource was addressed by a call the
/// service accepted. The wait is bounded by [`ACTIVE_DEADLINE`], and a setting that never
/// reaches `status` panics naming the last description rather than hanging. `case` names
/// the row the assertions belong to.
pub async fn insights_when(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    index: Option<&str>,
    status: &str,
) -> Value {
    let mut request = json!({"TableName": table});
    if let Some(index) = index {
        request["IndexName"] = json!(index);
    }
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < ACTIVE_DEADLINE {
        let (code, body) = call(router, "DescribeContributorInsights", request.clone()).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeContributorInsights must answer for a resource an update \
             named, got {code} with {body}"
        );
        if body["ContributorInsightsStatus"] == json!(status) {
            return body;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the contributor insights of {table} never reached {status} within \
         {ACTIVE_DEADLINE:?}, last {last}"
    )
}

/// Every record a table's stream holds, read from its trim horizon through the STREAMS
/// surface — the loop a change-data-capture consumer runs, shared by every suite that
/// reads a change feed. `case` names the row the assertions below belong to.
///
/// The walk is bounded, so a handler that never advances its iterator fails rather than
/// hanging, and it asserts the stream serves a shard and a position in it: a drain that
/// silently answered nothing would surface only as a record mismatch far from its cause.
pub async fn drain_stream(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
) -> Vec<Value> {
    let (status, described) =
        call_streams(router, "DescribeStream", json!({"StreamArn": stream_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream must resolve the table's stream, got {status} with \
         {described}"
    );
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !shards.is_empty(),
        "case {case:?}: DescribeStream must list at least one shard, got {described}"
    );
    let mut records: Vec<Value> = Vec::new();
    for shard in &shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default();
        let (status, minted) = call_streams(
            router,
            "GetShardIterator",
            json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetShardIterator answered {status} with {minted}"
        );
        let mut iterator = minted["ShardIterator"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !iterator.is_empty(),
            "case {case:?}: GetShardIterator must mint a position in shard {shard_id:?}, got \
             {minted}"
        );
        for _ in 0..16 {
            let (status, page) =
                call_streams(router, "GetRecords", json!({"ShardIterator": iterator})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetRecords answered {status} with {page}"
            );
            let batch = page["Records"].as_array().cloned().unwrap_or_default();
            records.extend(batch.iter().cloned());
            match page["NextShardIterator"].as_str() {
                Some(next) if !batch.is_empty() => iterator = next.to_string(),
                _ => break,
            }
        }
    }
    records
}

/// Every change record the KINESIS data stream `stream` holds, as the shard it landed in
/// and the JSON document its `Data` blob carries — what a consumer of a table's Kinesis
/// streaming destination reads, shared by every suite that asserts a delivery.
///
/// Read from each shard's trim horizon through that service's own surface, because that is
/// where the records live. The walk is bounded, so a delivery that never advances an
/// iterator fails rather than hanging, and it asserts the stream serves a shard and a
/// position in it. `case` names the row the assertions belong to.
pub async fn drain_kinesis_stream(
    router: &MultiServiceRouter,
    case: &str,
    stream: &str,
) -> Vec<(String, Value)> {
    let listed = ok_kinesis(router, "ListShards", json!({"StreamName": stream})).await;
    let shards = listed["Shards"].as_array().cloned().unwrap_or_default();
    assert!(
        !shards.is_empty(),
        "case {case:?}: the destination's stream must list at least one shard, got {listed}"
    );
    let mut records: Vec<(String, Value)> = Vec::new();
    for shard in &shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default().to_string();
        let minted = ok_kinesis(
            router,
            "GetShardIterator",
            json!({
                "StreamName": stream,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        let mut iterator = minted["ShardIterator"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !iterator.is_empty(),
            "case {case:?}: GetShardIterator must mint a position in {shard_id:?}, got {minted}"
        );
        for _ in 0..16 {
            let page = ok_kinesis(router, "GetRecords", json!({"ShardIterator": iterator})).await;
            let batch = page["Records"].as_array().cloned().unwrap_or_default();
            for record in &batch {
                let encoded = record["Data"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: every record carries Data, got {record}")
                });
                let decoded = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: a record's Data is base64, got {error}")
                });
                let document: Value = serde_json::from_slice(&decoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: a delivered record carries JSON, got {error}")
                });
                records.push((shard_id.clone(), document));
            }
            match page["NextShardIterator"].as_str() {
                Some(next) if !batch.is_empty() => iterator = next.to_string(),
                _ => break,
            }
        }
    }
    records
}

/// The `eventName` of every stream record, in order.
pub fn event_names(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| record["eventName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The records of ONE item's own key, in the order the stream holds them: one drain carries
/// every key a suite wrote, and a row of a table-driven suite is about one key.
///
/// `key` is the table's partition-key attribute and `id` the string value this item is held
/// under, which is how a record names the item it describes ("The primary key attribute(s)
/// for the DynamoDB item that was modified", `StreamRecord$Keys`).
pub fn records_of(records: &[Value], key: &str, id: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record["dynamodb"]["Keys"][key]["S"] == json!(id))
        .cloned()
        .collect()
}

/// Send one operation that must answer 200, answering its body, with `case` naming the row
/// the call belongs to — [`ok`] for a table-driven suite, whose every panic has to say which
/// row raised it.
pub async fn ok_case(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Create `table` keyed on the string attribute `key` with a stream capturing BOTH images,
/// wait until it serves writes, and answer the ARN of the stream it registered — the setup
/// every suite that reads a table's own change feed starts from.
///
/// On demand, so nothing here is about provisioned throughput, and the wait is the harness's
/// own bounded poll for the documented `CREATING` -> `ACTIVE` transition
/// ([`describe_when_active`]) rather than a span of the caller's choosing.
pub async fn streamed_table(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
) -> String {
    let created = ok_case(
        router,
        case,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": key, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": key, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": {
                "StreamEnabled": true,
                "StreamViewType": "NEW_AND_OLD_IMAGES",
            },
        }),
    )
    .await;
    let stream_arn = created["TableDescription"]["LatestStreamArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !stream_arn.is_empty(),
        "case {case:?}: an enabled stream must be reported, got {created}"
    );
    describe_when_active(router, case, table).await;
    stream_arn
}

/// The item `GetItem` reports for the key `id` of `table` — that table's own account of what
/// it stores, which is what a write that modified nothing must have left alone.
///
/// Read consistently, so what it answers is what the write that just landed left, and with
/// every set's members in one order ([`with_sorted_sets`]), because a set is unordered
/// ("Amazon DynamoDB does not preserve the order of the members of a set") and two readings
/// of one set are therefore compared as sets.
pub async fn stored_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
    id: &str,
) -> Value {
    let read = ok_case(
        router,
        case,
        "GetItem",
        json!({
            "TableName": table,
            "Key": {key: {"S": id}},
            "ConsistentRead": true,
        }),
    )
    .await;
    with_sorted_sets(&read["Item"])
}

/// The same value with every SET's members sorted.
///
/// A DynamoDB set is unordered — "Amazon DynamoDB does not preserve the order of the members
/// of a set" — so a suite that compares a stored set to an expected one compares the MEMBERS,
/// not the order the wire happened to list them in.
pub fn with_sorted_sets(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(name, inner)| {
                    let sorted = if matches!(name.as_str(), "SS" | "NS" | "BS") {
                        let mut members: Vec<Value> = inner.as_array().cloned().unwrap_or_default();
                        members.sort_by_key(ToString::to_string);
                        Value::Array(members)
                    } else {
                        with_sorted_sets(inner)
                    };
                    (name.clone(), sorted)
                })
                .collect(),
        ),
        Value::Array(members) => Value::Array(members.iter().map(with_sorted_sets).collect()),
        other => other.clone(),
    }
}

/// The `GlobalSecondaryIndexes` entry of a description by index name, or `Null` when
/// that description carries no index by that name.
pub fn index_named(description: &Value, name: &str) -> Value {
    description["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|index| index["IndexName"] == serde_json::json!(name))
        .unwrap_or(Value::Null)
}

/// The entry a `TableDescription` carries under the group-member list `member`
/// (`Replicas`, `GlobalTableWitnesses`) for `region`, or `Null` when it carries none.
///
/// One reader for both lists a replication group's description reports its members through,
/// so a suite about a group reads a member the same way whichever kind of member it is.
pub fn group_member_in(description: &Value, member: &str, region: &str) -> Value {
    description[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry["RegionName"] == serde_json::json!(region))
        .unwrap_or(Value::Null)
}

/// The restorable window `name` reports right now: `(earliest, latest)` as the
/// continuous-backups reader states them, which is the window a point in time named for
/// that table is held to.
///
/// Shared by every suite that names an instant inside or outside it, so they all read the
/// bounds from the one reader that publishes them.
pub async fn restorable_window(router: &MultiServiceRouter, name: &str) -> (f64, f64) {
    let described = ok(
        router,
        "DescribeContinuousBackups",
        json!({"TableName": name}),
    )
    .await;
    let recovery = &described["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"];
    (
        recovery["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default(),
        recovery["LatestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default(),
    )
}

/// The ARN the export in an `ExportDescription`-bearing response is recorded under.
pub fn export_arn(body: &Value) -> String {
    body["ExportDescription"]["ExportArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `message` a refusal carries, under either of the casings a client reads it by.
pub fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The wire error code of a refusal: the shape name the `__type` field carries, which
/// is what an SDK client matches an error on.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The clause one member with NO VALUE earns, as the framework words it: the path the
/// validating walk reports the member under, and the requirement that it not be null.
///
/// Here rather than in one suite because several of them read that clause back — a member
/// stated `null` and the same member left out are the same absence, so the wording is one.
pub fn absent_clause(path: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
}

/// The whole message a constraint refusal reporting `clauses` carries: the envelope counts
/// them, and they are joined in the order the refusal states them.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [clause] => format!("1 validation error detected: {clause}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}
