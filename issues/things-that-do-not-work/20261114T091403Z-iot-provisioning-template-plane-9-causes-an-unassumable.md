Issue-Source: trace-sweep-divergence
Issue-Trace: iot-provisioning-template-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-provisioning-template-error-paths iot-provisioning-template-lifecycle-errors iot-provisioning-template-role-trust-error iot-provisioning-template-not-found-errors iot-register-thing-invalid-template
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateProvisioningTemplate|4xx->2xx|$.message
Issue-Key: iot|CreateProvisioningTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateProvisioningTemplate|4xx->2xx|status
Issue-Key: iot|DescribeProvisioningTemplate|4xx->2xx|$.message
Issue-Key: iot|DescribeProvisioningTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeProvisioningTemplate|4xx->2xx|status
Issue-Key: iot|DescribeProvisioningTemplateVersion|4xx->2xx|$.message
Issue-Key: iot|DescribeProvisioningTemplateVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeProvisioningTemplateVersion|4xx->2xx|status
Issue-Key: iot|CreateProvisioningTemplateVersion|4xx->2xx|$.message
Issue-Key: iot|CreateProvisioningTemplateVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateProvisioningTemplateVersion|4xx->2xx|status
Issue-Key: iot|DeleteProvisioningTemplateVersion|4xx->4xx|$.message
Issue-Key: iot|DeleteProvisioningTemplateVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteProvisioningTemplateVersion|4xx->4xx|status
Issue-Key: iot|DeleteProvisioningTemplate|4xx->2xx|$
Issue-Key: iot|DeleteProvisioningTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteProvisioningTemplate|4xx->2xx|status
Issue-Key: iot|DescribeProvisioningTemplateVersion|4xx->4xx|$.message
Issue-Key: iot|DeleteProvisioningTemplate|4xx->4xx|$.message
Issue-Key: iot|CreateProvisioningTemplate|4xx->4xx|$.message
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot provisioning-template plane: 9 causes — an unassumable provisioning role is accepted, so every follow-on read and delete answers success instead of 404, plus reworded refusals

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProvisioningTemplate

## Description
Nine net-new wire divergences in the IoT fleet-provisioning template plane. The first is the root one: CreateProvisioningTemplate performs no trust-policy pre-flight on provisioningRoleArn, so a template AWS never creates exists in the emulator and five subsequent not-found paths in the same fixture answer success. The remaining three are refusal-message fidelity defects on their own handlers. Nine causes must be closed.

