// SPDX-License-Identifier: CC0-1.0

//! The device-parameters document a quantum task RAN UNDER.
//!
//! `CreateQuantumTask`'s `deviceParameters` is OPTIONAL while `GetQuantumTask` reports it
//! as REQUIRED — "The parameters for the device on which the quantum task ran"
//! (`smithy_query(query="operation",operation="CreateQuantumTask,GetQuantumTask")`) — so
//! every task has such a document whether or not the create carried one: a create that
//! supplies the document runs under it, and a create that omits it runs under the DEFAULT
//! parameters document of the device it named, which is what the read paths then report.
//!
//! A gate-model device's parameters document is the device's own parameters schema wrapping
//! the gate-model paradigm parameters the program needs: how many qubits the program
//! declares, and whether the device may rewire them. Both come from the task itself — the
//! qubit count from the program that ran, the schema name from the device the task named —
//! so nothing here is a value fitted to one request.

use super::quantum_simulation::TaskMeasurements;
use super::Device;

/// The version every Braket device-schema document is published at, as the schema package
/// numbers them (the same version [`super::task_results`] writes into a result document).
const SCHEMA_VERSION: &str = "1";

/// The schema of the gate-model paradigm parameters nested in a gate-model device's
/// parameters document.
const GATE_MODEL_PARAMETERS_SCHEMA: &str = "braket.device_schema.gate_model_parameters";

/// Whether the device may NOT rewire the program's qubits onto other physical ones.
///
/// A program is submitted without pinning its qubits unless the caller says otherwise, so
/// the default document reports rewiring as enabled.
const DISABLE_QUBIT_REWIRING_DEFAULT: bool = false;

/// The document a task carries when nothing states its parameters: an empty JSON object,
/// so the required member is still a JSON document a client can parse.
pub const EMPTY_PARAMETERS: &str = "{}";

/// The schema `device`'s own parameters document declares itself to be an instance of.
///
/// A client reads this header to pick the device-schema model it validates its parameters
/// with, so it names the concrete schema of the device's family: the gate-model simulator
/// schema for a simulator, and the provider's own device-parameters schema for a QPU, which
/// lives in the module named after the provider segment of the device's ARN — the same
/// family rule the seeded inventory names a device's capability schema by (the
/// `capabilities_schema` of this module's parent).
#[must_use]
pub fn parameters_schema(device: &Device) -> String {
    if device.device_type == super::DEVICE_TYPE_SIMULATOR {
        "braket.device_schema.simulators.gate_model_simulator_device_parameters".to_string()
    } else {
        let namespace = super::provider_namespace(&device.device_arn);
        format!("braket.device_schema.{namespace}.{namespace}_device_parameters")
    }
}

/// The document the task runs under: the one the create SUPPLIED, as it was sent, or the
/// default document of the device the task named.
///
/// `run` is what the task's program produced, which is where the qubit count of the default
/// document comes from; a task whose program never ran has no program to state parameters
/// for, so it carries [`EMPTY_PARAMETERS`].
#[must_use]
pub fn resolved(
    device: &Device,
    submitted: Option<&str>,
    run: Option<&TaskMeasurements>,
) -> String {
    match (submitted, run) {
        // The caller's own document is reported as it was sent: it is what the task ran
        // under, so nothing here re-renders or completes it.
        (Some(document), _) => document.to_string(),
        (None, Some(run)) => default_document(device, run.qubit_count),
        (None, None) => EMPTY_PARAMETERS.to_string(),
    }
}

