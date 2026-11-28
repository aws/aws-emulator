// SPDX-License-Identifier: CC0-1.0

//! What `UpdateEventDestination` leaves on a configuration set's event destination, end to
//! end over the wire.
//!
//! Every request goes through [`MultiServiceRouter::dispatch`] as `awsJson1_0` — `POST /`
//! with `X-Amz-Target: PinpointSMSVoiceV2.<Op>` — so these tests exercise routing,
//! deserialization, constraint validation, the handler and serialization, exactly as a real
//! SDK client would reach them.
//!
//! The rule under test: an update writes every member the request carries onto the stored
//! event-destination record and leaves every member the request omits exactly as it was, so
//! the destination's state is what the last request naming each member set — and that is
//! what EVERY later read of it reports, the record `DeleteEventDestination` echoes included.
//! The rows below drive each updatable member (`Enabled`, `MatchingEventTypes` and the one
//! destination member the record holds), one and several at a time, over each of the three
//! destination kinds the model declares.
//!
//! Its harness is this suite's own: the sibling suites' helpers are private to their
//! modules, and a test target shares nothing it does not export.

use std::sync::Arc;

use service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2 as sms_voice;
use sms_voice::ops::types::EVENT_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for, so the ARNs under assertion are not the default one.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT_ID: &str = emulator_server::EMULATOR_ACCOUNT_ID;

/// `MatchingEventTypes` is modelled `EventTypeList` with `length {min 1, max 52}`
/// (`smithy_query(query="operation",operation="UpdateEventDestination")`).
const MIN_MATCHING_EVENT_TYPES: usize = 1;
const MAX_MATCHING_EVENT_TYPES: usize = 52;

/// One emulator: the shared engine plus a router with only this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(sms_voice::EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and answer `(http status, parsed JSON body)`.
    async fn call(&self, operation: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sms-voice/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require HTTP 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: serde_json::Value) -> serde_json::Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create one configuration set under the given name.
    async fn create_configuration_set(&self, name: &str) {
        self.ok(
            "CreateConfigurationSet",
            serde_json::json!({"ConfigurationSetName": name}),
        )
        .await;
    }

    /// Create one event destination on a stored set and answer the record the create
    /// reports, which is the state every row below starts from.
    async fn create_destination(
        &self,
        set: &str,
        name: &str,
        event_types: &[&str],
        destination: &serde_json::Value,
    ) -> serde_json::Value {
        let mut request = serde_json::json!({
            "ConfigurationSetName": set,
            "EventDestinationName": name,
            "MatchingEventTypes": event_types,
        });
        merge_into(&mut request, destination);
        let created = self.ok("CreateEventDestination", request).await;
        created
            .get("EventDestination")
            .cloned()
            .unwrap_or_else(|| panic!("expected an EventDestination in {created}"))
    }

    /// The destination one stored set holds under `name`, as the whole-account describe
    /// reports it.
    async fn stored_destination(&self, set: &str, name: &str) -> serde_json::Value {
        let described = self
            .ok("DescribeConfigurationSets", serde_json::json!({}))
            .await;
        let sets = described
            .get("ConfigurationSets")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("expected a ConfigurationSets list in {described}"));
        let entry = sets
            .iter()
            .find(|entry| member(entry, "ConfigurationSetName") == set)
            .unwrap_or_else(|| panic!("expected a stored configuration set named {set:?}"));
        entry
            .get("EventDestinations")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("expected an EventDestinations list in {entry}"))
            .iter()
            .find(|destination| member(destination, "EventDestinationName") == name)
            .cloned()
            .unwrap_or_else(|| panic!("expected a stored destination named {name:?} in {entry}"))
    }
}

/// A string member of a response body.
fn member(body: &serde_json::Value, name: &str) -> String {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {name} in {body}"))
        .to_string()
}

/// The wire error code of a failed call (`__type`, with any namespace prefix dropped).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// `Fields` as `(Name, Message)` pairs, in the order the body lists them.
fn fields(body: &serde_json::Value) -> Vec<(String, String)> {
    body.get("Fields")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("expected a Fields list in {body}"))
        .iter()
        .map(|field| (member(field, "Name"), member(field, "Message")))
        .collect()
}

