Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.delete-broker-removes-it mq.reboot-broker-wrong-state-rejected mq.reboot-broker-applies-pending-user-change mq.promote-non-replica-broker-rejected mq.create-user-weak-password-rejected mq.native-protocol-stomp-roundtrip-activemq
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.delete-broker-removes-it|run#15|AssertionError|assert reboot_code == "NotFoundException", (
Issue-Key: mq.reboot-broker-wrong-state-rejected|run#12|AssertionError|assert code == "BadRequestException", code
Issue-Key: mq.reboot-broker-applies-pending-user-change|run#13|KeyError|assert described_before["Pending"]["PendingChange"] == "CREATE", described_before
Issue-Key: mq.promote-non-replica-broker-rejected|run#11|AssertionError|assert code == "BadRequestException", code
Issue-Key: mq.create-user-weak-password-rejected|run#14|AssertionError|assert message == "Password must contain at least [4] unique characters.", message
Issue-Key: mq.native-protocol-stomp-roundtrip-activemq|run#19|AssertionError|assert instances, "RUNNING broker has no BrokerInstances/endpoints"
Issue-Commit: d6d19e0455e24019f55b32e964a92214bc92b90a

# mq broker lifecycle: RebootBroker and Promote are unimplemented, user changes are never pending, DescribeBroker serves no BrokerInstances, and the weak-password refusal is worded wrong (5 root causes)

