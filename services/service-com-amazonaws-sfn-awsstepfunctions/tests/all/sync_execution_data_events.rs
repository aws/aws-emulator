// SPDX-License-Identifier: CC0-1.0

//! The DATA leg of this front door: the synchronous run of an Express state machine, which
//! CloudTrail records as a DATA event of the machine it ran.
//!
//! RULE: a call on the CONTENTS of a resource is a DATA event of that resource — recorded in
//! no event history (a lookup serves management and Insights events), and delivered as a
//! gzipped log file into the bucket of each trail of the account whose advanced event
//! selectors admit it, under that trail's own key prefix and the documented key layout. Such
//! a record names the serving service as its `eventSource`, the operation as its `eventName`,
//! `eventCategory` `Data` with `managementEvent` false, the operation's own read/write
//! semantics as `readOnly`, the resource it acted on by TYPE and ARN — which is what a
//! selector selects it by — the id the CALLER was answered under as `requestID`, the
//! redaction literal in place of every value the model declares `@sensitive`, and no
//! `responseElements` at all, because what such a call answers IS the resource's contents. A
//! selector is not retroactive: a call made before it existed is delivered nowhere.
//!
//! The administration of the same machines is unchanged: it stays a management event of the
//! account's event history, which the neighbouring `management_event_recording` suite pins.
//!
//! Two services are mounted on ONE engine, because the behaviour spans them: Step Functions
//! serves the calls, and the object store is where the log files land. Every Step Functions
//! call goes over the wire through the same router the production emulator uses, and the
//! bucket is created, listed and read through Amazon S3's own API over the same in-process
//! callback a handler delivers through — so a delivered log file counts only if the object
//! store itself answers with it. The account's trails are projected onto the shared log the
//! way CloudTrail projects them (`service_common::management_events::DeliveringTrail`), which
//! is the one account of what is logging that the delivery reads; the end-to-end leg through
//! CloudTrail's own `PutEventSelectors` is
//! `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/tests/all/trail_log_file_delivery_of_data_events.rs`.

