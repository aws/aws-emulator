Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-timestream
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-timestream
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="DatabasesPerAccount"]
Issue-Related: 20261030T005329Z-service-quotas-defaults-for-sns-sqs-sso-storagegateway.md
Issue-Commit: 7c1c192a4be1fd26f3761e762e61646b8dff0eae

# Default quota catalog serves no AWS timestream quota rows and substitutes non-L QuotaCodes

> _Note: this issue was identified against the code base at commit `7c1c192a4be1fd26f3761e762e61646b8dff0eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
ONE root cause.

Cause 1 of 1: ServiceCode timestream.
AWS: ListAWSDefaultServiceQuotas for ServiceCode timestream returns the AWS-issued L-coded default quotas for Amazon Timestream — about thirty complete ServiceQuota rows, each with ServiceCode 'timestream', ServiceName 'Amazon Timestream', QuotaArn of the form arn:aws:servicequotas:<region>::timestream/<QuotaCode>, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota, and for the request-rate quotas also Period and UsageMetric. Examples from the recording: L-F07A9AB4 'ListBatchLoadTasks request rate' (Description 'Maximum number of ListBatchLoadTasks requests per second.', Value 5.0, Period SECOND/1, UsageMetric MetricName 'CallCount' in MetricNamespace 'AWS/Usage' with MetricDimensions {Class 'None', Resource 'ListBatchLoadTasks', Service 'Timestream', Type 'API'}, MetricStatisticRecommendation 'Sum'); L-798CB19D 'CreateScheduledQuery request rate' (Value 1.0); L-C49D398A 'CreateBatchLoadTask request rate'. The full expected code set in this step is L-00158752, L-05CF2F29, L-0C6E4BCB, L-1222731D, L-16B72159, L-279D7B08, L-3F031494, L-6044E01C, L-6B1915C6, L-6C25862A, L-71D80F70, L-7565C2A8, L-798CB19D, L-7C9FE50E, L-8892F7F6, L-89E3B36D, L-8BF444C0, L-8DFD99E7, L-8FD9593E, L-9F20DF3A, L-A83B13DB, L-AE604CE2, L-BFCD0C9E, L-C0E9F2ED, L-C49D398A, L-F07A9AB4, L-F1AC16A2, L-F98EACE7, L-FA296006, L-FD5A0C1A and L-FEB34FC6. Smithy: ListAWSDefaultServiceQuotas -> Quotas: list<ServiceQuota>; ServiceQuota.QuotaCode is @length(1,128) @pattern(^[a-zA-Z][a-zA-Z0-9-]{1,128}$) and is the stable identity other operations (GetServiceQuota, RequestServiceQuotaIncrease) take.
Emulator: the call returns 200 with a well-formed Quotas list, but NOT ONE of those AWS rows is present — every expected QuotaCode above resolves to nothing. In their place the catalog carries invented identifiers that are not AWS QuotaCodes at all (the recording's comparison flags an entry keyed 'DatabasesPerAccount'), so the response is a whole-row substitution rather than a field-level drift.
Why: a client cannot discover any real Timestream quota: GetServiceQuota/RequestServiceQuotaIncrease with a documented L-code finds nothing in the listing, and the identifiers that ARE returned do not exist in AWS, so any code path that round-trips a QuotaCode from this listing into another Service Quotas call breaks against the real service. Utilization tooling additionally gets no Period/UsageMetric to build its CloudWatch query from.
Fix: the default-quota catalog data for ServiceCode timestream in the service-quotas crate (the seed table the ListAWSDefaultServiceQuotas handler reads) — replace the invented, non-L identifiers with the AWS code set and their Description/Value/Unit/Adjustable/GlobalQuota/Period/UsageMetric fields. The handler, its pagination and its serializer frame the list correctly and are not at fault.
Traces: service-quotas-catalog-list-aws-default-service-quotas-timestream#task#1.

The open issues 20261030T005329Z-service-quotas-defaults-for-sns-sqs-sso-storagegateway.md and the other 20261030T005329Z / 20261029T203936Z default-catalog issues mention 'timestream' only among other ServiceCodes and do not carry this step's replay key, so this cause is still open and unexercised by them.

## Evidence
Replay of trace set simparity-wire-aws-v1, one divergence cluster: service=service-quotas, operation=ListAWSDefaultServiceQuotas, verdict=mismatch, expected_status=200, actual_status=200, fixture service-quotas-catalog-list-aws-default-service-quotas-timestream, step task#1 (affected_count=1).

Detail (whole rows absent, not field drift):
- $.Quotas[QuotaCode="L-F07A9AB4"]: missing — expected {'Adjustable': False, 'Description': 'Maximum number of ListBatchLoadTasks requests per second.', 'GlobalQuota': False, 'Period': {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::timestream/L-F07A9AB4', 'QuotaCode': 'L-F07A9AB4', 'QuotaName': 'ListBatchLoadTasks request rate', 'ServiceCode': 'timestream', 'ServiceName': 'Amazon Timestream', 'Unit': 'None', 'UsageMetric': {'MetricDimensions': {'Class': 'None', 'Resource': 'ListBatchLoadTasks', 'Service': 'Timestream', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, 'Value': 5.0}, got None.
- $.Quotas[QuotaCode="L-798CB19D"]: missing — expected 'CreateScheduledQuery request rate', Value 1.0, Period SECOND/1, UsageMetric CallCount/AWS/Usage.
- $.Quotas[QuotaCode="L-C49D398A"]: missing — expected 'CreateBatchLoadTask request rate', Period SECOND/1, UsageMetric Resource 'CreateBatchLoadTask'.
- Same 'missing' verdict for the remaining expected codes: L-00158752, L-05CF2F29, L-0C6E4BCB, L-1222731D, L-16B72159, L-279D7B08, L-3F031494, L-6044E01C, L-6B1915C6, L-6C25862A, L-71D80F70, L-7565C2A8, L-7C9FE50E, L-8892F7F6, L-89E3B36D, L-8BF444C0, L-8DFD99E7, L-8FD9593E, L-9F20DF3A, L-A83B13DB, L-AE604CE2, L-BFCD0C9E, L-C0E9F2ED, L-F1AC16A2, L-F98EACE7, L-FA296006, L-FD5A0C1A, L-FEB34FC6.
- The comparison additionally flags $.Quotas[QuotaCode="DatabasesPerAccount"] — an identifier the emulator serves that is not an AWS QuotaCode for this catalog.

Status and envelope are correct (200 -> 200, valid Quotas list); only the catalog contents diverge. Model evidence: ServiceQuota.QuotaCode @length(1,128) @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$; Description @length(0,350); Value QuotaValue double @range(0,10000000000); Period QuotaPeriod; UsageMetric MetricInfo.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-timestream, step task#1 — POST the Service Quotas endpoint with X-Amz-Target ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode":"timestream"}, following NextToken to the end of the listing.
3. Collect the QuotaCode of every returned ServiceQuota and compare against the recorded set (L-F07A9AB4, L-798CB19D, L-C49D398A and the other 28 L-codes listed in the evidence).
4. Observe that none of the recorded L-codes appears and that non-AWS identifiers such as 'DatabasesPerAccount' are served instead; AWS returns the complete L-coded Timestream catalog with Description, Value, Unit, Adjustable, GlobalQuota and, for rate quotas, Period and UsageMetric.
