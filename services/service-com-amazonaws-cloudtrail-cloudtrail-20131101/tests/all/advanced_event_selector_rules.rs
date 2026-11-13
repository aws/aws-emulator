// SPDX-License-Identifier: CC0-1.0

//! The rules an ADVANCED event selector set is held to, over every operation that stores
//! one, and the sentence each refusal carries.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the error
//! code and the message.
//!
//! The rules under test, each refused with `InvalidEventSelectorsException` (HTTP 400)
//! and the fixed sentence CloudTrail words that error class in:
//!
//! * every field selector states at least one condition on its field;
//! * every selector states the CATEGORY of events it selects, on `eventCategory`;
//! * that category is ONE value;
//! * every other field it selects on is a field that category admits — which for an
//!   event data store is the trail's catalogue plus the fields the API Reference page
//!   for `AdvancedFieldSelector` calls "additional fields … available for event data
//!   stores";
//! * a `resources.type` condition names a resource type data events are logged for.
//!
//! A refusal applies NOTHING: the selector set the resource already carried is the set
//! every later read reports. A set the rules admit is stored and read back verbatim.
//!
//! The expected sentences are written out here rather than read from the service, so the
//! wording is asserted against CloudTrail's recorded refusals instead of against itself.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::advanced_event_selectors::{
    ADVANCED_DATA_RESOURCE_TYPES, DATA_EVENT_CATEGORY, MANAGEMENT_EVENT_CATEGORY,
    NETWORK_ACTIVITY_EVENT_CATEGORY,
};

use crate::emulator::{Emulator, Reply};

/// The status CloudTrail answers its client errors with.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error the model declares for a selector set the service will not install
/// (`smithy_query(query="operation", operation="PutEventSelectors")`).
const INVALID_EVENT_SELECTORS: &str = "InvalidEventSelectorsException";

/// How CloudTrail words a selector that states no `eventCategory`: entirely fixed text,
/// whose bracketed name is the MISSING member and not a value of the request.
const MISSING_EVENT_CATEGORY: &str = "The following fields are required: [eventCategory].";

/// How CloudTrail words more than one value under `eventCategory`: entirely fixed text,
/// naming the field and the one value it may carry.
const TOO_MANY_EVENT_CATEGORIES: &str = "There are too many values specified for eventCategory. \
     The eventCategory field can have a maximum of 1 values.";

/// How CloudTrail words a `resources.type` value outside the resource types data events
/// are logged for: entirely fixed text.
const RESOURCES_TYPE_NOT_VALID: &str = "The resources.type field value is not valid.";

/// How CloudTrail words a field the selector's event category does not admit: fixed text
/// with the category and the field copied from the request.
fn field_not_allowed(category: &str, field: &str) -> String {
    format!(
        "The following field is not allowed when the eventCategory field value equals \
         {category}: {field}."
    )
}

/// How CloudTrail words a field selector carrying no condition at all: fixed text with
/// the field copied from the request, bare.
fn empty_selector(field: &str) -> String {
    format!("One or more of the selectors for the {field} field is empty.")
}

/// A field selector on `field` under `Equals` of `values`.
fn equals(field: &str, values: &[&str]) -> Value {
    json!({"Field": field, "Equals": values})
}

/// One named selector over `field_selectors`.
fn selector(name: &str, field_selectors: Vec<Value>) -> Value {
    json!([{"Name": name, "FieldSelectors": field_selectors}])
}

/// The baseline every trail case starts from: the management-event selector a client
/// installs first, carrying a condition under a negating operator as well as `Equals`.
fn baseline_selectors() -> Value {
    selector(
        "management-events",
        vec![
            equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
            json!({
                "Field": "eventSource",
                "NotEquals": ["kms.amazonaws.com", "rdsdata.amazonaws.com"],
            }),
            equals("readOnly", &["false"]),
        ],
    )
}

/// The field every advanced event selector states its category on.
const CATEGORY_FIELD: &str = "eventCategory";

