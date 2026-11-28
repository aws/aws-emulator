Issue-Source: trace-sweep-divergence
Issue-Trace: braket-search-and-inspect-devices
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-search-and-inspect-devices
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|GetDevice|2xx->2xx|$.deviceCapabilities
Issue-Commit: e65e3cf1e802bab7496756e57d57cc6ca3c0fc00

# Braket GetDevice publishes a device-capability document that diverges from the recorded catalog

> _Note: this issue was identified against the code base at commit `e65e3cf1e802bab7496756e57d57cc6ca3c0fc00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDevice

## Description
Cause 1 of 1: `GetDevice` answers 200 with a `deviceCapabilities` JSON document whose contents do not match the document AWS publishes for the device, so the action/pragma/operation catalog a client reads is wrong even though the envelope and the status are right.

AWS: Smithy `GetDevice` (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json) models `GetDeviceResponse.deviceCapabilities` as a **required** `JsonValue` with `@mediaType("application/json")`; its documentation says only "Details about the capabilities of the device", so the document's CONTENT is recorded behavior only. The recorded 200 response for the Amazon Braket state-vector simulator (`arn:aws:braket:::device/quantum-simulator/amazon/sv1`, `deviceDocumentation.summary` = "Amazon Braket state vector simulator") carries a Braket device-capability document in which:
  * `action["braket.ir.openqasm.program"].supportedOperations` is the OpenQASM gate set `barrier, ccnot, cnot, cphaseshift, cphaseshift00, cphaseshift01, cphaseshift10, cswap, cy, cz, ecr, h, i, iswap, pswap, phaseshift, rx, ry, rz, s, si, swap, t, ti, v, vi, x, xx, xy, y, yy, z, zz, gpi, gpi2, ms` — it does NOT contain `cv`, `gphase`, `measure`, `prx`, `reset`, `u` or `unitary`.
  * `supportedPragmas` contains `braket_unitary_matrix` and `braket_basis_rotation` together with `braket_result_type_{sample,expectation,variance,probability,amplitude,adjoint_gradient}`, while `braket_result_type_state_vector`, `braket_result_type_density_matrix` and the `braket_noise_*` pragmas are in `forbiddenPragmas`. Neither list carries a bare `verbatim` entry.
  * `service.deviceDocumentation.externalDocumentationUrl` anchors the simulator section of `braket-devices.html` (`#choose-a-simulator`), `imageUrl` is the published SiteMerch asset for the entry, and `service.updatedAt` is `2022-06-22T10:18:00` — a FIXED property of the published catalog row, not a clock reading, so it is the same for every caller and every run.
  * `maximumQubitArrays`/`maximumClassicalArrays` are 1 and `forbiddenArrayOperations` lists the array operations the schema forbids (these the emulator already matches).

Emulator: the document is generated in `crate::state` (`capabilities_document`, `action_properties`, `pragmas`, `device_properties`) from the `DEVICE_SEEDS` catalog row, and for this device it emits instead:
  * `supportedOperations` = the whole `GATE_MODEL_OPERATIONS` constant, i.e. the recorded set PLUS `cv`, `gphase`, `measure`, `prx`, `reset`, `u`, `unitary`, and with `gpi`/`gpi2`/`ms` in alphabetical position rather than appended after `zz`;
  * `supportedPragmas` containing `braket_result_type_state_vector` (the seed publishes the `StateVector` result type, and the pragma list is derived from the result types), and NO `braket_basis_rotation` at all — that pragma name exists nowhere in the service;
  * `forbiddenPragmas` led by `verbatim`, an entry the recorded document does not carry in either list (the emulator sorts `VERBATIM_PRAGMA` into `forbidden` for every non-verbatim device);
  * `service.deviceDocumentation.imageUrl` = `https://d1.awsstatic.com/braket/devices/amazon-sv1.png`, `externalDocumentationUrl` anchored `#braket-simulator-sv1`, and `service.updatedAt` = `2020-08-13T00:00:00`;
  * the top-level object led by `braketSchemaHeader`, where the recorded document leads with `service`.
Replay reports `$.deviceCapabilities: value_mismatch` on an otherwise matching 200 response (expected status 200, actual 200; no other member diverged).

Why: `deviceCapabilities` is the contract a Braket client (the Braket SDK, or any program builder) deserialises to decide what it may submit — which gates a program may emit, which pragmas it may attach, whether a verbatim box is allowed, and which result types it may request. Advertising operations AWS does not accept (`unitary`, `measure`, `reset`, `u`, `gphase`, `cv`, `prx`) leads a client to build and submit a program the real service refuses; omitting `braket_basis_rotation` and classifying `braket_result_type_state_vector` as supported make the same client reject or mis-plan a program AWS accepts. The documentation links and the published-at instant are what the SDK surfaces to users and what catalog caches diff on, so a differing value shows as a catalog change nobody made.

Fix: the Braket device-inventory STATE, not the `GetDevice` handler — the handler (services/service-com-amazonaws-braket-braket/src/ops/get_device.rs) correctly returns the stored `device.device_capabilities` verbatim. The capability-document builders and the catalog row they read are what must agree with the published document: `capabilities_document`, `action_properties`, `pragmas`, the `GATE_MODEL_OPERATIONS` / `RESULT_TYPE_PRAGMAS` / `VERBATIM_PRAGMA` catalogs and the per-device `DEVICE_SEEDS` fields (`supported_operations`, `result_types`, `image_url`, `documentation_section`, `published_at`) in services/service-com-amazonaws-braket-braket/src/state.rs. Note that `CreateQuantumTask` validates against the same seeded catalog, so the published document and the enforced rule must be changed together.

