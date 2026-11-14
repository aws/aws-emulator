// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the schema-registry WRITES that change a stored record: the
//! `UpdateRegistry` whose description a later `GetRegistry` serves, the `UpdateSchema` whose
//! compatibility mode and description a later `GetSchema` serves, and the version numbering
//! those reads report.
//!
//! The rules under test:
//!
//! 1. An update of a registry writes the `Description` it carries onto the stored registry,
//!    so every later read of that registry reports the new description — whichever form the
//!    `RegistryId` addressed it by — and a `RegistryId` naming a registry this account holds
//!    none of is the modelled miss (`EntityNotFoundException`, HTTP 400).
//! 2. An update of a schema set writes the members it carries — the new `Compatibility`, the
//!    new `Description` — onto the stored schema set, so every later read reports them; an
//!    update that names neither a `Compatibility` nor a `SchemaVersionNumber` asks for
//!    nothing and is refused, and a version number naming a version the set does not hold is
//!    refused as well.
//! 3. A version NUMBER is assigned once and never reused: `LatestSchemaVersion` is the number
//!    the last registration took and `NextSchemaVersion` is one past it, so deleting versions
//!    frees neither their numbers nor the numbers above them.
//! 4. A `SchemaDefinition` the schema set's own `DataFormat` does not accept registers no
//!    version at all: the call is refused with the service's own `InvalidInputException`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the form and
//! length of the description (the empty one and the published maximum included), which of the
//! modelled `Compatibility` values the update names, how the record is addressed (by name and
//! by ARN), and how many versions were registered and which of them were deleted.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, DEFAULT_REGISTRY_NAME, SCHEMA_UPDATE_REQUIRES_ONE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The Region the harness serves every call in, which the ARNs below are minted for.
const REGION: &str = "us-east-1";

/// The longest `Description` the model accepts on either update: `DescriptionString`,
/// "len 0..2048" (`smithy_query operation=UpdateRegistry`).
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// Every value the modelled `Compatibility` enum admits, in the order the model lists them
/// (`smithy_query operation=UpdateSchema`, member `Compatibility`).
const COMPATIBILITY_VALUES: [&str; 8] = [
    "BACKWARD",
    "BACKWARD_ALL",
    "DISABLED",
    "FORWARD",
    "FORWARD_ALL",
    "FULL",
    "FULL_ALL",
    "NONE",
];

/// An Avro record schema: the first version of every schema set below.
const AVRO_BASE: &str = concat!(
    r#"{"type":"record","name":"Payment","namespace":"com.example","fields":"#,
    r#"[{"name":"id","type":"string"}]}"#
);

/// The ARN of registry `name` as this endpoint mints it.
fn registry_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:registry/{name}")
}

/// A description of exactly `length` characters.
fn description_of_length(length: usize) -> String {
    "d".repeat(length)
}