/// A trail named `trail` carrying [`baseline_selectors`], applied through the API so what
/// a later read reports is what the service stored.
async fn trail_with_baseline(emulator: &Emulator, trail: &str) {
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": format!("{trail}-bucket")}),
        )
        .await
        .expect_ok("CreateTrail");
    let applied = emulator
        .call(
            "PutEventSelectors",
            &json!({"TrailName": trail, "AdvancedEventSelectors": baseline_selectors()}),
        )
        .await;
    let applied = applied.expect_ok("PutEventSelectors: the baseline").clone();
    assert_eq!(
        applied.get("AdvancedEventSelectors"),
        Some(&baseline_selectors()),
        "case the baseline: expected the applied set echoed back, got {applied:?}"
    );
    assert_eq!(
        applied.get("EventSelectors"),
        None,
        "case the baseline: an advanced set replaces the basic one, got {applied:?}"
    );
}

/// What `GetEventSelectors` reports for `trail` as the pair
/// (`EventSelectors`, `AdvancedEventSelectors`).
async fn read_selectors(emulator: &Emulator, trail: &str, case: &str) -> (Value, Value) {
    let reply = emulator
        .call("GetEventSelectors", &json!({"TrailName": trail}))
        .await;
    let body = reply
        .expect_ok(&format!("GetEventSelectors after {case}"))
        .clone();
    (
        body.get("EventSelectors").cloned().unwrap_or(Value::Null),
        body.get("AdvancedEventSelectors")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

/// RULE: a selector set outside the documented rules is refused with
/// `InvalidEventSelectorsException` (HTTP 400) and the fixed sentence for the rule it
/// broke, and the trail keeps the set it already carried.
///
/// The rows differ along WHICH rule is broken and along the shape of the breach: the
/// field that is missing, the category the offending field is illegal for, which field
/// carries no condition, how many values the category carries, and the form of the
/// unsupported `resources.type` value.
#[tokio::test]
async fn an_advanced_selector_set_outside_the_documented_rules_is_refused() {
    let emulator = Emulator::new();
    let trail = "advanced-refusals";
    trail_with_baseline(&emulator, trail).await;
    // A real resource type written in another case, held outside the table so the row
    // borrows it rather than a temporary.
    let mis_cased = ADVANCED_DATA_RESOURCE_TYPES[0].to_lowercase();

    // (case, the AdvancedEventSelectors the request carries, the message the refusal
    //  carries)
    let rows: Vec<(String, Value, String)> = vec![
        (
            "a selector whose only field selector is readOnly".to_string(),
            selector("no-category", vec![equals("readOnly", &["false"])]),
            MISSING_EVENT_CATEGORY.to_string(),
        ),
        (
            "a selector naming resources but no category".to_string(),
            selector(
                "resources-only",
                vec![
                    equals("resources.type", &[ADVANCED_DATA_RESOURCE_TYPES[0]]),
                    json!({"Field": "resources.ARN", "StartsWith": ["arn:aws:s3:::b/"]}),
                ],
            ),
            MISSING_EVENT_CATEGORY.to_string(),
        ),
        (
            "the second of two selectors states no category".to_string(),
            json!([
                {
                    "Name": "complete",
                    "FieldSelectors": [equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY])],
                },
                {
                    "Name": "incomplete",
                    "FieldSelectors": [json!({
                        "Field": "eventSource",
                        "NotEquals": ["kms.amazonaws.com"],
                    })],
                },
            ]),
            MISSING_EVENT_CATEGORY.to_string(),
        ),
        (
            "a field name that is no selector field, under Management".to_string(),
            selector(
                "unknown-field",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    equals("notARealSelectorField", &["anything"]),
                ],
            ),
            field_not_allowed(MANAGEMENT_EVENT_CATEGORY, "notARealSelectorField"),
        ),
        (
            "a data-event field under Management".to_string(),
            selector(
                "event-name-under-management",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    equals("eventName", &["PutEventSelectors"]),
                ],
            ),
            field_not_allowed(MANAGEMENT_EVENT_CATEGORY, "eventName"),
        ),
        (
            "the resource-type field under Management".to_string(),
            selector(
                "resources-type-under-management",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    equals("resources.type", &[ADVANCED_DATA_RESOURCE_TYPES[0]]),
                ],
            ),
            field_not_allowed(MANAGEMENT_EVENT_CATEGORY, "resources.type"),
        ),
        (
            "a network-activity field under Data".to_string(),
            selector(
                "vpc-endpoint-under-data",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("vpcEndpointId", &["vpce-00000000000000001"]),
                ],
            ),
            field_not_allowed(DATA_EVENT_CATEGORY, "vpcEndpointId"),
        ),
        (
            "a field name that is no selector field, under NetworkActivity".to_string(),
            selector(
                "unknown-field-under-network-activity",
                vec![
                    equals(CATEGORY_FIELD, &[NETWORK_ACTIVITY_EVENT_CATEGORY]),
                    equals("notARealSelectorField", &["anything"]),
                ],
            ),
            field_not_allowed(NETWORK_ACTIVITY_EVENT_CATEGORY, "notARealSelectorField"),
        ),
        (
            "a field selector carrying no operator at all".to_string(),
            selector(
                "operator-less",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    json!({"Field": "readOnly"}),
                ],
            ),
            empty_selector("readOnly"),
        ),
        (
            "the category field itself carrying no operator".to_string(),
            selector("category-without-operator", vec![json!({"Field": CATEGORY_FIELD})]),
            empty_selector(CATEGORY_FIELD),
        ),
        (
            "a resource field carrying no operator beside complete ones".to_string(),
            selector(
                "resources-arn-without-operator",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &[ADVANCED_DATA_RESOURCE_TYPES[0]]),
                    json!({"Field": "resources.ARN"}),
                ],
            ),
            empty_selector("resources.ARN"),
        ),
        (
            "two values under the category field".to_string(),
            selector(
                "two-categories",
                vec![equals(
                    CATEGORY_FIELD,
                    &[MANAGEMENT_EVENT_CATEGORY, DATA_EVENT_CATEGORY],
                )],
            ),
            TOO_MANY_EVENT_CATEGORIES.to_string(),
        ),
        (
            "three values under the category field".to_string(),
            selector(
                "three-categories",
                vec![equals(
                    CATEGORY_FIELD,
                    &[
                        MANAGEMENT_EVENT_CATEGORY,
                        DATA_EVENT_CATEGORY,
                        NETWORK_ACTIVITY_EVENT_CATEGORY,
                    ],
                )],
            ),
            TOO_MANY_EVENT_CATEGORIES.to_string(),
        ),
        (
            "one category spread over two field selectors".to_string(),
            selector(
                "category-twice",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                ],
            ),
            TOO_MANY_EVENT_CATEGORIES.to_string(),
        ),
        (
            "a resource type nothing logs data events for".to_string(),
            selector(
                "unsupported-resource-type",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &["AWS::Sim2Real::Nonexistent"]),
                ],
            ),
            RESOURCES_TYPE_NOT_VALID.to_string(),
        ),
        (
            // A real resource type written in another case is a different value, and
            // the catalogue is matched exactly.
            "a resource type differing from a real one in case".to_string(),
            selector(
                "mis-cased-resource-type",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &[mis_cased.as_str()]),
                ],
            ),
            RESOURCES_TYPE_NOT_VALID.to_string(),
        ),
        (
            // A resource type a BASIC data resource names, spelt as a CloudFormation
            // type that CloudTrail logs no data events for.
            "a resource type of a service with no data events".to_string(),
            selector(
                "no-data-events-for-type",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &["AWS::EC2::Instance"]),
                ],
            ),
            RESOURCES_TYPE_NOT_VALID.to_string(),
        ),
    ];

    for (case, selectors, message) in rows {
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail, "AdvancedEventSelectors": selectors}),
            )
            .await;
        reply.expect_error(&case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
        assert_eq!(
            reply.message(),
            message,
            "case {case:?}: expected the fixed sentence {message:?}, got {:?}",
            reply.body
        );

        let read = read_selectors(&emulator, trail, &case).await;
        assert_eq!(
            read,
            (Value::Null, baseline_selectors()),
            "case {case:?}: a refused request must leave the applied selectors \
             untouched, got {read:?}"
        );
    }
}

