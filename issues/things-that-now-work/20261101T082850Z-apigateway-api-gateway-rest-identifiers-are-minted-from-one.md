Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-deploy-and-export-api-definition
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-deploy-and-export-api-definition apigateway-import-rest-api-deploy-export-sdk apigateway-rest-api-import-export-put apigateway-rest-api-update-list-and-sdk-generation apigateway-documentation-part-conflict-duplicate-location apigateway-documentation-parts-multiple-locations-and-listing apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation apigateway-tag-usageplan-bulk-tags-and-pagination
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateDeployment|2xx->2xx|$.id
Issue-Key: apigateway|CreateDocumentationPart|2xx->2xx|$.id
Issue-Key: apigateway|ImportApiKeys|2xx->2xx|$.ids[*]
Issue-Key: apigateway|CreateUsagePlan|2xx->2xx|$.id
Issue-Commit: 2294bca91bc8bfde7308721c1d7853ded4942c2e

# API Gateway REST identifiers are minted from one shared process counter, so ids are predictable and uncorrelated (4 causes)

> _Note: this issue was identified against the code base at commit `2294bca91bc8bfde7308721c1d7853ded4942c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Four net-new wire divergences on the API Gateway REST control plane (service token `apigateway`) share one root cause: every server-minted identifier this service returns — a deployment id, a documentation-part id, a usage-plan id, an imported API-key id — comes out of ONE process-wide counter, so the identifier a resource gets is a function of how many identifiers the process minted before it rather than of the resource, and the identifier the recording correlates with a resource is not the one the emulator reports for it.

Shared shape of the defect — AWS: the Smithy model types all of these members as plain `String` with no @pattern or @length (`Deployment.id`, `DocumentationPart.id`, `UsagePlan.id`, `ApiKeyIds.ids`), so the format is recorded behavior only: API Gateway mints a 10-character lowercase-alphanumeric token drawn independently per resource, and the id a create returns is the id that resource carries in every later read, listing and cross-reference. Emulator: `ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id` (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/state.rs) increments a single shared `id_seed` and scrambles it through a fixed bijection, so (a) the value is fully determined by the counter's position — every id the corpus observes is a base-36 rendering of a small counter and therefore ends in the constant literal `aaa` (`5mw7f4baaa`, `6ofuwnbaaa`, `8sh3tqaaaa`, `7qygd7aaaa`), which a random 10-character base-36 token does with probability 1/46656, so no run of real AWS emits it on every id; (b) the one counter is shared by every resource kind, so the same value is handed out for resources of different kinds across the corpus (`6ofuwnbaaa` for both a documentation part and an imported API key, `8sh3tqaaaa` for both a usage plan and the API-keys pagination cursor); and (c) the id a resource receives shifts with unrelated creations earlier in the process, which is why the id asserted for a resource by the recording is not the id the emulator reports for it in each cause below. Why: a client that treats an API Gateway id as an opaque, unpredictable, per-resource-unique handle is wrong on all three counts here — ids are guessable from the creation count, two unrelated resources can collide in a cache keyed by id, and an id captured from a create does not reliably identify the same resource later. Fix: the shared identifier helper `mint_id` in the service crate's state module (and the create/import handlers in src/ops/ that call it), which must mint an independent 10-character base-36 token per resource and store it so the resource keeps it. The concrete characters of any one id are a per-run value and are NOT the requirement; the requirement is the family the id is drawn from and that the id stays bound to its resource.

Cause 1 of 4 — CreateDeployment reports a deployment id that is not the one the fixture correlates with the deployment it just created. AWS: 201 with `$.id` the newly created deployment's id, which the recording then correlates with the same deployment in later steps (the fixture asserts the member against its captured deployment id). Emulator: 201 with `$.id` = `5mw7f4baaa` — a counter-derived token with the constant `aaa` tail, which does not satisfy the correlation the recording asserts. The same divergence appears on three further fixtures that create a deployment (import/export/SDK-generation flows), so it is the create path and not one fixture. Trace: apigateway-deploy-and-export-api-definition#task#2 (also apigateway-import-rest-api-deploy-export-sdk#task#2, apigateway-rest-api-import-export-put#setup#3, apigateway-rest-api-update-list-and-sdk-generation#task#6).

Cause 2 of 4 — CreateDocumentationPart reports a part id that is not the one the fixture correlates with that part. AWS: 201 with `$.id` the new documentation part's id, correlated by the recording with the part in the later conflict/listing steps. Emulator: 201 with `$.id` = `6ofuwnbaaa`, the same counter-derived value this run also hands to an imported API key in another fixture. Trace: apigateway-documentation-part-conflict-duplicate-location#task#2 (also apigateway-documentation-parts-multiple-locations-and-listing#task#5, #task#6, #task#7).

Cause 3 of 4 — ImportApiKeys reports an id for the first imported key that is not the id the recording correlates with that key. AWS: 201 with `$.ids` listing the ids of the keys the CSV import created, each being the id under which the key is afterwards addressable by GetApiKey/DeleteApiKey. Emulator: 201 with `$.ids[0]` = `6ofuwnbaaa` where the fixture's correlated id for that key is `8sh3tqaaaa` — the import mints the next counter value instead of reporting the id the key is held under. Why: a client that imports keys and then addresses them by the returned ids operates on a different key, or on none. Trace: apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation#task#7.

Cause 4 of 4 — CreateUsagePlan reports a usage-plan id that is not the one the fixture correlates with the plan. AWS: 201 with `$.id` the new usage plan's id, which the fixture's later tag and pagination steps address the plan by. Emulator: 201 with `$.id` = `8sh3tqaaaa`, again the next shared-counter value (the same value this run also emits as a GetApiKeys pagination cursor). Trace: apigateway-tag-usageplan-bulk-tags-and-pagination#setup#1.

No open issue carries the replay keys of these four clusters and no unimplemented stub blocks them. The open issue `20261029T223422Z-apigateway-generated-resource-identifiers-diverge-from-the-recorded.md` reports the same `aaa`-tailed counter family on the API Gateway **v2** surface (GetRoutes/CreateIntegration/CreateModel, shape id `com.amazonaws.apigatewayv2#ApiGatewayV2`) and owns none of the REST steps above; the REST control plane has its own `mint_id` in its own service crate, so fixing that issue does not fix these.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 193 fixtures, 1873 steps, 1591 passed, score 84.9%, 100 divergence clusters (every one service=apigateway). The four clusters below have an empty `owned_by` and an empty `stub_path`.

