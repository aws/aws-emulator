Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: simpledbv2.export-lifecycle-lands-verified-data-in-s3 simpledbv2.list-exports-pagination-surfaces-own-exports simpledbv2.start-domain-export-idempotent-retry-returns-same-export simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: simpledbv2|CreateDomain|InvalidAction
Issue-Commit: 7722f7a491f17adc9c16c0d1d844ff39f4bc9907

# REOPENS: SimpleDB 2009-04-15 CreateDomain on the sdb endpoint is still refused 400 InvalidAction "unknown action: CreateDomain", so the whole simpledbv2 export surface stays unreachable

> _Note: this issue was identified against the code base at commit `7722f7a491f17adc9c16c0d1d844ff39f4bc9907`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
REOPENS 20261109T154301Z-simpledbv2-simpledb-2009-04-15-query-api-unserved-on.md: its fix left the call refused. All four behavioral checks of this service still error on their very first call, with the identical refusal that issue was filed for, so nothing of the domain-export lifecycle is exercised.

Cause 1 of 1: a form-encoded Amazon SimpleDB 2009-04-15 request on the sdb endpoint's root path is not resolved to the query-API surface, so CreateDomain is answered by the framework's routing-error default instead of by a handler.

  AWS: the sdb endpoint (Smithy: com.amazonaws.simpledbv2#SimpleDBv2, aws.api#service endpointPrefix "sdb" / arnNamespace "sdb", aws.auth#sigv4 name "sdb", protocol restJson1, us-east-1 special-case endpoint https://sdb.amazonaws.com) serves BOTH the three modelled /v2/* export operations and the Amazon SimpleDB 2009-04-15 AWS Query API on "/" — a form-encoded POST carrying Action=<action>&Version=2009-04-15 plus that action's parameters. This repo carries no Smithy model for the 2009-04-15 surface, so for that half this is documented/recorded behavior only; the v2 model is cited only for the shared wire identity. CreateDomain answers HTTP 200 with <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"> whose only content is <ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata>, and is idempotent on a name the account already holds; the domain it creates is the domain a later v2 StartDomainExport/ListExports resolves domainName against. When an Action genuinely is not one of the service's, AWS words the refusal InvalidAction as "The action <name> is not valid for this web service." (fixed text plus the Action value from the request) — which is also the wording this checkout's own handler module already carries (service/sdb_query_api/envelope.rs invalid_action).
  Emulator: sdb.create_domain(...) — POST "/" with Action=CreateDomain&DomainName=<name>&Version=2009-04-15 — is answered HTTP 400, Error Code `InvalidAction`, Error Message `unknown action: CreateDomain`. The fixed text is "unknown action: " and the variable part is the request's Action value. That message is NOT produced by this service's query-API module (which would say "The action CreateDomain is not valid for this web service." and is reached only for an action outside its own catalogue, where CreateDomain is listed), so the request never reaches the implemented surface at all: it falls through to the protocol-default routing error the generated entry renders for a request that named no modelled operation. Hypothesis: the operation-resolution seam does not recognise the request — addresses_query_api reads the Action out of the request body, and the body is not readable at that point (req.body().bytes() yields nothing), so no Action is seen, the query-string fallback finds none either, and the generated routing table (which binds only POST /v2/*) refuses POST "/".
  Why: a client doing exactly what AWS requires — create a domain, write items into it with PutAttributes, then export it — is refused at its first call, so the export lifecycle (status progression to SUCCEEDED, clientToken idempotency, token reuse with changed parameters, ListExports pagination) is unobservable and the service's only client-reachable way to get a domain does not exist. The code is wrong as well as the status: InvalidAction tells the SDK the action is not part of this service, when it is served on this endpoint, and InvalidAction is not a modelled error of any SimpleDB action, so no client error branch handles it. Even on a genuinely unknown action the message wording diverges from AWS's.
  Fix: the simpledbv2 service's operation-resolution seam and the query-API addressing predicate it calls — the seam must claim a form-encoded POST on "/" naming an Action (reading the parameters from a body that is actually available at that point, or from the request parts the router does buffer) and hand it to the already-implemented 2009-04-15 handlers, rather than letting it fall through to the generated routing table's default refusal. File and line citations are under Traces.
  Traces: simpledbv2.export-lifecycle-lands-verified-data-in-s3, simpledbv2.list-exports-pagination-surfaces-own-exports, simpledbv2.start-domain-export-idempotent-retry-returns-same-export, simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected. Code citations: services/service-com-amazonaws-simpledbv2-simpledbv2/src/service.rs:104-123 (override_operation, which returns Some only when addresses_query_api matches) and :148-160 (override_routing_error returns None, so the protocol-default routing refusal is rendered); services/service-com-amazonaws-simpledbv2-simpledbv2/src/service/sdb_query_api.rs:111-123 (addresses_query_api) and :201-213 (the action dispatch, where "CreateDomain" IS a served arm); services/service-com-amazonaws-simpledbv2-simpledbv2/src/service/sdb_query_api/parameters.rs:36-53 (QueryParameters::of, which assumes the router has buffered the body).

Related, not covering this: the open issue 20261109T154301Z-s3-simpledbv2-domain-export-to-s3-checks-blocked-simpledb-2009.md carries the two cross-service (s3__simpledbv2) checks for the same refusal under a different service heading; 20261110T055258Z-sdb-wire-replay-domain-export-lifecycle-fixture-refused-end.md is a wire-corpus issue keyed to fixture steps, not to these behavioral checks.

## Evidence
All four simpledbv2 behavioral checks error identically on their first SimpleDB call:

  botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateDomain operation: unknown action: CreateDomain

raised from sdb.create_domain(DomainName=<unique name>) with sdb = ctx.client("sdb") (Amazon SimpleDB, API version 2009-04-15) — a form-encoded POST to "/" on the same sdb endpoint the v2 client uses.

Expected (AWS): HTTP 200, body <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"><ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata></CreateDomainResponse>; the domain then exists and is a valid domainName for StartDomainExport. (RequestId and BoxUsage are per-run values, not requirements.)
Actual (emulator): HTTP 400, Error Code "InvalidAction", Error Message "unknown action: CreateDomain" — fixed text "unknown action: " plus the request's Action value.

The same fall-through blocks the other legacy calls these checks need (PutAttributes, DeleteDomain), so no assertion about the v2 export lifecycle was reached: no exportStatus progression, no clientToken idempotency, no token-reuse rejection, no ListExports page.

That the refusal is the framework's routing default and not this service's own is shown by the message text: the checkout already contains a complete 2009-04-15 handler set whose own InvalidAction wording is "The action <name> is not valid for this web service." and whose dispatch lists "CreateDomain" as a served action, so a request that reached it could not have been answered "unknown action: CreateDomain".

Suite report for this run: simpledbv2 0/4 (0%), 4 failed; every failure is this error.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Point a botocore client at it: sdb = boto3.client("sdb") (API version 2009-04-15).
3. Call sdb.create_domain(DomainName="behav-domain-1") — i.e. POST "/" with Action=CreateDomain&DomainName=behav-domain-1&Version=2009-04-15. Observe HTTP 400 / InvalidAction / "unknown action: CreateDomain" instead of a 200 CreateDomainResponse in the http://sdb.amazonaws.com/doc/2009-04-15/ namespace.
4. Same for sdb.put_attributes(DomainName="behav-domain-1", ItemName="item-widget", Attributes=[{"Name":"color","Value":"blue","Replace":True}]) and sdb.delete_domain(DomainName="behav-domain-1").
5. Then call the v2 surface: StartDomainExport(domainName="behav-domain-1", s3Bucket=<bucket>) and observe that no client-created domain can ever be resolved.
6. Re-replay checks: simpledbv2.export-lifecycle-lands-verified-data-in-s3, simpledbv2.list-exports-pagination-surfaces-own-exports, simpledbv2.start-domain-export-idempotent-retry-returns-same-export, simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected.
