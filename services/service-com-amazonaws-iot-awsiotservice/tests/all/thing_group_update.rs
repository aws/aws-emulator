// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateThingGroup`: what a change to a registered thing
//! group record does, and what the `DescribeThingGroup` reader then reports.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * `thingGroupProperties` is `@required` on this operation
//!   (`smithy_query(query="operation",operation="UpdateThingGroup")`), so what the request
//!   carries IS the group's properties afterwards: the description it carries replaces the
//!   stored one (and a request carrying none leaves the group without one), and the
//!   attribute list it provides overwrites the stored attributes — unless
//!   `attributePayload.merge` asks for them to be "merged with the attributes stored in
//!   the registry, instead of overwriting them", and in either case "To remove an
//!   attribute, call `UpdateThing` with an empty attribute value";
//! * one successful update is one write of the record, so the `version` it answers — "The
//!   version of the updated thing group" — is the stored version advanced by exactly one,
//!   and it is what the next read reports;
//! * `expectedVersion` is optimistic concurrency: "If this does not match the version of
//!   the thing group being updated, the update will fail" — refused
//!   `VersionConflictException` (409), with the record and its version left exactly as
//!   they were;
//! * a `thingGroupName` the registry does not hold is refused `ResourceNotFoundException`
//!   (404);
//! * an update changes only the group's properties: the identity minted on create (id,
//!   ARN, creation date) and the group's place in the hierarchy survive it, because
//!   `UpdateThingGroup` models no member for either.
//!
//! The tables span what those rules generalise over: whether the provided list is merged,
//! how many attributes it carries and of what form (including the empty value that removes
//! one), whether the payload or its `attributes` map is present at all, whether a
//! description is carried and of what length, and how a supplied `expectedVersion` relates
//! to the stored one.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// region: the scope is how the router picks the claiming service. The signature itself is
/// never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `version` member, or `None` when it is absent or not a number.
    fn version(&self) -> Option<i64> {
        self.body.get("version").and_then(Value::as_i64)
    }

    /// `thingGroupProperties.thingGroupDescription`, when the read reports one.
    fn description(&self) -> Option<String> {
        self.body
            .get("thingGroupProperties")?
            .get("thingGroupDescription")?
            .as_str()
            .map(str::to_string)
    }

    /// `thingGroupProperties.attributePayload.attributes` as an ordered map, so two maps
    /// compare independently of JSON member order. An absent payload is the empty map: a
    /// group that carries no attributes.
    fn attributes(&self) -> BTreeMap<String, String> {
        let Some(members) = self
            .body
            .get("thingGroupProperties")
            .and_then(|properties| properties.get("attributePayload"))
            .and_then(|payload| payload.get("attributes"))
            .and_then(Value::as_object)
        else {
            return BTreeMap::new();
        };
        members
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// `thingGroupMetadata.parentGroupName`, when the read reports one.
    fn parent_group_name(&self) -> Option<String> {
        self.body
            .get("thingGroupMetadata")?
            .get("parentGroupName")?
            .as_str()
            .map(str::to_string)
    }

    /// `thingGroupMetadata.creationDate`, as the epoch seconds the restJson1 timestamp
    /// carries.
    fn creation_date(&self) -> Option<f64> {
        self.body
            .get("thingGroupMetadata")?
            .get("creationDate")?
            .as_f64()
    }

    /// The error message the refusal carries, or the empty string when it carries none.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_group(&self, thing_group_name: &str, request: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/thing-groups/{thing_group_name}"),
            Some(request),
        )
        .await
    }

    async fn update_group(&self, thing_group_name: &str, request: &Value) -> Reply {
        self.call(
            "PATCH",
            &format!("/thing-groups/{thing_group_name}"),
            Some(request),
        )
        .await
    }

    async fn describe_group(&self, thing_group_name: &str) -> Reply {
        self.call("GET", &format!("/thing-groups/{thing_group_name}"), None)
            .await
    }
}

