Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.op-add-thing-to-missing-group-rejected iot.stream-delete-conflict-when-referenced-by-ota-rejected iot.certificate-provider-delete-conflict-when-referenced-rejected iot.provisioning-template-version-lifecycle-and-default iot.topic-rule-lifecycle-disable-enable iot.negative-domain-configuration-invalid-certificate-blocked iot.fleet-metric-invalid-aggregation-rejected iot.telemetry-fleet-metric-invalid-aggregation-rejected iot.fleet-metric-query-string-preserved
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.op-add-thing-to-missing-group-rejected|run#8|AssertionError|assert message == (
Issue-Key: iot.stream-delete-conflict-when-referenced-by-ota-rejected|run#15|AssertionError|assert dup_code_message == f"Requested stream {stream_id} stream already exists.", (
Issue-Key: iot.certificate-provider-delete-conflict-when-referenced-rejected|run#13|AssertionError|assert code == ("ResourceAlreadyExistsException", f"Certificate provider [{provider_name}] already exists for this account"), (
Issue-Key: iot.provisioning-template-version-lifecycle-and-default|run#22|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.topic-rule-lifecycle-disable-enable|run#20|AssertionError|assert message == f"Access to topic rule '{rule_name}' was denied", message
Issue-Key: iot.negative-domain-configuration-invalid-certificate-blocked|run#13|AssertionError|assert not_found_message == (
Issue-Key: iot.fleet-metric-invalid-aggregation-rejected|run#11|AssertionError|assert not_found_message == expected_not_found, (
Issue-Key: iot.telemetry-fleet-metric-invalid-aggregation-rejected|run#15|AssertionError|assert not_found_code_message == f"An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric {ctx.account}:{bad_
Issue-Key: iot.fleet-metric-query-string-preserved|run#16|AssertionError|assert message == (
Issue-Commit: dafb6a92f3304d60364f819845fa972c02211566

# iot refusal-message fidelity: 7 causes — AddThingToThingGroup, CreateStream, CreateCertificateProvider, DescribeProvisioningTemplateVersion, GetTopicRule, DescribeDomainConfiguration and DescribeFleetMetric refusals are emulator-authored text

> _Note: this issue was identified against the code base at commit `dafb6a92f3304d60364f819845fa972c02211566`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddThingToThingGroup

## Description
Seven IoT control-plane refusals return the correct HTTP status and error code but a message the emulator wrote itself instead of the text AWS sends for that error class. Each cause is a different operation and a different handler, so they close independently. In every case the error code already matches; only the Message member diverges.