1. CreateDeployment — apigateway-deploy-and-export-api-definition#task#2, expected 201 / got 201: `$.id: value_mismatch (expected '{{deployment_id}}', got '5mw7f4baaa')`. affected_count 4 (apigateway-import-rest-api-deploy-export-sdk#task#2, apigateway-rest-api-import-export-put#setup#3, apigateway-rest-api-update-list-and-sdk-generation#task#6).
2. CreateDocumentationPart — apigateway-documentation-part-conflict-duplicate-location#task#2, 201/201: `$.id: value_mismatch (expected '{{first_part_id}}', got '6ofuwnbaaa')`. affected_count 4 (apigateway-documentation-parts-multiple-locations-and-listing#task#5, #task#6, #task#7).
3. ImportApiKeys — apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation#task#7, 201/201: `$.ids[0]: value_mismatch (expected '8sh3tqaaaa', got '6ofuwnbaaa')`. affected_count 1.
4. CreateUsagePlan — apigateway-tag-usageplan-bulk-tags-and-pagination#setup#1, 201/201: `$.id: value_mismatch (expected '{{usage_plan_id}}', got '8sh3tqaaaa')`. affected_count 1.

Every one answers the recorded status (201); only the identifier member diverges.

Emulator code read: src/state.rs — `pub id_seed: u64` ("Drives ... mint_id") and `mint_id`, which does `self.id_seed = self.id_seed.wrapping_add(1)` and renders `(id_seed * ID_MULTIPLIER + ID_OFFSET) % ID_SPACE` as 10 base-36 digits, i.e. one counter shared by every resource kind of this service. The constant `aaa` tail observed on every minted id in this corpus (`5mw7f4baaa`, `6ofuwnbaaa`, `8sh3tqaaaa`, `7qygd7aaaa`) is the signature of that small-counter rendering; the same minted value also appears for resources of different kinds across the corpus (`6ofuwnbaaa`: documentation part and imported API key; `8sh3tqaaaa`: usage plan and GetApiKeys `position`).

Smithy consulted (model services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json): `ImportApiKeys` (POST /apikeys?mode=import, code 201, output `ApiKeyIds{ids -> ListOfString, warnings}`); the id members of `Deployment`, `DocumentationPart` and `UsagePlan` target plain `String` with no @pattern/@length, so the 10-character lowercase-alphanumeric format is recorded behavior only.

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay apigateway-deploy-and-export-api-definition and read `$.id` of step task#2 (CreateDeployment) — a counter-derived token ending in `aaa`, not the deployment id the fixture correlates with the deployment.
3. Cause 2: replay apigateway-documentation-part-conflict-duplicate-location step task#2 (CreateDocumentationPart) and apigateway-documentation-parts-multiple-locations-and-listing steps task#5-7 and read `$.id`.
4. Cause 3: replay apigateway-import-api-keys-csv-bulk-and-failonwarnings-validation step task#7 (ImportApiKeys) and read `$.ids[0]` — it is the next shared-counter value rather than the id the imported key is held under.
5. Cause 4: replay apigateway-tag-usageplan-bulk-tags-and-pagination step setup#1 (CreateUsagePlan) and read `$.id`.
6. Cross-check the generator: every id the run emits ends in the literal `aaa`, and the same value recurs for resources of different kinds (a documentation part and an imported API key both get `6ofuwnbaaa`; a usage plan and a GetApiKeys `position` both get `8sh3tqaaaa`).
