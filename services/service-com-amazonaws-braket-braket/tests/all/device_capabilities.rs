// SPDX-License-Identifier: CC0-1.0

//! The device-capability document `GetDevice` publishes, over the WHOLE inventory.
//!
//! The rule under test: a published device's `deviceCapabilities` document carries the
//! Braket device-schema properties a client introspects the device through — per
//! action type, the operations, result types and pragmas a program may use and the
//! schema versions the action accepts in their published `major.minor` form; and in
//! the `service` section, the shot range, what a run costs and where the device is
//! documented — each value taken from that device's own catalog row.
//!
//! A device is PUBLISHED by Amazon Braket rather than created through the API, so the
//! last suite pins the other half of that: every value of the document, its two
//! documentation links and its published instant included, is data of the catalog entry,
//! so a later boot of the emulator publishes the same document for the same device.
//!
//! The table is the seeded inventory itself: its rows differ in device type, status,
//! accepted action types, shot range, native gate set, result types, noise support and
//! cost unit, so a document that is a fixed stand-in rather than the device's own row
//! fails here. Every assertion goes over HTTP through the production router.

use serde_json::Value;

use crate::quantum_task_loop::{member, Braket, Reply};

/// The action types this inventory's devices accept, from
/// `service_com_amazonaws_braket_braket::state`.
use service_com_amazonaws_braket_braket::state::{ACTION_AHS, ACTION_JAQCD, ACTION_OPENQASM};

/// The device vocabularies, as generated from the Smithy model
/// (`ops::types::shared_types`), so a status or a reason code this service publishes is
/// checked against the model rather than against a list written out here.
use service_com_amazonaws_braket_braket::ops::types::{
    DEVICE_STATUS_REASON_CODE_VALUES, DEVICE_STATUS_VALUES,
};

/// The GATE-MODEL program actions: only a program of gates names operations, result
/// types or pragmas, so only these entries publish them.
const GATE_MODEL_ACTIONS: &[&str] = &[ACTION_OPENQASM, ACTION_JAQCD];

/// The units a `deviceCost` may be charged per.
///
/// Source: the `deviceCost` property of the Braket device-service-properties schema,
/// whose unit is the pricing unit Amazon Braket's pricing page publishes — simulators
/// are charged per minute of simulation, QPUs per shot and per task.
const COST_UNITS: &[&str] = &["minute", "hour", "shot", "task"];

/// The prefix of the pragmas that ask for a result type.
const RESULT_TYPE_PRAGMA_PREFIX: &str = "braket_result_type_";
/// The prefix of the pragmas that apply a noise channel.
const NOISE_PRAGMA_PREFIX: &str = "braket_noise_";

/// The page every device's `externalDocumentationUrl` names: the Amazon Braket developer
/// guide's supported-devices page, which each device's link anchors on its own section.
const DEVICE_LIST_PAGE: &str =
    "https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html";

/// Whether `text` is an instant in the `yyyy-mm-ddThh:mm:ss` form a Braket
/// device-service-properties document carries `updatedAt` in — the published instant of
/// the catalog entry, which is a date and a time of day and nothing else.
///
/// Spelled out here (rather than parsed with a date library) so the suite states the form
/// it requires: ten characters of date, the `T`, eight of time, and digits exactly where
/// the form has digits.
fn is_published_instant(text: &str) -> bool {
    let shape = "0000-00-00T00:00:00";
    text.len() == shape.len()
        && text.chars().zip(shape.chars()).all(|(got, want)| match want {
            '0' => got.is_ascii_digit(),
            other => got == other,
        })
}

/// A result-type name as the OpenQASM pragma that requests it: the Braket pragma
/// names are the result-type name in snake_case under [`RESULT_TYPE_PRAGMA_PREFIX`],
/// so `StateVector` is `braket_result_type_state_vector`. Derived here independently
/// of the service, so the service and the test cannot agree on a wrong name.
fn result_type_pragma(result_type: &str) -> String {
    let mut snake = String::with_capacity(result_type.len() + 4);
    for (index, ch) in result_type.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    format!("{RESULT_TYPE_PRAGMA_PREFIX}{snake}")
}

/// The named member as an array, or a panic naming the case.
fn array<'v>(value: &'v Value, name: &str, case: &str) -> &'v Vec<Value> {
    value
        .get(name)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected an array member {name}, got {value}"))
}

/// Every element of the named array member, as strings.
fn string_array(value: &Value, name: &str, case: &str) -> Vec<String> {
    array(value, name, case)
        .iter()
        .map(|element| {
            element
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected {name} to hold strings, got {element}")
                })
                .to_string()
        })
        .collect()
}

/// Which key a device must publish its computing properties under, derived from the
/// action families its document accepts: a device that computes with gates or with an
/// analog Hamiltonian publishes a `paradigm`, and a device of any other family (an
/// annealer, a photonic processor) has no gate-model paradigm, so what it publishes is
/// its provider's own properties.
fn expected_properties_key(actions: &serde_json::Map<String, Value>) -> &'static str {
    let computes_in_a_paradigm = actions
        .keys()
        .any(|action| GATE_MODEL_ACTIONS.contains(&action.as_str()) || action == ACTION_AHS);
    if computes_in_a_paradigm {
        "paradigm"
    } else {
        "provider"
    }
}

