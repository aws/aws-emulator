Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.e2e-command-execution-flow iot.e2e-command-execution-lifecycle-and-cleanup iot.e2e-command-issue-execute-and-observe-result iot.command-execution-delete-while-active-rejected iot.e2e-provisioning-claim-to-thing-registration iot.shadow-delta-is-pushed-over-mqtt-and-recomputed-against-reported iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end iot.iot-job-document-is-delivered-over-mqtt-and-the-device-reported-completion-is-recorded iot.role-alias-credential-duration-out-of-range-rejected
Issue-Key: iot.e2e-command-execution-flow|_wait_for_message#28|AssertionError|raise AssertionError(
Issue-Key: iot.e2e-command-execution-lifecycle-and-cleanup|run#32|AssertionError|assert arrived_topic == accepted_topic, f"device's UpdateCommandExecution was not accepted (got {arrived_topic!r})"
Issue-Key: iot.e2e-command-issue-execute-and-observe-result|_poll_until#32|AssertionError|raise AssertionError(f"condition never became true within {timeout_s}s (last value: {value!r})")
Issue-Key: iot.command-execution-delete-while-active-rejected|_poll_until#39|AssertionError|raise AssertionError(f"condition never became true within {timeout_s}s (last value: {value!r})")
Issue-Key: iot.e2e-provisioning-claim-to-thing-registration|_mqtt_request#33|AssertionError|raise AssertionError(f"no MQTT response on {response_topics} within {timeout_s}s")
Issue-Key: iot.shadow-delta-is-pushed-over-mqtt-and-recomputed-against-reported|_wait_for_inbox#21|Timeout|time.sleep(0.2)
Issue-Key: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end|run#30|Timeout|time.sleep(0.5)
Issue-Key: iot.iot-job-document-is-delivered-over-mqtt-and-the-device-reported-completion-is-recorded|_wait_for_topic#18|Timeout|time.sleep(0.2)
Issue-Key: iot.role-alias-credential-duration-out-of-range-rejected|run#17|AssertionError|assert high_message == "1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equa
Issue-Commit: 657b97543b8e608c75aab9edda765f25072992b5

# iot: 6 net-new causes — no reserved $aws/* topic is served (commands, fleet provisioning, shadow delta, jobs notify), the Republish rule action delivers nothing, and CreateRoleAlias's range-violation message is not AWS's

