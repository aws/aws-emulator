Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge rds-catalog-describe-orderable-db-instance-options-db-r5d-8xlarge-db-x2iedn-xlarge rds-catalog-describe-db-major-engine-versions rds-catalog-describe-event-categories rds-cluster-parameter-group-mutation-envelopes rds-db-parameter-group-lifecycle rds-db-parameter-group-lifecycle-and-engine-defaults rds-refarch-aur-cluster-parameter-hardening-route
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Key: rds|DescribeDBMajorEngineVersions|2xx->2xx|$.DescribeDBMajorEngineVersionsResponse.DescribeDBMajorEngineVersionsResult.DBMajorEngineVersions.DBMajorEngineVersion[*]
Issue-Key: rds|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*]
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*]
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Marker
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterName
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterValue
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].Source
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Marker
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].MinimumEngineVersion
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterName
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterValue
Issue-Related: 20261102T002746Z-rds-describeenginedefaultclusterparameters-and-createdbsubnetgroup-synthesize-data-the-request.md
Issue-Related: 20261109T083259Z-rds-serves-emulator-authored-values-on-success-paths.md
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# RDS reference catalogs and parameter metadata are emulator-authored, not AWS's published data (9 causes)

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrderableDBInstanceOptions

## Description
The emulator's RDS read-only reference surface (orderable instance options, major engine versions, event categories) and its parameter-metadata surface (DB parameters, cluster parameters, engine defaults) are served from hand-authored tables rather than from AWS's catalog. Every operation answers 200 with a well-formed envelope, so a client cannot detect the substitution; the CONTENT is wrong in a different way for each operation below. There are NINE distinct causes in this bundle and each must be closed separately.

This is distinct from the already-open issue 20261108T233943Z-rds-describedbengineversions-serves-a-synthetic-engine-version-catalog-for.md, which owns DescribeDBEngineVersions only. None of the nine causes below is covered by it or by any other open issue.

