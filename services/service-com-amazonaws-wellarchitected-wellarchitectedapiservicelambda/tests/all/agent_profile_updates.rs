// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAgentProfile` — the update half of the agent-profile
//! lifecycle whose create and read are pinned by `super::get_agent_profile` and whose
//! delete is pinned by `super::agent_profile_lifecycle`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. An update MERGES: every member the request carries replaces the stored one and
//!    every member it omits is left exactly as it was — never nulled — whatever the
//!    profile carried and whichever members the request names. The members no update
//!    request models (`arn`, `name`, `createdBy`, `createdAt`) cannot change, and the tags
//!    the create recorded are left where they are — `UpdateAgentProfileResponse` declares
//!    no `tags` member, so the update's own answer carries none
//!    (`smithy_query(query="operation", operation="UpdateAgentProfile")`).
//! 2. The modification is RECORDED: `lastModifiedBy` is the account the request is
//!    attributed to and `lastModifiedAt` the instant it happened — both absent on a
//!    profile nothing has updated yet.
//! 3. The reporting members are DERIVED from the profile as it now stands, not from the
//!    create: an update that leaves the profile with no scope to analyse reports
//!    `eligibleFor…` false and a `fieldErrors` entry for the member at fault, and one that
//!    gives it a scope again reports them eligible with no `fieldErrors`.
//! 4. The update is persisted: `GetAgentProfile` of the ARN afterwards reports exactly
//!    what the update answered, and every OTHER stored profile is untouched.
//! 5. The stored `deletionProtection` is what the delete reads, so an update that clears
//!    it lets the profile be deleted and one that sets it refuses the delete.
//! 6. An `AgentProfileArn` is RESOLVED against the store: one naming no stored profile is
//!    a `404 ResourceNotFoundException` carrying the ARN as sent, and it stores nothing.
//! 7. `clientToken` makes the update idempotent: the same token with the same parameters
//!    applies nothing a second time — it answers the profile as it stands and records no
//!    second modification — while the same token with other parameters is the update it
//!    is. A delete drops the tokens that resolved to the profile.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`.

use std::time::SystemTime;

use serde_json::{json, Value};

use super::agent_profile_lifecycle::delete_profile;
use super::workload_harness::{encode_path_segment, Answer, Client};

/// `CreateAgentProfile`: `POST /api/v1/agent-profiles`; `UpdateAgentProfile`:
/// `PUT /api/v1/agent-profiles/{profileArn}`
/// (`smithy_query(query="operation", operation="UpdateAgentProfile")`).
const PROFILES_PATH: &str = "/api/v1/agent-profiles";

/// Every value the `Pillar` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="Pillar")`). `Pillars` is `@length(1, 5)` with
/// `uniqueItems`, so this is also the longest list an update may carry.
const PILLARS: [&str; 5] = [
    "COST_OPTIMIZATION",
    "SECURITY",
    "RESILIENCE",
    "PERFORMANCE",
    "OPERATIONAL_EXCELLENCE",
];

/// The Region the harness signs every request for, which is the Region a minted
/// `AgentProfileArn` carries.
const REQUEST_REGION: &str = "us-west-2";

/// The member `fieldErrors` keys a generation-eligibility finding under
/// (`UpdateAgentProfileResponse$fieldErrors`, "a map of field paths to error messages").
const AGGREGATION_CONFIGURATION: &str = "aggregationConfiguration";

/// A `RoleArn` the modelled pattern accepts
/// (`^arn:([a-z\-]+):iam::\d{12}:role/(service-role/)?[a-zA-Z0-9+=,.@\-_]+$`), in the
/// emulator's one account.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One `AggregationConfiguration` — "which Amazon Web Services accounts and Regions to
/// analyze" (`smithy_query(query="shape", shape="AggregationConfiguration")`).
fn aggregation(account_id: &str, regions: &[&str], role: &str) -> Value {
    json!({
        "accountId": account_id,
        "regions": regions,
        "accessRoleArn": role_arn(role),
    })
}