> _Note: this issue was identified against the code base at commit `657b97543b8e608c75aab9edda765f25072992b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommandExecution

## Description
The broker at the published iot:Data-ATS endpoint now completes sessions (sibling checks obtain CONNACK rc=0, SUBACK [1] and PUBACK), so the open issues 20261118T161720Z-iot-the-mqtt-broker-at-the-published-iot.md and 20261117T202147Z-iot-no-mqtt-broker-completes-a-session-at.md — both of which attribute these failures to a broker that never answers CONNACK — no longer describe what is served and do not cover the causes below. What is missing is the reserved $aws/* topic namespace and the rule engine's republish delivery. 6 causes.

Cause 1 of 6: the IoT Commands data plane over the reserved $aws/commands/* topics does not exist — an execution never reaches the device and a device-reported status is never accepted.
AWS: StartCommandExecution publishes the command request to the reserved topic $aws/commands/things/<thingName>/executions/<executionId>/request/json for the subscribed device, and a device reports progress by publishing to $aws/commands/things/<thingName>/executions/<executionId>/response/json (acknowledged on the matching .../response/accepted topic). GetCommandExecution then reports the reported status; Smithy: GetCommandExecution.status targets CommandExecutionStatus and DeleteCommandExecution models ConflictException(409)/ValidationException(400) — a non-terminal execution is refused and a terminal one deletes, after which GetCommandExecution answers 404 ResourceNotFoundException with message 'Command execution <executionId> cannot be found.' (fixed text 'Command execution ' + the request's executionId + ' cannot be found.').
Emulator: nothing is published to the request topic and nothing is accepted on the response topic. Replay: 'device never received a command execution request on the reserved MQTT topic'; "device's UpdateCommandExecution was not accepted (got None)"; and two checks poll GetCommandExecution for 30s while it stays pinned at status 'CREATED' (HTTP 200, startedAt set, never IN_PROGRESS/SUCCEEDED) — 'condition never became true within 30s (last value: {... status: CREATED ...})'.
Why: the whole command round trip is unusable. A device that subscribes to the reserved request topic never sees a command, a device that publishes its result gets no acknowledgement and no state change, and the control-plane status stays CREATED forever — so the terminal-state rules that gate DeleteCommandExecution (409 ConflictException while non-terminal, success once terminal) can never be reached.
Fix: the reserved-topic handling in the iot data-plane/broker component together with the command-execution state machine behind StartCommandExecution / UpdateCommandExecution / GetCommandExecution / DeleteCommandExecution — publish the request on execution start, accept and apply device reports from the response topic, and drive the status transitions.
Traces: iot.e2e-command-execution-flow, iot.e2e-command-execution-lifecycle-and-cleanup, iot.e2e-command-issue-execute-and-observe-result, iot.command-execution-delete-while-active-rejected.

Cause 2 of 6: the reserved fleet-provisioning topics $aws/certificates/create/json/{accepted,rejected} are not served.
AWS: a device connected with a provisioning claim certificate publishes to $aws/certificates/create/json and the service answers on $aws/certificates/create/json/accepted (with the new certificate, its ownership token and the private key) or on .../rejected; the device then registers the thing via $aws/provisioning-templates/<template>/provision/json.
Emulator: no response arrives on either reply topic — 'no MQTT response on [$aws/certificates/create/json/accepted, $aws/certificates/create/json/rejected] within 25s'.
Why: fleet provisioning by claim certificate cannot complete at all: a device bootstrapping its own identity blocks on a reply that never comes, so neither the accepted nor the rejected branch of the flow can be exercised.
Fix: the reserved provisioning topics in the iot data-plane/broker component and the CreateKeysAndCertificate/RegisterThing handling behind them.
Traces: iot.e2e-provisioning-claim-to-thing-registration.

Cause 3 of 6: the Device Shadow service publishes no delta over MQTT and does not recompute it against reported.
AWS: an update to a shadow's desired state publishes the delta on $aws/things/<thingName>/shadow/update/delta (and the accepted/documents topics); when the device reports a value that matches desired, that key drops out of the recomputed delta.
Emulator: the check subscribes to the reserved shadow topics and the replay records 'Check timed out after 60.0s' — no delta (or accepted/documents) message is ever delivered.
Why: the shadow's push model is the whole point of the feature; a device that waits for a delta before acting never acts, and the recomputation that stops a satisfied desired value from being re-delivered cannot be observed.
Fix: the shadow service's MQTT publication path in the iot data-plane component (reserved $aws/things/+/shadow/* topics) and its delta computation against the reported state.
Traces: iot.shadow-delta-is-pushed-over-mqtt-and-recomputed-against-reported.

Cause 4 of 6: a topic rule's Republish action never reaches a native MQTT subscriber.
AWS: a topic rule with a Republish action re-publishes the (SQL-transformed) payload to the configured target topic, where an ordinary MQTT subscriber receives it.
Emulator: the subscriber on the republish target topic receives nothing and the check runs out its budget — 'Check timed out after 60.0s'.
Why: rule-driven topic rewriting and fan-out silently drop every message, so a pipeline whose next hop is an MQTT subscriber on the republish topic sees no traffic.
Fix: the rule engine's Republish action in the iot rules component — it must publish back into the broker so native subscribers match.
Traces: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end.

Cause 5 of 6: a job's document is not delivered to the device over the reserved jobs topics and a device-reported completion is not recorded.
AWS: a job targeting a thing is announced on the reserved $aws/things/<thingName>/jobs/* topics (notify / notify-next, and get/start-next accepted), the device fetches the job document there, and its UpdateJobExecution report (IN_PROGRESS then SUCCEEDED) is recorded on the job execution.
Emulator: the device never receives the job document and the check runs out its budget — 'Check timed out after 60.0s'.
Why: the MQTT-based jobs workflow — the only one a constrained device uses — delivers nothing, so a device never learns it has work and the job execution never advances past its initial state.
Fix: the reserved jobs topics in the iot data-plane/broker component and the job-execution state transitions driven from them.
Traces: iot.iot-job-document-is-delivered-over-mqtt-and-the-device-reported-completion-is-recorded.

Cause 6 of 6: CreateRoleAlias's credentialDurationSeconds range-violation message inserts the offending value, which AWS's message for this member does not.
AWS: Smithy: CreateRoleAlias.credentialDurationSeconds targets CredentialDurationSeconds with @range min 900 max 43200; the modeled refusal is InvalidRequestException (HTTP 400). Recorded behavior: the message is exactly '1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200'. Fixed text: '1 validation error detected: Value at '' + the member name + '' failed to satisfy constraint: Member must have value less than or equal to ' + the modeled maximum; the variable parts are the member name ('credentialDurationSeconds') and the limit (43200, from the model). The rejected value is NOT part of this message.
Emulator: it returns InvalidRequestException with '1 validation error detected: Value '46800' at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200' — the same template with the request's value spliced in after 'Value'.
Why: the status and code are right but the message is not AWS's, so a client (or test) that matches the refusal message, or a human reading it, sees a message real AWS never emits; message-equality assertions on this refusal fail against real AWS or against the emulator depending on which side they were written on.
Fix: the validation/error-message formatting for CreateRoleAlias's credentialDurationSeconds range check in that operation's validation path — emit the valueless 'Value at '<member>'' form for this member's range violation.
Traces: iot.role-alias-credential-duration-out-of-range-rejected.

## Evidence
Cause 1 — expected: the device receives a request on $aws/commands/things/<thing>/executions/<id>/request/json, its report on .../response/json is accepted, and GetCommandExecution reports SUCCEEDED; DeleteCommandExecution refuses 409 ConflictException while non-terminal and, after the terminal delete, GetCommandExecution answers 404 ResourceNotFoundException 'Command execution <executionId> cannot be found.'. Actual: 'device never received a command execution request on the reserved MQTT topic'; "device's UpdateCommandExecution was not accepted (got None)"; GetCommandExecution 200 with status stuck at 'CREATED' for the full 30s poll (last value shows executionId/commandArn/targetArn/executionTimeoutSeconds 60/createdAt/lastUpdatedAt/startedAt/timeToLive, status 'CREATED').
Cause 2 — expected a reply on $aws/certificates/create/json/accepted (or /rejected); actual 'no MQTT response on [$aws/certificates/create/json/accepted, $aws/certificates/create/json/rejected] within 25s'.
Causes 3-5 — expected a shadow delta on $aws/things/<thing>/shadow/update/delta, a republished message on the rule's target topic, and a job document on $aws/things/<thing>/jobs/*; actual the check reaches that wait and the replay reports 'Check timed out after 60.0s' with nothing delivered.
Cause 6 — expected InvalidRequestException with message "1 validation error detected: Value at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200"; actual InvalidRequestException with "1 validation error detected: Value '46800' at 'credentialDurationSeconds' failed to satisfy constraint: Member must have value less than or equal to 43200" (46800 is the request's value for this run and is not part of AWS's template).

## Steps to Reproduce
1. Build and boot the emulator; DescribeEndpoint for iot:Data-ATS and iot:Jobs endpoints.
2. Commands: CreateThing + CreateKeysAndCertificate + policy, CreateCommand, StartCommandExecution; connect the device over mTLS and subscribe to $aws/commands/things/<thing>/executions/+/request/json — nothing arrives. Publish {"status":"SUCCEEDED"} to $aws/commands/things/<thing>/executions/<id>/response/json and poll GetCommandExecution — status stays CREATED. Call DeleteCommandExecution on the non-terminal execution and observe it is not refused with 409 ConflictException.
3. Fleet provisioning: create a provisioning template and a claim via CreateProvisioningClaim, connect with the claim certificate, publish to $aws/certificates/create/json and subscribe to .../accepted and .../rejected — neither answers.
4. Shadow: subscribe to $aws/things/<thing>/shadow/update/delta, call UpdateThingShadow with a desired state — no delta arrives.
5. Republish: create a topic rule with a Republish action to topic T, subscribe natively to T, publish to the rule's source topic — nothing arrives on T.
6. Jobs: CreateJob targeting the thing, subscribe to $aws/things/<thing>/jobs/notify-next — no job document arrives.
7. Role alias: create an IAM role trusted by credentials.iot.amazonaws.com, then CreateRoleAlias(credentialDurationSeconds=46800) and compare the InvalidRequestException message to AWS's valueless template.
Replay: iot.e2e-command-execution-flow, iot.e2e-command-execution-lifecycle-and-cleanup, iot.e2e-command-issue-execute-and-observe-result, iot.command-execution-delete-while-active-rejected, iot.e2e-provisioning-claim-to-thing-registration, iot.shadow-delta-is-pushed-over-mqtt-and-recomputed-against-reported, iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end, iot.iot-job-document-is-delivered-over-mqtt-and-the-device-reported-completion-is-recorded, iot.role-alias-credential-duration-out-of-range-rejected