/// The device's own computing-properties section, under the key its family publishes it
/// as: a `paradigm` for a gate-model or analog device, the provider's own properties for
/// a family that has no gate-model paradigm.
fn device_properties<'v>(document: &'v Value, case: &str) -> &'v Value {
    document
        .get("paradigm")
        .or_else(|| document.get("provider"))
        .unwrap_or_else(|| {
            panic!("case {case}: the document publishes no device properties: {document}")
        })
}

/// The parsed capability document of one device, read back over HTTP.
async fn capabilities(braket: &Braket, device_arn: &str) -> Value {
    let reply = braket.get_device(device_arn).await;
    assert_eq!(
        reply.status, 200,
        "case device {device_arn}: GetDevice must answer 200, got {} ({:?}) {}",
        reply.status, reply.error, reply.body
    );
    let document = member(&reply.body, "deviceCapabilities").to_string();
    serde_json::from_str(&document).unwrap_or_else(|e| {
        panic!("case device {device_arn}: deviceCapabilities must be JSON ({e}): {document}")
    })
}

/// Every published device's `service` section carries the shot range, the cost and the
/// documentation properties a client reads, each from that device's catalog row.
#[tokio::test]
async fn the_service_section_publishes_the_cost_and_documentation_properties() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;
    assert!(
        inventory.len() >= 2,
        "the seeded inventory must hold at least two devices to span the rule, got {}",
        inventory.len()
    );

    let mut prices: Vec<String> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let document = capabilities(&braket, arn).await;
        let service = document
            .get("service")
            .unwrap_or_else(|| panic!("case {case}: no service section: {document}"));

        assert_eq!(
            member(
                service
                    .get("braketSchemaHeader")
                    .unwrap_or_else(|| panic!("case {case}: no service schema header: {service}")),
                "name"
            ),
            "braket.device_schema.device_service_properties",
            "case {case}: the service section names its own schema, got {service}"
        );

        assert!(
            !array(service, "executionWindows", &case).is_empty(),
            "case {case}: expected at least one execution window, got {service}"
        );

        let shots = array(service, "shotsRange", &case);
        let bounds: Vec<i64> = shots.iter().filter_map(Value::as_i64).collect();
        assert_eq!(
            bounds.len(),
            2,
            "case {case}: shotsRange is [min, max] of integers, got {shots:?}"
        );
        assert!(
            bounds[0] <= bounds[1],
            "case {case}: expected min <= max in shotsRange, got {bounds:?}"
        );

        // deviceCost: a price and the unit it is charged per.
        let cost = service
            .get("deviceCost")
            .unwrap_or_else(|| panic!("case {case}: no deviceCost: {service}"));
        let price = cost
            .get("price")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: deviceCost.price must be a number: {cost}"));
        assert!(
            price > 0.0 && price.is_finite(),
            "case {case}: expected a positive finite price, got {price}"
        );
        let unit = member(cost, "unit");
        assert!(
            COST_UNITS.contains(&unit),
            "case {case}: expected a published cost unit from {COST_UNITS:?}, got {unit:?}"
        );
        prices.push(format!("{price}/{unit}"));

        // deviceDocumentation: the summary plus both urls.
        let documentation = service
            .get("deviceDocumentation")
            .unwrap_or_else(|| panic!("case {case}: no deviceDocumentation: {service}"));
        let text = member(documentation, "summary");
        assert!(
            !text.trim().is_empty(),
            "case {case}: expected a non-empty documentation summary, got {text:?}"
        );
        summaries.push(text.to_string());
        for url_member in ["imageUrl", "externalDocumentationUrl"] {
            let url = member(documentation, url_member);
            assert!(
                url.starts_with("https://"),
                "case {case}: expected {url_member} to be an https url, got {url:?}"
            );
        }

        // updatedAt: when the catalog entry a caller is reading was published.
        let updated_at = member(service, "updatedAt");
        assert!(
            is_published_instant(updated_at),
            "case {case}: expected updatedAt as a published instant in the \
             yyyy-mm-ddThh:mm:ss form the device-service-properties document carries, \
             got {updated_at:?}"
        );
    }

    // A document built from the device's own row, not one stand-in served for all:
    // the catalog's costs and summaries differ across the inventory.
    summaries.sort();
    summaries.dedup();
    assert_eq!(
        summaries.len(),
        inventory.len(),
        "each device documents itself: expected {} distinct summaries, got {}",
        inventory.len(),
        summaries.len()
    );
    prices.sort();
    prices.dedup();
    assert!(
        prices.len() >= 2,
        "the catalog prices devices differently: expected at least two distinct \
         deviceCost values across the inventory, got {prices:?}"
    );
}