use std::io::Read;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::service::data_events::STATE_MACHINE_RESOURCE_TYPE;
use service_com_amazonaws_sfn_awsstepfunctions::service::management_events::{
    management_event_log, EVENT_SOURCE,
};
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;
use service_common::data_events::{
    DataEventFieldSelector, DataEventSelector, EVENT_CATEGORY_FIELD, READ_ONLY_FIELD,
    RESOURCES_ARN_FIELD, RESOURCES_TYPE_FIELD,
};
use service_common::management_events::{
    DeliveringTrail, ManagementEvent, ManagementEventClass, HIDDEN_SENSITIVE_VALUE,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The `X-Amz-Target` prefix of this service, and the signing name every call is signed for.
const TARGET_PREFIX: &str = "AWSStepFunctions";
const SIGNING_NAME: &str = "states";

/// The access key id every call below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The Region every call is signed for, which is the Region the trails below are homed in and
/// so the Region their log files land under.
const REGION: &str = "us-east-1";

/// The account every ARN of this emulator carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A role ARN inside the modelled `Arn` length (1..256).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// A definition that succeeds immediately with a static result.
const HELLO_DEFINITION: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// The execution input every synchronous run below carries: a value that appears nowhere
/// else, so finding it inside a delivered record would be unambiguous. The model declares
/// `StartSyncExecutionInput$input` `@sensitive`, so no record may carry it.
const SECRET_INPUT: &str = r#"{"card":"n0t-in-the-trail-input"}"#;

/// The `eventCategory` of a data event, and the one of a control-plane call.
const DATA_CATEGORY: &str = "Data";
const MANAGEMENT_CATEGORY: &str = "Management";

/// Another data-event resource type, for the selector that admits a class this account never
/// serves.
const S3_OBJECT_TYPE: &str = "AWS::S3::Object";

/// The header a response names the request it answers in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The segments of a log file's key, and what its name ends with.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The member a log file's document holds its records in.
const RECORDS_MEMBER: &str = "Records";

/// One answer as its caller reads it: the status, the parsed body, and the id the response
/// named the request by.
struct Answered {
    status: u16,
    body: Value,
    request_id: String,
}

/// Step Functions and the object store over one shared engine, with the in-process callback
/// the delivery writes through.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            callback,
            engine,
        }
    }

    /// One Step Functions call, and the three things its caller observes.
    async fn call(&self, operation: &str, body: &Value) -> Answered {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{SIGNING_NAME}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answered {
            status,
            body,
            request_id,
        }
    }

    /// One Step Functions call that has to be served.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Answered {
        let answered = self.call(operation, body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: {operation} must answer 200, got {} with {}",
            answered.status, answered.body
        );
        answered
    }

    /// Create an EXPRESS state machine, which is the type a synchronous start serves, and
    /// answer its ARN.
    async fn express_machine(&self, name: &str) -> String {
        let created = self
            .ok(
                "setup",
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "type": "EXPRESS",
                }),
            )
            .await;
        created.body["stateMachineArn"]
            .as_str()
            .expect("CreateStateMachine answers a stateMachineArn")
            .to_string()
    }

    /// The Amazon S3 client, over the same in-process callback the delivery uses — built by
    /// the same shared constructor, so the test's transport is the delivery's by
    /// construction.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create the bucket the trails below deliver into.
    async fn log_bucket(&self, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
    }

    /// Replace the account's logging trails with `trails`, the way CloudTrail recomputes the
    /// projection whenever a trail of the account is created, reconfigured, started or
    /// stopped.
    fn trails_delivering(&self, trails: Vec<DeliveringTrail>) {
        let log = management_event_log(&*self.engine)
            .expect("this engine carries the shared management-event log");
        let mut projection = log.lock();
        projection.trails_delivering(trails);
    }

    /// Every record in the shared management-event log, oldest first, as the CloudTrail event
    /// document a lookup reports.
    fn recorded_events(&self) -> Vec<Value> {
        let log = management_event_log(&*self.engine)
            .expect("this engine carries the shared management-event log");
        let held = log.lock();
        held.iter()
            .map(ManagementEvent::to_cloudtrail_event_json)
            .collect()
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
                panic!("listing {bucket}/{prefix} must succeed, got {error:?}")
            });
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        keys
    }

    /// The records one delivered log file holds, read through `GetObject` and gunzipped.
    async fn records(&self, bucket: &str, key: &str) -> Vec<Value> {
        let object = self
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("reading {key} must succeed, got {error:?}"));
        let body = object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key}'s body must succeed, got {error:?}"))
            .into_bytes();
        let mut document = String::new();
        flate2::read::GzDecoder::new(body.as_ref())
            .read_to_string(&mut document)
            .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
        let parsed: Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("{key} must gunzip to JSON, got {error}"));
        parsed
            .get(RECORDS_MEMBER)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("{key} must hold {RECORDS_MEMBER}, got {parsed}"))
    }

    /// Every record delivered under `prefix`, with the key each one arrived in.
    async fn delivered(&self, bucket: &str, prefix: &str) -> Vec<(String, Value)> {
        let mut delivered = Vec::new();
        for key in self.keys_under(bucket, prefix).await {
            for record in self.records(bucket, &key).await {
                delivered.push((key.clone(), record));
            }
        }
        delivered
    }
}

/// One trail of the account, delivering into `bucket` under `key_prefix` with `selectors` for
/// its data events and management events logged only where `management` says so.
fn trail(
    name: &str,
    bucket: &str,
    key_prefix: &str,
    management: bool,
    selectors: Vec<DataEventSelector>,
) -> DeliveringTrail {
    DeliveringTrail {
        trail_arn: format!("arn:aws:cloudtrail:{REGION}:{ACCOUNT}:trail/{name}"),
        home_region: REGION.to_string(),
        s3_bucket_name: bucket.to_string(),
        s3_key_prefix: Some(key_prefix.to_string()),
        multi_region: false,
        global_service_events: false,
        management_events: management,
        event_class: ManagementEventClass::All,
        data_event_selectors: selectors,
    }
}

