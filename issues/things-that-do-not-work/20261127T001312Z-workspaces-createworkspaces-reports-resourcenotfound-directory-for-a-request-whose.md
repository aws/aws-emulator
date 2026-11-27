Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-workspaces-batch-start-stop-reboot
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-workspaces-batch-start-stop-reboot
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Commit: 730c79e9a78b8174020f0edbe823eb16fd629eef

# CreateWorkspaces reports ResourceNotFound.Directory for a request whose bundle does not exist

> _Note: this issue was identified against the code base at commit `730c79e9a78b8174020f0edbe823eb16fd629eef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaces

## Description
Cause 1 of 1: CreateWorkspaces mis-attributes the per-request failure reason in its FailedRequests entries, blaming the directory for a request whose DirectoryId is valid and whose BundleId is the thing that does not exist.

AWS: CreateWorkspaces is a batch operation that always answers HTTP 200 and reports per-request failures in FailedRequests, each a FailedCreateWorkspaceRequest carrying the offending WorkspaceRequest plus ErrorCode (ErrorType) and ErrorMessage (Description) (Smithy: CreateWorkspaces -> CreateWorkspacesResult.FailedRequests : FailedCreateWorkspaceRequests; FailedCreateWorkspaceRequest.ErrorCode/.ErrorMessage; WorkspaceRequest.DirectoryId required @pattern ^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$, WorkspaceRequest.BundleId required @pattern ^wsb-[0-9a-z]{8,63}$; modeled operation errors are only InvalidParameterValuesException and ResourceLimitExceededException, so an unresolvable directory or bundle is NOT a top-level fault). The model does not enumerate the ErrorType values, so the per-request codes are recorded behavior only: the recording shows that a request naming a bundle that does not resolve is failed with ErrorCode "ResourceNotFound.Bundle" and ErrorMessage "The specified bundle could not be found in the specified region.", while a request naming an unresolvable directory is failed with "ResourceNotFound.Directory" / "The specified directory could not be found in the specified region.". Both messages are entirely fixed text for their error class — they interpolate nothing from the request or from state, so the only requirement is that the code and the sentence match the resource that actually failed to resolve.

Emulator: for the batch element whose DirectoryId resolves but whose BundleId does not, the emulator still emits the directory verdict: FailedRequests[1].ErrorCode is "ResourceNotFound.Directory" instead of "ResourceNotFound.Bundle" and FailedRequests[1].ErrorMessage is "The specified directory could not be found in the specified region." instead of "The specified bundle could not be found in the specified region.". The HTTP status (200), the FailedRequests/PendingRequests partition and the element's position are all correct — only the failure reason is wrong, which indicates the per-request resolution check reports a single generic/first-checked cause rather than the resource it actually failed to look up.

Why: FailedRequests entries are the only machine-readable signal a batch client has for deciding what to do with each rejected WorkspaceRequest. A client that branches on ErrorCode — re-resolving bundles via DescribeWorkspaceBundles and retrying on ResourceNotFound.Bundle, but escalating or re-registering a directory on ResourceNotFound.Directory — takes the wrong remediation path, reports a healthy directory as missing, and never discovers that the bundle id is the bad input. Operators reading the surfaced ErrorMessage are pointed at the wrong resource entirely.

Fix: the CreateWorkspaces handler's per-request validation in the workspaces service — the step that resolves each WorkspaceRequest's DirectoryId and BundleId against state and builds its FailedCreateWorkspaceRequest. Each resolution failure must set the code/message pair for the resource that failed (bundle lookup failure -> ResourceNotFound.Bundle with the bundle sentence; directory lookup failure -> ResourceNotFound.Directory with the directory sentence) instead of reusing one cause for every unresolved reference.

Traces: workspaces-create-workspaces-batch-start-stop-reboot#task#3 ($.FailedRequests[1].ErrorCode, $.FailedRequests[1].ErrorMessage).

Note: a separate open issue, 20261108T072651Z-workspaces-describeworkspacebundles-nexttoken-is-an-internal-positional-offset-not.md, owns the DescribeWorkspaceBundles NextToken divergence seen in this same replay; it does not cover this CreateWorkspaces failure-reason defect.

## Evidence
Fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3, operation CreateWorkspaces. Expected status 200, actual status 200 (status correct).

Expected (recorded AWS response):
  $.FailedRequests[1].ErrorCode    = "ResourceNotFound.Bundle"
  $.FailedRequests[1].ErrorMessage = "The specified bundle could not be found in the specified region."

Actual (emulator):
  $.FailedRequests[1].ErrorCode    = "ResourceNotFound.Directory"
  $.FailedRequests[1].ErrorMessage = "The specified directory could not be found in the specified region."

Replay detail: "$.FailedRequests[1].ErrorCode: value_mismatch (expected 'ResourceNotFound.Bundle', got 'ResourceNotFound.Directory')" / "$.FailedRequests[1].ErrorMessage: value_mismatch (expected 'The specified bundle could not be found in the specified region.', got 'The specified directory could not be found in the specified region.')". Both recorded strings are fixed text for their error class with no request- or state-derived substitutions. Cluster divergence_key: service=workspaces|CreateWorkspaces|status=200->200|keys=$.FailedRequests[1].ErrorCode,$.FailedRequests[1].ErrorMessage (affected_count 1).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
2. Fixture workspaces-create-workspaces-batch-start-stop-reboot reaches step task#3, a CreateWorkspaces call whose Workspaces list mixes creatable requests with ones that cannot be satisfied: at least one element names an existing DirectoryId together with a BundleId (matching ^wsb-[0-9a-z]{8,63}$) that is not present in the emulator's bundle catalog.
3. Read the 200 response's FailedRequests list and inspect the entry for the element whose directory resolves but whose bundle does not.
4. Observe ErrorCode "ResourceNotFound.Directory" and ErrorMessage "The specified directory could not be found in the specified region." where AWS returns ErrorCode "ResourceNotFound.Bundle" and ErrorMessage "The specified bundle could not be found in the specified region.".