fn attributes_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// A `thingGroupProperties` body carrying `description` and `attributes` as its
/// `attributePayload`, omitting each member the case leaves out. `merge` is written into
/// the payload only when the case names it, so one row can leave the modelled
/// `default false` to decide.
fn properties_of(
    description: Option<&str>,
    attributes: Option<&[(&str, &str)]>,
    merge: Option<bool>,
) -> Value {
    let mut properties = Map::new();
    if let Some(description) = description {
        properties.insert("thingGroupDescription".to_string(), json!(description));
    }
    if attributes.is_some() || merge.is_some() {
        let mut payload = Map::new();
        if let Some(pairs) = attributes {
            let attributes: Map<String, Value> = pairs
                .iter()
                .map(|(name, value)| ((*name).to_string(), json!(value)))
                .collect();
            payload.insert("attributes".to_string(), Value::Object(attributes));
        }
        if let Some(merge) = merge {
            payload.insert("merge".to_string(), json!(merge));
        }
        properties.insert("attributePayload".to_string(), Value::Object(payload));
    }
    Value::Object(properties)
}

/// One properties-update case: what the group is created with, the
/// `thingGroupProperties` the update carries, and the description and attributes the
/// record must hold afterwards.
struct PropertiesCase {
    label: &'static str,
    created: Value,
    update: Value,
    expected_description: Option<&'static str>,
    expected_attributes: BTreeMap<String, String>,
}

