// SPDX-License-Identifier: CC0-1.0

//! Amazon EVS environment connectors, end to end over awsJson1_0.
//!
//! The rules under test, and where each comes from:
//!
//! * `CreateEnvironmentConnector` PERSISTS one connector in the named environment and
//!   `ListEnvironmentConnectors` reports exactly that record, whatever the connector
//!   type, the form or the length of the appliance FQDN, and whether the secret is
//!   given as an ARN or as a name ("The ARN or name of the Amazon Web Services Secrets
//!   Manager secret", `secretIdentifier` doc) — a name being reported as the ARN it
//!   resolves to, which `tests/all/connector_secret_arn.rs` owns.
//! * A new connector is `CREATING` and the clock owes it `ACTIVE`: "A connector allows
//!   the Amazon EVS control plane to interface with VCF appliances using a fully
//!   qualified domain name" (`CreateEnvironmentConnector` doc) — a connection to
//!   establish, not just a record to write.
//! * "You can create only one connector of each type per environment"
//!   (`CreateEnvironmentConnector` doc), and the operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="CreateEnvironmentConnector")`): the
//!   same create again reports the stored record, a create of that type with different
//!   metadata is refused.
//! * `UpdateEnvironmentConnector` reconfigures EXACTLY ONE piece of connector
//!   metadata: "You can update the Amazon Web Services Secrets Manager secret ARN or
//!   the appliance FQDN to reconfigure the connector metadata. You cannot update both
//!   the secret and the FQDN in the same request." The member it names takes the new
//!   value, the other one holds, the connector goes `UPDATING` and the clock owes it
//!   `ACTIVE` again.
//! * The operation is `@idempotent`: an update asking for the value the connector
//!   already carries changes nothing — not its state and not its `modifiedAt`.
//! * An `environmentId` that names no live environment at this endpoint's Region, or a
//!   `connectorId` that environment holds no connector for, is a
//!   `ResourceNotFoundException` naming the resource the request asked for, and
//!   nothing is written for it.
//! * A read never changes what a later caller sees: two lists report the same records.
//!
//! The harness (`emulator`, `call`, `call_in_region`, `create`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, call_in_region, create_settled, emulator, error_type, poll_connector_active,
    register_credential_secret, secret_arn_of, Emulator,
};

/// Every `ConnectorType` the model declares, from the catalog this crate generated
/// from the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::CONNECTOR_TYPE_VALUES;

/// Long enough for the `CREATING`/`UPDATING` -> `ACTIVE` change the clock owes a
/// connector to come due (its span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// A secret given as a bare name, the other form it accepts. The ARN reported for it is
/// the ARN of that name (`environment_lifecycle::secret_arn_of`), because `secretArn` is
/// "The Amazon Resource Name (ARN) of" the secret — the rule
/// `tests/all/connector_secret_arn.rs` owns.
const SECRET_NAME: &str = "evs/vcf-appliance-credentials";

/// An FQDN at the modelled `ApplianceFqdn` maximum of 253 characters: three 63-byte
/// labels (the longest a DNS label can be under that member's own pattern) and a
/// 61-byte one, dot-separated.
fn longest_fqdn() -> String {
    let label = "a".repeat(63);
    format!("{label}.{label}.{label}.{}", "a".repeat(61))
}

/// A `CreateEnvironmentConnector` request.
fn create_request(
    environment_id: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
) -> Value {
    json!({
        "environmentId": environment_id,
        "type": connector_type,
        "applianceFqdn": appliance_fqdn,
        "secretIdentifier": secret_identifier,
    })
}

/// A `ListEnvironmentConnectors` request for one environment.
fn list_request(environment_id: &str) -> Value {
    json!({ "environmentId": environment_id })
}

/// Whether `id` satisfies the modelled `^(cnctr-[a-zA-Z0-9]{10})$`.
fn is_connector_id(id: &str) -> bool {
    match id.strip_prefix("cnctr-") {
        Some(rest) => rest.len() == 10 && rest.bytes().all(|b| b.is_ascii_alphanumeric()),
        None => false,
    }
}

