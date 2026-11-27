Issue-Source: trace-sweep-divergence
Issue-Trace: glue-get-plan-script-generation
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-get-plan-script-generation glue-connection-type-catalog
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetPlan|2xx->2xx|$.PythonScript
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Categories
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.LogoUrl
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Vendor
Issue-Commit: 489c1197152cf1b4a01399a5eda62086fe4d180a

# Glue synthesized read documents diverge from AWS: GetPlan script text and the DescribeConnectionType catalog entry

> _Note: this issue was identified against the code base at commit `489c1197152cf1b4a01399a5eda62086fe4d180a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlan

## Description
This bundle carries 2 causes. Both are read operations whose whole response document is synthesized by the emulator rather than reproduced as AWS composes it: the generated ETL script, and the connection-type catalog entry. A fixer must close both before submitting.

Cause 1 of 2 — GetPlan's generated PythonScript is not the script AWS generates.
AWS: Smithy (service-com-amazonaws-glue-awsglue): GetPlan input Mapping (required), Source (required), Sinks, Location, Language, AdditionalPlanOptionsMap; output PythonScript / ScalaCode. The script text itself is recorded behavior only, and the recording shows a stable template: a "## @params: [JOB_NAME]" comment above the getResolvedOptions line, and every generated frame carrying a transformation_ctx argument plus "## @inputs: [frame = <previous>]" naming its input by keyword. The variable parts of that template come from the request — the Source's database/table names, the Mapping tuples, and the sink's connection_type/connection_options/format — and the frame names (datasource0, applymapping1, datasink2) are positional, not per-run values.
Emulator: answers 200 with a PythonScript built from a different skeleton: the "## @params: [JOB_NAME]" comment is missing, the generated from_catalog/ApplyMapping/write_dynamic_frame frames omit the transformation_ctx argument, and the "## @inputs" comments list positional frames instead of the "[frame = ...]" keyword form.
Why: the returned script is what a caller writes to S3 and hands to a Glue job. Without transformation_ctx the generated job has no job-bookmark context for its source/sink, so bookmark-based incremental runs silently reprocess data; tooling (and the Glue console round-trip) that parses the "## @type/@args/@inputs" annotation frames to rebuild the DAG reads a different graph than AWS emits, and a diff against a stored AWS-generated script never converges.
Fix: the GetPlan handler's script generator (Glue script-generation component) must emit AWS's template — the @params preamble, a transformation_ctx argument per generated frame, and keyword-form @inputs/@args comments — rather than its own skeleton. The already-open 20261125T000203Z-glue-createscript-generates-script-text-aws-does-not.md records the same skeleton defect for CreateScript; it does not cover GetPlan, which is a separate handler, and the fix should be shared by both.
Traces: glue-get-plan-script-generation#task#3.

Cause 2 of 2 — DescribeConnectionType serves a placeholder entry instead of AWS's connection-type catalog document.
AWS: Smithy: DescribeConnectionType input ConnectionType (required); output ConnectionType, Description, Capabilities, ConnectionProperties, ConnectionOptions, AuthenticationConfiguration (AuthConfiguration), ComputeEnvironmentConfigurations, PhysicalConnectionRequirements, AthenaConnectionProperties, PythonConnectionProperties, SparkConnectionProperties, RestConfiguration. Recorded behavior shows AWS returning the full published catalog entry for the requested connection type: a populated AuthenticationConfiguration (an "AuthConfiguration" document whose AuthenticationType/OAuth2Properties entries each carry DisplayName, Description, DataType, Phase, PropertyType(s), Required, Validations and AllowedValues/DefaultValue), plus Capabilities, ConnectionProperties, ConnectionOptions, PhysicalConnectionRequirements, Description — and the members Categories, DisplayName, LogoUrl and Vendor, which the on-disk model does not even declare on the output shape.
Emulator: answers 200 with a document that diverges on every one of those members (AuthenticationConfiguration, Capabilities, Categories, ConnectionOptions, ConnectionProperties, Description, DisplayName, LogoUrl, PhysicalConnectionRequirements, Vendor) — a generic placeholder rather than the per-connection-type catalog entry.
Why: this operation IS the catalog a client reads to learn which authentication types and properties a connection type accepts, and to render a connection form. A caller gets no AllowedValues/Required/Phase metadata, so it cannot validate or build a CreateConnection payload, and the absent Categories/DisplayName/LogoUrl/Vendor members break any consumer that renders the connection-type listing. Note the model's output shape is also missing Categories/DisplayName/LogoUrl/Vendor, so serving them needs the serializer to carry members beyond the checked-in model.
Fix: the DescribeConnectionType handler needs a real per-connection-type catalog data source behind it (and the response serializer extended to carry the undocumented Categories/DisplayName/LogoUrl/Vendor members) instead of a synthesized placeholder.
Traces: glue-connection-type-catalog#task#2.

## Evidence
Cause 1 (GetPlan, glue-get-plan-script-generation#task#3, status 200 both sides): $.PythonScript value_mismatch. Expected text contains "## @params: [JOB_NAME]", "transformation_ctx = \"datasource0\"" on the from_catalog call, "## @inputs: [frame = datasource0]" and a write_dynamic_frame.from_options carrying transformation_ctx; the emulator's text drops the @params comment, the transformation_ctx arguments and the keyword @inputs form. Database/table/bucket names in the recording are that fixture's values, not part of the requirement.
Cause 2 (DescribeConnectionType, glue-connection-type-catalog#task#2, status 200 both sides): divergence keys $.AuthenticationConfiguration, $.Capabilities, $.Categories, $.ConnectionOptions, $.ConnectionProperties, $.Description, $.DisplayName, $.LogoUrl, $.PhysicalConnectionRequirements, $.Vendor — expected a populated AuthConfiguration (AuthenticationType with AllowedValues/Required/Phase/PropertyTypes, OAuth2Properties entries with DefaultValue/ConditionallyRequired, etc.) and the catalog display members; the emulator's document matches none of them.

## Steps to Reproduce
Cause 1: create a database and table, then call GetPlan with Mapping, Source = that database/table and a Sinks entry (or Location) and Language=PYTHON -> compare the returned PythonScript with AWS's: the emulator's omits the "## @params: [JOB_NAME]" preamble, every transformation_ctx argument and the "## @inputs: [frame = ...]" keyword form. Replay glue-get-plan-script-generation#task#3.
Cause 2: call DescribeConnectionType with ConnectionType set to a published connection type -> the emulator returns a placeholder document; AWS returns the catalog entry with AuthenticationConfiguration/Capabilities/ConnectionProperties/ConnectionOptions/PhysicalConnectionRequirements/Description plus Categories, DisplayName, LogoUrl and Vendor. Replay glue-connection-type-catalog#task#2.
