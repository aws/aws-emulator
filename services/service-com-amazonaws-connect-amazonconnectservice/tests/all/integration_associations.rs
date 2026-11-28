// SPDX-License-Identifier: CC0-1.0

//! `CreateIntegrationAssociation` end to end over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! RULE under test: the call stores an integration association on the instance its
//! `InstanceId` names — one record per (`IntegrationType`, `IntegrationArn`) pair, carrying
//! exactly the members the request described — mints its `IntegrationAssociationId` and the
//! ARN that hangs off the instance's own (`<instance ARN>/integration-association/<id>`),
//! and reports both; the stored record is what every reader of it projects, and the tags it
//! was created with are the ones the tagging plane reports for that ARN. A request the
//! service cannot read is refused 400 before anything is resolved (a path label outside the
//! identifier's modelled bound, a value in neither enum's catalog, a member the
//! `IntegrationType` requires left out), an `InstanceId` the account does not hold is 404,
//! and the same pair associated twice is 409 — and after any refusal the instance holds
//! exactly the associations it held before.
//!
//! The rows span what the rule generalises over: the integration TYPE (every modelled
//! value is covered by the catalog row, and the table itself crosses the kinds of target an
//! association points at — a Lambda function, a domain of another service, an external
//! application), whether the optional members are present, and the size of the tag map.
//!
//! What this suite does NOT assert is the hop into Lambda an integration whose target is a
//! function takes: that needs Lambda mounted beside this service, so it is pinned in
//! `core/router/tests/all/connect_integration_association_lambda_grant.rs`. With no Lambda
//! to ask, such a target is judged against nothing and the reference is stored as it stands,
//! which is what the rows carrying a function ARN here show.

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::{
    IntegrationAssociationSummary, INTEGRATION_TYPE_VALUES, SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_connect_amazonconnectservice::state as connect_state;

use super::harness::{
    connect_managed, looks_like_a_uuid, tag_map, tags_of, Answer, Connect, ABSENT_ID,
    RESOURCE_NOT_FOUND, TAG_MAP_MAXIMUM,
};

impl Connect {
    /// `CreateIntegrationAssociation`: `PUT /instance/{InstanceId}/integration-associations`.
    async fn create_integration_association(&self, instance_id: &str, body: Value) -> Answer {
        self.send_answer(
            "PUT",
            &format!("/instance/{instance_id}/integration-associations"),
            Some(body),
        )
        .await
    }

    /// Every integration association the store holds for `instance_id`, as the modelled
    /// summary a reader projects from the stored record.
    fn stored_associations(&self, instance_id: &str) -> Vec<IntegrationAssociationSummary> {
        let state = connect_state::service_state(self.engine())
            .expect("the shared in-memory engine carries this service's store");
        let store = state.lock();
        let instance = store
            .instances
            .get(instance_id)
            .unwrap_or_else(|| panic!("the store holds the instance {instance_id}"));
        instance
            .integration_associations
            .values()
            .map(connect_state::IntegrationAssociationRecord::to_summary)
            .collect()
    }
}

/// One create: the integration type, the ARN of the resource it points at, the optional
/// members the request carries, and the tag map it is created with.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    /// A member of the model's `IntegrationType` catalog, which
    /// [`every_case_names_a_modelled_integration_type`] checks this table against.
    integration_type: &'static str,
    integration_arn: &'static str,
    source_application_name: Option<&'static str>,
    source_application_url: Option<&'static str>,
    /// A member of the model's `SourceType` catalog where a row carries one.
    source_type: Option<&'static str>,
    tags: Option<Value>,
}

/// A Lambda function ARN: the target of the integration types whose resource is a function.
const FUNCTION_ARN: &str = "arn:aws:lambda:us-east-1:000000000000:function:message-processor";