/// The profile every row starts from: every member an update can change is set to
/// something recognisable, so a row that omits one proves the stored value survived.
fn base_profile(name: &str) -> Value {
    json!({
        "name": name,
        "displayName": "Display name before the update",
        "description": "the description before the update",
        "businessOverview": "the business overview before the update",
        "pillars": ["SECURITY", "COST_OPTIMIZATION"],
        "executionRoleArn": role_arn("ExecutionBeforeTheUpdate"),
        "aggregationConfiguration": [
            aggregation(engine::emulator_account_id(), &["us-east-1"], "AccessBeforeTheUpdate")
        ],
        "deletionProtection": true,
        "tags": [{ "key": "Env", "value": "test" }],
    })
}

/// `request` with every member of `overrides` applied, so a row states only what it
/// varies.
fn with_overrides(mut request: Value, overrides: &Value) -> Value {
    for (name, value) in overrides.as_object().expect("overrides are an object") {
        request[name] = value.clone();
    }
    request
}

async fn create_profile(client: &Client, body: Value) -> Answer {
    client.send("POST", PROFILES_PATH, Some(body)).await
}

/// `PUT /api/v1/agent-profiles/{profileArn}`: the ARN is a non-greedy `@httpLabel`, so it
/// travels percent-encoded in one path segment, exactly as an SDK sends it.
///
/// `pub(crate)` because an agent profile is changed by exactly ONE operation, so the suite
/// about the update that supplies no member (`super::agent_no_op_updates`) sends its
/// requests through this builder rather than building the same request again.
pub(crate) async fn update_profile(client: &Client, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "PUT",
            &format!("{PROFILES_PATH}/{}", encode_path_segment(profile_arn)),
            Some(body),
        )
        .await
}

async fn get_profile(client: &Client, profile_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!("{PROFILES_PATH}/{}", encode_path_segment(profile_arn)),
            None,
        )
        .await
}

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A well-formed `AgentProfileArn` (the modelled
/// `^arn:aws([a-z0-9-]+)?:wellarchitected:[a-z0-9-]{6,64}:\d{12}:agent-profile/([a-zA-Z0-9_-]+)$`)
/// for `resource` in `region`.
fn unresolvable_arn(region: &str, resource: &str) -> String {
    format!(
        "arn:aws:wellarchitected:{region}:{}:agent-profile/{resource}",
        engine::emulator_account_id()
    )
}

/// The `arn` a successful `CreateAgentProfile` answered with, with the create asserted.
fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentProfile 200, got {answer:?}"
    );
    member(answer, "arn").to_string()
}

/// What the store holds as the profile's `lastModifiedAt`, read through the shared state so
/// two updates can be ordered without parsing a wire timestamp.
fn stored_last_modified_at(client: &Client, profile_arn: &str) -> Option<SystemTime> {
    client
        .store()
        .lock()
        .agent_profiles
        .get(profile_arn)
        .expect("the profile under test must be stored")
        .last_modified_at
}

/// The response an update of `created` with `overrides` must carry, apart from
/// `lastModifiedAt` and `fieldErrors` (asserted on their own): the created profile with
/// the request's members applied, the eligibility members recomputed from the scope the
/// profile is left with, and the modification attributed to the emulator's one account.
fn expected_body(created: &Value, overrides: &Value) -> Value {
    let mut expected = with_overrides(created.clone(), overrides);
    // `UpdateAgentProfileResponse` declares no `tags` member, while the create's answer
    // this expectation starts from does: the update neither reports nor changes them.
    expected
        .as_object_mut()
        .expect("the expectation is an object")
        .remove("tags");
    let eligible = expected
        .get(AGGREGATION_CONFIGURATION)
        .and_then(Value::as_array)
        .is_some_and(|configurations| !configurations.is_empty());
    expected["eligibleForScheduledGeneration"] = json!(eligible);
    expected["eligibleForArchitectureGeneration"] = json!(eligible);
    expected["lastModifiedBy"] = json!(engine::emulator_account_id());
    expected
}

