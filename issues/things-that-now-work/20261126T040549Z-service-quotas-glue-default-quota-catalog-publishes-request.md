Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-glue
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-glue
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-2E9FB93F"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-2E9FB93F"].QuotaName
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-2E9FB93F"].Value
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-337244C9"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-337244C9"].QuotaName
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-337244C9"].Value
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-5E4153CA"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-5E4153CA"].QuotaName
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-5E4153CA"].Value
Issue-Related: 20261123T024600Z-service-quotas-listawsdefaultservicequotas-rows-misbound-across-events-fms-fsx-gamelift.md
Issue-Related: 20261124T090000Z-service-quotas-listawsdefaultservicequotas-glue-codes-whose-recorded-quota-no-source-states.md
Issue-Commit: 8bd7ae765f95c35533341a888dc9e286a4b308e1

# service-quotas glue default-quota catalog publishes request-rate rows under capacity QuotaCodes and drops Description

> _Note: this issue was identified against the code base at commit `8bd7ae765f95c35533341a888dc9e286a4b308e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
Cause 1 of 1: ListAWSDefaultServiceQuotas for ServiceCode "glue" answers 200 with a Quotas list whose rows are bound to the wrong QuotaCode, and whose Description is omitted on rows that do carry the right quota. Fixture service-quotas-catalog-list-aws-default-service-quotas-glue, step task#1.

AWS: Smithy (com.amazonaws.servicequotas#ServiceQuotasV20190624) models ListAWSDefaultServiceQuotas with required input member ServiceCode (@length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$) and output member Quotas -> ServiceQuotaListDefinition of ServiceQuota. ServiceQuota members: ServiceCode, ServiceName, QuotaArn, QuotaCode (@length 1..128), QuotaName, Value -> QuotaValue (double, @range 0..10000000000), Unit, Adjustable (boolean, default false), GlobalQuota (boolean, default false), UsageMetric -> MetricInfo, Period -> QuotaPeriod, QuotaContext, Description (@length 0..350). Modeled errors: AccessDeniedException 403, IllegalArgumentException 400, InvalidPaginationTokenException 400, NoSuchResourceException 404, ServiceException 500, TooManyRequestsException 429. The model fixes the row SHAPE only; which quota is published under which opaque QuotaCode, and the wording of its QuotaName/Description, is recorded behavior only. The recording for ServiceCode "glue" is therefore the reference: for every QuotaCode the service publishes, the row under that code carries that quota's own QuotaName, Description, Value, Unit, Adjustable, GlobalQuota, Period, UsageMetric and QuotaContext, worded as the service words them, and no published QuotaCode is absent from the list.

Emulator: the replay reports expected 200 / actual 200 with a body divergence spanning 56 member keys of $.Quotas, in three distinct failure modes.
(a) Rows bound to the wrong quota. The emulator publishes an API-request-rate quota under a code the recording publishes a capacity/account-plane quota under. Example keys from the replay detail: $.Quotas[QuotaCode="L-C3952F6C"].QuotaName expected 'Max concurrent data quality ruleset evaluation runs per account' / Description expected 'The maximum number of concurrent data quality ruleset evaluation runs in your account.', got QuotaName 'Rate of GetJob requests' / Description 'The maximum number of GetJob requests you can make per second in this account in the current Region.'; $.Quotas[QuotaCode="L-D987EC31"] expected 'Max functions per account' / 'The maximum number of functions in your account.', got 'Rate of GetPartition requests'; $.Quotas[QuotaCode="L-F953935E"] expected 'Max databases per account' / 'The maximum number of databases in your account.' / Value 10000.0, got 'Rate of UpdateTable requests' / 'The maximum number of UpdateTable requests you can make per second in this account in the current Region.' / Value 10.0; $.Quotas[QuotaCode="L-EAC30D24"] expected 'Max Named Tag Expressions' / 'The maximum number of named tag expression allowed per account', got 'Rate of GetTableVersions requests'. The same QuotaName/Description/Value triple diverges on L-071ABE08, L-2E9FB93F, L-337244C9, L-4071B0E3, L-5E4153CA, L-76EC689B, L-7DD7C33A, L-94D025B7, L-ACAC12B6, L-AD871090, L-B8497671, L-CB69DFA0, L-EEC98450 and L-F574AED9, and QuotaName/Description on L-64A88F0A.
(b) Description absent where the row is otherwise right: $.Quotas[QuotaCode="L-F545C262"].Description missing (expected 'The maximum number of connection types per AWS Region for this account.'); $.Quotas[QuotaCode="L-83192DBF"].Description missing (expected 'The maximum number of security configurations in your account.'). Description is a modeled, non-sensitive member of ServiceQuota and the recording populates it, so omitting it is a serializer/state gap, not an optional field.
(c) Value understated on the misbound rows, because the row carries a per-second call-rate number where a capacity number is published (L-F953935E 10.0 vs 10000.0 is the clearest instance).
The emulator's Description text for the misbound rows is itself a generated per-operation sentence template ('The maximum number of <Op> requests you can make per second in this account in the current Region.'), which is how the mis-binding is visible even without comparing values.

Why: a client that reads the default-quota catalog by QuotaCode gets the wrong quota entirely. Infrastructure that pre-flights a glue deployment against 'Max databases per account' reads 10 instead of 10000 and refuses a legal deployment; a capacity planner that keys on L-C3952F6C or L-D987EC31 is handed a request-rate quota and plans against a per-second number as if it were a resource count. Rows whose Description is dropped break consumers that render or match on the quota sentence, and because the status is 200 there is no error for a client to fall back on - the response is silently wrong.

Fix: the glue entries of the service-quotas default-quota catalog state (the per-ServiceCode minted catalog the ListAWSDefaultServiceQuotas handler reads), not the handler's pagination or serialization: each glue QuotaCode must carry the quota the service publishes under that code, with that quota's QuotaName, Description, Value, Unit, Adjustable, GlobalQuota, Period and UsageMetric, and the generated 'Rate of <Op> requests' row template must stop being emitted for codes that publish a capacity or account plane. The two Description-only keys need the sentence added to rows that are already bound correctly.

Note on existing reports: open issue 20261124T090000Z-service-quotas-listawsdefaultservicequotas-glue-codes-whose-recorded-quota-no-source-states.md carries the same fixture/step but only three Issue-Key entries (L-2E9FB93F, L-337244C9, L-071ABE08.QuotaName) and declares the remaining codes unauthorable; 20261123T024600Z-service-quotas-listawsdefaultservicequotas-rows-misbound-across-events-fms-fsx-gamelift.md and 20261111T000000Z-service-quotas-listawsdefaultservicequotas-residual-misbound-rows-events-firehose-fms-fsx-gamelift-glue-imagebuilder.md mention glue among other codes. The replay reports this cluster as owned by none of them (owned_by is empty), i.e. the current 56-key divergence set is not covered by any open issue's replay key, so it is filed here with the full key set and the recorded expectations the replay does quote.

Traces: service-quotas-catalog-list-aws-default-service-quotas-glue#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (summary: 270 steps, 54 passed, 216 mismatched, score 20.0%, emulator_up true, bootstrap_signal false).

Divergence cluster (verdict mismatch, stub_path empty, owned_by empty, affected_count 1):
  fixture_id: service-quotas-catalog-list-aws-default-service-quotas-glue
  step_key:   task#1
  service:    service-quotas
  operation:  ListAWSDefaultServiceQuotas
  expected_status 200, actual_status 200 - body divergence only.
  divergence_key: service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-071ABE08"].Description,.QuotaName,.Value, L-2E9FB93F(.Description,.QuotaName,.Value), L-337244C9(.Description,.QuotaName,.Value), L-4071B0E3(.Description,.QuotaName,.Value), L-5E4153CA(.Description,.QuotaName,.Value), L-64A88F0A(.Description,.QuotaName), L-76EC689B(.Description,.QuotaName,.Value), L-7DD7C33A(.Description,.QuotaName,.Value), L-83192DBF(.Description), L-94D025B7(.Description,.QuotaName,.Value), L-ACAC12B6(.Description,.QuotaName,.Value), L-AD871090(.Description,.QuotaName,.Value), L-B8497671(.Description,.QuotaName,.Value), L-C3952F6C(.Description,.QuotaName), L-CB69DFA0(.Description,.QuotaName,.Value), L-D987EC31(.Description,.QuotaName), L-EAC30D24(.Description,.QuotaName,.Value), L-EEC98450(.Description,.QuotaName), L-F545C262(.Description), L-F574AED9(.Description,.QuotaName,.Value), L-F953935E(.Description,.QuotaName,.Value)

Expected vs actual, verbatim from the replay detail:
  $.Quotas[QuotaCode="L-F545C262"].Description: missing - expected 'The maximum number of connection types per AWS Region for this account.', got None
  $.Quotas[QuotaCode="L-83192DBF"].Description: missing - expected 'The maximum number of security configurations in your account.', got None
  $.Quotas[QuotaCode="L-C3952F6C"].Description: expected 'The maximum number of concurrent data quality ruleset evaluation runs in your account.', got 'The maximum number of GetJob requests you can make per second in this account in the current Region.'
  $.Quotas[QuotaCode="L-C3952F6C"].QuotaName: expected 'Max concurrent data quality ruleset evaluation runs per account', got 'Rate of GetJob requests'
  $.Quotas[QuotaCode="L-D987EC31"].Description: expected 'The maximum number of functions in your account.', got 'The maximum number of GetPartition requests you can make per second in this account in the current Region.'
  $.Quotas[QuotaCode="L-D987EC31"].QuotaName: expected 'Max functions per account', got 'Rate of GetPartition requests'
  $.Quotas[QuotaCode="L-F953935E"].Description: expected 'The maximum number of databases in your account.', got 'The maximum number of UpdateTable requests you can make per second in this account in the current Region.'
  $.Quotas[QuotaCode="L-F953935E"].QuotaName: expected 'Max databases per account', got 'Rate of UpdateTable requests'
  $.Quotas[QuotaCode="L-F953935E"].Value: expected 10000.0, got 10.0
  $.Quotas[QuotaCode="L-EAC30D24"].Description: expected 'The maximum number of named tag expression allowed per account', got 'The maximum number of GetTableVersions requests you can make per second in this account in the current Region.'
  $.Quotas[QuotaCode="L-EAC30D24"].QuotaName: expected 'Max Named Tag Expressions', got 'Rate of GetTableVersions requests'

Fixed vs variable text. The requirement is the wording the service uses for the quota published under the code: the capacity sentences are fixed text with the resource noun and the scope ('The maximum number of <resource> in your account.', 'The maximum number of <resource> per AWS Region for this account.') and the QuotaName is the service's own label for that plane; the emulator's 'Rate of <Operation> requests' / 'The maximum number of <Operation> requests you can make per second in this account in the current Region.' is the request-rate template and belongs only on the codes that publish a call rate. QuotaCode, ServiceCode and the numeric Value come from the catalog for that ServiceCode, not from one run; the QuotaArn region segment and any request id in the response envelope are per-environment and are not part of the requirement.

Smithy cited: ListAWSDefaultServiceQuotas input ServiceCode required @length 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; output Quotas -> ServiceQuotaListDefinition; ServiceQuota.Description @length 0..350, ServiceQuota.Value -> QuotaValue @range 0..10000000000, ServiceQuota.QuotaCode @length 1..128, ServiceQuota.Adjustable/GlobalQuota default false.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay trace set simparity-wire-aws-v1, fixture service-quotas-catalog-list-aws-default-service-quotas-glue, step task#1.
3. Equivalently, by hand: POST the service-quotas endpoint with header X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode": "glue"}.
4. Index the response's $.Quotas array by QuotaCode and compare each row field-by-field against the recorded response row for the SAME QuotaCode.
5. Observed: 200 with 56 diverging member keys - L-C3952F6C, L-D987EC31, L-F953935E, L-EAC30D24, L-071ABE08, L-2E9FB93F, L-337244C9, L-4071B0E3, L-5E4153CA, L-64A88F0A, L-76EC689B, L-7DD7C33A, L-94D025B7, L-ACAC12B6, L-AD871090, L-B8497671, L-CB69DFA0, L-EEC98450, L-F574AED9 carry another quota's QuotaName/Description (and Value where it diverges), and L-F545C262 and L-83192DBF return no Description.
6. Fixed when every returned glue row's QuotaName, Description, Value, Unit, Adjustable, GlobalQuota, Period, UsageMetric and QuotaContext match the recorded row for the same QuotaCode, every recorded glue QuotaCode is present, and no row carries the generated 'Rate of <Operation> requests' template unless the recording publishes a request-rate quota under that code.
