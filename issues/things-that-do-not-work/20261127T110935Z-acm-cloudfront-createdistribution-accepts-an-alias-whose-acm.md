Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudfront.create-distribution-rejects-alias-certificate-wrong-region-or-pending
Issue-Key: acm__cloudfront.create-distribution-rejects-alias-certificate-wrong-region-or-pending|expect_error#21|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__cloudfront: CreateDistribution accepts an alias whose ACM certificate is still PENDING_VALIDATION instead of refusing with InvalidViewerCertificate

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the viewer-certificate gate checks the certificate's region but not that it is valid/ISSUED.

AWS: CloudFront CreateDistribution with a non-empty Aliases list and ViewerCertificate.ACMCertificateArn requires that certificate to exist in us-east-1 AND to be a valid, issued certificate with a certificate chain. A certificate that is still PENDING_VALIDATION fails that requirement and the call is refused with HTTP 400, error code InvalidViewerCertificate (Smithy: cloudfront error InvalidViewerCertificate, "A viewer certificate specified is not valid."), message — a fixed sentence with no variable parts — "The specified SSL certificate doesn't exist, isn't in us-east-1 region, isn't valid, or doesn't include a valid certificate chain." No distribution is created and the alias stays unused.

Emulator: the request succeeds with HTTP 201 and returns a Distribution (Status 'InProgress') whose DistributionConfig carries the alias and the PENDING_VALIDATION certificate. The neighbouring arm of the same guard works — the identical call with a certificate in a non-us-east-1 region IS refused with exactly the AWS code and message — so only the "isn't valid" arm (certificate Status != ISSUED) is missing.

Why: CloudFront's refusal is what tells a caller its certificate is not yet validated. Here the distribution is created and reported as deploying over a certificate that cannot terminate TLS, so the alias appears claimed, the failure surfaces only later as an unusable HTTPS endpoint, and a caller that treats a successful CreateDistribution as "certificate accepted" never goes back to publish its validation record.

Fix: cloudfront CreateDistribution's viewer-certificate validation (and the identical path in UpdateDistribution) — extend the existing region check on ViewerCertificate.ACMCertificateArn to also require the referenced ACM certificate's Status to be ISSUED, refusing with InvalidViewerCertificate otherwise.

Traces: acm__cloudfront.create-distribution-rejects-alias-certificate-wrong-region-or-pending

## Evidence
Call: cloudfront CreateDistribution with Aliases={Quantity:1, Items:[<alias host>]} and ViewerCertificate={ACMCertificateArn: <us-east-1 certificate still PENDING_VALIDATION>, SSLSupportMethod: 'sni-only', MinimumProtocolVersion: 'TLSv1.2_2021'}.
Expected: ClientError ('InvalidViewerCertificate', "The specified SSL certificate doesn't exist, isn't in us-east-1 region, isn't valid, or doesn't include a valid certificate chain.") — fixed text, no variable parts.
Actual: AssertionError: expected the call to be refused, but it succeeded: HTTPStatusCode 201 with Distribution {Status: 'InProgress', DistributionConfig: {Aliases: {Quantity: 1, Items: [<alias>]}, ...}}.
The preceding call in the same check — the same shape with a certificate outside us-east-1 — was refused with exactly that code and message, isolating the missing condition to the certificate's validation status.

## Steps to Reproduce
1. In us-east-1, acm RequestCertificate with ValidationMethod=DNS for a domain whose validation record is never published, so it stays PENDING_VALIDATION.
2. cloudfront CreateDistribution with Aliases=[<that domain>] and ViewerCertificate={ACMCertificateArn: <that certificate>, SSLSupportMethod: 'sni-only', MinimumProtocolVersion: 'TLSv1.2_2021'}.
3. Observe HTTP 201 and a created distribution. AWS refuses with 400 InvalidViewerCertificate and the sentence above, and ListDistributions shows the alias still unused.