/// One field selector stating `Equals` over `values`, which is the only operator the
/// category, read/write and resource-type fields take.
fn equals(field: &str, values: &[&str]) -> DataEventFieldSelector {
    DataEventFieldSelector {
        field: field.to_string(),
        equals: values.iter().map(ToString::to_string).collect(),
        ..DataEventFieldSelector::default()
    }
}

/// One advanced event selector, as the fields it states.
fn selector(fields: Vec<DataEventFieldSelector>) -> DataEventSelector {
    DataEventSelector {
        field_selectors: fields,
    }
}

/// The selector a client writes for the data events of this service's state machines: the
/// `Data` category and the state-machine resource type.
fn state_machine_data_selector() -> DataEventSelector {
    selector(vec![
        equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
        equals(RESOURCES_TYPE_FIELD, &[STATE_MACHINE_RESOURCE_TYPE]),
    ])
}

/// The trail's whole delivery area under the key prefix it was created with, which is where a
/// consumer of the trail looks for its files.
fn delivery_prefix(key_prefix: &str) -> String {
    format!("{key_prefix}/{LOG_FILE_ROOT_SEGMENT}/{ACCOUNT}/{LOG_FILE_SERVICE_SEGMENT}/")
}

/// The text a record holds at `member`, or `""` for one that holds none.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert `key` is laid out the way a CloudTrail log file of this trail is: the trail's own
/// delivery area, then the Region, the day, and a file named for the account, the service and
/// the Region.
fn assert_log_file_key(case: &str, key: &str, key_prefix: &str) {
    let prefix = format!("{}{REGION}/", delivery_prefix(key_prefix));
    assert!(
        key.starts_with(&prefix),
        "case {case:?}: expected a log file under {prefix:?}, got {key:?}"
    );
    let under_region = &key[prefix.len()..];
    let segments: Vec<&str> = under_region.split('/').collect();
    assert_eq!(
        segments.len(),
        4,
        "case {case:?}: expected <year>/<month>/<day>/<file> under {prefix:?}, got {key:?}"
    );
    for (index, width) in [(0_usize, 4_usize), (1, 2), (2, 2)] {
        let segment = segments[index];
        assert!(
            segment.len() == width && segment.chars().all(char::is_numeric),
            "case {case:?}: expected a {width}-digit date segment, got {segment:?} in {key:?}"
        );
    }
    let file = segments[3];
    let expected_start = format!("{ACCOUNT}_{LOG_FILE_SERVICE_SEGMENT}_{REGION}_");
    assert!(
        file.starts_with(&expected_start) && file.ends_with(LOG_FILE_SUFFIX),
        "case {case:?}: expected a file named {expected_start}<minute>_<unique>\
         {LOG_FILE_SUFFIX}, got {file:?}"
    );
}

/// One row of the delivery table: the trail the account carries, the machine the call runs,
/// and whether that trail delivers the data event of it.
struct Delivery {
    case: &'static str,
    /// Whether the trail logs management events at all.
    management: bool,
    /// The trail's data-event selectors.
    selectors: Vec<DataEventSelector>,
    /// Whether the call is made on the SUBJECT machine; the other machine is the bystander
    /// an ARN-scoped selector must not admit.
    subject: bool,
    /// How many records the trail must have delivered for the call.
    delivered: usize,
}

