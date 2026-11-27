Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-parameter-group-name-length-boundary
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-parameter-group-name-length-boundary
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|CreateParameterGroup|2xx->4xx|$.ParameterGroup
Issue-Key: memorydb|CreateParameterGroup|2xx->4xx|status
Issue-Key: memorydb|DescribeParameterGroups|2xx->4xx|$.ParameterGroups
Issue-Key: memorydb|DescribeParameterGroups|2xx->4xx|status
Issue-Key: memorydb|DeleteParameterGroup|2xx->4xx|$.ParameterGroup
Issue-Key: memorydb|DeleteParameterGroup|2xx->4xx|status
Issue-Key: memorydb|CreateParameterGroup|4xx->4xx|$.message
Issue-Key: memorydb|DescribeParameterGroups|4xx->4xx|$.__type
Issue-Key: memorydb|DescribeParameterGroups|4xx->4xx|$.message
Issue-Commit: f91d45c7866d80628978681094afeabba34e454b

# ParameterGroupName length rejected at 40 characters instead of 255, with wrong over-limit error

> _Note: this issue was identified against the code base at commit `f91d45c7866d80628978681094afeabba34e454b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateParameterGroup

## Description
MemoryDB's parameter-group name validation in the emulator uses the wrong length boundary and the wrong rejection shape. The emulator caps ParameterGroupName at 39 characters ("must be less than 40"), so every legal name of 40 characters or more is refused, and the one name length AWS really does refuse (over 255 characters) is reported with the emulator's own 40-character message instead of AWS's InvalidParameterValue text. Five causes, all in one fixture that walks the name-length boundary.

Cause 1 of 5: CreateParameterGroup rejects a valid name of 40+ characters.
AWS: Smithy: CreateParameterGroup.ParameterGroupName targets `String` (required) with NO @length constraint, so the model states no 40-character ceiling; modeled client errors are InvalidParameterValueException / InvalidParameterCombinationException / ParameterGroupAlreadyExistsFault / ParameterGroupQuotaExceededFault, all httpError 400. Recorded behavior only: AWS accepts a parameter group name of 41 characters and answers HTTP 200 with a ParameterGroup object carrying Name, Family, Description and an ARN of the form arn:aws:memorydb:<region>:<account>:parametergroup/<name>.
Emulator: answers HTTP 400 with message "Parameter group name length must be less than 40." and no ParameterGroup member at all, so the group is never created.
Why: any client whose naming convention produces names of 40 characters or more (environment-prefixed or hash-suffixed names are routinely longer) cannot create a parameter group at all against the emulator, while the identical call succeeds on AWS. The failure is a hard 400, so provisioning aborts rather than degrading.
Fix: the CreateParameterGroup handler's input validation for ParameterGroupName — remove the 39-character ceiling and allow names up to the length AWS accepts (255).
Traces: memorydb-parameter-group-name-length-boundary#task#5 (also task#9).

Cause 2 of 5: DescribeParameterGroups cannot return a 40+ character parameter group.
AWS: Smithy: DescribeParameterGroups.ParameterGroupName targets `String` with no length constraint; output ParameterGroups is a ParameterGroupList; errors InvalidParameterValueException (400) and ParameterGroupNotFoundFault (404). Recorded behavior only: filtering by the 41-character name returns HTTP 200 and a one-element ParameterGroups list describing that group.
Emulator: answers HTTP 400 and omits ParameterGroups entirely, because the same 40-character name check is applied on the lookup path (and the group was never created, per cause 1).
Why: a client that reads back the parameter group it just created — the normal confirm-after-create step — gets a 400 instead of its description, so it cannot discover the group's Family or ARN.
Fix: the DescribeParameterGroups handler's validation/lookup path — a long but well-formed name must be looked up, not rejected.
Traces: memorydb-parameter-group-name-length-boundary#task#6 (also task#10).

Cause 3 of 5: DeleteParameterGroup cannot delete a 40+ character parameter group.
AWS: Smithy: DeleteParameterGroup.ParameterGroupName targets `String` (required), no length constraint; output ParameterGroup; errors InvalidParameterValueException (400), InvalidParameterGroupStateFault (400), ParameterGroupNotFoundFault (404). Recorded behavior only: deleting by the 41-character name answers HTTP 200 and echoes the deleted ParameterGroup object.
Emulator: answers HTTP 400 with no ParameterGroup member, applying the same sub-40 name rule.
Why: the resource is unreachable for teardown, so a client cannot clean up and a delete-then-recreate cycle never completes.
Fix: the DeleteParameterGroup handler's validation/lookup path — same ceiling removal as causes 1 and 2.
Traces: memorydb-parameter-group-name-length-boundary#task#7 (also task#11).

Cause 4 of 5: the over-255-character CreateParameterGroup rejection carries the wrong message.
AWS: recorded behavior only (the model imposes no @length, so the limit is observed, not modeled): a ParameterGroupName longer than 255 characters is refused with HTTP 400 and message "The parameter ParameterGroupName is not a valid identifier because it is longer than 255 characters." In that message the fixed text is "The parameter " ... " is not a valid identifier because it is longer than 255 characters."; the only variable part is the offending member name taken from the request (ParameterGroupName) — the limit 255 and the wording are fixed, and nothing in the message comes from a per-run value.
Emulator: returns HTTP 400 but with message "Parameter group name length must be less than 40." — different wording and a different, wrong limit.
Why: a client that matches on the error text to distinguish "name too long" from other validation faults mis-classifies the failure, and an operator reading the emulator's message is told the limit is 40 when AWS's is 255.
Fix: the CreateParameterGroup validation's error construction — emit InvalidParameterValue with AWS's identifier-too-long message template, parameterized by the member name, once the ceiling is corrected to 255.
Traces: memorydb-parameter-group-name-length-boundary#task#13.

Cause 5 of 5: DescribeParameterGroups answers an over-length name with the wrong error code.
AWS: Smithy: DescribeParameterGroups binds both InvalidParameterValueException (client, httpError 400) and ParameterGroupNotFoundFault (client, httpError 404). Recorded behavior only: a ParameterGroupName longer than 255 characters fails input validation before any lookup, so the answer is HTTP 400, __type "InvalidParameterValueException", message "The parameter ParameterGroupName is not a valid identifier because it is longer than 255 characters." (fixed text as in cause 4, with the member name from the request).
Emulator: answers __type "ParameterGroupNotFoundFault" with message "Parameter Group not found." — it skips length validation and falls through to a state lookup miss.
Why: validation faults and not-found faults mean opposite things to a client: InvalidParameterValue is non-retryable and names the bad input, while ParameterGroupNotFound invites create-then-retry logic that will keep failing. Retry and error-mapping code built against AWS takes the wrong branch.
Fix: the DescribeParameterGroups handler's validation ordering — validate ParameterGroupName length and raise InvalidParameterValue before consulting state, rather than letting an unvalidated name reach the lookup.
Traces: memorydb-parameter-group-name-length-boundary#task#14.

No open memorydb issue covers this: 20261104T053017Z-memorydb-createsubnetgroup-stub.md is the CreateSubnetGroup stub, 20261115T013411Z-memorydb-omitted-subnet-group-description-fabricated-tls-less.md covers subnet-group Description, TLS-less CreateCluster and RESP data, and 20261109T115509Z-cloudformation-memorydb-... covers CloudFormation-provisioned users. None mentions parameter-group name validation.

## Evidence
Fixture memorydb-parameter-group-name-length-boundary, name of 41 characters (sim2real-probe-mdbpgl-<suffix>):

- task#5 CreateParameterGroup — expected 200 with $.ParameterGroup = {ARN: arn:aws:memorydb:<region>:<account>:parametergroup/<name>, Description, Family: memorydb_valkey7, Name}; actual 400, $.ParameterGroup missing, message "Parameter group name length must be less than 40." (also task#9)
- task#6 DescribeParameterGroups — expected 200 with $.ParameterGroups = [<that group>]; actual 400, $.ParameterGroups missing (also task#10)
- task#7 DeleteParameterGroup — expected 200 with $.ParameterGroup = <that group>; actual 400, $.ParameterGroup missing (also task#11)

Name longer than 255 characters:

- task#13 CreateParameterGroup — expected 400 $.message "The parameter ParameterGroupName is not a valid identifier because it is longer than 255 characters."; actual 400 $.message "Parameter group name length must be less than 40."
- task#14 DescribeParameterGroups — expected 400 $.__type "InvalidParameterValueException", $.message "The parameter ParameterGroupName is not a valid identifier because it is longer than 255 characters."; actual 400 $.__type "ParameterGroupNotFoundFault", $.message "Parameter Group not found."

Smithy (com-amazonaws-memorydb-amazonmemorydb): CreateParameterGroup/DeleteParameterGroup/DescribeParameterGroups ParameterGroupName targets String with no @length trait; CreateParameterGroup errors include InvalidParameterValueException (httpError 400); DescribeParameterGroups and DeleteParameterGroup bind both InvalidParameterValueException (400) and ParameterGroupNotFoundFault (404).

## Steps to Reproduce
1. Replay fixture memorydb-parameter-group-name-length-boundary against the emulator (replay_wire_corpus).
2. Step task#5: CreateParameterGroup with a ParameterGroupName of 41 characters and Family memorydb_valkey7 — the emulator answers 400 "Parameter group name length must be less than 40." where AWS answers 200 with the ParameterGroup.
3. Steps task#6 / task#7: DescribeParameterGroups and DeleteParameterGroup for that same 41-character name — both 400 instead of 200 with the group echoed.
4. Step task#13: CreateParameterGroup with a ParameterGroupName longer than 255 characters — compare the returned $.message against "The parameter ParameterGroupName is not a valid identifier because it is longer than 255 characters.".
5. Step task#14: DescribeParameterGroups with that same over-255-character name — the emulator returns __type ParameterGroupNotFoundFault / "Parameter Group not found." instead of InvalidParameterValueException with the identifier-too-long message.
