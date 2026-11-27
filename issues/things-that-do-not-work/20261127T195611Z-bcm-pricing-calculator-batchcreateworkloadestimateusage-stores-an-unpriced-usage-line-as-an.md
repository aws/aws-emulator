Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-pricing-calculator.batch-create-workload-estimate-usage-mixed-priced-and-unpriced-lines
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator.batch-create-workload-estimate-usage-mixed-priced-and-unpriced-lines|run#10|AssertionError|assert [item["key"] for item in batch["items"]] == [priced_key], (
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# BatchCreateWorkloadEstimateUsage stores an unpriced usage line as an INVALID item instead of rejecting it in errors with BAD_REQUEST

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateWorkloadEstimateUsage

## Description
Cause 1 of 1: a usage entry whose (serviceCode, usageType, operation) triple has no matching priced SKU is accepted, stored on the estimate and reported as a successful item with status INVALID, instead of being refused as a per-entry error. Failing check: bcm-pricing-calculator.batch-create-workload-estimate-usage-mixed-priced-and-unpriced-lines

AWS: BatchCreateWorkloadEstimateUsage splits its answer into two modeled lists — `items` (BatchCreateWorkloadEstimateUsageItems, "the list of successful usage line items that were created") and `errors` (BatchCreateWorkloadEstimateUsageErrors, "the list of errors reason and the usage item keys that CANNOT be created"). Smithy: BatchCreateWorkloadEstimateUsageError members {key, errorCode: BatchCreateWorkloadEstimateUsageCode enum [BAD_REQUEST, NOT_FOUND, CONFLICT, INTERNAL_SERVER_ERROR], errorMessage}. An entry naming usage the price list carries no rate for cannot be created, so AWS puts it in `errors`, keyed by that entry's `key`, with errorCode `BAD_REQUEST` and the errorMessage (fixed text, no variable parts) "No matching usage found for the provided ServiceCode, UsageType, and Operation. Check your inputs and try again." — recorded behavior, confirmed by the recording. The rejected entry is never stored: the estimate afterwards holds only the priced lines, its terminal status is VALID and its totalCost/costCurrency are computed over those priced lines alone. The whole request still succeeds with HTTP 200; the per-entry failure never becomes a top-level error.

Emulator: the unpriced entry is accepted into `items` alongside the priced one, with the cost members left unset and status `INVALID` (`items` came back as [{key: priced, status: VALID, cost: <rate>, currency: USD}, {key: unpriced, status: INVALID, no cost, no currency}]), and `errors` carries nothing for it. Because the line is also pushed onto the estimate, the two downstream observations diverge as well: the estimate's recomputed cost/status is taken over a line that AWS never created, and ListWorkloadEstimateUsage reports the unpriced line as a stored member of the estimate.

Why: a client cannot tell a rejected entry from an accepted one. The documented contract is "everything in `items` was created, everything in `errors` was not", so a caller iterating `items` treats a line AWS refused as live, never surfaces the BAD_REQUEST reason to its user, and reads a total cost (and an estimate status) computed over a line that does not exist on real AWS. Retry/reconciliation logic keyed on `errors[].key` sees an empty error list and concludes the whole batch priced cleanly.

