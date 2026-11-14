Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected
Issue-Key: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected|run#21|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 9ffeef3b6017fc24d3bc6fe787f5ae12108b3509

# route53 CreateHealthCheck: an off-enum InsufficientDataHealthStatus is refused with the generic "1 validation error detected" envelope instead of Route 53's XML-schema (cvc-pattern-valid) refusal

> _Note: this issue was identified against the code base at commit `9ffeef3b6017fc24d3bc6fe787f5ae12108b3509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHealthCheck

## Description
One net-new root cause.

Cause 1 of 1: Route 53 CreateHealthCheck's refusal message for a HealthCheckConfig enum member given a value outside its value set is emulator-authored. Failing check: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected.

AWS: Route 53 is a REST-XML service (Smithy: CreateHealthCheck POST /2013-04-01/healthcheck, errors InvalidInput httpError 400; HealthCheckConfig.InsufficientDataHealthStatus targets enum InsufficientDataHealthStatus with values [Healthy, Unhealthy, LastKnownStatus]). A request body whose InsufficientDataHealthStatus element text is not one of those values is rejected by the service's XML schema validation before the operation's own validation, with HTTP 400 and the message

  Invalid XML ; javax.xml.stream.XMLStreamException: org.xml.sax.SAXParseException; lineNumber: 1; columnNumber: <column>; cvc-pattern-valid: Value '<submitted value>' is not facet-valid with respect to pattern '[Hh][Ee][Aa][Ll][Tt][Hh][Yy]|[Uu][Nn][Hh][Ee][Aa][Ll][Tt][Hh][Yy]|[Ll][Aa][Ss][Tt][Kk][Nn][Oo][Ww][Nn][Ss][Tt][Aa][Tt][Uu][Ss]' for type 'InsufficientDataHealthStatus'.

Fixed text of that message: everything except the two variable parts. Variable parts are (a) the submitted element text, taken verbatim from the request, and (b) columnNumber, the parser's position in the serialized body (it moves with the region and with name lengths, so it is not a constant). The case-insensitive character-class pattern and the type name 'InsufficientDataHealthStatus' are fixed for this member and derive from the member's schema type, not from the request. Recorded behavior confirms the status (400) and, via CloudFormation, that AWS::Route53::HealthCheck surfaces it as ResourceStatusReason `Resource handler returned message: "<the Route 53 message> (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` and rolls the stack back to ROLLBACK_COMPLETE.

Emulator: returns 400 with HandlerErrorCode InvalidRequest (both correct) but the message is the shared Smithy constraint-report envelope: "1 validation error detected: Value 'Bogus' at 'healthCheckConfig.insufficientDataHealthStatus' failed to satisfy constraint: Member must satisfy enum value set: [Healthy, Unhealthy, LastKnownStatus] (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)". That is the counted JSON-protocol validation form, carrying a lowerCamel member path ('healthCheckConfig.insufficientDataHealthStatus') that a REST-XML Route 53 request never produces; AWS emits no "1 validation error detected" text and no member path for this class of refusal.

Why: a client that keys off the refusal text — a deploy pipeline or test grading CloudFormation's ResourceStatusReason, a log/alert matcher, or any code distinguishing malformed-XML refusals from semantic ones — sees a message in a wholly different family. It cannot tell which element the service blamed in AWS's vocabulary (the schema type name) and any matcher written against real Route 53 fails, even though status and HandlerErrorCode line up.

Fix: belongs in the route53 service's CreateHealthCheck request deserialization/validation path (the REST-XML body parse for HealthCheckConfig enum members), which must refuse an off-enum enum element with the schema-level Invalid XML / cvc-pattern-valid InvalidInput message rather than deferring to the generic cross-protocol constraint-report envelope. The same path serves UpdateHealthCheck and the other HealthCheckConfig enum members (Type), so fix it where the enum element text is validated, not per operation.

Traces: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected (the check's positive control — the same template with InsufficientDataHealthStatus=Healthy — deploys and reads back, so only the refusal text diverges). No open issue covers this: the nearest neighbours (20261109T183935Z-cloudformation-cloudwatch-route53-a-stack-that-declares-a.md, 20261101T205428Z-route53-route-53-gethealthcheckstatus-reports-a-type-keyed-constant.md) concern health-check status reporting and alarm creation, not request validation wording.

## Evidence
Expected (AWS, as masked by the check): handler reason message = "Invalid XML ; javax.xml.stream.XMLStreamException: org.xml.sax.SAXParseException; lineNumber: 1; columnNumber: <n>; cvc-pattern-valid: Value 'Bogus' is not facet-valid with respect to pattern '[Hh][Ee][Aa][Ll][Tt][Hh][Yy]|[Uu][Nn][Hh][Ee][Aa][Ll][Tt][Hh][Yy]|[Ll][Aa][Ss][Tt][Kk][Nn][Oo][Ww][Nn][Ss][Tt][Aa][Tt][Uu][Ss]' for type 'InsufficientDataHealthStatus'. (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", HandlerErrorCode = "InvalidRequest".

Actual (emulator): message = "1 validation error detected: Value 'Bogus' at 'healthCheckConfig.insufficientDataHealthStatus' failed to satisfy constraint: Member must satisfy enum value set: [Healthy, Unhealthy, LastKnownStatus] (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", HandlerErrorCode = "InvalidRequest".

Assertion raised: check_cloudformation__cloudwatch__route53_deploy_route53_healthcheck_invalid_insufficient_data_status_rejected.py line 236 — `assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"`.

Matching parts: HTTP status 400, HandlerErrorCode InvalidRequest, stack reaching ROLLBACK_COMPLETE, and rollback absence (no health check on the alarm, no alarm left). Diverging part: the Route 53 error message text only.

## Steps to Reproduce
1. Create a CloudFormation stack with two resources: an AWS::CloudWatch::Alarm (Namespace sim2real/healthcheck, Statistic Average, Period 60, EvaluationPeriods 1, Threshold 1, ComparisonOperator GreaterThanThreshold) and an AWS::Route53::HealthCheck whose HealthCheckConfig is {Type: CLOUDWATCH_METRIC, AlarmIdentifier: {Name: {Ref: Alarm}, Region: {Ref: AWS::Region}}, InsufficientDataHealthStatus: "Bogus"} — any string outside [Healthy, Unhealthy, LastKnownStatus].
2. Wait for the stack to reach ROLLBACK_COMPLETE, then read DescribeStackEvents and take the ResourceStatusReason on the CREATE_FAILED row of the health-check logical id.
3. Parse out the handler message and HandlerErrorCode. Observe the message is the "1 validation error detected: ... Member must satisfy enum value set: [...]" envelope instead of AWS's "Invalid XML ; ... cvc-pattern-valid: Value 'Bogus' is not facet-valid with respect to pattern ... for type 'InsufficientDataHealthStatus'."
4. Equivalently, call route53 CreateHealthCheck directly with HealthCheckConfig.Type=CLOUDWATCH_METRIC and InsufficientDataHealthStatus set to an off-enum string and compare the 400 InvalidInput message.
5. Control: the same template with InsufficientDataHealthStatus="Healthy" reaches CREATE_COMPLETE and the health check reads back with InsufficientDataHealthStatus=Healthy.