/// The `connector` of a response body.
fn connector(case: &str, body: &Value) -> Value {
    let reported = &body["connector"];
    assert!(
        reported.is_object(),
        "case {case:?}: no connector in {body}"
    );
    reported.clone()
}

/// Every connector of one environment, following `nextToken` to the end. `page` is the
/// `maxResults` each call asks for, when the caller names one.
async fn list_connectors(
    router: &MultiServiceRouter,
    case: &str,
    environment_id: &str,
    page: Option<i32>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // One round per connector type plus one, so a token that never clears fails here
    // instead of looping forever.
    for _ in 0..=CONNECTOR_TYPES.len() {
        let mut request = list_request(environment_id);
        if let Some(page) = page {
            request["maxResults"] = json!(page);
        }
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListEnvironmentConnectors", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list expected 200, got {status} {body}"
        );
        let page_items = body["connectors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
            .clone();
        collected.extend(page_items);
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return collected,
        }
    }
    panic!(
        "case {case:?}: nextToken never cleared after {} pages",
        CONNECTOR_TYPES.len() + 1
    );
}

/// The one connector of an environment, as `ListEnvironmentConnectors` reports it.
async fn only_connector(router: &MultiServiceRouter, case: &str, environment_id: &str) -> Value {
    let listed = list_connectors(router, case, environment_id, None).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly one stored connector, got {listed:?}"
    );
    listed[0].clone()
}

/// Create an environment with one connector and return the environment id and the
/// reported connector.
async fn environment_with_connector(
    router: &Emulator,
    case: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
) -> (String, Value) {
    // Settled: a list addressed to an environment that is still initializing is
    // refused (`tests/all/list_while_initializing.rs`), and every test here reads its
    // connector back through `ListEnvironmentConnectors`.
    let environment = create_settled(router, None).await;
    // A connector is created against a credential secret the account really holds, which
    // the row's identifier names ([`register_credential_secret`]);
    // `tests/all/connector_credential_secret.rs` owns that rule.
    register_credential_secret(&router.engine, secret_identifier);
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &create_request(&environment, connector_type, appliance_fqdn, secret_identifier),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create expected 200, got {status} {body}"
    );
    let created = connector(case, &body);
    (environment, created)
}

/// Assert a member of a reported connector.
fn assert_member(case: &str, reported: &Value, member: &str, expected: &Value) {
    assert_eq!(
        &reported[member], expected,
        "case {case:?}: {member}: expected {expected}, got {}",
        reported[member]
    );
}

