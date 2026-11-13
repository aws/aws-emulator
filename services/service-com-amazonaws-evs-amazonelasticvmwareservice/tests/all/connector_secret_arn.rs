// SPDX-License-Identifier: CC0-1.0

//! What `Connector$secretArn` reports for the credential secret an Amazon EVS request
//! named, end to end over the wire.
//!
//! The rule under test: `secretIdentifier` is "The ARN or name of the Amazon Web
//! Services Secrets Manager secret" — the modelled `SecretIdentifier` pattern spells
//! both alternatives — while the member it feeds, `Connector$secretArn`, is "The Amazon
//! Resource Name (ARN) of" that secret. So a connector reports an ARN whichever form the
//! request used: an identifier that already is an ARN names the secret the caller meant
//! and comes back unchanged, and a NAME comes back as the ARN of that name in the Region
//! the request was addressed to and this account — never as the name.
//!
//! The rows differ along what the rule generalises over: the FORM the secret is named in
//! (a bare name, a full ARN, a full ARN of another Region, the partial ARN form Secrets
//! Manager appends its six random characters to), the characters and the length of the
//! name, the connector type, and the operation that writes the value
//! (`CreateEnvironmentConnector` and `UpdateEnvironmentConnector`). Every value is read
//! back through `ListEnvironmentConnectors`, because the ARN has to be what was STORED.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, create_settled, emulator, poll_connector_active, register_credential_secret,
    secret_arn_of, Emulator, SECRET_ARN_PREFIX,
};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`), so the rows span
/// the whole catalog rather than one type.
const CONNECTOR_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::CONNECTOR_TYPE_VALUES;

/// Create a connector in a fresh settled environment and return the environment id and
/// the reported connector.
async fn connector_with_secret(
    router: &Emulator,
    case: &str,
    connector_type: &str,
    secret_identifier: &str,
) -> (String, Value) {
    let environment = create_settled(router, None).await;
    // The account really holds the secret the row names: a connector is created only
    // against credentials that are there (`tests/all/connector_credential_secret.rs` owns
    // that rule), and this suite is about the ARN such a secret is REPORTED by.
    register_credential_secret(&router.engine, secret_identifier);
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment,
            "type": connector_type,
            "applianceFqdn": APPLIANCE_FQDN,
            "secretIdentifier": secret_identifier,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create expected 200, got {status} {body}"
    );
    (environment, body["connector"].clone())
}

/// The appliance every connector here connects to: this suite varies the SECRET, so the
/// FQDN is held fixed (`tests/all/connectors.rs` owns the FQDN class).
const APPLIANCE_FQDN: &str = "vcenter.evs.example.com";

/// The one connector of an environment, as `ListEnvironmentConnectors` reports it.
async fn only_connector(router: &MultiServiceRouter, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list expected 200, got {status} {body}"
    );
    let listed = body["connectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly one stored connector, got {listed:?}"
    );
    listed[0].clone()
}

/// Assert `reported` carries the ARN the rule requires — and that it IS an ARN, not the
/// name the request may have used.
fn assert_secret_arn(case: &str, reported: &Value, expected: &str) {
    let actual = reported["secretArn"].as_str().unwrap_or_default();
    assert!(
        actual.starts_with(SECRET_ARN_PREFIX),
        "case {case:?}: secretArn must be an ARN of a Secrets Manager secret, got {actual:?}"
    );
    assert_eq!(
        actual, expected,
        "case {case:?}: secretArn: expected {expected:?}, got {actual:?}"
    );
}

/// A bare secret name, one form `secretIdentifier` accepts.
const NAME: &str = "evs/vcf-vcenter-creds";

/// A bare name carrying every character the modelled `SecretIdentifier` pattern admits
/// besides letters and digits (`[a-zA-Z0-9/_+=.@!-]`).
const EVERY_CHARACTER_NAME: &str = "Evs/vcf_creds+1=2.a@b!-X";

/// A bare name at the shortest the modelled `SecretIdentifier` allows (len 1..2048).
const SHORTEST_NAME: &str = "s";

/// A full Secrets Manager ARN, the other form it accepts: a secret's name followed by
/// the hyphen and six random characters that service appends when it creates one.
const FULL_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// A full ARN of a secret in ANOTHER Region than the one the request is addressed to: it
/// names the secret the caller meant, so it is reported as written rather than relabelled.
const OTHER_REGION_ARN: &str =
    "arn:aws:secretsmanager:eu-central-1:000000000000:secret:evs/vcf-ZyXw98";

/// The PARTIAL ARN form — a secret ARN without the trailing hyphen and six random
/// characters, which Secrets Manager also accepts as an identifier.
const PARTIAL_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf";

#[tokio::test]
async fn a_created_connector_reports_an_arn_for_every_identifier_form() {
    let router = emulator();

    // Each row: the case, and the `secretIdentifier` the request carries. The ARN each
    // must report is the rule applied to that identifier, not a value written out here.
    let rows: Vec<(&str, String)> = vec![
        ("a bare secret name", NAME.to_string()),
        (
            "a bare name carrying every admitted character",
            EVERY_CHARACTER_NAME.to_string(),
        ),
        (
            "a bare name at the modelled 1-character minimum",
            SHORTEST_NAME.to_string(),
        ),
        (
            "a bare name at the modelled 2048-character maximum",
            // The ARN of a name this long is longer than the 2048 characters
            // `SecretIdentifier` bounds, which no real secret reaches (Secrets Manager
            // bounds a secret NAME at 512 characters), so no AWS rule covers this input
            // and the ARN of the name is reported uniformly.
            "s".repeat(2048),
        ),
        ("a full secret ARN", FULL_ARN.to_string()),
        (
            "a full secret ARN of another Region",
            OTHER_REGION_ARN.to_string(),
        ),
        ("a partial secret ARN", PARTIAL_ARN.to_string()),
    ];

    for (index, (case, secret_identifier)) in rows.iter().enumerate() {
        let case = *case;
        let expected = secret_arn_of(secret_identifier);
        // The type is cycled over the whole modelled catalog, so the rule is not tested
        // on one connector type only.
        let connector_type = CONNECTOR_TYPES[index % CONNECTOR_TYPES.len()];
        let (environment, created) =
            connector_with_secret(&router, case, connector_type, secret_identifier).await;
        assert_secret_arn(case, &created, &expected);

        // The round trip: the ARN has to be the value that was STORED, not one the
        // create answered and forgot.
        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, created,
            "case {case:?}: the list must report the created connector, got {listed}"
        );
        assert_secret_arn(case, &listed, &expected);

        // And the value survives the state change the clock owes the connector: the test
        // waits for the connector to BE ACTIVE, not for a span of wall clock.
        let settled = poll_connector_active(
            &router,
            case,
            &environment,
            created["connectorId"].as_str().unwrap_or_default(),
        )
        .await;
        assert_secret_arn(case, &settled, &expected);
    }
}

#[tokio::test]
async fn naming_one_secret_in_either_form_is_the_same_secret_to_a_create_and_an_update() {
    let router = emulator();

    // The connector is created naming the secret by NAME, and reports that name's ARN.
    let case = "a create naming the secret by name";
    let (environment, created) = connector_with_secret(&router, case, "VCENTER", NAME).await;
    let connector_id = created["connectorId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let name_arn = secret_arn_of(NAME);
    assert_secret_arn(case, &created, &name_arn);
    // Wait for the create to have settled, so an UPDATING below is the update's own doing.
    let settled = poll_connector_active(&router, case, &environment, &connector_id).await;
    assert_secret_arn(case, &settled, &name_arn);

    // The same create again, naming that ONE secret by the ARN the name resolves to: the
    // same secret and the same appliance, so the `@idempotent` create reports the stored
    // record rather than refusing a second connector of the type.
    let case = "the repeat create naming the same secret by its ARN";
    let (status, body) = call(
        &router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment,
            "type": "VCENTER",
            "applianceFqdn": APPLIANCE_FQDN,
            "secretIdentifier": name_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["connector"], settled,
        "case {case:?}: the repeat create must report the stored connector, got {body}"
    );

    // Each row: the case, the `secretIdentifier` the update names, and whether that is a
    // change to the stored record. The ARN each must report is the rule applied to the
    // identifier.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "an update naming the stored secret by the ARN of its name",
            name_arn.clone(),
            false,
        ),
        (
            "an update naming the stored secret by its bare name again",
            NAME.to_string(),
            false,
        ),
        (
            "an update naming another secret by name",
            EVERY_CHARACTER_NAME.to_string(),
            true,
        ),
        (
            "an update naming another secret by a full ARN",
            FULL_ARN.to_string(),
            true,
        ),
        (
            "an update naming the shortest admitted name",
            SHORTEST_NAME.to_string(),
            true,
        ),
    ];

    let mut before = settled;
    for (case, secret_identifier, changes) in rows {
        let expected = secret_arn_of(&secret_identifier);
        // The account holds the secret each row names: a connector is pointed only at
        // credentials that are there (`tests/all/connector_credential_secret.rs`).
        register_credential_secret(&router.engine, &secret_identifier);
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "secretIdentifier": secret_identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} {body}"
        );
        let updated = body["connector"].clone();
        assert_secret_arn(case, &updated, &expected);
        assert_eq!(
            updated["applianceFqdn"], before["applianceFqdn"],
            "case {case:?}: the appliance FQDN must hold, got {updated}"
        );

        if changes {
            // The new secret has to be connected with before the connector is active
            // again, so the caller sees the transient state.
            assert_eq!(
                updated["state"],
                json!("UPDATING"),
                "case {case:?}: a reconfigured connector is UPDATING, got {updated}"
            );
        } else {
            // Naming the secret the connector already carries, by name or by the ARN
            // that name resolves to, is not a change: the record keeps its state and
            // its modifiedAt.
            assert_eq!(
                updated, before,
                "case {case:?}: an update to the stored secret must change nothing, got {updated}"
            );
        }

        let listed = only_connector(&router, case, &environment).await;
        assert_eq!(
            listed, updated,
            "case {case:?}: the list must report the updated connector, got {listed}"
        );
        // The next row compares against this record, once the connector has settled.
        before = poll_connector_active(&router, case, &environment, &connector_id).await;
        assert_secret_arn(case, &before, &expected);
    }
}
