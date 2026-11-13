// SPDX-License-Identifier: CC0-1.0

//! `UpdatePolicy` and `DeletePolicy` over the wire, and the `tags` member every `Policy`
//! body carries.
//!
//! Three rules are under test, each asserted over a TABLE of inputs rather than on one
//! recorded request:
//!
//! 1. A policy's `tags` member is the SET of tags it carries, so every `Policy` body
//!    reports it — the EMPTY map for a policy created without tags — rather than
//!    withholding it, and a client never has to tell "no tags" from "not told".
//! 2. `UpdatePolicy` applies the target blocks the caller sent to the stored policy
//!    (each sent block REPLACES the stored one, each absent block is left alone) and
//!    bumps `updatedAt`, so the `@readonly` `GetPolicy` projection reads them back.
//! 3. `DeletePolicy` removes the policy, so every later reader of its ARN answers
//!    `ResourceNotFoundException` — HTTP 404, the shape named in `x-amzn-errortype`,
//!    `resourceId` the resource segment of the requested ARN, `resourceType` the
//!    resource's type and `message` that type, " not found: " and the same identifier.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the
//! modelled constraints, the restJson1 (de)serializers and the HTTP status codes are
//! exercised exactly as a client would exercise them.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    POLICY_RESOURCE, POLICY_RESOURCE_TYPE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
const CREATE_POLICY: &str = "/v2/create-policy";
const GET_POLICY: &str = "/v2/get-policy";
const UPDATE_POLICY: &str = "/v2/update-policy";
const DELETE_POLICY: &str = "/v2/delete-policy";

/// The Region these suites sign for.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// `TagMap`'s modelled bounds.
///
/// SOURCE: `smithy_query(query="shape", shape="TagMap")` — `length {min: 1, max: 50}`, so
/// a REQUEST carries between one and fifty tags; the number a policy carries after a
/// create is zero or more.
const TAG_MAP_MAX_ENTRIES: usize = 50;

/// The blocks `UpdatePolicy` can set on a policy.
///
/// SOURCE: `smithy_query(query="operation", operation="UpdatePolicy")` — the request
/// members other than `policyArn` (the identifier) and `sharingEnabled` (a flag with a
/// stored default, asserted separately).
const UPDATABLE_BLOCKS: [&str; 5] = [
    "description",
    "availabilitySlo",
    "multiAz",
    "multiRegion",
    "dataRecovery",
];

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and Region. Signature verification is off for these tests, so
/// the value only has to parse.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

/// What the emulator answered: the HTTP status, the `x-amzn-errortype` header restJson1
/// names the error shape in, and the parsed body.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `policy` member of a successful create, update or read.
    fn policy(&self, case: &str) -> Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            self.status, self.body
        );
        let policy = &self.body["policy"];
        assert!(
            policy.is_object(),
            "case {case:?}: the @required policy member is absent from {:?}",
            self.body
        );
        policy.clone()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn create_policy(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", CREATE_POLICY, Some(body)).await
}

async fn get_policy(router: &MultiServiceRouter, policy_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{GET_POLICY}?policyArn={policy_arn}"),
        None,
    )
    .await
}

async fn update_policy(router: &MultiServiceRouter, policy_arn: &str, patch: &Value) -> Answer {
    let mut body = patch.clone();
    body["policyArn"] = json!(policy_arn);
    call(router, "POST", UPDATE_POLICY, Some(body)).await
}

async fn delete_policy(router: &MultiServiceRouter, policy_arn: &str) -> Answer {
    call(
        router,
        "POST",
        DELETE_POLICY,
        Some(json!({"policyArn": policy_arn})),
    )
    .await
}

/// The ARN of the policy a create answered.
fn policy_arn_of(case: &str, policy: &Value) -> String {
    policy["policyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no policyArn in {policy:?}"))
        .to_string()
}

/// The resource segment of an ARN — what `resourceId` reports for the resource it names.
fn arn_resource_id(arn: &str) -> &str {
    arn.splitn(6, ':').nth(5).unwrap_or(arn)
}

/// Assert `answer` is the not-found refusal for `policy_arn`: HTTP 404, the shape in
/// `x-amzn-errortype`, and the three members identifying the resource.
fn assert_not_found(case: &str, policy_arn: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404 for {policy_arn}, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_type.contains("ResourceNotFoundException"),
        "case {case:?}: expected x-amzn-errortype to name ResourceNotFoundException, got \
         {:?}",
        answer.error_type
    );
    let resource_id = arn_resource_id(policy_arn);
    assert_eq!(
        answer.body["resourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: expected resourceId {resource_id:?}, got {:?}",
        answer.body["resourceId"]
    );
    assert_eq!(
        answer.body["resourceType"].as_str(),
        Some(POLICY_RESOURCE_TYPE),
        "case {case:?}: expected resourceType {POLICY_RESOURCE_TYPE:?}, got {:?}",
        answer.body["resourceType"]
    );
    let expected = format!("{POLICY_RESOURCE_TYPE} not found: {resource_id}");
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected message {expected:?}, got {:?}",
        answer.body["message"]
    );
}