#[tokio::test]
async fn a_connector_is_created_and_read_back_by_list() {
    let router = emulator();

    // Each row: the case, the connector type, the appliance FQDN and the secret
    // identifier. The rows differ along what the rule generalises over — the type (the
    // whole modelled catalog, including the OPERATIONS_MANAGER value the issue did not
    // cite), the form and the length of the FQDN (both modelled bounds of
    // `ApplianceFqdn`, len 1..253), and the form and length of the secret (an ARN, a
    // bare name, and the modelled 2048-character maximum of `SecretIdentifier`).
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "a vCenter connector with a three-label FQDN and an ARN secret",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN.to_string(),
        ),
        (
            "an SDDC Manager connector with a named secret",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            SECRET_NAME.to_string(),
        ),
        (
            "an Operations Manager connector with a single-label FQDN",
            "OPERATIONS_MANAGER",
            "operations-manager".to_string(),
            SECRET_NAME.to_string(),
        ),
        (
            "an FQDN at the modelled 1-character minimum",
            "VCENTER",
            "a".to_string(),
            SECRET_ARN.to_string(),
        ),
        (
            "an FQDN at the modelled 253-character maximum",
            "VCENTER",
            longest_fqdn(),
            SECRET_ARN.to_string(),
        ),
        (
            // The ARN reported for a name this long is longer than the 2048 characters
            // `SecretIdentifier` bounds, which no real secret reaches: Secrets Manager
            // bounds a secret NAME at 512 characters, so no AWS rule covers this input
            // and the ARN of the name is reported uniformly.
            "a secret name at the modelled 2048-character maximum",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            "s".repeat(2048),
        ),
    ];
    // Every modelled type is covered by the rows above, so the catalog and the table
    // cannot drift apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter().any(|(_, row_type, _, _)| row_type == connector_type),
            "the table must cover the modelled type {connector_type:?}"
        );
    }

    for (case, connector_type, appliance_fqdn, secret_identifier) in rows {
        // A fresh environment per row, so each row reads back only its own record.
        let (environment, created) = environment_with_connector(
            &router,
            case,
            connector_type,
            &appliance_fqdn,
            &secret_identifier,
        )
        .await;

        let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_connector_id(&connector_id),
            "case {case:?}: connectorId {connector_id:?} must match ^(cnctr-[a-zA-Z0-9]{{10}})$"
        );
        for (member, expected) in [
            ("environmentId", json!(environment)),
            ("type", json!(connector_type)),
            ("applianceFqdn", json!(appliance_fqdn)),
            // `secretArn` reports the ARN of the secret the request named, whichever
            // form it used: "The Amazon Resource Name (ARN) of the Amazon Web Services
            // Secrets Manager secret".
            ("secretArn", json!(secret_arn_of(&secret_identifier))),
            // The connection is still being established.
            ("state", json!("CREATING")),
            // The connection is still being established, so no check of it has
            // concluded: the connectivity-health roll-up is UNKNOWN and there are no
            // checks (`tests/all/connector_health_checks.rs` owns that rule).
            ("status", json!("UNKNOWN")),
            ("checks", Value::Null),
            ("stateDetails", Value::Null),
        ] {
            assert_member(case, &created, member, &expected);
        }
        assert_eq!(
            created["createdAt"], created["modifiedAt"],
            "case {case:?}: an untouched connector was modified when it was created, got {created}"
        );

        // The read-back: the list must report the record the create stored.
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, created,
            "case {case:?}: the list must report the created connector, got {listed}"
        );
        // And a read changes nothing a later caller sees.
        let again = only_connector(&router, case, &environment).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second list must report the same record, got {again}"
        );

        // The clock owes the connector ACTIVE, and the stamps hold across the change.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = only_connector(&router, case, &environment).await;
        assert_member(case, &settled, "state", &json!("ACTIVE"));
        assert_member(case, &settled, "createdAt", &created["createdAt"]);
        assert_member(case, &settled, "applianceFqdn", &json!(appliance_fqdn));
        assert_member(
            case,
            &settled,
            "secretArn",
            &json!(secret_arn_of(&secret_identifier)),
        );
    }
}