Fix: the per-entry classification in the BatchCreateWorkloadEstimateUsage handler, which today prices every accepted entry unconditionally and only varies the reported cost status. An entry the rate card publishes no rate for must be diverted into `errors` (key = the entry's key, errorCode = BAD_REQUEST, errorMessage = the fixed sentence above) and must NOT be appended to the estimate's usage, so the estimate's recomputed cost and its later reads cover the priced lines only. The INVALID cost-status path in the shared usage state is then only for a line whose price is lost after creation, not for an entry that was never creatable.

Traces: bcm-pricing-calculator.batch-create-workload-estimate-usage-mixed-priced-and-unpriced-lines. Relevant code: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/batch_create_workload_estimate_usage.rs lines 24-28 (the documented choice to store an unrated usage type with no cost and status INVALID) and lines 379-400 (every accepted entry is priced, pushed onto the estimate and reported in `items`); the rate card and cost status live in services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/state.rs.

## Evidence
Request: CreateWorkloadEstimate(name=sim2real-we-*), then one BatchCreateWorkloadEstimateUsage(workloadEstimateId=<id>) with two entries:
  key="priced"   serviceCode=AmazonS3,  usageType=TimedStorage-ByteHrs,            operation=StandardStorage,                 amount=100.0
  key="unpriced" serviceCode=AmazonEC2, usageType=sim2real-NotAPricedUsageType,    operation=sim2real-NotAPricedOperation,    amount=1.0

Expected (AWS):
  HTTP 200
  items  = [ { key: "priced", status: "VALID", cost: <priced rate>, currency: "USD", ... } ]   # one item only
  errors = [ { key: "unpriced", errorCode: "BAD_REQUEST",
               errorMessage: "No matching usage found for the provided ServiceCode, UsageType, and Operation. Check your inputs and try again." } ]
  GetWorkloadEstimate(identifier) settles on status="VALID", totalCost = the priced line's cost alone, costCurrency="USD"
  ListWorkloadEstimateUsage(workloadEstimateId) = exactly the priced line's id

Actual (emulator):
  HTTP 200
  items  = [ {'serviceCode': 'AmazonS3', 'usageType': 'TimedStorage-ByteHrs', 'operation': 'StandardStorage', 'location': 'us-east-1', 'id': '<uuid>', 'usageAccountId': '<account>', 'quantity': {'unit': 'GB-Mo', 'amount': 100.0}, 'cost': 2.3, 'currency': 'USD', 'status': 'VALID', 'key': 'priced'},
             {'serviceCode': 'AmazonEC2', 'usageType': 'sim2real-NotAPricedUsageType', 'operation': 'sim2real-NotAPricedOperation', 'location': 'us-east-1', 'id': '<uuid>', 'usageAccountId': '<account>', 'quantity': {'amount': 1.0}, 'status': 'INVALID', 'key': 'unpriced'} ]
  errors = [] (nothing for key 'unpriced')

Assertion that tripped:
  AssertionError: expected only 'priced' to come back priced, got items [... 'key': 'priced' ..., ... 'status': 'INVALID', 'key': 'unpriced'}]

Message breakdown for the missing per-entry error: errorCode "BAD_REQUEST" is one of the four values BatchCreateWorkloadEstimateUsageCode declares; the errorMessage is entirely fixed text with no variable parts; the only request-derived value in the error entry is `key`, echoed from the rejected entry (here "unpriced"). The uuids, account id and the 2.3 rate above are per-run/per-rate-card values, not part of the requirement.

## Steps to Reproduce
1. client = boto3 client for bcm-pricing-calculator against the emulator.
2. estimate_id = client.create_workload_estimate(name="sim2real-we-x")["id"].
3. batch = client.batch_create_workload_estimate_usage(workloadEstimateId=estimate_id, usage=[
     {"key": "priced", "serviceCode": "AmazonS3", "usageType": "TimedStorage-ByteHrs", "operation": "StandardStorage", "usageAccountId": <account>, "amount": 100.0},
     {"key": "unpriced", "serviceCode": "AmazonEC2", "usageType": "sim2real-NotAPricedUsageType", "operation": "sim2real-NotAPricedOperation", "usageAccountId": <account>, "amount": 1.0}]).
4. Observe [i["key"] for i in batch["items"]] == ["priced", "unpriced"] (AWS: ["priced"]) and batch["errors"] == [] (AWS: one entry keyed "unpriced" with errorCode BAD_REQUEST and the fixed 'No matching usage found...' message).
5. Then client.list_workload_estimate_usage(workloadEstimateId=estimate_id)["items"] still holds the unpriced line, and client.get_workload_estimate(identifier=estimate_id) reports a totalCost/status computed over it instead of settling on the priced line alone.