/// An Avro record schema carrying `fields` many optional fields, so every row of a numbering
/// table registers a definition no other row registered.
fn avro_with_fields(fields: usize) -> String {
    let declared: Vec<String> = (0..fields)
        .map(|index| format!(r#"{{"name":"f{index}","type":["null","string"],"default":null}}"#))
        .collect();
    format!(
        r#"{{"type":"record","name":"Payment","namespace":"com.example","fields":[{}]}}"#,
        declared.join(",")
    )
}

/// Create the registry `name` with `description`.
async fn given_registry(router: &MultiServiceRouter, name: &str, description: &str) {
    ok(
        router,
        "CreateRegistry",
        &json!({ "RegistryName": name, "Description": description }),
    )
    .await;
}

/// Create the schema set `name` in the default registry, holding [`AVRO_BASE`] as its first
/// version under the compatibility mode `compatibility`.
async fn given_schema(router: &MultiServiceRouter, name: &str, compatibility: &str) {
    ok(
        router,
        "CreateSchema",
        &json!({
            "SchemaName": name,
            "DataFormat": "AVRO",
            "Compatibility": compatibility,
            "SchemaDefinition": AVRO_BASE,
        }),
    )
    .await;
}

/// The `GetSchema` answer for the schema set `name` of the default registry.
async fn schema_read(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "GetSchema",
        &json!({ "SchemaId": { "SchemaName": name } }),
    )
    .await
}

/// An update of a registry is what every later read of it reports, whichever form the
/// request addressed the registry by.
///
/// The rows differ along the form and length of the description and the form of the
/// `RegistryId`: a plain sentence, the empty description, the published maximum, one carrying
/// the line breaks the pattern admits, and an update addressed by ARN rather than by name.
#[tokio::test]
async fn an_update_of_a_registry_is_what_a_later_read_reports() {
    // (case, the description the update carries, whether the update addresses the registry
    //  by its ARN)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a plain description", "the edited registry".to_string(), false),
        ("the empty description", String::new(), false),
        (
            "the published maximum length",
            description_of_length(MAX_DESCRIPTION_LENGTH),
            false,
        ),
        (
            "a description carrying the line breaks the pattern admits",
            "first line\r\nsecond line\tindented".to_string(),
            false,
        ),
        (
            "an update addressed by the registry's ARN",
            "addressed by ARN".to_string(),
            true,
        ),
    ];

    for (index, (case, description, by_arn)) in rows.into_iter().enumerate() {
        let router = emulator();
        let registry = format!("update-registry-{index}");
        given_registry(&router, &registry, "the description the create carried").await;

        let registry_id = if by_arn {
            json!({ "RegistryArn": registry_arn(&registry) })
        } else {
            json!({ "RegistryName": registry })
        };
        let updated = ok(
            &router,
            "UpdateRegistry",
            &json!({ "RegistryId": registry_id, "Description": description }),
        )
        .await;
        assert_eq!(
            updated.get("RegistryName").and_then(Value::as_str),
            Some(registry.as_str()),
            "case {case}: the update reports the registry it wrote to, got {updated}"
        );
        assert_eq!(
            updated.get("RegistryArn").and_then(Value::as_str),
            Some(registry_arn(&registry).as_str()),
            "case {case}: the update reports that registry's ARN, got {updated}"
        );

        let read = ok(
            &router,
            "GetRegistry",
            &json!({ "RegistryId": { "RegistryName": registry } }),
        )
        .await;
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            Some(description.as_str()),
            "case {case}: the read must report the description the update wrote, got {read}"
        );
        // Two reads report the same thing, so the read serves what the write stored rather
        // than anything computed per call.
        let again = ok(
            &router,
            "GetRegistry",
            &json!({ "RegistryId": { "RegistryName": registry } }),
        )
        .await;
        assert_eq!(
            again.get("Description"),
            read.get("Description"),
            "case {case}: two reads of an updated registry must agree, got {again}"
        );
    }
}

/// An update of a registry this account holds none of is the modelled miss, and writes
/// nothing.
#[tokio::test]
async fn an_update_of_an_absent_registry_is_the_modelled_miss() {
    let router = emulator();
    for (case, registry_id, named) in [
        (
            "a name nothing was created under",
            json!({ "RegistryName": "no-such-registry" }),
            "no-such-registry",
        ),
        (
            "an ARN naming a registry nothing was created under",
            json!({ "RegistryArn": registry_arn("no-such-registry") }),
            "no-such-registry",
        ),
        (
            "an ARN of another account's registry",
            json!({ "RegistryArn": "arn:aws:glue:us-east-1:999999999999:registry/elsewhere" }),
            "elsewhere",
        ),
    ] {
        let (status, body) = call(
            &router,
            "UpdateRegistry",
            &json!({ "RegistryId": registry_id, "Description": "an edit with nowhere to land" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::registry_not_found(named),
            "case {case}: the refusal names the registry the request addressed, got {body}"
        );
    }
}

/// An update of a schema set is what every later read of it reports: the new compatibility
/// mode and the new description, for every value the modelled enum admits.
#[tokio::test]
async fn an_update_of_a_schema_set_is_what_a_later_read_reports() {
    for (index, compatibility) in COMPATIBILITY_VALUES.into_iter().enumerate() {
        let case = compatibility;
        let router = emulator();
        let schema = format!("update-schema-{index}");
        // Created under a mode the row does not ask for, so what the read reports can only
        // have come from the update.
        let created_under = if compatibility == "NONE" {
            "BACKWARD"
        } else {
            "NONE"
        };
        given_schema(&router, &schema, created_under).await;

        let description = format!("the schema set under {compatibility}");
        let updated = ok(
            &router,
            "UpdateSchema",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "Compatibility": compatibility,
                "Description": description,
            }),
        )
        .await;
        assert_eq!(
            updated.get("SchemaName").and_then(Value::as_str),
            Some(schema.as_str()),
            "case {case}: the update reports the schema set it wrote to, got {updated}"
        );
        assert_eq!(
            updated.get("RegistryName").and_then(Value::as_str),
            Some(DEFAULT_REGISTRY_NAME),
            "case {case}: the update reports the registry that holds it, got {updated}"
        );

        let read = schema_read(&router, &schema).await;
        assert_eq!(
            read.get("Compatibility").and_then(Value::as_str),
            Some(compatibility),
            "case {case}: the read must report the mode the update wrote, got {read}"
        );
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            Some(description.as_str()),
            "case {case}: the read must report the description the update wrote, got {read}"
        );
    }
}

