Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: simpledbv2.export-lifecycle-lands-verified-data-in-s3 simpledbv2.list-exports-pagination-surfaces-own-exports simpledbv2.start-domain-export-idempotent-retry-returns-same-export simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: simpledbv2|CreateDomain|InvalidAction
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# SimpleDB 2009-04-15 query API unserved on the sdb endpoint: CreateDomain/PutAttributes/DeleteDomain refused with 400 InvalidAction

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Every behavioral check of this service fails on its very first call, all with the same refusal, so the whole domain-export surface is unreachable end to end.

Cause 1 of 1: the legacy Amazon SimpleDB 2009-04-15 AWS Query API is not served on the `sdb` wire, so no domain can be created and nothing downstream of it is exercised.

  AWS: the `sdb` endpoint (Smithy: com.amazonaws.simpledbv2#SimpleDBv2, aws.api#service endpointPrefix "sdb" / arnNamespace "sdb", aws.auth#sigv4 name "sdb", us-east-1 special case https://sdb.amazonaws.com) serves BOTH the restJson1 export operations under /v2/* (GetExport, ListExports, StartDomainExport) and the Amazon SimpleDB 2009-04-15 AWS Query API on "/" — a form-encoded POST carrying Action=<op>&Version=2009-04-15 plus that action's parameters. CreateDomain is a modeled action of that API: it answers HTTP 200 with an XML <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"> whose only content is <ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata>, and it is idempotent on a name that already exists. PutAttributes and DeleteDomain answer 200 with their own <PutAttributesResponse>/<DeleteDomainResponse> envelopes in the same namespace, and DeleteDomain on an absent domain is a 200 no-op. The domain those calls create is the same domain the v2 StartDomainExport/ListExports resolve `domainName` against — the v2 model's NoSuchDomainException ("The specified domain does not exist.") is the answer only for a name that was never created. The repo carries no Smithy model for the 2009-04-15 surface (there is no service crate for it under services/), so for that half this is documented/recorded behavior only; the v2 model is cited above only for the shared wire identity (endpoint prefix and signing name).
  Emulator: the 2009-04-15 request reaches the simpledbv2 service (it claims the request because the resolved route key equals its signing name `sdb`), names no modeled operation, and falls through to the protocol-default routing error, so the caller gets HTTP 400 with Error Code `InvalidAction` and message `unknown action: CreateDomain` — fixed text "unknown action: " followed by the Action parameter value taken from the request. Nothing is created and the export operations can therefore never see a caller-created domain: the domain registry they resolve against is populated only from the SIMPLEDB_EXISTING_DOMAINS environment variable plus the domain of already-stored exports.
  Why: a client that does what AWS requires — create a domain, write items into it, then export it — is refused at the first call with an error code (InvalidAction) that tells it the action does not exist on this service, which is wrong: the action exists on this endpoint. Because `InvalidAction` is not a modeled error of any SimpleDB action, SDK retry/branching logic cannot handle it, and no amount of client-side correction makes the export surface usable, since the only other way to get a domain is an out-of-band operator environment variable that no AWS client can set. Even the refusal's wording diverges: when an action genuinely is unknown, AWS words InvalidAction as "The action <name> is not valid for this web service.", not "unknown action: <name>".
  Fix: the simpledbv2 service's request-claiming/operation-resolution seam plus new handlers and state for the 2009-04-15 query surface — route a form-encoded POST on "/" by its `Action` field to CreateDomain / PutAttributes / DeleteDomain handlers, serialize their 2009-04-15 XML response envelopes, and hold the created domains and their items in this service's own state so the export operations' domain registry resolves a just-created domain instead of only operator-declared ones.
  Traces: simpledbv2.export-lifecycle-lands-verified-data-in-s3, simpledbv2.list-exports-pagination-surfaces-own-exports, simpledbv2.start-domain-export-idempotent-retry-returns-same-export, simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected. Code citations: services/service-com-amazonaws-simpledbv2-simpledbv2/src/service.rs:76-96 (can_handle claims any request whose route key is `sdb`; override_operation returns None) and src/service.rs:112-124 (the unmatched request renders the protocol-default routing error); src/state.rs:28-48 (the domain registry is fed only by SIMPLEDB_EXISTING_DOMAINS, with the comment that "NOTHING on this wire creates a domain").

Note: the one open issue for this service (20261023T081506Z-sdb-simpledbv2-client-errors-carry-non-aws-validation-message.md) is about the wording of three v2 client-error messages and does not cover this unserved legacy surface.

## Evidence
All four checks of this service error identically on their first SimpleDB call:

  botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateDomain operation: unknown action: CreateDomain

raised from sdb.create_domain(DomainName=<unique name>) where sdb = ctx.client("sdb") (Amazon SimpleDB, API version 2009-04-15), i.e. a form-encoded POST to "/" on the same `sdb` endpoint the v2 client uses.

Expected (AWS): HTTP 200, body <CreateDomainResponse xmlns="http://sdb.amazonaws.com/doc/2009-04-15/"><ResponseMetadata><RequestId>..</RequestId><BoxUsage>..</BoxUsage></ResponseMetadata></CreateDomainResponse>; the domain then exists and is a valid `domainName` for StartDomainExport. (RequestId/BoxUsage are per-run values, not requirements.)
Actual (emulator): HTTP 400, Error Code "InvalidAction", Error Message "unknown action: CreateDomain".

The same refusal blocks the other legacy calls these checks need: sdb.put_attributes(...) and sdb.delete_domain(...). Because the first call fails, no assertion about the v2 export lifecycle (status progression PENDING→IN_PROGRESS→SUCCEEDED, clientToken idempotency, token reuse with changed parameters, ListExports pagination) was reached at all.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it: sdb = boto3.client("sdb") (API version 2009-04-15).
2. Call sdb.create_domain(DomainName="behav-domain-1") — i.e. POST "/" with Action=CreateDomain&DomainName=behav-domain-1&Version=2009-04-15. Observe HTTP 400 / InvalidAction / "unknown action: CreateDomain" instead of a 200 CreateDomainResponse.
3. Same for sdb.put_attributes(DomainName="behav-domain-1", ItemName="item", Attributes=[{"Name":"k","Value":"v"}]) and sdb.delete_domain(DomainName="behav-domain-1").
4. Then call simpledbv2 StartDomainExport(domainName="behav-domain-1", s3Bucket=<bucket>) and observe that, with no way to create the domain on the wire, the export surface answers NoSuchDomainException for every domain a client could have made.
5. Re-replay checks: simpledbv2.export-lifecycle-lands-verified-data-in-s3, simpledbv2.list-exports-pagination-surfaces-own-exports, simpledbv2.start-domain-export-idempotent-retry-returns-same-export, simpledbv2.start-domain-export-token-reuse-with-changed-params-rejected.