/// Every action entry carries its action type and the `major.minor` schema versions it
/// accepts, and a gate-model entry carries the operations, result types and pragmas a
/// program may use — while an entry of any other action family, whose problems and
/// programs name no gates or pragmas, carries none of them.
#[tokio::test]
async fn every_action_entry_publishes_the_properties_its_schema_defines() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut native_gate_sets: Vec<String> = Vec::new();
    let mut gate_model_entries = 0usize;
    let mut other_family_entries = 0usize;
    let mut declared_schemas: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let is_qpu = member(summary, "deviceType") == "QPU";
        let document = capabilities(&braket, arn).await;
        let actions = document
            .get("action")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case device {arn}: no action map: {document}"));
        assert!(
            !actions.is_empty(),
            "case device {arn}: expected at least one action type, got {document}"
        );

        // The properties go under the key the device's FAMILY publishes them as, and
        // under that key only: a gate-model device publishing a provider section, or an
        // annealer publishing a paradigm, is not the document its schema defines.
        let expected_key = expected_properties_key(actions);
        let other_key = if expected_key == "paradigm" {
            "provider"
        } else {
            "paradigm"
        };
        assert!(
            document.get(expected_key).is_some() && document.get(other_key).is_none(),
            "case device {arn}: the action types {:?} are published under {expected_key}, \
             and not under {other_key}, got {document}",
            actions.keys().collect::<Vec<_>>()
        );

        // Each section of the document names the schema it is an instance of, so a
        // client knows which device-schema model to read it with.
        for (section, header) in [
            ("document", document.get("braketSchemaHeader")),
            (
                "device properties",
                device_properties(&document, &format!("device {arn}")).get("braketSchemaHeader"),
            ),
        ] {
            let header = header.unwrap_or_else(|| {
                panic!("case device {arn}: the {section} names no schema: {document}")
            });
            let name = member(header, "name").to_string();
            assert!(
                name.starts_with("braket.device_schema."),
                "case device {arn} {section}: expected a braket device schema name, got {name:?}"
            );
            if section == "document" {
                declared_schemas.push(name);
            }
        }

        // deviceParameters is the schema a client validates its task parameters
        // against, so it is a schema document rather than an empty object.
        let parameters = document
            .get("deviceParameters")
            .unwrap_or_else(|| panic!("case device {arn}: no deviceParameters: {document}"));
        assert!(
            parameters.get("$schema").and_then(Value::as_str).is_some()
                && parameters.get("type").and_then(Value::as_str) == Some("object"),
            "case device {arn}: expected deviceParameters to be a JSON Schema object, \
             got {parameters}"
        );

        let paradigm = device_properties(&document, &format!("device {arn}"));
        let native_gates: Vec<String> = paradigm
            .get("nativeGateSet")
            .map(|_| string_array(paradigm, "nativeGateSet", &format!("device {arn}")))
            .unwrap_or_default();
        native_gate_sets.push(native_gates.join(","));

        for (action_type, entry) in actions {
            let case = format!("device {arn} action {action_type}");
            assert_eq!(
                member(entry, "actionType"),
                action_type,
                "case {case}: the entry names the action type it is keyed by, got {entry}"
            );

            // The action `version` is the LIST of program schema versions the action
            // accepts, each a `major.minor` BraketSchemas version.
            let versions = string_array(entry, "version", &case);
            assert!(
                !versions.is_empty(),
                "case {case}: expected at least one accepted schema version, got {entry}"
            );
            for version in &versions {
                let parts: Vec<&str> = version.split('.').collect();
                assert!(
                    parts.len() == 2
                        && parts
                            .iter()
                            .all(|part| !part.is_empty()
                                && part.chars().all(|c| c.is_ascii_digit())),
                    "case {case}: expected a major.minor schema version, got {version:?}"
                );
            }

            if !GATE_MODEL_ACTIONS.contains(&action_type.as_str()) {
                other_family_entries += 1;
                for absent in [
                    "supportedOperations",
                    "supportedPragmas",
                    "forbiddenPragmas",
                    "supportedResultTypes",
                ] {
                    assert!(
                        entry.get(absent).is_none(),
                        "case {case}: a program that names no gates has no pragmas or result \
                         types either, so {absent} must be absent, got {entry}"
                    );
                }
                continue;
            }

            gate_model_entries += 1;
            let operations = string_array(entry, "supportedOperations", &case);
            assert!(
                !operations.is_empty(),
                "case {case}: expected the operations a program may use, got {entry}"
            );
            for gate in &native_gates {
                assert!(
                    operations.contains(gate),
                    "case {case}: the native gate {gate:?} must be an operation a program \
                     may use, but supportedOperations is {operations:?}"
                );
            }

            // supportedResultTypes: what the device computes, and the shots each needs.
            let result_types = array(entry, "supportedResultTypes", &case).clone();
            let mut names: Vec<String> = Vec::new();
            for result_type in &result_types {
                let name = member(result_type, "name").to_string();
                let min_shots = result_type
                    .get("minShots")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("case {case}: result type {name} needs minShots: {result_type}")
                    });
                let max_shots = result_type
                    .get("maxShots")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("case {case}: result type {name} needs maxShots: {result_type}")
                    });
                assert!(
                    min_shots <= max_shots && min_shots >= 0,
                    "case {case} result type {name}: expected 0 <= minShots <= maxShots, \
                     got {min_shots}..{max_shots}"
                );
                let device_max = array(
                    document.get("service").expect("a service section"),
                    "shotsRange",
                    &case,
                )[1]
                .as_i64()
                .expect("an integer shot bound");
                assert!(
                    max_shots <= device_max,
                    "case {case} result type {name}: a result type cannot need more shots \
                     than the device accepts ({device_max}), got {max_shots}"
                );
                names.push(name);
            }

            if action_type != ACTION_OPENQASM {
                // Pragmas are an OpenQASM concept; the legacy action declares none.
                continue;
            }

            let supported = string_array(entry, "supportedPragmas", &case);
            let forbidden = string_array(entry, "forbiddenPragmas", &case);
            assert!(
                !supported.is_empty() && !forbidden.is_empty(),
                "case {case}: expected both pragma lists to be published, got \
                 supported {supported:?} forbidden {forbidden:?}"
            );
            for pragma in &supported {
                assert!(
                    !forbidden.contains(pragma),
                    "case {case}: {pragma:?} cannot be both supported and forbidden \
                     (forbidden is {forbidden:?})"
                );
            }

            // A result-type pragma is supported exactly where the device publishes the
            // result type it asks for.
            for name in &names {
                let pragma = result_type_pragma(name);
                assert!(
                    supported.contains(&pragma),
                    "case {case}: result type {name} is published, so {pragma:?} must be \
                     a supported pragma, got {supported:?}"
                );
            }
            for pragma in &forbidden {
                if let Some(suffix) = pragma.strip_prefix(RESULT_TYPE_PRAGMA_PREFIX) {
                    let forbidden_type = names
                        .iter()
                        .find(|name| result_type_pragma(name) == *pragma)
                        .map(String::as_str);
                    assert!(
                        forbidden_type.is_none(),
                        "case {case}: {pragma:?} is forbidden, so the result type it asks \
                         for ({suffix}) must not be published, got {names:?}"
                    );
                }
            }

            // The noise pragmas are an all-or-nothing family: a device either models
            // noise or it does not.
            let noise_supported = supported
                .iter()
                .filter(|pragma| pragma.starts_with(NOISE_PRAGMA_PREFIX))
                .count();
            let noise_forbidden = forbidden
                .iter()
                .filter(|pragma| pragma.starts_with(NOISE_PRAGMA_PREFIX))
                .count();
            assert!(
                (noise_supported == 0) != (noise_forbidden == 0),
                "case {case}: the noise pragmas belong to one list or the other, got \
                 {noise_supported} supported and {noise_forbidden} forbidden"
            );

            // The array limits the OpenQASM action schema defines.
            for limit in ["maximumQubitArrays", "maximumClassicalArrays"] {
                let value = entry
                    .get(limit)
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("case {case}: {limit} must be a number: {entry}"));
                assert!(
                    value >= 1,
                    "case {case}: expected {limit} to admit at least one array, got {value}"
                );
            }
            assert!(
                !string_array(entry, "forbiddenArrayOperations", &case).is_empty(),
                "case {case}: expected the forbidden array operations, got {entry}"
            );

            // The hardware constraints a client checks before it builds a program —
            // booleans, and coherent with the device type and the pragmas the same
            // response publishes.
            for property in [
                "supportPhysicalQubits",
                "requiresAllQubitsMeasurement",
                "requiresContiguousQubitIndices",
                "disabledQubitRewiringSupported",
                "supportsUnassignedMeasurements",
                "supportsPartialVerbatimBox",
            ] {
                assert!(
                    entry.get(property).and_then(Value::as_bool).is_some(),
                    "case {case}: expected {property} as a boolean, got {entry}"
                );
            }
            assert_eq!(
                entry.get("supportPhysicalQubits").and_then(Value::as_bool),
                Some(is_qpu),
                "case {case}: a physical qubit can only be addressed on hardware \
                 (deviceType QPU: {is_qpu}), got {entry}"
            );
            assert_eq!(
                entry
                    .get("supportsPartialVerbatimBox")
                    .and_then(Value::as_bool),
                Some(supported.iter().any(|pragma| pragma == "verbatim")),
                "case {case}: a partial verbatim box is supported exactly where the \
                 verbatim pragma is (supported pragmas {supported:?}), got {entry}"
            );
        }
    }

    declared_schemas.sort();
    declared_schemas.dedup();
    assert!(
        declared_schemas.len() >= 2,
        "a device declares the schema of its OWN family: expected at least two distinct \
         document schemas across the inventory, got {declared_schemas:?}"
    );

    assert!(
        gate_model_entries >= 2 && other_family_entries >= 1,
        "the inventory must span both action families: got {gate_model_entries} gate-model \
         and {other_family_entries} non-gate-model action entries"
    );

    // The device properties report each device's OWN native gates, not one fixed set.
    native_gate_sets.sort();
    native_gate_sets.dedup();
    assert!(
        native_gate_sets.len() >= 3,
        "the catalog's devices run different native gates: expected at least three \
         distinct native gate sets across the inventory, got {native_gate_sets:?}"
    );
}