/// Which JSON kind a member was rendered as, so a timestamp assertion can tie
/// `lastModifiedAt` to the way the same response renders `createdAt` without pinning one
/// wire form.
fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Assert the update's answer against rules 1, 2 and 3.
fn assert_updated_body(created: &Value, overrides: &Value, updated: &Answer, case: &str) {
    assert_eq!(
        updated.status, 200,
        "case {case:?}: expected UpdateAgentProfile 200, got {updated:?}"
    );

    let mut actual = updated.body.clone();
    let mut expected = expected_body(created, overrides);
    let eligible = expected
        .get("eligibleForScheduledGeneration")
        .and_then(Value::as_bool)
        == Some(true);

    // `fieldErrors` is derived from the profile the update LEFT behind, so it is asserted
    // on its content rather than compared to a message this test spells out.
    let field_errors = actual
        .as_object_mut()
        .expect("the response is an object")
        .remove("fieldErrors");
    expected
        .as_object_mut()
        .expect("the expectation is an object")
        .remove("fieldErrors");
    match eligible {
        true => assert!(
            field_errors.is_none(),
            "case {case:?}: a profile with a scope to analyse must report no fieldErrors, \
             got {field_errors:?}"
        ),
        false => {
            let errors = field_errors.clone().unwrap_or(Value::Null);
            assert!(
                errors
                    .get(AGGREGATION_CONFIGURATION)
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: a profile with no scope to analyse must report a \
                 {AGGREGATION_CONFIGURATION} fieldError, got {field_errors:?}"
            );
            assert_eq!(
                errors.as_object().map(serde_json::Map::len),
                Some(1),
                "case {case:?}: only the member at fault is reported, got {field_errors:?}"
            );
        }
    }

    // `lastModifiedAt` is the clock's, so the assertion is that the update recorded one,
    // rendered the way this service's other `Timestamp` members are.
    let last_modified_at = actual
        .as_object_mut()
        .expect("the response is an object")
        .remove("lastModifiedAt");
    let created_at = actual.get("createdAt").cloned().unwrap_or(Value::Null);
    assert!(
        last_modified_at
            .as_ref()
            .is_some_and(|instant| !instant.is_null()),
        "case {case:?}: an update must record lastModifiedAt, got {last_modified_at:?}"
    );
    assert!(
        last_modified_at.as_ref().map(value_kind) == Some(value_kind(&created_at)),
        "case {case:?}: lastModifiedAt must be rendered as createdAt is ({}), got \
         {last_modified_at:?}",
        value_kind(&created_at)
    );

    assert_eq!(
        actual, expected,
        "case {case:?}: expected the merged profile {expected:#}, got {:#}",
        updated.body
    );
}

/// Every row of the update table: the case name and the members the `UpdateAgentProfile`
/// request carries. The rows differ along WHICH members are sent, how many values each
/// carries and whether the profile is left with a scope to analyse — because the rule is
/// about the members a request carries, not about one particular request. Every row sends
/// at least one writable member, so every row records a modification; a request that sends
/// NONE of them asks for no change and is pinned by `super::agent_no_op_updates` instead.
fn update_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "display-name-only",
            json!({ "displayName": "Display name after the update" }),
        ),
        (
            "description-only",
            json!({ "description": "the description after the update" }),
        ),
        (
            "business-overview-only",
            json!({ "businessOverview": "the business overview after the update" }),
        ),
        // `displayName` is `@length(3, 128)`: each published bound, and one past the
        // minimum is refused by the boundary rather than this handler.
        (
            "display-name-at-the-minimum-length",
            json!({ "displayName": "abc" }),
        ),
        (
            "display-name-at-the-maximum-length",
            json!({ "displayName": "d".repeat(128) }),
        ),
        // `Pillars` is `@length(1, 5)` with `uniqueItems`: one pillar is the published
        // minimum, the whole `Pillar` value set the maximum.
        ("one-pillar", json!({ "pillars": ["OPERATIONAL_EXCELLENCE"] })),
        ("every-pillar", json!({ "pillars": PILLARS })),
        (
            "execution-role-only",
            json!({ "executionRoleArn": role_arn("ExecutionAfterTheUpdate") }),
        ),
        // `AggregationConfigurations` is `@length(0, 100)`, so an update may leave a
        // profile with no scope at all, one account, or several.
        (
            "no-aggregation-configuration",
            json!({ "aggregationConfiguration": [] }),
        ),
        (
            "two-aggregation-configurations",
            json!({
                "aggregationConfiguration": [
                    aggregation("123456789012", &["us-east-1", "eu-west-1"], "AccessOne"),
                    aggregation("210987654321", &["ap-southeast-2"], "AccessTwo"),
                ],
            }),
        ),
        (
            "deletion-protection-cleared",
            json!({ "deletionProtection": false }),
        ),
        (
            "every-updatable-member",
            json!({
                "displayName": "Every member after the update",
                "description": "every member after the update",
                "businessOverview": "every business overview after the update",
                "pillars": ["RESILIENCE", "PERFORMANCE", "SECURITY"],
                "executionRoleArn": role_arn("EveryMemberExecution"),
                "aggregationConfiguration": [
                    aggregation("123456789012", &["eu-central-1"], "EveryMemberAccess")
                ],
                "deletionProtection": false,
            }),
        ),
        // A token is carried by a request that also changes something: the replay rules
        // are pinned on their own below.
        (
            "with-a-client-token",
            json!({
                "clientToken": "tok-update-table",
                "description": "updated under an idempotency token",
            }),
        ),
    ]
}