> _Note: this issue was identified against the code base at commit `d6d19e0455e24019f55b32e964a92214bc92b90a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootBroker

## Description
FIVE net-new root causes in the mq broker/user lifecycle, all in the same service and all grounded in failing behavioral checks. Model references are to services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json. None of these is covered by an existing open issue: 20261103T045546Z-mq-refuses-invalid-requests-with-the-wrong-status.md covers CreateBroker/ListBrokers/UpdateConfiguration/DescribeConfigurationRevision validation and the CreateBroker users[0].password refusal (a DIFFERENT message class and a different operation than Cause 4 below), 20261103T045546Z-mq-response-bodies-carry-wrong-revision-state-and.md covers configuration revisions and instance-option storageType, and 20261030T033328Z / 20261031T013311Z / 20261030T154619Z cover instance options, revision reads and tagging content-length. All five causes must be closed before this issue is submitted.

Cause 1 of 5: RebootBroker is an unimplemented scaffold stub, so every reboot — accepted or refused — answers 501 NotImplemented.
AWS: Smithy models RebootBroker as POST /v1/brokers/{BrokerId}/reboot, input BrokerId (httpLabel, required), empty output, success 200, modelled errors BadRequestException (400), ForbiddenException (403), NotFoundException (404), InternalServerErrorException (500); doc: "Reboots a broker. Note: This API is asynchronous." Three distinct AWS behaviors are exercised: (a) on a broker id that no longer exists, 404 NotFoundException with fixed text "Cannot find broker ID [" + <the BrokerId from the request> + "]. Verify that your broker exists or check broker permissions." (only the bracketed id is a request value); (b) while the broker is still CREATION_IN_PROGRESS, 400 BadRequestException with fixed text "Broker [" + <the broker's name> + "] can't be restarted while in state [" + <the broker's current BrokerState> + "]" (name and state are values from the request/state, e.g. CREATION_IN_PROGRESS); (c) on a RUNNING broker, 200 with an empty body, the broker transitions through REBOOT_IN_PROGRESS back to RUNNING, and pending user changes are applied by that reboot.
Emulator: services/service-com-amazonaws-mq-mq/src/ops/reboot_broker.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and its handle() returns Scaffolded::not_implemented("RebootBroker"), i.e. HTTP 501 with error code NotImplemented and message "operation RebootBroker is scaffolded but not implemented in this emulator", for every one of those three cases. Replay: mq.delete-broker-removes-it got code NotImplemented where NotFoundException was required; mq.reboot-broker-wrong-state-rejected got NotImplemented where BadRequestException was required; mq.reboot-broker-applies-pending-user-change cannot reach its reboot step at all.
Why: no client can restart a broker against the emulator, and the three error classes a client switches on (404 gone, 400 wrong state, 200 accepted) all collapse into one non-AWS 501 that no SDK models for this operation — retry/wait loops keyed on NotFoundException or BadRequestException never terminate.
Fix: the RebootBroker handler (services/service-com-amazonaws-mq-mq/src/ops/reboot_broker.rs) plus the broker-state transition in crate::state; delete the SCAFFOLDED marker once implemented.
Traces: mq.delete-broker-removes-it, mq.reboot-broker-wrong-state-rejected, mq.reboot-broker-applies-pending-user-change.

Cause 2 of 5: Promote is an unimplemented scaffold stub, so a promote on a broker that is not a cross-Region replica answers 501 NotImplemented instead of being refused.
AWS: Smithy models Promote as POST /v1/brokers/{BrokerId}/promote, input BrokerId (httpLabel, required) and Mode (jsonName "mode", required, enum PromoteMode = SWITCHOVER|FAILOVER), output BrokerId, modelled errors BadRequestException (400), ForbiddenException (403), NotFoundException (404), InternalServerErrorException (500). Recorded behavior: promoting a broker with no data-replication configuration is refused with 400 BadRequestException and the wholly fixed message "This broker is not configured for cross Region data replication." (no request values appear in it), and the broker is left in RUNNING — the call has no effect.
Emulator: services/service-com-amazonaws-mq-mq/src/ops/promote.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and answers 501 NotImplemented ("operation Promote is scaffolded but not implemented in this emulator"). Replay: expected error code BadRequestException, got NotImplemented.
Why: a client cannot tell "this broker is not a replica" (a permanent, actionable 400) from "the service is broken" (501), and a DR runbook that probes promote-ability gets an unmodelled status.
Fix: the Promote handler (services/service-com-amazonaws-mq-mq/src/ops/promote.rs) — gate on the broker's DataReplicationMode/DataReplicationMetadata and emit the modelled BadRequestException with the fixed text above; delete the SCAFFOLDED marker once implemented.
Traces: mq.promote-non-replica-broker-rejected.

Cause 3 of 5: CreateUser applies a new user immediately, so DescribeUser never reports a pending change — AWS holds it as pending until the next reboot.
AWS: Smithy DescribeUser (GET /v1/brokers/{BrokerId}/users/{Username}) has output member Pending (jsonName "pending") targeting UserPendingChanges, whose required member PendingChange targets enum ChangeType = CREATE|UPDATE|DELETE and is documented "Required. The type of change pending for the ActiveMQ user."; DescribeBroker's UserSummary carries the same PendingChange. Amazon MQ's user changes are asynchronous: right after CreateUser, DescribeUser returns Pending.PendingChange = "CREATE", and only after RebootBroker completes does Pending disappear from the response.
Emulator: CreateUser (services/service-com-amazonaws-mq-mq/src/ops/create_user.rs) inserts state::BrokerUser directly into broker.users with no pending-change field, and DescribeBroker emits UserSummary { pending_change: None } unconditionally (describe_broker.rs around the users mapping), so DescribeUser's response has no "pending" member at all. Replay: KeyError 'Pending' on the DescribeUser response immediately after CreateUser.
Why: a client that waits for Pending to clear before using new credentials sees no pending state at all and so cannot distinguish "change staged" from "change applied"; conversely credentials work on the emulator before the reboot that AWS requires, so code written against the emulator skips the reboot and the user never exists on AWS.
Fix: the broker-user state (crate::state BrokerUser) plus the CreateUser/DeleteUser/UpdateUser handlers and the DescribeUser / DescribeBroker serializers — record a pending ChangeType per user, report it as Pending/PendingChange, and clear it when RebootBroker (Cause 1) completes.
Traces: mq.reboot-broker-applies-pending-user-change.

Cause 4 of 5: CreateUser's weak-password refusal carries the emulator's own wording instead of Amazon MQ's.
AWS: Smithy CreateUser models Password (jsonName "password", required) and User.Password is documented "must be at least 12 characters long, must contain at least 4 unique characters, and must not contain commas, colons, or equal signs (,:=)"; errors include BadRequestException (400). Recorded behavior for the unique-character rule: 400 BadRequestException with the message "Password must contain at least [4] unique characters." — that text is entirely FIXED (the bracketed 4 is the service's rule, not a request value), and it does NOT quote the submitted password or how many unique characters it had. The user is not created.
Emulator: right status and right error code (BadRequestException) but message "password must contain at least 4 unique characters, got 1" — different wording, no bracketed limit, and it leaks a property of the submitted secret (its unique-character count). Replay: assertion on the message text failed with that string.
Why: any client, console or test that surfaces or matches the service message gets text no AWS SDK would ever produce, and the message discloses information about the rejected password that AWS deliberately withholds.
Fix: the shared password validation/error serializer this handler uses (state::validate_password, consumed by services/service-com-amazonaws-mq-mq/src/ops/create_user.rs around line 169) — emit Amazon MQ's fixed text per rule rather than a generated description of the input.
Traces: mq.create-user-weak-password-rejected.

Cause 5 of 5: DescribeBroker serves no BrokerInstances for a RUNNING broker, so the broker publishes no wire endpoints or console URL.
AWS: Smithy DescribeBroker output includes BrokerInstances (jsonName "brokerInstances", list of BrokerInstance), and BrokerInstance carries Endpoints ("The broker's wire-level protocol endpoints"), ConsoleURL ("The brokers web console URL") and IpAddress ("Does not apply to RabbitMQ brokers"). For a SINGLE_INSTANCE ACTIVEMQ broker in state RUNNING, AWS returns exactly one BrokerInstance whose Endpoints list the broker's protocol endpoints (one URI per supported wire protocol, e.g. the ssl/amqp+ssl/stomp+ssl/mqtt+ssl/wss forms) and whose ConsoleURL addresses the web console; hostnames, ports and ids are environment values, the presence of the member and of one endpoint per served protocol is the requirement.
Emulator: DescribeBroker's handler (services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs, the DescribeBrokerOutput construction ending in `..Default::default()`) never populates broker_instances, so the 200 response omits the member entirely even once the broker reports RUNNING. Replay: "RUNNING broker has no BrokerInstances/endpoints". The same check then needs that STOMP+SSL endpoint to accept a CONNECT/SEND/SUBSCRIBE round-trip, which it cannot do while no endpoint is advertised.
Why: a client cannot discover where to connect — every Amazon MQ data-plane consumer (JMS/STOMP/AMQP client, Lambda event source mapping, console link) reads BrokerInstances[].Endpoints, and an absent member reads as "a RUNNING broker with no addresses", which AWS never returns.
Fix: the broker state plus DescribeBroker's (and ListBrokers'/CreateBroker's) response construction — derive and store the instance endpoints/console URL when the broker reaches RUNNING and serve them in BrokerInstances.
Traces: mq.native-protocol-stomp-roundtrip-activemq.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): 12/23 passed; service mq 12/21 (57%), 9 failed. Three mq failures are excluded here as already covered: mq.delete-user-last-admin-accepted and mq.describe-broker-engine-types-lists-both-engines are marked ALREADY FILED (DeleteUser / DescribeBrokerEngineTypes stubs), and mq.create-configuration-duplicate-name-conflict-rejected is OWNED by 20261030T170907Z-mq-configuration-operations-createconfiguration-wrongly-refuses-a-duplicate.md.

Cause 1 — RebootBroker 501:
  mq.delete-broker-removes-it: after DeleteBroker and DescribeBroker converging on NotFoundException, RebootBroker on the dead id was expected to answer 404 NotFoundException with "Cannot find broker ID [<broker-id>]. Verify that your broker exists or check broker permissions."; replay: "expected NotFoundException rebooting deleted broker b-<uuid>, got NotImplemented".
  mq.reboot-broker-wrong-state-rejected: RebootBroker while BrokerState == CREATION_IN_PROGRESS was expected to answer 400 BadRequestException with "Broker [<broker-name>] can't be restarted while in state [CREATION_IN_PROGRESS]"; replay: AssertionError on `assert code == "BadRequestException"` with actual code NotImplemented.
  mq.reboot-broker-applies-pending-user-change: never reaches reboot_broker (fails earlier, see Cause 3).
  Source: services/service-com-amazonaws-mq-mq/src/ops/reboot_broker.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED") and line 152 (`Err(crate::scaffolded::Scaffolded::not_implemented("RebootBroker"))`).

Cause 2 — Promote 501:
  mq.promote-non-replica-broker-rejected: Promote on a RUNNING non-replica broker expected 400 BadRequestException / "This broker is not configured for cross Region data replication."; replay: AssertionError on `assert code == "BadRequestException"` with actual NotImplemented.
  Source: services/service-com-amazonaws-mq-mq/src/ops/promote.rs line 4 ("SCAFFOLDED, NOT IMPLEMENTED").

Cause 3 — no pending user change:
  mq.reboot-broker-applies-pending-user-change: `assert described_before["Pending"]["PendingChange"] == "CREATE"` raised KeyError: 'Pending' on the DescribeUser response taken immediately after a successful CreateUser.
  Source: services/service-com-amazonaws-mq-mq/src/ops/create_user.rs lines 212-221 (user inserted with no pending state) and services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 297-306 (`pending_change: None`).

Cause 4 — weak-password message:
  mq.create-user-weak-password-rejected: CreateUser with Password "aaaaaaaaaaaa" (12 chars, 1 unique char). Expected 400 BadRequestException, message "Password must contain at least [4] unique characters." (fixed text; nothing in it comes from the request). Actual code BadRequestException (correct), actual message "password must contain at least 4 unique characters, got 1".
  Source: state::validate_password, called from services/service-com-amazonaws-mq-mq/src/ops/create_user.rs line 169.

Cause 5 — no BrokerInstances:
  mq.native-protocol-stomp-roundtrip-activemq: after DescribeBroker reported BrokerState RUNNING, `assert instances, "RUNNING broker has no BrokerInstances/endpoints"` failed — the 200 response carries no brokerInstances member, so the check had no STOMP+SSL endpoint to dial.
  Source: services/service-com-amazonaws-mq-mq/src/ops/describe_broker.rs lines 232-307 — broker_instances is never set and falls to `..Default::default()`.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from that checkout, and replay the behavioral suite (simparity-behavioral-aws-v1), service mq.
2. Cause 1a (mq.delete-broker-removes-it): CreateBroker (ACTIVEMQ, SINGLE_INSTANCE, mq.t3.micro), wait for RUNNING, DeleteBroker, wait until DescribeBroker answers 404 NotFoundException, then POST /v1/brokers/{deleted-id}/reboot. Expect 404 NotFoundException "Cannot find broker ID [<id>]. Verify that your broker exists or check broker permissions."; the emulator answers 501 NotImplemented.
3. Cause 1b (mq.reboot-broker-wrong-state-rejected): CreateBroker and, while DescribeBroker still reports CREATION_IN_PROGRESS, POST /v1/brokers/{id}/reboot. Expect 400 BadRequestException "Broker [<name>] can't be restarted while in state [CREATION_IN_PROGRESS]"; the emulator answers 501 NotImplemented. Then wait for RUNNING and reboot again: expect 200 and a return to RUNNING.
4. Cause 2 (mq.promote-non-replica-broker-rejected): on a RUNNING broker created with no data-replication configuration, POST /v1/brokers/{id}/promote with mode FAILOVER. Expect 400 BadRequestException "This broker is not configured for cross Region data replication." and the broker still RUNNING; the emulator answers 501 NotImplemented.
5. Cause 3 (mq.reboot-broker-applies-pending-user-change): on a RUNNING broker, CreateUser with a compliant password, then GET /v1/brokers/{id}/users/{username}. Expect pending.pendingChange == "CREATE"; the emulator's response has no pending member. After RebootBroker completes, expect pending to be absent.
6. Cause 4 (mq.create-user-weak-password-rejected): on a RUNNING broker, CreateUser with password "aaaaaaaaaaaa". Compare the 400's message against the fixed text "Password must contain at least [4] unique characters."; the emulator emits "password must contain at least 4 unique characters, got 1".
7. Cause 5 (mq.native-protocol-stomp-roundtrip-activemq): CreateBroker, wait for RUNNING, GET /v1/brokers/{id} and read brokerInstances. Expect one instance with consoleURL and a non-empty endpoints list including the broker's STOMP+SSL endpoint (then a STOMP CONNECT/SEND/SUBSCRIBE on it round-trips a payload); the emulator omits brokerInstances entirely.
