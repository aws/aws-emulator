Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected acm__opensearch.half-specified-custom-endpoint-rejected
Issue-Key: acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}: {message!r}"
Issue-Key: acm__opensearch.half-specified-custom-endpoint-rejected|_absent#7|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Commit: 3fffece81be5f9cdcba63f22f8252b41b774e64d

# OpenSearch CreateDomain does not validate DomainEndpointOptions custom endpoints: an incomplete endpoint, an unknown certificate ARN, and a certificate that does not cover the hostname are all accepted and create a domain

> _Note: this issue was identified against the code base at commit `3fffece81be5f9cdcba63f22f8252b41b774e64d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
TWO root causes, both in the OpenSearch CreateDomain handler's DomainEndpointOptions validation. A fixer must close BOTH before submitting. Service token taken from the failing checks' ids (acm__opensearch — these are cross-service checks that import a certificate into ACM and then create an OpenSearch domain against it); the defect itself lives in the opensearch crate.

Cause 1 of 2: CreateDomain accepts a custom endpoint whose ACM certificate does not cover the requested hostname.
AWS: Smithy com.amazonaws.opensearch#CreateDomain (POST /2021-01-01/opensearch/domain, success 200 -> DomainStatus) models ValidationException (HTTP 400); DomainEndpointOptions carries CustomEndpointEnabled, CustomEndpoint (a DomainNameFqdn) and CustomEndpointCertificateArn (an ACM ARN). Recorded behavior: OpenSearch reads the named certificate's subject alternative names out of ACM and compares them with CustomEndpoint; when the hostname is not covered the request is refused with 400 ValidationException and the message 'Certificate <cert-arn> does not contain the custom domain name <custom-endpoint>.' The fixed text is every word but the two substitutions, which come from the request itself: <cert-arn> is DomainEndpointOptions.CustomEndpointCertificateArn and <custom-endpoint> is DomainEndpointOptions.CustomEndpoint. The refused request creates NO domain (DescribeDomain for that name answers ResourceNotFoundException). The identical request naming a hostname the certificate's SAN does cover is accepted and round-trips EnforceHTTPS, CustomEndpointEnabled, CustomEndpoint and CustomEndpointCertificateArn.
Emulator: HTTP 200. The domain is created (Created=true, Processing=true) and echoes back the uncovered CustomEndpoint and the CustomEndpointCertificateArn unchanged; no comparison against the certificate's SANs happens at all.
Why: over-permissive. A client provisioning a domain behind a custom hostname is told the configuration is valid, then gets a TLS endpoint whose certificate does not match the name clients will use — the failure surfaces as an unrelated handshake error at request time instead of as a 400 at provisioning time, and the domain (and its cost) exists where AWS would have created nothing.
Fix: the CreateDomain handler's DomainEndpointOptions validation in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs, including the cross-service read of the ACM certificate's subject alternative names it needs to perform the comparison.
Traces: acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected.

Cause 2 of 2: CreateDomain accepts a half-specified custom endpoint, and accepts a certificate ARN that names no certificate in the account.
AWS: same operation and shapes as Cause 1; all three of the following are refused with 400 ValidationException and none creates a domain.
  - CustomEndpointEnabled=true with NEITHER CustomEndpoint nor CustomEndpointCertificateArn: 'Please provide both CustomEndpoint and CustomEndpointCertificateArn fields to create a custom endpoint.' (entirely fixed text — no substitutions).
  - CustomEndpointEnabled=true with CustomEndpoint but no CustomEndpointCertificateArn: the same fixed sentence.
  - CustomEndpointEnabled=true fully specified but with a well-formed ARN that names no certificate in the account: 'ACM Certificate <cert-arn> not found in your account.' — fixed text apart from <cert-arn>, taken from the request's CustomEndpointCertificateArn.
The fully specified, resolvable request is accepted and, with no TLSSecurityPolicy supplied, defaults to Policy-Min-TLS-1-2-2019-07.
Emulator: each of these requests answers HTTP 200 and creates a domain. For the 'enabled with no endpoint' case DescribeDomain then resolves a live domain whose DomainEndpointOptions read {'EnforceHTTPS': True, 'CustomEndpointEnabled': True} with no CustomEndpoint and no CustomEndpointCertificateArn at all — a custom endpoint turned on with nothing behind it.
Why: the client gets a success for a configuration that cannot work and a resource it did not intend to own; a reconciler comparing desired against actual state sees CustomEndpointEnabled=true with no endpoint and has no error to act on. The unknown-certificate case additionally hides a typo'd or cross-account ARN until TLS termination fails.
Fix: the same DomainEndpointOptions validation path in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs — completeness check on the (CustomEndpoint, CustomEndpointCertificateArn) pair when CustomEndpointEnabled is true, plus an existence lookup of the certificate in the caller's account — and it must refuse BEFORE any domain state is written.
Traces: acm__opensearch.half-specified-custom-endpoint-rejected.

## Evidence
From the behavioral replay (suite simparity-behavioral-ga-prod-v1), acm__opensearch 0/2 passed.

Cause 1 — acm__opensearch.custom-endpoint-not-covered-by-its-certificate-rejected: AssertionError: expected ValidationException, got 'OK' with a full DomainStatus: {'DomainName': 'sim2real-ce-...', 'Created': True, 'Processing': True, ..., 'DomainEndpointOptions': {'EnforceHTTPS': True, 'CustomEndpointEnabled': True, 'CustomEndpoint': 'dashboards.sim2real-ce-....example.com', 'CustomEndpointCertificateArn': 'arn:aws:acm:us-west-2:...:certificate/...'}, ...}. The certificate had been imported moments earlier with a single SAN naming an internal ALB hostname, NOT the 'dashboards.<token>.example.com' passed as CustomEndpoint. Expected: HTTP 400 ValidationException, message 'Certificate <cert-arn> does not contain the custom domain name dashboards.<token>.example.com.', and DescribeDomain answering ResourceNotFoundException.

Cause 2 — acm__opensearch.half-specified-custom-endpoint-rejected: AssertionError at `assert _absent(ctx, tok) == "ResourceNotFoundException", f"{label}: the refusal must leave no domain"` — 'expected a rejection, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'DomainStatus': {'DomainName': 'sim2real-ce-8d2a2ba5c9e8', 'Created': True, ..., 'DomainEndpointOptions': {'EnforceHTTPS': True, 'CustomEndpointEnabled': True}, ...}}' — i.e. the first half-specified request (CustomEndpointEnabled=true, no CustomEndpoint, no CustomEndpointCertificateArn) was accepted and the domain is readable afterwards. Expected for that leg: HTTP 400 ValidationException 'Please provide both CustomEndpoint and CustomEndpointCertificateArn fields to create a custom endpoint.' and no domain.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2).
2. Mint a self-signed RSA-2048 leaf whose only SAN is 'internal-<token>-1234567890.us-west-2.elb.example.com' and acm.import_certificate() it; keep the returned ARN.
3. Cause 1: opensearch.create_domain(DomainName='<token>', EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, NodeToNodeEncryptionOptions={'Enabled':True}, EncryptionAtRestOptions={'Enabled':True}, DomainEndpointOptions={'EnforceHTTPS':True,'CustomEndpointEnabled':True,'CustomEndpoint':'dashboards.<token>.example.com','CustomEndpointCertificateArn':<the arn>}) — observe HTTP 200 and a created domain instead of 400 ValidationException 'Certificate <arn> does not contain the custom domain name dashboards.<token>.example.com.'
4. Cause 2, leg a: the same call with DomainEndpointOptions={'EnforceHTTPS':True,'CustomEndpointEnabled':True} — HTTP 200; describe_domain() then shows DomainEndpointOptions {'EnforceHTTPS': True, 'CustomEndpointEnabled': True}. Expected 400 ValidationException 'Please provide both CustomEndpoint and CustomEndpointCertificateArn fields to create a custom endpoint.' and no domain.
5. Cause 2, leg b: as leg a but with CustomEndpoint set and CustomEndpointCertificateArn omitted — same expected refusal.
6. Cause 2, leg c: fully specified but with CustomEndpointCertificateArn mutated to a well-formed ARN naming no certificate — expected 400 ValidationException 'ACM Certificate <arn> not found in your account.' and no domain.
7. Re-replay both check ids in trace_ids to confirm.