/// Rules 1, 2, 3 and 4.
#[tokio::test]
async fn an_update_merges_its_members_into_the_stored_profile_and_persists_them() {
    let client = Client::new();

    // A bystander profile, created first and never updated: every row must leave it
    // reporting exactly what it did, so an update touches ONE profile.
    let bystander = created_arn(
        &create_profile(&client, base_profile("bystander-profile")).await,
        "bystander",
    );
    let bystander_body = get_profile(&client, &bystander).await.body;

    for (case, request) in update_cases() {
        let created = create_profile(&client, base_profile(&format!("updated-{case}"))).await;
        let arn = created_arn(&created, case);

        // Rule 2: nothing has modified a freshly created profile.
        for absent in ["lastModifiedBy", "lastModifiedAt"] {
            assert!(
                created.body.get(absent).is_none(),
                "case {case:?}: a created profile must report no {absent}, got {created:?}"
            );
        }

        let updated = update_profile(&client, &arn, request.clone()).await;
        // `clientToken` is an input-only member: the response does not carry it, so the
        // expectation is built from the members that CHANGE the profile.
        let mut overrides = request.clone();
        overrides
            .as_object_mut()
            .expect("the request is an object")
            .remove("clientToken");
        assert_updated_body(&created.body, &overrides, &updated, case);

        // Rule 4: the update is persisted, so the read reports exactly what it answered —
        // through the read's own shape, which declares two members the update's does not:
        // the `tags` the create recorded and the `profileType` the profile was minted as.
        let fetched = get_profile(&client, &arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the updated profile must be readable, got {fetched:?}"
        );
        let mut read_back = fetched.body.clone();
        let read_only = read_back
            .as_object_mut()
            .expect("the answer is an object");
        let read_tags = read_only.remove("tags");
        let read_profile_type = read_only.remove("profileType");
        assert_eq!(
            read_back, updated.body,
            "case {case:?}: GetAgentProfile must report what the update answered"
        );
        assert_eq!(
            read_tags,
            created.body.get("tags").cloned(),
            "case {case:?}: an update leaves the tags the create recorded, got {read_tags:?}"
        );
        assert_eq!(
            read_profile_type.as_ref().and_then(Value::as_str),
            Some("STANDARD"),
            "case {case:?}: the profile keeps the kind it was minted as, \
             got {read_profile_type:?}"
        );

        let still_there = get_profile(&client, &bystander).await;
        assert_eq!(
            still_there.body, bystander_body,
            "case {case:?}: updating one profile must leave the others exactly as they were"
        );
    }
}

/// Rule 2's ordering half: a second update records a LATER modification instant than the
/// first, whatever each of them changes.
#[tokio::test]
async fn each_update_records_its_own_modification_instant() {
    let client = Client::new();

    let arn = created_arn(
        &create_profile(&client, base_profile("sequential-updates")).await,
        "sequential",
    );
    assert_eq!(
        stored_last_modified_at(&client, &arn),
        None,
        "case \"created\": a profile nothing has updated carries no modification instant"
    );

    let mut previous = SystemTime::UNIX_EPOCH;
    for (case, request) in update_cases() {
        assert_eq!(
            update_profile(&client, &arn, request).await.status,
            200,
            "case {case:?}: expected UpdateAgentProfile 200"
        );
        let recorded = stored_last_modified_at(&client, &arn)
            .unwrap_or_else(|| panic!("case {case:?}: the update must record an instant"));
        assert!(
            recorded >= previous,
            "case {case:?}: the recorded instant must not move backwards, got {recorded:?} \
             after {previous:?}"
        );
        previous = recorded;
    }
}