=== Cause 1 of 9: DescribeOrderableDBInstanceOptions does not return an OrderableDBInstanceOption list ===
AWS: Smithy: DescribeOrderableDBInstanceOptions output member OrderableDBInstanceOptions targets OrderableDBInstanceOptionsList (a list), paginated {inputToken: Marker, outputToken: Marker, items: OrderableDBInstanceOptions, pageSize: MaxRecords}; input Engine is @required, with optional EngineVersion/DBInstanceClass/LicenseModel/Vpc filters. In the awsQuery wire form the list serializes as <OrderableDBInstanceOptions><OrderableDBInstanceOption>...</OrderableDBInstanceOption>...</OrderableDBInstanceOptions>, one OrderableDBInstanceOption element per orderable combination matching the request's engine, engine version, instance class and license model, each carrying AvailabilityZones, StorageType, MinStorageSize/MaxStorageSize, the Supports* capability flags and MultiAZCapable/ReadReplicaCapable.
Emulator: the replay reports verdict type_mismatch at $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions — the node is not the member-wrapped list of OrderableDBInstanceOption structures the recording carries, so no orderable combination is described for the requested engine/instance class at all. This is the single largest cluster in the corpus: 269 steps across the orderable-options fixtures land on it.
Why: this is the API clients use to decide what they are allowed to create. A client that enumerates orderable options to pick a storage type, a valid storage range, an AZ set or a capability flag gets nothing usable, so instance sizing and pre-flight validation logic either crashes on the unexpected shape or silently concludes no instance class is orderable. Terraform/CDK-style tooling that validates an instance class before CreateDBInstance cannot proceed.
Fix: the DescribeOrderableDBInstanceOptions handler and its serializer — it must emit the OrderableDBInstanceOptionsList as repeated OrderableDBInstanceOption members and must filter the catalog by the request's Engine/EngineVersion/DBInstanceClass/LicenseModel/Vpc.
Traces: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 (representative; 269 steps affected, task#1..task#25 of that fixture and others).

=== Cause 2 of 9: DescribeDBMajorEngineVersions returns the wrong engines, in the wrong order, with wrong lifecycle dates ===
AWS: Smithy: DescribeDBMajorEngineVersions -> DescribeDBMajorEngineVersionsResponse.DBMajorEngineVersions (DBMajorEngineVersionsList), paginated on Marker; each DBMajorEngineVersion has Engine, MajorEngineVersion and SupportedEngineLifecycles (SupportedEngineLifecycleList of {LifecycleSupportName, LifecycleSupportStartDate, LifecycleSupportEndDate}). The set of (Engine, MajorEngineVersion) pairs, their order, and each lifecycle's name and start/end dates are AWS catalog facts, not emulator choices.
Emulator: the list diverges positionally from index 1 onward over ~39 entries. At index 1 the emulator serves aurora-mysql 8.0 with open-source-rds-standard-support 2022-01-27 -> 2026-10-31 and extended-support 2026-11-01 -> 2029-10-31, where the recording has 2021-11-18 -> 2028-04-30 and 2028-05-01 -> 2029-07-31. At index 2 the emulator serves aurora-postgresql 11 where the recording has aurora-mysql 8.4 (whose SupportedEngineLifecycles is empty) — i.e. the emulator's catalog is both differently ordered and missing members, so every subsequent index is shifted.
Why: lifecycle dates are what end-of-support and extended-support tooling reads to decide whether a cluster must be upgraded. Wrong dates make a compliance or upgrade-planning client report the wrong deadline, and the missing/shifted entries make a lookup of a specific major version miss.
Fix: the DescribeDBMajorEngineVersions handler's catalog data and its ordering/pagination, in that operation's state/catalog table.
Traces: rds-catalog-describe-db-major-engine-versions#task#1.

=== Cause 3 of 9: DescribeEventCategories omits three source types ===
AWS: Smithy: DescribeEventCategories -> EventCategoriesMessage.EventCategoriesMapList; each EventCategoriesMap is {SourceType, EventCategories}. With no SourceType filter in the request, AWS returns a map for EVERY event source type it supports.
Emulator: the response stops after index 7. Three EventCategoriesMap entries the recording carries are absent (got None): SourceType 'blue-green-deployment' with categories [creation, failure, deletion, notification]; SourceType 'db-shard-group' with [creation, deletion, availability, maintenance, failure, notification]; SourceType 'zero-etl' with [creation, deletion, failure, notification].
Why: a client that builds an event-subscription UI or validates a CreateEventSubscription SourceType against this list will reject blue-green deployments, shard groups and zero-ETL integrations as unknown source types, even though the emulator's other operations model those resources.
Fix: the DescribeEventCategories handler's source-type/category table.
Traces: rds-catalog-describe-event-categories#task#1.

=== Cause 4 of 9: DescribeDBClusterParameters paraphrases Aurora MySQL parameter descriptions and lists a parameter name AWS never stores ===
AWS: Smithy: DescribeDBClusterParameters -> DBClusterParameterGroupDetails.Parameters (ParametersList of Parameter); Parameter.Description is documented as 'Provides a description of the parameter.' The description string is the engine catalog's own text for that parameter and is a fixed, per-parameter value — for long_query_time AWS returns exactly '(ms) Sets the minimum execution time above which statements will be logged.' and for log_statement 'Sets the type of statements logged.' Recorded behavior only for the extra entry: the recorded DescribeDBClusterParameters response for a group modified with an unknown parameter name contains NO entry for that name at the position (got an extra entry where the recording has None).
Emulator: Parameter[0].Description is 'The minimum execution time in milliseconds above which a statement is logged; -1 disables the logging.' and Parameter[1].Description is 'Which SQL statements are logged.' — emulator prose, not the catalog text. Additionally Parameter[2] is an extra entry {ParameterName: <an unmodelled name>, ParameterValue: '1', Source: 'user', ApplyMethod: 'immediate', SupportedEngineModes: [provisioned]} that the recording does not contain: the emulator stored a parameter name that is not in the family's catalog and then reports it back as a user-sourced parameter. (Hypothesis: AWS rejects the unknown name at ModifyDBClusterParameterGroup time with InvalidParameterValue, which is why it never appears in the read-back; the recording only establishes that it is absent from the read-back.)
Why: clients diff parameter descriptions and the parameter set to build configuration drift reports and to decide which parameters are tunable; an invented description breaks exact-match assertions, and a phantom user-sourced parameter makes a drift check report a modification that does not exist on AWS and would make a hardening/compliance run pass against a parameter AWS never accepted.
Fix: the DescribeDBClusterParameters handler's parameter catalog (descriptions), and the cluster-parameter-group state plus ModifyDBClusterParameterGroup validation (unknown names must not be stored).
Traces: rds-cluster-parameter-group-mutation-envelopes#task#6.

=== Cause 5 of 9: DescribeDBParameters reports the wrong Source and IsModifiable and a different parameter set ===
AWS: Smithy: DescribeDBParameters -> Parameters (ParametersList); Parameter.Source is 'The source of the parameter value.' and takes the values 'engine-default', 'system' and 'user'; Parameter.IsModifiable indicates whether the parameter can be changed. Which parameters exist in a family, and each one's Source, IsModifiable, Description and ParameterValue, are engine catalog facts.
Emulator: positional divergence over indices 9..17 of the MySQL family. For 'basedir' the emulator reports Source 'engine-default' and drops ParameterValue entirely, where the recording has Source 'system' with ParameterValue '/rdsdbbin/mysql'. For 'big_tables' the emulator reports IsModifiable 'true' and an invented Description ('Whether all internal temporary tables are stored on disk rather than in memory.') where the recording has IsModifiable 'false' and an empty Description. The parameter 'bind_address' is missing from the emulator's list altogether (the emulator has 'binlog_cache_size' at that index), so the list is both short and shifted.
Why: Source distinguishes a value AWS imposes ('system') from a default and from a user override, and IsModifiable tells a client whether ModifyDBParameterGroup will be accepted. Getting these wrong makes drift detection misattribute values and makes a client attempt modifications AWS would refuse, or skip ones it would accept.
Fix: the DescribeDBParameters handler's per-family parameter catalog and the Source/IsModifiable/ParameterValue projection in its serializer.
Traces: rds-db-parameter-group-lifecycle#task#3.

=== Cause 6 of 9: DescribeEngineDefaultParameters serves emulator prose and fabricated ParameterValues for engine defaults ===
AWS: Smithy: DescribeEngineDefaultParameters -> EngineDefaults {DBParameterGroupFamily, Marker, Parameters}. An engine default's Description, ParameterValue and MinimumEngineVersion are the engine catalog's values.
Emulator: divergence across indices 0 and 3..19. For 'activate_all_roles_on_login' the emulator serves Description 'Whether the server activates all roles granted to an account when a client authenticates as that account.' plus MinimumEngineVersion '8.0.2', where the recording has 'Automatically set all granted roles as active after the user has authenticated successfully.' and no MinimumEngineVersion. For 'auto_increment_increment' the emulator serves Description 'Controls the increment between successive AUTO_INCREMENT values.' and ParameterValue '1' where the recording has 'Intended for use with master-to-master replication, and can be used to control the operation of AUTO_INCREMENT columns' and NO ParameterValue; likewise 'auto_increment_offset' gains a ParameterValue the recording omits.
Why: an engine default with no ParameterValue means AWS does not pin that default; inventing one makes a client believe a value is in effect that is not, and the paraphrased descriptions break any exact comparison against AWS's catalog.
Fix: the DescribeEngineDefaultParameters handler's engine-default catalog — descriptions, MinimumEngineVersion, and the presence/absence of ParameterValue.
Traces: rds-db-parameter-group-lifecycle#task#4.

=== Cause 7 of 9: DescribeEngineDefaultParameters returns the wrong pagination Marker, so a paging client lands on the wrong page ===
AWS: Smithy: DescribeEngineDefaultParameters is paginated with outputToken EngineDefaults.Marker and pageSize MaxRecords; EngineDefaults.Marker is documented as 'An optional pagination token provided by a previous EngineDefaults request. If this parameter is specified, the response includes only records beyond the marker.' AWS's marker is the token identifying the first record of the NEXT page, i.e. it must be derived from the last parameter actually returned in this page.
Emulator: the Marker is 'YmlubG9nX3Jvd19pbWFnZQ==' (base64 'binlog_row_image') where the recording returns 'YmlubG9nX3Jvd3NfcXVlcnlfbG9nX2V2ZW50cw==' (base64 'binlog_rows_query_log_events'). The marker names a parameter earlier in the ordering than the page boundary the recording reports, which follows from cause 6's divergent parameter set/ordering. The same step also shows the same paraphrased-description defect on indices 2..19 (e.g. 'Sets the autocommit mode.' with a trailing period for 'Sets the autocommit mode', and emulator prose for sql_mode-adjacent and ALTER TABLE temporal parameters).
Why: a client that pages the engine defaults with this marker re-reads or skips parameters, so an enumeration of the full default set is incomplete or duplicated — and because the marker is content-derived, a fix to the catalog alone will not make the marker correct unless the boundary computation is also right.
Fix: the DescribeEngineDefaultParameters handler's pagination — compute the Marker from the last record emitted in the page.
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#10.

=== Cause 8 of 9: DescribeEngineDefaultClusterParameters omits the Marker entirely and serves a different parameter set with wrong SupportedEngineModes ===
AWS: Smithy: DescribeEngineDefaultClusterParameters -> DescribeEngineDefaultClusterParametersResult.EngineDefaults, paginated with outputToken EngineDefaults.Marker, items EngineDefaults.Parameters, pageSize MaxRecords; Parameter.SupportedEngineModes targets EngineModeList and is 'The valid DB engine modes' for that parameter.
Emulator: EngineDefaults.Marker is absent (got None) where the recording returns a token, so the response claims the page is the last one and a paging client stops after one page. On top of that the parameter page itself diverges: for 'aurora_binlog_read_buffer_size' the emulator serves SupportedEngineModes [provisioned, serverless] where the recording has provisioned only, plus emulator prose for the Description; and the entry at index 1 is a different parameter altogether (the emulator has a string/static ROW,STATEMENT,MIXED,OFF parameter where the recording has an integer/dynamic 0-36000 parameter with MinimumEngineVersion 5.7.mysql_aurora.2.04.5), shifting indices 1..19.
Why: a missing Marker silently truncates enumeration of the Aurora cluster engine defaults — the client has no way to know more pages exist. A wrong SupportedEngineModes makes a client apply a provisioned-only parameter to a serverless cluster, which AWS would refuse.
Fix: the DescribeEngineDefaultClusterParameters handler — emit EngineDefaults.Marker when more records remain, and correct the per-family cluster parameter catalog (parameter set, ordering, SupportedEngineModes, DataType/ApplyType, MinimumEngineVersion).
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#11.

=== Cause 9 of 9: DescribeDBClusterParameters paraphrases Aurora PostgreSQL parameter descriptions ===
AWS: Smithy: DescribeDBClusterParameters -> Parameters (ParametersList of Parameter), Parameter.Description = 'Provides a description of the parameter.' For the Aurora PostgreSQL family AWS returns exactly '(ms) Sets the minimum execution time above which statements will be logged.' for log_min_duration_statement and 'Enables logical decoding.' for rds.logical_replication.
Emulator: 'The minimum execution time in milliseconds above which a statement is logged; -1 disables the logging.' and 'Enables logical decoding, which a logical replication slot needs.' — emulator prose in place of the catalog text. This is the same defect class as cause 4 but on the PostgreSQL family's catalog, so fixing the MySQL strings alone does not close it.
Why: a hardening or drift-reporting client that compares the served description against AWS's catalog text reports a difference on every parameter it inspects.
Fix: the DescribeDBClusterParameters handler's Aurora PostgreSQL parameter catalog (Description strings).
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health 200). All nine causes return the EXPECTED HTTP status (200) with the expected envelope; only the body content diverges, so nothing surfaces as an error.