/// The table of property updates. Its rows differ along what the replacement-and-merge
/// rule generalises over:
///
/// * whether the provided list is MERGED into the stored attributes or OVERWRITES them
///   (`AttributePayload$merge`, modelled `default false`);
/// * how many attributes the list carries — none, one, several — and how many the record
///   already held;
/// * whether an entry's value is EMPTY, which removes the attribute;
/// * whether the request carries an `attributePayload` at all, and whether that payload
///   carries an `attributes` map;
/// * whether the request carries a description, and of what length and alphabet —
///   `ThingGroupDescription` is modelled `length 0..2028, pattern ^[\p{Graph}\x20]*$`
///   (`smithy_query(query="shape",shape="ThingGroupProperties")`), so one row carries the
///   printable punctuation it allows and one a description at the modelled maximum
///   length.
fn properties_cases() -> Vec<PropertiesCase> {
    let longest_description: &'static str = Box::leak("d".repeat(2028).into_boxed_str());
    vec![
        PropertiesCase {
            label: "a description-only update replaces the description and the attributes",
            created: properties_of(Some("as created"), Some(&[("color", "red")]), None),
            update: properties_of(Some("as updated"), None, None),
            expected_description: Some("as updated"),
            expected_attributes: BTreeMap::new(),
        },
        PropertiesCase {
            label: "empty properties leave the group with neither description nor attributes",
            created: properties_of(Some("as created"), Some(&[("color", "red")]), None),
            update: json!({}),
            expected_description: None,
            expected_attributes: BTreeMap::new(),
        },
        PropertiesCase {
            label: "merge adds an attribute and keeps the stored ones",
            created: properties_of(Some("as created"), Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(Some("as created"), Some(&[("size", "large")]), Some(true)),
            expected_description: Some("as created"),
            expected_attributes: attributes_of(&[
                ("color", "red"),
                ("floor", "3"),
                ("size", "large"),
            ]),
        },
        PropertiesCase {
            label: "merge overwrites one stored attribute",
            created: properties_of(None, Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(None, Some(&[("color", "blue")]), Some(true)),
            expected_description: None,
            expected_attributes: attributes_of(&[("color", "blue"), ("floor", "3")]),
        },
        PropertiesCase {
            label: "merge with an empty value removes that attribute only",
            created: properties_of(None, Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(None, Some(&[("color", "")]), Some(true)),
            expected_description: None,
            expected_attributes: attributes_of(&[("floor", "3")]),
        },
        PropertiesCase {
            label: "merge of several attributes at once",
            created: properties_of(None, Some(&[("color", "red")]), None),
            update: properties_of(
                None,
                Some(&[("color", "green"), ("size", "small"), ("floor", "2")]),
                Some(true),
            ),
            expected_description: None,
            expected_attributes: attributes_of(&[
                ("color", "green"),
                ("floor", "2"),
                ("size", "small"),
            ]),
        },
        PropertiesCase {
            label: "merge of an empty list leaves the stored attributes alone",
            created: properties_of(None, Some(&[("color", "red")]), None),
            update: properties_of(None, Some(&[]), Some(true)),
            expected_description: None,
            expected_attributes: attributes_of(&[("color", "red")]),
        },
        PropertiesCase {
            label: "a payload carrying only merge leaves the stored attributes alone",
            created: properties_of(None, Some(&[("color", "red")]), None),
            update: properties_of(None, None, Some(true)),
            expected_description: None,
            expected_attributes: attributes_of(&[("color", "red")]),
        },
        PropertiesCase {
            label: "no merge: the provided list overwrites the stored attributes",
            created: properties_of(None, Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(None, Some(&[("size", "large")]), Some(false)),
            expected_description: None,
            expected_attributes: attributes_of(&[("size", "large")]),
        },
        PropertiesCase {
            label: "no merge, with merge omitted from the payload altogether",
            created: properties_of(None, Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(None, Some(&[("size", "large")]), None),
            expected_description: None,
            expected_attributes: attributes_of(&[("size", "large")]),
        },
        PropertiesCase {
            label: "no merge with an empty list leaves the group with no attributes",
            created: properties_of(None, Some(&[("color", "red"), ("floor", "3")]), None),
            update: properties_of(None, Some(&[]), Some(false)),
            expected_description: None,
            expected_attributes: BTreeMap::new(),
        },
        PropertiesCase {
            label: "an attribute name and value using the whole modelled alphabet",
            created: properties_of(None, None, None),
            update: properties_of(
                None,
                Some(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")],),
                Some(false),
            ),
            expected_description: None,
            expected_attributes: attributes_of(&[("build.no,v@1/2:3#4-5", "a=b[c]-d.e,f@g/h:i#j")]),
        },
        PropertiesCase {
            label: "a description carrying the printable punctuation the pattern allows",
            created: properties_of(Some("plain"), None, None),
            update: properties_of(Some("fleet #7 (east) - \"prod\"; 50% rolled out!"), None, None),
            expected_description: Some("fleet #7 (east) - \"prod\"; 50% rolled out!"),
            expected_attributes: BTreeMap::new(),
        },
        PropertiesCase {
            label: "a description at the modelled maximum length",
            created: properties_of(None, None, None),
            update: properties_of(Some(longest_description), None, None),
            expected_description: Some(longest_description),
            expected_attributes: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn an_update_applies_its_properties_and_a_read_reports_them() {
    let emulator = Emulator::new();

    for (index, case) in properties_cases().into_iter().enumerate() {
        let label = case.label;
        let group_name = format!("props-group-{index}");
        let created = emulator
            .create_group(&group_name, &json!({ "thingGroupProperties": case.created }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the group must be created, got {created:?}"
        );
        let before = emulator.describe_group(&group_name).await;
        let created_version = before.version();

        let updated = emulator
            .update_group(&group_name, &json!({ "thingGroupProperties": case.update }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateThingGroup must succeed, got {updated:?}"
        );
        assert_eq!(
            updated.version(),
            created_version.map(|version| version + 1),
            "case {label:?}: the answered version must be the stored one advanced by one, \
             got {updated:?}"
        );

        let described = emulator.describe_group(&group_name).await;
        assert_eq!(
            described.description(),
            case.expected_description.map(str::to_string),
            "case {label:?}: expected description {:?}, got {described:?}",
            case.expected_description
        );
        assert_eq!(
            described.attributes(),
            case.expected_attributes,
            "case {label:?}: expected attributes {:?}, got {described:?}",
            case.expected_attributes
        );
        assert_eq!(
            described.version(),
            updated.version(),
            "case {label:?}: the read must report the version the update answered, \
             got {described:?}"
        );
    }
}

#[tokio::test]
async fn each_update_advances_the_version_by_one_and_keeps_the_created_identity() {
    let emulator = Emulator::new();

    let parent = emulator
        .create_group("versioned-parent", &json!({}))
        .await;
    assert_eq!(
        parent.status, 200,
        "the parent group must be created, got {parent:?}"
    );
    let created = emulator
        .create_group(
            "versioned-child",
            &json!({
                "parentGroupName": "versioned-parent",
                "thingGroupProperties": properties_of(Some("first"), Some(&[("color", "red")]), None),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group must be created, got {created:?}"
    );
    let before = emulator.describe_group("versioned-child").await;
    let created_version = before
        .version()
        .expect("a created group reports its version");
    let created_id = before.text("thingGroupId");
    let created_arn = before.text("thingGroupArn");
    let created_at = before.creation_date();

    // Four updates in a row, each a single write of the record: the version advances by
    // exactly one per call, whether the update changes anything or repeats what the record
    // already held.
    let updates = [
        properties_of(Some("second"), Some(&[("color", "blue")]), Some(true)),
        properties_of(Some("second"), Some(&[("color", "blue")]), Some(true)),
        properties_of(None, None, None),
        properties_of(Some("fourth"), Some(&[("size", "small")]), Some(false)),
    ];
    for (step, properties) in updates.iter().enumerate() {
        let updated = emulator
            .update_group(
                "versioned-child",
                &json!({ "thingGroupProperties": properties }),
            )
            .await;
        let expected_version = created_version + step as i64 + 1;
        assert_eq!(
            updated.status, 200,
            "update {step}: UpdateThingGroup must succeed, got {updated:?}"
        );
        assert_eq!(
            updated.version(),
            Some(expected_version),
            "update {step}: expected version {expected_version}, got {updated:?}"
        );
    }

    let described = emulator.describe_group("versioned-child").await;
    assert_eq!(
        described.version(),
        Some(created_version + updates.len() as i64),
        "the read must report the version the last update left, got {described:?}"
    );
    assert_eq!(
        described.text("thingGroupId"),
        created_id,
        "an update mints no new id, got {described:?}"
    );
    assert_eq!(
        described.text("thingGroupArn"),
        created_arn,
        "an update mints no new ARN, got {described:?}"
    );
    assert_eq!(
        described.creation_date(),
        created_at,
        "an update does not re-date the group, got {described:?}"
    );
    assert_eq!(
        described.parent_group_name().as_deref(),
        Some("versioned-parent"),
        "an update models no parentGroupName, so the hierarchy survives it, got {described:?}"
    );
    assert_eq!(
        described.attributes(),
        attributes_of(&[("size", "small")]),
        "the last update's list overwrites the stored attributes, got {described:?}"
    );
    assert_eq!(
        described.description().as_deref(),
        Some("fourth"),
        "the last update's description is the one stored, got {described:?}"
    );
}

/// One `expectedVersion` case: the version the request supplies relative to the stored
/// one, and whether the update then applies.
struct VersionCase {
    label: &'static str,
    expected_version: Option<i64>,
    applies: bool,
}

/// The table spans how a supplied `expectedVersion` can relate to the stored version: it
/// can be absent, equal, one behind, one ahead, the `OptionalVersion` zero a caller that
/// never read the group sends, and a negative value no record ever carries.
fn version_cases(stored: i64) -> Vec<VersionCase> {
    vec![
        VersionCase {
            label: "no expectedVersion at all",
            expected_version: None,
            applies: true,
        },
        VersionCase {
            label: "the stored version",
            expected_version: Some(stored),
            applies: true,
        },
        VersionCase {
            label: "one version behind the record",
            expected_version: Some(stored - 1),
            applies: false,
        },
        VersionCase {
            label: "one version ahead of the record",
            expected_version: Some(stored + 1),
            applies: false,
        },
        VersionCase {
            label: "zero",
            expected_version: Some(0),
            applies: false,
        },
        VersionCase {
            label: "a negative version",
            expected_version: Some(-1),
            applies: false,
        },
    ]
}

#[tokio::test]
async fn expected_version_decides_whether_the_update_applies() {
    let emulator = Emulator::new();

    for (index, case) in version_cases(2).into_iter().enumerate() {
        let label = case.label;
        let group_name = format!("version-group-{index}");
        let created = emulator
            .create_group(
                &group_name,
                &json!({
                    "thingGroupProperties":
                        properties_of(Some("as created"), Some(&[("color", "red")]), None),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the group must be created, got {created:?}"
        );
        // One update with no expectation puts the record at version 2, which is the stored
        // version every row of the table is written against.
        let bumped = emulator
            .update_group(
                &group_name,
                &json!({
                    "thingGroupProperties":
                        properties_of(Some("as created"), Some(&[("color", "red")]), Some(true)),
                }),
            )
            .await;
        assert_eq!(
            bumped.version(),
            Some(2),
            "case {label:?}: the record must stand at version 2 before the case runs, \
             got {bumped:?}"
        );

        let mut request = Map::new();
        request.insert(
            "thingGroupProperties".to_string(),
            properties_of(Some("as expected"), Some(&[("color", "green")]), Some(true)),
        );
        if let Some(expected_version) = case.expected_version {
            request.insert("expectedVersion".to_string(), json!(expected_version));
        }
        let updated = emulator
            .update_group(&group_name, &Value::Object(request))
            .await;

        let described = emulator.describe_group(&group_name).await;
        if case.applies {
            assert_eq!(
                updated.status, 200,
                "case {label:?}: UpdateThingGroup must succeed, got {updated:?}"
            );
            assert_eq!(
                updated.version(),
                Some(3),
                "case {label:?}: an applied update advances the version, got {updated:?}"
            );
            assert_eq!(
                described.attributes(),
                attributes_of(&[("color", "green")]),
                "case {label:?}: an applied update is what the read reports, got {described:?}"
            );
            assert_eq!(
                described.description().as_deref(),
                Some("as expected"),
                "case {label:?}: an applied update is what the read reports, got {described:?}"
            );
        } else {
            assert_eq!(
                updated.status, 409,
                "case {label:?}: UpdateThingGroup must be refused 409, got {updated:?}"
            );
            assert_eq!(
                updated.error_code.as_deref(),
                Some("VersionConflictException"),
                "case {label:?}: expected VersionConflictException, got {updated:?}"
            );
            let message = updated.message();
            assert_eq!(
                message,
                format!(
                    "The version for thing group {group_name} does not match the expected version."
                ),
                "case {label:?}: the refusal must be the thing-group family's version-conflict \
                 sentence, got {message:?}"
            );
            assert_eq!(
                described.version(),
                Some(2),
                "case {label:?}: a refused update advances nothing, got {described:?}"
            );
            assert_eq!(
                described.attributes(),
                attributes_of(&[("color", "red")]),
                "case {label:?}: a refused update changes nothing, got {described:?}"
            );
            assert_eq!(
                described.description().as_deref(),
                Some("as created"),
                "case {label:?}: a refused update changes nothing, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn updating_a_group_the_registry_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // The same miss, asked over the forms an update request takes: with and without a
    // description, an attribute payload and an expectation.
    let requests = [
        json!({ "thingGroupProperties": {} }),
        json!({ "thingGroupProperties": { "thingGroupDescription": "ghosts" } }),
        json!({
            "thingGroupProperties": properties_of(None, Some(&[("color", "red")]), Some(true)),
            "expectedVersion": 1,
        }),
    ];
    for (index, request) in requests.iter().enumerate() {
        let refused = emulator.update_group("ghost-group", request).await;
        assert_eq!(
            refused.status, 404,
            "request {index}: UpdateThingGroup of an unregistered name must be refused 404, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "request {index}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            "Group ghost-group does not exist".to_string(),
            "request {index}: the refusal must be this operation's own sentence, \
             got {refused:?}"
        );
    }

    // Nothing was created by the refused updates: the reader still holds no such group.
    let described = emulator.describe_group("ghost-group").await;
    assert_eq!(
        described.status, 404,
        "a refused update persists nothing, got {described:?}"
    );
}
