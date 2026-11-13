// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeConfigurationTemplates`: the vended-log delivery
//! configuration templates this service publishes.
//!
//! The rule under test: the operation reports the SERVICE-MANAGED catalogue of configuration
//! templates — one template per (service, resource type, log type, delivery destination type)
//! the service supports — selecting the templates every filter the request names applies to,
//! in a stable total order, one `limit`-sized page at a time; the values a template carries
//! are derived from what receives the delivery, and no call changes the catalogue.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire.
//!
//! The table spans what the rule generalises over rather than one request: each filter member
//! on its own and in combination, with one value and with several, including a value the
//! catalogue does not carry and a combination nothing satisfies; every page size from one
//! template to more than the catalogue holds, plus the default; and the invariants each
//! reported template must satisfy, checked over the WHOLE catalogue rather than one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::CONFIGURATION_TEMPLATE_SOURCES;

use crate::harness::Logs;

/// The operation under test, and the two it is read beside.
const DESCRIBE: &str = "DescribeConfigurationTemplates";
const PUT_SOURCE: &str = "PutDeliverySource";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const DESCRIBE_SOURCES: &str = "DescribeDeliverySources";

/// The refusal this operation models for a request it cannot answer.
const VALIDATION: &str = "ValidationException";

/// The `DeliveryDestinationType`s the model's enum names
/// (`smithy_query(query="shape",shape="DeliveryDestinationType")`), the one of them that
/// receives traces rather than log records, and the order the enum DECLARES them in — which is
/// the order one log type's templates are published in.
const DESTINATION_TYPES: [&str; 4] = ["CWL", "FH", "S3", "XRAY"];
const TRACE_DESTINATION_TYPE: &str = "XRAY";
const DESTINATION_TYPE_ORDER: [&str; 4] = ["S3", "CWL", "FH", "XRAY"];

/// The `OutputFormat`s the model's enum names
/// (`smithy_query(query="shape",shape="OutputFormat")`).
const OUTPUT_FORMATS: [&str; 5] = ["json", "parquet", "plain", "raw", "w3c"];

/// The `logType` of a source that sends traces rather than log records, as the `logType`
/// member's own catalogue spells it
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
const TRACE_LOG_TYPE: &str = "TRACES";

/// What the operation must publish for a delivery that can be written in a delimited format,
/// and the delimiter it defaults to: `FieldDelimiter` is modelled `@length 0..5`, so NO
/// delimiter is a value in its own right and it is the default a template publishes.
const FIELD_DELIMITERS: [&str; 3] = ["\t", " ", ","];
const DEFAULT_FIELD_DELIMITER: &str = "";
/// The variable field every S3 delivery's suffix path opens with, the Region it must also name,
/// and the time partition every one of them closes with. Spelled out here rather than imported,
/// so an edit to either published set is caught instead of agreeing with itself.
const SUFFIX_PATH_ACCOUNT_FIELD: &str = "accountid";
const SUFFIX_PATH_REGION_FIELD: &str = "region";
const SUFFIX_PATH_TIME_FIELDS: [&str; 4] = ["yyyy", "MM", "dd", "HH"];

/// The fields a template ALLOWS that a delivery created without a record field list does not
/// carry: `allowedFields` is "the allowed fields that a caller can use in the recordFields
/// parameter" while `defaultDeliveryConfigValues.recordFields` is "the default record fields
/// that will be delivered when a list of record fields is not provided", so the default list is
/// the allowed fields less these — except a mandatory one, which "must be present in the
/// recordFields parameter" and so is always delivered.
const FIELDS_NOT_DELIVERED_BY_DEFAULT: [&str; 2] = ["timestamp", "resource_id"];

/// The IAM action, without its service prefix, that a template names as the permission a
/// caller needs "to be able to successfully create a delivery source on the desired resource
/// type when calling PutDeliverySource".
const VENDED_LOGS_DELIVERY_ACTION: &str = "AllowVendedLogDeliveryForResource";

