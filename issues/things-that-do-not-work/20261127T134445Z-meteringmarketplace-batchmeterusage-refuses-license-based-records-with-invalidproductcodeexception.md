Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected
Issue-Shape-Id: com.amazonaws.marketplacemetering#AWSMPMeteringService
Issue-Key: meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected|run#7|AssertionError|assert code == "InvalidLicenseException", (
Issue-Commit: 692629b6d1ae5ae78e012290c3136f29b80245ac

# meteringmarketplace: BatchMeterUsage refuses license-based records with InvalidProductCodeException, and its TimestampOutOfBoundsException message does not name the record's instant

> _Note: this issue was identified against the code base at commit `692629b6d1ae5ae78e012290c3136f29b80245ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchMeterUsage

## Description
Two root causes in BatchMeterUsage, both exercised by the single failing behavioral check meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected (it fails on Cause 1 before it can reach Cause 2; both are visible in the handler).

Cause 1 of 2: a record that identifies its product by LicenseArn (no request-level ProductCode) is refused as an unknown product code instead of being validated as a license.
AWS: Smithy model com.amazonaws.marketplacemetering#BatchMeterUsage — BatchMeterUsageRequest$ProductCode is OPTIONAL (target ProductCode, length 0..255, pattern ^[-a-zA-Z0-9/=:_.@]*$, addedDefault ""), and its documentation states: "ProductCode is required only for legacy integrations that use CustomerIdentifier. For new integrations using LicenseArn (Concurrent Agreements), do NOT include ProductCode at the request level. The LicenseArn in each UsageRecord identifies both the product and the specific agreement." UsageRecord$LicenseArn (pattern ^arn:aws[a-zA-Z-]*:...) is the product+agreement identifier for that style. So a request carrying UsageRecords[].LicenseArn and no ProductCode is a well-formed license-style request; it must NOT be refused for a product code the caller is documented not to send. The modeled error for a LicenseArn that does not resolve is InvalidLicenseException (HTTP 400, "Ensure the LicenseArn is valid, matches the customer, and usage is within the license activation period"); for an unknown/inactive license AWS answers HTTP 400, code InvalidLicenseException, message "Provide valid, active LicenseArns for a single product and retry." (fixed text, no variable parts — recorded behavior, confirmed by the check's assertion).
Emulator: the request is refused with HTTP 400, code InvalidProductCodeException, message "The product code  does not match the product code used for publishing the product" — the handler reads input.product_code.unwrap_or_default(), gets the modeled "" default, looks "" up in state.products, misses, and raises InvalidProductCodeException whose message interpolates the empty product code (note the doubled space where the value should be). LicenseArn is never consulted, so InvalidLicenseException is never produced at all. Timestamp-window, dimension, allocation and customer validation are all unreachable for license-style requests because the product lookup precedes them.
Why: every client on the Concurrent Agreements integration (the one AWS requires for new SaaS products) gets a wrong error class and a nonsense message. The code is not retryable-distinguishable: a caller that keys on InvalidProductCodeException will "fix" its ProductCode (which AWS tells it not to send) instead of fixing the license, and an SDK/test that asserts InvalidLicenseException for a bad license never sees it. No license-based usage can ever be metered against the emulator.
Fix: the BatchMeterUsage handler's product/license resolution and its validation order — resolve the product and agreement from UsageRecord$LicenseArn when the request names no ProductCode, refuse an unresolvable license with InvalidLicenseException (and keep InvalidProductCodeException only for a request that actually names a ProductCode), with the license state it needs added to the service store.

Cause 2 of 2: the TimestampOutOfBoundsException message is the model's doc sentence plus the emulator's own limit instead of AWS's message naming the offending instant.
AWS: TimestampOutOfBoundsException (HTTP 400). Message text is "Timestamp <instant> is out of allowed range", where the fixed text is "Timestamp " ... " is out of allowed range" and the single variable part is the offending UsageRecord's own Timestamp, rendered as %Y-%m-%dT%H:%M:%SZ (recorded behavior; the model documents only the condition, not the text). The window itself: UsageRecord$Timestamp — usage may be metered up to 24 hours in the past, plus the end-of-cycle grace the operation documents ("At the end of each billing cycle, a 6-hour grace period applies. We accept usage records for the previous billing month until 06:00 UTC on the first day of the next month... After 06:00 UTC on April 1, March records are rejected regardless of the normal 24-hour submission window").
Emulator: message is "The timestamp value passed in the UsageRecord is out of allowed range: usage may be metered for up to 24 hours in the past and not in the future" — it names no instant, and it is a paraphrase of the model's error documentation plus the handler's own constant. The handler also implements the window as a plain now-24h..now band (BATCH_METER_USAGE_BACKDATING_SECS) with no billing-cycle/grace-period boundary, so a record inside the normal 24 hours but before this month's start is accepted and a last-month record past 06:00 UTC on the 1st is rejected only incidentally.
Why: a client that parses the message to report which record it must drop (BatchMeterUsage rejects the whole batch, so the instant is the only pointer to the bad record) gets nothing to parse, and a message-equality assertion against AWS fails.
Fix: the BatchMeterUsage handler's timestamp validation and its error construction — build the message from the offending record's own timestamp in AWS's wording, and apply the billing-cycle grace boundary alongside the 24-hour band.

Traces: meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected (fails at Cause 1; its third assertion covers Cause 2).
No existing open issue covers this service or operation (list_existing_issues query='metering' returns only apigateway/cloudwatch/ecs items); the handler's module doc claims "the bootstrap issue defers richer LicenseArn resolution to its own issue", but no such issue exists, so this is it.

## Evidence
Check meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected, request: BatchMeterUsage with no ProductCode and UsageRecords=[{Timestamp: now-1h, CustomerAWSAccountId: <account>, LicenseArn: arn:aws:license-manager::<account>:license:l-<id>, Dimension: Users, Quantity: 1}].

Expected (AWS): HTTP 400, Error.Code="InvalidLicenseException", Error.Message="Provide valid, active LicenseArns for a single product and retry."
Actual (emulator): HTTP 400, Error.Code="InvalidProductCodeException", Error.Message="The product code  does not match the product code used for publishing the product"
AssertionError: "a record one hour old should pass the window and fail on its license, got 'InvalidProductCodeException': 'The product code  does not match the product code used for publishing the product'"

Same for the 23-hours-old record (second iteration of the check's loop).

Stale record (>24h, i.e. now-25h or the prior month's last hour):
Expected (AWS): HTTP 400, Error.Code="TimestampOutOfBoundsException", Error.Message="Timestamp <instant> is out of allowed range" with <instant> = that record's own Timestamp formatted %Y-%m-%dT%H:%M:%SZ.
Actual (emulator, from the handler): HTTP 400, "TimestampOutOfBoundsException", "The timestamp value passed in the UsageRecord is out of allowed range: usage may be metered for up to 24 hours in the past and not in the future" — no instant named.

Source: services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/src/ops/batch_meter_usage.rs lines 255-273 (product lookup on the defaulted empty ProductCode, before any record validation; LicenseArn never read) and lines 277-292 (window band + message); services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/src/state.rs line 39 (BATCH_METER_USAGE_BACKDATING_SECS, no grace-period boundary).

## Steps to Reproduce
1. Boot the emulator and point a botocore meteringmarketplace client at it.
2. Cause 1: call BatchMeterUsage with NO ProductCode and one UsageRecord carrying Timestamp=now-1h, CustomerAWSAccountId=<account>, LicenseArn="arn:aws:license-manager::<account>:license:l-<32 hex>", Dimension="Users", Quantity=1. Observe InvalidProductCodeException with an empty product code in the message; AWS answers InvalidLicenseException / "Provide valid, active LicenseArns for a single product and retry." Repeat with Timestamp=now-23h — same wrong refusal.
3. Cause 2: repeat with Timestamp=now-25h (or, if that lands before this month's start, the last hour of the previous month). Observe TimestampOutOfBoundsException whose message is the generic paraphrase and names no instant; AWS answers "Timestamp <that record's timestamp as %Y-%m-%dT%H:%M:%SZ> is out of allowed range".
4. Replay check meteringmarketplace.batch-meter-usage-record-older-than-a-day-rejected to confirm both.