Not filed here (already owned): the two `content-length` divergences on the 204 answers of `TagResource`/`UntagResource` in `braket-spending-limit-lifecycle` are owned by the open issue `20261119T014646Z-braket-204-no-content-answers-omit-the-content.md`.

## Evidence
Fixture `braket-search-and-inspect-devices`, step `task#2`, operation `GetDevice`, service `braket`.
verdict: mismatch; expected_status 200, actual_status 200.
detail: `$.deviceCapabilities: value_mismatch`.

Expected (recorded) document, as the replay printed it (prefix):
`{"service": {"braketSchemaHeader": {"name": "braket.device_schema.device_service_properties", "version": "1"}, "executionWindows": [{"executionDay": "Everyday", "windowStartHour": "00:00:00", "windowEndHour": "23:59:59"}], "shotsRange": [0, 100000], "deviceCost": {"price": 0.075, "unit": "minute"}, "deviceDocumentation": {"imageUrl": "https://d1.awsstatic.com/about-aws/whats-new/lpm-assets/services/SiteMerch_Qx_Editorial-reInvent.<asset-hash>.png", "summary": "Amazon Braket state vector simulator", "externalDocumentationUrl": "https://docs.aws.amazon.com/braket/latest/developerguide/braket-devices.html#choose-a-simulator"}, "updatedAt": "2022-06-22T10:18:00"}, "action": {"braket.ir.openqasm.program": {"version": ["1.0"], "actionType": "braket.ir.openqasm.program", "supportedOperations": ["barrier", "ccnot", "cnot", "cphaseshift", "cphaseshift00", "cphaseshift01", "cphaseshift10", "cswap", "cy", "cz", "ecr", "h", "i", "iswap", "pswap", "phaseshift", "rx", "ry", "rz", "s", "si", "swap", "t", "ti", "v", "vi", "x", "xx", "xy", "y", "yy", "z", "zz", "gpi", "gpi2", "ms"], "supportedPragmas": ["braket_unitary_matrix", "braket_basis_rotation", "braket_result_type_sample", "braket_result_type_expectation", "braket_result_type_variance", "braket_result_type_probability", "braket_result_type_amplitude", "braket_result_type_adjoint_gradient"], "forbiddenPragmas": ["braket_result_type_state_vector", "braket_result_type_density_matrix", "braket_noise_amplitude_damping", ...], "maximumQubitArrays": 1, "maximumClassicalArrays": 1, "forbiddenArrayOperations": ["concatenation", "negativeIndex", "range", "rangeWithStep", "slicing", ...] ...`

Actual, as the emulator generates it (services/service-com-amazonaws-braket-braket/src/state.rs):
* `GATE_MODEL_OPERATIONS` (state.rs:997-1041) is the `supportedOperations` value for every gate-model device and additionally contains `cv`, `gphase`, `measure`, `prx`, `reset`, `u`, `unitary`.
* `pragmas()` (state.rs:1705-1731) builds the two lists from `VERBATIM_PRAGMA` ("verbatim"), `UNITARY_MATRIX_PRAGMA`, `NOISE_PRAGMAS` and `RESULT_TYPE_PRAGMAS` only: `braket_basis_rotation` is absent from the whole crate, `verbatim` is emitted into `forbiddenPragmas` for this device, and `braket_result_type_state_vector` lands in `supportedPragmas` because the SV1 seed publishes the `StateVector` result type (state.rs:954-962).
* the SV1 catalog row (state.rs:1123-1145) carries `image_url: "https://d1.awsstatic.com/braket/devices/amazon-sv1.png"`, `documentation_section: "braket-simulator-sv1"` (rendered as the `#braket-simulator-sv1` anchor) and `published_at: "2020-08-13T00:00:00"`.
* `capabilities_document()` (state.rs:1453-1507) emits `braketSchemaHeader` as the first top-level key; the recording leads with `service`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus `simparity-wire-aws-v1`.
2. Replay fixture `braket-search-and-inspect-devices`, step `task#2`: `GET /device/arn%3Aaws%3Abraket%3A%3A%3Adevice%2Fquantum-simulator%2Famazon%2Fsv1` (`GetDevice` on the Amazon Braket state-vector simulator).
3. The response is 200, as recorded. Parse the `deviceCapabilities` member (a JSON string) and compare it against the recorded document.
4. Observe `$.deviceCapabilities: value_mismatch`: `action["braket.ir.openqasm.program"].supportedOperations` carries `cv`, `gphase`, `measure`, `prx`, `reset`, `u`, `unitary` which the recorded document does not; `supportedPragmas` omits `braket_basis_rotation` and includes `braket_result_type_state_vector` (recorded: forbidden); `forbiddenPragmas` carries a `verbatim` entry the recording has in neither list; and `service.deviceDocumentation.imageUrl` / `externalDocumentationUrl` / `service.updatedAt` differ from the published catalog values.