/// The ARN one stored configuration set carries, as `CreateConfigurationSet` builds it from
/// the signing region and the emulator's account id.
fn configuration_set_arn(name: &str) -> String {
    format!("arn:aws:sms-voice:{REGION}:{ACCOUNT_ID}:configuration-set/{name}")
}

/// Copy every member of `patch` onto `target`, which is how a request carries the members a
/// row names and how the record those members are expected to leave is built.
fn merge_into(target: &mut serde_json::Value, patch: &serde_json::Value) {
    let object = target
        .as_object_mut()
        .expect("expected an object to merge into");
    for (name, value) in patch
        .as_object()
        .unwrap_or_else(|| panic!("expected an object to merge, got {patch}"))
    {
        object.insert(name.clone(), value.clone());
    }
}

/// An `UpdateEventDestination` request for one destination, carrying the members `patch`
/// names and nothing else.
fn update_request(set: &str, name: &str, patch: &serde_json::Value) -> serde_json::Value {
    let mut request = serde_json::json!({
        "ConfigurationSetName": set,
        "EventDestinationName": name,
    });
    merge_into(&mut request, patch);
    request
}

/// Event types taken from the model's own `EventType` catalog, re-exported as
/// [`EVENT_TYPE_VALUES`] (`smithy_query(query="shape",shape="EventType")`), so no row
/// restates a list of its own: `count` of them, starting `skip` in.
fn event_types(skip: usize, count: usize) -> Vec<&'static str> {
    let chosen: Vec<&'static str> = EVENT_TYPE_VALUES.iter().copied().skip(skip).take(count).collect();
    assert_eq!(
        chosen.len(),
        count,
        "the model's EventType catalog must hold at least {} values to take {count} from {skip} \
         in, got {}",
        skip + count,
        EVENT_TYPE_VALUES.len()
    );
    chosen
}

/// One destination kind: the member that names it, the value a create stores, and the value
/// an update writes over it.
struct Kind {
    member: &'static str,
    created: serde_json::Value,
    replacement: serde_json::Value,
}

/// The three destination kinds `EventDestination` declares
/// (`smithy_query(query="shape",shape="EventDestination")`), each with a second value of
/// the same kind for an update to write.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            member: "CloudWatchLogsDestination",
            created: serde_json::json!({"CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::111122223333:role/sms-voice-events",
                "LogGroupArn": "arn:aws:logs:eu-west-1:111122223333:log-group:/aws/sms-voice/first",
            }}),
            replacement: serde_json::json!({"CloudWatchLogsDestination": {
                "IamRoleArn": "arn:aws:iam::111122223333:role/sms-voice-events-rotated",
                "LogGroupArn": "arn:aws:logs:eu-west-1:111122223333:log-group:/aws/sms-voice/second",
            }}),
        },
        Kind {
            member: "KinesisFirehoseDestination",
            created: serde_json::json!({"KinesisFirehoseDestination": {
                "IamRoleArn": "arn:aws:iam::111122223333:role/sms-voice-events",
                "DeliveryStreamArn": "arn:aws:firehose:eu-west-1:111122223333:deliverystream/first",
            }}),
            replacement: serde_json::json!({"KinesisFirehoseDestination": {
                "IamRoleArn": "arn:aws:iam::111122223333:role/sms-voice-events-rotated",
                "DeliveryStreamArn": "arn:aws:firehose:eu-west-1:111122223333:deliverystream/second",
            }}),
        },
        Kind {
            member: "SnsDestination",
            created: serde_json::json!({"SnsDestination": {
                "TopicArn": "arn:aws:sns:eu-west-1:111122223333:sms-voice-first",
            }}),
            replacement: serde_json::json!({"SnsDestination": {
                "TopicArn": "arn:aws:sns:eu-west-1:111122223333:sms-voice-second",
            }}),
        },
    ]
}

