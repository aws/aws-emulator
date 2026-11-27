Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.archive-wave-with-active-application-rejected mgn.archived-application-refuses-update-until-unarchived-rejected mgn.delete-wave-detaches-its-applications mgn.list-network-migration-executions-unissued-next-token-rejected mgn.network-migration-definition-create-stages-listed-execution
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|CreateWave|UninitializedAccountException
Issue-Key: mgn|CreateApplication|UninitializedAccountException
Issue-Key: mgn.list-network-migration-executions-unissued-next-token-rejected|run#16|AssertionError|assert got == ("ValidationException", "Validation failed", 400), (
Issue-Key: mgn.network-migration-definition-create-stages-listed-execution|run#16|AssertionError|assert summary == ("PENDING", "MAPPING", "MAPPING"), (
Issue-Commit: 9a0283ef64a159d5cd09f281cdd10e3b4b9ba485

# mgn: 3 net-new causes — the whole wave/application control plane answers UninitializedAccountException after initialization, the unissued-nextToken refusal is emulator-worded, and a staged network-migration execution reports no activity

> _Note: this issue was identified against the code base at commit `9a0283ef64a159d5cd09f281cdd10e3b4b9ba485`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWave

## Description
Three net-new root causes in the mgn (ApplicationMigrationService) crate. Each must be closed separately; all three checks listed under each cause must pass before this issue is closed.

