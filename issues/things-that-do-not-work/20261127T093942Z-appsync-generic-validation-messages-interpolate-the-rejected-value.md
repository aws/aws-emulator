Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-channel-namespace-code-handlers-size-limit
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-channel-namespace-code-handlers-size-limit appsync-channel-namespace-name-pattern-rejections appsync-delete-channel-namespace-name-length-bounds appsync-evaluate-template-below-minimum-length
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|UpdateChannelNamespace|4xx->4xx|$.message
Issue-Key: appsync|UpdateChannelNamespace|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateChannelNamespace|4xx->4xx|$.message
Issue-Key: appsync|CreateChannelNamespace|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteChannelNamespace|4xx->4xx|$.message
Issue-Key: appsync|DeleteChannelNamespace|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|EvaluateMappingTemplate|4xx->4xx|$.message
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync generic validation messages interpolate the rejected value and anchor the published pattern

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannelNamespace

## Description
Where AppSync does answer with the generic Smithy constraint frame, the emulator builds it wrongly for these members: it interpolates the rejected value into the message and prints the regex with its ^...$ anchors, while the recorded AWS message for the same members omits the value and publishes the pattern unanchored. Status (400) and the constraint list are right; the frame is not. 4 causes in this issue; all four must be closed before submitting. The open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md describes this same formatter defect for CreateGraphqlApi$name, but it carries none of the replay keys below (its grounding is a behavioral check), so none of these causes is covered by it — and these are four further operations whose steps must be re-replayed.

=== Cause 1 of 4: UpdateChannelNamespace codeHandlers over its length ceiling echoes the whole payload ===
AWS: Smithy (appsync) UpdateChannelNamespace, ChannelNamespace$codeHandlers: Code @length 1-32768; modeled BadRequestException|400. Recorded behavior for codeHandlers longer than 32768 is 400 with "1 validation error detected: Value at 'codeHandlers' failed to satisfy constraint: Member must have length less than or equal to 32768" — fixed frame; variable parts are the member name and the modeled ceiling only.
Emulator: the same frame but with the entire rejected 32769-byte handler source interpolated after "Value ", i.e. "1 validation error detected: Value '<the whole submitted code>' at 'codeHandlers' failed to satisfy ...". x-amzn-errortype comes back 'BadRequestException', which still fails the recorded header's shape.
Why: beyond not matching AWS's text, echoing an over-limit payload back into an error message multiplies the response size and writes caller source into every client log for a class where AWS deliberately does not.
Fix: the shared constraint-message formatter used by the AppSync validation layer (omit the submitted value for length violations).
Traces: appsync-channel-namespace-code-handlers-size-limit#task#5 (cluster also covers #task#6).

=== Cause 2 of 4: CreateChannelNamespace name pattern violation echoes the value and the anchors ===
AWS: Smithy (appsync) CreateChannelNamespace, name: Namespace @length 1-50 @pattern ^([A-Za-z0-9](?:[A-Za-z0-9\-]{0,48}[A-Za-z0-9])?)$; modeled BadRequestException|400. Recorded behavior for a name that does not match is 400 with "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ([A-Za-z0-9](?:[A-Za-z0-9\-]{0,48}[A-Za-z0-9])?)" — the pattern is published WITHOUT the ^ and $ anchors and the submitted name is absent.
Emulator: "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([A-Za-z0-9](?:[A-Za-z0-9\-]{0,48}[A-Za-z0-9])?)$" — value interpolated, pattern anchored.
Why: the published pattern is what a client compiles or shows to a user; the anchored form and the leaked value make the message non-AWS on both counts.
Fix: the same formatter as Cause 1 — strip the anchors when rendering a @pattern and omit the value.
Traces: appsync-channel-namespace-name-pattern-rejections#task#2 (cluster also covers #task#3).

=== Cause 3 of 4: DeleteChannelNamespace two-violation message echoes the value in both clauses ===
AWS: Smithy (appsync) DeleteChannelNamespace, name: Namespace @length 1-50 and the pattern above; modeled BadRequestException|400. Recorded behavior for a name that violates both is 400 with "2 validation errors detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 50; Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ([A-Za-z0-9](?:[A-Za-z0-9\-]{0,48}[A-Za-z0-9])?)" — the plural count, the two clauses in that order joined by '; ', each omitting the value, and the unanchored pattern.
Emulator: the same two clauses but with the submitted name interpolated into each and the pattern anchored.
Why: as Causes 1-2; the multi-violation join is otherwise correct, so only the per-clause rendering is wrong.
Fix: the same formatter as Cause 1.
Traces: appsync-delete-channel-namespace-name-length-bounds#task#4.

=== Cause 4 of 4: EvaluateMappingTemplate template below its minimum length echoes the value ===
AWS: Smithy (appsync) EvaluateMappingTemplate — POST /v1/dataplane-evaluatetemplate, template: Template @required @length 2-65536; modeled BadRequestException|400. Recorded behavior for a one-character template is 400 with "1 validation error detected: Value at 'template' failed to satisfy constraint: Member must have length greater than or equal to 2" — value omitted.
Emulator: "1 validation error detected: Value 'x' at 'template' failed to satisfy constraint: Member must have length greater than or equal to 2".
Why: as Cause 1; this is the lower-bound variant of the same frame on a different handler.
Fix: the same formatter as Cause 1.
Traces: appsync-evaluate-template-below-minimum-length#task#2 (cluster also covers #task#3).

## Evidence
Cause 1 — appsync-channel-namespace-code-handlers-size-limit#task#5 UpdateChannelNamespace: 400 both sides; $.message expected "1 validation error detected: Value at 'codeHandlers' failed to satisfy constraint: Member must have length less than or equal to 32768", got the same frame with the full 32769-byte submitted code interpolated after "Value "; $headers.x-amzn-errortype shape_mismatch, got 'BadRequestException'.
Cause 2 — appsync-channel-namespace-name-pattern-rejections#task#2 CreateChannelNamespace: expected "...Value at 'name' ... regular expression pattern: ([A-Za-z0-9](?:[A-Za-z0-9\\-]{0,48}[A-Za-z0-9])?)", got "...Value '-sim2real-probe-ns-617c0b94' at 'name' ... pattern: ^([A-Za-z0-9](?:[A-Za-z0-9\\-]{0,48}[A-Za-z0-9])?)$".
Cause 3 — appsync-delete-channel-namespace-name-length-bounds#task#4 DeleteChannelNamespace: expected "2 validation errors detected: Value at 'name' ... length less than or equal to 50; Value at 'name' ... pattern: ([A-Za-z0-9](?:[A-Za-z0-9\\-]{0,48}[A-Za-z0-9])?)", got both clauses with the submitted 51-character name interpolated and the pattern anchored.
Cause 4 — appsync-evaluate-template-below-minimum-length#task#2 EvaluateMappingTemplate: expected "1 validation error detected: Value at 'template' ... length greater than or equal to 2", got the same frame with "Value 'x'".

## Steps to Reproduce
Cause 1: CreateApi + CreateChannelNamespace, then UpdateChannelNamespace(apiId, name, codeHandlers=<32769 chars>) -> 400 whose message omits the submitted code.
Cause 2: CreateChannelNamespace(apiId, name='-leading-hyphen-name') -> 400 whose message omits the name and prints the pattern without ^/$.
Cause 3: DeleteChannelNamespace(apiId, name=<51 chars ending in a character the pattern forbids>) -> 400 '2 validation errors detected: ...' with neither clause echoing the name.
Cause 4: EvaluateMappingTemplate(template='x', context='{}') -> 400 whose message omits the submitted template.