/// How many templates the published catalogue holds, and a few of the templates it must
/// publish — stated here rather than derived, so an accidental edit to a catalogue row is
/// caught instead of agreed with. Each pinned row is
/// `service|logType|resourceType|deliveryDestinationType`: a CloudFront access log delivered
/// to S3, a WorkMail access-control log delivered to a log group, an Application Load
/// Balancer access log delivered to Firehose, an AgentCore trace delivered to X-Ray, a
/// Security Hub finding log delivered to S3 from each of the two hub resources that send one,
/// and an Amazon Q Business event log delivered to a log group.
const CATALOGUE_TEMPLATES: usize = 188;
const PINNED_TEMPLATES: [&str; 9] = [
    "cloudfront|ACCESS_LOGS|distribution|S3",
    "workmail|ACCESS_CONTROL_LOGS|organization|CWL",
    "elasticloadbalancing|ALB_ACCESS_LOGS|loadbalancer|FH",
    "bedrock-agentcore|TRACES|runtime|XRAY",
    "apigateway|EXECUTION_LOGS|stages|S3",
    "eks|AUTO_MODE_LOAD_BALANCING_LOGS|cluster|FH",
    "securityhub|SECURITY_FINDING_LOGS|hub|S3",
    "securityhub|SECURITY_FINDING_LOGS|hubv2|S3",
    "qbusiness|EVENT_LOGS|application|CWL",
];

/// The published bounds of the members a request names: `limit` is `DescribeLimit`
/// (`range 1..50`), `logTypes` and `resourceTypes` are `len 1..10`,
/// `deliveryDestinationTypes` is `len 1..4`, `FieldHeader` (a record field name) is
/// `len 1..64`, `FieldDelimiter` is `len 0..5` and `DeliverySuffixPath` is `len 1..256`.
/// Source: `smithy_query(query="operation",operation="DescribeConfigurationTemplates")` and
/// the closure of its output shape.
const MIN_LIMIT: usize = 1;
const MAX_LIMIT: usize = 50;
const MAX_FIELD_HEADER_CHARS: usize = 64;
const MAX_FIELD_DELIMITER_CHARS: usize = 5;
const MAX_SUFFIX_PATH_CHARS: usize = 256;

/// One resource whose logs a delivery source can represent, used only to put something in the
/// account's store: the templates must not move when it does.
const WORKMAIL_ARN: &str =
    "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234abcd1234abcd1234";
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-destination";

/// What identifies one reported template, and the order the operation enumerates them in: its
/// service, log type, resource type and delivery destination type.
fn key_of(template: &Value) -> String {
    let member = |name: &str| {
        template
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a reported template carries {name}, got {template}"))
            .to_string()
    };
    [
        member("service"),
        member("logType"),
        member("resourceType"),
        member("deliveryDestinationType"),
    ]
    .join("|")
}

/// Every template the published catalogue implies, keyed as [`key_of`] keys a reported one, in
/// the order the catalogue publishes them.
///
/// DERIVED here from the catalogue data (`CONFIGURATION_TEMPLATE_SOURCES`, whose service and
/// log-type pairings come from the `logType` member of `PutDeliverySourceRequest`) by the rule
/// under test: one template per log type per delivery destination type it can be sent to — the
/// X-Ray trace destination receives a TRACES log type and nothing else, while every log type,
/// traces included, can be delivered to each destination type that receives records — taken in
/// the order the model's enum declares the destination types.
fn catalogue() -> Vec<(String, String)> {
    assert!(
        DESTINATION_TYPE_ORDER.iter().all(|destination_type| DESTINATION_TYPES.contains(destination_type))
            && DESTINATION_TYPE_ORDER.len() == DESTINATION_TYPES.len(),
        "the published order names every destination type the model does, got {DESTINATION_TYPE_ORDER:?}"
    );
    let mut templates: Vec<(String, String)> = Vec::new();
    for source in CONFIGURATION_TEMPLATE_SOURCES {
        for log_type in source.log_types {
            let log_type = log_type.log_type;
            for destination_type in DESTINATION_TYPE_ORDER {
                if destination_type == TRACE_DESTINATION_TYPE && log_type != TRACE_LOG_TYPE {
                    continue;
                }
                templates.push((
                    format!(
                        "{}|{log_type}|{}|{destination_type}",
                        source.service, source.resource_type
                    ),
                    log_type.to_string(),
                ));
            }
        }
    }
    templates
}

