Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-datasource-function-resolver-pipeline
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-datasource-function-resolver-pipeline appsync-source-api-association-auto-merge-reconfigure
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.dns.GRAPHQL
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.dns.REALTIME
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.GRAPHQL
Issue-Key: appsync|CreateGraphqlApi|2xx->2xx|$.graphqlApi.uris.REALTIME
Issue-Commit: 621b0aac2bdb395afe23e9d5f994db1d7c2eedcd

# AppSync CreateGraphqlApi publishes uris/dns as a bare loopback host:port, not an AppSync endpoint pair

> _Note: this issue was identified against the code base at commit `621b0aac2bdb395afe23e9d5f994db1d7c2eedcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: CreateGraphqlApi's response projection fills GraphqlApi$uris and GraphqlApi$dns with a raw loopback address and port instead of the AppSync endpoint pair the corpus binds, and it uses a different port for the GRAPHQL and the REALTIME entry.

AWS: Smithy (com.amazonaws.appsync, CreateGraphqlApi -> GraphqlApi): `uris` is a MapOfStringToString documented as "The URIs" and `dns` a MapOfStringToString documented as "The DNS records for the API"; the model constrains neither value, so this is recorded behavior only for the exact text. What AWS's recorded response establishes about the SHAPE of those two members is: (a) `uris` is keyed GRAPHQL and REALTIME, the GRAPHQL value is an `https://` URL ending in `/graphql` and the REALTIME value is a `wss://` URL ending in `/graphql`; (b) `dns` is keyed GRAPHQL and REALTIME and each value is the bare HOST of the corresponding `uris` entry -- a host name with NO scheme, NO port and NO path; and (c) the GRAPHQL and REALTIME entries are two endpoints of the SAME API, derived from the same api identity, not two independently allocated listeners. The fixture asserts all four members through bindings ({{uris_graphql}}, {{uris_realtime}}, {{dns_graphql}}, {{dns_realtime}}), i.e. the concrete host is explicitly NOT the requirement -- the requirement is that the four values form that self-consistent, resolvable GRAPHQL/REALTIME endpoint pair.

Emulator: the replayed response answers 200 with $.graphqlApi.uris.GRAPHQL = 'https://127.0.0.1:<portA>/graphql', $.graphqlApi.uris.REALTIME = 'wss://127.0.0.1:<portB>/graphql', $.graphqlApi.dns.GRAPHQL = '127.0.0.1:<portA>' and $.graphqlApi.dns.REALTIME = '127.0.0.1:<portB>', with portA != portB and neither equal to the data listener the replay reached the emulator on. So the dns entries are not host names at all (they carry a port, which a DNS record cannot), and the two endpoints are unrelated per-API port allocations rather than one API's GRAPHQL/REALTIME pair. None of the four asserted members satisfies its binding, so the step diverges even though the status matches.

Why: a client that reads the API's endpoint out of the create response gets values it cannot use as AWS documents them. Anything that treats `dns.GRAPHQL`/`dns.REALTIME` as a host -- a Route53 alias/CNAME record, a custom-domain association, an SDK or CDK construct that composes host + path, a connection-init that derives the realtime host from the GraphQL host -- is handed 'host:port' and either fails to resolve or builds a malformed URL, and a client that derives the realtime endpoint from the GraphQL endpoint lands on a port that serves nothing. The observable contract (dns = host of uri; GRAPHQL and REALTIME are the same API's two endpoints) is broken independently of which host or port the environment happens to use.

Fix: the CreateGraphqlApi handler's response projection for the GraphqlApi endpoint members in the appsync service crate (the same projection ReadGraphqlApi/GetGraphqlApi/ListGraphqlApis reuse, so the fix must hold for every operation that returns a GraphqlApi): derive both `uris` entries from one endpoint identity for the API and derive each `dns` entry as the HOST component of the matching `uris` entry, with no port or path, rather than formatting 'addr:port' into `dns` and allocating GRAPHQL and REALTIME independently.

Related open issues, none of which carries this step's replay key: '20261110T162302Z-appsync-cloudwatch-appsync-publishes-real-aws-hostnames-in-graphqlapi-uris.md', '20261111T065707Z-appsync-3-net-new-causes-graphqlapi-uris-dns.md' and '20261113T045825Z-appsync-publishes-graphqlapi-uris-graphql-on-a-real.md' all report the opposite symptom (uris published on real-AWS appsync-api.<region>.amazonaws.com hosts the emulator does not serve). This divergence is the loopback form and the uris/dns inconsistency within it, and it is reported by the wire corpus, so it is filed separately and named here for the fixer.

Traces: appsync-datasource-function-resolver-pipeline#task#1 (representative), appsync-source-api-association-auto-merge-reconfigure#setup#2.

## Evidence
Fixture appsync-datasource-function-resolver-pipeline, step task#1, CreateGraphqlApi, expected status 200 / actual status 200, verdict mismatch. Diverging members reported by the replay:
  $.graphqlApi.uris.GRAPHQL:  expected '{{uris_graphql}}'   (binding), got 'https://127.0.0.1:43923/graphql'
  $.graphqlApi.uris.REALTIME: expected '{{uris_realtime}}'  (binding), got 'wss://127.0.0.1:39643/graphql'
  $.graphqlApi.dns.GRAPHQL:   expected '{{dns_graphql}}'    (binding), got '127.0.0.1:43923'
  $.graphqlApi.dns.REALTIME:  expected '{{dns_realtime}}'   (binding), got '127.0.0.1:39643'
Cluster divergence_key: service=appsync|CreateGraphqlApi|status=200->200|keys=$.graphqlApi.dns.GRAPHQL,$.graphqlApi.dns.REALTIME,$.graphqlApi.uris.GRAPHQL,$.graphqlApi.uris.REALTIME (affected_count 2; also appsync-source-api-association-auto-merge-reconfigure#setup#2).
The literal addresses above are this run's values and are NOT the requirement. What is environment-independent in them, and is what diverges: (1) a `dns` value that is 'host:port' instead of a bare host, and (2) a REALTIME endpoint whose authority differs from the GRAPHQL endpoint's authority (two separate ports) instead of being the same API's companion endpoint. The replay's own emulator_detail records the data listener as a third port, so neither published port is the address the emulator was reached on.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture appsync-datasource-function-resolver-pipeline and stop at step task#1 (CreateGraphqlApi, creating a GRAPHQL-type API with API_KEY auth).
3. Read the 200 response's $.graphqlApi.uris and $.graphqlApi.dns maps.
4. Observe each `dns` entry is '<addr>:<port>' rather than the bare host of the matching `uris` entry, and that uris.GRAPHQL and uris.REALTIME carry two different ports (neither being the listener the request arrived on), so all four asserted members fail their bindings. The same projection diverges at appsync-source-api-association-auto-merge-reconfigure#setup#2.
