Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes iotsitewise-windfarm-trajwire-window-interval-spellings
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="FleetAverageActivePower"].path[*]
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="FleetAverageActivePower"].type.metric.variables[*].value.hierarchyId
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="TotalActivePower"].path[*]
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="ActivePower"].path[*]
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="ActivePowerAvg"].path[*]
Issue-Key: iotsitewise|ListAssetModelProperties|2xx->2xx|$.assetModelPropertySummaries[name="ActivePowerAvg"].type.metric.variables[*].value.propertyId
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelProperties[*].type.transform.variables[*].value.propertyId
Issue-Related: 20261029T190317Z-iotsitewise-asset-and-asset-model-reads-omit-property-path.md
Issue-Related: 20261103T025010Z-iotsitewise-invalid-asset-model-definitions-settle-active-instead-of.md
Issue-Commit: bd9fe9d226ef24de2d25d46b8c18046d76d56a4d

# Metric and transform variable references echo request names instead of resolved property ids, and property paths are rooted at the property

> _Note: this issue was identified against the code base at commit `bd9fe9d226ef24de2d25d46b8c18046d76d56a4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssetModel

## Description
Asset-model read projections return the raw strings the caller supplied for metric/transform variable references, and build the property `path` from the property alone instead of from the root of the asset model. Five clusters across DescribeAssetModel and ListAssetModelProperties show the same two defects. The open issue 20261102T084333Z-iotsitewise-a-metric-variable-s-propertyid-given-as-a.md reports the propertyId half for DescribeAssetModel but carries none of these replay keys, and does not cover the hierarchyId half or ListAssetModelProperties.

The AWS rule for both members: Smithy VariableValue.propertyId doc — 'The ID of the property to use as the variable. You can use the property name if it's from the same asset model'; VariableValue.hierarchyId doc — 'The ID of the hierarchy to query for the property ID. You can use the hierarchy's name instead of the hierarchy's ID'. A name is accepted on INPUT; what is read back is the resolved server-assigned id. And AssetModelProperty.path / AssetModelPropertySummary.path are documented as 'The structured path to the property from the root of the asset model', so path[0] is the asset model itself ({id: <assetModelId>, name: <assetModelName>}) and the property's own entry comes last.

Cause 1 of 5: DescribeAssetModel on a model whose metric variable was defined with a property NAME.
  AWS: as the rule above — assetModelProperties[].type.metric.variables[].value.propertyId reads back as the property's generated UUID, and assetModelProperties[].path is [{id: <assetModelId>, name: <assetModelName>}, {id: <propertyId>, name: <propertyName>}].
  Emulator: returns the literal name the create request carried ('ActivePower') as propertyId, and omits path on every property (and omits assetModelCompositeModels / assetModelCompositeModelSummaries, which AWS returns as empty lists).
  Why: a client that reads a model back to discover ids — to address the variable's source property, or to render the model tree — gets a name where an id belongs, so any subsequent id-keyed call fails or addresses nothing. Round-tripping a described model back through UpdateAssetModel re-sends the unresolved reference.
  Fix: the asset-model read serializer/state — resolve variable references to ids at definition time (or at projection) and populate path from the model root.
  Traces: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup#task#2 (cluster also covers iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#2, iotsitewise-windfarm-trajwire-window-interval-spellings#task#4, #task#5).

Cause 2 of 5: DescribeAssetModel on a parent model whose metric variable was defined with a hierarchy NAME.
  AWS: assetModelProperties[].type.metric.variables[].value.hierarchyId reads back as the hierarchy's server-assigned id, not the name supplied on create.
  Emulator: returns the literal hierarchy name ('Turbines'); property path again absent and both composite-model collections absent where AWS returns [].
  Why: the hierarchyId is exactly how a client picks which of several hierarchies sharing a child model the metric rolls up (the member's own doc says so). Reading back a name makes the roll-up unaddressable, and two hierarchies can share a name across models.
  Fix: same component as cause 1, hierarchyId branch — recorded separately because it is a different member resolved by a different lookup.
  Traces: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup#task#4.

Cause 3 of 5: ListAssetModelProperties on the parent model — path rooted wrongly and hierarchyId unresolved.
  AWS: Smithy AssetModelPropertySummary.path -> 'The structured path to the property from the root of the asset model', so path[0] = {id: <assetModelId>, name: <assetModelName>}; and the metric variable's hierarchyId is the resolved hierarchy id.
  Emulator: path[0] is the PROPERTY itself ({id: <propertyId>, name: <propertyName>}) — the model root entry is missing entirely — and hierarchyId is the supplied name.
  Why: a path that starts at the property conveys no position in the model, so a client cannot reconstruct the model tree or disambiguate same-named properties in different composite models.
  Fix: ListAssetModelProperties projection — prepend the asset-model root entry to path and resolve variable references, sharing the resolution used by DescribeAssetModel.
  Traces: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup#task#5.

Cause 4 of 5: ListAssetModelProperties on the child model — same path rooting defect plus an unresolved propertyId.
  AWS: path[0] = {id: <assetModelId>, name: <assetModelName>}; the metric variable's propertyId is the referenced property's id.
  Emulator: path[0] is the property itself; propertyId is the literal name 'ActivePower'.
  Why: as cause 3, and the unresolved propertyId means the summary does not identify which property the metric consumes.
  Fix: same component as cause 3; recorded separately because it exercises the propertyId resolution rather than hierarchyId, and a fix for one need not cover the other.
  Traces: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup#task#6.

Cause 5 of 5: DescribeAssetModel where the unresolved reference is on a TRANSFORM property, not a metric.
  AWS: assetModelProperties[].type.transform.variables[].value.propertyId reads back as the referenced property's id; path is rooted at the asset model; assetModelCompositeModels and assetModelCompositeModelSummaries are returned as [].
  Emulator: transform variable propertyId is the literal name 'ActivePower', path is absent on every property, both composite-model collections absent.
  Why: the transform property type is served by a separate serialization branch, so clients using transforms are affected even once the metric branch is corrected.
  Fix: same read-projection component, transform branch.
  Traces: iotsitewise-windfarm-trajwire-window-interval-spellings#task#6.

## Evidence
Cause 1: iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup#task#2 (200/200) — $.assetModelProperties[1].type.metric.variables[0].value.propertyId expected a shaped id, got 'ActivePower'; $.assetModelProperties[0].path and [1].path expected [{id:<assetModelId>,name:<assetModelName>},{id:<propId>,name:<propName>}], got None; $.assetModelCompositeModels and $.assetModelCompositeModelSummaries expected [], got None.
Cause 2: ...#task#4 — $.assetModelProperties[1].type.metric.variables[0].value.hierarchyId expected a shaped id, got 'Turbines'; paths and composite-model collections as above.
Cause 3: ...#task#5 (ListAssetModelProperties) — $.assetModelPropertySummaries[name="TotalActivePower"].path[0] expected {id:<assetModelId>,name:<assetModelName>}, got {id:<propertyId>,name:'TotalActivePower'}; same for FleetAverageActivePower; its metric hierarchyId expected the resolved hierarchy id, got 'Turbines'.
Cause 4: ...#task#6 (ListAssetModelProperties) — path[0] for ActivePower and ActivePowerAvg expected the asset-model root entry, got the property's own entry; ActivePowerAvg metric variable propertyId expected the ActivePower property id, got 'ActivePower'.
Cause 5: iotsitewise-windfarm-trajwire-window-interval-spellings#task#6 — $.assetModelProperties[1].type.transform.variables[0].value.propertyId expected a shaped id, got 'ActivePower'; both property paths expected, got None; composite-model collections expected [], got None.
Smithy grounding: VariableValue.propertyId / VariableValue.hierarchyId docs (a name is accepted as input, the id identifies the object); AssetModelProperty.path and AssetModelPropertySummary.path -> AssetModelPropertyPath, documented 'The structured path to the property from the root of the asset model'; DescribeAssetModel output members assetModelCompositeModels and assetModelCompositeModelSummaries.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1/2: replay iotsitewise-windfarm-trajwire-reference-path-child-metric-rollup — create a child model with a metric variable naming a property by NAME and a parent model with a metric variable naming a hierarchy by NAME, then DescribeAssetModel at task#2 and task#4; inspect type.metric.variables[0].value.propertyId / .hierarchyId and each property's path.
3. Cause 3/4: on the same fixture, call ListAssetModelProperties at task#5 (parent) and task#6 (child); inspect assetModelPropertySummaries[].path[0] and the variable references.
4. Cause 5: replay iotsitewise-windfarm-trajwire-window-interval-spellings task#6 and inspect type.transform.variables[0].value.propertyId.
5. In every case confirm path[0] is the asset model ({id,name}) and that variable references are server-assigned ids, and that assetModelCompositeModels/assetModelCompositeModelSummaries are serialized as empty lists.