/// RULE: a synchronous run is delivered as a DATA event of the state machine it ran, into the
/// bucket of each trail whose selectors admit it and of no other.
///
/// The rows differ along what the rule generalises over: which fields the trail's selector
/// pins (the category alone, the resource type, the resource ARN, the event name, the
/// read/write kind), the values it pins them to, whether the trail logs management events,
/// and whether the call was made on the machine an ARN-scoped selector names or on a sibling.
#[tokio::test]
async fn a_sync_execution_is_delivered_only_to_a_trail_whose_selector_admits_it() {
    let emulator = Emulator::new();
    let bucket = "sfn-data-event-logs";
    emulator.log_bucket(bucket).await;
    let subject = emulator.express_machine("data-event-subject").await;
    let bystander = emulator.express_machine("data-event-bystander").await;

    let rows: Vec<Delivery> = vec![
        Delivery {
            case: "the published state-machine data selector",
            management: true,
            selectors: vec![state_machine_data_selector()],
            subject: true,
            delivered: 1,
        },
        Delivery {
            case: "a selector scoped to the machine the call ran",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals(RESOURCES_TYPE_FIELD, &[STATE_MACHINE_RESOURCE_TYPE]),
                equals(RESOURCES_ARN_FIELD, &[&subject]),
            ])],
            subject: true,
            delivered: 1,
        },
        Delivery {
            case: "a selector scoped to a sibling machine, called on the sibling",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals(RESOURCES_ARN_FIELD, &[&bystander]),
            ])],
            subject: false,
            delivered: 1,
        },
        Delivery {
            case: "a selector scoped to a sibling machine, called on the subject",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals(RESOURCES_ARN_FIELD, &[&bystander]),
            ])],
            subject: true,
            delivered: 0,
        },
        Delivery {
            case: "a trail logging management events and no data event at all",
            management: true,
            selectors: Vec::new(),
            subject: true,
            delivered: 0,
        },
        Delivery {
            case: "a data selector over another resource type",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals(RESOURCES_TYPE_FIELD, &[S3_OBJECT_TYPE]),
            ])],
            subject: true,
            delivered: 0,
        },
        Delivery {
            case: "a data selector narrowed to reads, which a run is not",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals(READ_ONLY_FIELD, &["true"]),
            ])],
            subject: true,
            delivered: 0,
        },
        Delivery {
            case: "a data selector naming the operation",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals("eventName", &["StartSyncExecution"]),
            ])],
            subject: true,
            delivered: 1,
        },
        Delivery {
            case: "a data selector naming another operation",
            management: false,
            selectors: vec![selector(vec![
                equals(EVENT_CATEGORY_FIELD, &[DATA_CATEGORY]),
                equals("eventName", &["StartExecution"]),
            ])],
            subject: true,
            delivered: 0,
        },
        Delivery {
            case: "the management category, which a data event is not",
            management: true,
            selectors: vec![selector(vec![equals(
                EVENT_CATEGORY_FIELD,
                &[MANAGEMENT_CATEGORY],
            )])],
            subject: true,
            delivered: 0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One trail per row, each with a key prefix of its own, so what one row delivered is
        // read apart from every other row's.
        let key_prefix = format!("row-{index}");
        emulator.trails_delivering(vec![trail(
            &format!("row-{index}-trail"),
            bucket,
            &key_prefix,
            row.management,
            row.selectors.clone(),
        )]);
        let machine = if row.subject { &subject } else { &bystander };
        let answered = emulator
            .ok(
                row.case,
                "StartSyncExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": format!("run-{index}"),
                    "input": SECRET_INPUT,
                }),
            )
            .await;

        let delivered = emulator.delivered(bucket, &delivery_prefix(&key_prefix)).await;
        assert_eq!(
            delivered.len(),
            row.delivered,
            "case {:?}: expected {} delivered record(s), got {delivered:?}",
            row.case,
            row.delivered
        );
        for (key, record) in &delivered {
            assert_log_file_key(row.case, key, &key_prefix);
            for (member, expected) in [
                ("eventName", "StartSyncExecution".to_string()),
                ("eventSource", EVENT_SOURCE.to_string()),
                ("eventCategory", DATA_CATEGORY.to_string()),
                ("awsRegion", REGION.to_string()),
                ("recipientAccountId", ACCOUNT.to_string()),
                ("requestID", answered.request_id.clone()),
            ] {
                let actual = text(record, member);
                assert_eq!(
                    actual, expected,
                    "case {:?}: expected {member} {expected:?}, got {actual:?} in {record}",
                    row.case
                );
            }
            assert_eq!(
                record.get("managementEvent"),
                Some(&json!(false)),
                "case {:?}: a data event is no management event, got {record}",
                row.case
            );
            assert_eq!(
                record.get("readOnly"),
                Some(&json!(false)),
                "case {:?}: a run changes the machine's state, got {record}",
                row.case
            );
            assert_eq!(
                record.get("responseElements"),
                Some(&Value::Null),
                "case {:?}: a data event reports no responseElements, got {record}",
                row.case
            );
            assert_eq!(
                record.get("resources"),
                Some(&json!([{
                    "accountId": ACCOUNT,
                    "type": STATE_MACHINE_RESOURCE_TYPE,
                    "ARN": machine,
                }])),
                "case {:?}: expected the machine named as the call's resource, got {record}",
                row.case
            );
            assert_eq!(
                record.pointer("/requestParameters/input"),
                Some(&json!(HIDDEN_SENSITIVE_VALUE)),
                "case {:?}: the run's own input is withheld, got {record}",
                row.case
            );
            assert_eq!(
                record.pointer("/requestParameters/stateMachineArn"),
                Some(&json!(machine)),
                "case {:?}: expected the machine the request addressed, got {record}",
                row.case
            );
            assert!(
                !record.to_string().contains("n0t-in-the-trail-input"),
                "case {:?}: no record may carry the run's own data, got {record}",
                row.case
            );
        }
    }
}

