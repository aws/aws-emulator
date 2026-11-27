Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-public-upload-layer-part-invalid-parameters
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-public-upload-layer-part-invalid-parameters ecr-public-upload-layer-part-region-gate ecr-public-upload-layer-part-unregistered-registry-alias
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|UploadLayerPart|4xx->4xx|$.message
Issue-Key: ecr-public|UploadLayerPart|4xx->4xx|$.__type
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ECR Public UploadLayerPart error bodies diverge: unenforced region/registry gate and non-AWS @pattern message

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadLayerPart

## Description
Wire replay of simparity-wire-aws-v1 against this checkout: 16 fixtures / 180 steps, 173 passed, 7 mismatched (score 96.1%), emulator_up=true, bootstrap_signal=false. ecr-public IS bootstrapped (service-com-amazonaws-ecrpublic-spencerfrontendservice is in the bootstrapped set), so these are fidelity defects, not a bootstrap gap. All 7 mismatched steps belong to ecr-public UploadLayerPart and cluster into the TWO root causes below; no cluster carried a stub_path or an owned_by, so both are net-new. This issue bundles 2 causes and a fixer must close BOTH before submitting.

=== Cause 1 of 2: the modelled-@pattern InvalidParameterException message uses a non-AWS template ===

AWS: Smithy UploadLayerPart input member uploadId -> com.amazonaws.ecrpublic#UploadId, required, @pattern "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"; the operation models InvalidParameterException (client, httpError 400, doc "The specified parameter is invalid. Review the available parameters for the API request."). A uploadId that violates that pattern is refused by input validation before any upload or repository lookup, with HTTP 400, __type InvalidParameterException and the fixed AWS template naming the member path and the violated rule:
  "Invalid parameter at '<member>' failed to satisfy constraint: 'Member must satisfy regular expression pattern: <pattern>'"
FIXED text: "Invalid parameter at '", "' failed to satisfy constraint: 'Member must satisfy regular expression pattern: ", and the closing "'". VARIABLE parts: <member>, the offending input member's path taken from the request, and <pattern>, that member's modelled @pattern rendered WITHOUT the ^/$ anchors and WITHOUT surrounding quotes. For uploadId that instantiates as "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'". The template is the requirement; whatever malformed uploadId one recording happened to send is not.

Emulator: HTTP status 400 and __type InvalidParameterException are both correct; only $.message diverges. It emits "Invalid parameter at 'uploadId' failed to satisfy constraint: 'must satisfy regular expression '<pattern>''" — three divergences in the FIXED text: the rule clause drops the leading "Member " qualifier, it says "regular expression" where AWS says "regular expression pattern:", and it wraps the pattern in single quotes (yielding a doubled trailing quote) where AWS emits the bare pattern after a colon. The member path and the unanchored pattern body are themselves correct.

Why: this message is the only place the service says which member and which rule failed. A client that must distinguish a malformed uploadId (re-mint a UUID in the caller) from a well-formed but unissued one (restart the upload session) parses this body; SDK wrappers, CLIs and conformance suites that match AWS's "Member must satisfy regular expression pattern:" clause extract nothing, and the doubled quote makes the pattern unparseable for a consumer that unquotes the constraint clause.

Fix: the shared modelled-@pattern -> InvalidParameterException rendering in the ecr-public input-validation layer that sits in front of the operation handlers (the same layer that renders @length violations) must emit AWS's clause verbatim. Note the already-open BEHAVIORAL issue 20261029T173115Z-ecr-public-the-pattern-constraint-invalidparameterexception-message-omits.md reports the identical template defect on CompleteLayerUpload; it does NOT carry this wire cluster's replay key, so this cause is filed separately and both must be re-replayed after the validation layer is corrected.

Traces: ecr-public-upload-layer-part-invalid-parameters#task#2 (representative); same cluster: ecr-public-upload-layer-part-invalid-parameters#task#2, ecr-public-upload-layer-part-region-gate#task#3, ecr-public-upload-layer-part-unregistered-registry-alias#task#3.

