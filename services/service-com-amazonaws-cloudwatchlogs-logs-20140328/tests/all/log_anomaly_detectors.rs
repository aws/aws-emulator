// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateLogAnomalyDetector` and the read it is proved through,
//! `GetLogAnomalyDetector`.
//!
//! The rule under test: `CreateLogAnomalyDetector` persists ONE anomaly detector over the
//! single log group ARN the request names, mints an ARN that identifies it and hands that
//! ARN back, and `GetLogAnomalyDetector` reports, under that ARN, exactly the configuration
//! the create stored — a member the service defaults (`evaluationFrequency`,
//! `anomalyVisibilityTime`) carrying that default when the create named none, every other
//! optional member present only when a call named it — in the status the CLOCK has made due.
//! A request that names no log group, more than one, an ARN that is not a log group's, or a
//! log group the account does not hold creates nothing and is refused; so is a read of an
//! ARN no detector is stored under.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: a detector named
//! and unnamed, EVERY value of the model's `EvaluationFrequency` enum (source:
//! `smithy_query(query="shape",shape="EvaluationFrequency")`), `anomalyVisibilityTime` at
//! both published bounds of its `@range(7..90)` and one past each, each form a
//! `filterPattern` takes (absent, empty, a term and a bracketed one) and an unbalanced one,
//! a KMS key assigned and not, tags and none, and a log-group list of zero, one and two
//! ARNs, in this region's account and in another region's and another account's. The
//! account's detector ceiling has its own case, filled to exactly the bound.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    anomaly_detector_arn, log_group_arn, AnomalyDetectorRecord,
    ComAmazonawsCloudwatchlogsLogs20140328State, ANOMALY_DETECTOR_ANALYSIS_DELAY_MS,
    ANOMALY_DETECTOR_STATUS_ANALYZING, DEFAULT_ANOMALY_VISIBILITY_TIME_DAYS,
    DEFAULT_EVALUATION_FREQUENCY, MAX_ANOMALY_DETECTORS,
};

use crate::harness::{admissible_detector_statuses, now_ms, Logs, REGION};

/// The operation that creates a detector, and the one that reports it back.
const CREATE: &str = "CreateLogAnomalyDetector";
const GET: &str = "GetLogAnomalyDetector";

/// The log group every accepted case watches.
const GROUP: &str = "anomaly-detectors/watched";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what
/// `CreateLogAnomalyDetector` declares for a refusal ("A parameter is specified
/// incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a create would grow the account's store past its ceiling.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The code answered when the request provided none of an input the operation requires,
/// which is a request-validation failure rather than a value the service rejected.
const VALIDATION: &str = "ValidationException";

/// A KMS key ARN of the form `DetectorKmsKeyArn`'s `@pattern` admits, for the rows that
/// assign one: "Optionally assigns a KMS key to secure this anomaly detector".
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// Every value of the model's `EvaluationFrequency` enum, in the order the model lists
/// them. Source: `smithy_query(query="shape",shape="EvaluationFrequency")`.
const EVALUATION_FREQUENCIES: [&str; 6] = [
    "ONE_MIN",
    "FIVE_MIN",
    "TEN_MIN",
    "FIFTEEN_MIN",
    "THIRTY_MIN",
    "ONE_HOUR",
];

/// One accepted create: the optional members it names, each of which the read must report
/// back exactly when the create named it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    detector_name: Option<&'static str>,
    evaluation_frequency: Option<&'static str>,
    filter_pattern: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    anomaly_visibility_time: Option<i64>,
    tags: Option<&'static [(&'static str, &'static str)]>,
}

impl Row {
    /// A row naming nothing but the required log group list.
    fn bare(case: &'static str) -> Self {
        Self {
            case,
            detector_name: None,
            evaluation_frequency: None,
            filter_pattern: None,
            kms_key_id: None,
            anomaly_visibility_time: None,
            tags: None,
        }
    }
}

