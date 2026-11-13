Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# secretIdentifier and applianceFqdn are never resolved, so every connector reaches ACTIVE and CREATE_FAILED is unreachable

_Filed: 2026-10-24T15:31:13Z_
_Operation: CreateEnvironmentConnector | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A connector exists to let "the Amazon EVS control plane ... interface with VCF appliances using a fully qualified domain name" with "the credentials stored in Amazon Web Services Secrets Manager" (CreateEnvironmentConnector / Connector docs), and the secret the request names must both exist and carry the type-specific fields: "SDDC_MANAGER requires an apiKey field; OPERATIONS_MANAGER and VCENTER require username and password fields" (CreateEnvironmentConnectorRequest$secretIdentifier doc).

Expected: a create naming a `secretIdentifier` the account holds no Secrets Manager secret for (or one missing the fields the chosen `type` requires) cannot establish a connection — real EVS either refuses it up front with the operation's ValidationException naming `secretIdentifier`, or the connector settles into the modeled `ConnectorState` CREATE_FAILED with `stateDetails` explaining why. Either way an unresolvable secret must not produce a healthy ACTIVE connector.

Actual: the handler stores the supplied `secretIdentifier` verbatim (`StoredConnector::creating`, src/state.rs: "The Secrets Manager secret holding the appliance credentials, EXACTLY as the request supplied it"), never reads any Secrets Manager state, and unconditionally schedules `PendingConnectorState::starting_now(CONNECTOR_ACTIVE)`. So `arn:aws:secretsmanager:us-west-2:000000000000:secret:does-not-exist` — or the bare name `nope` — yields HTTP 200 and a connector that settles to ACTIVE, and the CREATE_FAILED / UPDATE_FAILED states the model declares are unreachable for every input. `applianceFqdn` is likewise accepted for any syntactically valid hostname that resolves to nothing. `UpdateEnvironmentConnector` reconfigures the same two members with the same absence of resolution (src/ops/update_environment_connector.rs lines 270-281), so a connector can be pointed at a non-existent secret after the fact too.

This is implementable the way the crate already resolves EC2 facts: `src/state/ec2_resources.rs` cross-reads `service_com_amazonaws_ec2_amazonec2::state`, and the repo carries `services/service-com-amazonaws-secretsmanager-secretsmanager` for the secret side.

## Evidence

Spec: CreateEnvironmentConnector doc ("A connector allows the Amazon EVS control plane to interface with VCF appliances...", "Amazon EVS requires an active connector to SDDC Manager or VCF Operations Manager to monitor environment health"), CreateEnvironmentConnectorRequest$secretIdentifier doc (secret stores the credentials; per-type required fields), `ConnectorState` enum which declares CREATE_FAILED and UPDATE_FAILED (smithy_query shape=ConnectorState). Handler: src/ops/create_environment_connector.rs lines 196-314 — the only preconditions evaluated are environment existence/Region/not-DELETED and the one-connector-per-type rule; lines 278-288 mint the record with an unconditional CREATING->ACTIVE pending transition. src/state.rs `StoredConnector::creating` (lines ~505-531) copies `secret_identifier` into `secret_arn` and sets `checks: Vec::new()`. src/ops/update_environment_connector.rs lines 241-295 writes the new `secretIdentifier`/`applianceFqdn` with no lookup. Tests: tests/all/connectors.rs uses the fixed constant SECRET_ARN (line 58) and a rotated ARN (line 436) without registering any secret anywhere, and asserts 200 — no test asserts a refusal or a failed connector for an unresolvable secret, so nothing covers this rule. Precedent in this backlog: the equivalent gap for hosts is filed as "Host placement references and esxVersion are accepted without being resolved against EC2 or the version catalog" (CreateEnvironmentHost/validation).

## Steps to Reproduce

1. Register a VPC and service access subnet in the shared EC2 store, CreateEnvironment (termsAccepted=true, ten non-overlapping initialVlans) and wait for the environment to reach CREATED.
2. Call CreateEnvironmentConnector with type=VCENTER, applianceFqdn=vcenter.example.com and secretIdentifier=arn:aws:secretsmanager:us-west-2:000000000000:secret:does-not-exist-AbCd12 (no such secret exists in the account's Secrets Manager store).
3. Observe HTTP 200 and a connector in CREATING, instead of a ValidationException naming secretIdentifier.
4. Wait for the pending transition and call ListEnvironmentConnectors: the connector reports state ACTIVE, i.e. a connection was established to an appliance whose credentials secret does not exist, and no CREATE_FAILED state is ever produced.
5. Repeat with a secret that exists but carries no username/password while type=VCENTER (per-type field requirement), and with UpdateEnvironmentConnector setting secretIdentifier to a non-existent secret — both are accepted.
