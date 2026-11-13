Issue-Source: trace-sweep-divergence
Issue-Trace: logs-catalog-describe-configuration-templates
Issue-Step: task#30
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-catalog-describe-configuration-templates logs-b2-delivery-source-destination-pipeline logs-end-to-end-delivery-pipeline-create-configure-update logs-agentcore-traj-rewire-delivery-for-deleted-runtime
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateDelivery|2xx->2xx|$.delivery.recordFields[*]
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.recordFields[*]
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.s3DeliveryConfiguration.suffixPath
Issue-Related: 20261111T095441Z-logs-describeconfigurationtemplates-catalog-omits-allowedfielddelimiters-allowedsuffixpathfields-and-fielddelimiter-and.md
Issue-Commit: b18a62dd9722f81a5987bef5e25a27694a55571a

# Vended-log delivery defaults are not taken from the configuration template: workmail, qbusiness and securityhub catalog entries plus CreateDelivery/GetDelivery recordFields and suffixPath diverge (5 causes)

> _Note: this issue was identified against the code base at commit `b18a62dd9722f81a5987bef5e25a27694a55571a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
Three more per-service entries of the vended-log configuration-template catalog diverge from AWS, and — the same defect seen from the write side — CreateDelivery/GetDelivery do not apply the template's defaultDeliveryConfigValues when the request omits recordFields and s3DeliveryConfiguration, so the delivery AWS would create and the delivery the emulator creates are different resources.

Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): CreateDelivery input {deliverySourceName (required), deliveryDestinationArn (required), recordFields (RecordFields, length 0..128), fieldDelimiter, s3DeliveryConfiguration, tags} -> output {delivery: Delivery}; GetDelivery input {id (required)} -> output {delivery: Delivery}. Delivery members include recordFields and s3DeliveryConfiguration (S3DeliveryConfiguration{suffixPath @length min 1 max 256, enableHiveCompatiblePath}). ConfigurationTemplateDeliveryConfigValues.recordFields is documented as "the default record fields that will be delivered when a list of record fields is not provided in a CreateDelivery", and S3DeliveryConfiguration.suffixPath documents that the valid variables "vary by log type" and are found via DescribeConfigurationTemplates' allowedSuffixPathFields. The concrete catalog contents are recorded behavior only.

No open issue covers this: the DescribeConfigurationTemplates catalog has none, and the open delivery-related logs issues (20261030T193849Z-logs-delivery-destination-policy-delete-and-read-back.md, 20261102T090835Z-bedrock-agentcore-control-logs-agentcore-runtime-vended-log-group-is-never-created.md) are about destination policies and the vended log group, not about delivery defaults. The companion bundle filed in this same sweep ("DescribeConfigurationTemplates catalog omits allowedFieldDelimiters ...") carries ten further catalog causes; these five are separate causes and must be closed on their own.

Cause 1 of 5 — workmail templates: allowedOutputFormats truncated, allowedFieldDelimiters and allowedSuffixPathFields dropped.
AWS: the workmail ACCESS_CONTROL_LOGS/organization S3 template carries allowedOutputFormats ['plain','json','w3c','parquet'], allowedFieldDelimiters ["\t", " ", ","] and allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'] (recorded behavior only).
Emulator: HTTP 200 with allowedOutputFormats ['json'] and neither allowedFieldDelimiters nor allowedSuffixPathFields; 15 templates in the workmail page diverge.
Why: a client that validates its outputFormat or its suffixPath variables against this page is told that only 'json' and no suffix variables are legal, so legal AWS requests are rejected client-side.
Fix: the DescribeConfigurationTemplates handler's catalog entries plus the ConfigurationTemplate serializer.
Traces: logs-catalog-describe-configuration-templates#task#30.

Cause 2 of 5 — the qbusiness EVENT_LOGS/application CWL template is not the entry served at that position.
AWS: DescribeConfigurationTemplates in the delivery-source/destination pipeline returns, at configurationTemplates[1], the qbusiness EVENT_LOGS/application CWL template (allowedActionForAllowVendedLogsDeliveryForResource 'qbusiness:AllowVendedLogDeliveryForResource', allowedFieldDelimiters ["\t", " ", ","], allowedOutputFormats ['plain','json'], a 14-entry defaultDeliveryConfigValues.recordFields list starting application_id, event_timestamp, log_type, account_id, and defaultDeliveryConfigValues.fieldDelimiter '') (recorded behavior only).
Emulator: HTTP 200 with a template for service 'aidevops' at that position instead.
Why: a client enumerating the catalog to pick the Q Business event-log template for its delivery never finds it, and the entry it does find belongs to another service.
Fix: the DescribeConfigurationTemplates handler's catalog contents and ordering.
Traces: logs-b2-delivery-source-destination-pipeline#task#2.

Cause 3 of 5 — securityhub hub/hubv2 templates are swapped and their allowedOutputFormats and delimiters are wrong.
AWS: for service 'securityhub', configurationTemplates[0] is the SECURITY_FINDING_LOGS template for resourceType 'hubv2' and [1] the one for resourceType 'hub'; each carries allowedOutputFormats ['plain'], allowedFieldDelimiters ["\t", " ", ","] and defaultDeliveryConfigValues.fieldDelimiter '' (recorded behavior only).
Emulator: HTTP 200 with the two resourceTypes in the opposite order, allowedOutputFormats ['json'], no allowedFieldDelimiters and no fieldDelimiter.
Why: a client that takes the first SECURITY_FINDING_LOGS template, or that validates outputFormat 'plain', binds its delivery to the wrong Security Hub resource type or refuses the only format AWS allows.
Fix: the DescribeConfigurationTemplates handler's catalog contents/ordering and the ConfigurationTemplate serializer.
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#setup#3.

Cause 4 of 5 — CreateDelivery does not default recordFields from the configuration template.
AWS: a CreateDelivery that omits recordFields gets the template's defaultDeliveryConfigValues.recordFields. For the AgentCore runtime APPLICATION_LOGS source that is the ordered list resource_arn, event_timestamp, account_id, request_id, session_id, span_id, trace_id, service_name, operation, request_payload, response_payload, resource, attributes, timeUnixNano, severityNumber, severityText, body, traceId, spanId — 19 fields, returned in delivery.recordFields (Smithy: CreateDelivery -> Delivery.recordFields; ConfigurationTemplateDeliveryConfigValues.recordFields documented as the default "when a list of record fields is not provided in a CreateDelivery").
Emulator: HTTP 200 but delivery.recordFields is only the six-element tail ['timeUnixNano','severityNumber','severityText','body','traceId','spanId'] — indices 0..5 hold the wrong names and indices 6..18 are absent.
Why: the created delivery emits six OTLP-shaped columns instead of the 19 AWS vends, so every downstream consumer of the delivered records loses resource_arn, event_timestamp, account_id, request_id, session_id, trace/span ids, service_name, operation and both payload columns; a client that reads back recordFields to render or diff its delivery sees a configuration it never asked for.
Fix: CreateDelivery's handler — the default-value resolution that should read defaultDeliveryConfigValues from the matching configuration template before persisting the Delivery.
Traces: logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#5 (cluster also covers task#6).

Cause 5 of 5 — GetDelivery returns an empty s3DeliveryConfiguration.suffixPath, violating the member's minimum length, and repeats the wrong recordFields.
AWS: GetDelivery returns the delivery as created, so an S3 delivery created without an explicit suffixPath reads back with the template default '{region}/{yyyy}/{MM}/{dd}/{HH}/', and recordFields reads back as the 19-field default of cause 4. Smithy: S3DeliveryConfiguration.suffixPath is @length(min: 1, max: 256), so an empty string is not a value AWS can return.
Emulator: HTTP 200 with delivery.s3DeliveryConfiguration.suffixPath '' (empty string) and the same six-element recordFields tail as cause 4.
Why: a client that reads the delivery back to learn where its objects land gets an empty prefix — it cannot compute the S3 object key — and an SDK enforcing the modelled minimum length fails to deserialize the response.
Fix: GetDelivery's serializer and the Delivery state record it reads — the stored delivery must carry the template's default suffixPath rather than an empty string.
Traces: logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (186 fixtures, 1217 steps, 1057 passed). Five net-new divergence clusters, all HTTP 200 vs 200 with body mismatches:

1. logs-catalog-describe-configuration-templates#task#30 — DescribeConfigurationTemplates(service=workmail): $.configurationTemplates[0]..[14] mismatch; expected allowedOutputFormats ['plain','json','w3c','parquet'] + allowedFieldDelimiters ['\t',' ',','] + allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'], got allowedOutputFormats ['json'] and neither of the other two members.
2. logs-b2-delivery-source-destination-pipeline#task#2 — DescribeConfigurationTemplates: $.configurationTemplates[1] expected the qbusiness EVENT_LOGS/application CWL template ('qbusiness:AllowVendedLogDeliveryForResource', allowedOutputFormats ['plain','json'], 14 default recordFields), got {'service': 'aidevops', ...}.
3. logs-end-to-end-delivery-pipeline-create-configure-update#setup#3 — DescribeConfigurationTemplates(service=securityhub): $.configurationTemplates[0] expected resourceType 'hubv2' / allowedOutputFormats ['plain'], got resourceType 'hub' / ['json']; $.configurationTemplates[1] expected 'hub', got 'hubv2'; allowedFieldDelimiters and defaultDeliveryConfigValues.fieldDelimiter absent in both.
4. logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#5 (also task#6) — CreateDelivery: $.delivery.recordFields[0] expected 'resource_arn' got 'timeUnixNano'; [1] expected 'event_timestamp' got 'severityNumber'; [2] expected 'account_id' got 'severityText'; [3] expected 'request_id' got 'body'; [4] expected 'session_id' got 'traceId'; [5] expected 'span_id' got 'spanId'; [6]..[18] ('trace_id','service_name','operation','request_payload','response_payload','resource','attributes','timeUnixNano','severityNumber','severityText','body','traceId','spanId') all missing (got None).
5. logs-agentcore-traj-rewire-delivery-for-deleted-runtime#task#7 — GetDelivery: the same recordFields[0]..[18] divergence, plus $.delivery.s3DeliveryConfiguration.suffixPath expected '{region}/{yyyy}/{MM}/{dd}/{HH}/', got '' (an empty string, below the modelled @length min 1).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture logs-catalog-describe-configuration-templates step task#30 (DescribeConfigurationTemplates for service workmail) and compare allowedOutputFormats, allowedFieldDelimiters and allowedSuffixPathFields with the recording.
3. Replay fixture logs-b2-delivery-source-destination-pipeline step task#2 and inspect configurationTemplates[1] — the qbusiness EVENT_LOGS/application template is not there.
4. Replay fixture logs-end-to-end-delivery-pipeline-create-configure-update step setup#3 and compare the order of the securityhub 'hub' / 'hubv2' templates and their allowedOutputFormats.
5. Replay fixture logs-agentcore-traj-rewire-delivery-for-deleted-runtime: at step task#5 call CreateDelivery for the AgentCore runtime APPLICATION_LOGS delivery source with an S3 delivery destination and WITHOUT recordFields or s3DeliveryConfiguration, and compare delivery.recordFields with the recorded 19-field default.
6. In the same fixture, step task#7, call GetDelivery with that delivery's id and inspect delivery.s3DeliveryConfiguration.suffixPath (empty string instead of '{region}/{yyyy}/{MM}/{dd}/{HH}/') and delivery.recordFields.

## Resolution

Every replay key this report carries is `fixed`: `CreateDelivery`/`GetDelivery` `recordFields` and
`GetDelivery` `s3DeliveryConfiguration.suffixPath`.

- Causes 4 and 5 are FIXED. A delivery's OBJECT LAYOUT is the one its SENDER publishes — an object
  layout is published per log type, so it is read off the template for delivering those logs to an
  S3 bucket and reported whatever kind of destination the delivery pairs with, instead of being
  lost for every delivery to a log group or a stream
  (`src/state/delivery_configuration.rs::delivery_object_layout`,
  `src/state/deliveries.rs::resolve_configuration`). An AgentCore resource's published record is
  the vended-log envelope — the resource, account, request, session, span, trace, service,
  operation and payloads — followed by the OpenTelemetry log record it envelopes, so a delivery
  created without `recordFields` carries the whole 19-field default rather than the OTLP tail
  alone (`src/state/vended_log_templates.rs`).
- Cause 1 is FIXED: at this commit the WorkMail S3 template already publishes
  `allowedOutputFormats` ['plain','json','w3c','parquet'], `allowedFieldDelimiters` and
  `allowedSuffixPathFields`, pinned by `tests/all/vended_log_template_schemas.rs` and
  `tests/all/configuration_templates.rs`.
- Cause 2 is FIXED for the missing row: the catalogue now publishes the Amazon Q Business
  `application` templates for both log types that sender sends, so the slice a client enumerates
  carries them and a delivery source can be created for each.
- Cause 3 is FIXED for the ordering: the newer Security Hub hub resource is published first.
- What remains of causes 2 and 3 — which record fields, output formats and field delimiters each
  catalogue ROW publishes, and where a row sits in the whole-catalogue enumeration — is one rule
  across the catalogue, carried by the `logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]`
  key, which is NOT among this report's keys: the open issue
  `20261111T095441Z-logs-describeconfigurationtemplates-catalog-omits-allowedfielddelimiters-allowedsuffixpathfields-and-fielddelimiter-and.md`
  owns it and closes that part.
