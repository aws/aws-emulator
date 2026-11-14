Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-ecracc-repository-policy-principal-forms
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-ecracc-repository-policy-principal-forms
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|SetRepositoryPolicy|4xx->2xx|$.__type
Issue-Key: ecr|SetRepositoryPolicy|4xx->2xx|$.message
Issue-Key: ecr|SetRepositoryPolicy|4xx->2xx|status
Issue-Key: ecr|PutRegistryPolicy|4xx->2xx|$.__type
Issue-Key: ecr|PutRegistryPolicy|4xx->2xx|$.message
Issue-Key: ecr|PutRegistryPolicy|4xx->2xx|status
Issue-Key: ecr|SetRepositoryPolicy|2xx->2xx|$.policyText
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ECR repository and registry policy documents are neither validated nor normalized

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRepositoryPolicy

## Description
3 causes, one shared fault: the policy document handed to ECR's policy writers is stored as given — never checked for validity and never canonicalised — so documents AWS refuses are accepted, and a document AWS rewrites is read back unrewritten. Related to but distinct from the open issue 'PutBucketPolicy does not store the policy document verbatim' (id 20261103T021518Z-...-putbucketpolicy-does-not-store-the-policy-document-verbatim.md), which is about S3 rewriting a document it should keep; here ECR keeps a document it should validate and canonicalise.

Cause 1 of 3: SetRepositoryPolicy accepts an invalid repository policy.
AWS: Smithy: SetRepositoryPolicy.policyText -> RepositoryPolicyText (required, @length 0..10240), error InvalidParameterException (httpError 400). A document whose principal form (or other element) is not valid for a repository policy is refused with HTTP 400, error code InvalidParameterException, and message, fixed text exactly: "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid repository policy provided'". Both the member path ('PolicyText', capitalised as AWS emits it here) and the constraint clause are fixed text; there are no variable parts.
Emulator: answers HTTP 200 and stores the policy — no __type, no message.
Why: the repository ends up carrying a policy AWS would never hold, so later authorization decisions are made against a document that cannot exist in AWS, and a caller testing that bad policies are rejected gets a false pass.
Fix: SetRepositoryPolicy's policyText validation, ahead of the state write.
Traces: ecr-ecracc-repository-policy-principal-forms#task#6.

Cause 2 of 3: PutRegistryPolicy accepts an invalid registry policy.
AWS: Smithy: PutRegistryPolicy.policyText -> RegistryPolicyText (required, @length 0..10240), errors InvalidParameterException (400) and ValidationException (400). An invalid registry policy is refused with HTTP 400, error code InvalidParameterException, message fixed text exactly: "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid registry policy provided'" — same template as the repository case but with the registry wording.
Emulator: answers HTTP 200 and stores the policy — no __type, no message.
Why: same as cause 1 at registry scope — the registry carries an unenforceable policy and the negative test passes.
Fix: PutRegistryPolicy's policyText validation, ahead of the state write.
Traces: ecr-ecracc-repository-policy-principal-forms#task#7.

Cause 3 of 3: a bare-account-id Principal is not canonicalised to the account root ARN.
AWS: Smithy: SetRepositoryPolicy returns policyText (RepositoryPolicyText), and GetRepositoryPolicy returns the same stored document. Recorded behavior only for the canonicalisation rule: when a statement's Principal.AWS is given as a bare 12-digit account id, AWS stores and returns it expanded to the account root ARN form arn:aws:iam::<accountId>:root; the account id is the variable part, taken from the submitted document, and the rest of the document (Version, Sid, Effect, Action order, pretty-printed layout) is returned unchanged.
Emulator: returns the Principal.AWS exactly as submitted — the bare account id — so the response (and the stored document) is not the canonical form AWS would hold.
Why: a client that compares the policy it reads back against the canonical form AWS produces (drift detection, CloudFormation-style convergence, idempotent re-put) sees a permanent difference and rewrites the policy forever; code that parses Principal.AWS expecting an ARN mis-handles a bare id.
Fix: the policy-document normalisation applied by SetRepositoryPolicy before storing (and reflected by GetRepositoryPolicy), i.e. the policy state/serializer, not the handler's response assembly.
Traces: ecr-ecracc-repository-policy-principal-forms#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture ecr-ecracc-repository-policy-principal-forms.

Cause 1 — task#6 SetRepositoryPolicy: status expected 400, got 200
  $.__type expected 'InvalidParameterException', got None
  $.message expected "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid repository policy provided'", got None

Cause 2 — task#7 PutRegistryPolicy: status expected 400, got 200
  $.__type expected 'InvalidParameterException', got None
  $.message expected "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid registry policy provided'", got None

Cause 3 — task#8 SetRepositoryPolicy, 200 -> 200, $.policyText value_mismatch — identical documents except the principal:
  expected ... "Principal" : { "AWS" : "arn:aws:iam::000000000000:root" } ...
  actual   ... "Principal" : { "AWS" : "000000000000" } ...
  (the account id is the value from the submitted document; the expansion to the arn:aws:iam::<accountId>:root form is the requirement.)

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1), fixture ecr-ecracc-repository-policy-principal-forms.
2. Cause 1: SetRepositoryPolicy(repositoryName=<repo>, policyText=<a repository policy with an invalid principal form>); observe 200 where AWS answers 400 InvalidParameterException with "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid repository policy provided'".
3. Cause 2: PutRegistryPolicy(policyText=<an invalid registry policy>); observe 200 where AWS answers 400 InvalidParameterException with "... 'Invalid registry policy provided'".
4. Cause 3: SetRepositoryPolicy with a statement whose Principal.AWS is a bare 12-digit account id, then read $.policyText from the response (and from GetRepositoryPolicy); the principal is still the bare id instead of arn:aws:iam::<accountId>:root.