Cause 1 — rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1, DescribeOrderableDBInstanceOptions, 200 -> 200:
  $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions: type_mismatch — expected a member-wrapped OrderableDBInstanceOption list (first entry: Engine 'mysql', EngineVersion '8.4.9', DBInstanceClass 'db.c6gd.12xlarge', StorageType 'gp2', MinStorageSize '5', MaxStorageSize '65536', AvailabilityZones us-west-2a..d, MultiAZCapable 'true', ReadReplicaCapable 'true'); the emulator's node is of a different type. affected_count = 269.

Cause 2 — rds-catalog-describe-db-major-engine-versions#task#1, DescribeDBMajorEngineVersions, 200 -> 200:
  DBMajorEngineVersion[1]: expected aurora-mysql 8.0 lifecycles standard 2021-11-18T00:00:00Z -> 2028-04-30T23:59:59.999Z and extended 2028-05-01T00:00:00Z -> 2029-07-31T23:59:59.999Z; got aurora-mysql 8.0 with standard 2022-01-27T00:00:00Z -> 2026-10-31T23:59:59.999Z and extended 2026-11-01T00:00:00Z -> 2029-10-31T23:59:59.999Z.
  DBMajorEngineVersion[2]: expected aurora-mysql 8.4 with empty SupportedEngineLifecycles; got aurora-postgresql 11. ~39 indices diverge in total (1,2,3..8,13..27,29,33,36..46,48,52,56).