/// Each row of the table is one update request, named by the members it carries; the record
/// it must leave is the previous record with exactly those members replaced.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest_as_they_were() {
    for kind in kinds() {
        let kind_name = kind.member;
        let emulator = Emulator::new();
        let set = "update-probe-set";
        let destination_name = "primary";
        emulator.create_configuration_set(set).await;
        let created = emulator
            .create_destination(set, destination_name, &event_types(6, 1), &kind.created)
            .await;

        let rows: Vec<(&str, serde_json::Value)> = vec![
            ("logging turned off", serde_json::json!({"Enabled": false})),
            ("logging turned on again", serde_json::json!({"Enabled": true})),
            (
                "the event type list replaced by three types",
                serde_json::json!({"MatchingEventTypes": event_types(20, 3)}),
            ),
            (
                "the event type list narrowed to the declared minimum",
                serde_json::json!({"MatchingEventTypes": event_types(0, MIN_MATCHING_EVENT_TYPES)}),
            ),
            (
                "the event type list grown to the declared maximum",
                serde_json::json!({"MatchingEventTypes": event_types(0, MAX_MATCHING_EVENT_TYPES)}),
            ),
            ("the destination's own configuration replaced", kind.replacement.clone()),
            (
                "logging turned off and the event types replaced in one call",
                serde_json::json!({
                    "Enabled": false,
                    "MatchingEventTypes": event_types(43, 2),
                }),
            ),
            ("no updatable member at all", serde_json::json!({})),
            (
                "the destination's own configuration and both other members together",
                {
                    let mut patch = serde_json::json!({
                        "Enabled": true,
                        "MatchingEventTypes": event_types(31, 4),
                    });
                    merge_into(&mut patch, &kind.created);
                    patch
                },
            ),
        ];

        let mut expected = created;
        for (case, patch) in rows {
            let answered = emulator
                .ok(
                    "UpdateEventDestination",
                    update_request(set, destination_name, &patch),
                )
                .await;
            merge_into(&mut expected, &patch);

            assert_eq!(
                answered.get("EventDestination"),
                Some(&expected),
                "case {kind_name}/{case:?}: expected the record the update left, got {answered}"
            );
            assert_eq!(
                (
                    member(&answered, "ConfigurationSetName"),
                    member(&answered, "ConfigurationSetArn"),
                ),
                (set.to_string(), configuration_set_arn(set)),
                "case {kind_name}/{case:?}: the update reports the set it changed, got {answered}"
            );

            // The round trip: a later read of the same destination reports exactly what the
            // update persisted, so the response is not a one-off rendering of the request.
            let stored = emulator.stored_destination(set, destination_name).await;
            assert_eq!(
                stored, expected,
                "case {kind_name}/{case:?}: a later read must report the persisted record, got \
                 {stored}"
            );
        }

        // The record the delete echoes is the destination as it stood at deletion, which is
        // the state the last row left.
        let deleted = emulator
            .ok(
                "DeleteEventDestination",
                serde_json::json!({
                    "ConfigurationSetName": set,
                    "EventDestinationName": destination_name,
                }),
            )
            .await;
        assert_eq!(
            deleted.get("EventDestination"),
            Some(&expected),
            "case {kind_name}: the delete echoes the record the updates left, got {deleted}"
        );
    }
}

/// One row of the enablement table: how the destination was driven before it was deleted,
/// and the `Enabled` the delete must echo.
struct Enablement {
    case: &'static str,
    /// The `Enabled` each update in turn sets, in order; an empty list is a destination no
    /// update ever touched.
    updates: Vec<bool>,
    expected: bool,
}

