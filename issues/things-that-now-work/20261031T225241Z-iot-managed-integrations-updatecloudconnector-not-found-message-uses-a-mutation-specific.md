Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-cloud-connector-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-cloud-connector-error-paths
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|UpdateCloudConnector|4xx->4xx|$.Message
Issue-Commit: 1184b33254953722da367e2321295173ed6a80b9

# UpdateCloudConnector not-found message uses a mutation-specific sentence AWS never emits

> _Note: this issue was identified against the code base at commit `1184b33254953722da367e2321295173ed6a80b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCloudConnector

## Description
This bundle has 1 cause; it must be fixed before the issue is closed.

Cause 1 of 1: UpdateCloudConnector against a CloudConnectorId no cloud connector is held under returns the right status and error class but a message AWS does not word.

AWS: Smithy (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json): UpdateCloudConnector, PUT /cloud-connectors/{Identifier}, success code 200 with an empty (Unit) body; input Identifier is an @required @httpLabel of CloudConnectorId with @length(1..64) and @pattern ^[A-Za-z0-9-_]+$; modelled errors include ResourceNotFoundException (httpError 404) whose members Message/ResourceId/ResourceType are all optional. The model does not state the message text, so: recorded behavior only — for an identifier the account holds no cloud connector for, AWS answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message exactly 'Cloud connector not found.'. That whole sentence is FIXED text for this error class: it names the resource class in sentence case and interpolates NO value from the request (the submitted identifier does not appear). It is the SAME sentence the read path uses — the recording for GetCloudConnector on an unknown identifier carries the identical Message — so AWS does not word a separate mutation-specific variant.

Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException (status and class correct) and Message 'Cloud connector cannot be found.' — a different sentence ('cannot be found.' where AWS writes 'not found.'). The divergence is the fixed text only; no variable part is involved on either side.

Why: an error message is part of the error contract. A client, SDK test or integration suite that matches, logs or displays AWS's wording for a dead cloud-connector reference sees a sentence AWS never emits, so exact-text comparisons fail and message-derived remediation text differs from production. It also makes the emulator inconsistent with itself: the same missing resource is reported with two different sentences depending on whether the caller read it or updated it.

Fix: UpdateCloudConnector's not-found error construction, and the shared mutation-path formatter it calls rather than the handler alone — services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/update_cloud_connector.rs:187-190 raises ResourceNotFoundException via crate::state::cloud_connector_not_found_on_mutation(&input.identifier), described in that call site's comment as 'the one formatter every mutating path of this service shares'. That formatter emits the 'cannot be found.' wording; the read path emits its own. Both paths must render this service's single recorded sentence for an absent cloud connector, so fixing the shared formatter (in src/state.rs) is what closes it for every mutating cloud-connector operation, not just this one.

Related but NOT the same filing: open issue 20261029T171839Z-iot-managed-integrations-error-messages-for-not-found-and-invalid-reference.md covers GetCloudConnector's own divergence ('Cloud Connector resource not found.', its Cause 2 of 6) and carries that read step's replay key; it says nothing about UpdateCloudConnector or the mutation formatter, and the replay reports this step as owned by no issue. Closing that issue would leave this step still diverging.

Traces: iot-managed-integrations-cloud-connector-error-paths#task#3 (the representative and only member of this cluster).

## Evidence
Replay of corpus simparity-wire-aws-v1 (trace_set_id simparity-wire-aws-v1), fixture iot-managed-integrations-cloud-connector-error-paths, step task#3, operation UpdateCloudConnector, verdict mismatch, affected_count 1, stub_path empty, owned_by empty.

Expected status 404, actual status 404 (status matches; x-amzn-errortype ResourceNotFoundException matches).
Detail: $.Message: value_mismatch (expected 'Cloud connector not found.', got 'Cloud connector cannot be found.').

Expected body Message: 'Cloud connector not found.'   — fixed text, no interpolated request value.
Actual   body Message: 'Cloud connector cannot be found.'

Corroborating recording in the same fixture: step task#2 (GetCloudConnector on the same unknown identifier) expects the identical Message 'Cloud connector not found.', so the recorded sentence is this service's single wording for an absent cloud connector rather than a read-only variant.

Code path: src/ops/update_cloud_connector.rs lines 173-190 — the handler resolves store.cloud_connector_in_mut(&region, &input.identifier) and, on None, builds the 404 through crate::state::cloud_connector_not_found_on_mutation(&input.identifier) (the shared mutating-path formatter), which is the source of the 'cannot be found.' wording.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an iot-managed-integrations client at its data listener (region eu-west-1, matching the fixture).
2. Replay iot-managed-integrations-cloud-connector-error-paths#task#3: send PUT /cloud-connectors/{Identifier} with an Identifier no cloud connector exists for in that region (the fixture uses an identifier that was never created), with a body naming Name and/or Description.
3. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException and body Message 'Cloud connector cannot be found.'.
4. Compare with the recorded AWS response for that step: HTTP 404, ResourceNotFoundException, Message exactly 'Cloud connector not found.' — fixed text with no request value interpolated.
5. For contrast, replay step task#2 of the same fixture (GET /cloud-connectors/{same unknown Identifier}): it must carry that same recorded sentence, confirming the read and mutate paths owe one wording.