Cause 1 of 9: CreateProvisioningTemplate accepts a provisioning role that AWS IoT cannot assume.
  AWS: Smithy: CreateProvisioningTemplate -> POST /provisioning-templates; provisioningRoleArn is @required (RoleArn, @length min 20 max 2048); modeled errors include InvalidRequestException (httpError 400). Recorded response: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and message 'The provisioning role cannot be assumed by AWS IoT' — that message is entirely fixed text with no variable part. AWS validates that the named role's trust policy lets iot.amazonaws.com assume it BEFORE the template is created.
  Emulator: 200 with a created template; no message and no x-amzn-errortype header.
  Why: a client that depends on the create failing (so it can fix the role's trust policy and retry) is told the template is ready; the fleet-provisioning flow then fails at claim time with no diagnosable cause, and the account is left holding a template AWS would not have.
  Fix: CreateProvisioningTemplate's input validation — a provisioning-role trust pre-flight ahead of state insertion.
  Traces: iot-provisioning-template-error-paths#task#1 (representative), plus iot-provisioning-template-lifecycle-errors#task#1 and iot-provisioning-template-role-trust-error#task#1 in the same cluster.

Cause 2 of 9: DescribeProvisioningTemplate answers 200 for a template name that was never (validly) created.
  AWS: Smithy: DescribeProvisioningTemplate -> GET /provisioning-templates/{templateName}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404, x-amzn-errortype 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', message 'Unable to find template named ' + <templateName from the request> — fixed text 'Unable to find template named ', variable part the requested name.
  Emulator: 200 with a full template projection.
  Why: existence probes (an idempotent create-if-absent) conclude the template exists.
  Fix: the DescribeProvisioningTemplate handler's lookup/not-found path.
  Traces: iot-provisioning-template-error-paths#task#2, iot-provisioning-template-lifecycle-errors#task#2.

Cause 3 of 9: DescribeProvisioningTemplateVersion answers 200 for a version of a template that does not exist.
  AWS: Smithy: DescribeProvisioningTemplateVersion -> GET /provisioning-templates/{templateName}/versions/{versionId}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 ResourceNotFoundException with message 'Unable to find a template named ' + <templateName> (fixed text 'Unable to find a template named ', variable part the requested name).
  Emulator: 200 with a version projection.
  Fix: the DescribeProvisioningTemplateVersion handler's template-existence check, which must run before any version lookup.
  Traces: iot-provisioning-template-error-paths#task#3.

Cause 4 of 9: CreateProvisioningTemplateVersion answers 200 for a template that does not exist.
  AWS: Smithy: CreateProvisioningTemplateVersion -> POST /provisioning-templates/{templateName}/versions, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 ResourceNotFoundException, message 'Unable to find template named ' + <templateName>.
  Emulator: 200, creating a version against a template that should not exist.
  Fix: the CreateProvisioningTemplateVersion handler's template-existence check.
  Traces: iot-provisioning-template-error-paths#task#5.

Cause 5 of 9: DeleteProvisioningTemplateVersion applies the default-version rule before checking that the template exists, so the refusal is the wrong class.
  AWS: Smithy: DeleteProvisioningTemplateVersion -> DELETE /provisioning-templates/{templateName}/versions/{versionId}; errors include ResourceNotFoundException (404) and InvalidRequestException (400). Recorded response for an unknown template: 404, x-amzn-errortype 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', message 'Unable to find template named ' + <templateName>. Resource existence is evaluated first; the 'cannot delete the default version' rule only applies to a template that exists.
  Emulator: 400 InvalidRequestException with message 'Unable to delete template version 1, it is the default version'.
  Why: a client retry/branching on 404-vs-400 (missing resource vs. an invalid request it could amend) takes the wrong branch, and the message names a version of a template that does not exist.
  Fix: ordering inside the DeleteProvisioningTemplateVersion handler — existence check ahead of the default-version guard.
  Traces: iot-provisioning-template-error-paths#task#6.

Cause 6 of 9: DeleteProvisioningTemplate answers 200 with an empty body for a template that does not exist.
  AWS: Smithy: DeleteProvisioningTemplate -> DELETE /provisioning-templates/{templateName}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 ResourceNotFoundException with message 'Unable to delete template, template not found' — entirely fixed text, no variable part (AWS does not name the template here).
  Emulator: 200 with {}.
  Why: delete is reported as having removed something that never existed, so teardown/reconciliation code cannot distinguish a real delete from a no-op.
  Fix: the DeleteProvisioningTemplate handler's not-found path.
  Traces: iot-provisioning-template-error-paths#task#8, iot-provisioning-template-lifecycle-errors#task#3.

Cause 7 of 9: DescribeProvisioningTemplateVersion's not-found message drops the article AWS uses.
  AWS: recorded response: 404 ResourceNotFoundException with message 'Unable to find a template named ' + <templateName>. This operation's wording differs from DescribeProvisioningTemplate's ('Unable to find template named '), and both forms are requirements for their own operation.
  Emulator: 404 ResourceNotFoundException (status and code correct) with 'Unable to find template named ' + <templateName> — DescribeProvisioningTemplate's wording reused here.
  Why: a client matching the message to tell the two lookups apart, or surfacing it to an operator, sees text AWS does not send for this operation.
  Fix: the DescribeProvisioningTemplateVersion handler's not-found message.
  Traces: iot-provisioning-template-lifecycle-errors#task#5, iot-provisioning-template-not-found-errors#task#5. Related open issue 20261114T082236Z-iot-refusal-message-fidelity-7-causes-addthingtothinggroup-createstream.md reports a DIFFERENT DescribeProvisioningTemplateVersion message (the missing-VERSION text 'Unable to find template version <id>') from the behavioral corpus; it does not carry this wire key or this missing-TEMPLATE wording.

Cause 8 of 9: DeleteProvisioningTemplate's not-found message is the describe-style text instead of the delete-style text.
  AWS: recorded response: 404 ResourceNotFoundException with message 'Unable to delete template, template not found' — fixed text, resource name NOT interpolated.
  Emulator: 404 ResourceNotFoundException with 'Unable to find template named ' + <templateName>.
  Fix: the DeleteProvisioningTemplate handler's not-found message.
  Traces: iot-provisioning-template-not-found-errors#task#2.

Cause 9 of 9: CreateProvisioningTemplate's malformed-templateBody refusal leaks the emulator's JSON parser diagnostics.
  AWS: Smithy: templateBody is @required (@length 0..10240, pattern ^[\s\S]*$); modeled error InvalidRequestException (400). Recorded response: 400 with message 'The template body is invalid' — entirely fixed text, with no parser position and no echo of the body.
  Emulator: 400 with 'templateBody is not a valid JSON document: expected ident at line 1 column 2' — serde-style parser text naming a line and column.
  Why: the message is not AWS's, and it exposes the emulator's parser implementation; clients matching on the fixed text do not match.
  Fix: the CreateProvisioningTemplate validation path's templateBody parse-failure message.
  Traces: iot-register-thing-invalid-template#task#2.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up: true, 1104 steps, 472 passed). All nine clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-provisioning-template-error-paths#task#1 (CreateProvisioningTemplate):
  expected status 400, got 200
  $.message: missing (expected 'The provisioning role cannot be assumed by AWS IoT', got None)
  $headers.x-amzn-errortype: missing (expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got None)
  also affected: iot-provisioning-template-lifecycle-errors#task#1, iot-provisioning-template-role-trust-error#task#1

Cause 2 — iot-provisioning-template-error-paths#task#2 (DescribeProvisioningTemplate):
  expected status 404, got 200
  $.message: missing (expected 'Unable to find template named <templateName>', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', got None)
  also affected: iot-provisioning-template-lifecycle-errors#task#2

Cause 3 — iot-provisioning-template-error-paths#task#3 (DescribeProvisioningTemplateVersion):
  expected status 404, got 200; expected message 'Unable to find a template named <templateName>'

Cause 4 — iot-provisioning-template-error-paths#task#5 (CreateProvisioningTemplateVersion):
  expected status 404, got 200; expected message 'Unable to find template named <templateName>'

Cause 5 — iot-provisioning-template-error-paths#task#6 (DeleteProvisioningTemplateVersion):
  expected status 404, got 400
  $.message: expected 'Unable to find template named <templateName>', got 'Unable to delete template version 1, it is the default version'
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', got 'InvalidRequestException'

Cause 6 — iot-provisioning-template-error-paths#task#8 (DeleteProvisioningTemplate):
  expected status 404, got 200; expected body {'message': 'Unable to delete template, template not found'}, got {}
  also affected: iot-provisioning-template-lifecycle-errors#task#3

Cause 7 — iot-provisioning-template-lifecycle-errors#task#5 (DescribeProvisioningTemplateVersion), status 404 as expected:
  $.message: expected 'Unable to find a template named <templateName>', got 'Unable to find template named <templateName>'
  also affected: iot-provisioning-template-not-found-errors#task#5

Cause 8 — iot-provisioning-template-not-found-errors#task#2 (DeleteProvisioningTemplate), status 404 as expected:
  $.message: expected 'Unable to delete template, template not found', got 'Unable to find template named <templateName>'

Cause 9 — iot-register-thing-invalid-template#task#2 (CreateProvisioningTemplate), status 400 as expected:
  $.message: expected 'The template body is invalid', got 'templateBody is not a valid JSON document: expected ident at line 1 column 2'

The template names and role ARNs in the recordings are per-run probe values; the requirement is each message's fixed text plus the request value it interpolates, as named per cause. Causes 2-6 all occur after cause 1 in the same fixture, so closing cause 1 may close them — each still has to be verified independently, because each one's handler must answer 404 for an unknown template name regardless of how the name came to be absent.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — CreateProvisioningTemplate (POST /provisioning-templates; provisioningRoleArn required; InvalidRequestException 400), DescribeProvisioningTemplate / DeleteProvisioningTemplate (GET|DELETE /provisioning-templates/{templateName}; ResourceNotFoundException 404), DescribeProvisioningTemplateVersion / DeleteProvisioningTemplateVersion (GET|DELETE /provisioning-templates/{templateName}/versions/{versionId}; ResourceNotFoundException 404, InvalidRequestException 400), CreateProvisioningTemplateVersion (POST /provisioning-templates/{templateName}/versions; ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: CreateProvisioningTemplate(templateName=T, templateBody=<valid body>, provisioningRoleArn=<an IAM role whose trust policy does NOT allow iot.amazonaws.com>) — expect 400 InvalidRequestException 'The provisioning role cannot be assumed by AWS IoT'.
3. Causes 2-6, against a template name that does not exist: DescribeProvisioningTemplate(T), DescribeProvisioningTemplateVersion(T, 1), CreateProvisioningTemplateVersion(T, templateBody=...), DeleteProvisioningTemplateVersion(T, 1), DeleteProvisioningTemplate(T) — expect 404 ResourceNotFoundException from each, with the per-cause message above.
4. Cause 7: create a template, then DescribeProvisioningTemplateVersion against a DIFFERENT, nonexistent template name and read Error.Message.
5. Cause 8: DeleteProvisioningTemplate against a nonexistent name and read Error.Message.
6. Cause 9: CreateProvisioningTemplate(templateName=T, templateBody='{not json', provisioningRoleArn=<assumable role>) and read Error.Message.
7. Re-replay the representative step iot-provisioning-template-error-paths#task#1 and every fixture#step in trace_ids.