/// RULE: a selector set the documented rules admit is applied and read back verbatim.
///
/// The rows differ along the CATEGORY selected, which fields that category admits, the
/// operators the conditions are stated under, and the number of selectors in the set;
/// every resource type the catalogue carries is then applied in its own row, so no entry
/// of it is left untested.
#[tokio::test]
async fn an_advanced_selector_set_the_rules_admit_is_applied_and_read_back() {
    let emulator = Emulator::new();
    let trail = "advanced-accepted";
    trail_with_baseline(&emulator, trail).await;
    // The LAST entry of the catalogue, so a row carries a type the issue never cited.
    let last_resource_type =
        ADVANCED_DATA_RESOURCE_TYPES[ADVANCED_DATA_RESOURCE_TYPES.len() - 1];

    // (case, the AdvancedEventSelectors the request carries)
    let mut rows: Vec<(String, Value)> = vec![
        (
            "the management-event fields, under Equals and NotEquals".to_string(),
            baseline_selectors(),
        ),
        (
            "a selector stating nothing but its category".to_string(),
            selector(
                "category-only",
                vec![equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY])],
            ),
        ),
        (
            "every field a data-event selector admits".to_string(),
            selector(
                "data-events",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &[ADVANCED_DATA_RESOURCE_TYPES[0]]),
                    json!({"Field": "resources.ARN", "StartsWith": ["arn:aws:s3:::b/"]}),
                    equals("readOnly", &["true"]),
                    json!({"Field": "eventName", "NotStartsWith": ["Delete"]}),
                ],
            ),
        ),
        (
            "every field a network-activity selector admits".to_string(),
            selector(
                "network-activity",
                vec![
                    equals(CATEGORY_FIELD, &[NETWORK_ACTIVITY_EVENT_CATEGORY]),
                    equals("eventSource", &["s3.amazonaws.com"]),
                    json!({"Field": "eventName", "EndsWith": ["Object"]}),
                    equals("errorCode", &["VpceAccessDenied"]),
                    json!({"Field": "vpcEndpointId", "StartsWith": ["vpce-"]}),
                ],
            ),
        ),
        (
            "two selectors of different categories at once".to_string(),
            json!([
                {
                    "Name": "management",
                    "FieldSelectors": [equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY])],
                },
                {
                    "Name": "data",
                    "FieldSelectors": [
                        equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                        equals("resources.type", &[last_resource_type]),
                    ],
                },
            ]),
        ),
    ];
    // One row per resource type the catalogue carries, so a type the issue never cited
    // is applied as well as the ones it did.
    for resource_type in ADVANCED_DATA_RESOURCE_TYPES {
        rows.push((
            format!("data events for {resource_type}"),
            selector(
                "one-resource-type",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &[resource_type]),
                ],
            ),
        ));
    }

    for (case, selectors) in rows {
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail, "AdvancedEventSelectors": selectors}),
            )
            .await;
        let applied = reply.expect_ok(&case).clone();
        assert_eq!(
            applied.get("AdvancedEventSelectors"),
            Some(&selectors),
            "case {case:?}: expected the set to be stored as given, got {applied:?}"
        );

        let read = read_selectors(&emulator, trail, &case).await;
        assert_eq!(
            read,
            (Value::Null, selectors.clone()),
            "case {case:?}: expected the applied set read back verbatim, got {read:?}"
        );
    }
}