/// The table the success suites are driven over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a message processor, no optional members, untagged",
            integration_type: "MESSAGE_PROCESSOR",
            integration_arn: FUNCTION_ARN,
            source_application_name: None,
            source_application_url: None,
            source_type: None,
            tags: None,
        },
        Case {
            name: "a file scanner function, one tag",
            integration_type: "FILE_SCANNER",
            integration_arn: "arn:aws:lambda:us-east-1:000000000000:function:scanner",
            source_application_name: None,
            source_application_url: None,
            source_type: None,
            tags: Some(json!({ "env": "prod" })),
        },
        Case {
            name: "a Voice ID domain, several tags",
            integration_type: "VOICE_ID",
            integration_arn: "arn:aws:voiceid:us-east-1:000000000000:domain/EXAMPLE1234567890",
            source_application_name: None,
            source_application_url: None,
            source_type: None,
            tags: Some(json!({ "env": "staging", "team": "voice" })),
        },
        Case {
            name: "a Cases domain, the modelled maximum tag map",
            integration_type: "CASES_DOMAIN",
            integration_arn: "arn:aws:cases:us-east-1:000000000000:domain/EXAMPLE1234567890",
            source_application_name: None,
            source_application_url: None,
            source_type: None,
            tags: Some(tag_map(TAG_MAP_MAXIMUM)),
        },
        Case {
            name: "an external application event source, every optional member",
            integration_type: "EVENT",
            integration_arn: "arn:aws:app-integrations:us-east-1:000000000000:event-integration/orders",
            source_application_name: Some("Order Desk"),
            source_application_url: Some("https://orders.example.com/app"),
            source_type: Some("SALESFORCE"),
            tags: None,
        },
        Case {
            name: "a third-party application, an application target",
            integration_type: "APPLICATION",
            integration_arn: "arn:aws:app-integrations:us-east-1:000000000000:application/EXAMPLE1234567890",
            source_application_name: None,
            source_application_url: None,
            source_type: None,
            tags: Some(json!({ "owner": "crm" })),
        },
    ]
}

