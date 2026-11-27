// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AppIntegrations data-integration lifecycle:
//! `CreateDataIntegration`, `GetDataIntegration`, `ListDataIntegrations` and
//! `DeleteDataIntegration`.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), so a response that is not computed
//! from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (name form, description length, source-URI scheme, tag count,
//! configuration presence, identifier form, `KmsKey` form and resolvability, page
//! size), and the case the issue recorded is only one row.

use serde_json::{json, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `MaxResults` is `range 1..50` on `ListDataIntegrations` (model:
/// `smithy_query(query="operation", operation="ListDataIntegrations")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// `Description` is `len 0..1000` on `CreateDataIntegrationRequest` (same query).
const DESCRIPTION_MAX: usize = 1000;

/// Send one DataIntegration request.
async fn create(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/dataIntegrations", REGION, Some(body))
        .await
}

async fn get(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/dataIntegrations/{}", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

async fn delete(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!("/dataIntegrations/{}", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

async fn list(emulator: &Emulator, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/dataIntegrations".to_string()
    } else {
        format!("/dataIntegrations?{query}")
    };
    emulator.send("GET", &uri, REGION, None).await
}

/// One row of the round-trip table: a request body, and what makes it different from
/// the others.
struct Case {
    label: &'static str,
    name: String,
    body: Value,
}

/// The table every round-trip assertion runs over. Rows differ in description presence
/// and length (including the published upper bound), source-URI scheme, tag count and
/// which optional configuration blocks the request carries.
fn round_trip_cases(kms_key: &str) -> Vec<Case> {
    let rows: Vec<(&'static str, &'static str, Value)> = vec![
        (
            "minimal: only the required members",
            "minimal.integration",
            json!({}),
        ),
        (
            "an s3 source, a description and two tags",
            "s3-source_integration",
            json!({
                "Description": "pulls the account feed",
                "SourceURI": "s3://feed-bucket/accounts",
                "Tags": { "team": "crm", "tier": "gold" },
            }),
        ),
        (
            "an https source with a schedule and a file configuration",
            "scheduled/integration",
            json!({
                "Description": "x",
                "SourceURI": "https://example.com/feed",
                "ScheduleConfig": {
                    "ScheduleExpression": "rate(1 hour)",
                    "FirstExecutionFrom": "1685400000",
                    "Object": "Account",
                },
                "FileConfiguration": {
                    "Folders": ["inbox", "archive"],
                    "Filters": { "Account": ["Id", "Name"] },
                },
            }),
        ),
        (
            "a description at its published upper bound and an object configuration",
            "object-config-integration",
            json!({
                "Description": "d".repeat(DESCRIPTION_MAX),
                "SourceURI": "salesforce://my.instance/data",
                "ObjectConfiguration": {
                    "Customer": { "Account": ["Id", "Name"] },
                },
            }),
        ),
    ];

    rows.into_iter()
        .map(|(label, name, extra)| {
            let mut body = json!({ "Name": name, "KmsKey": kms_key });
            let object = body.as_object_mut().expect("the body is a JSON object");
            for (member, value) in extra.as_object().expect("the extras are an object") {
                object.insert(member.clone(), value.clone());
            }
            Case {
                label,
                name: name.to_string(),
                body,
            }
        })
        .collect()
}

/// Every member the request carried, plus the ones the service minted, as the reply
/// reports them.
fn assert_reports_request(label: &str, reply: &Reply, case: &Case, kms_key: &str) {
    let request = case.body.as_object().expect("the body is a JSON object");
    assert_eq!(
        reply.string("Name").as_deref(),
        Some(case.name.as_str()),
        "case {label:?}: expected the stored Name, got {:?}",
        reply.field("Name")
    );
    assert_eq!(
        reply.string("KmsKey").as_deref(),
        Some(kms_key),
        "case {label:?}: expected the accepted KmsKey echoed back, got {:?}",
        reply.field("KmsKey")
    );
    // Each optional member is reported when the request carried it, and absent when it
    // did not — `ScheduleConfig` on the way in is `ScheduleConfiguration` on the way out.
    for (request_member, reported_member) in [
        ("Description", "Description"),
        ("SourceURI", "SourceURI"),
        ("ScheduleConfig", "ScheduleConfiguration"),
        ("FileConfiguration", "FileConfiguration"),
        ("ObjectConfiguration", "ObjectConfiguration"),
    ] {
        assert_eq!(
            reply.field(reported_member),
            request.get(request_member),
            "case {label:?}: expected {reported_member} to report what the request's \
             {request_member} carried, got {:?}",
            reply.field(reported_member)
        );
    }
    // `Tags` is a member the SERVICE fills in: an untagged integration reports the empty
    // map rather than no member at all.
    let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!({}));
    assert_eq!(
        reply.field("Tags"),
        Some(&expected_tags),
        "case {label:?}: expected Tags {expected_tags}, got {:?}",
        reply.field("Tags")
    );

    let id = reply
        .string("Id")
        .unwrap_or_else(|| panic!("case {label:?}: expected an Id, got {}", reply.body));
    assert!(
        is_uuid_shaped(&id),
        "case {label:?}: expected Id to be a UUID (model: UUID, \
         pattern ^[a-f0-9]{{8}}-[a-f0-9]{{4}}-[a-f0-9]{{4}}-[a-f0-9]{{4}}-[a-f0-9]{{12}}$), \
         got {id:?}"
    );
    assert_eq!(
        reply.string("Arn"),
        Some(format!(
            "arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration/{id}"
        )),
        "case {label:?}: expected the ARN built from the minted Id, got {:?}",
        reply.field("Arn")
    );
}

/// Whether `value` has the shape the model's `UUID` pattern publishes:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_uuid_shaped(value: &str) -> bool {
    let group_lengths: Vec<usize> = value.split('-').map(str::len).collect();
    group_lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f' | '-'))
}

#[tokio::test]
async fn a_created_data_integration_is_reported_by_get_and_list_exactly_as_stored() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    for case in round_trip_cases(&kms_key) {
        let label = case.label;
        let created = create(&emulator, case.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateDataIntegration, got {} / {}",
            created.status, created.body
        );
        assert_reports_request(&format!("{label} (create)"), &created, &case, &kms_key);

        let id = created.string("Id").expect("the create reported an Id");
        let arn = created.string("Arn").expect("the create reported an ARN");

        // A DataIntegration is addressed by the Id the service minted or by the ARN
        // built from it, and both forms must report the same resource.
        for (form, identifier) in [("by id", id.clone()), ("by arn", arn.clone())] {
            let fetched = get(&emulator, &identifier).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?} {form}: expected HTTP 200 from GetDataIntegration, got {} / {}",
                fetched.status, fetched.body
            );
            assert_reports_request(&format!("{label} (get {form})"), &fetched, &case, &kms_key);
            // A read reports the members the model declares and no others: the kind of
            // integration this service assigns is persisted, but
            // `GetDataIntegrationResponse` declares no member for it
            // (`smithy_query(query="operation", operation="GetDataIntegration")`), so a
            // schema-driven response carries none.
            assert_eq!(
                fetched.field("IntegrationType"),
                None,
                "case {label:?} {form}: GetDataIntegrationResponse models no \
                 IntegrationType, got {:?}",
                fetched.field("IntegrationType")
            );
            assert_eq!(
                fetched.string("Id"),
                Some(id.clone()),
                "case {label:?} {form}: expected the same Id the create minted, got {:?}",
                fetched.field("Id")
            );
            // `GetDataIntegration` reports no ClientToken: it is not a member of
            // `GetDataIntegrationResponse`.
            assert_eq!(
                fetched.field("ClientToken"),
                None,
                "case {label:?} {form}: GetDataIntegrationResponse models no ClientToken, \
                 got {:?}",
                fetched.field("ClientToken")
            );
        }

        let listed = list(&emulator, "").await;
        let summaries = listed.field("DataIntegrations").cloned().unwrap_or(json!([]));
        let summary = summaries
            .as_array()
            .expect("DataIntegrations is a list")
            .iter()
            .find(|entry| entry.get("Arn") == Some(&json!(arn)))
            .unwrap_or_else(|| panic!("case {label:?}: expected {arn} in {summaries}"))
            .clone();
        let mut expected_summary = json!({ "Arn": arn, "Name": case.name });
        if let Some(source_uri) = case.body.get("SourceURI") {
            expected_summary
                .as_object_mut()
                .expect("the summary is a JSON object")
                .insert("SourceURI".to_string(), source_uri.clone());
        }
        assert_eq!(
            summary, expected_summary,
            "case {label:?}: expected the summary DataIntegrationSummary models \
             (Arn, Name, SourceURI), got {summary}"
        );
    }
}