/// One page of templates, and the token that continues the enumeration.
async fn page(logs: &Logs, body: &Value) -> (Vec<Value>, Option<String>) {
    let response = logs.ok(DESCRIBE, body).await;
    let page = response
        .get("configurationTemplates")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE} {body}: the response must carry configurationTemplates, got {response}")
        })
        .clone();
    let token = response
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (page, token)
}

/// Every template one filtered enumeration holds, walked page by page, asserting the page
/// discipline as it goes: a page holds at most `limit` templates, and a token is handed back
/// exactly while templates remain.
async fn walk(logs: &Logs, case: &str, filter: &Value, limit: Option<usize>) -> Vec<Value> {
    let mut templates: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = filter.clone();
        if let Some(limit) = limit {
            body["limit"] = json!(limit);
        }
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let (reported, next) = page(logs, &body).await;
        if let Some(limit) = limit {
            assert!(
                reported.len() <= limit,
                "case {case}: a page holds at most {limit} templates, got {}",
                reported.len()
            );
        }
        templates.extend(reported);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    templates
}

/// The whole catalogue is reported, in one stable total order, whatever the page size — and the
/// pages concatenated are the enumeration, with nothing repeated and nothing left out.
#[tokio::test]
async fn the_catalogue_is_enumerated_in_one_order_whatever_the_page_size() {
    let logs = Logs::new();
    let expected: Vec<String> = catalogue().into_iter().map(|(key, _)| key).collect();
    assert_eq!(
        expected.len(),
        CATALOGUE_TEMPLATES,
        "the catalogue holds {CATALOGUE_TEMPLATES} templates, got {}",
        expected.len()
    );
    assert!(
        expected.len() > MAX_LIMIT,
        "the catalogue must hold more templates than the largest page, so paging is exercised: got {}",
        expected.len()
    );
    // A handful of rows stated here rather than derived, so an edit to a catalogue row — a
    // service namespace, a resource type, a log type — fails this suite instead of agreeing
    // with itself. Each is a (service, log type, resource type, destination type) the rule
    // implies for a row of the published catalogue.
    for key in PINNED_TEMPLATES {
        assert!(
            expected.contains(&key.to_string()),
            "case {key:?}: the catalogue publishes this template, got {expected:?}"
        );
    }

    // Every page size from one template to more than the catalogue holds, plus the default
    // (the request names no `limit` at all).
    for limit in [None, Some(MIN_LIMIT), Some(2), Some(7), Some(MAX_LIMIT)] {
        let case = format!("limit {limit:?}");
        let reported = walk(&logs, &case, &json!({}), limit).await;
        let keys: Vec<String> = reported.iter().map(key_of).collect();
        assert_eq!(
            keys, expected,
            "case {case}: expected the pages to enumerate {expected:?}, got {keys:?}"
        );
    }
}