/// The accepted corpus: every optional member absent, each present on its own, and all of
/// them together, plus one row per `EvaluationFrequency` value and one per published bound
/// of `anomalyVisibilityTime`.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row::bare("nothing but the log group it watches"),
        Row {
            detector_name: Some("named-detector"),
            ..Row::bare("a detector name")
        },
        Row {
            filter_pattern: Some(""),
            ..Row::bare("the empty filter pattern, which FilterPattern's length 0.. admits")
        },
        Row {
            filter_pattern: Some("ERROR"),
            ..Row::bare("a term filter pattern")
        },
        Row {
            filter_pattern: Some("{ $.level = \"ERROR\" }"),
            ..Row::bare("a bracketed JSON filter pattern")
        },
        Row {
            kms_key_id: Some(KMS_KEY),
            ..Row::bare("a KMS key assigned to the detector")
        },
        Row {
            tags: Some(&[("team", "observability"), ("env", "test")]),
            ..Row::bare("two tags")
        },
        Row {
            detector_name: Some("fully-configured"),
            evaluation_frequency: Some("FIFTEEN_MIN"),
            filter_pattern: Some("[ip, user, timestamp]"),
            kms_key_id: Some(KMS_KEY),
            anomaly_visibility_time: Some(30),
            tags: Some(&[("owner", "logs")]),
            case: "every optional member at once",
        },
    ];
    for frequency in EVALUATION_FREQUENCIES {
        rows.push(Row {
            evaluation_frequency: Some(frequency),
            ..Row::bare(frequency)
        });
    }
    // `AnomalyVisibilityTime` is `@range(min: 7, max: 90)`: both published bounds, and a
    // value between them.
    for days in [7, 21, 90] {
        rows.push(Row {
            anomaly_visibility_time: Some(days),
            ..Row::bare("an anomaly visibility time")
        });
    }
    rows
}

/// The create body for one row: the log group it watches plus only the members the row
/// names, so an absent member is absent on the wire too.
fn create_body(group_arn: &str, row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("logGroupArnList".to_string(), json!([group_arn]));
    if let Some(name) = row.detector_name {
        body.insert("detectorName".to_string(), json!(name));
    }
    if let Some(frequency) = row.evaluation_frequency {
        body.insert("evaluationFrequency".to_string(), json!(frequency));
    }
    if let Some(pattern) = row.filter_pattern {
        body.insert("filterPattern".to_string(), json!(pattern));
    }
    if let Some(key) = row.kms_key_id {
        body.insert("kmsKeyId".to_string(), json!(key));
    }
    if let Some(days) = row.anomaly_visibility_time {
        body.insert("anomalyVisibilityTime".to_string(), json!(days));
    }
    if let Some(tags) = row.tags {
        body.insert(
            "tags".to_string(),
            Value::Object(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// One string member of a response, or `None` when the response does not carry it.
fn member<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

/// One epoch-millisecond member of a response.
fn stamp(body: &Value, key: &str, case: &str) -> i64 {
    body.get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: a detector carries {key}, got {body}"))
}

/// The log group every accepted case watches, created, with the ARN form the modelled
/// `LogGroupArn` pattern admits (`^[\w#+=/:,.@-]*$` has no `*`, so the `LogGroup$arn` form
/// ending in `:*` is not one of them).
async fn watched_group(logs: &Logs) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (_arn, log_group_arn) = logs.group_arns(GROUP).await;
    log_group_arn
}

/// Creating a detector persists it, and reading it back reports every member the create
/// stored — and only those: an optional member the create did not name is absent.
#[tokio::test]
async fn a_created_detector_is_reported_back_exactly_as_it_was_configured() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;
    let arn_prefix = anomaly_detector_arn(REGION, "");

    let mut minted: Vec<String> = Vec::new();
    for row in rows() {
        let case = row.case;
        let before = now_ms();
        let created = logs.ok(CREATE, &create_body(&group, &row)).await;
        let arn = member(&created, "anomalyDetectorArn")
            .unwrap_or_else(|| {
                panic!("case {case:?}: a create answers anomalyDetectorArn, got {created}")
            })
            .to_string();

        assert!(
            arn.starts_with(&arn_prefix),
            "case {case:?}: expected an ARN under {arn_prefix:?}, got {arn:?}"
        );
        let id = &arn[arn_prefix.len()..];
        assert!(
            id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "case {case:?}: expected a UUID-shaped detector id, got {id:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: every create mints a new ARN, but {arn:?} was handed out before"
        );
        minted.push(arn.clone());

        let reported = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let after = now_ms();

        assert_eq!(
            reported.get("logGroupArnList"),
            Some(&json!([group])),
            "case {case:?}: expected the watched log group {group:?}, got {reported}"
        );
        // (member, what the create named) — `evaluationFrequency` is the one the service
        // DEFAULTS, so a create that named none reports the default rather than nothing.
        let echoed: [(&str, Option<&str>); 3] = [
            ("detectorName", row.detector_name),
            (
                "evaluationFrequency",
                Some(
                    row.evaluation_frequency
                        .unwrap_or(DEFAULT_EVALUATION_FREQUENCY),
                ),
            ),
            ("filterPattern", row.filter_pattern),
        ];
        for (key, named) in echoed {
            assert_eq!(
                member(&reported, key),
                named,
                "case {case:?}: expected {key} {named:?}, got {reported}"
            );
        }
        assert_eq!(
            member(&reported, "kmsKeyId"),
            row.kms_key_id,
            "case {case:?}: expected kmsKeyId {:?}, got {reported}",
            row.kms_key_id
        );
        // The other member the service defaults: a create that named none reports the
        // default life cycle the detector runs with.
        let expected_visibility = row
            .anomaly_visibility_time
            .unwrap_or(DEFAULT_ANOMALY_VISIBILITY_TIME_DAYS);
        assert_eq!(
            reported.get("anomalyVisibilityTime").and_then(Value::as_i64),
            Some(expected_visibility),
            "case {case:?}: expected anomalyVisibilityTime {expected_visibility:?}, got {reported}"
        );

        let creation = stamp(&reported, "creationTimeStamp", case);
        assert!(
            (before..=after).contains(&creation),
            "case {case:?}: expected a creation stamp in {before}..={after}, got {creation}"
        );
        let modified = stamp(&reported, "lastModifiedTimeStamp", case);
        assert_eq!(
            modified, creation,
            "case {case:?}: a detector nothing has updated was last modified when it was \
             created ({creation}), got {modified}"
        );

        let admissible = admissible_detector_statuses(creation, before, after);
        let status = member(&reported, "anomalyDetectorStatus").unwrap_or_else(|| {
            panic!("case {case:?}: a detector carries anomalyDetectorStatus, got {reported}")
        });
        assert!(
            admissible.contains(&status),
            "case {case:?}: expected one of {admissible:?}, got {status:?}"
        );

        // No modelled `AnomalyDetector` member reports a detector's tags, so the tags the
        // create was given are checked where they are stored; a wire reader for them
        // arrives with the tagging operations that reach this resource.
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let stored: Vec<(String, String)> = state
            .lock()
            .anomaly_detectors
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case:?}: the created detector is stored under {arn}"))
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut expected: Vec<(String, String)> = row
            .tags
            .unwrap_or(&[])
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the tags {expected:?}, got {stored:?}"
        );
    }

    assert_eq!(
        minted.len(),
        rows().len(),
        "every accepted create persists one detector, expected {} ARNs, got {minted:?}",
        rows().len()
    );
}

