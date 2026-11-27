Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.create-configuration-duplicate-name-conflict-rejected mq.update-configuration-revision-concurrency
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|CreateConfiguration|ConflictException
Issue-Key: mq.update-configuration-revision-concurrency|run#10|AssertionError|assert len(successes) >= 1, "expected at least one racer to succeed"
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# mq configuration operations: CreateConfiguration wrongly refuses a duplicate Name, and UpdateConfiguration never serves a revision (2 root causes)

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguration

## Description
Two distinct root causes in the mq configuration surface, both grounded in failing behavioral checks. A fixer must close BOTH before submitting.

Cause 1 of 2: CreateConfiguration refuses a second configuration that reuses an existing Name with ConflictException, but Amazon MQ allows it.
Failing check: mq.create-configuration-duplicate-name-conflict-rejected
AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json): CreateConfiguration POST /v1/configurations, output member `Id` (jsonName "id") is documented "Required. The unique ID that Amazon MQ generates for the configuration"; `Name` carries only a value rule (alphanumerics, dashes, periods, underscores, tildes; 1-150 chars). The model declares NO uniqueness constraint on `Name`, and nothing in the modelled ConflictException doc ties it to a reused name. Recorded behavior (the check documents this as validated against real AWS): configuration identity is the SERVER-GENERATED `Id`, not `Name`; two CreateConfiguration calls with the same `Name` both return HTTP 200 with distinct `Id` values, and ListConfigurations then reports two entries sharing that `Name`.
Emulator: the second create is refused with HTTP 409 `ConflictException`, message `a configuration named "<name>" already exists in this account as "<existing configuration id>"` and `errorAttribute: "name"`. In the replay the message read: a configuration named "sim2real-mqcfg-4ef79ffe681c" already exists in this account as "c-7d29e022-06dd-3887-434f-b36acdfd8dfa" — the quoted name and the quoted id are values taken from the request and from stored state; the surrounding sentence is the emulator's own fixed text. AWS emits no error at all here, so there is no correct wording for this refusal: the whole response is the defect, not its phrasing. The handler's own comment (services/service-com-amazonaws-mq-mq/src/ops/create_configuration.rs, the `configuration_by_name` branch) states it invented this refusal and invented the sentence because "No recorded response covers the SENTENCE for this refusal".
Why: a client that names configurations by convention (per-environment or per-broker templates, repeated IaC applies, blue/green rollouts) gets a hard 409 where AWS mints a second configuration. Code that keys configurations by `Id` — the AWS-documented identity — cannot create its second generation at all, and a caller that treats 409 as "already converged" will silently skip creating a configuration that does not exist. Any teardown/recreate loop that does not delete by `Id` first becomes permanently wedged.
Fix: CreateConfiguration's handler in services/service-com-amazonaws-mq-mq/src/ops/create_configuration.rs — remove the name-uniqueness refusal so a taken `Name` proceeds to the normal mint path (the store already mints a unique `Id` per create). The ConflictException variant stays modelled for genuine conflicts; it must not be raised on `Name`. ListConfigurations must then report both configurations.

Cause 2 of 2: UpdateConfiguration serves no revision at all — every call is refused, so revision numbering and its concurrency semantics are unobservable.
Failing check: mq.update-configuration-revision-concurrency
AWS: Smithy: UpdateConfiguration PUT /v1/configurations/{ConfigurationId}, required `ConfigurationId` (httpLabel) and `Data`, success HTTP 200 with `LatestRevision` (ConfigurationRevision, jsonName "latestRevision"), `Arn`, `Created`, `Id`, `Name`, `Warnings`; modelled errors BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, InternalServerErrorException 500. Recorded behavior: each successful UpdateConfiguration advances LatestRevision.Revision by exactly one, every successful concurrent writer is assigned a DISTINCT revision number, a writer that loses the race is refused with `ConflictException`, DescribeConfiguration afterwards reports LatestRevision.Revision == 1 + number of successful updates, and ListConfigurationRevisions lists revision 1 plus every assigned revision.
Emulator: UpdateConfiguration is a scaffold stub (services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs returns `crate::scaffolded::Scaffolded::not_implemented("UpdateConfiguration")`, HTTP 501 `NotImplemented`). With 8 concurrent writers against one freshly created configuration, ALL 8 calls were refused and ZERO succeeded — the check failed on "expected at least one racer to succeed". A separate auto-filed stub issue names this operation and file for the bare 501; what this check adds, and what that issue does not state, is the revision contract the implementation must satisfy: monotonic +1 increment, unique revision per concurrent winner, `ConflictException` (409) and nothing else for a loser, and a `LatestRevision`/ListConfigurationRevisions view consistent with the winners. Note also that it must refuse a well-formed but never-issued ConfigurationId with NotFoundException (404) rather than 501 or an implicit create.
Why: a client cannot push a configuration revision or pin a broker to one, so every ActiveMQ/RabbitMQ configuration workflow (broker configuration rollout, rollback to a prior revision) is dead against the emulator; and because the refusal is 501, retrying clients treat it as a transient server fault and loop instead of failing fast.
Fix: implement `handle` in services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs, with the revision mint taken under the same store lock that reads the current latest revision so two concurrent writers cannot be assigned the same number; state lives in services/service-com-amazonaws-mq-mq/src/state.rs (ConfigurationRecord.revisions, FIRST_CONFIGURATION_REVISION).