/// Rule 5. The rows differ along WHICH WAY the flag is moved, because the delete reads the
/// stored profile rather than the request that created it.
#[tokio::test]
async fn an_update_of_deletion_protection_decides_whether_the_delete_is_refused() {
    let client = Client::new();

    for (case, created_protected) in [("protected-then-cleared", true), ("cleared-then-set", false)]
    {
        let arn = created_arn(
            &create_profile(
                &client,
                with_overrides(
                    base_profile(&format!("protection-{case}")),
                    &json!({ "deletionProtection": created_protected }),
                ),
            )
            .await,
            case,
        );

        let updated = update_profile(
            &client,
            &arn,
            json!({ "deletionProtection": !created_protected }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateAgentProfile 200, got {updated:?}"
        );
        assert_eq!(
            updated.body.get("deletionProtection").and_then(Value::as_bool),
            Some(!created_protected),
            "case {case:?}: the update must report the flag it stored, got {updated:?}"
        );

        let deleted = delete_profile(&client, &arn).await;
        match created_protected {
            // Protection was cleared, so the delete goes through.
            true => assert_eq!(
                deleted.status, 200,
                "case {case:?}: a profile whose protection was cleared must delete, \
                 got {deleted:?}"
            ),
            // Protection was set, so the delete is refused and the profile stays.
            false => {
                assert_eq!(
                    deleted.status, 409,
                    "case {case:?}: a profile whose protection was set must refuse the \
                     delete, got {deleted:?}"
                );
                assert_eq!(
                    deleted.error_code, "ConflictException",
                    "case {case:?}: expected ConflictException, got {deleted:?}"
                );
                assert_eq!(
                    get_profile(&client, &arn).await.status,
                    200,
                    "case {case:?}: a refused delete must leave the profile readable"
                );
            }
        }
    }
}

/// Rule 6. The rows differ along the SHAPE of the ARN's resource part and the Region it
/// names — every one satisfying the modelled `AgentProfileArn` pattern, so each reaches
/// the lookup rather than the boundary — and along what the update would have changed.
#[tokio::test]
async fn updating_an_unresolvable_profile_arn_is_a_not_found_that_stores_nothing() {
    let client = Client::new();

    // One profile exists, so the 404s below are a real miss and not an empty store.
    let present = created_arn(
        &create_profile(&client, base_profile("present-profile")).await,
        "present",
    );
    let present_body = get_profile(&client, &present).await.body;

    // A deleted profile's ARN resolves to nothing too.
    let deleted = created_arn(
        &create_profile(
            &client,
            with_overrides(
                base_profile("deleted-profile"),
                &json!({ "deletionProtection": false }),
            ),
        )
        .await,
        "deleted",
    );
    assert_eq!(
        delete_profile(&client, &deleted).await.status,
        200,
        "case \"deleted\": the profile under test must delete cleanly"
    );

    let cases: Vec<(&str, String, Value)> = vec![
        (
            "hex-resource-part",
            unresolvable_arn(REQUEST_REGION, "0123456789abcdef0123456789abcdef"),
            json!({ "displayName": "never stored" }),
        ),
        (
            "name-shaped-resource-part",
            unresolvable_arn(REQUEST_REGION, "no_such-profile"),
            json!({ "pillars": PILLARS }),
        ),
        // The same resource part in another Region names a DIFFERENT profile.
        (
            "another-region",
            unresolvable_arn("eu-central-1", "0123456789abcdef0123456789abcdef"),
            json!({}),
        ),
        (
            "deleted-profile",
            deleted.clone(),
            json!({ "deletionProtection": true }),
        ),
    ];

    for (case, arn, request) in cases {
        assert_ne!(
            arn, present,
            "case {case:?}: the row must not name the profile that exists"
        );
        let refused = update_profile(&client, &arn, request).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceId"),
            arn,
            "case {case:?}: ResourceId must be the ARN as sent, got {refused:?}"
        );
        assert!(
            member(&refused, "Message").contains(&arn),
            "case {case:?}: the refusal must name the ARN as sent, got {refused:?}"
        );

        // A refused update stores nothing: the ARN still resolves to no profile, and the
        // one profile that does exist is untouched.
        let store = client.store();
        assert!(
            !store.lock().agent_profiles.contains_key(&arn),
            "case {case:?}: a refused update must not store a profile under {arn:?}"
        );
        assert_eq!(
            store.lock().agent_profiles.len(),
            1,
            "case {case:?}: a refused update must not change how many profiles are stored"
        );
        assert_eq!(
            get_profile(&client, &present).await.body,
            present_body,
            "case {case:?}: a refused update must leave the stored profile alone"
        );
    }
}

