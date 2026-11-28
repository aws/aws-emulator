Issue-Source: trace-sweep-divergence
Issue-Trace: es-package-management-errors
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-package-management-errors opensearch-associate-packages-missing-package-error es-vpc-endpoint-errors opensearch-vpc-endpoint-error-paths opensearch-tag-error-paths opensearch-tags-error-paths opensearch-describe-reserved-instance-offering-not-found opensearch-reserved-instance-offering-describe-and-purchase-errors opensearch-purchase-nonexistent-offering-error opensearch-purchase-reserved-instance-offering-invalid-id-error opensearch-purchase-reserved-instance-offering-not-found
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DissociatePackage|4xx->4xx|$.message
Issue-Key: es|DissociatePackages|4xx->4xx|$.message
Issue-Key: es|AssociatePackages|4xx->4xx|$.message
Issue-Key: es|CreateVpcEndpoint|4xx->4xx|$.message
Issue-Key: es|AuthorizeVpcEndpointAccess|4xx->4xx|$.message
Issue-Key: es|AuthorizeVpcEndpointAccess|4xx->4xx|status
Issue-Key: es|AddTags|4xx->4xx|$.message
Issue-Key: es|DescribeReservedInstanceOfferings|4xx->4xx|status
Issue-Key: es|PurchaseReservedInstanceOffering|4xx->4xx|$.message
Issue-Key: es|PurchaseReservedInstanceOffering|4xx->4xx|status
Issue-Related: 20261029T190009Z-es-constraint-violation-refusal-messages-are-emulator-composed.md
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: package, VPC-endpoint, tagging and reserved-instance refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
9 net-new root causes: refusals whose status and error code are right but whose message is emulator-authored prose, plus two where the emulator checks the wrong precondition.

Cause 1 of 9 - DissociatePackage checks the wrong precondition. AWS: 400 with the fixed text "The domain is not on the latest software version. Upgrade the domain to start using this feature. " (note the trailing space - it is part of the recorded text) and no variable part. Emulator: 'The domain <domainName> does not exist.'. Trace: es-package-management-errors#task#8.

Cause 2 of 9 - DissociatePackages. AWS: 400 "Package is not associated to this domain." (fixed). Emulator: 'The domain <domainName> does not exist.'. Trace: es-package-management-errors#task#9.

Cause 3 of 9 - AssociatePackages missing-package refusal. AWS: "Operation not allowed. Package with id <the package id from the request> not found." - fixed text with the requested id interpolated. Emulator: 'Package not found.', dropping the id. Trace: opensearch-associate-packages-missing-package-error#task#2.

Cause 4 of 9 - CreateVpcEndpoint does not validate the subnet. AWS: 400 with "The subnet ID '<the subnetId from the request>' does not exist". Emulator: the domain-permission text "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain.", i.e. it fails the domain check and never reaches the subnet. Trace: es-vpc-endpoint-errors#task#1 (affected also opensearch-vpc-endpoint-error-paths#task#2).

Cause 5 of 9 - AuthorizeVpcEndpointAccess on an absent domain. AWS: 404 with the same domain-permission text quoted in cause 4 - it deliberately does not confirm whether the domain exists. Emulator: 409 with 'Domain not found: <domainName>', leaking the existence answer. Trace: es-vpc-endpoint-errors#task#5.

Cause 6 of 9 - AddTags with a malformed ARN. AWS: 400 "Invalid ARN format." (fixed). Emulator: 'Invalid ARN. Domain not found.', which also conflates a format error with a not-found. Trace: opensearch-tag-error-paths#task#2 (affected also opensearch-tags-error-paths#task#2).

Cause 7 of 9 - AddTags over the tag limit. AWS: 409 "You have exceeded the maximum number of tags allowed for this resource" (fixed, no variable part). Emulator: 'arn:aws:es:...:domain/<name> would carry 61 tags, more than the 10 a resource may have.' - emulator-authored prose that interpolates the ARN, a computed count and a limit. Trace: opensearch-tag-error-paths#task#6.

Cause 8 of 9 - DescribeReservedInstanceOfferings for an unknown offering. AWS: 404 "Offering id not found" (fixed, the id is not echoed). Emulator: 409 'Reserved Instance offering not found: <the id from the request>'. Trace: opensearch-describe-reserved-instance-offering-not-found#task#1 (affected also opensearch-reserved-instance-offering-describe-and-purchase-errors#task#2).

Cause 9 of 9 - PurchaseReservedInstanceOffering for an unknown offering. AWS: 404 "Offering id not found" (fixed). Emulator: 409 with the id interpolated. Trace: opensearch-purchase-nonexistent-offering-error#task#1 (affected also opensearch-purchase-reserved-instance-offering-invalid-id-error#task#1, opensearch-purchase-reserved-instance-offering-not-found#task#1, opensearch-reserved-instance-offering-describe-and-purchase-errors#task#3).

Why: the message is part of the contract - clients and runbooks match on it, and causes 1, 2 and 4 additionally point an operator at the wrong problem ("the domain does not exist" when the real condition is a stale software version, a missing package association, or a nonexistent subnet). Causes 5, 6 and 8/9 turn a 404 into a 409 or leak existence information AWS withholds.

Fix: the refusal text and the precondition order in the named handlers (DissociatePackage, DissociatePackages, AssociatePackages, CreateVpcEndpoint, AuthorizeVpcEndpointAccess, AddTags, DescribeReservedInstanceOfferings, PurchaseReservedInstanceOffering).

## Evidence
Cause 1: expected 'The domain is not on the latest software version. Upgrade the domain to start using this feature. '; actual 'The domain sim2real-probe-absent does not exist.'.
Cause 2: expected 'Package is not associated to this domain.'; actual 'The domain sim2real-probe-absent does not exist.'.
Cause 3: expected 'Operation not allowed. Package with id <pkg-id> not found.'; actual 'Package not found.'.
Cause 4: expected "The subnet ID '<subnet-id>' does not exist"; actual "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain.".
Cause 5: expected 404 + the domain-permission text; actual 409 + 'Domain not found: <domainName>'.
Cause 6: expected 'Invalid ARN format.'; actual 'Invalid ARN. Domain not found.'.
Cause 7: expected 'You have exceeded the maximum number of tags allowed for this resource'; actual '<arn> would carry 61 tags, more than the 10 a resource may have.'.
Cause 8/9: expected 404 + 'Offering id not found'; actual 409 + 'Reserved Instance offering not found: <id>'.

## Steps to Reproduce
1. Replay es-package-management-errors steps task#8 and task#9, and opensearch-associate-packages-missing-package-error step task#2, comparing $.message with the recordings.
2. Replay es-vpc-endpoint-errors steps task#1 and task#5 for the subnet check and the AuthorizeVpcEndpointAccess status/text.
3. Replay opensearch-tag-error-paths steps task#2 and task#6, opensearch-describe-reserved-instance-offering-not-found step task#1 and opensearch-purchase-nonexistent-offering-error step task#1.