/// Each filter "include[s] only the configuration templates that apply to" the values it names,
/// and every filter a request names narrows the response further.
#[tokio::test]
async fn each_filter_selects_what_it_names_and_several_filters_narrow() {
    let logs = Logs::new();
    let catalogue = catalogue();

    // (case, the filter, the predicate the rule says it selects by)
    type Selects = fn(&str) -> bool;
    let cases: [(&str, Value, Selects); 12] = [
        ("no filter", json!({}), |_key| true),
        (
            "one service",
            json!({"service": "workmail"}),
            |key| key.starts_with("workmail|"),
        ),
        (
            "another service",
            json!({"service": "bedrock-agentcore"}),
            |key| key.starts_with("bedrock-agentcore|"),
        ),
        (
            "a service the catalogue does not carry",
            json!({"service": "not_a_service"}),
            |_key| false,
        ),
        (
            "one log type",
            json!({"logTypes": ["ACCESS_LOGS"]}),
            |key| field(key, 1) == "ACCESS_LOGS",
        ),
        (
            "several log types",
            json!({"logTypes": ["ACCESS_LOGS", "USAGE_LOGS", TRACE_LOG_TYPE]}),
            |key| ["ACCESS_LOGS", "USAGE_LOGS", TRACE_LOG_TYPE].contains(&field(key, 1)),
        ),
        (
            "one resource type",
            json!({"resourceTypes": ["organization"]}),
            |key| field(key, 2) == "organization",
        ),
        (
            "several resource types",
            json!({"resourceTypes": ["app", "bucket", "log-group"]}),
            |key| ["app", "bucket", "log-group"].contains(&field(key, 2)),
        ),
        (
            "one delivery destination type",
            json!({"deliveryDestinationTypes": ["S3"]}),
            |key| field(key, 3) == "S3",
        ),
        (
            "every delivery destination type",
            json!({"deliveryDestinationTypes": DESTINATION_TYPES}),
            |_key| true,
        ),
        (
            "a service and a log type together",
            json!({"service": "ses", "logTypes": ["APPLICATION_LOGS"]}),
            |key| key.starts_with("ses|") && field(key, 1) == "APPLICATION_LOGS",
        ),
        (
            "a log type and a resource type nothing pairs",
            json!({"logTypes": [TRACE_LOG_TYPE], "resourceTypes": ["organization"]}),
            |_key| false,
        ),
    ];

    for (case, filter, selects) in cases {
        let expected: Vec<String> = catalogue
            .iter()
            .map(|(key, _)| key.clone())
            .filter(|key| selects(key))
            .collect();
        let reported = walk(&logs, case, &filter, Some(MAX_LIMIT)).await;
        let keys: Vec<String> = reported.iter().map(key_of).collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected {expected:?}, got {keys:?}"
        );
    }

    // Each destination type selects the templates that can be delivered to it, and the trace
    // destination carries the trace log type alone.
    for destination_type in DESTINATION_TYPES {
        let case = format!("destination type {destination_type}");
        let reported = walk(
            &logs,
            &case,
            &json!({"deliveryDestinationTypes": [destination_type]}),
            Some(MAX_LIMIT),
        )
        .await;
        assert!(
            !reported.is_empty(),
            "case {case}: every destination type the model names has templates, got none"
        );
        for template in &reported {
            let key = key_of(template);
            assert_eq!(
                field(&key, 3),
                destination_type,
                "case {case}: every selected template is for that destination type, got {key}"
            );
            if destination_type == TRACE_DESTINATION_TYPE {
                assert_eq!(
                    field(&key, 1),
                    TRACE_LOG_TYPE,
                    "case {case}: the trace destination receives traces alone, got {key}"
                );
            }
        }
    }
}

/// The nth value of a template key, as [`key_of`] joins them.
fn field(key: &str, index: usize) -> &str {
    key.split('|')
        .nth(index)
        .unwrap_or_else(|| panic!("a template key carries four values, got {key}"))
}

