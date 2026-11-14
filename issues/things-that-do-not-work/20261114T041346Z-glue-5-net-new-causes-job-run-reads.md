Issue-Source: trace-sweep-divergence
Issue-Trace: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning glue-classifier-custom-entity-and-data-quality-ruleset glue-workflow-trigger-lifecycle glue-schema-registry-versioning
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.ExecutionTime
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].ExecutionTime
Issue-Key: glue|CreateDataQualityRuleset|2xx->2xx|$.Identifier
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Edges[*].DestinationId
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Edges[*].SourceId
Issue-Key: glue|GetWorkflow|2xx->2xx|$.Workflow.Graph.Nodes[*].UniqueId
Issue-Key: glue|RegisterSchemaVersion|4xx->4xx|$.Message
Issue-Related: 20261102T132146Z-glue-job-and-job-run-reads-omit-modeled.md
Issue-Related: 20261111T142022Z-glue-job-run-session-and-workflow-reads-misreport.md
Issue-Commit: eadb2d0c274461491404935b82a39edede7a2374

# glue: 5 net-new causes — job-run reads drop AWS's executor envelope and zeroed fields, generated ids are hash digests, and the AVRO parse refusal is emulator-authored

> _Note: this issue was identified against the code base at commit `eadb2d0c274461491404935b82a39edede7a2374`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
Five net-new wire causes on the glue service, all reached AFTER the emulator answered the operation (every step below returned a 200/400 from a live handler, so none is a bootstrap gap or an unimplemented stub). A fixer must close ALL FIVE before submitting; each cause names its own fixture and step.

Cause 1 of 5 — GetJobRun's JobRun record for a run that failed BEFORE provisioning drops AWS's job-executor envelope on ErrorMessage, omits ExecutionArguments and reports a non-zero ExecutionTime.
AWS: Smithy: GetJobRun -> JobRun; JobRun.ErrorMessage is ErrorString, documented only as 'An error message associated with this job run', and JobRun.ExecutionTime is ExecutionTime (integer, @default 0) documented as 'The amount of time (in seconds) that the job run consumed resources'. JobRun carries no ExecutionArguments member in the model, so for that member and for the ErrorMessage text this is recorded behavior only. The recording shows that when the run cannot start because its execution role does not trust Glue, AWS (a) wraps the underlying reason in a fixed executor envelope: 'AccountId:<account-id> and JobName:<job-name> and JobRunId:<run-id> failed to execute with exception <reason sentence>\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <request-id>; Proxy: null)'. Fixed text: 'AccountId:', ' and JobName:', ' and JobRunId:', ' failed to execute with exception ', and the trailing '\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: ' ... '; Proxy: null)'. Variable parts taken from the request and from state: the account id, the job name, the run id, and the reason sentence (here the assume-role sentence 'Role <role-arn> should be given assume role permissions for Glue Service.'); the request id is a per-run value and is NOT part of the requirement. (b) the record carries ExecutionArguments — {} when the run was started with no argument overrides (and the run's arguments otherwise, as the recording in glue-job-run-arguments-propagated shows). (c) ExecutionTime is 0 for a run that never consumed resources, because the run failed before any executor was provisioned.
Emulator: ErrorMessage is only the bare reason sentence ('Role <role-arn> should be given assume role permissions for Glue Service.') with no envelope; ExecutionArguments is absent from the JobRun object; ExecutionTime is the wall-clock age of the record (replayed value 6) instead of 0.
Why: the envelope is how clients and log pipelines classify a Glue failure — the 'Service: AWSGlueJobExecutor' / 'Error Code: InvalidInputException' segments and the AccountId/JobName/JobRunId triple are what operators and parsers key on, so a bare sentence defeats every such matcher and loses the run's identity. A missing ExecutionArguments makes argument-echo checks and audit tooling read the run as having had no arguments at all, and a non-zero ExecutionTime for a run that never provisioned reports consumed DPU-seconds for work that never ran, so cost/attribution logic over job runs is wrong.
Fix: the glue job-run executor/serializer behind GetJobRun — the ErrorMessage formatter for pre-provisioning failures, the JobRun serializer's ExecutionArguments member, and the ExecutionTime accounting (which must stay 0 unless an executor actually ran). Related open issue that does NOT cover this: 20261109T085123Z-athena-glue-s3-3-net-new-causes-glue.md (cause 2) is about the in-job botocore ClientError text and the assumed-role session name, not the AWSGlueJobExecutor envelope of a run that failed before provisioning; 20261111T142022Z-glue-job-run-session-and-workflow-reads-misreport.md owns the ExecutionArguments omission for a run started WITH arguments, not this record.
Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4.

Cause 2 of 5 — the same job-run record read through GetJobRuns diverges identically, so the defect is in the shared JobRun projection, not in one read path.
AWS: as cause 1 (Smithy: GetJobRuns -> JobRuns: list of JobRun). The recorded JobRuns[0] carries the enveloped ErrorMessage, ExecutionArguments {} and ExecutionTime 0.
Emulator: JobRuns[0].ErrorMessage is the bare reason sentence, JobRuns[0].ExecutionArguments is absent, JobRuns[0].ExecutionTime is non-zero (replayed value 6).
Why: a client that lists a job's runs to find why it failed gets the same degraded record, so fixing only the single-run read would leave every list-based consumer diverging.
Fix: the shared JobRun projection used by both GetJobRun and GetJobRuns in the glue job-run serializer.
Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#5.

Cause 3 of 5 — CreateDataQualityRuleset returns an Identifier that is a deterministic hash digest of the request instead of a generated identifier of AWS's form.
AWS: Smithy: CreateDataQualityRulesetResponse declares only Name (NameString), so the Identifier the recorded response carries is recorded behavior only. The corpus asserts that member by SHAPE (its recorded value is not compared literally, since a generated id differs per run), i.e. AWS returns a freshly generated opaque identifier whose FORM is the requirement. Hypothesis: that form is the dashed UUID shape Glue uses for its other generated identifiers (compare Smithy SchemaVersionIdString, pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$).
Emulator: emits a 32-character lowercase hex digest with no separators, derived deterministically from the ruleset's identity (replayed value 'aeefa45fb936ea1555d59586d7e3c413'), which fails the shape assertion the recording's value satisfies.
Why: a client that stores, validates or parses the ruleset identifier (or compares it with identifiers returned elsewhere in the Data Quality surface) sees a token of the wrong form; worse, because the value is a pure function of the request, two creates that AWS would distinguish by identifier are indistinguishable here, so identifier-keyed bookkeeping collides.
Fix: the CreateDataQualityRuleset handler's identifier generation in the glue data-quality state (generate a per-create identifier of AWS's form and persist it with the ruleset, rather than hashing the request).
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#11.

