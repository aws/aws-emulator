Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__route53.deploy-route53-alias-recordset-without-evaluate-target-health cloudformation__route53.deploy-route53-hostedzone-comment-and-healthcheck-tags-update-in-place cloudformation__route53.deploy-route53-recordset-and-group-update-in-place
Issue-Key: cloudformation__route53.deploy-route53-alias-recordset-without-evaluate-target-health|run#7|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__route53.deploy-route53-hostedzone-comment-and-healthcheck-tags-update-in-place|_update#10|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__route53.deploy-route53-recordset-and-group-update-in-place|run#12|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# cloudformation__route53: 3 net-new causes — AWS::Route53::RecordSet alias without EvaluateTargetHealth is rolled back, and RecordSet/RecordSetGroup/HealthCheck in-place updates answer NotImplemented

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three independent CloudFormation-resource defects for Route 53 types. Each makes a stack that AWS completes end in a rollback state.

Cause 1 of 3: an AWS::Route53::RecordSet whose AliasTarget omits EvaluateTargetHealth is rejected, and the stack rolls back.
AWS: in the CloudFormation resource schema, AWS::Route53::RecordSet AliasTarget.EvaluateTargetHealth is optional; the resource handler supplies the value when calling Route 53, whose API member AliasTarget.EvaluateTargetHealth is required (Smithy: services/service-com-amazonaws-route53-awsdnsv20130401, AliasTarget.EvaluateTargetHealth required boolean). The stack therefore reaches CREATE_COMPLETE and the alias record set exists.
Emulator: the resource handler forwards the template as-is, so Route 53 refuses with the generic validation envelope "1 validation error detected: Value null at 'changeBatch.changes.<n>.member.resourceRecordSet.aliasTarget.evaluateTargetHealth' failed to satisfy constraint: Member must not be null (Service: Route53, Status Code: 400, ...)" and the stack ends ROLLBACK_COMPLETE with HandlerErrorCode InvalidRequest.
Why: every template that writes an alias record set without spelling out EvaluateTargetHealth — the common CDK/console form — fails to deploy against the emulator.
Fix: the AWS::Route53::RecordSet resource handler in the CloudFormation resource registry must default the optional AliasTarget.EvaluateTargetHealth before calling ChangeResourceRecordSets.
Traces: cloudformation__route53.deploy-route53-alias-recordset-without-evaluate-target-health.

Cause 2 of 3: AWS::Route53::HealthCheck cannot be updated in place.
AWS: an UPDATE that changes a health check's properties (its comment/tags) is applied in place by the resource handler and the stack reaches UPDATE_COMPLETE.
Emulator: the update fails with "NotImplemented: this emulator does not change the properties of an existing AWS::Route53::HealthCheck in place; the resource was left as it is." and the stack ends UPDATE_ROLLBACK_COMPLETE.
Why: a client cannot evolve a deployed health check; any stack update touching it is rolled back, and the update path of its deployment pipeline is untestable.
Fix: implement the update path of the AWS::Route53::HealthCheck resource handler (UpdateHealthCheck / ChangeTagsForResource).
Traces: cloudformation__route53.deploy-route53-hostedzone-comment-and-healthcheck-tags-update-in-place.

Cause 3 of 3: AWS::Route53::RecordSet and AWS::Route53::RecordSetGroup cannot be updated in place.
AWS: an UPDATE that changes a record set's properties (values, TTL, members of a group) is applied in place through ChangeResourceRecordSets UPSERT and the stack reaches UPDATE_COMPLETE.
Emulator: both resources fail with "NotImplemented: this emulator does not change the properties of an existing AWS::Route53::RecordSet in place; the resource was left as it is." (and the same sentence for AWS::Route53::RecordSetGroup), and the stack ends UPDATE_ROLLBACK_COMPLETE.
Why: DNS records provisioned by CloudFormation can never be changed; a client's routine record update rolls the whole stack back.
Fix: implement the update paths of the AWS::Route53::RecordSet and AWS::Route53::RecordSetGroup resource handlers.
Traces: cloudformation__route53.deploy-route53-recordset-and-group-update-in-place.

## Evidence
From the replay output:
- cloudformation__route53.deploy-route53-alias-recordset-without-evaluate-target-health: AssertionError: stack ended 'ROLLBACK_COMPLETE': [('Alias', 'Resource handler returned message: "1 validation error detected: Value null at \'changeBatch.changes.1.member.resourceRecordSet.aliasTarget.evaluateTargetHealth\' failed to satisfy constraint: Member must not be null (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)')] — expected CREATE_COMPLETE.
- cloudformation__route53.deploy-route53-hostedzone-comment-and-healthcheck-tags-update-in-place: AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE': [('HC', 'NotImplemented: this emulator does not change the properties of an existing AWS::Route53::HealthCheck in place; the resource was left as it is.')]
- cloudformation__route53.deploy-route53-recordset-and-group-update-in-place: AssertionError: record update ended 'UPDATE_ROLLBACK_COMPLETE': [('Rec', 'NotImplemented: this emulator does not change the properties of an existing AWS::Route53::RecordSet in place; the resource was left as it is.'), ('Grp', 'NotImplemented: this emulator does not change the properties of an existing AWS::Route53::RecordSetGroup in place; the resource was left as it is.')]

## Steps to Reproduce
1. Cause 1: CreateStack with an AWS::Route53::HostedZone, a target record, and an AWS::Route53::RecordSet whose AliasTarget sets only DNSName and HostedZoneId (no EvaluateTargetHealth). Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
2. Cause 2: CreateStack with an AWS::Route53::HostedZone and an AWS::Route53::HealthCheck; then UpdateStack changing the zone comment and the health-check tags. The stack ends UPDATE_ROLLBACK_COMPLETE with the NotImplemented reason on the HealthCheck resource.
3. Cause 3: CreateStack with an AWS::Route53::RecordSet and an AWS::Route53::RecordSetGroup; then UpdateStack changing their record values/TTL. The stack ends UPDATE_ROLLBACK_COMPLETE with the NotImplemented reason on both resources.