Cause 1 of 3: after the account-initialization prerequisite has been satisfied, every account-gated wave/application operation still refuses with UninitializedAccountException, so the wave and application control plane cannot be exercised at all.
AWS: Smithy (com.amazonaws.mgn#CreateWave, #CreateApplication) binds POST /CreateWave and POST /CreateApplication with success code 201 and models ConflictException (409), ServiceQuotaExceededException (402) and UninitializedAccountException (400). UninitializedAccountException is the answer for an account that has NOT been initialized; for an account that HAS been (the state these checks declare as their prerequisite, `requires:mgn-initialized`), the recorded behavior is a 201 carrying a `waveID` matching ^wave-[0-9a-zA-Z]{17}$ / an `applicationID` matching ^app-[0-9a-zA-Z]{17}$. The recording shows the whole downstream flow succeeding on that account: AssociateApplications, ListWaves, ArchiveWave/UnarchiveWave, DeleteWave.
Emulator: every one of those creates is refused with HTTP 400, error code UninitializedAccountException, message "Account not initialized" — the gate never clears for the account the suite acts on. Replay detail: "botocore.errorfactory.UninitializedAccountException: An error occurred (UninitializedAccountException) when calling the CreateWave operation: Account not initialized" (and the same on CreateApplication). The gate itself is services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_wave.rs:186 (`if !store.is_initialized(&account)`), mirrored in create_application.rs and the other gated handlers. Hypothesis: the only path to an initialized account is InitializeService (src/ops/initialize_service.rs), which records the flag ONLY after crate::state::establish_account_initialization (src/state/account_initialization.rs:153) reports the service-linked role AND both SERVER_INSTANCE_PROFILES as established by a read of emulated IAM; one of those reads answers `Some(false)` here (e.g. an instance profile that exists but holds no role), so the attempt concludes `Failed`, InitializeService answers the modelled ValidationException instead of 204, and `initialized_accounts` is never written — leaving every gated operation permanently refusing. The emulator's own replay shows no gated operation ever succeeding.
Why: a client that initializes the account and then builds its migration plan gets a hard 400 on the very first write. Waves, applications, their association, archival and deletion are MGN's primary control plane; with the gate stuck closed none of it is reachable, and the error a client sees tells it to initialize an account it has already initialized, so no retry strategy recovers.
Fix: the account-initialization state and the handler that establishes it — src/ops/initialize_service.rs together with src/state/account_initialization.rs (the IAM establishment it concludes from) and the `is_initialized` gate the create/update handlers read (src/state.rs, create_wave.rs, create_application.rs). A successful InitializeService must leave the caller's account recorded as initialized so the gated operations stop refusing; the gate must only refuse an account that genuinely has not been initialized.
Traces: mgn.archive-wave-with-active-application-rejected, mgn.archived-application-refuses-update-until-unarchived-rejected, mgn.delete-wave-detaches-its-applications (each aborts on the first CreateWave/CreateApplication, so nothing after it was observed). The same root cause is reported for the mgn__s3 heading in the companion issue filed in this sweep (cause 2 there), whose check must be re-replayed with these.

Cause 2 of 3: the ListNetworkMigrationExecutions refusal for a nextToken the service never issued carries the emulator's own sentence instead of the AWS message.
AWS: Smithy (com.amazonaws.mgn#ListNetworkMigrationExecutions) binds POST /network-migration/ListNetworkMigrationExecutions with nextToken (PaginationToken, @length 0..2048) and models only AccessDeniedException (403) and ResourceNotFoundException (404) — recorded behavior only for this refusal: a nextToken the service never issued is answered with HTTP 400, error code ValidationException and the message "Validation failed". That message is entirely FIXED text — it has no variable part, and in particular it does not echo the offending token. An empty nextToken is not a refusal: it lists the first page.
Emulator: HTTP 400 and error code ValidationException (both correct) with the message "nextToken 'sim2real-unissued-page-marker' cannot be resumed: it is not a token this service issued, or the network migration execution it continues from is no longer in this selection" — a hand-written sentence that also interpolates the caller's token. Replay detail: "AssertionError: nextToken 'sim2real-unissued-page-marker' was answered ('ValidationException', \"nextToken '...' cannot be resumed: ...\", 400), expected ('ValidationException', 'Validation failed', 400)". The sentence is built in `unresumable_token` at services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/list_network_migration_executions.rs:276-280 via crate::scaffolded::Scaffolded::validation.
Why: the message is part of the refusal contract. A client (or a test harness, or a log-based alarm) that matches on AWS's "Validation failed" sees an unrecognised sentence, and echoing the caller's token back changes the message per request so nothing can match it at all.
Fix: the refusal builder for this operation's pagination token (`unresumable_token` in list_network_migration_executions.rs) must word the modelled ValidationException with AWS's fixed sentence "Validation failed" and carry no request-derived text; the 400 status and ValidationException code it already produces are correct.
Traces: mgn.list-network-migration-executions-unissued-next-token-rejected (both an unissued plain string and an unissued well-formed base64 token are refused the same way).

Cause 3 of 3: the execution a new network migration definition stages carries no `activity`.
AWS: Smithy (com.amazonaws.mgn#NetworkMigrationExecution) models `status` (ExecutionStatus: PENDING|STARTED|SUCCEEDED|FAILED), `stage` (ExecutionStage: MAPPING|CODE_GENERATION|DEPLOY|DEPLOYED_STACKS_DELETION|ANALYZE) and `activity` (ExecutionStageActivity: MAPPING|MAPPING_UPDATE|CODE_GENERATION|DEPLOY|DEPLOYED_STACKS_DELETION|ANALYZE, documented as "The current activity being performed in the execution"). Recorded behavior: CreateNetworkMigrationDefinition stages exactly one execution and the next ListNetworkMigrationExecutions reports it as (status, stage, activity) = (PENDING, MAPPING, MAPPING) — the activity member is present on a freshly staged execution and names the stage's activity, it is not absent.
Emulator: the listed execution reports (PENDING, MAPPING, None) — `activity` is omitted. Replay detail: "AssertionError: a new definition's execution had (status, stage, activity) ('PENDING', 'MAPPING', None), expected ('PENDING', 'MAPPING', 'MAPPING')". The staged record is built with `activity: None` in NetworkMigrationExecution::staged (services/service-com-amazonaws-mgn-applicationmigrationservice/src/state.rs, in the `staged` constructor, with the comment "with no activity, because nothing has run for it yet"); `to_shape` then projects that absence straight onto the wire.
Why: a client reading an execution to decide what the pipeline is doing gets a null where AWS always reports an ExecutionStageActivity, so code that switches on the activity (or simply formats it) sees a missing key on every freshly staged execution and cannot tell a staged execution apart from one whose activity the service declined to report.
Fix: the execution state that a definition's create stages — the `staged` constructor in src/state.rs — must set the activity of the pipeline's first stage (the crate already names it, EXECUTION_ACTIVITY_MAPPING) alongside the stage it sets, so every read of a staged execution projects it.
Traces: mgn.network-migration-definition-create-stages-listed-execution.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 — mgn 21/28 passed, 7 failed (2 of the 7 are already OWNED by 20261023T003710Z-mgn-tag-api-and-createlaunchconfigurationtemplate-answer-501-notimplemented.md and are not part of this issue).

[error] mgn.archive-wave-with-active-application-rejected
  UninitializedAccountException: An error occurred (UninitializedAccountException) when calling the CreateWave operation: Account not initialized
  Expected (recorded AWS, account initialized per the check's `requires:mgn-initialized` prerequisite): HTTP 201 with waveID matching ^wave-[0-9a-zA-Z]{17}$.
  Actual (emulator): HTTP 400, code UninitializedAccountException, message 'Account not initialized'.

[error] mgn.archived-application-refuses-update-until-unarchived-rejected
  UninitializedAccountException ... when calling the CreateApplication operation: Account not initialized
  Expected: HTTP 201 with applicationID matching ^app-[0-9a-zA-Z]{17}$.

[error] mgn.delete-wave-detaches-its-applications
  UninitializedAccountException ... when calling the CreateWave operation: Account not initialized
  Gate site: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_wave.rs:186; the only path that clears it is src/ops/initialize_service.rs via src/state/account_initialization.rs:153.

[mismatch] mgn.list-network-migration-executions-unissued-next-token-rejected
  AssertionError: nextToken 'sim2real-unissued-page-marker' was answered ('ValidationException', "nextToken 'sim2real-unissued-page-marker' cannot be resumed: it is not a token this service issued, or the network migration execution it continues from is no longer in this selection", 400), expected ('ValidationException', 'Validation failed', 400)
  Expected (recorded AWS): 400 / ValidationException / 'Validation failed' (wholly fixed text, no interpolated token).
  Actual (emulator): 400 / ValidationException / the sentence above, built at src/ops/list_network_migration_executions.rs:276-280.
  Positive controls in the same check pass: no token and an empty nextToken both list the one execution.

[mismatch] mgn.network-migration-definition-create-stages-listed-execution
  AssertionError: a new definition's execution had (status, stage, activity) ('PENDING', 'MAPPING', None), expected ('PENDING', 'MAPPING', 'MAPPING')
  Expected (recorded AWS): the staged execution reports activity = MAPPING (model: ExecutionStageActivity values MAPPING, MAPPING_UPDATE, CODE_GENERATION, DEPLOY, DEPLOYED_STACKS_DELETION, ANALYZE).
  Actual (emulator): activity absent; set to None in NetworkMigrationExecution::staged (src/state.rs) and projected by to_shape.

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  CreateWave: POST /CreateWave, success 201; errors ConflictException(409), ServiceQuotaExceededException(402), UninitializedAccountException(400).
  CreateApplication: POST /CreateApplication, success 201; same three errors.
  ListNetworkMigrationExecutions: POST /network-migration/ListNetworkMigrationExecutions, success 200; errors AccessDeniedException(403), ResourceNotFoundException(404) only — the ValidationException for a bad token is recorded behavior.
  NetworkMigrationExecution members: networkMigrationDefinitionID, networkMigrationExecutionID, status, stage, activity, createdAt, updatedAt, tags.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, or just the five checks named in trace_ids.
3. Cause 1: satisfy the mgn account-initialization prerequisite (InitializeService), then call CreateWave(name=<fresh name>) and CreateApplication(name=<fresh name>). The emulator answers HTTP 400 UninitializedAccountException 'Account not initialized'; AWS answers 201 with the minted waveID/applicationID, and the rest of the wave/application lifecycle (AssociateApplications, ArchiveWave, DeleteWave) proceeds.
4. Cause 2: CreateNetworkMigrationDefinition, wait for its staged execution to list, then call ListNetworkMigrationExecutions(networkMigrationDefinitionID=<nmd>, nextToken='sim2real-unissued-page-marker') and again with nextToken='eyJwYWdlIjogMn0='. Both must be refused 400 / ValidationException / 'Validation failed'; the emulator answers the 'cannot be resumed: ...' sentence instead.
5. Cause 3: CreateNetworkMigrationDefinition, then ListNetworkMigrationExecutions for it. The one staged execution must report (status, stage, activity) = ('PENDING', 'MAPPING', 'MAPPING'); the emulator omits activity.
6. Re-replay mgn.archive-wave-with-active-application-rejected, mgn.archived-application-refuses-update-until-unarchived-rejected, mgn.delete-wave-detaches-its-applications, mgn.list-network-migration-executions-unissued-next-token-rejected and mgn.network-migration-definition-create-stages-listed-execution; ALL FIVE must pass before closing.