Cause 1 of 7: AddThingToThingGroup on a thing group that does not exist.
  AWS: Smithy: AddThingToThingGroup -> PUT /thing-groups/addThingToThingGroup, errors include ResourceNotFoundException (httpError 404). Recorded behavior: the message is the fixed text "Cannot find requested thing group for accountId: " + <caller's account id> + " and thing group name: " + <the thingGroupName from the request>. Fixed text: "Cannot find requested thing group for accountId: " and " and thing group name: "; variable parts: the account id from the caller's identity and the group name from the request.
  Emulator: 404 ResourceNotFoundException with "ThingGroup <account>:<groupName> not found." — the registry-style "<account>:<name> not found." template, which AWS uses for DescribeThingGroup, not for this operation.
  Why: the refusal is correctly classified but a client that matches on the message to distinguish "the group is missing" from other 404s on the same code, or that surfaces it to an operator, shows wording AWS never sends; the same emulator text is now served for two operations AWS words differently.
  Fix: the AddThingToThingGroup handler's not-found path.
  Traces: iot.op-add-thing-to-missing-group-rejected (the check's own DescribeThingGroup assertion, which does expect the "ThingGroup <account>:<name> not found." form, passed).

Cause 2 of 7: CreateStream with a streamId that already exists.
  AWS: Smithy: CreateStream -> POST /streams/{streamId}, errors include ResourceAlreadyExistsException (httpError 409). Recorded behavior: the message is "Requested stream " + <streamId> + " stream already exists." Fixed text: "Requested stream ", " stream already exists."; variable part: the streamId from the request.
  Emulator: 409 ResourceAlreadyExistsException with "Stream <streamId> already exists" — different wording and no trailing period.
  Why: a caller that keys idempotent create-or-reuse logic on the conflict message, or logs it, sees text AWS never emits.
  Fix: the CreateStream handler's duplicate-id path. Related open issue 20261109T163411Z-cloudformation-iam-iot-s3-iot-createstream-answers-notimplemented-so-aws-iot-stream.md reports CreateStream answering NotImplemented; in this replay CreateStream is implemented and answers 409, so that issue does not cover this wording. 20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md covers other operations' already-exists text, not CreateStream's.
  Traces: iot.stream-delete-conflict-when-referenced-by-ota-rejected.

Cause 3 of 7: CreateCertificateProvider with a name that already exists.
  AWS: recorded behavior only for the text; the code is ResourceAlreadyExistsException. Recorded behavior: "Certificate provider [" + <certificateProviderName> + "] already exists for this account" — note the square brackets around the name and the trailing "for this account". Fixed text: "Certificate provider [", "] already exists for this account"; variable part: the provider name from the request.
  Emulator: "Certificate provider <name> already exists" — no brackets, no account clause.
  Why: same class of defect; the message is the only part of the refusal that names the conflicting resource in AWS's own form.
  Fix: the CreateCertificateProvider handler's duplicate-name path.
  Traces: iot.certificate-provider-delete-conflict-when-referenced-rejected.

Cause 4 of 7: DescribeProvisioningTemplateVersion for a version that was deleted or never existed.
  AWS: Smithy: DescribeProvisioningTemplateVersion -> GET /provisioning-templates/{templateName}/versions/{versionId}, errors include ResourceNotFoundException (httpError 404). Recorded behavior: "Unable to find template version " + <versionId>. Fixed text: "Unable to find template version "; variable part: the versionId from the request. AWS does not name the template in this message.
  Emulator: "Unable to find a template version with ID <versionId> for template <templateName>" — reworded and carrying a resource name AWS leaves out.
  Fix: the DescribeProvisioningTemplateVersion handler's not-found path (and DeleteProvisioningTemplateVersion if it shares the text).
  Traces: iot.provisioning-template-version-lifecycle-and-default (the version lifecycle itself — v2 becomes versionId 2 and the default, v1 deletes cleanly — passed; only the message diverges).

Cause 5 of 7: GetTopicRule for a rule name that does not exist.
  AWS: Smithy: GetTopicRule -> GET /rules/{ruleName}; its modeled errors are InternalException, InvalidRequestException, ServiceUnavailableException and UnauthorizedException (httpError 401) — there is no ResourceNotFoundException, which is why AWS answers a missing rule with UnauthorizedException. Recorded behavior: the message is "Access to topic rule '" + <ruleName> + "' was denied". Fixed text: "Access to topic rule '" and "' was denied"; variable part: the rule name from the request.
  Emulator: UnauthorizedException (code matches) with "Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:<region>:<account>:rule/<ruleName>" — an IAM-style denial message, and it leaks a constructed rule ARN.
  Fix: the GetTopicRule handler's unknown-rule path.
  Traces: iot.topic-rule-lifecycle-disable-enable (create/disable/re-enable via ReplaceTopicRule and the post-delete UnauthorizedException code all passed).

Cause 6 of 7: DescribeDomainConfiguration for a configuration that does not exist.
  AWS: Smithy: DescribeDomainConfiguration -> GET /domainConfigurations/{domainConfigurationName}, errors include ResourceNotFoundException (httpError 404). Recorded behavior: "Could not find domain configuration for given account ID and domain configuration name, AccountId: " + <account id> + ", DomainConfigurationName: " + <name>. Fixed text: "Could not find domain configuration for given account ID and domain configuration name, AccountId: " and ", DomainConfigurationName: "; variable parts: the caller's account id and the requested name.
  Emulator: 404 ResourceNotFoundException with "Domain configuration with name [<name>] not found".
  Fix: the DescribeDomainConfiguration handler's not-found path. (The same check's CreateDomainConfiguration leg — InvalidRequestException "Certificate <arn> does not exist." for ACM ARNs that match nothing — passed, so this is purely the describe message.)
  Traces: iot.negative-domain-configuration-invalid-certificate-blocked.

Cause 7 of 7: DescribeFleetMetric for a metric that does not exist — the Message member carries only the tail of the recorded text.
  AWS: Smithy: DescribeFleetMetric -> GET /fleet-metric/{metricName}, errors include ResourceNotFoundException (httpError 404). Recorded behavior: three independent checks assert the Message member equals "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric " + <account>:<metricName> + " not found."
  Emulator: 404 ResourceNotFoundException with "FleetMetric <account>:<metricName> not found." — identical to the tail of the recorded text, missing the leading "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: " segment.
  Why: a client comparing the message against the recorded text does not match. Hypothesis: that leading segment is how botocore renders a ClientError's str(), so the recorded expectation may encode the rendered exception rather than the Message member — in which case the emulator's tail is already correct and nothing in these three checks diverges beyond this comparison. A fixer should confirm against the recording BEFORE prepending that prefix: emitting it inside the Message member would itself be a divergence from AWS, since AWS never duplicates the operation name inside the error message. This cause is filed to carry the three checks' replay keys and to get that determination made, not to assert the prefix as a requirement.
  Fix: the DescribeFleetMetric handler's not-found message (only if the recording shows the prefix is part of the Message member).
  Traces: iot.fleet-metric-invalid-aggregation-rejected, iot.telemetry-fleet-metric-invalid-aggregation-rejected, iot.fleet-metric-query-string-preserved.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service heading '### iot'). All seven responses carry the correct HTTP status and error code; only the Message member diverges.