/// An update that asks for nothing, and one naming a version the schema set does not hold,
/// are refused with the operation's own input error and change nothing.
#[tokio::test]
async fn an_update_a_schema_set_cannot_act_on_is_refused() {
    // (case, the UpdateSchema body minus its SchemaId, the whole Message the refusal carries)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "neither a compatibility mode nor a version number",
            json!({ "Description": "an edit that says nothing" }),
            SCHEMA_UPDATE_REQUIRES_ONE.to_string(),
        ),
        (
            "a version number the schema set does not hold",
            json!({ "SchemaVersionNumber": { "VersionNumber": 9 } }),
            messages::SCHEMA_VERSION_NOT_FOUND.to_string(),
        ),
        (
            "a version number naming neither of its two members",
            json!({ "SchemaVersionNumber": { "LatestVersion": false } }),
            "One of LatestVersion or VersionNumber has to be provided.".to_string(),
        ),
    ];

    for (index, (case, asked, expected)) in rows.into_iter().enumerate() {
        let router = emulator();
        let schema = format!("refused-update-{index}");
        given_schema(&router, &schema, "BACKWARD").await;

        let mut body = asked;
        body["SchemaId"] = json!({ "SchemaName": schema });
        let (status, answered) = call(&router, "UpdateSchema", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answered}"
        );
        assert!(
            error_code(&answered).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case}: expected the refusal's own wording, got {answered}"
        );

        // Nothing was written: the schema set still reports what its create recorded.
        let read = schema_read(&router, &schema).await;
        assert_eq!(
            read.get("Compatibility").and_then(Value::as_str),
            Some("BACKWARD"),
            "case {case}: a refused update must leave the mode as it was, got {read}"
        );
        assert!(
            read.get("Description").is_none_or(Value::is_null),
            "case {case}: a refused update must leave the description as it was, got {read}"
        );
    }
}

/// The `SchemaCheckpoint` a schema set carries is the version in force when its definition
/// was registered: an update validates the version number it names against the versions the
/// set holds and leaves that checkpoint where it was, which a later read reports and which
/// the delete of a version then refuses to remove.
///
/// RECORDED behaviour: the recorded read of a schema set whose compatibility and description
/// this operation updated reports the checkpoint its CREATE set, and replaying that fixture
/// with the named version written through diverges on `$.SchemaCheckpoint`.
///
/// The rows differ along how the version is named — "the latest version available for the
/// schema" and an explicit version number — and along whether the update names one at all.
#[tokio::test]
async fn an_update_validates_its_version_number_and_keeps_the_checkpoint() {
    // (case, the SchemaVersionNumber the update names)
    let rows: Vec<(&str, Option<Value>)> = vec![
        (
            "the latest version available for the schema",
            Some(json!({ "LatestVersion": true })),
        ),
        ("an explicit version number", Some(json!({ "VersionNumber": 2 }))),
        ("no version number at all", None),
    ];

    for (index, (case, named)) in rows.into_iter().enumerate() {
        let router = emulator();
        let schema = format!("checkpointed-schema-{index}");
        // `NONE` accepts any further version without a compatibility check, so the rows vary
        // only in which version the update names.
        given_schema(&router, &schema, "NONE").await;
        for fields in 1..=2 {
            ok(
                &router,
                "RegisterSchemaVersion",
                &json!({
                    "SchemaId": { "SchemaName": schema },
                    "SchemaDefinition": avro_with_fields(fields),
                }),
            )
            .await;
        }
        // The create registered the first version, so that is the checkpoint it set.
        let created_checkpoint = 1;
        let before = schema_read(&router, &schema).await;
        assert_eq!(
            before["SchemaCheckpoint"],
            json!(created_checkpoint),
            "case {case}: the create check-pointed the version it registered, got {before}"
        );

        let mut body = json!({
            "SchemaId": { "SchemaName": schema },
            "Compatibility": "FULL",
        });
        if let Some(named) = named {
            body["SchemaVersionNumber"] = named;
        }
        ok(&router, "UpdateSchema", &body).await;

        let read = schema_read(&router, &schema).await;
        assert_eq!(
            read["SchemaCheckpoint"],
            json!(created_checkpoint),
            "case {case}: the update leaves the checkpoint where the create set it, got {read}"
        );
        assert_eq!(
            read.get("Compatibility").and_then(Value::as_str),
            Some("FULL"),
            "case {case}: the update still wrote the mode it carried, got {read}"
        );

        // "When the range of version numbers contain check pointed version, the API will …
        // not proceed with the deletion": the checkpoint the set carries is the version the
        // delete refuses to remove, so the stored value is observable through that refusal
        // too.
        let (status, refused) = call(
            &router,
            "DeleteSchemaVersions",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "Versions": created_checkpoint.to_string(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a delete of the check-pointed version is refused, got {status} with \
             {refused}"
        );
        assert!(
            error_code(&refused).contains("ConcurrentModificationException"),
            "case {case}: expected ConcurrentModificationException, got {refused}"
        );
    }
}

/// A version number is assigned once and never reused: the counters a read reports count the
/// registrations the schema set has taken, so deleting versions frees neither their numbers
/// nor the numbers above them — and the next registration takes the number the read
/// announced.
///
/// The rows differ along how many versions were registered and which of them were then
/// deleted: none deleted, the latest one, one in the middle, and a range of two.
#[tokio::test]
async fn version_numbers_count_registrations_and_are_never_reused() {
    // (case, how many versions to register after the create, the `Versions` span to delete)
    let rows: Vec<(&str, usize, Option<&str>)> = vec![
        ("nothing deleted", 2, None),
        ("the latest version deleted", 2, Some("3")),
        ("one version in the middle deleted", 3, Some("3")),
        ("a range of two deleted", 3, Some("3-4")),
    ];

    for (index, (case, registered, deleted)) in rows.into_iter().enumerate() {
        let router = emulator();
        let schema = format!("numbered-schema-{index}");
        // `NONE` "accepts any new version without undergoing a compatibility check", so what
        // the rows vary is the numbering and not the compatibility of each definition.
        given_schema(&router, &schema, "NONE").await;

        for fields in 1..=registered {
            ok(
                &router,
                "RegisterSchemaVersion",
                &json!({
                    "SchemaId": { "SchemaName": schema },
                    "SchemaDefinition": avro_with_fields(fields),
                }),
            )
            .await;
        }
        // The create registered the first version, so this many numbers have been assigned.
        let assigned = i64::try_from(registered + 1).expect("a row registers few versions");

        if let Some(span) = deleted {
            ok(
                &router,
                "DeleteSchemaVersions",
                &json!({ "SchemaId": { "SchemaName": schema }, "Versions": span }),
            )
            .await;
        }

        let read = schema_read(&router, &schema).await;
        assert_eq!(
            read["LatestSchemaVersion"],
            json!(assigned),
            "case {case}: {assigned} version numbers have been assigned, got {read}"
        );
        assert_eq!(
            read["NextSchemaVersion"],
            json!(assigned + 1),
            "case {case}: the next registration takes {}, got {read}",
            assigned + 1
        );

        // And it really does: the next registration takes the number the read announced.
        let next = ok(
            &router,
            "RegisterSchemaVersion",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaDefinition": avro_with_fields(registered + 1),
            }),
        )
        .await;
        assert_eq!(
            next["VersionNumber"],
            json!(assigned + 1),
            "case {case}: the registration must take the announced number, got {next}"
        );
    }
}