=== Cause 2 of 2: the region / registry gate is not enforced — the handler falls through to a repository lookup and answers RepositoryNotFoundException ===

AWS: Smithy UploadLayerPart models UnsupportedCommandException (client, httpError 400, doc "The action isn't supported in this Region.") and RegistryNotFoundException (client, 400, doc "The registry doesn't exist.") alongside RepositoryNotFoundException. ECR Public is a us-east-1-only API surface, so an UploadLayerPart call made outside us-east-1 is refused BEFORE any registry or repository state is consulted, with HTTP 400, __type UnsupportedCommandException and the message recorded as "UploadLayerPart command is only supported in us-east-1." — FIXED text " command is only supported in us-east-1." with the VARIABLE leading token being the invoked operation's name (so GetAuthorizationToken, InitiateLayerUpload, etc. each name themselves). The same precedence applies to the registryId/registry-alias argument (input member registryId -> RegistryIdOrAlias, @length 2..50): an alias that resolves to no registry is a registry-level refusal, not a repository-level one. In both recordings the gate fires regardless of whether the named repository exists.

Emulator: returns HTTP 400 (correct status) but the WRONG error class and message — __type RepositoryNotFoundException with "The repository with name '<repositoryName>' does not exist in the registry with id '<registryId>'". The gate is simply absent: the handler goes straight to its repository lookup in state, so the first thing that fails is the repository existence check and the region/registry condition is never evaluated. (Hypothesis: the ecr-public handler chain has no region/alias precondition step at all, which is why the same substitution appears for both the out-of-region call and the unregistered-alias call.)

Why: a client gets a retriable-looking "create the repository first" signal where AWS gives a terminal "you are calling the wrong Region / the registry alias is not yours" signal. Tooling that catches UnsupportedCommandException to redirect its endpoint to us-east-1, or RegistryNotFoundException to re-resolve a registry alias, instead sees RepositoryNotFoundException and loops creating or re-pushing repositories that will never make the call succeed; the recorded response also leaks a repository-existence answer for a request AWS refuses before touching that state.

Fix: the UploadLayerPart handler (and the shared ecr-public request precondition layer it should share with the other layer-upload operations) must evaluate the region/registry-alias gate before any repository lookup and raise UnsupportedCommandException / RegistryNotFoundException with AWS's messages, rather than letting the repository-existence branch answer first.

Traces: ecr-public-upload-layer-part-region-gate#task#2 (representative); same cluster: ecr-public-upload-layer-part-region-gate#task#2, ecr-public-upload-layer-part-region-gate#task#4, ecr-public-upload-layer-part-unregistered-registry-alias#task#2, ecr-public-upload-layer-part-unregistered-registry-alias#task#4.

## Evidence
Replay summary (simparity-wire-aws-v1): {"kind":"summary","emulator_up":true,"bootstrap_signal":false,"fixtures_replayed":16,"steps_total":180,"steps_passed":173,"steps_mismatched":7,"steps_errored":0,"steps_blocked":0,"not_implemented":0,"score_pct":96.1,"divergence_count":2}. emulator_detail: "/__admin/health answered 200 and the :41559 data listener accepts".

Cause 1 — cluster divergence_key "service=ecr-public|UploadLayerPart|status=400->400|keys=$.message", verdict mismatch, expected_status 400, actual_status 400, stub_path "", owned_by []. Representative ecr-public-upload-layer-part-invalid-parameters#task#2; affected_count 3 (ecr-public-upload-layer-part-invalid-parameters#task#2, ecr-public-upload-layer-part-region-gate#task#3, ecr-public-upload-layer-part-unregistered-registry-alias#task#3). detail:
  $.message: value_mismatch
  expected: "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'"
  actual:   "Invalid parameter at 'uploadId' failed to satisfy constraint: 'must satisfy regular expression '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}''"
