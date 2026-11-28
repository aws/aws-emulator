Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.describe-addon-versions-pagination-and-default-uniqueness
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonVersions|InvalidParameterException
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# eks DescribeAddonVersions refuses an out-of-range maxResults with 400 InvalidParameterException instead of accepting it and serving the whole catalogue

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
ONE net-new root cause in the eks service, grounded in a failing behavioral check that no open issue carries the replay key for.

Cause 1 of 1: DescribeAddonVersions enforces the modeled maxResults @range server-side and refuses a value above it, while AWS treats maxResults on this operation as advisory and accepts it.
AWS:      Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): DescribeAddonVersions is GET /addons/supported-versions, success 200, input member maxResults targets DescribeAddonVersionsRequestMaxResults with @range {min: 1, max: 100} and is @httpQuery "maxResults"; the operation is @paginated (inputToken/outputToken nextToken, items addons, pageSize maxResults); modeled errors InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500). Recorded behavior: real AWS does NOT paginate this read and does not enforce that range on the wire — a maxResults ABOVE the modeled maximum is accepted with HTTP 200 and the complete add-on catalogue selected by the request's filters is returned, with no nextToken offered; the modeled range is a client-side constraint only. The same recording shows the smallest legal page size (maxResults=1) also returning the complete catalogue with no nextToken.
Emulator: the call is refused with InvalidParameterException (HTTP 400) and the message "maxResults needs to be between 1 and 100". The value that is refused is the out-of-range maxResults the check sends after its in-range calls (it sends maxResults=1 first, which is inside the emulator's own stated bound, then an out-of-range value); the emulator applies the @range as a server-side request validation. Fixed text of the emulator's message: "maxResults needs to be between 1 and 100"; its variable parts would be the bounds. AWS returns no error at all here, so the requirement is a 200 with the filtered catalogue, not a differently-worded refusal.
Why:      a client that passes a page size larger than 100 (or any SDK/tool that forwards a user-supplied page size unchanged to this non-paginating read) gets a hard 400 from the emulator where AWS answers 200 with the full catalogue. Add-on discovery — which add-ons exist and which versions may be installed on a cluster version — fails outright instead of returning data, so CreateAddon/UpdateAddon callers cannot resolve a version.
Fix:      the DescribeAddonVersions handler's request validation in the eks service crate: the maxResults @range must not be enforced as a server-side refusal for this operation; the value is advisory and the handler must serve the complete filtered catalogue (and keep nextToken absent).
Note:     the open issues 20261121T151648Z-eks-describeaddonversions-catalog-record-selection-and-registercluster-invalid.md and 20261127T102354Z-eks-read-only-catalog-reads-regress-add-on.md concern WHICH catalogue rows DescribeAddonVersions serves in a 200; neither covers this 400 refusal of a request AWS accepts, and neither carries this check's replay key.
Traces:   eks.describe-addon-versions-pagination-and-default-uniqueness.

## Evidence
Check eks.describe-addon-versions-pagination-and-default-uniqueness, verdict [error]. Replay output (tail): botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the DescribeAddonVersions operation: maxResults needs to be between 1 and 100.

Expected (AWS, per the recording the check encodes): HTTP 200 with the same addonName -> addonVersions count map as the call made with no maxResults at all, and no nextToken ("maxResults above the modelled max is ACCEPTED (no @range enforcement server-side) and serves the same set -- an emulator that raises here diverges").
Actual (emulator): HTTP 400 InvalidParameterException, message "maxResults needs to be between 1 and 100". No response body with addons is produced, so the check's later assertions (nextToken absent, maxResults=1 does not truncate, an unrecognised nextToken is ignored, exactly one defaultVersion per (addonName, clusterVersion)) are never reached.

Smithy consulted: DescribeAddonVersions — http GET /addons/supported-versions code 200; maxResults -> DescribeAddonVersionsRequestMaxResults @range{min 1, max 100}, httpQuery maxResults; paginated{inputToken nextToken, outputToken nextToken, items addons, pageSize maxResults}; errors InvalidParameterException 400 / ResourceNotFoundException 404 / ServerException 500.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Call DescribeAddonVersions with owners=["aws"] and no maxResults; note the addons returned and that no nextToken is offered.
3. Call DescribeAddonVersions with the same filters plus a maxResults ABOVE the modeled maximum (e.g. maxResults=101, as GET /addons/supported-versions?owners=aws&kubernetesVersion=<latest advertised>&maxResults=101).
   Expected (AWS): 200 with the identical add-on -> version count map as step 2 and no nextToken.
   Actual: 400 InvalidParameterException "maxResults needs to be between 1 and 100".
4. Replay check eks.describe-addon-versions-pagination-and-default-uniqueness to confirm.