#[tokio::test]
async fn a_kms_key_kms_cannot_resolve_is_refused_and_nothing_is_persisted() {
    let emulator = Emulator::new();
    let real_arn = emulator.create_kms_key(REGION).await;
    let real_id = real_arn
        .rsplit_once('/')
        .expect("a key ARN ends in key/<id>")
        .1
        .to_string();

    // Rows differ in the FORM of the key reference and in whether KMS can resolve it:
    // every form `KeyId` accepts (a key ARN, a bare key id, `alias/<name>`) and a value
    // that is not a KMS ARN at all. The accepted column is what KMS answers for it.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a key ARN KMS resolves", real_arn.clone(), true),
        ("the bare id of that key", real_id, true),
        (
            "a well-formed key ARN naming no key",
            format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/00000000-0000-0000-0000-000000000000"),
            false,
        ),
        (
            "a well-formed key ARN in this account naming no key",
            format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"),
            false,
        ),
        (
            "an alias no key carries",
            "alias/no-such-key".to_string(),
            false,
        ),
        ("a bare id no key carries", "not-a-key-id".to_string(), false),
        (
            "an ARN that is not a KMS ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/keys"),
            false,
        ),
    ];

    for (index, (label, kms_key, accepted)) in rows.iter().enumerate() {
        let name = format!("kms-case-{index}");
        let reply = create(
            &emulator,
            json!({ "Name": name, "KmsKey": kms_key, "SourceURI": "s3://feed-bucket/accounts" }),
        )
        .await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 for a key KMS resolves, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.string("KmsKey").as_deref(),
                Some(kms_key.as_str()),
                "case {label:?}: expected the accepted KmsKey echoed back, got {:?}",
                reply.field("KmsKey")
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {label:?}: expected HTTP 400 for a key KMS cannot resolve, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "InvalidRequestException",
                "case {label:?}: expected InvalidRequestException, got {:?} / {}",
                reply.error_type, reply.body
            );
        }

        // Accepted or refused, the store must agree with the answer: a refused create
        // persists nothing, so the name it asked for is reported by no reader.
        let listed = list(&emulator, "").await;
        let names: Vec<String> = listed
            .field("DataIntegrations")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names.contains(&name),
            *accepted,
            "case {label:?}: expected ListDataIntegrations to report {name:?} == {accepted}, \
             got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_lookup_that_says_nothing_about_the_key_does_not_refuse_the_create() {
    // KMS is not mounted here, so the handler's in-process `DescribeKey` is answered by
    // nothing that spoke about the key: the create must go through rather than report a
    // key nobody called bad. The resource is then readable, so the accept is real.
    let emulator = Emulator::without_kms();

    // Rows differ in the key's form, so the rule is about the ANSWER and not about the
    // value: a key ARN, a bare id, an alias.
    let keys = [
        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/00000000-0000-0000-0000-000000000000"),
        "22222222-3333-4444-5555-666666666666".to_string(),
        "alias/unknowable".to_string(),
    ];

    for (index, kms_key) in keys.iter().enumerate() {
        let name = format!("indeterminate-{index}");
        let created = create(
            &emulator,
            json!({ "Name": name, "KmsKey": kms_key, "SourceURI": "s3://feed-bucket/accounts" }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {kms_key:?}: a lookup that answered nothing about the key must not \
             refuse the create, got {} / {}",
            created.status, created.body
        );
        let id = created.string("Id").expect("the create reported an Id");
        let fetched = get(&emulator, &id).await;
        assert_eq!(
            fetched.string("KmsKey").as_deref(),
            Some(kms_key.as_str()),
            "case {kms_key:?}: the accepted key must be readable back, got {:?}",
            fetched.field("KmsKey")
        );
    }
}

