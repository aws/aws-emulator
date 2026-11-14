Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.overly-permissive-storage-tier-dependency-rules
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|CreateDomain|ValidationException
Issue-Commit: e939f3162fad9baaa59a21c0fa7179928b05c50a

# opensearch: the warm-storage unsupported-instance-type ValidationException echoes the requested `.search` instance type instead of AWS's legacy `.elasticsearch` spelling

> _Note: this issue was identified against the code base at commit `e939f3162fad9baaa59a21c0fa7179928b05c50a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: CreateDomain's refusal of an UltraWarm (WarmEnabled) topology whose hot-tier instance family does not offer warm storage carries the right status and error code but the wrong instance-type token in its message — the emulator echoes the type exactly as the request spelled it (`<family>.<size>.search`), whereas AWS rewrites it into the legacy Elasticsearch suffix (`<family>.<size>.elasticsearch`).

AWS: Smithy (services/service-com-amazonaws-opensearch-amazonopensearchservice) declares CreateDomain with errors BaseException (400, defaulted), DisabledOperationException (409), InternalException (500), InvalidTypeException (409), LimitExceededException (409), ResourceAlreadyExistsException (409) and ValidationException (400); the warm-tier dependency rules themselves are not modelled (ClusterConfig.WarmType/WarmCount carry no modelled enum or range), so this is service-side validation plus recorded behavior. Recorded behavior: a CreateDomain whose ClusterConfig sets WarmEnabled=true on a burstable hot-tier family that does not offer UltraWarm is refused with HTTP 400, error code `ValidationException`, and the message `Warm storage is not available for the <instance-type> instance type. Select a different instance type.` The fixed text is `Warm storage is not available for the ` + <instance-type> + ` instance type. Select a different instance type.`; the only variable part is <instance-type>, and AWS fills it with the requested instance type re-suffixed to the legacy `.elasticsearch` spelling — i.e. a request naming `t3.small.search` is refused naming `t3.small.elasticsearch`. The check's three sibling negatives in the same chain (ColdStorage without UltraWarm -> `BaseException` / `To use cold storage, you must have UltraWarm enabled.`; UltraWarm without dedicated master nodes -> `ValidationException` / `To use warm storage, your domain must have dedicated master nodes.`; WarmCount below the minimum -> `ValidationException` / `Warm storage node count must be between 2 and 150.`) pass, so only this link's message diverges.

Emulator: the same request is refused with the same status and the same `ValidationException` code, but the message reads `Warm storage is not available for the t3.small.search instance type. Select a different instance type.` — the instance type is interpolated verbatim from the request's ClusterConfig.InstanceType with no suffix normalisation, so the message's one variable part is wrong for every `.search`-suffixed type.

Why: a client that keys off the refusal text — matching, logging or surfacing the offending instance type to an operator, or mapping the message back onto a catalogue of Elasticsearch-spelled instance types — reads a type name AWS never emits for this error. Because both spellings denote the same hardware, the mismatch is silent: the refusal looks correct and only string-level comparison or downstream lookup of the Elasticsearch-spelled type fails.

Fix: the CreateDomain cluster-config validation in the opensearch service crate — the warm-storage instance-family check must render the offending instance type in the legacy `.elasticsearch` spelling when it formats this ValidationException message, rather than echoing the request's `.search` token. The surrounding fixed text and the `ValidationException` code are already correct and must not change.

Traces: opensearch.overly-permissive-storage-tier-dependency-rules (negative 3 of the tier chain; the other three negatives and the accepted tiered control domain in the same check pass).

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### opensearch` section.

[mismatch] opensearch.overly-permissive-storage-tier-dependency-rules
  check_opensearch_overly_permissive_storage_tier_dependency_rules.py, line 94, in _expect_rejection
  AssertionError: UltraWarm on an unsupported instance family: each broken link in the tier chain has its own diagnosis, so an emulator cannot pass by mapping every tiered topology to one complaint; expected 'Warm storage is not available for the t3.small.elasticsearch instance type. Select a different instance type.', got 'Warm storage is not available for the t3.small.search instance type. Select a different instance type.'

Expected: HTTP 400, code ValidationException, message 'Warm storage is not available for the <requested-type-with-.elasticsearch-suffix> instance type. Select a different instance type.'
Actual:   HTTP 400, code ValidationException, message 'Warm storage is not available for the <requested-type-with-.search-suffix> instance type. Select a different instance type.'

The error code and status match; only the interpolated instance-type token diverges. The check also confirms the refusal leaves no domain behind (DescribeDomain answers ResourceNotFoundException), and that assertion passes.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Call opensearch CreateDomain with EngineVersion OpenSearch_2.11, EBSOptions {EBSEnabled: true, VolumeType: gp3, VolumeSize: 10} and a ClusterConfig that enables a warm tier on a burstable hot-tier family, e.g. {InstanceType: 't3.small.search', InstanceCount: 1, DedicatedMasterEnabled: true, DedicatedMasterType: 't3.small.search', DedicatedMasterCount: 3, ZoneAwarenessEnabled: false, WarmEnabled: true, WarmType: 'ultrawarm1.medium.search', WarmCount: 2}.
3. AWS refuses with HTTP 400 ValidationException and the message naming the instance type in its `.elasticsearch` spelling ('t3.small.elasticsearch'); the emulator refuses with the same status and code but names 't3.small.search'.
4. Replay opensearch.overly-permissive-storage-tier-dependency-rules from the behavioral suite; it fails on that message comparison in _expect_rejection.