/// What a template allows and defaults to is derived from what RECEIVES the delivery: the
/// formats it can be written in, whether a field delimiter applies at all, and whether an S3
/// object layout applies — and every default it reports is a value the same template allows.
#[tokio::test]
async fn every_template_derives_its_allowed_values_from_its_destination_type() {
    let logs = Logs::new();
    let templates = walk(&logs, "every template", &json!({}), Some(MAX_LIMIT)).await;
    assert_eq!(
        templates.len(),
        catalogue().len(),
        "the whole catalogue is under test, got {} templates",
        templates.len()
    );

    for template in &templates {
        let case = key_of(template);
        let destination_type = field(&case, 3).to_string();
        let service = field(&case, 0).to_string();

        // "The action permissions that a caller needs to have to be able to successfully
        // create a delivery source on the desired resource type": an IAM action of the
        // resource's own service.
        let expected_action = format!("{service}:{VENDED_LOGS_DELIVERY_ACTION}");
        let action = string(template, "allowedActionForAllowVendedLogsDeliveryForResource", &case);
        assert_eq!(
            action, expected_action,
            "case {case}: expected the action {expected_action}, got {action}"
        );

        // The formats: every one the model names is allowed where the destination can store
        // it, a destination that stores one structured record each takes only `json`, and the
        // columnar file format applies only where the delivery writes objects.
        let formats = strings(template, "allowedOutputFormats", &case);
        assert!(
            !formats.is_empty() && formats.iter().all(|format| OUTPUT_FORMATS.contains(&format.as_str())),
            "case {case}: the allowed formats are a non-empty subset of {OUTPUT_FORMATS:?}, got {formats:?}"
        );
        let one_record_each = destination_type == "CWL" || destination_type == TRACE_DESTINATION_TYPE;
        if one_record_each {
            assert_eq!(
                formats,
                vec!["json".to_string()],
                "case {case}: a destination storing one structured record each allows json alone, got {formats:?}"
            );
        }
        assert_eq!(
            formats.contains(&"parquet".to_string()),
            destination_type == "S3",
            "case {case}: the columnar format applies exactly where the delivery writes objects, got {formats:?}"
        );

        // The delimiter: the delimiters a caller MAY choose apply exactly where a delimited
        // format is allowed, while the DEFAULT every template publishes is the same one — none
        // at all.
        let delimited = formats
            .iter()
            .any(|format| ["plain", "raw", "w3c"].contains(&format.as_str()));
        let delimiters = template
            .get("allowedFieldDelimiters")
            .map(|_| strings(template, "allowedFieldDelimiters", &case));
        assert_eq!(
            delimiters.is_some(),
            delimited,
            "case {case}: delimiters are published exactly where a delimited format is allowed, got {delimiters:?}"
        );
        let defaults = template.get("defaultDeliveryConfigValues").unwrap_or_else(|| {
            panic!("case {case}: a template carries defaultDeliveryConfigValues, got {template}")
        });
        let default_delimiter = defaults
            .get("fieldDelimiter")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        // The default delimiter is published exactly where the delimiters to choose from are,
        // and it is the one a delivery that names none is given: `FieldDelimiter` is
        // `@length 0..5`, so NO delimiter is a value in its own right.
        assert_eq!(
            default_delimiter.as_deref(),
            delimiters.as_ref().map(|_| DEFAULT_FIELD_DELIMITER),
            "case {case}: expected the default delimiter {DEFAULT_FIELD_DELIMITER:?} exactly \
             where delimiters are allowed, got {default_delimiter:?} and {delimiters:?}"
        );
        if let Some(allowed) = &delimiters {
            assert_eq!(
                allowed.as_slice(),
                FIELD_DELIMITERS
                    .iter()
                    .map(|delimiter| (*delimiter).to_string())
                    .collect::<Vec<String>>()
                    .as_slice(),
                "case {case}: expected the published delimiters {FIELD_DELIMITERS:?}, got {allowed:?}"
            );
            for delimiter in allowed {
                assert!(
                    delimiter.chars().count() <= MAX_FIELD_DELIMITER_CHARS,
                    "case {case}: a delimiter is at most {MAX_FIELD_DELIMITER_CHARS} characters, got {delimiter:?}"
                );
            }
        }

        // The S3 object layout: reported exactly for a delivery that writes objects, with
        // every variable of its default suffix path a field the same template allows there.
        let s3 = defaults.get("s3DeliveryConfiguration");
        let suffix_fields = template
            .get("allowedSuffixPathFields")
            .map(|_| strings(template, "allowedSuffixPathFields", &case));
        assert_eq!(
            s3.is_some(),
            destination_type == "S3",
            "case {case}: an S3 delivery configuration is reported exactly for an S3 destination, got {s3:?}"
        );
        assert_eq!(
            suffix_fields.is_some(),
            destination_type == "S3",
            "case {case}: suffix path fields are reported exactly for an S3 destination, got {suffix_fields:?}"
        );
        if let (Some(s3), Some(suffix_fields)) = (s3, suffix_fields) {
            assert_eq!(
                suffix_fields.first().map(String::as_str),
                Some(SUFFIX_PATH_ACCOUNT_FIELD),
                "case {case}: the published suffix path fields open with {SUFFIX_PATH_ACCOUNT_FIELD:?}, got {suffix_fields:?}"
            );
            assert!(
                suffix_fields.contains(&SUFFIX_PATH_REGION_FIELD.to_string()),
                "case {case}: the published suffix path fields name {SUFFIX_PATH_REGION_FIELD:?}, got {suffix_fields:?}"
            );
            assert!(
                suffix_fields.ends_with(
                    &SUFFIX_PATH_TIME_FIELDS
                        .iter()
                        .map(|field| (*field).to_string())
                        .collect::<Vec<String>>()
                ),
                "case {case}: the published suffix path fields close with {SUFFIX_PATH_TIME_FIELDS:?}, got {suffix_fields:?}"
            );
            let suffix_path = s3
                .get("suffixPath")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: the default S3 configuration carries a suffixPath, got {s3}"));
            // A default suffix path fits the published maximum; a sender that publishes no
            // object prefix of its own publishes the empty path, because a delivery of its logs
            // writes under the prefix its destination configures. The lower bound
            // `DeliverySuffixPath` states bounds what a CALLER may state, not the default a
            // sender publishes for partitioning nothing.
            assert!(
                suffix_path.chars().count() <= MAX_SUFFIX_PATH_CHARS,
                "case {case}: a suffix path is at most {MAX_SUFFIX_PATH_CHARS} characters, got {suffix_path:?}"
            );
            for variable in suffix_path_variables(suffix_path) {
                assert!(
                    suffix_fields.contains(&variable),
                    "case {case}: the default suffix path uses {variable:?}, which is not among {suffix_fields:?}"
                );
            }
            assert_eq!(
                s3.get("enableHiveCompatiblePath").and_then(Value::as_bool),
                Some(false),
                "case {case}: the default suffix path is not Hive-compatible, got {s3}"
            );
        }

        // The fields: a non-empty catalogue of what a delivery may carry, the default record
        // field list is what a delivery carries when it names none — those allowed fields in
        // order, less the ones that are allowed without being delivered by default — and a
        // mandatory field is one of them.
        let allowed_fields = template
            .get("allowedFields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: a template carries allowedFields, got {template}"));
        assert!(
            !allowed_fields.is_empty(),
            "case {case}: allowedFields is not empty, got {allowed_fields:?}"
        );
        let named = |field: &Value| {
            field
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: an allowed field carries a name, got {field}"))
                .to_string()
        };
        let names: Vec<String> = allowed_fields.iter().map(named).collect();
        for name in &names {
            assert!(
                (1..=MAX_FIELD_HEADER_CHARS).contains(&name.chars().count()),
                "case {case}: a field name is 1..{MAX_FIELD_HEADER_CHARS} characters, got {name:?}"
            );
        }
        let default_fields = strings(defaults, "recordFields", &case);
        let expected_defaults: Vec<String> = allowed_fields
            .iter()
            .filter(|field| {
                field.get("mandatory").and_then(Value::as_bool) == Some(true)
                    || !FIELDS_NOT_DELIVERED_BY_DEFAULT.contains(&named(field).as_str())
            })
            .map(named)
            .collect();
        assert_eq!(
            default_fields, expected_defaults,
            "case {case}: the default record fields are the allowed fields a delivery carries when it names none, got {default_fields:?}"
        );
        let mandatory: Vec<String> = allowed_fields
            .iter()
            .filter(|field| field.get("mandatory").and_then(Value::as_bool) == Some(true))
            .map(|field| {
                field
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !mandatory.is_empty() && mandatory.iter().all(|name| default_fields.contains(name)),
            "case {case}: the mandatory fields are a non-empty subset of {default_fields:?}, got {mandatory:?}"
        );
    }
}

