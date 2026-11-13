Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-rbp-omitted-policy-version-defaults-to-2008-10-17
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-rbp-omitted-policy-version-defaults-to-2008-10-17|run#10|AssertionError|assert stored_bare.get("Version") == "2008-10-17", (
Issue-Commit: 381a390d6eaf89a951de4c9ff222f8a629952686

# dynamodb: PutResourcePolicy stores a document with no Version element verbatim — AWS defaults it to Version 2008-10-17, so GetResourcePolicy serves a policy with no Version

> _Note: this issue was identified against the code base at commit `381a390d6eaf89a951de4c9ff222f8a629952686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One net-new root cause in DynamoDB's resource-based-policy surface (PutResourcePolicy / GetResourcePolicy). It is a representation defect in what the service STORES, not a refusal.

Cause 1 of 1: a policy document submitted WITHOUT a top-level `Version` element is stored, and served back, exactly as submitted — with no `Version` at all.

AWS:      Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): PutResourcePolicy{ResourceArn (required, ResourceArnString, @length 1..1283), Policy (required, ResourcePolicy), ExpectedRevisionId, ConfirmRemoveSelfResourceAccess} -> {RevisionId}; GetResourcePolicy{ResourceArn (required)} -> {Policy, RevisionId}. The model constrains only the ARN and the revision id; it says nothing about the contents of the policy document, so for the document itself this is recorded behavior only. The recording shows that AWS normalises the submitted document before storing it: a document with no `Version` element comes back from GetResourcePolicy with the IAM default `"Version": "2008-10-17"` filled in, and a document that DID declare `"Version": "2012-10-17"` comes back with that declared value untouched. Apart from the defaulted `Version`, the rest of the stored document is semantically unchanged (AWS re-serialises it: whitespace dropped, a one-element Action list may come back as a bare string). Both calls succeed — there is no error code or non-2xx status involved in this cause.

Emulator: the put succeeds and returns a RevisionId, but GetResourcePolicy serves the document back with NO `Version` key: `{'Statement': [{'Effect': 'Allow', 'Principal': {'AWS': 'arn:aws:iam::<account>:root'}, 'Action': 'dynamodb:GetItem', 'Resource': '<table ARN>'}]}`. The control arm (a document that declares `Version` 2012-10-17) does round-trip correctly, so the defect is specifically the missing default, not general mangling of the document. (The account id, table ARN and the single action above are values this run's request supplied; the requirement is that the served document carries `"Version": "2008-10-17"` whenever the request omitted Version, and the request's own value whenever it declared one.)

Why:      the policy-language version is not cosmetic — it selects the evaluation semantics. Policy variables and several condition forms only evaluate under 2012-10-17; under the 2008-10-17 default they are treated as literals. A client that puts a Version-less document and then reads it back to decide what the grant means, to diff declared-vs-actual policy (CloudFormation/CDK drift, Terraform plan, policy linters), or to re-put the document it just read, sees a document AWS would never serve: the diff reports a spurious change on every refresh, and any tool keying on `Version` to pick an evaluation mode gets no value at all instead of the documented default.

Fix:      PutResourcePolicy's policy-document normalisation, in the shared resource-policy state used by PutResourcePolicy/GetResourcePolicy/DeleteResourcePolicy — the document must be stored with the IAM default `Version` 2008-10-17 inserted when the request omits it, and with a declared `Version` preserved verbatim.

Related but not covering this: the open issue 20261108T051530Z-dynamodb-6-net-new-resource-policy-causes-putresourcepolicy.md bundles six other resource-policy causes (over-permissive principal accept, four refusal-message mismatches, and a put that never settles). None of its causes or replay keys is this one — it never touches the stored document's Version element.

Traces:   dynamodb.ddb-rbp-omitted-policy-version-defaults-to-2008-10-17

## Evidence
Replay output for check dynamodb.ddb-rbp-omitted-policy-version-defaults-to-2008-10-17 (verdict: mismatch):

  File "check_dynamodb_ddb_rbp_omitted_policy_version_defaults_to_2008_10_17.py", line 35, in run
    assert stored_bare.get("Version") == "2008-10-17", (
  AssertionError: a policy put without Version is stored as {'Statement': [{'Effect': 'Allow', 'Principal': {'AWS': 'arn:aws:iam::000000000000:root'}, 'Action': 'dynamodb:GetItem', 'Resource': 'arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-rbp-ver-<run-token>-bare'}]}; real AWS fills in the IAM default Version 2008-10-17

Expected (real AWS): GetResourcePolicy on the table whose policy was put WITHOUT a Version element serves a document whose top-level `Version` is the string "2008-10-17"; everything else in the document is semantically identical to what was submitted.
Actual (emulator): the served document has no `Version` key at all.

Control arm that PASSED in the same check (so it is not a general serialisation fault): the sibling table whose policy declared `"Version": "2012-10-17"` is served back with `Version` == "2012-10-17".

Both PutResourcePolicy calls returned 2xx with a RevisionId on the emulator, and GetResourcePolicy returned 2xx; no error code or status is in dispute for this cause. The account id (000000000000), the region, and the generated table-name suffix in the text above are per-run values, not part of the requirement.

## Steps to Reproduce
Against the emulator (region/account/table name are free choices):
1. CreateTable with BillingMode PAY_PER_REQUEST, KeySchema id (HASH), AttributeDefinitions id:S; wait for ACTIVE. Do this twice, giving a "bare" table and a "declared" table.
2. PutResourcePolicy on the bare table's TableArn with a document that OMITS Version:
   {"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"dynamodb:GetItem","Resource":"<bare TableArn>"}]}
3. PutResourcePolicy on the declared table's TableArn with the same statement plus "Version":"2012-10-17".
4. Poll GetResourcePolicy on each ARN until it serves the RevisionId each put returned, then json-parse the returned Policy.
5. Expect the declared table's stored document to carry Version "2012-10-17" (this passes today) AND the bare table's stored document to carry Version "2008-10-17". Observe instead that the bare table's stored document has no Version key.

Or: replay the behavioral suite and re-run the dynamodb check listed in trace_ids.
