Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: es.es-index-document-search-roundtrip es.index-name-uppercase-rejected-lowercase-control-indexes
Issue-Shape-Id: com.amazonaws.elasticsearchservice#AmazonElasticsearchService2015
Issue-Key: es.es-index-document-search-roundtrip|run#6|PublishedUrlRejected|base = ctx.endpoint_url(endpoint)
Issue-Key: es.index-name-uppercase-rejected-lowercase-control-indexes|run#6|PublishedUrlRejected|base = ctx.endpoint_url(endpoint)
Issue-Commit: f86c51338546a6b06507029fc2fe591abb220b62

# es DescribeElasticsearchDomain publishes DomainStatus.Endpoint on a real-AWS search-*.<region>.es.amazonaws.com host the emulator does not serve, so the Elasticsearch data plane is unaddressable

> _Note: this issue was identified against the code base at commit `f86c51338546a6b06507029fc2fe591abb220b62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeElasticsearchDomain

## Description
One net-new cause in the es (legacy Elasticsearch Service) domain read path. Both failing behavioral checks in this service stop at the same point, before any data-plane request is attempted.

Cause 1 of 1: the domain endpoint the emulator publishes for a created Elasticsearch domain is minted on an AWS production hostname instead of the address the emulator is reached on, so every client that follows it leaves the emulator.

AWS: Smithy (com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015): DescribeElasticsearchDomain, GET /2015-01-01/es/domain/{DomainName}, success code 200, output member DomainStatus of type ElasticsearchDomainStatus; its member Endpoint is documented as "The Elasticsearch domain endpoint that you use to submit index and search requests." The model puts no pattern or length constraint on Endpoint, so what the value must be is recorded behavior only: it is the host a client signs (service name 'es') and sends index/_refresh/_search/_doc requests to, and it must therefore be a host the serving endpoint actually answers on. Real AWS publishes a host it itself serves; an emulator's equivalent requirement is host ownership, i.e. the endpoint must resolve to the emulator's own listener (the host:port the DescribeElasticsearchDomain call itself was made to, or a per-resource listener the emulator owns). Neither check asserts a literal URL — the domain name, hash label and region are per-run values, never part of the requirement.

Emulator: after CreateElasticsearchDomain, DescribeElasticsearchDomain reports Processing=false and DomainStatus.Endpoint built from the AWS production template https://search-<domain-name>-<hash>.<region>.es.amazonaws.com (the replay observed values of exactly that shape for the two probe domains). The control-plane status, envelope and the rest of DomainStatus are accepted; only the announced host diverges. Handing that value to the harness's endpoint resolver is refused immediately: simparity.published.PublishedUrlRejected: "endpoint_url was given https://search-<domain>-<hash>.us-west-2.es.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The fixed text of that refusal is the harness's; the host, 127.0.0.1 and the port are per-run values.

Why: a client that creates a domain, waits for it to become searchable and then indexes or searches against DomainStatus.Endpoint is sent off the emulator entirely. Nothing in the emulator listens on *.es.amazonaws.com, so the signed PUT /<index>/_doc/1, POST /<index>/_refresh, POST /<index>/_search and GET /<index>/_doc/1 either reach real AWS, are refused by egress, or fail name resolution. Every Elasticsearch data-plane behaviour is therefore unreachable even though the control-plane create/describe calls succeed: a document round-trip cannot be observed, and the uppercase-index rejection class (HTTP 400 with error.type invalid_index_name_exception and error.reason "Invalid index name [<index>], must be lowercase", where the bracketed index name is the request's value and the rest is fixed text) cannot be exercised at all.

Fix: the domain projection served by the es DescribeElasticsearchDomain / DescribeElasticsearchDomains handlers — and whatever mints and stores the domain endpoint when CreateElasticsearchDomain accepts the domain, so the stored value is not already an AWS host — must derive the endpoint's scheme, host and port from the emulator's own serving endpoint, keeping only the per-domain label as a path segment or sub-label of a host the emulator owns. The host must never come from a hard-coded search-<name>-<hash>.<region>.es.amazonaws.com template. The same derivation belongs on EndpointsmultipleendpointsmapandanyVPC/Dashboards endpoint the domain status carries.

Traces: es.es-index-document-search-roundtrip and es.index-name-uppercase-rejected-lowercase-control-indexes, both failing at ctx.endpoint_url(DomainStatus.Endpoint) with PublishedUrlRejected host_not_own, before any signed data-plane request is sent.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service es: 7/9 passed, 2 mismatched. Both failures share one error:

  [mismatch] es.es-index-document-search-roundtrip
    simparity.published.PublishedUrlRejected: endpoint_url was given
    https://search-sim2real-behav-es-49a5db0a7c-f868931f7d461c666552c50492.us-west-2.es.amazonaws.com
    (published by the emulator or built by the check): its host is not one of the emulator's
    own hosts; a customer of the emulator needs http(s)://127.0.0.1:34243/<resource path>,
    or http(s)://<own host>:<port>/<path> on a per-resource listener

  [mismatch] es.index-name-uppercase-rejected-lowercase-control-indexes
    simparity.published.PublishedUrlRejected: endpoint_url was given
    https://search-sim2real-behav-es-950a6cb6cf-f50f84ed344c192015f7ea830d.us-west-2.es.amazonaws.com
    ... its host is not one of the emulator's own hosts ... needs http(s)://127.0.0.1:39329/...

Expected: DescribeElasticsearchDomain's DomainStatus.Endpoint is a URL whose host is the endpoint the emulator serves (the host the DescribeElasticsearchDomain request itself was made to, or a per-resource listener the emulator owns), with the per-domain label carried in the path or as a sub-label; signing it for service 'es' and sending PUT /<index>/_doc/1 reaches the emulator's Elasticsearch data plane.

Actual: DomainStatus.Endpoint is https://search-<domain-name>-<hash>.<region>.es.amazonaws.com — a real AWS host the emulator does not serve. The specific domain names, hash labels, region us-west-2 and the ports 34243/39329 above are per-run values filling the template, not requirements.

Both checks reached this point only because the control plane worked: CreateElasticsearchDomain (ElasticsearchVersion 7.10, t3.small.elasticsearch x1, gp2 10GiB, account-root AccessPolicies) was accepted and DescribeElasticsearchDomain reported Processing=false with a non-empty Endpoint. The divergence is purely the published host.

Related but NOT covering this: open issue 20261110T150745Z-es-getapplication-publishes-the-application-endpoint-on-a.md is the same class of defect on a different surface (OpenSearch GetApplication's $.endpoint on opensearch.amazonaws.com); it says nothing about DomainStatus.Endpoint of the legacy Elasticsearch domain API.

## Steps to Reproduce
1. Boot the emulator from this checkout and note the host:port its data listener is reached on.
2. es.CreateElasticsearchDomain(DomainName=<3-28 lowercase chars>, ElasticsearchVersion="7.10", ElasticsearchClusterConfig={InstanceType: t3.small.elasticsearch, InstanceCount: 1}, EBSOptions={EBSEnabled: true, VolumeType: gp2, VolumeSize: 10}, AccessPolicies=<policy allowing arn:aws:iam::<account>:root es:* on the domain>).
3. Poll es.DescribeElasticsearchDomain(DomainName=...) until DomainStatus.Processing is false and DomainStatus.Endpoint is present.
4. Read DomainStatus.Endpoint. Observed: a host of the form search-<domain>-<hash>.<region>.es.amazonaws.com. Expected: the emulator's own serving host:port.
5. SigV4-sign (service 'es') and send PUT https://<Endpoint>/<lowercase-index>/_doc/1 with {"title":"sim2real"}. Observed: the address is not one the emulator serves, so the request never reaches it (egress refusal / name resolution failure). Expected: HTTP 200 or 201, then POST /<index>/_refresh -> 200 and POST /<index>/_search -> 200 with the indexed _source.
6. Repeat step 5 with an index name containing uppercase characters. Expected (unreachable today): HTTP 400 with error.type invalid_index_name_exception and error.reason "Invalid index name [<index>], must be lowercase", and the lowercased form still absent (404 index_not_found_exception).