/// Create an event data store named `name` carrying `selectors`, and hand back the reply.
async fn create_store(emulator: &Emulator, name: &str, selectors: &Value) -> Reply {
    let mut request = Map::new();
    request.insert("Name".to_string(), json!(name));
    request.insert("AdvancedEventSelectors".to_string(), selectors.clone());
    emulator
        .call("CreateEventDataStore", &Value::Object(request))
        .await
}

/// The selectors `GetEventDataStore` reports for the store `identifier` names.
async fn read_store_selectors(emulator: &Emulator, identifier: &str, case: &str) -> Value {
    let reply = emulator
        .call(
            "GetEventDataStore",
            &json!({"EventDataStore": identifier}),
        )
        .await;
    reply
        .expect_ok(&format!("GetEventDataStore after {case}"))
        .get("AdvancedEventSelectors")
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: an event data store's selector set is held to the same rules, against the field
/// catalogue a STORE admits — the trail's fields plus the ones available only for event
/// data stores — and a refused create or update stores nothing.
#[tokio::test]
async fn an_event_data_store_is_held_to_the_rules_with_its_own_field_catalogue() {
    let emulator = Emulator::new();

    // The fields a management-event selector may name on a store and not on a trail.
    let store_only = selector(
        "management-events",
        vec![
            equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
            equals("eventName", &["PutEventSelectors"]),
            equals("eventType", &["AwsApiCall"]),
            equals("sessionCredentialFromConsole", &["false"]),
            json!({"Field": "userIdentity.arn", "StartsWith": ["arn:aws:sts::"]}),
        ],
    );

    let created = create_store(&emulator, "lake-store-fields", &store_only).await;
    let created = created
        .expect_ok("CreateEventDataStore with the fields a store admits")
        .clone();
    assert_eq!(
        created.get("AdvancedEventSelectors"),
        Some(&store_only),
        "case a store's own fields: expected the set stored as given, got {created:?}"
    );
    let identifier = created
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        read_store_selectors(&emulator, &identifier, "the create").await,
        store_only,
        "case a store's own fields: expected the created set read back verbatim"
    );

    // The very same set on a TRAIL is refused: those fields are not available there.
    let trail = "store-fields-on-a-trail";
    trail_with_baseline(&emulator, trail).await;
    let reply = emulator
        .call(
            "PutEventSelectors",
            &json!({"TrailName": trail, "AdvancedEventSelectors": store_only}),
        )
        .await;
    let case = "a store-only field on a trail";
    reply.expect_error(case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
    assert_eq!(
        reply.message(),
        field_not_allowed(MANAGEMENT_EVENT_CATEGORY, "eventName"),
        "case {case:?}: expected the field-not-allowed sentence, got {:?}",
        reply.body
    );

    // (case, the selector set, the message the refusal carries) — the same rules, on the
    // store operations.
    let rows: Vec<(String, Value, String)> = vec![
        (
            "a store selector stating no category".to_string(),
            selector("no-category", vec![equals("readOnly", &["false"])]),
            MISSING_EVENT_CATEGORY.to_string(),
        ),
        (
            "a store selector naming no selector field".to_string(),
            selector(
                "unknown-field",
                vec![
                    equals(CATEGORY_FIELD, &[MANAGEMENT_EVENT_CATEGORY]),
                    equals("notARealSelectorField", &["anything"]),
                ],
            ),
            field_not_allowed(MANAGEMENT_EVENT_CATEGORY, "notARealSelectorField"),
        ),
        (
            "a store selector carrying two categories".to_string(),
            selector(
                "two-categories",
                vec![equals(
                    CATEGORY_FIELD,
                    &[MANAGEMENT_EVENT_CATEGORY, DATA_EVENT_CATEGORY],
                )],
            ),
            TOO_MANY_EVENT_CATEGORIES.to_string(),
        ),
        (
            "a store selector carrying no condition".to_string(),
            selector(
                "operator-less",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    json!({"Field": "readOnly"}),
                ],
            ),
            empty_selector("readOnly"),
        ),
        (
            "a store selector naming an unsupported resource type".to_string(),
            selector(
                "unsupported-resource-type",
                vec![
                    equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
                    equals("resources.type", &["AWS::Sim2Real::Nonexistent"]),
                ],
            ),
            RESOURCES_TYPE_NOT_VALID.to_string(),
        ),
    ];

    for (index, (case, selectors, message)) in rows.into_iter().enumerate() {
        let created = create_store(&emulator, &format!("lake-refused-{index}"), &selectors).await;
        created.expect_error(&case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
        assert_eq!(
            created.message(),
            message,
            "case {case:?} on CreateEventDataStore: expected {message:?}, got {:?}",
            created.body
        );

        let updated = emulator
            .call(
                "UpdateEventDataStore",
                &json!({
                    "EventDataStore": identifier.clone(),
                    "AdvancedEventSelectors": selectors,
                }),
            )
            .await;
        updated.expect_error(&case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
        assert_eq!(
            updated.message(),
            message,
            "case {case:?} on UpdateEventDataStore: expected {message:?}, got {:?}",
            updated.body
        );
        assert_eq!(
            read_store_selectors(&emulator, &identifier, &case).await,
            store_only,
            "case {case:?}: a refused update must leave the store's selectors untouched"
        );
    }

    // A set the rules DO admit still applies, so the rows above refuse the rule and not
    // the operation.
    let applied = selector(
        "data-events",
        vec![
            equals(CATEGORY_FIELD, &[DATA_EVENT_CATEGORY]),
            equals("resources.type", &[ADVANCED_DATA_RESOURCE_TYPES[0]]),
        ],
    );
    let updated = emulator
        .call(
            "UpdateEventDataStore",
            &json!({
                "EventDataStore": identifier.clone(),
                "AdvancedEventSelectors": applied,
            }),
        )
        .await;
    let case = "an update the rules admit";
    assert_eq!(
        updated.expect_ok(case).get("AdvancedEventSelectors"),
        Some(&applied),
        "case {case:?}: expected the updated set echoed back, got {:?}",
        updated.body
    );
    assert_eq!(
        read_store_selectors(&emulator, &identifier, case).await,
        applied,
        "case {case:?}: expected the updated set read back verbatim"
    );
}