/// The status a read reports is the one the CLOCK already made due — a detector trains,
/// then analyzes — and reading a detector the clock owes nothing reports the same thing
/// every time.
#[tokio::test]
async fn the_status_the_clock_owes_is_reported_and_reading_changes_nothing() {
    let logs = Logs::new();
    let group = log_group_arn(REGION, GROUP, false);
    let now = now_ms();

    // (case, how long ago the detector was created)
    let ages: [(&str, i64); 4] = [
        ("created just now", 0),
        (
            "created one millisecond before analysis was due",
            ANOMALY_DETECTOR_ANALYSIS_DELAY_MS - 1,
        ),
        (
            "created after analysis became due",
            ANOMALY_DETECTOR_ANALYSIS_DELAY_MS + 1,
        ),
        ("created an hour ago", 60 * 60 * 1_000),
    ];
    let mut seeded: Vec<(&str, String, i64)> = Vec::new();
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for (index, (case, age)) in ages.into_iter().enumerate() {
            let created = now - age;
            let arn = anomaly_detector_arn(REGION, &format!("0000000{index}-0000-0000-0000-000000000000"));
            let record = AnomalyDetectorRecord::new(&arn, vec![group.clone()], created);
            store.anomaly_detectors.insert(arn.clone(), record);
            seeded.push((case, arn, created));
        }
    }

    for (case, arn, created) in &seeded {
        let before = now_ms();
        let reported = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
        let after = now_ms();
        let admissible = admissible_detector_statuses(*created, before, after);
        let status = member(&reported, "anomalyDetectorStatus").unwrap_or_else(|| {
            panic!("case {case:?}: a detector carries anomalyDetectorStatus, got {reported}")
        });
        assert!(
            admissible.contains(&status),
            "case {case:?}: expected one of {admissible:?}, got {status:?}"
        );
        assert_eq!(
            stamp(&reported, "lastModifiedTimeStamp", case),
            *created,
            "case {case:?}: a status the clock owed is not a modification of the detector, \
             so it was last modified when it was created ({created}), got {reported}"
        );
    }

    // A detector the clock owes nothing — the one created an hour ago, which has reached
    // ANALYZING — reports the same thing however often it is read.
    let (case, arn, _created) = seeded
        .last()
        .expect("the seeded detectors include the oldest");
    let first = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
    assert_eq!(
        member(&first, "anomalyDetectorStatus"),
        Some(ANOMALY_DETECTOR_STATUS_ANALYZING),
        "case {case:?}: a detector created an hour ago has finished training, got {first}"
    );
    let second = logs.ok(GET, &json!({"anomalyDetectorArn": arn})).await;
    assert_eq!(
        second, first,
        "case {case:?}: reading twice must report the same thing, expected {first}, got {second}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<String> = state.lock().anomaly_detectors.keys().cloned().collect();
    let mut expected: Vec<String> = seeded.iter().map(|(_, arn, _)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "reading detectors creates and removes none, expected {expected:?}, got {stored:?}"
    );
}