/// RULE: a selector is not retroactive — a call made before the trail carried one is
/// delivered nowhere, and the next call of the same kind is delivered.
#[tokio::test]
async fn a_selector_delivers_nothing_for_a_call_made_before_it_existed() {
    let emulator = Emulator::new();
    let bucket = "sfn-retroactivity-logs";
    let key_prefix = "retroactivity";
    emulator.log_bucket(bucket).await;
    let machine = emulator.express_machine("retroactivity-subject").await;
    let area = delivery_prefix(key_prefix);

    // Before: a trail that logs management events and no data event at all.
    emulator.trails_delivering(vec![trail(
        "before-trail",
        bucket,
        key_prefix,
        true,
        Vec::new(),
    )]);
    let before = emulator
        .ok(
            "before the selector",
            "StartSyncExecution",
            &json!({"stateMachineArn": &machine, "name": "before", "input": SECRET_INPUT}),
        )
        .await;
    let delivered = emulator.delivered(bucket, &area).await;
    assert!(
        delivered.is_empty(),
        "case \"before the selector\": expected nothing delivered, got {delivered:?}"
    );

    // After: the same trail, now carrying the state-machine data selector.
    emulator.trails_delivering(vec![trail(
        "before-trail",
        bucket,
        key_prefix,
        true,
        vec![state_machine_data_selector()],
    )]);
    let after = emulator
        .ok(
            "after the selector",
            "StartSyncExecution",
            &json!({"stateMachineArn": &machine, "name": "after", "input": SECRET_INPUT}),
        )
        .await;
    let delivered = emulator.delivered(bucket, &area).await;
    let answered: Vec<String> = delivered
        .iter()
        .map(|(_, record)| text(record, "requestID"))
        .collect();
    assert_eq!(
        answered,
        vec![after.request_id.clone()],
        "case \"after the selector\": expected only the later call delivered (the earlier one \
         was answered under {:?}), got {delivered:?}",
        before.request_id
    );
}

/// RULE: a REFUSED data-plane call is delivered too, with the error code and the message its
/// caller was answered with — so a reader can tell a refused call on a resource's contents
/// from one that never happened.
///
/// The rows differ along the refusal: a machine that was never created, and a machine of the
/// type this call does not serve.
#[tokio::test]
async fn a_refused_sync_execution_is_delivered_with_the_fault_its_caller_saw() {
    let emulator = Emulator::new();
    let bucket = "sfn-refused-data-event-logs";
    emulator.log_bucket(bucket).await;
    let standard = emulator
        .ok(
            "setup",
            "CreateStateMachine",
            &json!({
                "name": "refused-standard",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await
        .body["stateMachineArn"]
        .as_str()
        .expect("CreateStateMachine answers a stateMachineArn")
        .to_string();
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:refused-absent");

    // (case, the machine the call addresses)
    let rows = [
        ("a machine that was never created", absent),
        ("a machine of the type this call does not serve", standard),
    ];
    for (index, (case, machine)) in rows.iter().enumerate() {
        let key_prefix = format!("refused-{index}");
        emulator.trails_delivering(vec![trail(
            &format!("refused-{index}-trail"),
            bucket,
            &key_prefix,
            false,
            vec![state_machine_data_selector()],
        )]);
        let answered = emulator
            .call(
                "StartSyncExecution",
                &json!({"stateMachineArn": machine, "input": SECRET_INPUT}),
            )
            .await;
        assert_ne!(
            answered.status, 200,
            "case {case:?}: the call must be refused, got {}",
            answered.body
        );
        let shape = answered.body["__type"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an error body, got {}",
                    answered.body
                )
            })
            .rsplit(['#', ':'])
            .next()
            .unwrap_or_default()
            .to_string();
        let message = answered.body["message"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a refusal message, got {}",
                    answered.body
                )
            })
            .to_string();

        let delivered = emulator
            .delivered(bucket, &delivery_prefix(&key_prefix))
            .await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the refused call delivered once, got {delivered:?}"
        );
        let record = &delivered[0].1;
        assert_eq!(
            (
                text(record, "errorCode"),
                text(record, "errorMessage"),
                text(record, "requestID"),
                text(record, "eventCategory"),
            ),
            (
                shape,
                message,
                answered.request_id,
                DATA_CATEGORY.to_string()
            ),
            "case {case:?}: expected the fault the caller saw, got {record}"
        );
        assert_eq!(
            record.get("responseElements"),
            Some(&Value::Null),
            "case {case:?}: a refused call answered nothing, got {record}"
        );
    }
}

