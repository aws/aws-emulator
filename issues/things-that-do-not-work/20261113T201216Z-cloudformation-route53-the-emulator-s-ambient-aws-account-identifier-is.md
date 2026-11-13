Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected
Issue-Key: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected|run#15|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: 91ff39d61765158469badec04b624515fa0f07c8

# The emulator's ambient AWS account identifier is the degenerate all-zeros string 000000000000, so an account-masking client sees digit runs eaten out of otherwise-correct Route 53 refusal messages

> _Note: this issue was identified against the code base at commit `91ff39d61765158469badec04b624515fa0f07c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
Cause 1 of 1: the caller identity the emulator serves uses the all-zero 12-digit account id, which AWS never allocates.

AWS:      Route 53 refuses a ChangeResourceRecordSets that names a hosted zone id it never assigned with HTTP 404 and the NoSuchHostedZone message "No hosted zone found with ID: <HostedZoneId>"; CloudFormation's AWS::Route53::RecordSet handler surfaces that as the CREATE_FAILED reason `Resource handler returned message: "No hosted zone found with ID: <HostedZoneId> (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. Fixed text: "No hosted zone found with ID: " and the SDK suffix "(Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)"; the only variable part is the HostedZoneId copied from the request. Separately, the account identifier an AWS caller reports (sts:GetCallerIdentity Account, and the account segment of every ARN it owns) is a 12-digit identifier AWS allocates to that account; it is never the all-zero string, which AWS uses as a non-account sentinel. Smithy: com.amazonaws.sts#accountType is an unconstrained string, so the model states nothing about the value — documented/recorded behavior only for the account-id shape.

Emulator: the Route 53 refusal itself is faithful — status 404, HandlerErrorCode InvalidRequest, and both the fixed text and the echoed HostedZoneId match. What diverges is the identity the emulator speaks for: its ambient account id is the digit string "000000000000". The check masks the caller's own account out of the handler reason before comparing (`text.replace(account, "<account>")`), so those twelve zeros are consumed out of the MIDDLE of the correctly-echoed zone id "Z0000000000000NOTREAL" (Z + thirteen zeros + NOTREAL) and the compared reason becomes "No hosted zone found with ID: Z<account>0NOTREAL (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)" instead of "No hosted zone found with ID: Z0000000000000NOTREAL (...)". Derivation: the observed string differs from the expected one by exactly one twelve-zero run replaced with the mask token, which is only possible when the emulator's account value is "000000000000"; no other account value appears in, or would be removed from, this message.

Why:      a client that embeds, redacts or pattern-matches the caller's account id — log scrubbers, ARN parsers, cross-account policy comparisons, test harnesses like this one — collides with unrelated digit runs in other identifiers, because an all-zero account id is a substring of many perfectly ordinary ids and is not an identifier AWS would ever hand out. The same value flows into every ARN the emulator mints, so anything that treats the account segment as a real allocated account id (or rejects the reserved all-zero value) misbehaves against the emulator while working against AWS.

Fix:      the emulator's ambient caller identity / default account value — the account the STS identity resolution reports (services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_caller_identity.rs reads it from the shared identity state) and that the engine stamps into minted ARNs. It should be a non-degenerate 12-digit account id rather than 000000000000. Nothing in the Route 53 ChangeResourceRecordSets handler or its NoSuchHostedZone serializer needs to change: that refusal already matches AWS.

Traces:   cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected (the check's control stack deploys and the rejected stack reaches ROLLBACK_COMPLETE; only the masked-message comparison fails).

## Evidence
Failing check: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected

AssertionError: handler reason ('No hosted zone found with ID: Z<account>0NOTREAL (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'), expected ('No hosted zone found with ID: Z0000000000000NOTREAL (Service: Route53, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')

Expected (AWS): the handler reason echoes the requested HostedZoneId verbatim; with a real 12-digit account id nothing in the message is masked, so the comparison sees "No hosted zone found with ID: Z0000000000000NOTREAL ..." with HandlerErrorCode InvalidRequest.

Actual (emulator): same status (404), same error class (InvalidRequest), same fixed text, same echoed zone id — but the emulator's account value "000000000000" is a substring of the echoed id, so the harness's account mask turns the first twelve of the id's thirteen zeros into "<account>", yielding "Z<account>0NOTREAL". The mask token position proves the emulator's account id is the twelve-character all-zero string.

## Steps to Reproduce
1. Boot the emulator and call sts:GetCallerIdentity — observe Account == "000000000000".
2. Create a CloudFormation stack whose template declares an AWS::Route53::HostedZone plus an AWS::Route53::RecordSet (TXT, TTL 300) whose HostedZoneId is the syntactically valid but never-assigned id "Z0000000000000NOTREAL".
3. Wait for the stack to reach ROLLBACK_COMPLETE and read DescribeStackEvents for the CREATE_FAILED row of the RecordSet resource.
4. The ResourceStatusReason is correct, but replacing the caller's account id with a placeholder (as any account-redacting client does) removes twelve zeros from the middle of the echoed zone id, because the emulator's account id is itself twelve zeros.
5. Replay: cloudformation__route53.deploy-route53-recordset-nonexistent-hostedzone-rejected.