Cause 1 — iot.op-add-thing-to-missing-group-rejected [mismatch]:
  expected: "Cannot find requested thing group for accountId: <account> and thing group name: <missingGroup>"
  actual:   "ThingGroup <account>:<missingGroup> not found."  (code ResourceNotFoundException, as expected)

Cause 2 — iot.stream-delete-conflict-when-referenced-by-ota-rejected [mismatch]:
  expected: "Requested stream <streamId> stream already exists."
  actual:   "Stream <streamId> already exists"  (code ResourceAlreadyExistsException, as expected)

Cause 3 — iot.certificate-provider-delete-conflict-when-referenced-rejected [mismatch]:
  expected: "Certificate provider [<name>] already exists for this account"
  actual:   "Certificate provider <name> already exists"  (code ResourceAlreadyExistsException, as expected)

Cause 4 — iot.provisioning-template-version-lifecycle-and-default [mismatch]:
  expected: "Unable to find template version 1"
  actual:   "Unable to find a template version with ID 1 for template <templateName>"

Cause 5 — iot.topic-rule-lifecycle-disable-enable [mismatch]:
  expected: "Access to topic rule '<ruleName>' was denied"
  actual:   "Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:<region>:<account>:rule/<ruleName>"  (code UnauthorizedException, as expected)

Cause 6 — iot.negative-domain-configuration-invalid-certificate-blocked [mismatch]:
  expected: "Could not find domain configuration for given account ID and domain configuration name, AccountId: <account>, DomainConfigurationName: <name>"
  actual:   "Domain configuration with name [<name>] not found"  (code ResourceNotFoundException, as expected)

Cause 7 — iot.fleet-metric-invalid-aggregation-rejected, iot.telemetry-fleet-metric-invalid-aggregation-rejected, iot.fleet-metric-query-string-preserved [all mismatch]:
  expected: "An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric <account>:<metricName> not found."
  actual:   "FleetMetric <account>:<metricName> not found."  (code ResourceNotFoundException, as expected)
  In these three checks the preceding legs passed, including CreateFleetMetric's InvalidRequestException "Invalid values [average] for Statistics aggregation on String field. Supported values are [count]" and the character-for-character queryString round trip.

The group names, stream ids, provider names, rule names, metric names, account ids and regions above are per-run values produced by each check's own setup; the requirement is the fixed text of each message plus the request/state values it interpolates, as named per cause.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — AddThingToThingGroup (PUT /thing-groups/addThingToThingGroup, ResourceNotFoundException 404), CreateStream (POST /streams/{streamId}, ResourceAlreadyExistsException 409), DescribeProvisioningTemplateVersion (GET /provisioning-templates/{templateName}/versions/{versionId}, ResourceNotFoundException 404), GetTopicRule (GET /rules/{ruleName}; errors InternalException/InvalidRequestException/ServiceUnavailableException/UnauthorizedException 401 — no ResourceNotFoundException), DescribeDomainConfiguration (GET /domainConfigurations/{domainConfigurationName}, ResourceNotFoundException 404), DescribeFleetMetric (GET /fleet-metric/{metricName}, ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: CreateThing(thingName=T), then AddThingToThingGroup(thingGroupName=<a name never created>, thingName=T) and read Error.Message.
3. Cause 2: CreateStream(streamId=S, files=[{fileId:0, s3Location:{bucket,key}}], roleArn=R) twice; read the second call's Error.Message.
4. Cause 3: CreateCertificateProvider(certificateProviderName=P, lambdaFunctionArn=..., accountDefaultForOperations=['CreateCertificateFromCsr']) twice; read the second call's Error.Message.
5. Cause 4: CreateProvisioningTemplate(templateName=T), CreateProvisioningTemplateVersion(templateName=T, setAsDefault=true), DeleteProvisioningTemplateVersion(templateName=T, versionId=1), then DescribeProvisioningTemplateVersion(templateName=T, versionId=1) and read Error.Message.
6. Cause 5: CreateTopicRule(ruleName=R, ...), DeleteTopicRule(ruleName=R), then GetTopicRule(ruleName=R) and read Error.Message.
7. Cause 6: DescribeDomainConfiguration(domainConfigurationName=<a name never created>) and read Error.Message.
8. Cause 7: enable thing indexing, CreateFleetMetric(metricName=M, ...), DeleteFleetMetric(metricName=M), then DescribeFleetMetric(metricName=M) and compare Error.Message against the recorded text — and determine from the recording whether the "An error occurred (...) when calling the ... operation: " segment belongs to the Message member or is botocore's rendering of the exception.
9. Re-replay the nine checks in trace_ids to confirm each cause is closed.
