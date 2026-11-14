Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place
Issue-Key: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place|run#10|AssertionError|assert before["Endpoint"] == outputs["DomainEndpoint"], (
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# cloudformation__iam__opensearch__sts: AWS::OpenSearchService::Domain's DomainEndpoint attribute is a synthesized AWS-shaped hostname, not the domain's published DomainStatus.Endpoint, so the stack output addresses nothing (1 cause)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAttDomainEndpoint

## Description
Cause 1 of 1 — the CloudFormation provisioner for AWS::OpenSearchService::Domain mints its own `DomainEndpoint` attribute value instead of returning the endpoint the domain it created published.
AWS: `Fn::GetAtt <domain>.DomainEndpoint` on AWS::OpenSearchService::Domain returns exactly the value OpenSearch's DescribeDomain reports as `DomainStatus$Endpoint` — "the domain-specific endpoint used to submit index, search, and data upload requests". The two are the SAME string by construction; a stack output carrying DomainEndpoint is the documented way a template hands its consumers the address to send signed index/search requests to. Recorded behavior only for the emulator's values.
Emulator: the two disagree. DescribeDomain reports the emulator's own reachable endpoint (host:port plus the reserved `/_aws/es/<domain>` data-plane prefix), while the stack output reports an invented `search-<domain>-<suffix>.<region>.es.amazonaws.com` hostname that resolves nowhere and that the emulator never serves. The specific hosts, ports and generated suffixes are per-environment values; the requirement is that GetAtt DomainEndpoint equals DescribeDomain's DomainStatus.Endpoint for the same domain.
Why: every consumer of the stack — an exported output, a !GetAtt into a Lambda environment variable, an operator copying the output — is handed an address the emulator does not answer on, so the data plane is unreachable through the supported CloudFormation path even though the domain exists and its own endpoint works. The divergence is silent: the stack reports CREATE_COMPLETE with a plausible-looking hostname.
Fix: the AWS::OpenSearchService::Domain provisioner's attribute resolution in the cloudformation service crate — read the created domain's DomainStatus.Endpoint through OpenSearch rather than formatting a hostname, so DomainEndpoint (and the Endpoints map, where present) is the value the domain published.
Traces: cloudformation__iam__opensearch__sts.deploy-opensearch-1-changeset-swaps-allowed-and-denied-roles-in-place

## Evidence
DescribeDomain Endpoint '127.0.0.1:42463/_aws/es/sim2real-os1-a1abb898818a' != stack output DomainEndpoint 'search-sim2real-os1-a1abb898818a-8sh3tqaaaaaa.us-west-2.es.amazonaws.com'. (The host, port and the generated suffix are per-run values; the requirement is equality between the stack's DomainEndpoint attribute and DescribeDomain's DomainStatus.Endpoint, not either literal.)

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::OpenSearchService::Domain (OpenSearch_2.11, one t3.small.search node, gp3/10, an AccessPolicies document naming an IAM role) and an Output `DomainEndpoint` = !GetAtt <domain>.DomainEndpoint.
2. Wait for CREATE_COMPLETE and read the stack's Outputs.
3. Call OpenSearch DescribeDomain on the same DomainName and read DomainStatus.Endpoint.
4. Compare: the stack output is a synthesized search-<name>-<suffix>.<region>.es.amazonaws.com hostname, the DescribeDomain value is the emulator's served endpoint — they differ, and only the latter answers a signed request.