$.__type agrees (InvalidParameterException) and the status agrees, so only the message template is wrong.

Cause 2 — cluster divergence_key "service=ecr-public|UploadLayerPart|status=400->400|keys=$.__type,$.message", verdict mismatch, expected_status 400, actual_status 400, stub_path "", owned_by []. Representative ecr-public-upload-layer-part-region-gate#task#2; affected_count 4 (ecr-public-upload-layer-part-region-gate#task#2, ecr-public-upload-layer-part-region-gate#task#4, ecr-public-upload-layer-part-unregistered-registry-alias#task#2, ecr-public-upload-layer-part-unregistered-registry-alias#task#4). detail:
  $.__type: value_mismatch (expected 'UnsupportedCommandException', got 'RepositoryNotFoundException')
  $.message: value_mismatch (expected 'UploadLayerPart command is only supported in us-east-1.', got "The repository with name 'sim2real-probe-ecr-public-region-1bb79a16' does not exist in the registry with id '000000000000'")
The repository name and registry id in the observed message are per-run values from that recording, not part of any requirement; the requirement is the error class and the templated message above.

Model corroboration (smithy_query on services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json): UploadLayerPart input = registryId (RegistryIdOrAlias, @length 2..50), repositoryName (required, @length 2..205, @pattern ^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$), uploadId (required, @pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$), partFirstByte/partLastByte (required, long, min 0), layerPartBlob (required, blob, 0..20971520). Modelled errors include InvalidParameterException (client 400), RegistryNotFoundException (client 400, "The registry doesn't exist."), RepositoryNotFoundException (client 400), UnsupportedCommandException (client 400, "The action isn't supported in this Region."), UploadNotFoundException, InvalidLayerPartException, LimitExceededException, ServerException.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus). 7 of 180 steps mismatch, all on ecr-public UploadLayerPart.
2. Cause 1 — replay fixture ecr-public-upload-layer-part-invalid-parameters, step task#2 (also exercised by ecr-public-upload-layer-part-region-gate#task#3 and ecr-public-upload-layer-part-unregistered-registry-alias#task#3): call UploadLayerPart with a uploadId that violates the modelled @pattern on UploadId (any non-UUID string) plus otherwise valid repositoryName/partFirstByte/partLastByte/layerPartBlob. Expect HTTP 400, __type InvalidParameterException, message exactly "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'". The emulator drops the "Member " qualifier, writes "regular expression" instead of "regular expression pattern:", and single-quotes the pattern.
3. Cause 1 generalisation: send any other ecr-public request that violates a modelled @pattern (e.g. CreateRepository with a repositoryName breaking its pattern) and confirm the SAME template is emitted with that member's path and its unanchored, unquoted pattern, with no request value interpolated into the rule clause.
4. Cause 2 — replay fixture ecr-public-upload-layer-part-region-gate, step task#2 (and task#4): issue UploadLayerPart against the service from a non-us-east-1 Region, with a well-formed uploadId and a repositoryName that does NOT exist. Expect HTTP 400, __type UnsupportedCommandException, message "UploadLayerPart command is only supported in us-east-1." — the gate must fire before the repository lookup. The emulator instead returns __type RepositoryNotFoundException with "The repository with name '<name>' does not exist in the registry with id '<registryId>'".
5. Cause 2, second leg — replay fixture ecr-public-upload-layer-part-unregistered-registry-alias, steps task#2 and task#4: call UploadLayerPart with a registryId that is a registry alias no registry owns. The refusal must be the registry/region-level one modelled for this operation (UnsupportedCommandException / RegistryNotFoundException), not RepositoryNotFoundException, and must not reveal whether the repository exists.
6. Regression guard: a legitimate InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload sequence in the supported Region against an existing repository must keep returning 200 with registryId/repositoryName/uploadId/lastByteReceived echoed, so neither the message change nor the new precondition may start refusing valid uploads.
7. Re-replay both fixture steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids and confirm all 7 previously mismatched steps pass.
