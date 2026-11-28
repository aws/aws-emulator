Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.get-property-value-connector-failure-rejected
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker|CreateEntity|ValidationException
Issue-Commit: 96e759e54960d5d1fd2a95fa47259d85e3163a48

# IoT TwinMaker: attributePropertyValueReader* data connectors are not recognised, so a concrete component type is decided abstract and CreateEntity refuses every component of it

> _Note: this issue was identified against the code base at commit `96e759e54960d5d1fd2a95fa47259d85e3163a48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEntity

## Description
REOPENS 20261023T123634Z-iottwinmaker-iot-twinmaker-createentity-accepts-abstract-component-types-tagresource.md: its fix left the call refused.

AWS: CreateEntity accepts a component whose componentTypeId names a component type that supplies a data connector for its externally stored properties. `isAbstract` is not a member of CreateComponentTypeRequest (Smithy: CreateEntity input has components/compositeComponents only; CreateComponentType declares functions: map<String, FunctionRequest>, FunctionRequest.implementedBy -> DataConnector{lambda, isNative}), so the service decides abstractness from what the type declares. The IoT TwinMaker data-connector interface publishes the attribute-value readers beside the time-series ones: `schemaInitializer`, `dataReader`, `dataReaderByEntity`, `dataReaderByComponentType`, `attributePropertyValueReaderByEntity`, `attributePropertyValueReaderByComponentType`, `dataWriter` (data-connector-interfaces guide). A component type that declares `attributePropertyValueReaderByEntity` for its `isStoredExternally` property therefore HAS a way to serve that property's values: it is concrete, CreateEntity returns 200, and GetPropertyValue on the entity resolves the property through that connector.

Emulator: CreateEntity is refused 400 ValidationException with message "Abstract ComponentType <componentTypeId> can not be used for creating a component" (fixed text "Abstract ComponentType " + the request's componentTypeId + " can not be used for creating a component"). Cause: `crate::state::DATA_CONNECTOR_FUNCTION_NAMES` (services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:2250-2255) lists only `dataReader`, `dataReaderByEntity`, `dataReaderByComponentType`, `dataWriter`. `component_type_is_abstract` (state.rs:2338-2356) returns `stores_externally && !has_connector`, so a type whose only connector is an attribute-value reader is stored `is_abstract = true`, and `abstract_component_type_violation` (state.rs:2425-2434) then refuses every component of it.

Why: a client that models static, externally stored attributes the documented way — one `attributePropertyValueReaderByEntity` (or `...ByComponentType`) Lambda on the component type — cannot create a single entity against the emulator. The whole entity write is refused before anything is stored, so every downstream read, data-plane write and GetPropertyValue connector path is unreachable; GetComponentType/ListComponentTypes also report such a type as abstract when AWS reports it concrete.

Fix: the data-connector name set in the component-type state module (`DATA_CONNECTOR_FUNCTION_NAMES`) must cover the published attribute-value reader connectors, so `component_type_is_abstract` (and the CreateEntity/UpdateEntity abstractness refusal that reads its stored answer) sees a connector where one is declared. The abstractness refusal itself is correct for a type that really declares no connector.

Traces: iottwinmaker.get-property-value-connector-failure-rejected — the check creates a component type with functions={"attributePropertyValueReaderByEntity": {"implementedBy": {"lambda": {...}}}} and propertyDefinitions {Serial: STRING isStoredExternally, Label: STRING with defaultValue}; the first CreateEntity is refused, so the check never reaches its GetPropertyValue assertions. Code: state.rs:2250-2255, state.rs:2338-2356, state.rs:2425-2434.

## Evidence
Expected (AWS): CreateEntity(workspaceId=<ws>, entityId=<id>, components={"Gauge": {"componentTypeId": "sim2real.twin.externalreader"}}) -> HTTP 200 with entityId/arn/creationDateTime/state, the component type being concrete because it declares an attributePropertyValueReaderByEntity connector for its isStoredExternally property.
Actual (emulator): HTTP 400 ValidationException, "Abstract ComponentType sim2real.twin.externalreader can not be used for creating a component" (botocore.errorfactory.ValidationException raised on the CreateEntity call).

## Steps to Reproduce
1. Create a workspace.
2. CreateComponentType(workspaceId=<ws>, componentTypeId="sim2real.twin.externalreader", functions={"attributePropertyValueReaderByEntity": {"implementedBy": {"lambda": {"arn": <reader fn arn>}}}}, propertyDefinitions={"Serial": {"dataType": {"type": "STRING"}, "isStoredExternally": true}, "Label": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "stored-in-twin"}}}); wait for ACTIVE.
3. GetComponentType -> the emulator reports isAbstract=true.
4. CreateEntity(workspaceId=<ws>, entityId=<id>, entityName=<id>, components={"Gauge": {"componentTypeId": "sim2real.twin.externalreader"}}) -> 400 ValidationException "Abstract ComponentType ... can not be used for creating a component" instead of 200.