/// Reading a device twice reports the same document: a published catalog entry does
/// not change because somebody looked at it.
#[tokio::test]
async fn reading_a_device_twice_publishes_the_same_document() {
    let braket = Braket::new();
    for summary in braket.inventory().await {
        let arn = member(&summary, "deviceArn");
        let first = member(&braket.get_device(arn).await.body, "deviceCapabilities").to_string();
        let second = member(&braket.get_device(arn).await.body, "deviceCapabilities").to_string();
        assert_eq!(
            first, second,
            "case device {arn}: the capability document must be stable across reads"
        );
    }
}

/// A published catalog entry is DATA OF THE CATALOG, not of the emulator serving it:
/// every value in it — its two documentation links and the instant it states it was last
/// published included — comes from the device's own entry, so the document one emulator
/// publishes for a device is the document a later one publishes for it. A restart is not
/// a change to the catalog.
///
/// The table is the inventory itself, and the second emulator stands in for the restart:
/// it is built over its own engine, so it seeds its devices again from scratch.
#[tokio::test]
async fn a_published_catalog_entry_is_the_same_document_after_a_restart() {
    let published_by = Braket::new();
    let republished_by = Braket::new();

    let inventory = published_by.inventory().await;
    assert!(
        inventory.len() >= 2,
        "the seeded inventory must hold at least two devices to span the rule, got {}",
        inventory.len()
    );

    let mut images: Vec<String> = Vec::new();
    let mut anchors: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let published = capabilities(&published_by, arn).await;
        let republished = capabilities(&republished_by, arn).await;
        assert_eq!(
            published, republished,
            "case {case}: a later boot must publish the same document, got {published} \
             then {republished}"
        );

        let service = published
            .get("service")
            .unwrap_or_else(|| panic!("case {case}: no service section: {published}"));
        let updated_at = member(service, "updatedAt");
        assert!(
            is_published_instant(updated_at),
            "case {case}: updatedAt is the entry's published instant in the \
             yyyy-mm-ddThh:mm:ss form, got {updated_at:?}"
        );

        let documentation = service
            .get("deviceDocumentation")
            .unwrap_or_else(|| panic!("case {case}: no deviceDocumentation: {service}"));
        let link = member(documentation, "externalDocumentationUrl");
        let anchor = link
            .strip_prefix(DEVICE_LIST_PAGE)
            .and_then(|fragment| fragment.strip_prefix('#'))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the guide's device page anchored on the \
                     device's own documentation section ({DEVICE_LIST_PAGE}#<section>), \
                     got {link:?}"
                )
            });
        assert!(
            !anchor.is_empty(),
            "case {case}: the documentation link must name the section documenting the \
             device, got {link:?}"
        );
        anchors.push(anchor.to_string());
        images.push(member(documentation, "imageUrl").to_string());
    }

    // Each entry carries its OWN published image, not one asset served for every
    // device.
    let mut distinct_images = images.clone();
    distinct_images.sort();
    distinct_images.dedup();
    assert_eq!(
        distinct_images.len(),
        inventory.len(),
        "each device publishes its own image: expected {} distinct imageUrl values, \
         got {images:?}",
        inventory.len()
    );
    // And the catalog documents devices in more than one section, so a link is a
    // property of the entry rather than a single page everything points at.
    let mut distinct_anchors = anchors.clone();
    distinct_anchors.sort();
    distinct_anchors.dedup();
    assert!(
        distinct_anchors.len() >= 2,
        "the catalog documents its devices in different sections: expected at least two \
         distinct anchors across the inventory, got {anchors:?}"
    );
}