#[tokio::test]
async fn one_connector_of_each_type_lives_in_an_environment_and_the_list_pages_over_them() {
    let router = emulator();
    let environment = create_settled(&router, None).await;

    let mut created_ids = Vec::new();
    for connector_type in CONNECTOR_TYPES {
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(
                &environment,
                connector_type,
                &format!(
                    "{}.evs.example.com",
                    // The FQDN pattern allows no underscore, which the type names carry.
                    connector_type.to_lowercase().replace('_', "-")
                ),
                SECRET_ARN,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {connector_type:?}: create expected 200, got {status} {body}"
        );
        created_ids.push(
            connector(connector_type, &body)["connectorId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    created_ids.sort();

    // Each row: the page size asked for, and the number of connectors it must report
    // per page over the stored records.
    let rows: [(Option<i32>, usize); 4] = [
        (None, CONNECTOR_TYPES.len()),
        (Some(1), 1),
        (Some(2), 2),
        (Some(100), CONNECTOR_TYPES.len()),
    ];
    for (page, expected_first_page) in rows {
        let case = format!("maxResults {page:?}");
        let mut request = list_request(&environment);
        if let Some(page) = page {
            request["maxResults"] = json!(page);
        }
        let (status, body) = call(&router, "ListEnvironmentConnectors", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let first_page = body["connectors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            first_page.len(),
            expected_first_page,
            "case {case:?}: expected {expected_first_page} connector(s) on the first page, got {}",
            first_page.len()
        );

        // Every page together must report each stored connector exactly once.
        let mut seen: Vec<String> = list_connectors(&router, &case, &environment, page)
            .await
            .iter()
            .map(|entry| entry["connectorId"].as_str().unwrap_or_default().to_string())
            .collect();
        seen.sort();
        assert_eq!(
            seen, created_ids,
            "case {case:?}: the pages must report exactly the stored connectors, got {seen:?}"
        );
    }

    // An environment with no connector at all is an empty page, not a refusal.
    let bare = create_settled(&router, None).await;
    let (status, body) = call(&router, "ListEnvironmentConnectors", &list_request(&bare)).await;
    assert_eq!(
        status, 200,
        "case an environment with no connector: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["connectors"],
        json!([]),
        "case an environment with no connector: expected an empty list, got {body}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "case an environment with no connector: an exhausted list carries no nextToken, got {body}"
    );
}

#[tokio::test]
async fn an_update_reconfigures_exactly_the_member_it_names() {
    let router = emulator();
    const FQDN: &str = "vcenter.evs.example.com";

    // Each row: the case, the member the update names, and the new value it asks for.
    // The rows differ along what the rule generalises over — which of the two members
    // is reconfigured, and the form and length of the new value.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a new multi-label FQDN",
            "applianceFqdn",
            "vcenter-2.evs.example.com".to_string(),
        ),
        (
            "a new single-label FQDN",
            "applianceFqdn",
            "vcenter2".to_string(),
        ),
        (
            "a new FQDN at the modelled 253-character maximum",
            "applianceFqdn",
            longest_fqdn(),
        ),
        (
            "a new secret given as an ARN",
            "secretIdentifier",
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/rotated-ZyXw98".to_string(),
        ),
        (
            "a new secret given as a bare name",
            "secretIdentifier",
            "evs/rotated-credentials".to_string(),
        ),
    ];

    for (case, member, new_value) in rows {
        let (environment, created) =
            environment_with_connector(&router, case, "VCENTER", FQDN, SECRET_ARN).await;
        let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();
        // Wait for the create to have settled, so the UPDATING below is the update's own
        // doing: the test waits for the connector to BE ACTIVE, not for a span of wall
        // clock.
        let before = poll_connector_active(&router, case, &environment, &connector_id).await;

        // A connector is pointed only at a credential secret the account holds, so a row
        // that names a new secret registers it
        // (`tests/all/connector_credential_secret.rs` owns that rule); a row naming a new
        // FQDN registers nothing, since that identifier is no secret.
        if member == "secretIdentifier" {
            register_credential_secret(&router.engine, &new_value);
        }
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                member: new_value,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} {body}"
        );
        let updated = connector(case, &body);

        // The member named carries the new value; the other one holds; the identity of
        // the connector is untouched.
        let (changed, held, expected_new) = if member == "applianceFqdn" {
            ("applianceFqdn", "secretArn", new_value.clone())
        } else {
            ("secretArn", "applianceFqdn", secret_arn_of(&new_value))
        };
        assert_member(case, &updated, changed, &json!(expected_new));
        assert_member(case, &updated, held, &before[held]);
        assert_member(case, &updated, "connectorId", &json!(connector_id));
        assert_member(case, &updated, "environmentId", &json!(environment));
        assert_member(case, &updated, "type", &json!("VCENTER"));
        assert_member(case, &updated, "createdAt", &before["createdAt"]);
        // The new appliance or secret has to be connected with before the connector is
        // active again, so the caller sees the transient state.
        assert_member(case, &updated, "state", &json!("UPDATING"));
        let modified_at = updated["modifiedAt"].as_f64().unwrap_or_default();
        assert!(
            modified_at >= before["modifiedAt"].as_f64().unwrap_or_default(),
            "case {case:?}: modifiedAt must move forward, got {modified_at} after {}",
            before["modifiedAt"]
        );

        // The read-back: the list reports the record the update wrote.
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, updated,
            "case {case:?}: the list must report the updated connector, got {listed}"
        );

        // And the clock owes it ACTIVE again, with the new value kept.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = only_connector(&router, case, &environment).await;
        assert_member(case, &settled, "state", &json!("ACTIVE"));
        assert_member(case, &settled, changed, &json!(expected_new));
        assert_member(case, &settled, held, &before[held]);
    }
}