/// Rule 7.
#[tokio::test]
async fn a_client_token_makes_the_update_idempotent() {
    let client = Client::new();

    let arn = created_arn(
        &create_profile(
            &client,
            with_overrides(
                base_profile("idempotent-updates"),
                &json!({ "deletionProtection": false }),
            ),
        )
        .await,
        "created",
    );

    let token = "tok-update-idempotent";
    // The rows differ along WHAT the token's request changes, because the replay rule is
    // about the token and the parameters, not about one member.
    let cases: Vec<(&str, Value)> = vec![
        (
            "one-member",
            json!({ "clientToken": token, "displayName": "Display name under the token" }),
        ),
        (
            "several-members",
            json!({
                "clientToken": token,
                "description": "description under the token",
                "pillars": ["RESILIENCE"],
                "aggregationConfiguration": [],
            }),
        ),
        ("no-member", json!({ "clientToken": token })),
    ];

    for (case, request) in cases {
        let first = update_profile(&client, &arn, request.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected UpdateAgentProfile 200, got {first:?}"
        );
        let recorded = stored_last_modified_at(&client, &arn);
        assert!(
            client
                .store()
                .lock()
                .update_agent_profile_tokens
                .get(token)
                .is_some(),
            "case {case:?}: the update must remember the token it was spent under"
        );

        // The same token with the same parameters applies nothing a second time: the
        // answer is the profile as it stands, down to the modification instant the
        // original request recorded.
        let replay = update_profile(&client, &arn, request).await;
        assert_eq!(
            replay.body, first.body,
            "case {case:?}: a replayed update must answer the profile unchanged"
        );
        assert_eq!(
            stored_last_modified_at(&client, &arn),
            recorded,
            "case {case:?}: a replayed update must record no second modification"
        );

        // The SAME token carrying other parameters is the update it is.
        let changed = update_profile(
            &client,
            &arn,
            json!({ "clientToken": token, "displayName": format!("Display name for {case}") }),
        )
        .await;
        assert_eq!(
            changed.status, 200,
            "case {case:?}: expected UpdateAgentProfile 200, got {changed:?}"
        );
        assert_eq!(
            changed.body.get("displayName").and_then(Value::as_str),
            Some(format!("Display name for {case}").as_str()),
            "case {case:?}: a token carrying other parameters must apply them, got {changed:?}"
        );
        assert_ne!(
            stored_last_modified_at(&client, &arn),
            None,
            "case {case:?}: the applied update must record a modification"
        );
    }

    // Nothing outlives the profile it belongs to: the delete drops the update tokens that
    // resolved to it.
    assert_eq!(
        delete_profile(&client, &arn).await.status,
        200,
        "case \"deleted\": the profile under test must delete cleanly"
    );
    assert!(
        client
            .store()
            .lock()
            .update_agent_profile_tokens
            .get(token)
            .is_none(),
        "case \"deleted\": the delete must drop the update token that resolved to the profile"
    );
}

/// The boundary's own rule, pinned because the handler must never store a pillar the
/// `Pillar` enum does not publish: a value outside the published set is a `400
/// ValidationException` and the stored profile is left alone.
#[tokio::test]
async fn an_unpublished_pillar_is_refused_and_changes_nothing() {
    let client = Client::new();

    let arn = created_arn(
        &create_profile(&client, base_profile("pillar-validation")).await,
        "created",
    );
    let before = get_profile(&client, &arn).await.body;

    for (case, pillars) in [
        ("unknown-value", json!(["NOT_A_PILLAR"])),
        ("lowercase-value", json!(["security"])),
        ("one-published-one-not", json!(["SECURITY", "COST"])),
    ] {
        let refused = update_profile(&client, &arn, json!({ "pillars": pillars })).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            get_profile(&client, &arn).await.body,
            before,
            "case {case:?}: a refused update must leave the stored profile unchanged"
        );
    }
}