Cause 4 of 5 — GetWorkflow's graph node and edge identifiers are hash digests of the component names instead of AWS-form generated ids.
AWS: Smithy: GetWorkflow -> Workflow; Node.UniqueId is NameString, documented 'The unique Id assigned to the node within the workflow', and Edge.SourceId/Edge.DestinationId are NameString, documented as 'The unique of the node within the workflow where the edge starts/ends'. The concrete token form is recorded behavior only and the corpus asserts it by SHAPE for all four members (Workflow.Graph.Nodes[*].UniqueId and Workflow.Graph.Edges[0].SourceId/DestinationId): AWS assigns each node an id of its generated-identifier form and the edges reference those ids. Hypothesis: the recorded form is the dashed UUID shape, as elsewhere in Glue.
Emulator: every node id and the edge endpoints referencing them are 32-character lowercase hex digests derived from the component name (replayed node ids '481af93a71b316a393d5d12957996b00' and '732470ba160b565bb4daa1f7ebd24e00', reused verbatim as Edges[0].SourceId/DestinationId), which fail that shape assertion.
Why: the workflow graph is consumed by visualisation and orchestration tooling that treats UniqueId as an opaque AWS-issued id; a name-derived digest is both the wrong form and name-coupled, so renaming or recreating a component silently changes or collides ids that AWS would keep distinct and stable per workflow.
Fix: the GetWorkflow graph builder / workflow state in the glue service — assign and persist a node identifier of AWS's form when the node enters the workflow graph and have edges reference the stored ids.
Traces: glue-workflow-trigger-lifecycle#task#7.

Cause 5 of 5 — RegisterSchemaVersion's invalid-AVRO refusal carries the emulator's own JSON parser diagnostic instead of AWS's.
AWS: Smithy: RegisterSchemaVersion (SchemaDefinition is SchemaDefinitionString, @required, length 1..170000) models InvalidInputException (client error, HTTP 400) for a definition that is not valid for the schema's data format; the message text is recorded behavior only. The recording shows AWS answers 400 InvalidInputException with the fixed prefix 'Schema definition of AVRO data format is invalid: ' followed by the parser's own diagnostic in AWS's fixed wording: "Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]". Fixed text: the prefix, "Unrecognized token '", "': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')", and the "\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: ", ', column: ', ']' tail. Variable parts taken from the submitted definition: the offending token and the line and column at which it occurs.
Emulator: same status (400) and error code, but the message is 'Schema definition of AVRO data format is invalid: expected ident at line 1 column 2' — the emulator's own JSON library wording, no token quoted, no expectation list, no Source/REDACTED segment, and a different column than the recorded diagnostic reports for the same definition.
Why: the refusal message is the only feedback a schema author gets about WHERE their definition is malformed; a client (or a CI assertion) that parses the token/line/column out of AWS's diagnostic extracts nothing here, and the differing column points at the wrong character.
Fix: the RegisterSchemaVersion validation path in the glue schema-registry handler — map the definition parse failure onto AWS's diagnostic template (quoted token, expectation list, Source/line/column tail) instead of surfacing the serializer's native error string. Related open issue that does NOT cover this: 20261108T174217Z-glue-schema-registry-version-registration-never-validates-or.md is about registration never validating/failing a definition and the version-not-found wording; here the definition IS refused and only the message diverges.
Traces: glue-schema-registry-versioning#task#8.

