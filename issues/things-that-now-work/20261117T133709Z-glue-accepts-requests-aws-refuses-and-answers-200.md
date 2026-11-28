Issue-Source: trace-sweep-divergence
Issue-Trace: glue-get-dataflow-graph
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-get-dataflow-graph glue-get-plan-script-generation glue-table-schema-mapping-suggestion glue-usage-profile-update-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDataflowGraph|4xx->2xx|$.Message
Issue-Key: glue|GetDataflowGraph|4xx->2xx|$.__type
Issue-Key: glue|GetDataflowGraph|4xx->2xx|status
Issue-Key: glue|GetPlan|4xx->2xx|$.Message
Issue-Key: glue|GetPlan|4xx->2xx|$.__type
Issue-Key: glue|GetPlan|4xx->2xx|status
Issue-Key: glue|GetMapping|4xx->2xx|$.Message
Issue-Key: glue|GetMapping|4xx->2xx|$.__type
Issue-Key: glue|GetMapping|4xx->2xx|status
Issue-Key: glue|CreateUsageProfile|4xx->2xx|$.Message
Issue-Key: glue|CreateUsageProfile|4xx->2xx|$.__type
Issue-Key: glue|CreateUsageProfile|4xx->2xx|status
Issue-Key: glue|GetUsageProfile|4xx->2xx|$.Message
Issue-Key: glue|GetUsageProfile|4xx->2xx|$.__type
Issue-Key: glue|GetUsageProfile|4xx->2xx|status
Issue-Commit: b8e5c8616008a33e0a34d66e8eacd10130426f37

# Glue accepts requests AWS refuses and answers 200 where AWS returns 400

> _Note: this issue was identified against the code base at commit `b8e5c8616008a33e0a34d66e8eacd10130426f37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataflowGraph

## Description
Five Glue requests that AWS refuses with 400 are answered 200 by the emulator, with no error body at all. A refusal that never happens is the most damaging class of divergence: the client proceeds on a success it would never get from AWS.

Cause 1 of 5: GetDataflowGraph with a PythonScript that does not parse.
AWS: Smithy: GetDataflowGraph input PythonScript; modeled errors InvalidInputException (httpError 400), InternalServiceException, OperationTimeoutException. Recorded response: 400 __type 'InvalidInputException' with a Message naming the parse failure. The message is AWS's parser diagnostic: fixed text of the form 'line <l>:<c> no viable alternative at input '<the offending source fragment>'' — variable parts are the line, the column and the fragment of the submitted script.
Emulator: 200 with a DagNodes/DagEdges body and no error.
Why: a caller generating a job graph from a script gets a bogus graph instead of a parse error, so the bad script is persisted downstream.
Fix: the GetDataflowGraph handler's script parsing (it must fail the request when the annotated script does not parse).
Traces: glue-get-dataflow-graph#task#2.

Cause 2 of 5: GetPlan with neither Sinks nor Location.
AWS: Smithy: GetPlan input Mapping and Source @required, Sinks and Location both optional in the model, so the either/or rule is enforced by the service; InvalidInputException (400) is modeled. Recorded: 400 InvalidInputException, Message 'Either Sinks or Location must be non-empty.' (fixed text, no request-derived part).
Emulator: 200 with a generated PythonScript.
Fix: the GetPlan handler's validation.
Traces: glue-get-plan-script-generation#task#4.

Cause 3 of 5: GetMapping with neither Sinks nor Location.
AWS: Smithy: GetMapping input Source @required, Sinks/Location optional; InvalidInputException (400) modeled. Recorded: 400 InvalidInputException, Message 'Either Sinks or Location must be non-empty.' — the same fixed sentence as cause 2.
Emulator: 200 with a Mapping body.
Fix: the GetMapping handler's validation (shares the rule with GetPlan).
Traces: glue-table-schema-mapping-suggestion#task#4.

Cause 4 of 5: CreateUsageProfile whose Configuration sets a default worker type without a default number of workers (or vice versa).
AWS: Smithy: CreateUsageProfile input Name and Configuration @required; InvalidInputException (400) modeled. Recorded: 400 InvalidInputException, Message 'Please set default value for both Worker type and Number of workers in jobs configuration, if you intend to set default for either parameter.' (fixed text).
Emulator: 200 — the profile is created.
Why: the invalid profile is then readable and usable, and the client never learns the pairing rule.
Fix: the CreateUsageProfile handler's ProfileConfiguration validation.
Traces: glue-usage-profile-update-lifecycle#task#1.

Cause 5 of 5: GetUsageProfile for a profile that does not exist.
AWS: Smithy: GetUsageProfile input Name @required; errors EntityNotFoundException (400), InvalidInputException, OperationNotSupportedException. Recorded: 400 __type 'EntityNotFoundException', Message 'UsageProfile <Name> does not exists' — fixed text 'UsageProfile ' + ' does not exists' (AWS's wording, including the non-standard 'does not exists'), variable part the requested Name.
Emulator: 200 for a name that was never created.
Why: an absent profile reads as present, so existence checks and create-if-missing logic both break.
Fix: the GetUsageProfile handler's state lookup and not-found path.
Traces: glue-usage-profile-update-lifecycle#task#3.

An open issue (20261102T132146Z-glue-answers-200-for-absent-entities-and-an.md) covers the same family for GetDatabase/GetConnection/GetJob/CreateJob; it does not carry the replay keys for the five operations above.

## Evidence
1. glue-get-dataflow-graph#task#2 GetDataflowGraph — status expected 400, got 200; $.__type expected 'InvalidInputException', got absent; $.Message expected "line 2:35 no viable alternative at input '## @type: DataSource\n## @args: [database = \"example_db\" table_name'", got absent.
2. glue-get-plan-script-generation#task#4 GetPlan — status expected 400, got 200; expected InvalidInputException / 'Either Sinks or Location must be non-empty.', got no error body.
3. glue-table-schema-mapping-suggestion#task#4 GetMapping — status expected 400, got 200; expected InvalidInputException / 'Either Sinks or Location must be non-empty.', got no error body.
4. glue-usage-profile-update-lifecycle#task#1 CreateUsageProfile — status expected 400, got 200; expected InvalidInputException / 'Please set default value for both Worker type and Number of workers in jobs configuration, if you intend to set default for either parameter.', got no error body.
5. glue-usage-profile-update-lifecycle#task#3 GetUsageProfile — status expected 400, got 200; expected EntityNotFoundException / 'UsageProfile <name> does not exists', got no error body.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus).
2. Call GetDataflowGraph with a PythonScript whose '## @args:' annotation is syntactically broken — the emulator answers 200 with a graph.
3. Call GetPlan (with Mapping+Source) and GetMapping (with Source) and omit both Sinks and Location — both answer 200.
4. Call CreateUsageProfile with a Configuration that defaults only the worker type — it answers 200 and the profile is created.
5. Call GetUsageProfile with a name that was never created — it answers 200.