/// One `date-time` member of a reported structure, as epoch seconds, so two instants can
/// be compared. A `Timestamp` of this service travels as an RFC 3339 STRING
/// (`@timestampFormat("date-time")`).
fn instant(value: &Value, name: &str, case: &str) -> f64 {
    let rendered = value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a date-time {name}, got {value}"));
    ::aws_smithy_types::DateTime::from_str(
        rendered,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|e| {
        panic!("case {case}: {name} {rendered:?} is not an RFC 3339 date-time: {e}")
    })
    .as_secs_f64()
}

/// RULE: `GetDevice` reports a `deviceStatusReason` for exactly those devices the catalog
/// publishes in a status that HOLDS them out of service — a device taking work has nothing
/// to explain and a device the service has retired is gone for good rather than for a
/// reason that ends — and that reason carries a code the model declares together with the
/// instants it is published with.
///
/// The table is the inventory itself, whose rows differ along the status under test (every
/// value the model declares is published by some device), the device type and the provider.
#[tokio::test]
async fn a_device_publishes_the_reason_for_the_status_it_is_published_in() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut statuses: Vec<String> = Vec::new();
    let mut with_reason: Vec<String> = Vec::new();
    let mut without_reason: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let status = member(summary, "deviceStatus").to_string();
        let case = format!("the {status} device {arn}");
        assert!(
            DEVICE_STATUS_VALUES.contains(&status.as_str()),
            "case {case}: a published status is one the model declares \
             ({DEVICE_STATUS_VALUES:?}), got {status:?}"
        );
        statuses.push(status.clone());

        let reply = braket.get_device(arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: GetDevice must answer 200, got {} ({:?}) {}",
            reply.status, reply.error, reply.body
        );
        assert_eq!(
            member(&reply.body, "deviceStatus"),
            status,
            "case {case}: the device reports the status the catalog publishes it in, \
             got {}",
            reply.body
        );

        // Whether a reason is published is decided by the status alone: `ONLINE` is a
        // device taking work and `RETIRED` a device that is gone, and every other status
        // is the device held out of service for a reason.
        let held_out_of_service = status != "ONLINE" && status != "RETIRED";
        let Some(reason) = reply.body.get("deviceStatusReason") else {
            assert!(
                !held_out_of_service,
                "case {case}: a device held out of service publishes why, got {}",
                reply.body
            );
            without_reason.push(status);
            continue;
        };
        assert!(
            held_out_of_service,
            "case {case}: this status has no reason to publish, got {reason}"
        );

        let code = member(reason, "reason");
        assert!(
            DEVICE_STATUS_REASON_CODE_VALUES.contains(&code),
            "case {case}: a published reason is one the model declares \
             ({DEVICE_STATUS_REASON_CODE_VALUES:?}), got {code:?}"
        );
        // `updatedAt` is required, so it is always there; the two window members are
        // optional, and whichever of them is published bounds the other way round.
        let updated_at = instant(reason, "updatedAt", &case);
        if reason.get("startedAt").is_some() {
            let started_at = instant(reason, "startedAt", &case);
            assert!(
                started_at <= updated_at,
                "case {case}: the reason cannot be updated ({updated_at}) before it \
                 started ({started_at})"
            );
            if reason.get("estimatedEndAt").is_some() {
                let estimated_end_at = instant(reason, "estimatedEndAt", &case);
                assert!(
                    estimated_end_at >= started_at,
                    "case {case}: the reason cannot be estimated to end \
                     ({estimated_end_at}) before it started ({started_at})"
                );
            }
        }
        with_reason.push(status);
    }

    // Both halves of the rule are exercised by the catalog, and every status the model
    // declares is published by some device, so no row of the vocabulary goes untested.
    assert!(
        !with_reason.is_empty() && !without_reason.is_empty(),
        "case the published catalog: the table must span both halves of the rule, got \
         {with_reason:?} with a reason and {without_reason:?} without one"
    );
    for status in DEVICE_STATUS_VALUES {
        assert!(
            statuses.iter().any(|published| published == status),
            "case the published catalog: every modelled device status is published by \
             some device, got {statuses:?} with no {status}"
        );
    }
}