#[tokio::test]
async fn a_store_at_its_ceiling_refuses_another_create_until_one_is_deleted() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // The emulator's own ceiling on this in-memory store, read from the crate rather
    // than restated, so the test follows the constant the handler checks.
    let ceiling = service_com_amazonaws_appintegrations_amazonappintegrationservice::state::MAX_STORED_DATA_INTEGRATIONS;

    let mut first_id = None;
    for index in 0..ceiling {
        let reply = create(
            &emulator,
            json!({ "Name": format!("ceiling-{index:05}"), "KmsKey": kms_key }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {index}: a create below the ceiling must succeed, got {} / {}",
            reply.status, reply.body
        );
        if first_id.is_none() {
            first_id = reply.string("Id");
        }
    }

    // One past the ceiling: refused with the quota error the operation models, and
    // nothing is persisted for it.
    let refused = create(
        &emulator,
        json!({ "Name": "one-too-many", "KmsKey": kms_key }),
    )
    .await;
    assert_eq!(
        refused.status, 429,
        "case one past the ceiling: expected HTTP 429, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ResourceQuotaExceededException",
        "case one past the ceiling: expected ResourceQuotaExceededException, got {:?}",
        refused.error_type
    );

    // A delete frees a slot, so the ceiling bounds the store rather than ending its
    // usefulness.
    let freed = first_id.expect("the first create reported an Id");
    let removed = delete(&emulator, &freed).await;
    assert_eq!(
        removed.status, 200,
        "case freeing a slot: expected HTTP 200, got {} / {}",
        removed.status, removed.body
    );
    let accepted = create(
        &emulator,
        json!({ "Name": "one-too-many", "KmsKey": kms_key }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case after a delete: expected HTTP 200, got {} / {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn an_arn_this_service_did_not_mint_names_no_data_integration() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;
    let created = create(
        &emulator,
        json!({ "Name": "arn-forms", "KmsKey": kms_key }),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");
    let arn = created.string("Arn").expect("the create reported an ARN");

    // Rows differ in WHICH field of the stored ARN is wrong while the trailing id is
    // the right one: only the ARN this service minted addresses the resource.
    let foreign: Vec<(&str, String)> = vec![
        (
            "another account",
            format!("arn:aws:app-integrations:{REGION}:111122223333:data-integration/{id}"),
        ),
        (
            "another region",
            format!("arn:aws:app-integrations:eu-west-1:{ACCOUNT}:data-integration/{id}"),
        ),
        (
            "another vendor",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:data-integration/{id}"),
        ),
        (
            "another resource type",
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:event-integration/{id}"),
        ),
        ("not an ARN at all", format!("foo/{id}")),
    ];

    for (label, identifier) in &foreign {
        for (operation, reply) in [
            ("GetDataIntegration", get(&emulator, identifier).await),
            ("DeleteDataIntegration", delete(&emulator, identifier).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {label:?} ({operation}): expected HTTP 404 for an identifier this \
                 service never handed out, got {} / {}",
                reply.status, reply.body
            );
        }
    }

    // And none of those calls touched the resource the real identifiers name.
    for (form, identifier) in [("by id", id.clone()), ("by arn", arn)] {
        let fetched = get(&emulator, &identifier).await;
        assert_eq!(
            fetched.status, 200,
            "case {form:?}: the stored integration must survive, got {} / {}",
            fetched.status, fetched.body
        );
    }
}

#[tokio::test]
async fn a_second_create_of_a_live_name_is_refused_as_a_duplicate() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    for name in ["duplicate.one", "d", &"n".repeat(255)] {
        let body = json!({ "Name": name, "KmsKey": kms_key });
        let first = create(&emulator, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: expected the first create to succeed, got {} / {}",
            first.status, first.body
        );

        let second = create(&emulator, body).await;
        assert_eq!(
            second.status, 409,
            "case {name:?}: expected HTTP 409 for a duplicate name, got {} / {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_type, "DuplicateResourceException",
            "case {name:?}: expected DuplicateResourceException, got {:?}",
            second.error_type
        );
    }
}

#[tokio::test]
async fn a_create_repeated_with_its_client_token_reports_the_resource_it_already_minted() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the token's form and length, including the published bounds of
    // `ClientToken` (len 1..2048).
    for token in ["t", "retry-token-1", &"z".repeat(2048)] {
        let body = json!({
            "Name": format!("idempotent-{}", token.len()),
            "KmsKey": kms_key,
            "ClientToken": token,
        });
        let first = create(&emulator, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case token len {}: expected HTTP 200, got {} / {}",
            token.len(),
            first.status,
            first.body
        );

        let replay = create(&emulator, body).await;
        assert_eq!(
            replay.status, 200,
            "case token len {}: expected a retry under the same token to succeed, got {} / {}",
            token.len(),
            replay.status,
            replay.body
        );
        assert_eq!(
            replay.field("Id"),
            first.field("Id"),
            "case token len {}: expected the Id the first call minted, got {:?}",
            token.len(),
            replay.field("Id")
        );
        assert_eq!(
            replay.field("Arn"),
            first.field("Arn"),
            "case token len {}: expected the ARN the first call minted, got {:?}",
            token.len(),
            replay.field("Arn")
        );
    }

    // And exactly one resource exists per token used above.
    let listed = list(&emulator, "").await;
    let count = listed
        .field("DataIntegrations")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        count, 3,
        "expected one resource per distinct ClientToken, got {}",
        listed.body
    );
}

#[tokio::test]
async fn delete_removes_the_data_integration_and_only_that_one() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the identifier form the delete addresses the resource by.
    for form in ["by id", "by arn"] {
        let doomed = create(
            &emulator,
            json!({ "Name": format!("doomed-{}", form.replace(' ', "-")), "KmsKey": kms_key }),
        )
        .await;
        let neighbour = create(
            &emulator,
            json!({ "Name": format!("kept-{}", form.replace(' ', "-")), "KmsKey": kms_key }),
        )
        .await;
        let id = doomed.string("Id").expect("the create reported an Id");
        let arn = doomed.string("Arn").expect("the create reported an ARN");
        let identifier = if form == "by id" { id.clone() } else { arn };

        let removed = delete(&emulator, &identifier).await;
        assert_eq!(
            removed.status, 200,
            "case {form:?}: expected HTTP 200 from DeleteDataIntegration, got {} / {}",
            removed.status, removed.body
        );

        // Gone for reads…
        let fetched = get(&emulator, &identifier).await;
        assert_eq!(
            fetched.status, 404,
            "case {form:?}: expected HTTP 404 after the delete, got {} / {}",
            fetched.status, fetched.body
        );
        // …and for mutations: a second delete finds nothing.
        let again = delete(&emulator, &identifier).await;
        assert_eq!(
            again.status, 404,
            "case {form:?}: expected HTTP 404 from a repeated delete, got {} / {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type, "ResourceNotFoundException",
            "case {form:?}: expected ResourceNotFoundException, got {:?}",
            again.error_type
        );

        // The neighbour is untouched, and the deleted one is out of the listing.
        let listed = list(&emulator, "").await;
        let arns: Vec<String> = listed
            .field("DataIntegrations")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("Arn").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let kept = neighbour.string("Arn").expect("the create reported an ARN");
        assert!(
            arns.contains(&kept),
            "case {form:?}: expected the neighbour {kept} to survive, got {arns:?}"
        );
        assert!(
            !arns.iter().any(|reported| reported.ends_with(&id)),
            "case {form:?}: expected {id} to be gone from the listing, got {arns:?}"
        );

        // Clean the neighbour up, so each row starts from the same store.
        delete(&emulator, &kept).await;
    }
}