/// The request body `case` is created with.
fn body_of(case: &Case) -> Value {
    let mut body = json!({
        "IntegrationType": case.integration_type,
        "IntegrationArn": case.integration_arn,
    });
    if let Some(name) = case.source_application_name {
        body["SourceApplicationName"] = json!(name);
    }
    if let Some(url) = case.source_application_url {
        body["SourceApplicationUrl"] = json!(url);
    }
    if let Some(source_type) = case.source_type {
        body["SourceType"] = json!(source_type);
    }
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

/// The table is written by hand, so it is pinned to the model's own value lists: a row
/// naming anything else is a stale table, not a failing emulator. The catalog row below
/// covers every modelled type, so this only checks the hand-written table is in the
/// catalog.
#[test]
fn every_case_names_a_modelled_integration_type() {
    for case in cases() {
        assert!(
            INTEGRATION_TYPE_VALUES.contains(&case.integration_type),
            "case {:?}: IntegrationType does not name {}",
            case.name,
            case.integration_type
        );
        if let Some(source_type) = case.source_type {
            assert!(
                SOURCE_TYPE_VALUES.contains(&source_type),
                "case {:?}: SourceType does not name {source_type}",
                case.name
            );
        }
    }
}

/// RULE: the create stores the association it describes and reports the identifier and the
/// ARN of the record it stored — the ARN hanging off the instance's own — and the stored
/// record carries exactly the members the request carried.
#[tokio::test]
async fn a_created_association_is_stored_as_the_request_described_it() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = connect.create_instance(connect_managed("integrations")).await;

    for case in cases() {
        let created = connect
            .create_integration_association(&instance_id, body_of(&case))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateIntegrationAssociation got {} {}",
            case.name, created.status, created.body
        );

        let id = created.body["IntegrationAssociationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected an IntegrationAssociationId, got {}",
                    case.name, created.body
                )
            })
            .to_string();
        assert!(
            looks_like_a_uuid(&id),
            "case {:?}: expected a minted identifier, got {id:?}",
            case.name
        );
        let expected_arn = format!("{instance_arn}/integration-association/{id}");
        assert_eq!(
            created.body["IntegrationAssociationArn"],
            json!(expected_arn),
            "case {:?}: expected {expected_arn:?}, got {}",
            case.name,
            created.body["IntegrationAssociationArn"]
        );

        // The record behind that answer, as a reader of it projects it.
        let stored = connect.stored_associations(&instance_id);
        let summary = stored
            .iter()
            .find(|summary| summary.integration_association_id.as_deref() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the instance holds the association {id}, holds {:?}",
                    case.name,
                    stored
                        .iter()
                        .map(|summary| summary.integration_association_id.clone())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            summary.integration_association_id.as_deref(),
            Some(id.as_str()),
            "case {:?}: IntegrationAssociationId",
            case.name
        );
        assert_eq!(
            summary.integration_association_arn.as_deref(),
            Some(expected_arn.as_str()),
            "case {:?}: IntegrationAssociationArn, got {:?}",
            case.name,
            summary.integration_association_arn
        );
        assert_eq!(
            summary.instance_id.as_deref(),
            Some(instance_id.as_str()),
            "case {:?}: InstanceId, got {:?}",
            case.name,
            summary.instance_id
        );
        assert_eq!(
            summary.integration_type.as_deref(),
            Some(case.integration_type),
            "case {:?}: IntegrationType, got {:?}",
            case.name,
            summary.integration_type
        );
        assert_eq!(
            summary.integration_arn.as_deref(),
            Some(case.integration_arn),
            "case {:?}: IntegrationArn, got {:?}",
            case.name,
            summary.integration_arn
        );
        assert_eq!(
            summary.source_application_name.as_deref(),
            case.source_application_name,
            "case {:?}: SourceApplicationName, got {:?}",
            case.name,
            summary.source_application_name
        );
        assert_eq!(
            summary.source_application_url.as_deref(),
            case.source_application_url,
            "case {:?}: SourceApplicationUrl, got {:?}",
            case.name,
            summary.source_application_url
        );
        assert_eq!(
            summary.source_type.as_deref(),
            case.source_type,
            "case {:?}: SourceType, got {:?}",
            case.name,
            summary.source_type
        );
        assert_eq!(
            summary.last_modified_region.as_deref(),
            Some("us-east-1"),
            "case {:?}: LastModifiedRegion, got {:?}",
            case.name,
            summary.last_modified_region
        );
        assert!(
            summary.last_modified_time.is_some(),
            "case {:?}: expected a LastModifiedTime",
            case.name
        );
    }

    // Every row of the table is held at once: one instance carries as many associations as
    // were created on it, each under its own identifier.
    let held = connect.stored_associations(&instance_id).len();
    assert_eq!(
        held,
        cases().len(),
        "the instance holds one association per created row, got {held}"
    );
}

/// RULE: the tags a create stored on the association are the ones the tagging plane
/// reports for that association's ARN — the stored record is the one every reader reads.
#[tokio::test]
async fn the_tags_of_a_created_association_read_back_through_its_arn() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("integration-tags")).await;

    for case in cases() {
        let created = connect
            .create_integration_association(&instance_id, body_of(&case))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateIntegrationAssociation got {} {}",
            case.name, created.status, created.body
        );
        let arn = created.body["IntegrationAssociationArn"]
            .as_str()
            .expect("an IntegrationAssociationArn")
            .to_string();

        let listed = connect.list_tags_for_resource(&arn).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: ListTagsForResource got {} {}",
            case.name, listed.status, listed.body
        );
        let reported = tags_of(&listed);
        match &case.tags {
            None => assert!(
                reported.is_none_or(|tags| tags.is_empty()),
                "case {:?}: an untagged association reports no tags, got {}",
                case.name,
                listed.body
            ),
            Some(tags) => {
                let expected: std::collections::BTreeMap<String, String> = tags
                    .as_object()
                    .expect("a tag map")
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value.as_str().expect("a tag value").to_string(),
                        )
                    })
                    .collect();
                assert_eq!(
                    reported.as_ref(),
                    Some(&expected),
                    "case {:?}: expected {expected:?}, got {reported:?}",
                    case.name
                );
            }
        }
    }
}

