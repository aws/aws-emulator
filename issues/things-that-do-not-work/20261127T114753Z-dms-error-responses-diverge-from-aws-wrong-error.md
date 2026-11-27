Issue-Source: trace-sweep-divergence
Issue-Trace: dms-metadata-model-import-needs-an-existing-project
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dms-metadata-model-import-needs-an-existing-project replication-task-stop-unknown-arn-rejected start-replication-task-unknown-arn-rejected
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|StartMetadataModelImport|4xx->4xx|$.__type
Issue-Key: dms|StartMetadataModelImport|4xx->4xx|$.message
Issue-Key: dms|StopReplicationTask|4xx->4xx|$.message
Issue-Key: dms|StopReplicationTask|4xx->4xx|$.__type
Issue-Key: dms|StartReplicationTask|4xx->4xx|$.message
Issue-Key: dms|DescribeReplicationTasks|4xx->4xx|$.message
Issue-Commit: 8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea

# dms error responses diverge from AWS: wrong error codes and rephrased messages for replication-task ARN and enum validation refusals

> _Note: this issue was identified against the code base at commit `8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMetadataModelImport

## Description
Five distinct causes, all in the dms service: every refusal the emulator serves for a bad or absent replication-task identifier (and for an out-of-set enum member) carries the emulator's own uniform wording — and in two cases its own error code — in place of AWS's per-operation error code and message. All five arrive with the correct HTTP status (400), so a client sees a well-formed failure whose code and text are wrong. Each cause must be closed separately; they live in different handlers.

Cause 1 of 5: StartMetadataModelImport with an out-of-set Origin value.
AWS:      Smithy (dms) StartMetadataModelImport.Origin is a required member targeting enum OriginTypeValue with values [SOURCE, TARGET]; the operation's modeled errors are AccessDeniedFault, InvalidResourceStateFault, KMSKeyNotAccessibleFault, ResourceAlreadyExistsFault, ResourceNotFoundFault, ResourceQuotaExceededFault, S3AccessDeniedFault, S3ResourceNotFoundFault. The code and text used for an out-of-set enum member are recorded behavior only: the recording shows HTTP 400 with __type 'InvalidParameterValueException' and message whose fixed text is "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [TARGET, SOURCE]". Variable parts taken from the request: <value> is the rejected Origin value and <member> is the wire member name ('origin'). The bracketed value set is fixed text as AWS words it for this member — AWS enumerates it TARGET first, which is NOT the model's declaration order, so the set cannot be rendered by iterating the modeled enum.
Emulator: HTTP 400 with __type 'ValidationException' and message "1 validation error detected: Value 'BOTH' at 'origin' failed to satisfy constraint: Member must satisfy enum value set: [SOURCE, TARGET]" — the wrong error code, and the value set rendered in model-declaration order instead of AWS's order.
Why:      a client that branches on the error code never matches InvalidParameterValueException and treats a parameter rejection as an unmodeled/unknown fault; an operator or test that asserts the enum-set text sees a set in the wrong order.
Fix:      StartMetadataModelImport's input validation and its error serializer (the enum-member check must raise the InvalidParameterValueException code and emit the value set in AWS's documented order for this member).
Traces:   dms-metadata-model-import-needs-an-existing-project#task#4.

Cause 2 of 5: StopReplicationTask with a well-formed but absent replication-task ARN.
AWS:      Smithy (dms) StopReplicationTask takes required ReplicationTaskArn and models InvalidResourceStateFault and ResourceNotFoundFault (client errors, HTTP 400). The text is recorded behavior only: HTTP 400, error code ResourceNotFoundFault, message whose fixed text is "Replication Task ARN:<arn> does not exist" — the literal prefix "Replication Task ARN:" with no space after the colon, then the ReplicationTaskArn exactly as supplied in the request, then the literal suffix " does not exist".
Emulator: the error code matches, but the message is the emulator's own generic wording "ReplicationTaskArn <arn> does not name a replication task in this account".
Why:      the message is part of the error contract; a client or operator reading or matching the not-found text for a replication task gets wording AWS never emits, and the same generic sentence is reused for operations AWS words differently (see Cause 4).
Fix:      the StopReplicationTask handler's not-found path (its error-message construction), not a shared generic not-found string.
Traces:   replication-task-stop-unknown-arn-rejected#task#1.

Cause 3 of 5: StopReplicationTask with a value that is not an ARN at all.
AWS:      recorded behavior only — a ReplicationTaskArn that does not parse as an ARN is a parameter-validation failure, not a lookup miss: HTTP 400, error code InvalidParameterValueException, message whose fixed text is "Invalid value <value> for ReplicationTaskArn", where <value> is the request's own ReplicationTaskArn string and "ReplicationTaskArn" is the member name. (The Smithy model lists only InvalidResourceStateFault/ResourceNotFoundFault for this operation, so the malformed-input code is taken from the recording.)
Emulator: HTTP 400 with error code ResourceNotFoundFault and message "ReplicationTaskArn SIMPARITYABSENTTASKQZQZQZQZ does not name a replication task in this account" — it treats a syntactically invalid identifier as an absent resource, so both the code and the message are wrong.
Why:      a client cannot distinguish 'you sent me garbage' from 'that task is gone': retry/backfill logic keyed on ResourceNotFoundFault will swallow a genuine request-construction bug, and validation-error handling keyed on InvalidParameterValueException never fires.
Fix:      StopReplicationTask's input validation — the ARN syntax check must run before the state lookup and raise InvalidParameterValueException with the "Invalid value ... for <member>" message.
Traces:   replication-task-stop-unknown-arn-rejected#task#2.

Cause 4 of 5: StartReplicationTask with a well-formed but absent replication-task ARN.
AWS:      Smithy (dms) StartReplicationTask takes required ReplicationTaskArn and StartReplicationTaskType and models AccessDeniedFault, InvalidResourceStateFault and ResourceNotFoundFault (client errors, HTTP 400). The text is recorded behavior only, and it is NOT the same sentence StopReplicationTask uses: HTTP 400, error code ResourceNotFoundFault, message whose fixed text is "Replication Task does not exist for Arn : <arn>" — the literal prefix "Replication Task does not exist for Arn : " (space, colon, space exactly as shown) followed by the request's ReplicationTaskArn.
Emulator: the error code matches, but the message is again the shared generic "ReplicationTaskArn <arn> does not name a replication task in this account", identical to what it serves for StopReplicationTask — AWS words the two operations differently.
Why:      a client matching the start-path not-found text never matches, and the emulator erases the per-operation wording distinction AWS actually ships.
Fix:      the StartReplicationTask handler's not-found path (its own error-message construction, separate from StopReplicationTask's).
Traces:   replication-task-stop-unknown-arn-rejected#task#3 (same cluster: start-replication-task-unknown-arn-rejected#task#1).

Cause 5 of 5: DescribeReplicationTasks whose filters match nothing.
AWS:      Smithy (dms) DescribeReplicationTasks takes Filters/MaxRecords/Marker/WithoutSettings and models ResourceNotFoundFault (client error, HTTP 400) — a filter that selects no task is an error, not an empty ReplicationTasks list. The text is recorded behavior only and is entirely fixed, with no values from the request or state: HTTP 400, error code ResourceNotFoundFault, message "No tasks found using provided filter." (including the trailing period).
Emulator: HTTP 400 with the right code but the message "No ReplicationTasks found matching provided filters" — rephrased, pluralised differently and missing the trailing period.
Why:      this message has no variable parts, so any client, test or operator that matches it matches a literal string; the emulator's wording never matches AWS's.
Fix:      DescribeReplicationTasks' handler — the empty-filter-result refusal's message text.
Traces:   start-replication-task-unknown-arn-rejected#task#2.

Related open issues that do NOT cover these: 20261125T220330Z-dms-ec2-iam-s3-the-s3-to-s3.md is a behavioral finding about the DMS replication lifecycle never settling (its StopReplicationTask expectation is the InvalidResourceStateFault 'is currently not running' refusal, a different path) and carries no wire replay key; the three dms stub issues (CreateInstanceProfile, CreateReplicationConfig, StartReplicationTaskAssessmentRun) are unimplemented-operation scaffolds. None of them carries the fixture#step keys listed above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false). All five clusters: expected_status 400, actual_status 400 — the divergence is in the error code and/or message body.

Cause 1 — fixture dms-metadata-model-import-needs-an-existing-project, step task#4, StartMetadataModelImport:
  $.__type:   expected 'InvalidParameterValueException', got 'ValidationException'
  $.message:  expected "1 validation error detected: Value 'BOTH' at 'origin' failed to satisfy constraint: Member must satisfy enum value set: [TARGET, SOURCE]"
              got      "1 validation error detected: Value 'BOTH' at 'origin' failed to satisfy constraint: Member must satisfy enum value set: [SOURCE, TARGET]"
  (Fixed text: everything except the quoted value and the member name; 'BOTH' and 'origin' come from the request. The bracketed set is fixed: AWS emits TARGET before SOURCE, while Smithy declares OriginTypeValue as [SOURCE, TARGET].)

Cause 2 — fixture replication-task-stop-unknown-arn-rejected, step task#1, StopReplicationTask:
  $.message:  expected 'Replication Task ARN:<arn> does not exist'
              got      'ReplicationTaskArn <arn> does not name a replication task in this account'
  (Fixed text: 'Replication Task ARN:' and ' does not exist'; <arn> is the request's ReplicationTaskArn — in this recording an absent task ARN in the us-west-2/000000000000 account. __type matched.)

Cause 3 — fixture replication-task-stop-unknown-arn-rejected, step task#2, StopReplicationTask with a non-ARN identifier:
  $.__type:   expected 'InvalidParameterValueException', got 'ResourceNotFoundFault'
  $.message:  expected 'Invalid value <value> for ReplicationTaskArn'
              got      '<value> does not name a replication task in this account' (prefixed 'ReplicationTaskArn ')
  (Fixed text: 'Invalid value ' and ' for ReplicationTaskArn'; <value> is the request's own malformed ReplicationTaskArn string.)

Cause 4 — fixture replication-task-stop-unknown-arn-rejected, step task#3, StartReplicationTask (cluster also covers start-replication-task-unknown-arn-rejected#task#1):
  $.message:  expected 'Replication Task does not exist for Arn : <arn>'
              got      'ReplicationTaskArn <arn> does not name a replication task in this account'
  (Fixed text: 'Replication Task does not exist for Arn : '; <arn> is the request's ReplicationTaskArn. __type matched.)

Cause 5 — fixture start-replication-task-unknown-arn-rejected, step task#2, DescribeReplicationTasks:
  $.message:  expected 'No tasks found using provided filter.'
              got      'No ReplicationTasks found matching provided filters'
  (Entirely fixed text — no request or state values at all. __type matched.)

Model evidence: smithy_query operation StopReplicationTask/StartReplicationTask/DescribeReplicationTasks/StartMetadataModelImport and shape OriginTypeValue on services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json.

## Steps to Reproduce
1. Clone the repo and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: call StartMetadataModelImport with a required Origin value outside the OriginTypeValue set (e.g. 'BOTH') plus MigrationProjectIdentifier and SelectionRules. Expect HTTP 400, __type 'InvalidParameterValueException', message "1 validation error detected: Value 'BOTH' at 'origin' failed to satisfy constraint: Member must satisfy enum value set: [TARGET, SOURCE]". Observed: __type 'ValidationException' and the set rendered [SOURCE, TARGET]. (fixture dms-metadata-model-import-needs-an-existing-project, step task#4)
3. Cause 2: call StopReplicationTask with a syntactically valid replication-task ARN that does not exist. Expect HTTP 400 ResourceNotFoundFault, message 'Replication Task ARN:<that arn> does not exist'. Observed the emulator's generic 'ReplicationTaskArn <arn> does not name a replication task in this account'. (fixture replication-task-stop-unknown-arn-rejected, step task#1)
4. Cause 3: call StopReplicationTask with a ReplicationTaskArn that is not an ARN at all (a bare token). Expect HTTP 400 InvalidParameterValueException, message 'Invalid value <token> for ReplicationTaskArn'. Observed HTTP 400 ResourceNotFoundFault with the generic not-found sentence. (fixture replication-task-stop-unknown-arn-rejected, step task#2)
5. Cause 4: call StartReplicationTask with StartReplicationTaskType set and a valid-looking but absent ReplicationTaskArn. Expect HTTP 400 ResourceNotFoundFault, message 'Replication Task does not exist for Arn : <that arn>'. Observed the same generic sentence the stop path returns. (fixture replication-task-stop-unknown-arn-rejected, step task#3; also start-replication-task-unknown-arn-rejected, step task#1)
6. Cause 5: call DescribeReplicationTasks with Filters selecting a replication-task-arn that matches no task. Expect HTTP 400 ResourceNotFoundFault, message exactly 'No tasks found using provided filter.' Observed 'No ReplicationTasks found matching provided filters'. (fixture start-replication-task-unknown-arn-rejected, step task#2)
7. Re-replay those fixtures/steps after fixing; all five must match on __type and message.