/// `n` tags, as a request's `tags` map.
fn tag_map(n: usize) -> Value {
    let mut map = Map::new();
    for index in 0..n {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Rule 1: every Policy body reports its tag map, empty included
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_policy_reports_its_tag_map_including_the_empty_one() {
    // Rows differ along the number of tags the create supplied: none at all, one, a
    // handful, and `TagMap`'s modelled maximum. Each row is read back through GetPolicy
    // and through an UpdatePolicy that touches no tag, so the member is asserted on every
    // path that projects a policy.
    let router = emulator();
    let counts = [0_usize, 1, 3, TAG_MAP_MAX_ENTRIES];
    for count in counts {
        let case = format!("{count} tag(s)");
        let mut body = json!({"name": format!("Tagged-{count}")});
        if count > 0 {
            body["tags"] = tag_map(count);
        }
        let expected = tag_map(count);

        let created = create_policy(&router, body).await.policy(&case);
        assert_eq!(
            created["tags"], expected,
            "case {case:?}: CreatePolicy must report the policy's tag map, got {:?}",
            created["tags"]
        );
        let policy_arn = policy_arn_of(&case, &created);

        let read = get_policy(&router, &policy_arn).await.policy(&case);
        assert_eq!(
            read["tags"], expected,
            "case {case:?}: GetPolicy must report the same tag map, got {:?}",
            read["tags"]
        );

        let updated = update_policy(&router, &policy_arn, &json!({"description": "touched"}))
            .await
            .policy(&case);
        assert_eq!(
            updated["tags"], expected,
            "case {case:?}: an update that touches no tag must leave the tag map as \
             stored, got {:?}",
            updated["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: UpdatePolicy applies the blocks it was sent
// ---------------------------------------------------------------------------

/// One row of the update table: the policy as created, the patch sent to `UpdatePolicy`,
/// and the blocks the merged policy must carry afterwards.
#[derive(Debug, Clone)]
struct UpdateRow {
    case: &'static str,
    created: Value,
    patch: Value,
    expected: Value,
}

/// The update table.
///
/// Rows differ along which blocks the patch carries (none, one, every one of them), whether
/// the block was already set on the stored policy (so the patch replaces it) or absent (so
/// the patch adds it), whether the replacement is narrower than what it replaces, each
/// modelled disaster recovery approach reached through an update, the modelled bounds of
/// the target values, and `sharingEnabled` in both directions.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "multi-AZ targets added to a policy that had none",
            created: json!({"name": "Add-MultiAz"}),
            patch: json!({
                "multiAz": {
                    "rtoInMinutes": 15,
                    "rpoInMinutes": 5,
                    "disasterRecoveryApproach": "WARM_STANDBY",
                }
            }),
            expected: json!({
                "description": null,
                "availabilitySlo": null,
                "multiAz": {
                    "rtoInMinutes": 15,
                    "rpoInMinutes": 5,
                    "disasterRecoveryApproach": "WARM_STANDBY",
                },
                "multiRegion": null,
                "dataRecovery": null,
            }),
        },
        UpdateRow {
            case: "an empty patch leaves every block as stored",
            created: json!({
                "name": "Untouched",
                "description": "as created",
                "multiAz": {"rtoInMinutes": 1, "disasterRecoveryApproach": "PILOT_LIGHT"},
            }),
            patch: json!({}),
            expected: json!({
                "description": "as created",
                "availabilitySlo": null,
                "multiAz": {"rtoInMinutes": 1, "disasterRecoveryApproach": "PILOT_LIGHT"},
                "multiRegion": null,
                "dataRecovery": null,
            }),
        },
        UpdateRow {
            case: "a sent block replaces the stored one wholesale",
            created: json!({
                "name": "Replace-Block",
                "multiAz": {
                    "rtoInMinutes": 120,
                    "rpoInMinutes": 60,
                    "disasterRecoveryApproach": "HOT_STANDBY",
                },
            }),
            // The replacement names only `rpoInMinutes`, so the RTO and the approach the
            // policy used to carry are gone rather than merged member by member.
            patch: json!({"multiAz": {"rpoInMinutes": 0}}),
            expected: json!({
                "description": null,
                "availabilitySlo": null,
                "multiAz": {"rpoInMinutes": 0},
                "multiRegion": null,
                "dataRecovery": null,
            }),
        },
        UpdateRow {
            case: "every block at once, each at a modelled bound",
            created: json!({"name": "All-Blocks", "description": "before"}),
            patch: json!({
                "description": "",
                "availabilitySlo": {"target": 100.0},
                "multiAz": {
                    "rtoInMinutes": 0,
                    "rpoInMinutes": 0,
                    "disasterRecoveryApproach": "ACTIVE_ACTIVE",
                },
                "multiRegion": {
                    "rtoInMinutes": 1440,
                    "rpoInMinutes": 720,
                    "disasterRecoveryApproach": "BACKUP_AND_RESTORE",
                },
                "dataRecovery": {"timeBetweenBackupsInMinutes": 1},
            }),
            expected: json!({
                "description": "",
                "availabilitySlo": {"target": 100.0},
                "multiAz": {
                    "rtoInMinutes": 0,
                    "rpoInMinutes": 0,
                    "disasterRecoveryApproach": "ACTIVE_ACTIVE",
                },
                "multiRegion": {
                    "rtoInMinutes": 1440,
                    "rpoInMinutes": 720,
                    "disasterRecoveryApproach": "BACKUP_AND_RESTORE",
                },
                "dataRecovery": {"timeBetweenBackupsInMinutes": 1},
            }),
        },
        UpdateRow {
            case: "the availability SLO at its lower bound, on a policy that had targets",
            created: json!({
                "name": "Slo-Floor",
                "availabilitySlo": {"target": 99.9},
                "dataRecovery": {"timeBetweenBackupsInMinutes": 60},
            }),
            patch: json!({"availabilitySlo": {"target": 0.0}}),
            expected: json!({
                "description": null,
                "availabilitySlo": {"target": 0.0},
                "multiAz": null,
                "multiRegion": null,
                "dataRecovery": {"timeBetweenBackupsInMinutes": 60},
            }),
        },
        UpdateRow {
            case: "a multi-Region approach reached through an update",
            created: json!({
                "name": "Region-Approach",
                "multiRegion": {"disasterRecoveryApproach": "PILOT_LIGHT"},
            }),
            patch: json!({
                "multiRegion": {
                    "rtoInMinutes": 240,
                    "disasterRecoveryApproach": "ACTIVE_ACTIVE",
                }
            }),
            expected: json!({
                "description": null,
                "availabilitySlo": null,
                "multiAz": null,
                "multiRegion": {
                    "rtoInMinutes": 240,
                    "disasterRecoveryApproach": "ACTIVE_ACTIVE",
                },
                "dataRecovery": null,
            }),
        },
    ]
}

#[tokio::test]
async fn update_policy_applies_the_blocks_it_was_sent_and_a_read_shows_them() {
    let router = emulator();
    for row in update_rows() {
        let case = row.case;
        let created = create_policy(&router, row.created.clone()).await.policy(case);
        let policy_arn = policy_arn_of(case, &created);

        let answer = update_policy(&router, &policy_arn, &row.patch).await;
        let updated = answer.policy(case);
        for block in UPDATABLE_BLOCKS {
            assert_eq!(
                updated[block], row.expected[block],
                "case {case:?}: expected {block} {:?} after the update, got {:?}",
                row.expected[block], updated[block]
            );
        }
        // The policy was updated, not created again: its identity and creation instant
        // stay, and `updatedAt` is at or after `createdAt`.
        assert_eq!(
            updated["policyArn"], created["policyArn"],
            "case {case:?}: an update must not re-mint the ARN, got {:?}",
            updated["policyArn"]
        );
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {case:?}: an update must not move createdAt, got {:?}",
            updated["createdAt"]
        );
        let created_at = created["createdAt"].as_f64();
        let updated_at = updated["updatedAt"].as_f64();
        assert!(
            updated_at.is_some() && updated_at >= created_at,
            "case {case:?}: expected updatedAt at or after createdAt {created_at:?}, got \
             {updated_at:?}"
        );

        // The @readonly reader answers the updated projection, so the write reached the
        // store rather than only this response.
        let read = get_policy(&router, &policy_arn).await.policy(case);
        assert_eq!(
            read, updated,
            "case {case:?}: GetPolicy must answer the policy UpdatePolicy stored"
        );
    }
}

#[tokio::test]
async fn update_policy_sets_the_sharing_flag_in_both_directions() {
    // Rows differ along the flag as created (absent, false, true) and the value the update
    // sends (true, false), so both transitions and both no-ops are covered.
    let router = emulator();
    let rows = [
        (
            "sharing absent at create, enabled by the update",
            json!({"name": "Share-On"}),
            true,
        ),
        (
            "sharing enabled at create, disabled by the update",
            json!({"name": "Share-Off", "sharingEnabled": true}),
            false,
        ),
        (
            "sharing disabled at create, disabled again",
            json!({"name": "Share-Still-Off", "sharingEnabled": false}),
            false,
        ),
        (
            "sharing enabled at create, enabled again",
            json!({"name": "Share-Still-On", "sharingEnabled": true}),
            true,
        ),
    ];
    for (case, body, requested) in rows {
        let created = create_policy(&router, body).await.policy(case);
        let policy_arn = policy_arn_of(case, &created);
        let updated = update_policy(&router, &policy_arn, &json!({"sharingEnabled": requested}))
            .await
            .policy(case);
        assert_eq!(
            updated["sharingEnabled"].as_bool(),
            Some(requested),
            "case {case:?}: expected sharingEnabled {requested}, got {:?}",
            updated["sharingEnabled"]
        );
        let read = get_policy(&router, &policy_arn).await.policy(case);
        assert_eq!(
            read["sharingEnabled"].as_bool(),
            Some(requested),
            "case {case:?}: GetPolicy must report the stored flag, got {:?}",
            read["sharingEnabled"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a deleted policy is gone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_policy_is_no_longer_readable() {
    // Rows differ along what the deleted policy carried: nothing but its name, a full set
    // of target blocks and tags, a policy created with a `clientToken`, and one that was
    // updated before being deleted.
    let router = emulator();
    let rows = [
        ("only a name", json!({"name": "Doomed-Bare"}), None),
        (
            "every block and tags",
            json!({
                "name": "Doomed-Full",
                "description": "to be deleted",
                "availabilitySlo": {"target": 99.0},
                "multiAz": {"rtoInMinutes": 10, "rpoInMinutes": 5},
                "multiRegion": {"rtoInMinutes": 100, "rpoInMinutes": 50},
                "dataRecovery": {"timeBetweenBackupsInMinutes": 30},
                "tags": {"env": "test"},
            }),
            None,
        ),
        (
            "created with a clientToken",
            json!({"name": "Doomed-Token", "clientToken": "token-for-a-doomed-policy"}),
            Some("token-for-a-doomed-policy"),
        ),
        (
            "updated before the delete",
            json!({"name": "Doomed-Updated"}),
            None,
        ),
    ];
    for (case, body, token) in rows {
        let name = body["name"].as_str().expect("a row names its policy");
        let created = create_policy(&router, body.clone()).await.policy(case);
        let policy_arn = policy_arn_of(case, &created);
        if case == "updated before the delete" {
            let answer = update_policy(&router, &policy_arn, &json!({"description": "edited"})).await;
            answer.policy(case);
        }

        let deleted = delete_policy(&router, &policy_arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body["policyArn"].as_str(),
            Some(policy_arn.as_str()),
            "case {case:?}: the delete must answer the ARN it removed, got {:?}",
            deleted.body["policyArn"]
        );

        // Every reader of the ARN now refuses it, and so does a second delete.
        assert_not_found(case, &policy_arn, &get_policy(&router, &policy_arn).await);
        assert_not_found(
            case,
            &policy_arn,
            &update_policy(&router, &policy_arn, &json!({"description": "late"})).await,
        );
        assert_not_found(case, &policy_arn, &delete_policy(&router, &policy_arn).await);

        // The name the deleted policy held is free again, and a `clientToken` that named
        // it no longer replays into it: both indexes went with the record.
        let mut again = json!({"name": name});
        if let Some(token) = token {
            again["clientToken"] = json!(token);
        }
        let recreated = create_policy(&router, again).await.policy(case);
        assert_ne!(
            policy_arn_of(case, &recreated),
            policy_arn,
            "case {case:?}: re-creating {name:?} must mint a fresh policy, not resurrect \
             the deleted one"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_stored_policy_is_refused_by_every_operation() {
    // Rows differ along the ARN's form: another account's policy, another Region's, a
    // different resource type, a well-formed ARN of another service, and a resource
    // segment carrying its own `:` separators. Each satisfies `Arn`'s modelled pattern, so
    // each reaches a handler.
    let router = emulator();
    let arns = [
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{POLICY_RESOURCE}/missing"),
        format!("arn:aws:resiliencehub:eu-central-1:{ACCOUNT}:{POLICY_RESOURCE}/absent"),
        format!("arn:aws:resiliencehub:{REGION}:210987654321:{POLICY_RESOURCE}/elsewhere"),
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:service/not-a-policy"),
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{POLICY_RESOURCE}/nested:segment"),
    ];
    for arn in arns {
        let case = arn.as_str();
        assert_not_found(case, &arn, &get_policy(&router, &arn).await);
        assert_not_found(
            case,
            &arn,
            &update_policy(&router, &arn, &json!({"description": "nothing to update"})).await,
        );
        assert_not_found(case, &arn, &delete_policy(&router, &arn).await);
    }
}