Cause 3 — rds-catalog-describe-event-categories#task#1, DescribeEventCategories, 200 -> 200:
  EventCategoriesMap[8]: expected {SourceType 'blue-green-deployment', EventCategories [creation, failure, deletion, notification]}; got None.
  EventCategoriesMap[9]: expected {SourceType 'db-shard-group', EventCategories [creation, deletion, availability, maintenance, failure, notification]}; got None.
  EventCategoriesMap[10]: expected {SourceType 'zero-etl', EventCategories [creation, deletion, failure, notification]}; got None.

Cause 4 — rds-cluster-parameter-group-mutation-envelopes#task#6, DescribeDBClusterParameters, 200 -> 200:
  Parameter[0].Description: expected '(ms) Sets the minimum execution time above which statements will be logged.'; got 'The minimum execution time in milliseconds above which a statement is logged; -1 disables the logging.'
  Parameter[1].Description: expected 'Sets the type of statements logged.'; got 'Which SQL statements are logged.'
  Parameter[2]: expected None; got an extra {ParameterName: <unknown-name>, ParameterValue: '1', Source: 'user', ApplyMethod: 'immediate', SupportedEngineModes: [provisioned]}.

Cause 5 — rds-db-parameter-group-lifecycle#task#3, DescribeDBParameters, 200 -> 200:
  Parameter[9] 'basedir': expected Source 'system', ParameterValue '/rdsdbbin/mysql'; got Source 'engine-default' and no ParameterValue.
  Parameter[10] 'big_tables': expected Description '' and IsModifiable 'false'; got Description 'Whether all internal temporary tables are stored on disk rather than in memory.' and IsModifiable 'true'.
  Parameter[11]: expected 'bind_address' (ParameterValue '*'); got 'binlog_cache_size'. Indices 9,10,11,12,14,16,17 diverge.