/// `device`'s default parameters document for a program of `qubit_count` qubits.
fn default_document(device: &Device, qubit_count: usize) -> String {
    ::serde_json::json!({
        "braketSchemaHeader": {
            "name": parameters_schema(device),
            "version": SCHEMA_VERSION,
        },
        "paradigmParameters": {
            "braketSchemaHeader": {
                "name": GATE_MODEL_PARAMETERS_SCHEMA,
                "version": SCHEMA_VERSION,
            },
            "qubitCount": qubit_count,
            "disableQubitRewiring": DISABLE_QUBIT_REWIRING_DEFAULT,
        },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A device of each family, built as the seeded inventory builds one, so the schema
    /// name below is read off the device rather than written out per case.
    fn device(arn: &str, device_type: &str) -> Device {
        Device {
            device_arn: arn.to_string(),
            device_name: "A device".to_string(),
            provider_name: "A provider".to_string(),
            device_type: device_type.to_string(),
            device_status: super::super::DEVICE_ONLINE.to_string(),
            device_status_reason: None,
            device_capabilities: EMPTY_PARAMETERS.to_string(),
            shots_range: (1, 100),
            cost_price: 0.0,
            cost_unit: super::super::PRICE_UNIT_SHOT.to_string(),
        }
    }

    fn run_of(qubit_count: usize) -> TaskMeasurements {
        TaskMeasurements {
            qubit_count,
            measured_qubits: (0..qubit_count).collect(),
            rows: vec![vec![0; qubit_count]],
            duration: Duration::from_millis(1),
        }
    }

    /// RULE: the default document names the device's own parameters schema and states the
    /// program's qubit count. The rows span what the document generalises over: the device
    /// family that decides the schema name, and the program that decides the qubit count.
    #[test]
    fn the_default_document_states_the_device_and_the_program() {
        let rows: [(&str, &str, &str, usize, &str); 3] = [
            (
                "a simulator",
                "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
                super::super::DEVICE_TYPE_SIMULATOR,
                2,
                "braket.device_schema.simulators.gate_model_simulator_device_parameters",
            ),
            (
                "a QPU",
                "arn:aws:braket:eu-west-2:::device/qpu/oqc/Lucy",
                super::super::DEVICE_TYPE_QPU,
                3,
                "braket.device_schema.oqc.oqc_device_parameters",
            ),
            (
                "a QPU whose provider segment carries a separator",
                "arn:aws:braket:::device/qpu/d-wave/Advantage_system4",
                super::super::DEVICE_TYPE_QPU,
                5,
                "braket.device_schema.dwave.dwave_device_parameters",
            ),
        ];
        for (case, arn, device_type, qubits, schema) in rows {
            let device = device(arn, device_type);
            let rendered = resolved(&device, None, Some(&run_of(qubits)));
            let document: ::serde_json::Value = ::serde_json::from_str(&rendered)
                .unwrap_or_else(|e| panic!("case {case:?}: {rendered} is not JSON: {e}"));
            assert_eq!(
                document["braketSchemaHeader"]["name"].as_str(),
                Some(schema),
                "case {case:?}: expected the schema {schema}, got {document}"
            );
            assert_eq!(
                document["paradigmParameters"]["qubitCount"].as_u64(),
                Some(qubits as u64),
                "case {case:?}: expected qubitCount {qubits}, got {document}"
            );
            assert_eq!(
                document["paradigmParameters"]["disableQubitRewiring"].as_bool(),
                Some(false),
                "case {case:?}: expected rewiring to stay enabled, got {document}"
            );
        }
    }

    /// RULE: a document the create supplied is what the task runs under, reported as it
    /// was sent; a task whose program produced no run carries the empty document.
    #[test]
    fn a_supplied_document_is_reported_as_it_was_sent() {
        let device = device(
            "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
            super::super::DEVICE_TYPE_SIMULATOR,
        );
        let rows: [(&str, Option<&str>, Option<TaskMeasurements>, &str); 3] = [
            (
                "a document with the service's own spacing",
                Some("{\"braketSchemaHeader\": {\"name\": \"x\", \"version\": \"1\"}}"),
                Some(run_of(2)),
                "{\"braketSchemaHeader\": {\"name\": \"x\", \"version\": \"1\"}}",
            ),
            (
                "an empty document the caller sent",
                Some(EMPTY_PARAMETERS),
                Some(run_of(2)),
                EMPTY_PARAMETERS,
            ),
            ("no document and no run", None, None, EMPTY_PARAMETERS),
        ];
        for (case, submitted, run, expected) in rows {
            let rendered = resolved(&device, submitted, run.as_ref());
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }
}