/// A request that does not name exactly one log group ARN, or names something that is not
/// one, creates nothing and is refused.
#[tokio::test]
async fn a_request_that_names_no_single_log_group_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;
    let (wildcard_arn, _log_group_arn) = logs.group_arns(GROUP).await;
    let stream_arn = format!("{group}:log-stream:anomalies");

    // (case, the logGroupArnList, the code the refusal carries)
    let cases: Vec<(&str, Value, &str)> = vec![
        ("no log group at all", json!([]), VALIDATION),
        (
            "two log groups, where only one may be specified",
            json!([group.clone(), log_group_arn(REGION, "anomaly-detectors/other", false)]),
            INVALID_PARAMETER,
        ),
        (
            "a value that is not an ARN",
            json!(["not-an-arn"]),
            INVALID_PARAMETER,
        ),
        (
            "a well-formed ARN of another service",
            json!(["arn:aws:s3:::anomaly-detector-bucket"]),
            INVALID_PARAMETER,
        ),
        (
            "a log stream's ARN rather than its log group's",
            json!([stream_arn]),
            INVALID_PARAMETER,
        ),
        (
            "a log group that does not exist",
            json!([log_group_arn(REGION, "anomaly-detectors/absent", false)]),
            NOT_FOUND,
        ),
        (
            // The ARN names a region as well as a name, so this one names another region's
            // log group even though a group of that name is stored here.
            "a log group of this name in another region",
            json!([log_group_arn("us-east-1", GROUP, false)]),
            NOT_FOUND,
        ),
        (
            "a log group of this name in another account",
            json!([format!("arn:aws:logs:{REGION}:123456789012:log-group:{GROUP}")]),
            NOT_FOUND,
        ),
        (
            // `LogGroupArn`'s pattern `^[\w#+=/:,.@-]*$` has no `*`, so the `LogGroup$arn`
            // form ending in `:*` is refused at the boundary.
            "the wildcard log group ARN form",
            json!([wildcard_arn]),
            INVALID_PARAMETER,
        ),
    ];

    for (case, list, expected) in cases {
        let code = logs
            .err(CREATE, &json!({"logGroupArnList": list}))
            .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }

    // An absent `@required` member is refused with the same code.
    let code = logs.err(CREATE, &json!({"detectorName": "no-groups"})).await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case \"no logGroupArnList at all\": expected {INVALID_PARAMETER}, got {code}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().anomaly_detectors.len();
    assert_eq!(
        stored, 0,
        "a refused create persists nothing, expected an empty store, got {stored} detectors"
    );
}

