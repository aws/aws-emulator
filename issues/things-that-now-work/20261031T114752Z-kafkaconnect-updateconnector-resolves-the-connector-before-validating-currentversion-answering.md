Issue-Source: trace-sweep-divergence
Issue-Trace: kafkaconnect-connector-not-found-operations
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafkaconnect-connector-not-found-operations
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect|UpdateConnector|4xx->4xx|$.message
Issue-Key: kafkaconnect|UpdateConnector|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafkaconnect|UpdateConnector|4xx->4xx|status
Issue-Commit: bdbe8baa9177a8db6293f043e773851dfd709fdf

# UpdateConnector resolves the connector before validating currentVersion, answering 404 NotFoundException instead of 400 BadRequestException

> _Note: this issue was identified against the code base at commit `bdbe8baa9177a8db6293f043e773851dfd709fdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnector

## Description
One root cause in the kafkaconnect emulator's UpdateConnector handler: the order in which the operation's gates run, which changes the error CLASS (status + x-amzn-errortype) a caller sees.

Cause 1 of 1: UpdateConnector against a well-formed connectorArn this service does not hold is refused as a missing resource instead of as an invalid currentVersion.

AWS: UpdateConnector (Smithy: com.amazonaws.kafkaconnect#UpdateConnector, PUT /v1/connectors/{connectorArn}, input members connectorArn @required @httpLabel and currentVersion @required @httpQuery('currentVersion'); modeled errors BadRequestException HTTP 400 and NotFoundException HTTP 404). The model does not state which gate runs first or any message text, so: recorded behavior only — for a syntactically valid connectorArn of this service that names no connector the service holds, UpdateConnector answers HTTP 400, x-amzn-errortype BadRequestException, with the message "Invalid parameter currentVersion." That is, the currentVersion gate is evaluated BEFORE the connector lookup, and its message uses this service's no-reason parameter form: fixed text "Invalid parameter " + the blamed member name + ".", where the only variable part is the member name taken from the request (currentVersion here). It carries no reason clause, no ARN, no version value and nothing else from the request or state. The corroborating behavioral check kafkaconnect.update-connector-stale-version-rejected asserts the same pair (400 BadRequestException, "Invalid parameter currentVersion.").

Emulator: resolves the connector first and answers HTTP 404, x-amzn-errortype NotFoundException, with the message "Invalid parameter connectorArn: The requested resource doesn’t exist." (the shared invalid_parameter_because(member, RESOURCE_DOES_NOT_EXIST) form). The currentVersion comparison is only reached once a connector record has been found, and when it is reached it renders the reason-carrying form "Invalid parameter currentVersion: The specified version is not the current version of the resource." rather than AWS's bare "Invalid parameter currentVersion."

Why: a client branches on the error class, not the sentence. Against AWS this call is a 400 client-input error that must not be retried and must be surfaced as a stale/incorrect currentVersion; against the emulator it is a 404 absent-resource error, which SDK retry/waiter logic and typed catch blocks (except NotFoundException vs except BadRequestException) treat as a different outcome entirely — a reconciliation loop that recreates the connector on NotFoundException will recreate it when AWS would have told it to re-read the version, and a test suite asserting the modeled 400 path fails against the emulator. The message form divergence additionally breaks text-matching clients, since BadRequestException is the only channel kafkaconnect gives every bad input for this operation.

Fix: the UpdateConnector handler's gate ordering and its version-refusal message — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs, where the state.connector_in(&region, &input.connector_arn) lookup returning NotFoundException (around line 243) precedes the input.current_version != connector.current_version check (around line 257). The currentVersion refusal must be decided before the connector is resolved, and must render crate::state::invalid_parameter("currentVersion") (the no-reason form, src/state.rs around line 2193) rather than invalid_parameter_because("currentVersion", VERSION_IS_NOT_CURRENT) (src/state.rs around line 2154/2199).

Traces: fixture kafkaconnect-connector-not-found-operations, step task#3 (UpdateConnector). No other cluster member and no other fixture exercises this path.

Neighbouring issues, neither of which covers this: the open issue 20261029T194141Z-kafkaconnect-connector-lifecycle-never-reaches-failed-no-statedescription.md records that its behavioral check kafkaconnect.update-connector-stale-version-rejected never REACHED its stale-currentVersion assertions (it is blocked by the connector lifecycle never resting in FAILED), so it establishes nothing about this ordering or this message; the open issue 20261027T142522Z-kafkaconnect-createcustomplugin-validation.md is about CreateCustomPlugin's bucketArn validation.

## Evidence
Wire replay of simparity-wire-aws-v1 (6 fixtures, 35 steps, 34 passed, 1 mismatched, score 97.1%).

[mismatch] fixture kafkaconnect-connector-not-found-operations, step task#3, operation UpdateConnector, service kafkaconnect
  divergence_key: service=kafkaconnect|UpdateConnector|status=400->404|keys=$.message,$headers.x-amzn-errortype,status
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'NotFoundException'
  $.message: expected 'Invalid parameter currentVersion.', got 'Invalid parameter connectorArn: The requested resource doesn’t exist.'
  stub_path: (none)  owned_by: (none)  affected_count: 1

Expected (AWS, recorded): HTTP 400, x-amzn-errortype: BadRequestException, body {"message": "Invalid parameter currentVersion."}
Actual (emulator):        HTTP 404, x-amzn-errortype: NotFoundException, body {"message": "Invalid parameter connectorArn: The requested resource doesn’t exist."}

Source corroboration (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/):
  ops/update_connector.rs ~243  let Some(connector) = state.connector_in(&region, &input.connector_arn) else { ... NotFoundException(invalid_parameter_because("connectorArn", RESOURCE_DOES_NOT_EXIST)) } — runs before any currentVersion check.
  ops/update_connector.rs ~257  if input.current_version != connector.current_version { ... BadRequestException(invalid_parameter_because("currentVersion", VERSION_IS_NOT_CURRENT)) } — unreachable for an ARN the service does not hold, and renders the reason-carrying form.
  state.rs ~2154  VERSION_IS_NOT_CURRENT = "The specified version is not the current version of the resource."
  state.rs ~2193  invalid_parameter(member) -> "Invalid parameter <member>." — the form AWS uses here, currently unused on this path.

## Steps to Reproduce
Boot the emulator and point a kafkaconnect client at it (region us-east-1).

1. Without creating any connector, call UpdateConnector with:
     connectorArn = a syntactically valid kafkaconnect connector ARN of this service that no connector in the store uses, e.g. arn:aws:kafkaconnect:us-east-1:<account>:connector/<name>/<uuid>
     currentVersion = any version string
     capacity = exactly one valid form (e.g. {"provisionedCapacity": {"mcuCount": 1, "workerCount": 1}})
   Expected: HTTP 400, x-amzn-errortype BadRequestException, message "Invalid parameter currentVersion."
   Actual:   HTTP 404, x-amzn-errortype NotFoundException, message "Invalid parameter connectorArn: The requested resource doesn’t exist."

2. Replay the recorded unit directly: fixture kafkaconnect-connector-not-found-operations, step task#3.

3. (Message form, same cause) Create a connector, poll it out of CREATING, then call UpdateConnector on it with a currentVersion that is not the connector's.
   Expected: HTTP 400 BadRequestException, message "Invalid parameter currentVersion."
   Actual:   HTTP 400 BadRequestException, message "Invalid parameter currentVersion: The specified version is not the current version of the resource."