/// RULE: a status reason is DATA OF THE CATALOG ENTRY, like every other value of it: a
/// device published in a status reports the same reason however often it is read and
/// whichever emulator publishes it, so nothing about it is observed from the clock.
///
/// The table is the inventory again, and the second emulator stands in for a restart.
#[tokio::test]
async fn a_published_status_reason_is_the_same_reason_after_a_restart() {
    let published_by = Braket::new();
    let republished_by = Braket::new();

    let mut published_reasons = 0_usize;
    for summary in published_by.inventory().await {
        let arn = member(&summary, "deviceArn");
        let case = format!("device {arn}");
        let reason = |reply: Reply| reply.body.get("deviceStatusReason").cloned();
        let first = reason(published_by.get_device(arn).await);
        let again = reason(published_by.get_device(arn).await);
        let republished = reason(republished_by.get_device(arn).await);
        assert_eq!(
            first, again,
            "case {case}: reading a device twice reports the same status reason, got \
             {first:?} then {again:?}"
        );
        assert_eq!(
            first, republished,
            "case {case}: a later boot publishes the same status reason, got {first:?} \
             then {republished:?}"
        );
        if first.is_some() {
            published_reasons += 1;
        }
    }

    assert!(
        published_reasons >= 1,
        "case the published catalog: the rule is only exercised by a device that \
         publishes a status reason, and the catalog published none"
    );
}

/// The operations Amazon Braket accepts in a gate-model program, in the names its
/// device-action schemas publish them under.
///
/// Source: the `supportedOperations` property of the Braket gate-model device-action
/// schemas — Amazon Braket's supported-operations documentation for a program, whose
/// standard catalog is published first and the provider-native gates it also compiles
/// after it. Written out here, independently of the service, so a catalog the service
/// grew or shrank by itself fails this suite. A device publishes these and the gates it
/// runs natively, and nothing else.
const PROGRAM_OPERATIONS: &[&str] = &[
    "barrier",
    "ccnot",
    "cnot",
    "cphaseshift",
    "cphaseshift00",
    "cphaseshift01",
    "cphaseshift10",
    "cswap",
    "cy",
    "cz",
    "ecr",
    "h",
    "i",
    "iswap",
    "pswap",
    "phaseshift",
    "rx",
    "ry",
    "rz",
    "s",
    "si",
    "swap",
    "t",
    "ti",
    "v",
    "vi",
    "x",
    "xx",
    "xy",
    "y",
    "yy",
    "z",
    "zz",
    "gpi",
    "gpi2",
    "ms",
];

/// What a gate-model program expresses WITHOUT naming an operation, so no published
/// `supportedOperations` carries it: the statements that measure, reset or globally
/// phase a register, the standard gate library's general single-qubit unitary the named
/// gates are built out of, and a caller-supplied matrix — which a program asks for with
/// a pragma, not by naming a gate.
const NOT_PUBLISHED_AS_OPERATIONS: &[&str] = &["measure", "reset", "gphase", "u", "unitary"];

