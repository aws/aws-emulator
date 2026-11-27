Issue-Source: trace-sweep-divergence
Issue-Trace: glue-account-policy-paginated-listing
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-account-policy-paginated-listing
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetResourcePolicies|2xx->2xx|$.GetResourcePoliciesResponseList[*]
Issue-Commit: 3e9576030bb65d42a81780c53475bd039f2ab2f6

# Glue GetResourcePolicies returns an empty list instead of the account's stored resource policies

> _Note: this issue was identified against the code base at commit `3e9576030bb65d42a81780c53475bd039f2ab2f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicies

## Description
Cause 1 of 1: after a Data Catalog resource policy has been stored for the account, GetResourcePolicies answers 200 with an empty GetResourcePoliciesResponseList, so the policy that exists is invisible through the listing path.

AWS: Smithy (com-amazonaws-glue-awsglue) GetResourcePolicies(NextToken: Token, MaxResults: PageSize @range 1..1000) returns GetResourcePoliciesResponseList plus NextToken, and is @paginated (inputToken NextToken / outputToken NextToken / items GetResourcePoliciesResponseList / pageSize MaxResults). It is the enumerating counterpart of GetResourcePolicy: every resource policy attached in the account's catalog appears as one element of the list, each carrying the policy document (PolicyInJson), its PolicyHash and the CreateTime/UpdateTime of the attachment. Recorded behavior for the element: a catalog-level policy attached through PutResourcePolicy comes back as one list entry whose PolicyInJson is the document that was put (byte-for-byte as stored, including its formatting), whose PolicyHash is the hash the service computed for it, and whose CreateTime/UpdateTime are set. The number of entries and their contents follow from the account's state, not from any fixed value.

Emulator: the same call answers 200 but GetResourcePoliciesResponseList has no element 0 at all — the replay reports `$.GetResourcePoliciesResponseList[0]: missing`, i.e. the emulator serves an empty (or absent) list even though the account policy had been attached earlier in the same fixture. The status is right; the body reports no policies.

Why: GetResourcePolicies is how a client discovers what catalog-level sharing/authorization is in force without knowing the resource ARNs in advance — console-style policy inventories, drift detectors and "is anything shared out of this catalog?" audits all read it. Against the emulator they conclude the account has NO resource policy, so an audit passes a catalog that is in fact shared, a drift check re-attaches a policy that already exists, and any paginated walk over the list terminates immediately. Because the emulator answers 200, nothing signals that the data is missing rather than genuinely empty.

Fix: the GetResourcePolicies handler and the catalog resource-policy state it reads — the listing must enumerate the resource policies stored for the addressed catalog/account (the same records PutResourcePolicy writes and GetResourcePolicy reads back) and project each one into a GetResourcePoliciesResponseList element with PolicyInJson, PolicyHash and CreateTime/UpdateTime, paginating per the operation's @paginated contract.

Traces: glue-account-policy-paginated-listing#task#4 (the fixture attaches an account-level policy earlier in the run and then lists it).

No open issue carries this replay key; the open GetResourcePolicy-adjacent Glue issues concern other operations' read paths (e.g. 20261117T133709Z-glue-read-paths-omit-or-misreport-members-aws.md covers members omitted by other read operations, not this empty listing).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up). Net-new divergence cluster, service=glue, verdict=mismatch, stub_path empty, owned_by empty:

fixture glue-account-policy-paginated-listing, step task#4 — GetResourcePolicies, status 200 -> 200
  detail: $.GetResourcePoliciesResponseList[0]: missing
  expected element 0: {'CreateTime': <timestamp>, 'PolicyHash': <hash of the stored document>, 'PolicyInJson': <the policy document previously attached to the catalog, e.g. a Version/Statement document allowing glue:GetDatabases to the account root on the catalog ARN>, 'UpdateTime': <timestamp>}
  actual element 0:   None (list empty / absent)

The recorded PolicyHash, the account id inside PolicyInJson and the CreateTime/UpdateTime values are that run's values filling the shape; the requirement is that the account's attached policy appears as a list element with those members populated from state.

Model evidence (smithy_query, services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json):
  GetResourcePolicies input { NextToken: Token, MaxResults: PageSize @range min 1 max 1000 }
  output { GetResourcePoliciesResponseList: list, NextToken: Token }
  paginated { inputToken: NextToken, outputToken: NextToken, items: GetResourcePoliciesResponseList, pageSize: MaxResults }
  errors: GlueEncryptionException(400), InvalidInputException(400), OperationTimeoutException(400), InternalServiceException(500)

## Steps to Reproduce
1. Attach a Data Catalog resource policy for the account: glue:PutResourcePolicy with a PolicyInJson document whose Resource is the catalog ARN (arn:aws:glue:<region>:<account>:catalog).
2. Confirm it is stored: glue:GetResourcePolicy returns that document with a PolicyHash.
3. Call glue:GetResourcePolicies (no NextToken, default MaxResults).
4. AWS returns 200 with GetResourcePoliciesResponseList containing one element for the attached policy (PolicyInJson = the stored document, PolicyHash = its hash, CreateTime/UpdateTime set). The emulator returns 200 with an empty list — element 0 is missing.

Replay: fixture glue-account-policy-paginated-listing, step task#4 of trace set simparity-wire-aws-v1.