/// RULE: every value the model's `IntegrationType` catalog declares is a type an
/// association can be created under, and each is stored as its own record.
///
/// The catalog is the model's own list rather than a hand-written one, so a type added to
/// the model is covered the moment it is modelled.
#[tokio::test]
async fn every_modelled_integration_type_can_be_associated() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("integration-catalog")).await;

    for integration_type in INTEGRATION_TYPE_VALUES {
        let mut body = json!({
            "IntegrationType": integration_type,
            "IntegrationArn":
                format!("arn:aws:app-integrations:us-east-1:000000000000:resource/{integration_type}"),
        });
        // The one type whose request carries the external application's own members.
        if *integration_type == connect_state::EVENT_INTEGRATION_TYPE {
            body["SourceApplicationName"] = json!("Catalog App");
            body["SourceApplicationUrl"] = json!("https://catalog.example.com/app");
            body["SourceType"] = json!(SOURCE_TYPE_VALUES[0]);
        }
        let created = connect
            .create_integration_association(&instance_id, body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "type {integration_type}: CreateIntegrationAssociation {body} got {} {}",
            created.status, created.body
        );
    }

    let held = connect.stored_associations(&instance_id).len();
    assert_eq!(
        held,
        INTEGRATION_TYPE_VALUES.len(),
        "the instance holds one association per modelled type, got {held}"
    );
}

/// RULE: one resource is associated to an instance once per integration type — the same
/// pair again is the modelled 409 and adds nothing — while the same resource under ANOTHER
/// type, and another resource under the same type, are associations of their own.
#[tokio::test]
async fn the_same_pair_is_associated_once() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("integration-dupes")).await;

    for case in cases() {
        let first = connect
            .create_integration_association(&instance_id, body_of(&case))
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first create got {} {}",
            case.name, first.status, first.body
        );
        let before = connect.stored_associations(&instance_id).len();

        let again = connect
            .create_integration_association(&instance_id, body_of(&case))
            .await;
        assert_eq!(
            again.status, 409,
            "case {:?}: the second create of the same pair got {} {}",
            case.name, again.status, again.body
        );
        assert_eq!(
            again.error_code, "DuplicateResourceException",
            "case {:?}: expected DuplicateResourceException, got {:?} {}",
            case.name, again.error_code, again.body
        );
        let after = connect.stored_associations(&instance_id).len();
        assert_eq!(
            after, before,
            "case {:?}: a refused create adds nothing, held {before} then {after}",
            case.name
        );
    }

    // The same resource under a DIFFERENT type is its own association, and so is a
    // different resource under a type already used.
    let other_type = json!({
        "IntegrationType": "CALL_TRANSFER_CONNECTOR",
        "IntegrationArn": FUNCTION_ARN,
    });
    let created = connect
        .create_integration_association(&instance_id, other_type.clone())
        .await;
    assert_eq!(
        created.status, 200,
        "the same function under another type is its own association, got {} {}",
        created.status, created.body
    );
    let other_resource = json!({
        "IntegrationType": "MESSAGE_PROCESSOR",
        "IntegrationArn": "arn:aws:lambda:us-east-1:000000000000:function:second-processor",
    });
    let created = connect
        .create_integration_association(&instance_id, other_resource.clone())
        .await;
    assert_eq!(
        created.status, 200,
        "another function under the same type is its own association, got {} {}",
        created.status, created.body
    );
}