/// A value outside what the model declares for a member, and a filter pattern the service
/// cannot parse, are refused rather than stored.
#[tokio::test]
async fn a_value_the_model_or_the_pattern_syntax_rejects_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;

    // (case, the member and value, the refusal message when the service builds one)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an anomaly visibility time one below the minimum of 7",
            json!({"anomalyVisibilityTime": 6}),
            Some(
                "1 validation error detected: Value at 'anomalyVisibilityTime' failed to \
                 satisfy constraint: Member must have value greater than or equal to 7"
                    .to_string(),
            ),
        ),
        (
            "an anomaly visibility time one above the maximum of 90",
            json!({"anomalyVisibilityTime": 91}),
            Some(
                "1 validation error detected: Value at 'anomalyVisibilityTime' failed to \
                 satisfy constraint: Member must have value less than or equal to 90"
                    .to_string(),
            ),
        ),
        (
            "an evaluation frequency outside the modelled enum",
            json!({"evaluationFrequency": "FIVE_MINUTES"}),
            None,
        ),
        (
            "a kmsKeyId that is not a KMS key ARN",
            json!({"kmsKeyId": "not-a-key"}),
            None,
        ),
        (
            "a filter pattern that opens a bracketed form and never closes it",
            json!({"filterPattern": "{ $.level = \"ERROR\""}),
            Some("If a filter pattern starts with '{' it must end with '}'".to_string()),
        ),
    ];

    for (case, extra, expected_message) in cases {
        let mut body = Map::new();
        body.insert("logGroupArnList".to_string(), json!([group]));
        for (key, value) in extra.as_object().expect("each case names members") {
            body.insert(key.clone(), value.clone());
        }
        let body = Value::Object(body);
        let (code, message) = logs.refusal(CREATE, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().anomaly_detectors.len();
    assert_eq!(
        stored, 0,
        "a refused create persists nothing, expected an empty store, got {stored} detectors"
    );
}

/// A create that would grow the account's detector store past the emulator's own ceiling is
/// refused with the `LimitExceededException` the operation models, and stores nothing.
#[tokio::test]
async fn a_create_past_the_account_ceiling_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;

    // The store filled to exactly its ceiling, through the same record constructor the
    // create uses: the rule is about the COUNT the store holds, so how each detector got
    // there does not matter.
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for index in 0..MAX_ANOMALY_DETECTORS {
            let arn = anomaly_detector_arn(REGION, &format!("ceiling-{index}"));
            let record = AnomalyDetectorRecord::new(&arn, vec![group.clone()], now_ms());
            store.anomaly_detectors.insert(arn, record);
        }
    }

    let (code, message) = logs
        .refusal(CREATE, &json!({"logGroupArnList": [group]}))
        .await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case \"one create past the ceiling\": expected {LIMIT_EXCEEDED}, got {code} {message:?}"
    );
    assert!(
        message.contains(&MAX_ANOMALY_DETECTORS.to_string()),
        "case \"one create past the ceiling\": the refusal states the ceiling \
         ({MAX_ANOMALY_DETECTORS}), got {message:?}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().anomaly_detectors.len();
    assert_eq!(
        stored, MAX_ANOMALY_DETECTORS,
        "a refused create persists nothing, expected {MAX_ANOMALY_DETECTORS} detectors, got \
         {stored}"
    );
}

/// Reading an ARN no detector is stored under is a missing resource, not an empty report —
/// including an ARN this service's own ARN builder produces for an id nothing minted.
#[tokio::test]
async fn reading_a_detector_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let group = watched_group(&logs).await;
    let created = logs
        .ok(CREATE, &json!({"logGroupArnList": [group]}))
        .await;
    let held = member(&created, "anomalyDetectorArn")
        .expect("a create answers anomalyDetectorArn")
        .to_string();

    // (case, the ARN read)
    let cases: Vec<(&str, String)> = vec![
        (
            "an anomaly-detector ARN for an id nothing minted",
            anomaly_detector_arn(REGION, "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ),
        (
            "the same detector in another region",
            anomaly_detector_arn("us-east-1", &held[anomaly_detector_arn(REGION, "").len()..]),
        ),
        (
            "an ARN naming another kind of resource entirely",
            log_group_arn(REGION, GROUP, false),
        ),
        ("a value that is not an ARN at all", "detector-1".to_string()),
    ];

    for (case, arn) in cases {
        let (code, message) = logs
            .refusal(GET, &json!({"anomalyDetectorArn": arn}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("The specified ARN does not exist or already deleted : {arn}"),
            "case {case:?}: expected the refusal to name the ARN that was not found, got {message:?}"
        );
    }

    // The detector the account DOES hold is still readable: a refused read removed nothing.
    let reported = logs.ok(GET, &json!({"anomalyDetectorArn": held})).await;
    assert_eq!(
        reported.get("logGroupArnList").and_then(Value::as_array).map(Vec::len),
        Some(1),
        "the held detector still watches its log group, got {reported}"
    );
}