#[tokio::test]
async fn pagination_reports_every_stored_data_integration_exactly_once() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    let mut created = Vec::new();
    for index in 0..7 {
        let reply = create(
            &emulator,
            json!({ "Name": format!("page-{index:02}"), "KmsKey": kms_key }),
        )
        .await;
        created.push(reply.string("Arn").expect("the create reported an ARN"));
    }
    created.sort();

    // Page sizes differing in how they divide the store: one at a time, a size that
    // divides it unevenly, one larger than the store, and no `MaxResults` at all.
    for page_size in [Some(1), Some(3), Some(MAX_RESULTS_MAX), None] {
        let mut reported: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = Vec::new();
            if let Some(max) = page_size {
                query.push(format!("maxResults={max}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let reply = list(&emulator, &query.join("&")).await;
            assert_eq!(
                reply.status, 200,
                "case maxResults={page_size:?}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            let page = reply
                .field("DataIntegrations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(max) = page_size {
                assert!(
                    i64::try_from(page.len()).expect("a page fits in an i64") <= max,
                    "case maxResults={max}: a page reported {} entries",
                    page.len()
                );
            }
            reported.extend(
                page.iter()
                    .filter_map(|entry| entry.get("Arn").and_then(Value::as_str))
                    .map(str::to_string),
            );
            token = reply.string("NextToken");
            if token.is_none() {
                break;
            }
        }
        reported.sort();
        assert_eq!(
            reported, created,
            "case maxResults={page_size:?}: expected every stored integration exactly once"
        );
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    // One past each published bound of `MaxResults` (range 1..50), then each bound.
    for (max, expected) in [
        (MAX_RESULTS_MIN - 1, 400),
        (MAX_RESULTS_MAX + 1, 400),
        (MAX_RESULTS_MIN, 200),
        (MAX_RESULTS_MAX, 200),
    ] {
        let reply = list(&emulator, &format!("maxResults={max}")).await;
        assert_eq!(
            reply.status, expected,
            "case maxResults={max}: expected HTTP {expected}, got {} / {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    // Tokens differing in why they are not cursors: not hex at all, odd-length hex,
    // hex that decodes to bytes that are not text.
    for token in ["not-a-cursor", "abc", "fffe"] {
        let reply = list(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "InvalidRequestException",
            "case {token:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// Send one `UpdateDataIntegration` (`PATCH /dataIntegrations/{Identifier}`).
async fn update(emulator: &Emulator, identifier: &str, body: Value) -> Reply {
    emulator
        .send(
            "PATCH",
            &format!("/dataIntegrations/{}", encode_label(identifier)),
            REGION,
            Some(body),
        )
        .await
}

#[tokio::test]
async fn update_applies_the_members_it_carries_and_leaves_the_others_alone() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in WHICH members the PATCH carries and in their form: one member, the
    // other, both, a description at its published upper bound, a description cleared to
    // the empty string (`Description` is `len 0..1000`, so "" is a value), and a PATCH
    // that carries neither member.
    let rows: Vec<(&str, Value, Option<&str>, Option<String>)> = vec![
        (
            "only a description",
            json!({ "Description": "pulls the nightly feed" }),
            None,
            Some("pulls the nightly feed".to_string()),
        ),
        (
            "only a name",
            json!({ "Name": "renamed.integration" }),
            Some("renamed.integration"),
            Some("the original description".to_string()),
        ),
        (
            "both members",
            json!({ "Name": "renamed_both", "Description": "both changed" }),
            Some("renamed_both"),
            Some("both changed".to_string()),
        ),
        (
            "a description at its published upper bound",
            json!({ "Description": "d".repeat(DESCRIPTION_MAX) }),
            None,
            Some("d".repeat(DESCRIPTION_MAX)),
        ),
        (
            "an empty description",
            json!({ "Description": "" }),
            None,
            Some(String::new()),
        ),
        (
            "neither member",
            json!({}),
            None,
            Some("the original description".to_string()),
        ),
    ];

    for (index, (label, patch, expected_name, expected_description)) in rows.iter().enumerate() {
        // Both identifier forms the service hands out address the resource.
        for form in ["by id", "by arn"] {
            let original_name = format!("patched-{index}-{}", form.replace(' ', "-"));
            let created = create(
                &emulator,
                json!({
                    "Name": original_name,
                    "KmsKey": kms_key,
                    "Description": "the original description",
                    "SourceURI": "s3://feed-bucket/accounts",
                }),
            )
            .await;
            let id = created.string("Id").expect("the create reported an Id");
            let arn = created.string("Arn").expect("the create reported an ARN");
            let identifier = if form == "by id" { id.clone() } else { arn.clone() };

            let patched = update(&emulator, &identifier, patch.clone()).await;
            assert_eq!(
                patched.status, 200,
                "case {label:?} {form}: expected HTTP 200 from UpdateDataIntegration, got {} / {}",
                patched.status, patched.body
            );

            let fetched = get(&emulator, &identifier).await;
            assert_eq!(
                fetched.string("Name").as_deref(),
                Some(expected_name.unwrap_or(original_name.as_str())),
                "case {label:?} {form}: expected the Name the update left stored, got {:?}",
                fetched.field("Name")
            );
            assert_eq!(
                fetched.string("Description"),
                *expected_description,
                "case {label:?} {form}: expected the Description the update left stored, got {:?}",
                fetched.field("Description")
            );
            // The members the PATCH does not model are untouched, and so is identity.
            assert_eq!(
                fetched.string("SourceURI").as_deref(),
                Some("s3://feed-bucket/accounts"),
                "case {label:?} {form}: an update must not change SourceURI, got {:?}",
                fetched.field("SourceURI")
            );
            assert_eq!(
                fetched.string("KmsKey").as_deref(),
                Some(kms_key.as_str()),
                "case {label:?} {form}: an update must not change KmsKey, got {:?}",
                fetched.field("KmsKey")
            );
            assert_eq!(
                (fetched.string("Id"), fetched.string("Arn")),
                (Some(id.clone()), Some(arn.clone())),
                "case {label:?} {form}: an update must not change Id or Arn, got {}",
                fetched.body
            );

            delete(&emulator, &id).await;
        }
    }
}

#[tokio::test]
async fn update_refuses_an_identifier_this_service_did_not_mint() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;
    let created = create(
        &emulator,
        json!({ "Name": "update-target", "KmsKey": kms_key }),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");

    // Rows differ in why the identifier names nothing: a UUID never minted, a foreign
    // account, region, vendor and resource type on an otherwise real id, and a value
    // that is no identifier form at all.
    let unknown: Vec<(&str, String)> = vec![
        (
            "a UUID this service never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "another account",
            format!("arn:aws:app-integrations:{REGION}:111122223333:data-integration/{id}"),
        ),
        (
            "another region",
            format!("arn:aws:app-integrations:eu-west-1:{ACCOUNT}:data-integration/{id}"),
        ),
        (
            "another vendor",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:data-integration/{id}"),
        ),
        (
            "another resource type",
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:event-integration/{id}"),
        ),
        ("a name rather than an identifier", "update-target".to_string()),
    ];

    for (label, identifier) in &unknown {
        let reply = update(&emulator, identifier, json!({ "Description": "no such thing" })).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404 from UpdateDataIntegration, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
    }

    // And none of those calls changed the resource that does exist.
    let fetched = get(&emulator, &id).await;
    assert_eq!(
        fetched.field("Description"),
        None,
        "a refused update must persist nothing, got {}",
        fetched.body
    );
}

#[tokio::test]
async fn update_refuses_a_rename_onto_a_name_another_integration_holds() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the name's form and length, including the published bounds of
    // `Name` (len 1..255).
    for taken in ["held.one", "h", &"h".repeat(255)] {
        let holder = create(&emulator, json!({ "Name": taken, "KmsKey": kms_key })).await;
        let mover = create(
            &emulator,
            json!({ "Name": format!("mover-{}", taken.len()), "KmsKey": kms_key }),
        )
        .await;
        let holder_id = holder.string("Id").expect("the create reported an Id");
        let mover_id = mover.string("Id").expect("the create reported an Id");

        let refused = update(&emulator, &mover_id, json!({ "Name": taken })).await;
        assert_eq!(
            refused.status, 400,
            "case {taken:?}: expected HTTP 400 for a rename onto a held name, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "InvalidRequestException",
            "case {taken:?}: expected InvalidRequestException, got {:?}",
            refused.error_type
        );
        // Neither resource moved.
        assert_eq!(
            get(&emulator, &mover_id).await.string("Name"),
            Some(format!("mover-{}", taken.len())),
            "case {taken:?}: the refused rename must leave the name it had"
        );

        // Renaming a resource onto the name it already holds is not a duplicate.
        let kept = update(&emulator, &holder_id, json!({ "Name": taken })).await;
        assert_eq!(
            kept.status, 200,
            "case {taken:?}: a resource may keep its own name, got {} / {}",
            kept.status, kept.body
        );

        delete(&emulator, &holder_id).await;
        delete(&emulator, &mover_id).await;
    }
}
