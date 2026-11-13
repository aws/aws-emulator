Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected
Issue-Key: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected|_poll#18|Timeout|time.sleep(interval_s)
Issue-Commit: ffece56e68e804f0450b8a83661b5d243139afbd

# apigateway__logs__wafv2: both WAFv2 logging-configuration checks hang to the 60 s cap — the WAF-protected REST stage never answers (1 cause)

> _Note: this issue was identified against the code base at commit `ffece56e68e804f0450b8a83661b5d243139afbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
Cause 1 of 1: a request to a REST stage associated with a WAFv2 web ACL never completes, so neither the delivered log record nor the destination-name refusal can be observed.

AWS: a WAFv2 web ACL associated with an API Gateway stage inspects each request at the edge and the stage returns a bounded HTTP response (403 with the ACL's block action, 200 when allowed); PutLoggingConfiguration then delivers one record per inspected request to the configured destination, with the configured RedactedFields redacted, and it REFUSES a CloudWatch Logs destination whose log-group name does not start with the aws-waf-logs- prefix (Smithy: PutLoggingConfiguration errors include WAFInvalidParameterException, httpError 400 — the modeled client refusal for an unacceptable LoggingConfiguration member). Both checks must first generate a request at the stage for there to be anything to log or compare.

Emulator: both checks are killed by the harness at its 60 s cap ('Check timed out after 60.0s') — the request to the WAF-associated stage never returns, so no log record is ever produced and the refusal leg is never reached.

Why: a client cannot tell allowed from blocked traffic and gets no response at all from a WAF-protected stage; WAF logging — the only way to audit what the ACL did — is unobservable, and a misconfigured log destination that AWS refuses with a 400 is never refused here.

Note: the existing open issue 20261023T210711Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks.md describes this group hanging, but neither failing check is marked OWNED by it, so this is filed with the current replay keys.

Fix: the execute-api data plane's WAFv2 evaluation path (the request must terminate with a response once a web ACL is associated) and, behind it, the WAFv2 logging-configuration validation and record delivery for the associated stage.

Traces: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields, apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected.

## Evidence
Check apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields:
Error: Check timed out after 60.0s

Check apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected:
Error: Check timed out after 60.0s

Expected (AWS): the WAF-associated stage answers each request (403 for a blocked request, 200 otherwise); one log record per inspected request reaches the aws-waf-logs-* destination with RedactedFields redacted; PutLoggingConfiguration with a log group lacking the aws-waf-logs- prefix is refused with HTTP 400 WAFInvalidParameterException.
Actual (emulator): no HTTP response from the stage within the 60 s harness cap; neither the log record nor the refusal is ever reached.

## Steps to Reproduce
1. Create a REST API, deploy it to a stage.
2. wafv2 CreateWebACL (Scope=REGIONAL) with a blocking rule, then AssociateWebACL to the stage ARN.
3. wafv2 PutLoggingConfiguration pointing at a CloudWatch Logs group named aws-waf-logs-<token> with RedactedFields set; separately call PutLoggingConfiguration with a group name lacking the aws-waf-logs- prefix.
4. Send a request to the stage invoke URL that the rule should block.
5. Observe that the request never returns (harness kills the check at 60 s), so no log record is delivered and the prefix refusal is never exercised.