#[tokio::test]
async fn an_update_that_asks_for_the_stored_value_changes_nothing() {
    let router = emulator();
    const FQDN: &str = "vcenter.evs.example.com";
    let (environment, created) = environment_with_connector(
        &router,
        "the repeat update",
        "VCENTER",
        FQDN,
        SECRET_ARN,
    )
    .await;
    let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();
    std::thread::sleep(PAST_THE_TRANSITION);
    let settled = only_connector(&router, "the repeat update", &environment).await;

    // Each row: the case, and an update asking for exactly what the connector carries.
    let rows: [(&str, &str, &str); 2] = [
        ("the stored FQDN again", "applianceFqdn", FQDN),
        ("the stored secret again", "secretIdentifier", SECRET_ARN),
    ];
    for (case, member, value) in rows {
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                member: value,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            connector(case, &body),
            settled,
            "case {case:?}: an update to the stored value must change nothing, got {body}"
        );
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, settled,
            "case {case:?}: the stored record must be untouched, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_both_members_or_neither_is_refused_and_writes_nothing() {
    let router = emulator();
    const FQDN: &str = "vcenter.evs.example.com";
    let (environment, created) =
        environment_with_connector(&router, "the refusals", "VCENTER", FQDN, SECRET_ARN).await;
    let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();
    std::thread::sleep(PAST_THE_TRANSITION);
    let settled = only_connector(&router, "the refusals", &environment).await;

    // Each row: the case, and the members the request supplies beyond the ids. "You
    // cannot update both the secret and the FQDN in the same request", and a request
    // naming neither asks for no new metadata.
    let rows: Vec<(&str, Value)> = vec![
        (
            "both the FQDN and the secret",
            json!({
                "applianceFqdn": "vcenter-2.evs.example.com",
                "secretIdentifier": SECRET_NAME,
            }),
        ),
        ("neither member", json!({})),
    ];
    for (case, extra) in rows {
        let mut request = json!({
            "environmentId": environment,
            "connectorId": connector_id,
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, "UpdateEnvironmentConnector", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {}",
            error_type(&body)
        );
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, settled,
            "case {case:?}: a refused update must write nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_update_of_an_absent_environment_or_connector_is_a_not_found() {
    let router = emulator();
    const FQDN: &str = "vcenter.evs.example.com";
    let (environment, created) =
        environment_with_connector(&router, "the not-found table", "VCENTER", FQDN, SECRET_ARN)
            .await;
    let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();
    std::thread::sleep(PAST_THE_TRANSITION);
    let settled = only_connector(&router, "the not-found table", &environment).await;

    // Each row: the case, the environment id, the connector id, and the resource the
    // refusal must name. The ids span the modelled patterns
    // `^(env-[a-zA-Z0-9]{10})$` and `^(cnctr-[a-zA-Z0-9]{10})$` — digits, lower case,
    // upper case and mixed — and the connector rows address a REAL environment, so the
    // miss is the connector lookup's and not the environment's.
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an all-digit environment id",
            "env-0000000000".to_string(),
            connector_id.clone(),
            "environment",
            "env-0000000000".to_string(),
        ),
        (
            "a mixed-case environment id",
            "env-0A1b2C3d4E".to_string(),
            connector_id.clone(),
            "environment",
            "env-0A1b2C3d4E".to_string(),
        ),
        (
            "an upper-case environment id",
            "env-ZZZZZZZZZZ".to_string(),
            "cnctr-ZZZZZZZZZZ".to_string(),
            "environment",
            "env-ZZZZZZZZZZ".to_string(),
        ),
        (
            "an all-digit connector id in a real environment",
            environment.clone(),
            "cnctr-0000000000".to_string(),
            "connector",
            "cnctr-0000000000".to_string(),
        ),
        (
            "a mixed-case connector id in a real environment",
            environment.clone(),
            "cnctr-0A1b2C3d4E".to_string(),
            "connector",
            "cnctr-0A1b2C3d4E".to_string(),
        ),
    ];

    for (case, environment_id, connector, resource_type, resource_id) in rows {
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment_id,
                "connectorId": connector,
                "applianceFqdn": "vcenter-2.evs.example.com",
            }),
        )
        .await;
        // An awsJson front end reports every modeled client error with HTTP 400; the
        // body's `__type` is what tells one apart (see tests/all/error_wire_status.rs).
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["resourceId"],
            json!(resource_id),
            "case {case:?}: resourceId must name the requested resource, got {body}"
        );
        assert_eq!(
            body["resourceType"],
            json!(resource_type),
            "case {case:?}: unexpected resourceType, got {body}"
        );
    }

    // A connector of one Region's environment is not addressable from another Region.
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "applianceFqdn": "vcenter-2.evs.example.com",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Environment {environment} was not found")),
            "case {region:?}: the message must name the requested environment, got {body}"
        );
    }

    // None of the refusals wrote anything.
    let listed = only_connector(&router, "after the refusals", &environment).await;
    assert_eq!(
        listed, settled,
        "case after the refusals: a refused update must write nothing, got {listed}"
    );
}