/// `DeleteEventDestination` echoes "the event destination object that was deleted", so the
/// enablement it reports is the one the last update set — and the one a created destination
/// starts with when no update ever set it.
#[tokio::test]
async fn the_delete_echoes_the_enablement_the_last_update_set() {
    let rows = [
        Enablement {
            case: "a destination no update ever touched",
            updates: vec![],
            expected: true,
        },
        Enablement {
            case: "a destination disabled before deletion",
            updates: vec![false],
            expected: false,
        },
        Enablement {
            case: "a destination disabled and enabled again",
            updates: vec![false, true],
            expected: true,
        },
        Enablement {
            case: "a destination disabled twice",
            updates: vec![false, false],
            expected: false,
        },
        Enablement {
            case: "a destination enabled and then disabled",
            updates: vec![true, false],
            expected: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let set = "enablement-probe-set";
        let destination_name = "logged";
        emulator.create_configuration_set(set).await;
        emulator
            .create_destination(
                set,
                destination_name,
                &event_types(6, 1),
                &kinds()[2].created,
            )
            .await;

        for enabled in &row.updates {
            let answered = emulator
                .ok(
                    "UpdateEventDestination",
                    update_request(
                        set,
                        destination_name,
                        &serde_json::json!({"Enabled": enabled}),
                    ),
                )
                .await;
            assert_eq!(
                answered.pointer("/EventDestination/Enabled"),
                Some(&serde_json::Value::Bool(*enabled)),
                "case {case:?}: the update reports the enablement it wrote, got {answered}"
            );
        }

        let stored = emulator.stored_destination(set, destination_name).await;
        assert_eq!(
            stored.get("Enabled"),
            Some(&serde_json::Value::Bool(row.expected)),
            "case {case:?}: a later read reports the persisted enablement, got {stored}"
        );
        let deleted = emulator
            .ok(
                "DeleteEventDestination",
                serde_json::json!({
                    "ConfigurationSetName": set,
                    "EventDestinationName": destination_name,
                }),
            )
            .await;
        assert_eq!(
            deleted.pointer("/EventDestination/Enabled"),
            Some(&serde_json::Value::Bool(row.expected)),
            "case {case:?}: the delete echoes the enablement at deletion, got {deleted}"
        );
    }
}

/// An update that names a resource the account does not hold is a not-found, and one that
/// cannot be applied leaves the stored record exactly as it was.
#[tokio::test]
async fn an_update_of_a_resource_that_is_not_there_is_refused() {
    let emulator = Emulator::new();
    let set = "refusal-probe-set";
    let destination_name = "primary";
    emulator.create_configuration_set(set).await;
    let created = emulator
        .create_destination(set, destination_name, &event_types(6, 1), &kinds()[2].created)
        .await;

    let rows: Vec<(&str, serde_json::Value, &str, &str)> = vec![
        (
            "a set the account does not hold",
            update_request(
                "refusal-probe-never-created",
                destination_name,
                &serde_json::json!({"Enabled": false}),
            ),
            "configuration-set",
            "refusal-probe-never-created",
        ),
        (
            "a destination the stored set does not hold",
            update_request(set, "never-created", &serde_json::json!({"Enabled": false})),
            "event-destination",
            "never-created",
        ),
    ];

    for (case, request, resource_type, resource_id) in rows {
        let (status, body) = emulator.call("UpdateEventDestination", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} with {body}"
        );
        assert_eq!(
            (
                member(&body, "Message"),
                member(&body, "ResourceType"),
                member(&body, "ResourceId"),
            ),
            (
                format!(
                    "Resource Not Found - ResourceType=\"{resource_type}\" \
                     ResourceId=\"{resource_id}\""
                ),
                resource_type.to_string(),
                resource_id.to_string(),
            ),
            "case {case:?}: the refusal names the resource it could not find, got {body}"
        );
        let stored = emulator.stored_destination(set, destination_name).await;
        assert_eq!(
            stored, created,
            "case {case:?}: a refused update changes nothing, got {stored}"
        );
    }
}

/// "You can't change an event destination's type between CloudWatch Logs, Firehose, and
/// Amazon SNS": a request naming another kind than the stored record holds is a conflict,
/// and one naming two kinds does not say where events go.
#[tokio::test]
async fn an_update_cannot_change_the_kind_of_destination_it_names() {
    let all = kinds();
    for (index, kind) in all.iter().enumerate() {
        let kind_name = kind.member;
        let other = &all[(index + 1) % all.len()];
        let emulator = Emulator::new();
        let set = "kind-probe-set";
        let destination_name = "primary";
        emulator.create_configuration_set(set).await;
        let created = emulator
            .create_destination(set, destination_name, &event_types(6, 1), &kind.created)
            .await;

        // Another kind alone: the record's kind cannot change.
        let (status, body) = emulator
            .call(
                "UpdateEventDestination",
                update_request(set, destination_name, &other.created),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ConflictException"),
            "case {kind_name}: naming {} on a {kind_name} destination conflicts, got {status} \
             with {body}",
            other.member
        );
        assert_eq!(
            (
                member(&body, "Message"),
                member(&body, "Reason"),
                member(&body, "ResourceType"),
                member(&body, "ResourceId"),
            ),
            (
                format!(
                    "Conflict Occurred - Reason=\"EVENT_DESTINATION_MISMATCH\" \
                     ResourceType=\"event-destination\" ResourceId=\"{destination_name}\""
                ),
                "EVENT_DESTINATION_MISMATCH".to_string(),
                "event-destination".to_string(),
                destination_name.to_string(),
            ),
            "case {kind_name}: the conflict restates its own members, got {body}"
        );

        // Two kinds at once: refused as invalid, naming both members.
        let mut both = kind.replacement.clone();
        merge_into(&mut both, &other.created);
        let (status, body) = emulator
            .call(
                "UpdateEventDestination",
                update_request(set, destination_name, &both),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {kind_name}: naming two kinds is invalid, got {status} with {body}"
        );
        assert_eq!(
            member(&body, "Reason"),
            "PARAMETERS_CANNOT_BE_USED_TOGETHER",
            "case {kind_name}: got {body}"
        );
        let named: Vec<String> = fields(&body).into_iter().map(|(name, _)| name).collect();
        let mut expected_named = vec![kind.member.to_string(), other.member.to_string()];
        expected_named.sort();
        let mut reported = named.clone();
        reported.sort();
        assert_eq!(
            reported, expected_named,
            "case {kind_name}: the refusal names both destination members, got {body}"
        );

        let stored = emulator.stored_destination(set, destination_name).await;
        assert_eq!(
            stored, created,
            "case {kind_name}: a refused update changes nothing, got {stored}"
        );
    }
}