/// RULE: a DATA event is not event history — no call of this service's data plane appears in
/// the account's management-event log, whatever it answered, while every control-plane call
/// does.
///
/// The rows differ along the operation, along the outcome (served and refused, over two
/// different fault shapes) and along whether the call is on the data plane at all.
#[tokio::test]
async fn a_data_plane_call_reaches_the_management_event_log_never() {
    let emulator = Emulator::new();
    let subject = emulator.express_machine("classified-subject").await;
    // A STANDARD machine, so a synchronous start on it is refused for its TYPE rather than
    // for not existing.
    let standard = emulator
        .ok(
            "setup",
            "CreateStateMachine",
            &json!({
                "name": "classified-standard",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "STANDARD",
            }),
        )
        .await
        .body["stateMachineArn"]
        .as_str()
        .expect("CreateStateMachine answers a stateMachineArn")
        .to_string();
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:classified-absent");

    // (case, operation, request, whether event history holds a record of the call)
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "a served synchronous run",
            "StartSyncExecution",
            json!({"stateMachineArn": &subject, "input": SECRET_INPUT}),
            false,
        ),
        (
            "a synchronous run of a machine that was never created",
            "StartSyncExecution",
            json!({"stateMachineArn": &absent, "input": SECRET_INPUT}),
            false,
        ),
        (
            "a synchronous run of a machine of the wrong type",
            "StartSyncExecution",
            json!({"stateMachineArn": &standard, "input": SECRET_INPUT}),
            false,
        ),
        (
            "an asynchronous start, which is administration",
            "StartExecution",
            json!({"stateMachineArn": &subject, "input": SECRET_INPUT}),
            true,
        ),
        (
            "a read of the same machine",
            "DescribeStateMachine",
            json!({"stateMachineArn": &subject}),
            true,
        ),
        (
            "a refused read of a machine that was never created",
            "DescribeStateMachine",
            json!({"stateMachineArn": &absent}),
            true,
        ),
    ];

    for (case, operation, request, recorded) in rows {
        let before = emulator.recorded_events().len();
        let answered = emulator.call(operation, &request).await;
        let after = emulator.recorded_events();
        let added = &after[before..];
        if recorded {
            assert_eq!(
                added.len(),
                1,
                "case {case:?}: expected exactly one record of the served call (answered {} \
                 with {}), got {added:?}",
                answered.status,
                answered.body
            );
            assert_eq!(
                (
                    text(&added[0], "eventName"),
                    text(&added[0], "eventCategory")
                ),
                (operation.to_string(), MANAGEMENT_CATEGORY.to_string()),
                "case {case:?}: expected a {MANAGEMENT_CATEGORY} record of {operation}, got {:?}",
                added[0]
            );
        } else {
            assert!(
                added.is_empty(),
                "case {case:?}: a data event is no event history, but the call (answered {} \
                 with {}) left {added:?}",
                answered.status,
                answered.body
            );
        }
    }
}