/// The pragmas that apply a matrix the PROGRAM supplies: an arbitrary unitary, and the
/// rotation into the basis a measurement is taken in.
///
/// Source: the `supportedPragmas` / `forbiddenPragmas` vocabulary of the Braket OpenQASM
/// device-action schema.
const MATRIX_PRAGMAS: &[&str] = &["braket_unitary_matrix", "braket_basis_rotation"];

/// The result types that report the WHOLE computed state, which an OpenQASM program
/// cannot ask for.
///
/// Source: the `braket_result_type_*` pragmas the Braket OpenQASM device-action schema
/// forbids; a JAQCD program names its result types directly instead.
const WHOLE_STATE_RESULT_TYPES: &[&str] = &["StateVector", "DensityMatrix"];

/// The gate-model action entries of one device's document, as (action type, entry).
fn gate_model_entries(document: &Value, case: &str) -> Vec<(String, Value)> {
    document
        .get("action")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case}: no action map: {document}"))
        .iter()
        .filter(|(action_type, _)| GATE_MODEL_ACTIONS.contains(&action_type.as_str()))
        .map(|(action_type, entry)| (action_type.clone(), entry.clone()))
        .collect()
}

/// The names of the result types one action entry publishes.
fn result_type_names(entry: &Value, case: &str) -> Vec<String> {
    array(entry, "supportedResultTypes", case)
        .iter()
        .map(|result_type| member(result_type, "name").to_string())
        .collect()
}

/// The device's native gate set, empty for a device that publishes none.
fn native_gate_set(document: &Value, case: &str) -> Vec<String> {
    let properties = device_properties(document, case);
    properties
        .get("nativeGateSet")
        .map(|_| string_array(properties, "nativeGateSet", case))
        .unwrap_or_default()
}

/// RULE: a gate-model action entry publishes the operations the service ACCEPTS in a
/// program for that device — the catalog of operations Braket compiles for a gate-model
/// device, plus the gates the device runs natively — and nothing a program expresses in
/// any other way, because `supportedOperations` is what a client builds a submittable
/// program out of.
///
/// The table is the whole inventory, whose rows differ in device type, provider, native
/// gate set (empty, and three different hardware sets) and action family, so the rule is
/// checked on entries the catalog publishes for every family rather than on one device.
#[tokio::test]
async fn a_gate_model_entry_publishes_the_operations_the_service_accepts() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut checked_entries = 0usize;
    let mut with_native_gates = 0usize;
    let mut beyond_the_catalog: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let document = capabilities(&braket, arn).await;
        let natives = native_gate_set(&document, &case);

        for (action_type, entry) in gate_model_entries(&document, &case) {
            let case = format!("device {arn} action {action_type}");
            checked_entries += 1;
            let published = string_array(&entry, "supportedOperations", &case);

            for operation in &published {
                assert!(
                    PROGRAM_OPERATIONS.contains(&operation.as_str())
                        || natives.contains(operation),
                    "case {case}: {operation:?} is neither an operation Braket accepts \
                     in a program ({PROGRAM_OPERATIONS:?}) nor a gate this device runs \
                     natively ({natives:?}), so it must not be published, got {published:?}"
                );
            }
            for statement in NOT_PUBLISHED_AS_OPERATIONS {
                assert!(
                    !published.contains(&(*statement).to_string()),
                    "case {case}: {statement:?} is not an operation a program names, so \
                     it must not be published, got {published:?}"
                );
            }
            for native in &natives {
                assert!(
                    published.contains(native),
                    "case {case}: the device runs {native:?} natively, so a program may \
                     name it, got {published:?}"
                );
                if !PROGRAM_OPERATIONS.contains(&native.as_str()) {
                    beyond_the_catalog.push(native.clone());
                }
            }
            if natives.is_empty() {
                let published: Vec<&str> = published.iter().map(String::as_str).collect();
                assert_eq!(
                    published, PROGRAM_OPERATIONS,
                    "case {case}: a device with no native gate layer publishes exactly \
                     the catalog Braket compiles for it, got {published:?}"
                );
            } else {
                with_native_gates += 1;
            }
        }
    }

    assert!(
        checked_entries >= 2 && with_native_gates >= 1,
        "case the published catalog: the table must span devices with and without a \
         native gate layer, got {checked_entries} gate-model entries of which \
         {with_native_gates} publish native gates"
    );
    assert!(
        !beyond_the_catalog.is_empty(),
        "case the published catalog: the rule's second half is only exercised by a \
         device whose native gates the catalog does not already name, and the inventory \
         published none"
    );
}

