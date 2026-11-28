Issue-Source: trace-sweep-divergence
Issue-Trace: marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch marketplacemetering-batch-meter-usage-license-record-timestamp-out-of-bounds marketplacemetering-batch-meter-usage-product-code-and-batch-size-refusals marketplacemetering-batch-meter-usage-refusals marketplacemetering-batch-meter-usage-request-validation marketplacemetering-batch-meter-usage-timestamp-window-refusals meteringmarketplace-batch-meter-usage-license-record-refusals meteringmarketplace-batch-meter-usage-product-code-refusals meteringmarketplace-batch-meter-usage-record-identity-rejected meteringmarketplace-batch-meter-usage-unpublished-product-code-rejected marketplacemetering-meter-usage-request-validation meteringmarketplace-meter-usage-request-validation-refusals meteringmarketplace-meter-usage-request-validation-precedes-platform-check meteringmarketplace-register-usage-and-resolve-customer-validation-refusals meteringmarketplace-resolve-customer-empty-registration-token-rejected meteringmarketplace-register-usage-outside-container-rejected
Issue-Shape-Id: com.amazonaws.marketplacemetering#AWSMPMeteringService
Issue-Key: meteringmarketplace|BatchMeterUsage|4xx->4xx|$.__type
Issue-Key: meteringmarketplace|BatchMeterUsage|4xx->4xx|$.message
Issue-Key: meteringmarketplace|MeterUsage|4xx->4xx|$.message
Issue-Key: meteringmarketplace|MeterUsage|4xx->4xx|$.__type
Issue-Key: meteringmarketplace|MeterUsage|4xx->4xx|$.Message
Issue-Key: meteringmarketplace|ResolveCustomer|4xx->4xx|$.message
Issue-Key: meteringmarketplace|RegisterUsage|4xx->4xx|$.__type
Issue-Key: meteringmarketplace|RegisterUsage|4xx->4xx|$.message
Issue-Commit: 9f42440ef4b6e16da3a73b4df617b3d834ef29e8

# meteringmarketplace: refusals are misclassified as InvalidProductCodeException and validation-constraint messages diverge from AWS