/// The variables a suffix path names, each written `{name}`.
fn suffix_path_variables(suffix_path: &str) -> Vec<String> {
    suffix_path
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// A string member of a reported template.
fn string(template: &Value, member: &str, case: &str) -> String {
    template
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a template carries {member}, got {template}"))
        .to_string()
}

/// A list-of-strings member of a reported template.
fn strings(template: &Value, member: &str, case: &str) -> Vec<String> {
    template
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: a template carries {member}, got {template}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {member} holds strings, got {value}"))
                .to_string()
        })
        .collect()
}

/// The catalogue is server-managed: two calls report the same templates, and nothing a caller
/// puts in the account's store — a delivery source, a delivery destination — changes them.
#[tokio::test]
async fn the_catalogue_is_static_and_unchanged_by_the_accounts_resources() {
    let logs = Logs::new();
    let body = json!({"limit": MAX_LIMIT});

    let first = logs.ok(DESCRIBE, &body).await;
    let again = logs.ok(DESCRIBE, &body).await;
    assert_eq!(
        first, again,
        "case two calls: the same request reports the same templates, got {first} then {again}"
    );

    logs.ok(
        PUT_SOURCE,
        &json!({
            "name": "vended-source",
            "resourceArn": WORKMAIL_ARN,
            "logType": "ACCESS_CONTROL_LOGS",
        }),
    )
    .await;
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": "vended-destination",
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;

    let after = logs.ok(DESCRIBE, &body).await;
    assert_eq!(
        after, first,
        "case after a source and a destination are stored: the catalogue is unchanged, got {after}"
    );
}

