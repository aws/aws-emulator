Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudformation__route53.deploy-cfn-certificate-hostedzoneid-with-email-validation-rejected
Issue-Key: acm__cloudformation__route53.deploy-cfn-certificate-hostedzoneid-with-email-validation-rejected|run#13|AssertionError|assert not email_cert_arn, (
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__cloudformation__route53: a refused AWS::CertificateManager::Certificate resource still mints an ACM certificate and reports its ARN as PhysicalResourceId

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the Certificate resource handler calls ACM RequestCertificate before validating the HostedZoneId/ValidationMethod combination, so a CREATE_FAILED resource leaves a real certificate behind.

AWS: recorded behavior only (CloudFormation resource-handler semantics are not in a Smithy model). For AWS::CertificateManager::Certificate, a DomainValidationOptions entry that carries HostedZoneId together with ValidationMethod: EMAIL is invalid and is rejected by the resource handler BEFORE any ACM call. DescribeStackResources then reports the resource as ResourceStatus CREATE_FAILED with ResourceStatusReason whose fixed text is `Resource handler returned message: "HostedZoneId cannot be used with Email validation" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the only variable part is the per-invocation RequestToken — and with NO PhysicalResourceId, because nothing was created. The identical zone and domain with ValidationMethod: DNS succeeds and mints a PENDING_VALIDATION certificate.

Emulator: the resource does reach CREATE_FAILED, but it reports PhysicalResourceId set to a minted ACM certificate ARN (`arn:aws:acm:<region>:<account>:certificate/<uuid>`), i.e. RequestCertificate was issued first and the validation ran afterwards. A certificate for the refused resource exists in ACM.

Why: the stack reports a resource that failed yet owns a live certificate. A client reading PhysicalResourceId of a CREATE_FAILED resource gets an ARN AWS never hands out; rollback and stack deletion either leave the certificate orphaned (quota consumed, a validation record expected in Route 53 forever) or delete one the caller believes was never created. Any reconciliation that treats "CREATE_FAILED with no physical id" as "nothing to clean up" — which is what AWS guarantees — leaks on this emulator.

Fix: the CloudFormation resource handler for AWS::CertificateManager::Certificate — validate DomainValidationOptions (HostedZoneId is incompatible with ValidationMethod EMAIL) in the handler's input check, ahead of the ACM RequestCertificate call, and leave PhysicalResourceId unset when the create is refused.

Traces: acm__cloudformation__route53.deploy-cfn-certificate-hostedzoneid-with-email-validation-rejected

## Evidence
Template: AWS::CertificateManager::Certificate with DomainName=<domain>, ValidationMethod: EMAIL, DomainValidationOptions: [{DomainName: <domain>, HostedZoneId: <zone id>}]; CreateStack with DisableRollback=true.
Expected: DescribeStackResources -> ResourceStatus 'CREATE_FAILED', PhysicalResourceId absent/empty, ResourceStatusReason 'Resource handler returned message: "HostedZoneId cannot be used with Email validation" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)', and no certificate in ACM.
Actual: AssertionError: a refused Certificate resource must never reach ACM and mint a certificate; got PhysicalResourceId='arn:aws:acm:<region>:<account>:certificate/<uuid>'
The ResourceStatus assertion ahead of it passed, so the failure is specifically the ordering: ACM is called before the refusal.

## Steps to Reproduce
1. route53 CreateHostedZone for a fresh public domain; take the bare zone id.
2. cloudformation CreateStack with DisableRollback=true and a template whose only resource is:
   Certificate: Type AWS::CertificateManager::Certificate, Properties DomainName=<domain>, ValidationMethod: EMAIL, DomainValidationOptions: [{DomainName: <domain>, HostedZoneId: <zone id>}].
3. Poll cloudformation DescribeStackResources(StackName=..., LogicalResourceId="Certificate") until it leaves CREATE_IN_PROGRESS.
4. Observe ResourceStatus CREATE_FAILED but a non-empty PhysicalResourceId naming an ACM certificate; DescribeCertificate on that ARN succeeds. AWS reports no PhysicalResourceId and has no such certificate.