Traces: mq.create-configuration-duplicate-name-conflict-rejected (Cause 1), mq.update-configuration-revision-concurrency (Cause 2).

## Evidence
Cause 1 — mq.create-configuration-duplicate-name-conflict-rejected
Expected (AWS): two CreateConfiguration calls with EngineType=ACTIVEMQ and the SAME Name both return HTTP 200; the two `Id` values differ; ListConfigurations contains 2 entries whose Name equals that name.
Actual (emulator): the second call returns HTTP 409 ConflictException —
  botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the CreateConfiguration operation: a configuration named "sim2real-mqcfg-4ef79ffe681c" already exists in this account as "c-7d29e022-06dd-3887-434f-b36acdfd8dfa"
Fixed text invented by the emulator: 'a configuration named ' … ' already exists in this account as ' …; variable parts: the name from the request and the existing configuration id from state. AWS returns no error here, so no wording is correct.
Source: services/service-com-amazonaws-mq-mq/src/ops/create_configuration.rs — the `store.configuration_by_name(&input.name)` branch returning ConflictException with error_attribute "name".

Cause 2 — mq.update-configuration-revision-concurrency
Expected (AWS): of 8 concurrent UpdateConfiguration calls on one configuration, at least one succeeds (HTTP 200 with LatestRevision.Revision), all successful revisions are distinct, any failure code is exactly "ConflictException", DescribeConfiguration reports LatestRevision.Revision == 1 + successes, and ListConfigurationRevisions returns {1} ∪ successes.
Actual (emulator): 0 of 8 succeeded; assertion failed at check line 54 — AssertionError: expected at least one racer to succeed. UpdateConfiguration answers HTTP 501 NotImplemented from the scaffold stub at services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs:169.

## Steps to Reproduce
Cause 1:
1. mq.create_configuration(EngineType="ACTIVEMQ", Name="<any-valid-name>") -> note Id.
2. mq.create_configuration(EngineType="ACTIVEMQ", Name="<the same name>").
   Expected: HTTP 200 with a second, different Id. Observed: HTTP 409 ConflictException.
3. mq.list_configurations() -> expected 2 entries with that Name; the emulator holds only 1.
Replay: mq.create-configuration-duplicate-name-conflict-rejected

Cause 2:
1. mq.create_configuration(EngineType="ACTIVEMQ", Name="<any-valid-name>") -> ConfigurationId.
2. Issue 8 concurrent mq.update_configuration(ConfigurationId=<that id>, Data=<base64 ActiveMQ broker XML>) calls from a thread pool.
   Expected: >=1 HTTP 200 with distinct LatestRevision.Revision values; losers only ConflictException.
   Observed: all 8 refused with HTTP 501 NotImplemented.
3. mq.describe_configuration(ConfigurationId=<id>) -> expected LatestRevision.Revision == 1 + number of successes.
4. mq.update_configuration(ConfigurationId="c-00000000-0000-0000-0000-000000000000", Data=<base64 xml>) -> expected HTTP 404 NotFoundException, not 501.
Replay: mq.update-configuration-revision-concurrency