/// RULE: the two pragma lists of an OpenQASM entry classify the pragmas by what the
/// DEVICE can do: a pragma that applies a matrix the program supplies is supported
/// exactly where the device applies any matrix it is handed (a device with no native
/// gate layer) and forbidden on a device that compiles the program to its own native
/// gates; and `verbatim`, which addresses that native gate layer, is published only
/// where there IS one — a device without one publishes it in neither list rather than
/// refusing a pragma that means nothing to it.
///
/// The table is the whole inventory again: it spans devices with and without a native
/// gate layer, and the simulator that models noise.
#[tokio::test]
async fn the_pragma_lists_classify_a_pragma_by_what_the_device_can_do() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut applies_any_matrix = 0usize;
    let mut compiles_to_natives = 0usize;

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let document = capabilities(&braket, arn).await;
        let natives = native_gate_set(&document, &case);

        for (action_type, entry) in gate_model_entries(&document, &case) {
            if action_type != ACTION_OPENQASM {
                continue;
            }
            let case = format!("device {arn} action {action_type}");
            let supported = string_array(&entry, "supportedPragmas", &case);
            let forbidden = string_array(&entry, "forbiddenPragmas", &case);

            for pragma in MATRIX_PRAGMAS {
                let pragma = (*pragma).to_string();
                assert_eq!(
                    supported.contains(&pragma),
                    natives.is_empty(),
                    "case {case}: {pragma:?} applies a matrix the program supplies, so \
                     it is supported exactly where the device has no native gate layer \
                     (native gates {natives:?}), got supported {supported:?}"
                );
                assert!(
                    supported.contains(&pragma) != forbidden.contains(&pragma),
                    "case {case}: {pragma:?} belongs to exactly one list, got supported \
                     {supported:?} forbidden {forbidden:?}"
                );
            }

            let verbatim = "verbatim".to_string();
            assert!(
                !forbidden.contains(&verbatim),
                "case {case}: a device that runs no verbatim box publishes no verbatim \
                 pragma at all, got forbidden {forbidden:?}"
            );
            assert_eq!(
                supported.contains(&verbatim),
                !natives.is_empty(),
                "case {case}: a verbatim box addresses the device's native gate layer \
                 (native gates {natives:?}), got supported {supported:?}"
            );

            if natives.is_empty() {
                applies_any_matrix += 1;
            } else {
                compiles_to_natives += 1;
            }
        }
    }

    assert!(
        applies_any_matrix >= 1 && compiles_to_natives >= 1,
        "case the published catalog: the table must span both halves of the rule, got \
         {applies_any_matrix} devices with no native gate layer and \
         {compiles_to_natives} with one"
    );
}

/// RULE: a result type is published for the action whose language can ASK for it. An
/// OpenQASM program asks for a result type with a `braket_result_type_*` pragma, and the
/// OpenQASM action schema forbids the two pragmas that read the whole computed state, so
/// an OpenQASM entry publishes neither of those result types and forbids both pragmas;
/// the legacy JAQCD entry, whose programs name their result types directly, publishes
/// whichever of them the device computes.
///
/// The table is the inventory: it spans the device that computes the state vector, the
/// one that computes the density matrix, a simulator that computes neither and hardware
/// that only ever samples.
#[tokio::test]
async fn an_openqasm_entry_publishes_only_the_result_types_a_pragma_can_ask_for() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut published_by_jaqcd: Vec<String> = Vec::new();
    let mut openqasm_entries = 0usize;

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let document = capabilities(&braket, arn).await;

        for (action_type, entry) in gate_model_entries(&document, &case) {
            let case = format!("device {arn} action {action_type}");
            let names = result_type_names(&entry, &case);
            if action_type != ACTION_OPENQASM {
                for name in &names {
                    if WHOLE_STATE_RESULT_TYPES.contains(&name.as_str()) {
                        published_by_jaqcd.push(format!("{arn} {name}"));
                    }
                }
                continue;
            }
            openqasm_entries += 1;
            let forbidden = string_array(&entry, "forbiddenPragmas", &case);
            for whole_state in WHOLE_STATE_RESULT_TYPES {
                assert!(
                    !names.contains(&(*whole_state).to_string()),
                    "case {case}: an OpenQASM program cannot ask for {whole_state:?}, so \
                     the entry publishes no such result type, got {names:?}"
                );
                let pragma = result_type_pragma(whole_state);                assert!(
                    forbidden.contains(&pragma),
                    "case {case}: {pragma:?} is forbidden in an OpenQASM program, got \
                     forbidden {forbidden:?}"
                );
            }
            assert!(
                !names.is_empty(),
                "case {case}: the entry still publishes the result types a pragma can \
                 ask for, got {entry}"
            );
        }
    }

    assert!(
        openqasm_entries >= 2,
        "case the published catalog: expected the rule to be checked on more than one \
         OpenQASM entry, got {openqasm_entries}"
    );
    // The other half: a device that COMPUTES a whole-state result type still publishes
    // it for the action whose programs can name it.
    assert!(
        published_by_jaqcd.len() >= 2,
        "case the published catalog: the devices that compute a whole-state result type \
         publish it on their legacy action, got {published_by_jaqcd:?}"
    );
}

/// RULE: the published document carries its sections in the order the published catalog
/// document lays them out, led by the `service` section a client reads to decide whether
/// it may submit work at all.
///
/// The table is the whole inventory, whose documents differ in schema, action families
/// and device-properties section.
#[tokio::test]
async fn the_published_document_leads_with_the_service_section() {
    let braket = Braket::new();
    for summary in braket.inventory().await {
        let arn = member(&summary, "deviceArn");
        let document = member(&braket.get_device(arn).await.body, "deviceCapabilities").to_string();
        assert!(
            document.starts_with("{\"service\":"),
            "case device {arn}: the document leads with its service section, got {document}"
        );
    }
}
