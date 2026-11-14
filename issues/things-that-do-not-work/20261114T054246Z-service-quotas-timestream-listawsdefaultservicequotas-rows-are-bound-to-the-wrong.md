Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-timestream
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-timestream
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-0C6E4BCB"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-16B72159"]
Issue-Related: 20261109T154349Z-service-quotas-listawsdefaultservicequotas-misbinds-quota-metadata-for-sns-sqs-sso.md
Issue-Commit: 5bd292a070514de6b32d2befe2ccb8e6e62157b1

# timestream ListAWSDefaultServiceQuotas rows are bound to the wrong QuotaCodes and omit Period/UsageMetric

> _Note: this issue was identified against the code base at commit `5bd292a070514de6b32d2befe2ccb8e6e62157b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
1 net-new cause, in the emulator's static AWS-default quota catalog for ServiceCode 'timestream'. Unlike the catalog gaps where a ServiceCode returns no rows at all, here rows ARE returned, but almost every QuotaCode is bound to the WRONG quota definition: the emulator serves Timestream's resource/retention/limit quotas under the QuotaCodes AWS assigns to API request-rate quotas, it drops the Period and UsageMetric projections that AWS attaches to rate quotas, and it omits two QuotaCodes entirely.

Cause 1 of 1 - ServiceCode 'timestream' AWS-default quota rows are misbound (fixture service-quotas-catalog-list-aws-default-service-quotas-timestream, step task#1).

AWS: Smithy ListAWSDefaultServiceQuotas(ServiceCode required, @length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; NextToken @length 0..2048; MaxResults @range 1..100) -> ListAWSDefaultServiceQuotasResponse { Quotas: list<ServiceQuota>, NextToken }, paginated (inputToken NextToken / outputToken NextToken / items Quotas / pageSize MaxResults). Smithy: ServiceQuota members ServiceCode, ServiceName, QuotaArn, QuotaCode (@length 1..128, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$), QuotaName, Value (double @range 0..10000000000), Unit, Adjustable (boolean, @default false), GlobalQuota (@default false), UsageMetric (MetricInfo), Period (QuotaPeriod), Description (@length 0..350, @pattern ^.{0,350}$). Modelled errors: AccessDeniedException 403, IllegalArgumentException 400, InvalidPaginationTokenException 400, NoSuchResourceException 404, TooManyRequestsException 429, ServiceException 500 - none applies; AWS answers HTTP 200 with the complete AWS-published default quota set for the requested ServiceCode. WHICH QuotaCodes exist for a ServiceCode and what each one's QuotaName/Description/Unit/Value/Adjustable/GlobalQuota/Period/UsageMetric are is not stated by the model, so for that content this is recorded behavior only. The recording shows, for ServiceCode 'timestream': each QuotaCode identifies exactly one quota definition, and an API request-rate quota additionally carries Period {PeriodUnit 'SECOND', PeriodValue 1} and UsageMetric {MetricNamespace 'AWS/Usage', MetricName 'CallCount', MetricStatisticRecommendation 'Sum', MetricDimensions {Service 'Timestream', Type 'API', Class 'None', Resource <the API name>}}, with Unit 'None'. Examples from the recording: L-C0E9F2ED is QuotaName 'ExecuteScheduledQuery request rate', Description 'Maximum number of ExecuteScheduledQuery requests per second.', Unit 'None', Value 5.0, Period {SECOND,1}, UsageMetric Resource 'ExecuteScheduledQuery'; L-8DFD99E7 is QuotaName 'UpdateAccountSettings request rate', Description 'Maximum number of UpdateAccountSettings requests per second.', Unit 'None', Value 1.0, Adjustable false, Period {SECOND,1}, UsageMetric Resource 'UpdateAccountSettings'. QuotaArn is 'arn:aws:servicequotas:<region>::timestream/<QuotaCode>' - the region/account part is an environment value, the '<ServiceCode>/<QuotaCode>' tail is the requirement.

Emulator: HTTP 200 (expected 200 - the divergence is entirely in the response body). For 'timestream' it returns a Quotas list in which 26 of the recorded QuotaCodes carry another quota's definition and 2 are absent. Concretely, under L-C0E9F2ED it serves QuotaName 'Query timeout' / Description 'The time, in minutes, after which a running query is cancelled.' / Unit 'Minutes' / Value 60.0 and NO Period and NO UsageMetric; under L-8DFD99E7 it serves QuotaName 'Magnetic store retention' / Description 'The maximum time, in days, that a table retains data in its magnetic store.' / Unit 'Days' / Value 73000.0 / Adjustable true instead of the UpdateAccountSettings rate quota. The same substitution is reported for L-00158752, L-05CF2F29, L-1222731D, L-279D7B08, L-3F031494, L-6044E01C, L-6B1915C6, L-6C25862A, L-71D80F70, L-7565C2A8, L-7C9FE50E, L-8892F7F6, L-89E3B36D, L-8BF444C0, L-8FD9593E, L-9F20DF3A, L-A83B13DB, L-AE604CE2, L-BFCD0C9E, L-F1AC16A2, L-F98EACE7, L-FA296006, L-FD5A0C1A and L-FEB34FC6 (mismatching Description, QuotaName, Value, Unit, Adjustable, and missing Period / UsageMetric or wrong UsageMetric.MetricDimensions.Resource and .Type), and QuotaCodes L-0C6E4BCB and L-16B72159 are missing from the list altogether.

Why: QuotaCode is the stable identifier a client resolves once and then reuses - it is the key for ListServiceQuotas, GetServiceQuota, GetAWSDefaultServiceQuota and RequestServiceQuotaIncrease. A client that looks up a Timestream API throttle by its QuotaCode gets a retention or timeout quota back instead, so it compares a request-per-second budget against a value expressed in Days or Minutes and reads it as orders of magnitude of headroom it does not have; a client that keys off UsageMetric to build the matching CloudWatch AWS/Usage CallCount alarm finds no UsageMetric at all for rate quotas and silently builds no alarm. Both failures are silent: the call succeeds with HTTP 200 and no error to branch on.

Fix: the service-quotas crate's static AWS-default quota catalog for ServiceCode 'timestream' (the table the ListAWSDefaultServiceQuotas handler reads and pages over). Each QuotaCode must map to its own quota definition with every modelled ServiceQuota member populated, the two absent codes must be added, and rate-quota rows must carry Period and UsageMetric. The handler's paging and the response serializer are unchanged.

Traces: fixture service-quotas-catalog-list-aws-default-service-quotas-timestream, step task#1.

Related but NOT covering this ServiceCode: open issue 20261030T104829Z-service-quotas-listawsdefaultservicequotas-default-catalog-misbinds-and-omits-aws-quota.md (same shape of defect for apigateway, appconfig, application-autoscaling, appmesh, apprunner and others) and open issue 20261113T222805Z-service-quotas-airflow-serverless-amplifyuibuilder-aoss-and-7-more-listawsdefaultservicequotas.md (ServiceCodes that return no rows at all). Neither carries the timestream replay key, so this divergence is unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true, /__admin/health answered 200). Cluster verdict: mismatch; expected_status 200, actual_status 200; affected_count 1; stub_path empty; owned_by empty (net-new).

fixture_id service-quotas-catalog-list-aws-default-service-quotas-timestream, step_key task#1, divergence_key service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-00158752"].Adjustable,...,$.Quotas[QuotaCode="L-FEB34FC6"].Value

Expected vs actual, verbatim from the replay detail:
  $.Quotas[QuotaCode="L-C0E9F2ED"].QuotaName: value_mismatch (expected 'ExecuteScheduledQuery request rate', got 'Query timeout')
  $.Quotas[QuotaCode="L-C0E9F2ED"].Description: value_mismatch (expected 'Maximum number of ExecuteScheduledQuery requests per second.', got 'The time, in minutes, after which a running query is cancelled.')
  $.Quotas[QuotaCode="L-C0E9F2ED"].Unit: value_mismatch (expected 'None', got 'Minutes')
  $.Quotas[QuotaCode="L-C0E9F2ED"].Value: value_mismatch (expected 5.0, got 60.0)
  $.Quotas[QuotaCode="L-C0E9F2ED"].Period: missing (expected {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, got None)
  $.Quotas[QuotaCode="L-C0E9F2ED"].UsageMetric: missing (expected {'MetricDimensions': {'Class': 'None', 'Resource': 'ExecuteScheduledQuery', 'Service': 'Timestream', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, got None)
  $.Quotas[QuotaCode="L-8DFD99E7"].QuotaName: value_mismatch (expected 'UpdateAccountSettings request rate', got 'Magnetic store retention')
  $.Quotas[QuotaCode="L-8DFD99E7"].Description: value_mismatch (expected 'Maximum number of UpdateAccountSettings requests per second.', got 'The maximum time, in days, that a table retains data in its magnetic store.')
  $.Quotas[QuotaCode="L-8DFD99E7"].Unit: value_mismatch (expected 'None', got 'Days')
  $.Quotas[QuotaCode="L-8DFD99E7"].Value: value_mismatch (expected 1.0, got 73000.0)
  $.Quotas[QuotaCode="L-8DFD99E7"].Adjustable: value_mismatch (expected False, got True)
  $.Quotas[QuotaCode="L-8DFD99E7"].Period: missing (expected {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, got None)
  $.Quotas[QuotaCode="L-8DFD99E7"].UsageMetric: missing (expected {'MetricDimensions': {'Class': 'None', 'Resource': 'UpdateAccountSettings', 'Service': 'Timestream', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, got None)

Whole rows absent (no substitution, the QuotaCode is simply not in the list): $.Quotas[QuotaCode="L-0C6E4BCB"] and $.Quotas[QuotaCode="L-16B72159"].

Rows whose UsageMetric is present but mis-projected: $.Quotas[QuotaCode="L-1222731D"].UsageMetric.MetricDimensions.Resource / .MetricDimensions.Type / .MetricName / .MetricStatisticRecommendation and $.Quotas[QuotaCode="L-FD5A0C1A"] likewise, plus .Period.PeriodUnit / .Period.PeriodValue on both.

No HTTP status, error code or error message is at stake here: both sides answer 200, so the requirement at issue is purely the QuotaCode -> quota-definition binding and the completeness of each ServiceQuota row. The region/account segment of QuotaArn is an environment value and is not asserted.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm /__admin/health answers 200.
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-timestream, step task#1: POST / to the service-quotas endpoint with X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode":"timestream"}.
3. Observe HTTP 200, then index the returned Quotas list by QuotaCode.
4. Look up QuotaCode L-C0E9F2ED: the emulator returns QuotaName 'Query timeout' / Unit 'Minutes' / Value 60.0 with no Period and no UsageMetric, where the recording has QuotaName 'ExecuteScheduledQuery request rate' / Unit 'None' / Value 5.0 with Period {SECOND,1} and UsageMetric AWS/Usage CallCount for Resource 'ExecuteScheduledQuery'.
5. Look up QuotaCode L-8DFD99E7: the emulator returns 'Magnetic store retention' / Unit 'Days' / Value 73000.0 / Adjustable true, where the recording has 'UpdateAccountSettings request rate' / Unit 'None' / Value 1.0 / Adjustable false with Period and UsageMetric.
6. Confirm QuotaCodes L-0C6E4BCB and L-16B72159 are absent from the list entirely.
