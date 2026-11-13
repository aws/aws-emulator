Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|run#28|PublishedUrlRejected|base, route_headers = ctx.endpoint_route(domain)
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# cloudfront__kms__s3: a distribution's DomainName is published as <id>.cloudfront.net, an address the emulator does not serve, so the OAC/SSE-KMS edge contract cannot be exercised

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: CreateDistribution announces an AWS production edge host as the distribution's DomainName, so a client that follows the address the emulator itself published is sent off the emulator.
AWS: recorded behavior only — the model types DomainName as a string. In AWS the published DomainName is the address the distribution is actually reachable at; a client is expected to issue viewer requests to it. In an emulator the announced host must be one the emulator serves (its own host:port, or a per-resource listener on its own host), because the published value is the only address the caller has.
Emulator: the distribution's DomainName comes back as '<distribution-id>.cloudfront.net'. Using it as the endpoint is refused by the harness' egress guard: "endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". Nothing at the edge is therefore reachable, and the check that an OAC-protected S3 origin is denied when the KMS key policy omits the cloudfront.amazonaws.com principal never gets to send a viewer request.
Why: every viewer-facing behaviour of a distribution (origin access control, SSE-KMS denial at the edge, cache behaviour) is unobservable, and a client wired to the published DomainName talks to an address outside the emulator.
Fix: the DomainName the CreateDistribution/GetDistribution serializer publishes — announce a host the emulator serves for that distribution. Related open issues report the same announcement under other service tokens (20261109T143515Z-cloudfront-logs-a-cloudfront-distribution-s-domainname-is-published-as.md, 20261109T152222Z-apigateway-cloudfront-s3-a-distribution-s-domainname-is.md); neither carries this check.
Traces: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied.

## Evidence
Expected: the distribution's DomainName is an address the emulator serves, so a viewer request through it can be made. Actual: simparity.published.PublishedUrlRejected — "endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".

## Steps to Reproduce
1. Create an S3 bucket with SSE-KMS default encryption under a customer managed key whose key policy does NOT grant cloudfront.amazonaws.com.
2. CreateDistribution with that bucket as an S3 origin protected by an Origin Access Control; wait for Deployed.
3. Read DomainName from the response and issue a viewer GET to https://<DomainName>/<key>: the published host must be one the emulator serves, and the request must be answered (AWS answers 403 because the key policy denies CloudFront). The emulator publishes <id>.cloudfront.net, which it does not serve.
