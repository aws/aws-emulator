Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.schema-creation-then-merge-ordering appsync.start-schema-creation-builtin-scalar-matrix-or-failed
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.schema-creation-then-merge-ordering|run#17|AssertionError|assert fresh_merge_date is None, (
Issue-Key: appsync.start-schema-creation-builtin-scalar-matrix-or-failed|run#9|AssertionError|assert control_final["details"] == "Successfully created schema with 1 types.", (
Issue-Commit: a979fccb6b52301cb1b692ed521070b8834453a1

# appsync: 2 net-new causes — lastSuccessfulMergeDate is set before any merge has succeeded, and the schema-creation SUCCESS details over-counts the types compiled

> _Note: this issue was identified against the code base at commit `a979fccb6b52301cb1b692ed521070b8834453a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSourceApiAssociation

## Description
Two net-new root causes in the appsync emulator, each grounded in a failing behavioral check of this suite and each carrying its own failing check ids. Both must be closed before this issue is submitted.

=== Cause 1 of 2: a freshly created SourceApiAssociation already reports a lastSuccessfulMergeDate, so a merge that never ran looks like it succeeded ===
AWS: Smithy: SourceApiAssociation$lastSuccessfulMergeDate (target Date, timestamp) is documented as "The datetime value of the last successful merge of the source API association", and it is an OPTIONAL member. Immediately after AssociateSourceGraphqlApi (POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations) with sourceApiAssociationConfig.mergeType = MANUAL_MERGE, no merge has been performed, so GetSourceApiAssociation (GET /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}, 200) returns the association with lastSuccessfulMergeDate ABSENT; the member only appears once the association has reached MERGE_SUCCESS at least once, and then carries the time of that merge (recorded behavior for the absence; the model states the member's meaning).
Emulator: GetSourceApiAssociation on an association created moments earlier — before any StartSchemaCreation on the source API and before any StartSchemaMerge — already returns a populated lastSuccessfulMergeDate (the replayed check observed a timestamp whose value is simply the association time; the per-run timestamp value is not the requirement, its PRESENCE is). The emulator stamps the field at association time instead of at merge completion.
Why: the presence of lastSuccessfulMergeDate is the only signal a client has that the merged API's schema actually contains the source API's content, and it is what orchestration uses to decide whether a merge is still pending or how stale the merged schema is. Against the emulator every brand-new association claims a successful merge, so a client skips the StartSchemaMerge it must perform and then queries a merged API whose schema is empty; the "never merged" state is unreachable.
Fix: the source-API-association state plus the AssociateSourceGraphqlApi / AssociateMergedGraphqlApi / GetSourceApiAssociation / UpdateSourceApiAssociation projection — leave lastSuccessfulMergeDate unset when the association is created and only set it from the merge routine when the association transitions to MERGE_SUCCESS. NOTE: open issue 20261029T200818Z-appsync-source-api-association-projection-and-merge-status.md (Cause 6) describes a populated lastSuccessfulMergeDate on a merge that should have FAILED, for a different set of wire traces; this behavioral check was not marked owned by it and may be closed by the same fix.
Traces: appsync.schema-creation-then-merge-ordering (AssertionError: expected no lastSuccessfulMergeDate before any merge has succeeded, got a datetime).

=== Cause 2 of 2: GetSchemaCreationStatus's SUCCESS details over-counts the types created by the submitted SDL ===
AWS: Smithy: StartSchemaCreation (POST /v1/apis/{apiId}/schemacreation) returns status PROCESSING and GetSchemaCreationStatus (GET /v1/apis/{apiId}/schemacreation) returns {status: SchemaStatus, details: String}; SchemaStatus values are PROCESSING, ACTIVE, DELETING, FAILED, SUCCESS, NOT_APPLICABLE. Recorded behavior: on a successful compile the details string is the template "Successfully created schema with <N> types." — fixed text "Successfully created schema with " and " types.", with N taken from the submitted definition: the number of types the SDL itself declares. AppSync's service-defined scalars (AWSDate, AWSTime, AWSDateTime, AWSTimestamp, AWSEmail, AWSJSON, AWSPhone, AWSURL, AWSIPAddress) and the implicit schema/root wiring are NOT counted. For a definition declaring exactly one object type (`type Query { ... }`, all of whose fields are built-in scalars) AWS reports N = 1.
Emulator: for that same single-type SDL the terminal GetSchemaCreationStatus is SUCCESS but details is "Successfully created schema with 2 types." — N is one higher than the number of types the definition declares, i.e. the counter includes a type the SDL never declared (hypothesis: the implicit schema root / an injected operation type is counted alongside Query).
Why: details is the only report a client gets about what the asynchronous schema-creation job actually compiled. A client (or CI assertion, or console view) that reads the count to confirm its schema compiled as written is told the API holds a type it never declared, and the same off-by-one will scale with every schema, so no count the emulator reports can be trusted or diffed against AWS.
Fix: the schema-creation job's type-count computation that builds the SUCCESS details string, in the StartSchemaCreation / GetSchemaCreationStatus handler and schema state — count only the types declared by the submitted definition, excluding built-in scalars and any implicit root wiring. NOTE: open issue 20261124T160758Z-appsync-getschemacreationstatus-reports-a-static-details-string-instead.md concerns the details text for the "no schema creation ongoing" case; this is the SUCCESS-case count and is not covered by it.
Traces: appsync.start-schema-creation-builtin-scalar-matrix-or-failed (AssertionError: unexpected success details: 'Successfully created schema with 2 types.').

## Evidence
Cause 1 — check appsync.schema-creation-then-merge-ordering.
Setup: CreateGraphqlApi (source, API_KEY) -> CreateGraphqlApi (apiType=MERGED, mergedApiExecutionRoleArn=<iam role>) -> AssociateSourceGraphqlApi(mergedApiIdentifier=<merged>, sourceApiIdentifier=<source>, sourceApiAssociationConfig={mergeType: MANUAL_MERGE}).
Expected: GetSourceApiAssociation(associationId=<assoc>, mergedApiIdentifier=<merged>) -> 200 with sourceApiAssociation.lastSuccessfulMergeDate ABSENT (None), because neither StartSchemaCreation nor StartSchemaMerge has run.
Actual: the member is present and carries a timestamp — replay reported `AssertionError: expected no lastSuccessfulMergeDate before any merge has succeeded, got datetime.datetime(...)`. (The timestamp's literal value is a per-run value and is not part of the requirement; its presence is the divergence.)

Cause 2 — check appsync.start-schema-creation-builtin-scalar-matrix-or-failed.
Setup: CreateGraphqlApi(authenticationType=API_KEY), then StartSchemaCreation(apiId=<api>, definition="type Query {\n  birthday: AWSDate\n  curfew: AWSTime\n  checkin: AWSDateTime\n  epoch: AWSTimestamp\n  sender: AWSEmail\n  payload: AWSJSON\n  hotline: AWSPhone\n  homepage: AWSURL\n  origin: AWSIPAddress\n}\n") — one declared type.
Expected: StartSchemaCreation -> status PROCESSING; polling GetSchemaCreationStatus to a terminal status -> {status: 'SUCCESS', details: 'Successfully created schema with 1 types.'}.
Actual: status SUCCESS but details = 'Successfully created schema with 2 types.' — replay reported `AssertionError: unexpected success details: 'Successfully created schema with 2 types.'`. Fixed text matches AWS; the variable part (the type count derived from the request's definition) is wrong by one.

## Steps to Reproduce
Cause 1:
1. Clone the repo, boot the emulator, replay the behavioral suite and observe check appsync.schema-creation-then-merge-ordering.
2. By hand: appsync.create_graphql_api(name=<src>, authenticationType='API_KEY'); create an IAM role assumable by appsync.amazonaws.com; appsync.create_graphql_api(name=<merged>, authenticationType='API_KEY', apiType='MERGED', mergedApiExecutionRoleArn=<role arn>).
3. appsync.associate_source_graphql_api(mergedApiIdentifier=<merged id>, sourceApiIdentifier=<src id>, sourceApiAssociationConfig={'mergeType': 'MANUAL_MERGE'}).
4. appsync.get_source_api_association(associationId=<assoc id>, mergedApiIdentifier=<merged id>) and inspect sourceApiAssociation: lastSuccessfulMergeDate is present although no StartSchemaMerge has ever been called. It must be absent until the association reaches MERGE_SUCCESS.

Cause 2:
1. Replay the behavioral suite and observe check appsync.start-schema-creation-builtin-scalar-matrix-or-failed.
2. By hand: appsync.create_graphql_api(name=<api>, authenticationType='API_KEY'); appsync.start_schema_creation(apiId=<api id>, definition=b'type Query {\n  birthday: AWSDate\n  curfew: AWSTime\n  checkin: AWSDateTime\n  epoch: AWSTimestamp\n  sender: AWSEmail\n  payload: AWSJSON\n  hotline: AWSPhone\n  homepage: AWSURL\n  origin: AWSIPAddress\n}\n').
3. Poll appsync.get_schema_creation_status(apiId=<api id>) until status is SUCCESS or FAILED and read details: it says 'Successfully created schema with 2 types.' where AWS says 'Successfully created schema with 1 types.' for this one-type definition.