/// A `nextToken` is a position within ONE filtered enumeration: a value this operation never
/// minted, a token another operation minted, and one continued under another filter are all
/// refused rather than answered from a different list.
#[tokio::test]
async fn a_token_is_a_position_within_one_filtered_enumeration() {
    let logs = Logs::new();

    let (_, token) = page(&logs, &json!({"limit": MIN_LIMIT})).await;
    let token = token.expect("a first page of one template out of many carries a nextToken");

    // Continuing the same enumeration is answered; continuing it under another filter is not.
    let (continued, _) = page(&logs, &json!({"limit": MIN_LIMIT, "nextToken": token})).await;
    assert_eq!(
        continued.len(),
        MIN_LIMIT,
        "case the same filter: the next page holds one template, got {continued:?}"
    );
    let (_, token) = page(&logs, &json!({"limit": MIN_LIMIT})).await;
    let token = token.expect("a first page of one template out of many carries a nextToken");

    // (case, the request the token is replayed in)
    let refused: [(&str, Value); 5] = [
        (
            "a token continued under another service filter",
            json!({"limit": MIN_LIMIT, "nextToken": token, "service": "workmail"}),
        ),
        (
            "a token continued under another log type filter",
            json!({"limit": MIN_LIMIT, "nextToken": token, "logTypes": ["ACCESS_LOGS"]}),
        ),
        (
            "a token continued under another destination type filter",
            json!({"limit": MIN_LIMIT, "nextToken": token, "deliveryDestinationTypes": ["S3"]}),
        ),
        (
            "a value this service never minted",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a token whose payload is this service's but not a token's",
            json!({"nextToken": "7b7d"}),
        ),
    ];
    for (case, body) in refused {
        let code = logs.err(DESCRIBE, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code}"
        );
    }

    // A token another paginated operation minted is not a position in this enumeration. One
    // (resource, log type) pairing is represented by one delivery source, so the two sources
    // this pages name log types the sending service both sends.
    for (name, log_type) in [
        ("source-one", "ACCESS_CONTROL_LOGS"),
        ("source-two", "AUTHENTICATION_LOGS"),
    ] {
        logs.ok(
            PUT_SOURCE,
            &json!({"name": name, "resourceArn": WORKMAIL_ARN, "logType": log_type}),
        )
        .await;
    }
    let sources = logs
        .ok(DESCRIBE_SOURCES, &json!({"limit": MIN_LIMIT}))
        .await;
    let foreign = sources
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one source out of two carries a nextToken")
        .to_string();
    let code = logs.err(DESCRIBE, &json!({"nextToken": foreign})).await;
    assert_eq!(
        code, VALIDATION,
        "case a token another operation minted: expected {VALIDATION}, got {code}"
    );
}

/// The published bounds of the request's own members hold: `limit` is accepted at each bound
/// and refused one past it, and a filter list is refused when it is empty or longer than the
/// model admits.
#[tokio::test]
async fn the_published_request_bounds_are_accepted_and_one_past_each_is_refused() {
    let logs = Logs::new();

    for limit in [MIN_LIMIT, MAX_LIMIT] {
        let (page, _) = page(&logs, &json!({"limit": limit})).await;
        assert_eq!(
            page.len(),
            limit.min(catalogue().len()),
            "case limit {limit}: a page holds that many templates, got {}",
            page.len()
        );
    }

    // (case, the request outside a published bound)
    let refused: [(&str, Value); 6] = [
        ("limit below its range", json!({"limit": MIN_LIMIT - 1})),
        ("limit above its range", json!({"limit": MAX_LIMIT + 1})),
        ("an empty logTypes list", json!({"logTypes": []})),
        (
            "a logTypes list past its length",
            json!({"logTypes": ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K"]}),
        ),
        ("an empty resourceTypes list", json!({"resourceTypes": []})),
        (
            "a deliveryDestinationTypes list past its length",
            json!({"deliveryDestinationTypes": ["CWL", "FH", "S3", "XRAY", "CWL"]}),
        ),
    ];
    for (case, body) in refused {
        let code = logs.err(DESCRIBE, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code}"
        );
    }
}
