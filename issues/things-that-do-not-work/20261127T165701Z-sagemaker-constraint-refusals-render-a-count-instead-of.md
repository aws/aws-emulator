Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-inference-experiment-stop-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-inference-experiment-stop-refusals sagemaker-search-filter-operator-and-count-refusals sagemaker-start-session-unknown-space-refusals sagemaker-pipeline-run-condition-dependency-errors
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|StopInferenceExperiment|4xx->4xx|$.message
Issue-Key: sagemaker|Search|4xx->4xx|$.message
Issue-Key: sagemaker|StartSession|4xx->4xx|$.message
Issue-Key: sagemaker|Search|4xx->2xx|$.__type
Issue-Key: sagemaker|Search|4xx->2xx|$.message
Issue-Key: sagemaker|Search|4xx->2xx|status
Issue-Key: sagemaker|CreatePipeline|4xx->4xx|$.message
Issue-Commit: 0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad

# sagemaker constraint refusals render a count instead of the offending value, echo the raw Smithy pattern, and miss two validations entirely

> _Note: this issue was identified against the code base at commit `0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopInferenceExperiment, Search, StartSession, CreatePipeline

## Description
Five independent causes in the emulated SageMaker request-validation layer. Three are wording defects in the "1 validation error detected" envelope (the offending value is summarised as a count, and a @pattern is echoed with its Smithy anchors), one is a validation that is missing altogether (an unsupported Search operator is served a 200), and one is a pipeline-definition validation that misdiagnoses the fault. Each is numbered and each must be closed before this issue is submitted.

=== Cause 1 of 5: StopInferenceExperiment renders an empty ModelVariantActions map as '0 entries' instead of the value itself ===
AWS: Smithy — StopInferenceExperiment.ModelVariantActions is @required, a map with @length min 1 max 2. Recorded behavior: an empty map is refused HTTP 400 with the constraint envelope "1 validation error detected: Value '{}' at 'modelVariantActions' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text: "1 validation error detected: Value '" ... "' at '" ... "' failed to satisfy constraint: Member must have length greater than or equal to " + the bound; the variable parts are the OFFENDING VALUE rendered as AWS renders it (an empty map prints as {}), the lowerCamel member path taken from the request, and the modeled @length min.
Emulator: the status, code and member path are right, but the value slot holds the emulator's own summary '0 entries' instead of the value.
Why: AWS's constraint envelope is parsed and surfaced by SDK/CLI users and by tooling that extracts the rejected value from it; a count where a value belongs makes that extraction fail and makes emulator-run tests assert a string AWS never sends.
Fix: the constraint-violation message formatter shared by the validation layer — a collection's value slot must be rendered as the value ({} for an empty map, the bracketed element list otherwise), not as a cardinality summary.
Traces: sagemaker-inference-experiment-stop-refusals#task#2 (cluster also covers task#3).

=== Cause 2 of 5: an over-long Search filter list is rendered as '21 items' instead of the list of Filter(...) values ===
AWS: Smithy — SearchExpression.Filters targets FilterList with @length min 1 max 20; Filter has members Name, Operator, Value. Recorded behavior: 21 filters are refused HTTP 400 with "1 validation error detected: Value '[Filter(name=<Name>, operator=<Operator>, value=<Value>), ...]' at 'searchExpression.filters' failed to satisfy constraint: Member must have length less than or equal to 20" — i.e. the value slot holds every submitted filter rendered as Filter(name=..., operator=..., value=...), comma-space separated inside square brackets, in request order; the fixed text is the envelope and "Member must have length less than or equal to " + the modeled max (20); the member path 'searchExpression.filters' is the lowerCamel request path.
Emulator: the value slot holds '21 items' (and the trailing constraint clause is likewise truncated in the recorded comparison).
Why: same as cause 1 — this is the same formatter defect reached through a list rather than a map, and it is the case where the lost information matters most: the caller cannot see WHICH filters were submitted.
Fix: the same constraint-violation formatter, list rendering branch, plus the per-shape value renderer for Filter (lowerCamel member names inside Filter(...)).
Traces: sagemaker-search-filter-operator-and-count-refusals#task#2.

=== Cause 3 of 5: StartSession's ResourceIdentifier pattern violation echoes the Smithy regex verbatim (leading ^ kept, trailing .* missing) ===
AWS: Smithy — StartSession.ResourceIdentifier is @required, @length 1..256, @pattern ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:.*\\/ . Recorded behavior: a bare name is refused HTTP 400 with "1 validation error detected: Value '<the submitted value>' at 'resourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:.*\\/.*" — AWS prints the pattern in its unanchored service form: no leading ^, and the trailing segment completed with .* . Fixed text is the envelope plus "Member must satisfy regular expression pattern: "; the variable parts are the submitted value, the lowerCamel member path, and the pattern as the service words it.
Emulator: prints '^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:.*\\/' — the Smithy trait string copied straight through, keeping the ^ anchor and dropping the trailing .* .
Why: callers and test suites compare this message exactly, and a user reading it is shown a regex that is not the one the service documents. It is also a systemic defect: the same raw-trait echo is already filed for other members (20261127T123223Z-sagemaker-secretsmanager-the-gitconfig-secretarn-pattern-violation-message.md for GitConfig.SecretArn and 20261031T024432Z-sagemaker-runtime-validationerror-constraint-message-renders-the-member.md for sagemaker-runtime) — neither of those issues names StartSession.ResourceIdentifier, so this occurrence is unfixed and must be closed here.
Fix: the pattern-violation branch of the constraint-violation formatter — strip the Smithy anchors (^ and $) and render the service-form pattern when emitting "Member must satisfy regular expression pattern:".
Traces: sagemaker-start-session-unknown-space-refusals#task#2.

=== Cause 4 of 5: Search accepts an operator that is unsupported for a DATETIME property and answers 200 instead of refusing it ===
AWS: Smithy — Filter.Operator enum {Equals, NotEquals, GreaterThan, GreaterThanOrEqualTo, LessThan, LessThanOrEqualTo, Contains, Exists, NotExists, In}; the Operator documentation states Contains/In are "Only supported for text properties" and the relational operators are "Not supported for text properties", i.e. operator support is per property type. Recorded behavior: Search with a filter {Name:'CreationTime', Operator:'Contains', Value:...} is refused HTTP 400 with __type ValidationException and message "Operation Contains is unsupported for CreationTime property of type DATETIME. Supported Operations are [Equals, NotEquals, LessThan, LessThanOrEqualTo, GreaterThan, GreaterThanOrEqualTo, Exists, NotExists]." — fixed text "Operation " X " is unsupported for " Y " property of type " T ". Supported Operations are [" ... "].", where X is the request's Operator, Y the request's filter Name, T the property's indexed type and the bracketed list the operators that type does support, in that order.
Emulator: no refusal at all — HTTP 200 with a normal (empty) result body, no __type and no message.
Why: a silent-wrong-answer class. A client that mis-chooses an operator for a timestamp property is told "nothing matched" and concludes the resources do not exist, instead of learning the filter was never applicable. This is the same "Search applies no type rules" family as cause 3 of open issue 20261127T123223Z-sagemaker-6-net-new-causes-runtime-free-condition.md (a malformed CreationTime VALUE is accepted); that issue does not cover operator/property-type compatibility, which is this cause.
Fix: the Search handler's filter validation — resolve each filter's property to its indexed type and refuse an operator the type does not support with the ValidationException message above, before the index is consulted.
Traces: sagemaker-search-filter-operator-and-count-refusals#task#4.

=== Cause 5 of 5: CreatePipeline misdiagnoses a DependsOn edge that points at a FailStep as an unknown step name ===
AWS: Smithy — CreatePipeline.PipelineDefinition (@length 1..1048576) is validated server-side; the definition grammar's step kinds include Fail steps, and a step's DependsOn may not name one. Recorded behavior: a definition in which step <A> declares DependsOn <B> where <B> is a Fail step is refused HTTP 400 with message "Step: <A> can not depends on FailStep: <B>" — fixed text "Step: ", " can not depends on FailStep: " (AWS's wording, including the non-agreeing "depends"), with the two step names taken from the submitted definition.
Emulator: 'step <A> declares a DependsOn entry that is not a step name' — the status is right but the diagnosis is wrong: the emulator does not recognise the Fail step as a declared step at all, so it reports a dangling reference instead of the forbidden dependency, and the message is emulator-authored.
Why: the author of a pipeline definition is told their step name is misspelled when in fact the edge kind is illegal, so the definition cannot be repaired from the error; and any tool keying on AWS's wording never matches. It also indicates the definition parser's step table omits Fail steps, which affects every other rule that walks the step graph.
Fix: the CreatePipeline pipeline-definition validator — include Fail steps in the step table and emit the FailStep-dependency refusal with AWS's sentence when a DependsOn edge targets one.
Traces: sagemaker-pipeline-run-condition-dependency-errors#task#2 (cluster also covers task#3, task#4, task#5, task#6 of the same fixture).

## Evidence
Replay of the wire corpus (trace set simparity-wire-aws-v1) against the booted emulator, verbatim from the divergence clusters:

Cause 1 — fixture sagemaker-inference-experiment-stop-refusals, step task#2 (cluster also task#3), StopInferenceExperiment, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '{}' at 'modelVariantActions' failed to satisfy constraint: Member must have length greater than or equal to 1", got "1 validation error detected: Value '0 entries' at 'modelVariantActions' failed to satisfy constraint: Member must have length greater than or equal to 1")

Cause 2 — fixture sagemaker-search-filter-operator-and-count-refusals, step task#2, Search, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '[Filter(name=ExperimentName, operator=Equals, value=<n>), Filter(name=ExperimentName, operator=NotEquals, value=<n-00>), ... 21 filters in request order ...]' at 'searchExpression.filters' failed to satisfy constraint: Member must have length less than or equal to 20", got "1 validation error detected: Value '21 items' at 'searchExpression.filters' failed to satisfy constraint: Member must have length less than ")
  (the filter names/values are the per-run probe values from the request; the requirement is that each submitted filter is rendered as Filter(name=..., operator=..., value=...) in the value slot and that the clause ends "less than or equal to 20".)

Cause 3 — fixture sagemaker-start-session-unknown-space-refusals, step task#2, StartSession, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '<submitted value>' at 'resourceIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:.*\\/.*", got "... Member must satisfy regular expression pattern: ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:.*\\/")

Cause 4 — fixture sagemaker-search-filter-operator-and-count-refusals, step task#4, Search, expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'ValidationException', got None)
  $.message: missing (expected 'Operation Contains is unsupported for CreationTime property of type DATETIME. Supported Operations are [Equals, NotEquals, LessThan, LessThanOrEqualTo, GreaterThan, GreaterThanOrEqualTo, Exists, NotExists].', got None)

Cause 5 — fixture sagemaker-pipeline-run-condition-dependency-errors, step task#2, CreatePipeline, expected 400 / actual 400 (cluster size 5: task#2..task#6):
  $.message: value_mismatch (expected 'Step: Mop can not depends on FailStep: Halt', got 'step Mop declares a DependsOn entry that is not a step name')
  (Mop/Halt are the step names from the submitted definition; the sentence template around them is the requirement.)

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it (region us-west-2):

Cause 1: create an inference experiment, then sm.stop_inference_experiment(Name=<it>, ModelVariantActions={}). Observe the value slot '0 entries' instead of '{}' (fixture sagemaker-inference-experiment-stop-refusals step task#2).

Cause 2: sm.search(Resource='Experiment', SearchExpression={'Filters':[21 filters on ExperimentName]}). Observe the value slot '21 items' instead of the bracketed Filter(...) list, and the truncated "Member must have length less than " clause (fixture sagemaker-search-filter-operator-and-count-refusals step task#2).

Cause 3: sm.start_session(ResourceIdentifier=<a bare space name, not an ARN>). Observe the echoed pattern keeps its leading ^ and lacks the trailing .* (fixture sagemaker-start-session-unknown-space-refusals step task#2).

Cause 4: sm.search(Resource='Experiment', SearchExpression={'Filters':[{'Name':'CreationTime','Operator':'Contains','Value':<any>}]}). Observe HTTP 200 with a result body instead of 400 ValidationException with the "Operation Contains is unsupported for CreationTime property of type DATETIME..." message (fixture sagemaker-search-filter-operator-and-count-refusals step task#4).

Cause 5: sm.create_pipeline with a definition containing a Fail step (e.g. {"Name":"Halt","Type":"Fail",...}) and another step declaring DependsOn ["Halt"]. Observe 'step Mop declares a DependsOn entry that is not a step name' instead of 'Step: Mop can not depends on FailStep: Halt' (fixture sagemaker-pipeline-run-condition-dependency-errors step task#2).