/// `MatchingEventTypes` is served on each published bound of its declared length and
/// refused one past either of them, in this service's modelled refusal shape.
#[tokio::test]
async fn the_event_type_list_is_bounded_by_its_declared_length() {
    let emulator = Emulator::new();
    let set = "bound-probe-set";
    let destination_name = "primary";
    emulator.create_configuration_set(set).await;
    let created = emulator
        .create_destination(set, destination_name, &event_types(6, 1), &kinds()[2].created)
        .await;

    for count in [MIN_MATCHING_EVENT_TYPES, MAX_MATCHING_EVENT_TYPES] {
        let answered = emulator
            .ok(
                "UpdateEventDestination",
                update_request(
                    set,
                    destination_name,
                    &serde_json::json!({"MatchingEventTypes": event_types(0, count)}),
                ),
            )
            .await;
        assert_eq!(
            answered.pointer("/EventDestination/MatchingEventTypes"),
            Some(&serde_json::json!(event_types(0, count))),
            "case {count} event types: a bound of the declared length is served, got {answered}"
        );
    }

    // One past each bound: an empty list, and one entry more than the maximum (the catalog
    // is exactly as long as the maximum, so the row past it repeats its first value).
    let mut past_maximum: Vec<&str> = event_types(0, MAX_MATCHING_EVENT_TYPES);
    past_maximum.push(EVENT_TYPE_VALUES[0]);
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "below the declared minimum",
            Vec::new(),
            "Member must have length greater than or equal to 1",
        ),
        (
            "above the declared maximum",
            past_maximum,
            "Member must have length less than or equal to 52",
        ),
    ];
    for (case, event_types, requirement) in rows {
        let (status, body) = emulator
            .call(
                "UpdateEventDestination",
                update_request(
                    set,
                    destination_name,
                    &serde_json::json!({"MatchingEventTypes": event_types}),
                ),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled refusal, got {status} with {body}"
        );
        assert_eq!(
            (member(&body, "Message"), member(&body, "Reason")),
            (
                "The following parameter is null, empty or invalid: ['matchingEventTypes']"
                    .to_string(),
                "FIELD_VALIDATION_FAILED".to_string(),
            ),
            "case {case:?}: got {body}"
        );
        assert_eq!(
            fields(&body),
            vec![("matchingEventTypes".to_string(), requirement.to_string())],
            "case {case:?}: got {body}"
        );
        let stored = emulator.stored_destination(set, destination_name).await;
        assert_eq!(
            stored.get("MatchingEventTypes"),
            Some(&serde_json::json!(event_types_at_last_served())),
            "case {case:?}: a refused update changes nothing, got {stored}"
        );
        assert_eq!(
            stored.get("SnsDestination"),
            created.get("SnsDestination"),
            "case {case:?}: a refused update leaves the destination itself alone, got {stored}"
        );
    }
}

/// The event type list the last served row of
/// [`the_event_type_list_is_bounded_by_its_declared_length`] left on the record: the
/// declared maximum.
fn event_types_at_last_served() -> Vec<&'static str> {
    event_types(0, MAX_MATCHING_EVENT_TYPES)
}
