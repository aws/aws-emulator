Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connect.instance-tag-reserved-aws-prefix-rejected-whole-request
Issue-Shape-Id: com.amazonaws.connect#AmazonConnectService
Issue-Key: connect.instance-tag-reserved-aws-prefix-rejected-whole-request|_refusal#13|AssertionError|raise AssertionError(f"expected the request to be refused, but it succeeded: {result!r}")
Issue-Commit: 87d71c43287cb88d437f1b22d9a50457d3db170d

# connect TagResource accepts a reserved aws: tag key — the request answers HTTP 200 and stores the key instead of being refused BadRequestException

> _Note: this issue was identified against the code base at commit `87d71c43287cb88d437f1b22d9a50457d3db170d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause in the Amazon Connect tagging plane.

Cause 1 of 1: TagResource does not enforce the reserved-prefix rule on tag keys, so a tag map holding a key that starts with the reserved `aws:` prefix is accepted and merged into the resource's tag set, together with every other key in the same request. Failing check: connect.instance-tag-reserved-aws-prefix-rejected-whole-request.

AWS: the Connect model constrains every tag key through `com.amazonaws.connect#TagKey`, whose pattern is `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$` — a negative lookahead that excludes any key beginning with the literal `aws:`. `TagMap` reaches that same shape as its key (Smithy: TagResource.tags -> TagMap, key -> TagKey, @pattern `^(?!aws:)[...]*$`, @length 1..128; map @length 1..50). AWS refuses such a request outright and applies NONE of its keys. Recorded behavior for the refusal class: HTTP 400 with error code `BadRequestException` and the fixed message `Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)` — that whole sentence is fixed text, with no part taken from the request (the offending key is not echoed), and the rule is case-insensitive, so `AWS:`/`Aws:` are refused the same way. `BadRequestException` is not in TagResource's modeled error list (modeled: InternalServiceException 500, InvalidParameterException 400, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429), so the error shape itself is recorded behavior only; the modeled `@pattern` is what makes the key illegal. A key that merely begins with the letters `aws` but is not the `aws:` prefix — e.g. `awsx:stage` — satisfies the pattern and must be accepted.

Emulator: TagResource on an ACTIVE instance's ARN with `tags={"sim2real-companion": "...", "aws:stage": "refused"}` answers HTTP 200 with an empty body (`{'ResponseMetadata': {'HTTPStatusCode': 200, ...'content-length': '0'}}`) and merges both keys into the instance's tag set. No refusal of any kind is produced. The handler's `handle` body resolves the ARN, checks only the per-resource tag-count ceiling, then does `tags.insert(key, value)` for every entry — there is no key-pattern check, and the centralized request walk is not applying the `TagKey` @pattern either.

Why: a client that relies on AWS's behavior gets two wrong answers. First, a request AWS rejects reports success, so code that asserts the reserved AWS tag namespace is protected (tagging UIs, policy/compliance gates, IaC drift detectors that treat `aws:`-prefixed tags as system-owned) sees its guard silently pass. Second, the refusal is all-or-nothing on AWS: the legal companion key in the same map must NOT be stored. Here both keys land, so the resource's tag set and every later read of it (ListTagsForResource, DescribeInstance.Tags) diverges from AWS, and a caller that retries the request without the reserved key cannot distinguish "nothing was applied" from "everything was applied".

Fix: belongs in TagResource's input validation — the per-key pattern check must run before any mutation of the stored tag map, and must refuse the whole request without applying any key (so the validation precedes the merge loop, not inside it). The same rule applies to any other entry point that writes a caller-supplied tag map on this service (CreateInstance.Tags and the other taggable creates) since they reach the same `TagKey` shape.

Traces: connect.instance-tag-reserved-aws-prefix-rejected-whole-request — the check tags the instance with the legal neighbour key `awsx:stage` (accepted and read back correctly), then issues the two-key request containing `aws:stage` and asserts it is refused; the assertion fails at `_refusal` with "expected the request to be refused, but it succeeded". Handler: services/service-com-amazonaws-connect-amazonconnectservice/src/ops/tag_resource.rs lines 175-231 (the merge loop at lines 224-226 is reached with no key-pattern gate; the only pre-check, lines 215-222, is the tag-count ceiling).

## Evidence
Expected (AWS): TagResource(resourceArn=<instance ARN>, tags={"sim2real-companion":"refused-with-it","aws:stage":"refused"}) -> HTTP 400, Error.Code = "BadRequestException", Error.Message = "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)" (entire message is fixed text). No key from the request is applied: ListTagsForResource and DescribeInstance.Tags are unchanged from before the call, in particular they contain neither "aws:stage" nor "sim2real-companion".

Actual (emulator): HTTP 200 with an empty body — {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0'}, 'RetryAttempts': 0}}. Both keys are merged into the instance's stored tags.

Assertion from the replay: File "check_connect_instance_tag_reserved_aws_prefix_rejected_whole_request.py", line 93, in _refusal -> AssertionError: expected the request to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Model evidence: smithy_query(shape="TagKey") -> {"type":"string","length":{"min":1,"max":128},"pattern":"^(?!aws:)[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*$"}; TagMap.key targets TagKey. The control key "awsx:stage" matches the pattern and is correctly accepted, confirming the divergence is the `aws:` prefix rule alone and not tag writing in general.

## Steps to Reproduce
1. connect.CreateInstance(IdentityManagementType="CONNECT_MANAGED", InstanceAlias=<unique alias>, InboundCallsEnabled=True, OutboundCallsEnabled=False, Tags={"sim2real":"instance-tag-prefix"}); note Id and Arn.
2. Poll connect.DescribeInstance(InstanceId=<Id>) until InstanceStatus == "ACTIVE".
3. connect.TagResource(resourceArn=<Arn>, tags={"awsx:stage":"control"}) — succeeds, and ListTagsForResource shows awsx:stage=control (correct: the key is not the reserved prefix).
4. connect.TagResource(resourceArn=<Arn>, tags={"sim2real-companion":"refused-with-it", "aws:stage":"refused"}).
   Expected: HTTP 400 BadRequestException "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)".
   Observed: HTTP 200, empty body.
5. connect.ListTagsForResource(resourceArn=<Arn>) — observed tag set now contains both "aws:stage" and "sim2real-companion"; AWS leaves it exactly as it was after step 3. connect.DescribeInstance(InstanceId=<Id>).Instance.Tags shows the same divergence.
6. Cleanup: connect.DeleteInstance(InstanceId=<Id>).