/// A definition the schema set's data format does not accept registers no version: the call
/// is refused with the service's own input error, and the schema set is left as it was.
///
/// The rows differ along how the text fails to be a schema document of that format: text that
/// is not JSON at all, JSON that is not a schema, and a record declaration missing the member
/// a record needs.
#[tokio::test]
async fn a_definition_the_data_format_does_not_accept_registers_no_version() {
    let rows: Vec<(&str, &str)> = vec![
        ("text that is not JSON at all", "not a schema"),
        ("JSON that is no schema document", r#"{"unrelated":true}"#),
        (
            "a record declaration with no fields member",
            r#"{"type":"record","name":"Payment"}"#,
        ),
    ];

    for (index, (case, definition)) in rows.into_iter().enumerate() {
        let router = emulator();
        let schema = format!("invalid-definition-{index}");
        given_schema(&router, &schema, "NONE").await;

        let (status, body) = call(
            &router,
            "RegisterSchemaVersion",
            &json!({
                "SchemaId": { "SchemaName": schema },
                "SchemaDefinition": definition,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert!(
            error_message(&body)
                .starts_with("Schema definition of AVRO data format is invalid: "),
            "case {case}: the refusal says which format the definition is not one of, got \
             {body}"
        );

        // Nothing was registered: the counters are where the create left them.
        let read = schema_read(&router, &schema).await;
        assert_eq!(
            read["LatestSchemaVersion"],
            json!(1),
            "case {case}: a refused registration assigns no version number, got {read}"
        );
        assert_eq!(
            read["NextSchemaVersion"],
            json!(2),
            "case {case}: a refused registration assigns no version number, got {read}"
        );

        // The same text is refused by a CREATE that would register it as a first version.
        let (create_status, create_body) = call(
            &router,
            "CreateSchema",
            &json!({
                "SchemaName": format!("{schema}-created"),
                "DataFormat": "AVRO",
                "SchemaDefinition": definition,
            }),
        )
        .await;
        assert_eq!(
            create_status, 400,
            "case {case}: a create registering the same text is refused, got {create_status} \
             with {create_body}"
        );
        assert!(
            error_code(&create_body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException from the create, got {create_body}"
        );
    }
}
