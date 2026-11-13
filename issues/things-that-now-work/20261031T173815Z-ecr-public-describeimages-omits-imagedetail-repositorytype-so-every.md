Issue-Source: trace-sweep-divergence
Issue-Trace: api-repository-deletion-and-non-empty-error-paths
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-repository-deletion-and-non-empty-error-paths ecr-public-image-push-idempotent-replay-and-layer-reupload
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|DescribeImages|2xx->2xx|$.imageDetails[*]
Issue-Commit: 9737b41b9bb889e100c3bf9348295e20ff42401a

# ECR Public: DescribeImages omits ImageDetail.repositoryType, so every described image is missing its registry tier

> _Note: this issue was identified against the code base at commit `9737b41b9bb889e100c3bf9348295e20ff42401a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Wire replay of simparity-wire-aws-v1 (163/165 steps passed; ecr-public IS bootstrapped — service-com-amazonaws-ecrpublic-spencerfrontendservice is in the bootstrapped set and answered 200 here — so this is a fidelity divergence, not a bootstrap gap) surfaced ONE root cause, affecting two fixtures. This cause must be closed before this issue is resolved.

=== Cause 1 of 1: every ImageDetail returned by DescribeImages is missing the repositoryType member ===

AWS: recorded behavior only for this member. The checked-in Smithy model (services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json, shape com.amazonaws.ecrpublic#ImageDetail) declares only registryId, repositoryName, imageDigest, imageTags, imageSizeInBytes, imagePushedAt, imageManifestMediaType and artifactMediaType — it has no repositoryType, so the model is stale with respect to the service. The recording of DescribeImages (200, output DescribeImagesResponse.imageDetails -> ImageDetailList of ImageDetail, per smithy_query on DescribeImages) shows AWS populating one further member on EVERY ImageDetail entry: repositoryType, the tier of the public repository the image belongs to. For a repository created through ecr-public CreateRepository in the account's default public registry the recorded value is the enum-style string "FREE"; the value is a property of the repository the image was pushed to, not of the image or of the request, so it is the same for every image in a repository and is not a per-run value (unlike imagePushedAt, which the fixture does not assert). No status, error code or error message diverges: the call is 200 on both sides and every other member of the entry (registryId, repositoryName, imageDigest, imageTags, imageSizeInBytes, imagePushedAt, imageManifestMediaType, artifactMediaType) matches exactly.

Emulator: DescribeImages answers 200 with imageDetails[0] carrying the eight modeled members and NO repositoryType at all, so the replay reports the entry as simultaneously 'missing' the expected object and 'extra' with the shorter one. The member cannot be emitted today because the generated ImageDetail type has no such field: services/service-com-amazonaws-ecrpublic-spencerfrontendservice/src/ops/describe_images.rs builds its page from repository.image_detail(image) (crate::state) and serializes it through DESCRIBE_IMAGES_OUTPUT_SCHEMA, all of which are derived from the stale ImageDetail shape.

Why: repositoryType is how a client learns which registry tier an image sits in. A caller that reads imageDetails[*].repositoryType to render or branch on the tier (a publish/promotion pipeline, a console-style listing, a conformance check over the response shape) gets a KeyError/None against the emulator while AWS always supplies it, and a typed SDK model that marks the member present-on-read deserializes an incomplete object. Because the field is absent rather than wrong, nothing in the response signals the omission.

Fix: the ECR Public service's Smithy model must declare repositoryType on com.amazonaws.ecrpublic#ImageDetail (and, if it is an enum, its value shape with FREE), and the repository state's image_detail builder (crate::state, consumed by the DescribeImages handler) must populate it from the owning repository's tier so the DescribeImages serializer emits it on every entry. No other member and no error path changes.

Traces: api-repository-deletion-and-non-empty-error-paths#task#10 (representative), ecr-public-image-push-idempotent-replay-and-layer-reupload#task#10.

Already-filed ecr-public issues reviewed and NOT covering this: 20261023T055032Z-ecr-public-getauthorizationtoken-is-not-authorization-gated-stored.md (8 behavioral causes: auth gating, logo hosting, validation templates, layer-upload messages), 20261025T010839Z-ecr-public-putrepositorycatalogdata-accepts-an-under-sized-logoimageblob.md, 20261029T104628Z-ecr-public-putregistrycatalogdata-still-refuses-the-suite-s.md and 20261029T173115Z-ecr-public-the-pattern-constraint-invalidparameterexception-message-omits.md. None mentions DescribeImages or ImageDetail, and the replay marked this cluster neither stub-blocked nor owned.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, 12 fixtures replayed, 165 steps, 163 passed, 2 mismatched, 0 errored, 0 blocked, score 98.8%, emulator_up true (/__admin/health 200). divergence_count 1.

Cluster key service=ecr-public|DescribeImages|status=200->200|keys=$.imageDetails[0]; verdict mismatch; expected_status 200, actual_status 200; affected_count 2 (api-repository-deletion-and-non-empty-error-paths#task#10, ecr-public-image-push-idempotent-replay-and-layer-reupload#task#10).

Expected imageDetails[0] (recorded):
{'artifactMediaType': 'application/vnd.docker.container.image.v1+json', 'imageDigest': 'sha256:02b0e...20dd1', 'imageManifestMediaType': 'application/vnd.docker.distribution.manifest.v2+json', 'imagePushedAt': '<shaped-not-asserted>', 'imageSizeInBytes': 204, 'imageTags': ['tag-primary'], 'registryId': '000000000000', 'repositoryName': '<the repository created by the fixture>', 'repositoryType': 'FREE'}

Actual imageDetails[0] (emulator):
{'registryId': '000000000000', 'repositoryName': '<the same repository>', 'imageDigest': 'sha256:02b0e...20dd1', 'imageTags': ['tag-primary'], 'imageSizeInBytes': 204, 'imagePushedAt': <epoch seconds>, 'imageManifestMediaType': 'application/vnd.docker.distribution.manifest.v2+json', 'artifactMediaType': 'application/vnd.docker.container.image.v1+json'}

The ONLY difference is the absent repositoryType member; imagePushedAt is shaped-not-asserted and the digest/tag/size/media types match. Replay detail: "$.imageDetails[0]: missing (expected {... 'repositoryType': 'FREE'}, got {...})" plus the mirrored "extra" line for the same path.

Model evidence: smithy_query(query='shape', shape='ImageDetail', model_path='services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json') returns eight members, none named repositoryType. smithy_query(query='operation', operation='DescribeImages') shows output imageDetails -> ImageDetailList (paginated items), success 200.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture api-repository-deletion-and-non-empty-error-paths, step task#10 (and the identical cluster member ecr-public-image-push-idempotent-replay-and-layer-reupload, step task#10). Each fixture creates a public repository, pushes an image manifest with tag 'tag-primary' via PutImage, then calls ecr-public DescribeImages for that repository.
3. Inspect imageDetails[0] of the 200 response: it must carry repositoryType naming the tier of the owning public repository ('FREE' for a repository created through CreateRepository in the account's default public registry) alongside registryId, repositoryName, imageDigest, imageTags, imageSizeInBytes, imagePushedAt, imageManifestMediaType and artifactMediaType. The emulator returns the eight members and omits repositoryType entirely.
4. The fix is confirmed when both fixture steps above replay clean — i.e. DescribeImages emits repositoryType on every ImageDetail entry, for a repository with one image and for a repository with several, and no other member of the entry changes.