## Evidence
Cause 1 — fixture glue-gjwr-traj-start-the-copied-job-fails-before-provisioning, step task#4, GetJobRun, status 200 expected and 200 observed, three body divergences: $.JobRun.ErrorMessage value_mismatch — expected 'AccountId:<account-id> and JobName:<job-name> and JobRunId:<run-id> failed to execute with exception Role <role-arn> should be given assume role permissions for Glue Service.\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <request-id>; Proxy: null)', got only 'Role <role-arn> should be given assume role permissions for Glue Service.'; $.JobRun.ExecutionArguments missing — expected {}, got None; $.JobRun.ExecutionTime value_mismatch — expected 0, got 6 (the 6 is the emulator's elapsed wall clock; the requirement is 0 for a run that never provisioned, not the literal recorded number).

Cause 2 — same fixture, step task#5, GetJobRuns, status 200/200: $.JobRuns[0].ErrorMessage value_mismatch (bare reason sentence instead of the AWSGlueJobExecutor envelope), $.JobRuns[0].ExecutionArguments missing (expected {}, got None), $.JobRuns[0].ExecutionTime value_mismatch (expected 0, got 6).

Cause 3 — fixture glue-classifier-custom-entity-and-data-quality-ruleset, step task#11, CreateDataQualityRuleset, status 200/200: $.Identifier value_mismatch — expected a shape-asserted generated identifier ('<shaped-not-asserted>'), got the 32-hex digest 'aeefa45fb936ea1555d59586d7e3c413', which does not satisfy the recorded value's shape.

Cause 4 — fixture glue-workflow-trigger-lifecycle, step task#7, GetWorkflow, status 200/200: $.Workflow.Graph.Nodes[0].UniqueId, $.Workflow.Graph.Nodes[1].UniqueId, $.Workflow.Graph.Edges[0].SourceId and $.Workflow.Graph.Edges[0].DestinationId all value_mismatch — expected shape-asserted generated ids ('<shaped-not-asserted>'), got the 32-hex digests '481af93a71b316a393d5d12957996b00' and '732470ba160b565bb4daa1f7ebd24e00'.

Cause 5 — fixture glue-schema-registry-versioning, step task#8, RegisterSchemaVersion, status 400/400: $.Message value_mismatch — expected "Schema definition of AVRO data format is invalid: Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 4]", got 'Schema definition of AVRO data format is invalid: expected ident at line 1 column 2'. The token ('not'), line and column are the variable parts drawn from the submitted definition; everything else quoted here is fixed text.

## Steps to Reproduce
All five are reproduced by replaying the glue wire corpus (trace set simparity-wire-aws-v1) against the emulator.

Cause 1/2: replay glue-gjwr-traj-start-the-copied-job-fails-before-provisioning, steps task#4 and task#5 — CreateJob with a Spark command and an execution role that does not grant Glue assume-role permission, StartJobRun, then GetJobRun(JobName, RunId) and GetJobRuns(JobName) once the run is terminal, and read JobRun.ErrorMessage / ExecutionArguments / ExecutionTime.

Cause 3: replay glue-classifier-custom-entity-and-data-quality-ruleset, step task#11 — CreateDataQualityRuleset(Name, Ruleset, TargetTable) and inspect the returned Identifier's form.

Cause 4: replay glue-workflow-trigger-lifecycle, step task#7 — create a workflow plus triggers/jobs, then GetWorkflow(Name, IncludeGraph=true) and inspect Workflow.Graph.Nodes[*].UniqueId and Workflow.Graph.Edges[0].SourceId/DestinationId.

Cause 5: replay glue-schema-registry-versioning, step task#8 — CreateRegistry/CreateSchema with DataFormat AVRO, then RegisterSchemaVersion with a SchemaDefinition that is not valid JSON (one beginning with a bare token such as `not`) and read the InvalidInputException Message.