Cause 6 — rds-db-parameter-group-lifecycle#task#4, DescribeEngineDefaultParameters, 200 -> 200:
  Parameter[0] 'activate_all_roles_on_login': expected Description 'Automatically set all granted roles as active after the user has authenticated successfully.' and no MinimumEngineVersion; got 'Whether the server activates all roles granted to an account when a client authenticates as that account.' with MinimumEngineVersion '8.0.2'.
  Parameter[3] 'auto_increment_increment': expected Description 'Intended for use with master-to-master replication, and can be used to control the operation of AUTO_INCREMENT columns' and NO ParameterValue; got 'Controls the increment between successive AUTO_INCREMENT values.' with ParameterValue '1'.
  Parameter[4] 'auto_increment_offset': same pattern. Indices 0,3..19 diverge.

Cause 7 — rds-db-parameter-group-lifecycle-and-engine-defaults#task#10, DescribeEngineDefaultParameters, 200 -> 200:
  EngineDefaults.Marker: expected 'YmlubG9nX3Jvd3NfcXVlcnlfbG9nX2V2ZW50cw==' (base64 'binlog_rows_query_log_events'); got 'YmlubG9nX3Jvd19pbWFnZQ==' (base64 'binlog_row_image').
  Parameter[4].Description: expected 'Sets the autocommit mode'; got 'Sets the autocommit mode.'
  Parameter[5].Description: expected 'When this variable has a value of 1 (the default), the server automatically grants the EXECUTE and ALTER ROUTINE privileges to the creator of a stored routine, if the user cannot already execute and alter or drop the routine.'; got 'Whether the server automatically grants the EXECUTE and ALTER ROUTINE privileges to the creator of a stored routine.'
  Parameter[6].Description: expected 'This variable controls whether ALTER TABLE implicitly upgrades temporal columns found to be in pre-5.6.4 format.'; got 'Whether ALTER TABLE implicitly upgrades temporal columns found to be in pre-5.6.4 format.' Indices 2..19 diverge on Description/AllowedValues/DataType/ParameterName/ParameterValue/Source.

