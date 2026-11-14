Issue-Source: trace-sweep-divergence
Issue-Trace: eks-efsfips-traj-marketplace-fips-addon-detour
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-efsfips-traj-marketplace-fips-addon-detour
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeAddonVersions|2xx->2xx|$.addons[*].marketplaceInformation.productId
Issue-Related: 20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md
Issue-Related: 20261109T131212Z-eks-describeaddonversions-catalog-omits-further-marketplace-add-ons.md
Issue-Commit: 87f8763cddaacde10d4e73c0f6e88e76a96af320

# EKS DescribeAddonVersions synthesizes the Marketplace FIPS add-on entry instead of serving AWS catalog data

> _Note: this issue was identified against the code base at commit `87f8763cddaacde10d4e73c0f6e88e76a96af320`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddonVersions

## Description
ONE net-new root cause in the EKS add-on catalog projected by DescribeAddonVersions. It must be closed before this issue is resubmitted.

Cause 1 of 1: a name-filtered DescribeAddonVersions read for a Marketplace-owned, FIPS-capable storage add-on (the recording's example is the Chainguard-published FIPS EFS CSI driver listing) answers 200 with an AddonInfo whose publisher, marketplaceInformation.productId, marketplaceInformation.productUrl and whole newest addonVersions[0] record are emulator-synthesized rather than the catalog data AWS publishes. All four diverging keys come from the same cluster and the same root: the entry is generated from the add-on name instead of seeded from AWS's add-on catalog.

AWS: Smithy: DescribeAddonVersions GET /addons/supported-versions -> DescribeAddonVersionsResponse.addons (list of com.amazonaws.eks#AddonInfo), filterable by the httpQuery members addonName/types/publishers/owners/kubernetesVersion; errors InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500). AddonInfo members: addonName, type, addonVersions, publisher ("The publisher of the add-on"), owner, marketplaceInformation, defaultNamespace. MarketplaceInformation members: productId ("The product ID from the Amazon Web Services Marketplace") and productUrl ("The product URL from the Amazon Web Services Marketplace"). AddonVersionInfo members: addonVersion, architecture, computeTypes ("Indicates the compute type of the add-on version"), compatibilities (one Compatibility per supported Kubernetes minor, each with clusterVersion, platformVersions and defaultVersion, @default false), requiresConfiguration and requiresIamPermissions (both @default false). The rule: for a Marketplace-owned add-on, publisher is the seller's own published name (with the seller's capitalization), marketplaceInformation.productId is that listing's Marketplace product identifier and productUrl is that listing's Marketplace product URL, and each AddonVersionInfo carries the version's real architectures, its computeTypes, and one compatibilities entry per Kubernetes minor AWS actually supports for that version with defaultVersion set on the minor's default. Recorded behavior for this listing: publisher is the capitalized seller name 'Chainguard'; productId is a Marketplace product identifier of the 'prod-<id>' form ('prod-butapz3xfcrww'); productUrl is the listing URL of the '/marketplace/pp?sku=<sku>' form; and the newest version record ('v4.0.<build>-eksbuild.1') carries architecture ['amd64','arm64'], computeTypes ['ec2'] and six compatibilities entries (minors 1.35 down to 1.30, each defaultVersion false) with requiresIamPermissions true.

Emulator: answers 200 with the correct envelope but a fabricated entry. $.addons[0].publisher is the lower-cased add-on-name-derived token ('chainguard') instead of the seller's published name ('Chainguard'). $.addons[0].marketplaceInformation.productId is a synthesized UUID ('9ce8dbbf-29ac-0357-dc93-0cb6399ccbd0') instead of a 'prod-<id>' Marketplace product identifier. $.addons[0].marketplaceInformation.productUrl is built as '/marketplace/pp/prodview-<truncated synthesized id>' (reported by the replay as verdict 'host_not_own') rather than the listing's recorded '/marketplace/pp?sku=<sku>' URL — the host is aws.amazon.com in both, so the divergence is the path form and the embedded product identifier, not the host. $.addons[0].addonVersions[0] is a different, generic record: a 'v3.2.0-eksbuild.1' version with computeTypes omitted entirely and only two compatibilities entries pinned to the two newest minors the emulator knows (1.36 defaultVersion false, 1.37 defaultVersion true), in place of the FIPS version AWS publishes with its six supported minors. Status (200), error-free envelope and header shape are not at issue; only the reference data is.

Why: a client that discovers a FIPS-capable Marketplace add-on through this catalog cannot act on any of the four fields. The version it must pin for CreateAddon/UpdateAddon is absent, so no FIPS-capable version can be selected and the compute-type/compatibility checks a caller makes against its own cluster minor are answered from invented data. A subscriber that follows marketplaceInformation.productId/productUrl to accept the seller EULA — the step that gates installing a Marketplace add-on — is sent to a product identifier and page that do not exist, so the subscription can never be completed. And a client that keys on publisher (or filters with the publishers query parameter) using the seller's real capitalization matches nothing.

Fix: the static add-on catalog seed behind DescribeAddonVersions in the eks service crate — the Marketplace add-on entries' publisher/marketplaceInformation reference data and the per-version records (addonVersion, computeTypes, compatibilities) — plus whatever code path currently derives publisher, productId and productUrl from the add-on name rather than reading them from the seed. Not the serializer or the handler: the envelope, filtering and status are already correct.

Related open issues that do NOT cover this cause: '20261109T131212Z-eks-describeaddonversions-catalog-omits-further-marketplace-add-ons.md' (its cause owns the Datadog-operator omission and the AccuKnox/EG-Innovations and node-monitoring-agent fixtures' productUrl cluster, replay key eks|DescribeAddonVersions|2xx->2xx|$.addons[*].marketplaceInformation.productUrl) and '20261102T083656Z-eks-add-on-and-cluster-version-catalogs-serve.md' / '20261111T150633Z-eks-add-on-cluster-version-and-configuration-schema.md' (wrong version records for other add-ons and the cluster-version and configuration-schema catalogs). The replay reports this cluster as owned by none of them; it is a distinct replay unit covering a distinct catalog entry and the publisher/productId fields none of those issues names.

Traces: eks-efsfips-traj-marketplace-fips-addon-detour#task#6 (representative; cluster has 1 affected step).

Note on values: the concrete version string, sku, product id and minor list above are the recording's examples of the requirement — the catalog must carry, per Marketplace add-on, the seller's published publisher name, that listing's Marketplace productId and productUrl, and each version's real architectures, computeTypes and per-minor compatibilities. They are not literals to hard-code per run.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 55 fixtures, 393 steps, 278 passed, 114 mismatched, 1 errored, score 70.7%. bootstrap_signal false; emulator_up true (/__admin/health answered 200 and the data listener accepts).

Divergence cluster key: service=eks|DescribeAddonVersions|status=200->200|keys=$.addons[0].addonVersions[0],$.addons[0].marketplaceInformation.productId,$.addons[0].marketplaceInformation.productUrl,$.addons[0].publisher — 1 affected step, owned_by [] (owned by no open issue), stub_path empty.

Representative step: DescribeAddonVersions, fixture eks-efsfips-traj-marketplace-fips-addon-detour, step task#6. Expected status 200, actual status 200.

Key-by-key, verbatim from the replay detail:
  $.addons[0].addonVersions[0]: value_mismatch
    expected {'addonVersion': 'v4.0.20001020260428-eksbuild.1', 'architecture': ['amd64', 'arm64'], 'computeTypes': ['ec2'], 'compatibilities': [{'clusterVersion': '1.35', 'platformVersions': ['*'], 'defaultVersion': False}, {'clusterVersion': '1.34', ...False}, {'clusterVersion': '1.33', ...False}, {'clusterVersion': '1.32', ...False}, {'clusterVersion': '1.31', ...False}, {'clusterVersion': '1.30', ...False}], 'requiresConfiguration': False, 'requiresIamPermissions': True}
    got {'addonVersion': 'v3.2.0-eksbuild.1', 'architecture': ['amd64', 'arm64'], 'compatibilities': [{'clusterVersion': '1.36', 'platformVersions': ['*'], 'defaultVersion': False}, {'clusterVersion': '1.37', 'platformVersions': ['*'], 'defaultVersion': True}], 'requiresConfiguration': False, 'requiresIamPermissions': True}
    -> computeTypes member absent altogether; wrong version; 2 synthesized minors instead of the 6 recorded supported minors.
  $.addons[0].publisher: value_mismatch (expected 'Chainguard', got 'chainguard') -> capitalization of the seller's published name is lost because the value is derived from the add-on-name token.
  $.addons[0].marketplaceInformation.productId: value_mismatch (expected 'prod-butapz3xfcrww', got '9ce8dbbf-29ac-0357-dc93-0cb6399ccbd0') -> a synthesized UUID in place of a Marketplace 'prod-<id>' product identifier.
  $.addons[0].marketplaceInformation.productUrl: host_not_own (expected 'https://aws.amazon.com/marketplace/pp?sku=ev1rr6qr2ftfm4qjeljc5och7', got 'https://aws.amazon.com/marketplace/pp/prodview-9ce8dbbf29ac0') -> same host, different path form, and the embedded identifier is the first 13 characters of the synthesized productId above, confirming the entry is generated rather than seeded.

Model basis (smithy_query on services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): AddonInfo{addonName,type,addonVersions,publisher,owner,marketplaceInformation,defaultNamespace}; MarketplaceInformation{productId,productUrl}; AddonVersionInfo{addonVersion,architecture,computeTypes,compatibilities,requiresConfiguration,requiresIamPermissions}; Compatibility{clusterVersion,platformVersions,defaultVersion}. The model documents every one of these members but constrains none of their values, so the expected values are recorded behavior.

No status, error code or error message diverges in this cluster — the call succeeds in both AWS and the emulator; the defect is entirely in the returned catalog data.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it; confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1 (fixture eks-efsfips-traj-marketplace-fips-addon-detour) and go to step task#6.
3. That step issues the name-filtered catalog read GET /addons/supported-versions for the Marketplace FIPS EFS CSI driver add-on. The emulator answers 200, so the divergence is only visible in the body.
4. Inspect $.addons[0].publisher: the emulator returns the lower-cased name-derived token ('chainguard') where the recording has the seller's published, capitalized name ('Chainguard').
5. Inspect $.addons[0].marketplaceInformation: productId is a synthesized UUID instead of a Marketplace 'prod-<id>' identifier, and productUrl is built as '/marketplace/pp/prodview-<first 13 chars of that UUID>' instead of the listing's recorded '/marketplace/pp?sku=<sku>' URL. The shared identifier shows the two fields are generated from one synthesized seed.
6. Inspect $.addons[0].addonVersions[0]: the computeTypes member is missing entirely, the addonVersion is a generic 'v3.x' value rather than the FIPS version AWS publishes, and compatibilities lists only the two newest minors the emulator knows (one marked defaultVersion true) instead of the six supported minors in the recording.
7. Re-replay this fixture step after seeding the Marketplace add-on entries from AWS's catalog; the cluster key service=eks|DescribeAddonVersions|status=200->200|keys=$.addons[0].addonVersions[0],$.addons[0].marketplaceInformation.productId,$.addons[0].marketplaceInformation.productUrl,$.addons[0].publisher must disappear.
