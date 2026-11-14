Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-dsw-cloudfront-scope-on-regional-endpoint
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-dsw-cloudfront-scope-on-regional-endpoint wafv2-dsw-get-web-acl-arn-route-mistakes wafv2-dsw-get-web-acl-identifier-mistakes wafv2-dsw-scope-enum-and-missing-scope wafv2-dsw-list-web-acls-paging-bounds
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|UpdateWebACL|4xx->4xx|$.Field
Issue-Key: wafv2|UpdateWebACL|4xx->4xx|$.Parameter
Issue-Key: wafv2|UpdateWebACL|4xx->4xx|$.Reason
Issue-Key: wafv2|UpdateWebACL|4xx->4xx|$.__type
Issue-Key: wafv2|UpdateWebACL|4xx->4xx|$.message
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.Field
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.Parameter
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.Reason
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.__type
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.message
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.Message
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.message
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.Message
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.__type
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.Field
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.Parameter
Issue-Key: wafv2|ListWebACLs|4xx->4xx|$.Reason
Issue-Commit: 5ba72cc05e802971fb04990ddf25d58f3354550b

# wafv2 scope and identifier validation refusals carry the wrong exception class and message

> _Note: this issue was identified against the code base at commit `5ba72cc05e802971fb04990ddf25d58f3354550b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebACL

## Description
Six net-new root causes, all in the wafv2 request-validation / refusal-mapping layer that runs in front of the WebACL operations: a request that WAF refuses at parameter-validation time is instead routed through lookup (and so refused as a nonexistent item), or refused with the generic Smithy constraint reporter, so the HTTP 400 carries the wrong `__type`, the wrong message, and none of WAFInvalidParameterException's `Field` / `Parameter` / `Reason` detail members. These are distinct from the already-open issues `20261029T192556Z-wafv2-refusals-carry-the-wrong-exception-class-sentence.md`, `20261030T002309Z-wafv2-refusals-carry-the-wrong-class-or-not.md` and `20261029T192556Z-wafv2-request-validation-never-refuses-ten-operations-answer.md`, neither of which carries the replay keys below; the replay report marked these six clusters as owned by nothing. All six must be closed before submitting.

Cause 1 of 6: UpdateWebACL with Scope=CLOUDFRONT against a regional endpoint.
AWS: Smithy: UpdateWebACL errors include WAFInvalidParameterException (client, HTTP 400) with members `Field` (ParameterExceptionField enum, which contains SCOPE_VALUE), `Parameter`, `Reason`, `message`; CLOUDFRONT scope is only usable in us-east-1. Recorded response: HTTP 400, `__type` WAFInvalidParameterException, `Field` SCOPE_VALUE, `Parameter` CLOUDFRONT, `Reason` "The scope is not valid.", `message` "Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: CLOUDFRONT". Fixed text of that message is "Error reason: ", ", field: ", ", parameter: "; the variable parts are the reason sentence, the field token and the offending scope value from the request.
Emulator: HTTP 400 but `__type` WAFNonexistentItemException with message "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.", and no `Field` / `Parameter` / `Reason` members at all.
Why: a client that branches on WAFInvalidParameterException to correct the scope (or reads `Field`/`Parameter` to report which input was wrong) sees a not-found error instead and concludes the web ACL was deleted, retrying or recreating it rather than fixing the scope.
Fix: the scope precondition must be checked in UpdateWebACL's input validation, before state lookup, and the refusal built by the WAFInvalidParameterException serializer so the detail members are emitted.
Traces: wafv2-dsw-cloudfront-scope-on-regional-endpoint#task#2.

Cause 2 of 6: GetWebACL with Scope=CLOUDFRONT against a regional endpoint.
AWS: Same as cause 1 — Smithy: GetWebACL errors include WAFInvalidParameterException (client, 400). Recorded: 400, `__type` WAFInvalidParameterException, `Field` SCOPE_VALUE, `Parameter` CLOUDFRONT, `Reason` "The scope is not valid.", `message` "Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: CLOUDFRONT".
Emulator: 400 with `__type` WAFNonexistentItemException and WAF's resource-doesn't-exist sentence, no detail members.
Why: identical misclassification on the read path, so a caller cannot tell an invalid-scope request from a missing web ACL.
Fix: GetWebACL's input validation (same scope precondition as cause 1, shared validator) plus the WAFInvalidParameterException serializer.
Traces: wafv2-dsw-cloudfront-scope-on-regional-endpoint#task#3, wafv2-dsw-get-web-acl-arn-route-mistakes#task#2.

Cause 3 of 6: GetWebACL given an ARN that names a resource of another type (an IPSet ARN).
AWS: Smithy: GetWebACL.ARN targets ResourceArn (length 20..2048, pattern \\S); WAFInvalidParameterException's doc states "Your request references an ARN that is malformed, or corresponds to a resource with which a web ACL can't be associated." Recorded behavior for this shape of request: HTTP 400 with `__type` AccessDeniedException and `Message` "Invalid ARN provided for GetWebACLRequest <arn>" — fixed text "Invalid ARN provided for GetWebACLRequest " followed by the ARN exactly as supplied in the request (AccessDeniedException is not in the operation's modeled error list, so this class is recorded behavior only).
Emulator: HTTP 400 with `__type` WAFNonexistentItemException and `Message` "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.".
Why: an SDK that treats AccessDeniedException as non-retryable/credential-or-input-shaped sees a WAF not-found instead; tooling that validates ARNs by probing gets "deleted" for "wrong resource type" and silently mis-routes.
Fix: GetWebACL's identifier/ARN validation (reject a cross-type or malformed ARN before state lookup) and the error serializer that chooses the refusal class.
Traces: wafv2-dsw-get-web-acl-arn-route-mistakes#task#4, wafv2-dsw-get-web-acl-identifier-mistakes#task#4, wafv2-dsw-get-web-acl-identifier-mistakes#task#5, wafv2-dsw-scope-enum-and-missing-scope#task#6.

Cause 4 of 6: ListWebACLs with NextMarker="" — constraint-violation message text and clause order.
AWS: Smithy: ListWebACLs.NextMarker targets NextMarker with @length min 1 max 256 and @pattern \\S, so an empty marker violates both constraints. Recorded: HTTP 400 whose message reports the PATTERN clause first and renders the pattern as `.*\\S.*`: "2 validation errors detected: Value '' at 'nextMarker' failed to satisfy constraint: Member must satisfy regular expression pattern: .*\\S.*; Value '' at 'nextMarker' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text is the "<n> validation errors detected: " prefix and the two "Value '<v>' at '<member>' failed to satisfy constraint: ..." clauses joined by "; "; the variable parts are the count, the rejected value, the member name, and the limit.
Emulator: same status and same two clauses, but ordered LENGTH first and the pattern rendered bare as `\\S`: "2 validation errors detected: Value '' at 'nextMarker' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'nextMarker' failed to satisfy constraint: Member must satisfy regular expression pattern: \\S".
Why: clients and test suites that assert on or parse the validation message (a very common pattern for pagination-marker handling) do not match, and the reported regex is not the one AWS advertises, so a caller cannot reproduce the rule it must satisfy.
Fix: the shared constraint-validation reporter for wafv2 input members — clause ordering (pattern before length) and the pattern rendering used for `\\S`-style string patterns.
Traces: wafv2-dsw-list-web-acls-paging-bounds#task#6.

Cause 5 of 6: ListWebACLs with Scope omitted entirely.
AWS: Smithy: ListWebACLs.Scope is @required. Recorded behavior only (the class is not in the operation's modeled error list): HTTP 400 with `__type` AccessDeniedException and `Message` "Critical information is missing in your request: ListWebACLsRequest(scope=null, nextMarker=null, limit=null)." — fixed text "Critical information is missing in your request: " followed by the request shape name and its member=value rendering, with absent members rendered `null`.
Emulator: HTTP 400 with `__type` ValidationException and no `Message` member at all.
Why: the error class is one the AWS SDKs model differently (ValidationException is not a wafv2 error class), and the absent `Message` leaves the caller with a 400 carrying no diagnosis, so a missing required field surfaces as an unidentifiable failure.
Fix: ListWebACLs' missing-required-member handling and the error serializer that selects the refusal class and emits `Message`.
Traces: wafv2-dsw-scope-enum-and-missing-scope#task#3.

Cause 6 of 6: ListWebACLs with Scope=GLOBAL (a value outside the Scope enum).
AWS: Smithy: Scope is an enum whose values are CLOUDFRONT and REGIONAL; ListWebACLs models WAFInvalidParameterException (client, 400). Recorded: HTTP 400, `__type` WAFInvalidParameterException, `Field` SCOPE_VALUE, `Parameter` GLOBAL, `Reason` "The scope is not valid.", `message` "Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: GLOBAL" — i.e. WAF refuses an out-of-enum scope as an invalid WAF parameter, not as a generic enum-constraint violation, and echoes the supplied scope value in `Parameter`.
Emulator: HTTP 400 with `__type` ValidationException and message "1 validation error detected: Value 'GLOBAL' at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [CLOUDFRONT, REGIONAL]", with no `Field` / `Parameter` / `Reason`.
Why: callers catching WAFInvalidParameterException to retry with a corrected scope never see it, and the `Field`/`Parameter` members they use to point at the offending input are absent.
Fix: ListWebACLs' scope validation must raise the WAF parameter refusal instead of deferring to the generic enum-constraint reporter, and the WAFInvalidParameterException serializer must emit the detail members.
Traces: wafv2-dsw-scope-enum-and-missing-scope#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (46 fixtures, 253 steps, 199 passed, score 78.7%). Six clusters with no owning issue and no stub:

1) wafv2-dsw-cloudfront-scope-on-regional-endpoint#task#2 UpdateWebACL — status 400->400; expected __type 'WAFInvalidParameterException', got 'WAFNonexistentItemException'; Field 'SCOPE_VALUE' missing; Parameter 'CLOUDFRONT' missing; Reason 'The scope is not valid.' missing; message 'Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: CLOUDFRONT' missing.
2) wafv2-dsw-cloudfront-scope-on-regional-endpoint#task#3 GetWebACL — identical expected/actual pair (also wafv2-dsw-get-web-acl-arn-route-mistakes#task#2).
3) wafv2-dsw-get-web-acl-arn-route-mistakes#task#4 GetWebACL — expected __type 'AccessDeniedException' with Message 'Invalid ARN provided for GetWebACLRequest <ipset-arn>', got 'WAFNonexistentItemException' with 'AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.' (also wafv2-dsw-get-web-acl-identifier-mistakes#task#4, #task#5, wafv2-dsw-scope-enum-and-missing-scope#task#6).
4) wafv2-dsw-list-web-acls-paging-bounds#task#6 ListWebACLs — expected message "2 validation errors detected: Value '' at 'nextMarker' failed to satisfy constraint: Member must satisfy regular expression pattern: .*\\S.*; Value '' at 'nextMarker' failed to satisfy constraint: Member must have length greater than or equal to 1"; got the same two clauses in the opposite order with the pattern rendered as "\\S".
5) wafv2-dsw-scope-enum-and-missing-scope#task#3 ListWebACLs — expected __type 'AccessDeniedException', Message 'Critical information is missing in your request: ListWebACLsRequest(scope=null, nextMarker=null, limit=null).'; got __type 'ValidationException' with no Message.
6) wafv2-dsw-scope-enum-and-missing-scope#task#4 ListWebACLs — expected __type 'WAFInvalidParameterException' + Field SCOPE_VALUE / Parameter GLOBAL / Reason 'The scope is not valid.' / message 'Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: GLOBAL'; got __type 'ValidationException' with "1 validation error detected: Value 'GLOBAL' at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [CLOUDFRONT, REGIONAL]".

Model references: wafv2 Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729) — GetWebACL/UpdateWebACL/ListWebACLs all model WAFInvalidParameterException (client, httpError 400) with members message/Field/Parameter/Reason; ParameterExceptionField enum includes SCOPE_VALUE and RESOURCE_ARN; Scope enum = [CLOUDFRONT, REGIONAL]; ListWebACLs.Scope is @required; ListWebACLs.NextMarker @length min 1 max 256 and @pattern \\S; GetWebACL.ARN targets ResourceArn @length 20..2048.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay wafv2-dsw-cloudfront-scope-on-regional-endpoint step task#2 — UpdateWebACL with Scope=CLOUDFRONT against the regional endpoint; observe WAFNonexistentItemException instead of WAFInvalidParameterException + Field/Parameter/Reason.
4. Cause 2: same fixture step task#3 (GetWebACL, Scope=CLOUDFRONT regionally); same misclassification.
5. Cause 3: replay wafv2-dsw-get-web-acl-arn-route-mistakes step task#4 — GetWebACL with ARN=<an IPSet ARN>; observe WAFNonexistentItemException instead of AccessDeniedException 'Invalid ARN provided for GetWebACLRequest <arn>'.
6. Cause 4: replay wafv2-dsw-list-web-acls-paging-bounds step task#6 — ListWebACLs with NextMarker=""; compare the two constraint clauses' order and the rendered pattern.
7. Cause 5: replay wafv2-dsw-scope-enum-and-missing-scope step task#3 — ListWebACLs with no Scope; observe ValidationException with no Message.
8. Cause 6: same fixture step task#4 — ListWebACLs with Scope=GLOBAL; observe ValidationException enum text instead of WAFInvalidParameterException with Field SCOPE_VALUE / Parameter GLOBAL.
