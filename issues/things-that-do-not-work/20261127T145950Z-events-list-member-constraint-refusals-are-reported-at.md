Issue-Source: trace-sweep-divergence
Issue-Trace: events-remove-targets-id-length-bounds-rejected
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-remove-targets-id-length-bounds-rejected events-remove-targets-id-pattern-refuses-whole-request events-start-replay-filter-arn-member-length-bound untag-resource-tag-key-length-bounds create-connection-header-parameters-count-bound events-create-connection-body-parameters-count-bound
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|RemoveTargets|4xx->4xx|$.message
Issue-Key: events|StartReplay|4xx->4xx|$.message
Issue-Key: events|UntagResource|4xx->4xx|$.message
Issue-Key: events|CreateConnection|4xx->4xx|$.message
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: list-member constraint refusals are reported at the element path instead of AWS's collection-level message

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemoveTargets

## Description
4 replayed divergence clusters, 2 root causes, all 400 ValidationException bodies whose status and code already match.

Cause 1 of 2: a constraint violated by an element of a LIST is reported against the element, with one clause, instead of against the collection with the compound constraint.
AWS: reports ONE error at the collection's member path, echoes the whole collection in brackets, and lists every constraint of the element shape inside a 'Member must satisfy constraint: [ ... ]' clause:
  RemoveTargets: "1 validation error detected: Value '[<id>]' at 'ids' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+]" (Smithy: RemoveTargets.Ids -> TargetIdList @length 1..100, member TargetId @length 1..64 @pattern ^[\\.\\-_A-Za-z0-9]+$);
  StartReplay: "... Value '[<arn>]' at 'destination.filterArns' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1600, Member must have length greater than or equal to 1]" (Smithy: ReplayDestination.FilterArns -> ReplayDestinationFilters, member Arn @length 1..1600);
  UntagResource: "... Value '[<key>]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]" (Smithy: TagKeyList member TagKey @length 1..128).
The fixed text is the '<n> validation error(s) detected: ', 'Value '[...]' at '<collection path>' failed to satisfy constraint: Member must satisfy constraint: [...]' template; the bracketed value, the member path and the bounds/pattern come from the request and the model. ValidationException itself is unmodelled for this service, so the wording is recorded behavior.
Emulator: reports the element path with a single clause - "1 validation error detected: Value '<id>' at 'ids.1.member' failed to satisfy constraint: Member must have length less than or equal to 64" - dropping the brackets around the collection value, naming 'ids.1.member' instead of 'ids', and omitting the other constraints of the element shape. Same on 'destination.filterArns' and 'tagKeys'.
Why: a client that matches or displays AWS's validation text is told the wrong member path, and is shown only one of the bounds, so an operator fixing a 65-character target id is never told the id must also match the name grammar. The open behavioral issue 'events__sqs: 6 causes - a deleted rule keeps delivering, four PutTargets constraint violations are accepted, and the collection-constraint and InputTemplate refusal messages are malformed' (20261127T115057Z-events-sqs-6-causes-a-deleted-rule-keeps.md) describes the same renderer from the behavioral corpus but carries none of these replay keys, so these wire steps are unexercised by it.
Fix: the shared constraint-message renderer in the events validation layer, for list and map members (collection path, bracketed collection value, compound 'Member must satisfy constraint: [...]' clause).
Traces: events-remove-targets-id-length-bounds-rejected#task#5 (RemoveTargets; cluster also covers #task#6 and events-remove-targets-id-pattern-refuses-whole-request#task#5), events-start-replay-filter-arn-member-length-bound#task#4 (StartReplay; also #task#5, #task#6), untag-resource-tag-key-length-bounds#task#3 (UntagResource; also #task#4).

Cause 2 of 2: the count-bound refusal for CreateConnection's header parameters does not render the offending list the way AWS does.
AWS: refuses an over-long ConnectionHttpParameters header list with HTTP 400 ValidationException whose value segment is the whole list rendered element by element in the service's structure form, with every secret value masked: "1 validation error detected: Value '[ConnectionHeaderParameter(key=X-Trace-000, value=*** REDACTED ***, isValueSecret=false), ConnectionHeaderParameter(key=X-Trace-001, value=*** REDACTED ***, isValueSecret=false), ...]' ..." - recorded behavior (CreateConnection's AuthParameters/InvocationHttpParameters carry the list; the count limit is a service limit the model does not state). The fixed text is the 'ConnectionHeaderParameter(key=<k>, value=*** REDACTED ***, isValueSecret=<bool>)' element template and the redaction marker; the keys and flags come from the request.
Emulator: answers 400 with a $.message that diverges from that text (the replay report truncates the actual value at the cluster's detail limit, so the full emulator sentence is not visible; re-replay this step to capture it).
Why: the body of this refusal is the only place a client learns which header parameters were rejected, and AWS's masking of the value is a confidentiality requirement - a message that reproduces values instead of '*** REDACTED ***' would leak the secret header values the connection was created with.
Fix: the CreateConnection validation message for the header/body/query parameter lists - element rendering plus value redaction, in the same renderer as Cause 1.
Traces: create-connection-header-parameters-count-bound#task#3 (cluster also covers events-create-connection-body-parameters-count-bound#task#3).

## Evidence
Cause 1 (replay detail, RemoveTargets, events-remove-targets-id-length-bounds-rejected#task#5, HTTP 400 both sides):
expected $.message = "1 validation error detected: Value '[sim2real-probe-events-rmti-t-...x]' at 'ids' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\\.\\-_A-Za-z0-9]+]"
got      $.message = "1 validation error detected: Value 'sim2real-probe-events-rmti-t-...x' at 'ids.1.member' failed to satisfy constraint: Member must have length less than or equal to 64"
UntagResource: expected "... Value '[sim2real-ttt...]' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]", got "... Value 'sim2real-ttt...' at 'tagKeys.1.member' failed to satisfy constraint: Member must have length less than or equal to 128".
StartReplay: expected "... at 'destination.filterArns' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1600, Member must have length greater than or equal to 1]", got the element-path/single-clause form.

Cause 2 (CreateConnection, create-connection-header-parameters-count-bound#task#3, HTTP 400 both sides): '$.message: value_mismatch (expected "1 validation error detected: Value \'[ConnectionHeaderParameter(key=X-Trace-000, value=*** REDACTED ***, isValueSecret=false), ...]\'", got <diverging text, truncated in the replay report>)'.

## Steps to Reproduce
1. RemoveTargets on an existing rule with Ids=['<65-character id>'] and compare $.message against AWS's collection form ('[...]' value at 'ids' with the compound constraint list).
2. UntagResource with TagKeys=['<129-character key>']; StartReplay with Destination.FilterArns=['<1601-character arn>'] - same comparison at 'tagKeys' / 'destination.filterArns'.
3. CreateConnection with more header parameters than the service accepts and compare the rendered list and the '*** REDACTED ***' masking in $.message.
4. Re-replay the fixtures listed under Traces.