> _Note: this issue was identified against the code base at commit `9f42440ef4b6e16da3a73b4df617b3d834ef29e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchMeterUsage

## Description
Seven net-new root causes in this service's refusal paths, all found by replaying the wire corpus (48 of 74 steps mismatched, score 35.1%). They share one shape of fault: a request AWS refuses with a specific error class and a specific message is instead refused with the WRONG class (always InvalidProductCodeException, because every handler resolves the product code before anything else) or with the RIGHT class and a message whose constraint text the emulator words itself. All seven are HTTP 400 -> HTTP 400, so only the __type/message members diverge. Smithy model used throughout: services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/smithy-model/com-amazonaws-marketplacemetering-awsmpmeteringservice.json (com.amazonaws.marketplacemetering#AWSMPMeteringService, awsJson1_1).

RELATED OPEN ISSUE: 20261127T134445Z-meteringmarketplace-batchmeterusage-refuses-license-based-records-with-invalidproductcodeexception describes the same handler defect as Cause 1 below from a BEHAVIORAL check; it carries no wire replay key, so Cause 1 is filed here with the fixture binding a fixer must re-replay. That issue does not cover Causes 2-7.

Cause 1 of 7: BatchMeterUsage refuses a license-identified batch as an unknown product code instead of judging the license.
AWS: Smithy BatchMeterUsageRequest$ProductCode is OPTIONAL (target ProductCode, @length 0..255, pattern ^[-a-zA-Z0-9/=:_.@]*$, addedDefault "") and documents: "ProductCode is required only for legacy integrations that use CustomerIdentifier. For new integrations using LicenseArn (Concurrent Agreements), do NOT include ProductCode at the request level. The LicenseArn in each UsageRecord identifies both the product and the specific agreement." UsageRecord$LicenseArn (pattern ^arn:aws[a-zA-Z-]*:...) is that identifier. The modeled error for a LicenseArn that does not resolve is InvalidLicenseException (HTTP 400, doc: "Ensure the LicenseArn is valid, matches the customer, and usage is within the license activation period"). Recorded behavior: HTTP 400, __type "InvalidLicenseException", message "Provide valid, active LicenseArns for a single product and retry." — entirely fixed text, no variable parts.
Emulator: HTTP 400, __type "InvalidProductCodeException", message "The product code  does not match the product code used for publishing the product" — the handler looks up the defaulted empty ProductCode in its product store, misses, and interpolates the empty string (note the doubled space). LicenseArn is never read, so InvalidLicenseException is never produced, and timestamp/dimension/allocation/customer validation is unreachable for license-style batches. 23 steps across 11 fixtures collapse into this one cluster.
Why: every client on the Concurrent Agreements integration (the one AWS requires for new products) gets a class that tells it to fix a ProductCode AWS tells it not to send, and no license-based usage can ever be metered.
Fix: BatchMeterUsage's handler — resolve product and agreement from UsageRecord$LicenseArn when the request names no ProductCode, refuse an unresolvable/inactive license with InvalidLicenseException in AWS's wording, keep InvalidProductCodeException only for a request that actually names a ProductCode, and add the license state the store needs.
Traces: marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch#task#1 (representative; +22 affected steps, incl. marketplacemetering-batch-meter-usage-license-record-timestamp-out-of-bounds#task#1, meteringmarketplace-batch-meter-usage-license-record-refusals#task#1, meteringmarketplace-batch-meter-usage-record-identity-rejected#task#1).

Cause 2 of 7: the ValidationException constraint message renders an empty LIST as "0 items" instead of its literal value, for BatchMeterUsage's nested usageAllocations.
AWS: UsageRecord$UsageAllocations has @length min 1 (0..2500 on the list member: min 1, max 2500). The boundary validator's message is "1 validation error detected: Value '[]' at 'usageRecords.1.member.usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1" (recorded behavior; the model states the constraint, not the text). Fixed text: "<n> validation error(s) detected: Value '", "' at '", "' failed to satisfy constraint: Member must have length greater than or equal to ", and the limit. Variable parts: the offending value rendered as AWS renders it — a list as its JSON literal, here [] — the member path taken from the request (1-based index inside usageRecords, then .member.<memberName>), and the bound from the model.
Emulator: same status and class, but the value is rendered as a count: "... Value '0 items' at 'usageRecords.1.member.usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1". 15 steps across 9 fixtures.
Why: a client or SDK test that matches the refusal text (the only machine-readable pointer to WHICH record and member AWS rejected, since BatchMeterUsage rejects the whole batch) never matches, and "0 items" is not a value any AWS message emits.
Fix: the shared request-validation message renderer for @length violations on list members — render the value as its literal, not as an item count.
Traces: marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch#task#3 (+14, incl. marketplacemetering-batch-meter-usage-product-code-and-batch-size-refusals#task#1, meteringmarketplace-batch-meter-usage-unpublished-product-code-rejected#task#1).

Cause 3 of 7: the same empty-list rendering on MeterUsage's top-level usageAllocations.
AWS: MeterUsageRequest$UsageAllocations @length 1..2500. Recorded: HTTP 400, ValidationException, "1 validation error detected: Value '[]' at 'usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1".
Emulator: "... Value '0 items' at 'usageAllocations' ...". Filed as its own cause because it is a second operation's refusal that must be re-replayed to confirm the renderer fix reaches the non-nested path too.
Why: as Cause 2.
Fix: same validation-message renderer, exercised from MeterUsage's input validation.
Traces: marketplacemetering-meter-usage-request-validation#task#3 (+2: meteringmarketplace-meter-usage-request-validation-refusals#task#1, #task#4).

Cause 4 of 7: MeterUsage runs its product lookup BEFORE input validation, so an empty productCode is answered InvalidProductCodeException instead of ValidationException.
AWS: MeterUsageRequest$ProductCode is @required; the boundary validator rejects an empty value before the handler runs. Recorded: HTTP 400, __type "ValidationException", message "Value '' at 'productCode' failed to satisfy constraint: Member must have length greater than or equal to 1" — fixed text "Value '" / "' at '" / "' failed to satisfy constraint: Member must have length greater than or equal to " + the bound; variable parts are the submitted value (empty here) and the member path from the request. ValidationException is not in MeterUsage's modeled error list, confirming it is the boundary refusal, not a handler error.
Emulator: HTTP 400, __type "InvalidProductCodeException", message "The product code  does not match the product code used for publishing the product" — src/ops/meter_usage.rs resolves state.products.get(&input.product_code) (line ~281) with no prior emptiness check, so the empty code is treated as an unknown product.
Why: a caller that omitted or blanked a required member is told its product is unpublished, so it goes looking for a catalog problem instead of a request problem; and the refusal is not the modeled/boundary class an SDK's validation path expects.
Fix: MeterUsage's input validation — enforce the required/non-empty productCode constraint at the boundary, ahead of the handler's product resolution.
Traces: marketplacemetering-meter-usage-request-validation#task#4 (+1: meteringmarketplace-meter-usage-request-validation-precedes-platform-check#task#1).

Cause 5 of 7: MeterUsage has no caller-platform/authorization gate, so a call from an unsupported platform is refused as an unknown product code.
AWS: MeterUsage must be signed by the buyer's EC2 instance role / IRSA / ECS task role / AgentCore execution role (operation doc). A call that is not authorized or comes from an unsupported platform is refused HTTP 400 with an AccessDeniedException envelope whose message member is "Message" (capital M, unlike the modeled errors' lowercase "message"): "User is not authorized to call MeterUsage or calling from an unsupported platform." — entirely fixed text, no variable parts (recorded behavior; the model declares neither the shape nor the text).
Emulator: HTTP 400, __type "InvalidProductCodeException", with lowercase "message" = "The product code <code> does not match the product code used for publishing the product" (the recording shows a well-formed product code in that slot, so this is not the empty-code path of Cause 4 — the product simply does not exist in the store and no platform check exists at all).
Why: a container/agent client cannot distinguish "I am not allowed to meter from here" from "my product code is wrong", and it never sees the AccessDeniedException class or the capital-M Message member the AWS envelope uses.
Fix: MeterUsage's handler/authorization gate — judge caller platform and authorization before product resolution and answer the AccessDeniedException envelope AWS words.
Traces: meteringmarketplace-meter-usage-request-validation-precedes-platform-check#task#4.

Cause 6 of 7: ResolveCustomer's pattern-constraint message quotes the model's ANCHORED regex instead of the pattern AWS reports.
AWS: ResolveCustomerRequest$RegistrationToken targets NonEmptyString with pattern ^[\s\S]+$. Recorded refusal: HTTP 400, ValidationException, "1 validation error detected: Value '' at 'registrationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [\s\S]+" — AWS quotes the pattern WITHOUT the ^ and $ anchors. Fixed text: "<n> validation error(s) detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the submitted value, the member path, and the pattern as AWS reports it.
Emulator: identical except the pattern is rendered "^[\s\S]+$".
Why: any assertion or client-side parse of the refusal text differs by the two anchor characters, so an SDK conformance check on this refusal fails even though the rejection itself is right.
Fix: the shared validation-message renderer for @pattern violations — emit the unanchored pattern text AWS reports.
Traces: meteringmarketplace-register-usage-and-resolve-customer-validation-refusals#task#4 (+1: meteringmarketplace-resolve-customer-empty-registration-token-rejected#task#1).

Cause 7 of 7: RegisterUsage never raises PlatformNotSupportedException; a call from outside an ECS task / EKS pod is refused as an unknown product code.
AWS: PlatformNotSupportedException is a modeled RegisterUsage error (HTTP 400, doc: "Amazon Web Services Marketplace does not support metering usage from the underlying platform. Currently, Amazon ECS, Amazon EKS, and Fargate are supported"), and the operation doc states the platform refusal is raised on the initial call. Recorded: HTTP 400, __type "PlatformNotSupportedException", message "This operation can only be called from within an ECS task or an EKS pod" — entirely fixed text, no variable parts.
Emulator: HTTP 400, __type "InvalidProductCodeException", message "The product code <code> does not match the product code used for publishing the product" — src/ops/register_usage.rs resolves state.products.get(&input.product_code) (line ~220) as its first act; nothing in the handler ever constructs PlatformNotSupportedException, so the modeled variant is dead.
Why: a paid container product's startup guard keys on PlatformNotSupportedException to refuse to run outside a supported platform; it never sees it, and instead reports a product-catalog error for a platform problem.
Fix: RegisterUsage's handler — evaluate the caller-platform gate before product resolution and answer PlatformNotSupportedException with AWS's message.
Traces: meteringmarketplace-register-usage-outside-container-rejected#task#2 (+1: #task#3).

## Evidence
Replay of trace set simparity-wire-aws-v1: 17 fixtures replayed, 74 steps, 26 passed, 48 mismatched, score_pct 35.1, 7 divergence clusters — all of them on service "meteringmarketplace", all HTTP 400 vs HTTP 400 (only __type/message diverge).

Cause 1 — fixture marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch step task#1 (cluster of 23 steps):
  expected: 400, $.__type = "InvalidLicenseException", $.message = "Provide valid, active LicenseArns for a single product and retry."
  actual:   400, $.__type = "InvalidProductCodeException", $.message = "The product code  does not match the product code used for publishing the product"

Cause 2 — fixture marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch step task#3 (15 steps):
  expected: $.message = "1 validation error detected: Value '[]' at 'usageRecords.1.member.usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   $.message = "1 validation error detected: Value '0 items' at 'usageRecords.1.member.usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 3 — fixture marketplacemetering-meter-usage-request-validation step task#3 (3 steps):
  expected: $.message = "1 validation error detected: Value '[]' at 'usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   $.message = "1 validation error detected: Value '0 items' at 'usageAllocations' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 4 — fixture marketplacemetering-meter-usage-request-validation step task#4 (2 steps):
  expected: $.__type = "ValidationException", $.message = "Value '' at 'productCode' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   $.__type = "InvalidProductCodeException", $.message = "The product code  does not match the product code used for publishing the product"

Cause 5 — fixture meteringmarketplace-meter-usage-request-validation-precedes-platform-check step task#4 (1 step):
  expected: $.__type = "AccessDeniedException", $.Message = "User is not authorized to call MeterUsage or calling from an unsupported platform."
  actual:   $.__type = "InvalidProductCodeException", $.message = "The product code <productCode from the request> does not match the product code used for publishing the product" (the recording's own value was a generated code; the generated value is not the requirement — the class, the Message member name and the fixed text are)

Cause 6 — fixture meteringmarketplace-register-usage-and-resolve-customer-validation-refusals step task#4 (2 steps):
  expected: $.message = "1 validation error detected: Value '' at 'registrationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\s\\S]+"
  actual:   $.message = "1 validation error detected: Value '' at 'registrationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\s\\S]+$"

Cause 7 — fixture meteringmarketplace-register-usage-outside-container-rejected step task#2 (2 steps):
  expected: $.__type = "PlatformNotSupportedException", $.message = "This operation can only be called from within an ECS task or an EKS pod"
  actual:   $.__type = "InvalidProductCodeException", $.message = "The product code <productCode from the request> does not match the product code used for publishing the product"

Source of the shared ordering fault: services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/src/ops/meter_usage.rs lines 281-288 (product lookup is the handler's first judgement; no empty-code, platform or authorization check precedes it) and services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/src/ops/register_usage.rs lines 220-227 (same, and PlatformNotSupportedException is declared in RegisterUsageErrorModeled at line 149 but never constructed).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the wire corpus (replay_wire_corpus over trace set simparity-wire-aws-v1). All seven clusters reproduce; the 48 mismatched steps are these 7 causes.
2. Cause 1: POST AWSMPMeteringService.BatchMeterUsage with NO ProductCode and UsageRecords=[{Timestamp: now-1h, CustomerAWSAccountId: <account>, LicenseArn: "arn:aws:license-manager::<account>:license:l-<id>", Dimension: <dim>, Quantity: 1}]. Observe InvalidProductCodeException with an empty product code in the message; AWS answers InvalidLicenseException / "Provide valid, active LicenseArns for a single product and retry." Replay fixture marketplacemetering-batch-meter-usage-license-record-allocation-sum-mismatch step task#1.
3. Cause 2: BatchMeterUsage whose second UsageRecord carries UsageAllocations=[]. Observe "Value '0 items' at 'usageRecords.1.member.usageAllocations'"; AWS renders "Value '[]'". Replay ...allocation-sum-mismatch step task#3.
4. Cause 3: MeterUsage with UsageAllocations=[]. Observe the same "0 items" rendering at 'usageAllocations'. Replay marketplacemetering-meter-usage-request-validation step task#3.
5. Cause 4: MeterUsage with ProductCode="" (other members valid). Observe InvalidProductCodeException; AWS answers ValidationException naming 'productCode'. Replay marketplacemetering-meter-usage-request-validation step task#4.
6. Cause 5: MeterUsage from an unsupported platform / unauthorized caller with a well-formed, unpublished ProductCode. Observe InvalidProductCodeException; AWS answers AccessDeniedException with Message "User is not authorized to call MeterUsage or calling from an unsupported platform." Replay meteringmarketplace-meter-usage-request-validation-precedes-platform-check step task#4.
7. Cause 6: ResolveCustomer with RegistrationToken="". Observe the pattern quoted as ^[\\s\\S]+$; AWS quotes [\\s\\S]+. Replay meteringmarketplace-register-usage-and-resolve-customer-validation-refusals step task#4.
8. Cause 7: RegisterUsage from outside an ECS task / EKS pod with PublicKeyVersion=1. Observe InvalidProductCodeException; AWS answers PlatformNotSupportedException / "This operation can only be called from within an ECS task or an EKS pod". Replay meteringmarketplace-register-usage-outside-container-rejected step task#2 (and task#3).
9. Re-replay all the fixture#step bindings listed in this issue's Issue-Trace/Issue-Step and Issue-Trace-Ids before closing: all seven causes must pass.