Cause 8 — rds-db-parameter-group-lifecycle-and-engine-defaults#task#11, DescribeEngineDefaultClusterParameters, 200 -> 200:
  EngineDefaults.Marker: expected 'YXdzX2RlZmF1bHRfc2FnZW1ha2VyX3JvbGU=' (base64 'aws_default_sagemaker_role'); got None.
  Parameter[0].SupportedEngineModes.member: expected 'provisioned'; got ['provisioned', 'serverless'].
  Parameter[0].Description: expected 'Read buffer size used by master dump thread when the switch aurora_binlog_use_large_read_buffer is ON.'; got 'The size in bytes of the buffer a binary log dump thread reads binary log events into before sending them to a replica.'
  Parameter[1]: expected AllowedValues '0-36000', ApplyType 'dynamic', DataType 'integer', MinimumEngineVersion '5.7.mysql_aurora.2.04.5'; got AllowedValues 'ROW,STATEMENT,MIXED,OFF', ApplyType 'static', DataType 'string', MinimumEngineVersion None. Indices 1..19 diverge.

Cause 9 — rds-refarch-aur-cluster-parameter-hardening-route#task#9, DescribeDBClusterParameters, 200 -> 200:
  Parameter[0].Description: expected '(ms) Sets the minimum execution time above which statements will be logged.'; got 'The minimum execution time in milliseconds above which a statement is logged; -1 disables the logging.'
  Parameter[1].Description: expected 'Enables logical decoding.'; got 'Enables logical decoding, which a logical replication slot needs.'

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 against it.
3. Cause 1: replay fixture rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge step task#1 — DescribeOrderableDBInstanceOptions for Engine=mysql, EngineVersion=8.4.9, DBInstanceClass=db.c6gd.12xlarge; inspect $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions and confirm it is a member-wrapped list of OrderableDBInstanceOption structures.
4. Cause 2: replay rds-catalog-describe-db-major-engine-versions step task#1 — DescribeDBMajorEngineVersions with no filter; compare DBMajorEngineVersion entries 1,2 and the remaining ~39 diverging indices, including each SupportedEngineLifecycle's name and start/end dates.
5. Cause 3: replay rds-catalog-describe-event-categories step task#1 — DescribeEventCategories with no SourceType; confirm EventCategoriesMap entries for 'blue-green-deployment', 'db-shard-group' and 'zero-etl' are present with their recorded category lists.
6. Cause 4: replay rds-cluster-parameter-group-mutation-envelopes step task#6 — DescribeDBClusterParameters on the modified Aurora MySQL cluster parameter group; confirm the two Descriptions match AWS's catalog text and that the unknown parameter name is NOT listed (and that the earlier ModifyDBClusterParameterGroup in that fixture refuses it).
7. Cause 5: replay rds-db-parameter-group-lifecycle step task#3 — DescribeDBParameters on the MySQL group; check indices 9..17, in particular basedir (Source 'system' + ParameterValue), big_tables (IsModifiable 'false', empty Description) and the presence of bind_address.
8. Cause 6: replay rds-db-parameter-group-lifecycle step task#4 — DescribeEngineDefaultParameters; check indices 0 and 3..19 for Description text, MinimumEngineVersion and the absence of ParameterValue on auto_increment_increment/auto_increment_offset.
9. Cause 7: replay rds-db-parameter-group-lifecycle-and-engine-defaults step task#10 — DescribeEngineDefaultParameters with MaxRecords set; confirm EngineDefaults.Marker is derived from the last record emitted in the page.
10. Cause 8: replay rds-db-parameter-group-lifecycle-and-engine-defaults step task#11 — DescribeEngineDefaultClusterParameters with MaxRecords set; confirm EngineDefaults.Marker is emitted when more records remain, and check Parameter[0].SupportedEngineModes and the Parameter[1] identity.
11. Cause 9: replay rds-refarch-aur-cluster-parameter-hardening-route step task#9 — DescribeDBClusterParameters on the Aurora PostgreSQL group; confirm the log_min_duration_statement and rds.logical_replication Descriptions match AWS's catalog text.
12. All nine must pass before the issue is closed.