/// RULE: a request the service cannot read is refused 400 before anything is resolved, an
/// `InstanceId` the account does not hold is 404 — and nothing is stored either way.
///
/// The rows span the classes: a path label past the identifier's modelled `@length`
/// maximum, a value in neither enum catalog, each member the `EVENT` type requires left
/// out, and an identifier of the minted form that names no instance.
#[tokio::test]
async fn a_request_the_service_cannot_serve_stores_nothing() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("integration-refusals")).await;

    let event_body = |without: &str| {
        let mut body = json!({
            "IntegrationType": "EVENT",
            "IntegrationArn": "arn:aws:app-integrations:us-east-1:000000000000:event-integration/orders",
            "SourceApplicationName": "Order Desk",
            "SourceApplicationUrl": "https://orders.example.com/app",
            "SourceType": "SALESFORCE",
        });
        body.as_object_mut().expect("an object").remove(without);
        body
    };

    // Each row: the instance label the request addresses, the body, and the status, error
    // code and message the service answers with.
    struct Refusal {
        case: &'static str,
        label: String,
        body: Value,
        status: u16,
        code: &'static str,
        message: String,
    }

    let rows = vec![
        Refusal {
            case: "a label one past the identifier's modelled maximum",
            label: "i".repeat(101),
            body: json!({ "IntegrationType": "VOICE_ID", "IntegrationArn": FUNCTION_ARN }),
            status: 400,
            code: "InvalidRequestException",
            message: "Instance id is not valid".to_string(),
        },
        Refusal {
            case: "an integration type in no catalog",
            label: instance_id.clone(),
            body: json!({ "IntegrationType": "MESSAGE_PROCESSORS", "IntegrationArn": FUNCTION_ARN }),
            status: 400,
            code: "InvalidRequestException",
            message: "Invalid IntegrationType".to_string(),
        },
        Refusal {
            case: "a source type in no catalog",
            label: instance_id.clone(),
            body: json!({
                "IntegrationType": "EVENT",
                "IntegrationArn": "arn:aws:app-integrations:us-east-1:000000000000:event-integration/orders",
                "SourceApplicationName": "Order Desk",
                "SourceApplicationUrl": "https://orders.example.com/app",
                "SourceType": "HUBSPOT",
            }),
            status: 400,
            code: "InvalidRequestException",
            message: "Invalid SourceType".to_string(),
        },
        Refusal {
            case: "the EVENT type with no SourceApplicationUrl",
            label: instance_id.clone(),
            body: event_body("SourceApplicationUrl"),
            status: 400,
            code: "InvalidRequestException",
            message: "SourceApplicationUrl is required for the EVENT integration type".to_string(),
        },
        Refusal {
            case: "the EVENT type with no SourceApplicationName",
            label: instance_id.clone(),
            body: event_body("SourceApplicationName"),
            status: 400,
            code: "InvalidRequestException",
            message: "SourceApplicationName is required for the EVENT integration type".to_string(),
        },
        Refusal {
            case: "the EVENT type with no SourceType",
            label: instance_id.clone(),
            body: event_body("SourceType"),
            status: 400,
            code: "InvalidRequestException",
            message: "SourceType is required for the EVENT integration type".to_string(),
        },
        Refusal {
            case: "an identifier of the minted form that names no instance",
            label: ABSENT_ID.to_string(),
            body: json!({ "IntegrationType": "VOICE_ID", "IntegrationArn": FUNCTION_ARN }),
            status: 404,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND.to_string(),
        },
    ];

    for row in rows {
        let refused = connect
            .create_integration_association(&row.label, row.body.clone())
            .await;
        assert_eq!(
            refused.status, row.status,
            "case {:?}: expected HTTP {} for {}, got {} {}",
            row.case, row.status, row.body, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, row.code,
            "case {:?}: expected {}, got {:?} {}",
            row.case, row.code, refused.error_code, refused.body
        );
        assert_eq!(
            refused.body["Message"],
            json!(row.message),
            "case {:?}: expected {:?}, got {}",
            row.case,
            row.message,
            refused.body
        );
        let held = connect.stored_associations(&instance_id).len();
        assert_eq!(
            held, 0,
            "case {:?}: a refused create stores nothing, the instance holds {held}",
            row.case
        );
    }
}