#[tokio::test]
async fn only_one_live_connector_of_each_type_can_be_created() {
    let router = emulator();
    const FQDN: &str = "vcenter.evs.example.com";

    for connector_type in CONNECTOR_TYPES {
        let (environment, created) = environment_with_connector(
            &router,
            connector_type,
            connector_type,
            FQDN,
            SECRET_ARN,
        )
        .await;

        // The same create again reports the stored record and mints nothing: the
        // operation is `@idempotent`.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(&environment, connector_type, FQDN, SECRET_ARN),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {connector_type:?}: the repeat create expected 200, got {status} {body}"
        );
        assert_eq!(
            connector(connector_type, &body)["connectorId"],
            created["connectorId"],
            "case {connector_type:?}: the repeat create must report the stored connector, got {body}"
        );

        // A create of that type with different metadata cannot be a second connector.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(
                &environment,
                connector_type,
                "another-appliance.evs.example.com",
                SECRET_NAME,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {connector_type:?}: a conflicting create expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {connector_type:?}: expected a ValidationException, got {}",
            error_type(&body)
        );

        // Either way the environment still holds exactly the one connector, unchanged.
        let listed = only_connector(&router, connector_type, &environment).await;
        assert_eq!(
            listed["applianceFqdn"],
            json!(FQDN),
            "case {connector_type:?}: the stored connector must be untouched, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_environment_that_has_reached_deleted_holds_no_new_or_updated_connector() {
    let router = emulator();
    let (environment, created) = environment_with_connector(
        &router,
        "a DELETED environment",
        "VCENTER",
        "vcenter.evs.example.com",
        SECRET_ARN,
    )
    .await;
    let connector_id = created["connectorId"].as_str().unwrap_or_default().to_string();

    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the delete: expected 200, got {status} {body}"
    );
    std::thread::sleep(PAST_THE_TRANSITION);

    // Each row: the case, the operation and its request. An environment that has
    // reached DELETED holds no connector a create or an update can reach ("The
    // resource might not be specified correctly, or it may have a `state` of
    // `DELETED`", `ResourceNotFoundException` doc).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a create in a DELETED environment",
            "CreateEnvironmentConnector",
            create_request(&environment, "SDDC_MANAGER", "sddc.evs.example.com", SECRET_ARN),
        ),
        (
            "an update in a DELETED environment",
            "UpdateEnvironmentConnector",
            json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "applianceFqdn": "vcenter-2.evs.example.com",
            }),
        ),
    ];
    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["resourceId"],
            json!(environment),
            "case {case:?}: resourceId must echo the requested environment, got {body}"
        );
    }
}
