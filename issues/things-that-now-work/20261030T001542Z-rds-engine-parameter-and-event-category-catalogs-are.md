Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se rds-catalog-describe-db-major-engine-versions rds-catalog-describe-event-categories rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge rds-catalog-describe-orderable-db-instance-options-db-r5d-8xlarge-db-x2iedn-xlarge rds-cluster-parameter-group-mutation-envelopes rds-db-parameter-group-lifecycle rds-db-parameter-group-lifecycle-and-engine-defaults rds-event-subscription-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions
Issue-Key: rds|DescribeDBMajorEngineVersions|2xx->2xx|$.DescribeDBMajorEngineVersionsResponse.DescribeDBMajorEngineVersionsResult.DBMajorEngineVersions.DBMajorEngineVersion[*]
Issue-Key: rds|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*]
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].IsModifiable
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].SupportedEngineModes
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*]
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Marker
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].IsModifiable
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
Issue-Key: rds|DescribeEngineDefaultClusterParameters|2xx->2xx|$.DescribeEngineDefaultClusterParametersResponse.DescribeEngineDefaultClusterParametersResult.EngineDefaults.Parameters.Parameter[*].SupportedEngineModes
Issue-Key: rds|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap.EventCategories.EventCategory[*]
Issue-Commit: 263537c8d0bbf6adc085b4147c3053be10a9244f

# rds engine, parameter and event-category catalogs are synthesized placeholders instead of the real RDS catalog

> _Note: this issue was identified against the code base at commit `263537c8d0bbf6adc085b4147c3053be10a9244f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
The RDS emulator answers every read-only catalog operation (engine versions, major engine versions, event categories, orderable instance options, parameter and engine-default parameter listings) from a small hand-written placeholder table rather than from RDS's catalog. The responses are the right SHAPE but the wrong CONTENT: wrong members, wrong member sets per entry, and a different ordering from the one RDS serves. Ten independent catalog surfaces diverge; all share one root cause, the synthesized seed catalog, so they are bundled here. There are 10 causes to close.

Cause 1 of 10: DescribeDBEngineVersions serves engine-version entries that are not the service's.
AWS: Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage.DBEngineVersions (list of DBEngineVersion); recorded behavior only for catalog CONTENT (the model constrains no values). The recorded response for a filtered query (e.g. Engine=sqlserver-dev-se) returns one DBEngineVersion per real published version, each carrying DBEngineVersionDescription, DBParameterGroupFamily, MajorEngineVersion/MinorEngineVersion split, DefaultCharacterSet, the Supports* capability flags and a ValidUpgradeTarget list enumerating the real upgrade paths with AutoUpgrade/IsMajorVersionUpgrade per target.
Emulator: returns a DBEngineVersions element whose entry list does not match the catalog at all - the replay reports a type_mismatch on the whole DBEngineVersions member for the recorded filter, i.e. the emulator's synthesized entries and the service's entries do not line up on version, description, parameter-group family or upgrade targets.
Why: callers use this operation to pick an engine version before CreateDBInstance/CreateDBCluster and to compute upgrade paths. Against the emulator they select versions that do not exist and compute upgrade graphs that are empty or wrong, so provisioning and upgrade-planning code that works against AWS fails or silently picks a different engine.
Fix: the RDS engine-version catalog seed and the DescribeDBEngineVersions handler's filtering/projection (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_engine_versions.rs and the catalog it reads).
Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#4.

Cause 2 of 10: DescribeDBMajorEngineVersions reports the wrong lifecycle-support windows.
AWS: Smithy: DescribeDBMajorEngineVersions -> DBMajorEngineVersion with SupportedEngineLifecycles (SupportedEngineLifecycle: LifecycleSupportName, LifecycleSupportStartDate, LifecycleSupportEndDate); recorded behavior only for the dates. The recorded response gives, per (Engine, MajorEngineVersion), the published standard- and extended-support windows, with end dates expressed at the end of the day (…T23:59:59.999Z).
Emulator: returns the same (Engine, MajorEngineVersion) pairs but with fabricated windows - e.g. for aurora-mysql 5.7 standard support start 2017-10-24T00:00:00Z / end 2024-10-31T00:00:00Z and extended end 2027-10-31T00:00:00Z, where the service reports start 2018-02-06T00:00:00Z / end 2024-10-31T23:59:59.999Z and extended end 2029-06-30T23:59:59.999Z. 39 entries diverge.
Why: this operation exists so customers can plan version end-of-life. Wrong dates make every compliance/EOL report derived from the emulator wrong, and the truncated end-of-day timestamps also change date-boundary comparisons.
Fix: the major-engine-version lifecycle seed data behind DescribeDBMajorEngineVersions.
Traces: rds-catalog-describe-db-major-engine-versions#task#1.

Cause 3 of 10: DescribeEventCategories returns the wrong source-type set, in the wrong order, with the wrong category lists.
AWS: Smithy: DescribeEventCategories -> EventCategoriesMessage.EventCategoriesMapList of EventCategoriesMap {SourceType, EventCategories}; recorded behavior only for content/order. The recorded response begins with SourceType db-snapshot (categories notification, creation, deletion, restoration) and lists each source type's categories in the service's own order (for db-instance: backup, deletion, availability, creation, low storage, restoration, configuration change, failover, maintenance, failure, notification, read replica, recovery, security, backtrack, security patching).
Emulator: emits a different sequence of source types (its index 0 is db-instance, index 2 db-parameter-group, index 5 db-cluster-snapshot) and alphabetises each category list, so 9 of the EventCategoriesMap entries diverge.
Why: clients that index the list positionally, or that diff the category list for a source type before calling CreateEventSubscription, get the wrong categories; subscriptions are then created on categories RDS does not offer for that source type.
Fix: the static event-category table and its ordering in the DescribeEventCategories handler.
Traces: rds-catalog-describe-event-categories#task#1.

Cause 4 of 10: DescribeOrderableDBInstanceOptions serves a synthesized option catalog that never matches the service's.
AWS: Smithy: DescribeOrderableDBInstanceOptions -> OrderableDBInstanceOptionsMessage.OrderableDBInstanceOptions (OrderableDBInstanceOption); recorded behavior only for content. The recorded response enumerates one option per (Engine, EngineVersion, DBInstanceClass, StorageType, LicenseModel) combination actually orderable in the region, each carrying its AvailabilityZones list, Min/MaxStorageSize, Iops/StorageThroughput envelopes and the Supports* capability flags.
Emulator: answers 200 with an OrderableDBInstanceOptions element that type_mismatches the recorded one for every one of the 269 affected steps across the whole paginated sweep - the emulator's option rows are not the region's orderable rows.
Why: this is the operation clients use to decide whether a class/engine/storage combination can be provisioned at all. Against the emulator a capacity-planning or validation step accepts combinations AWS refuses and rejects combinations AWS allows.
Fix: the orderable-options catalog seed plus the filtering/projection in the DescribeOrderableDBInstanceOptions handler.
Traces: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 (269 affected steps in the cluster).

Cause 5 of 10: DescribeDBClusterParameters drops every descriptive member of each Parameter.
AWS: Smithy: Parameter = {ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion, ApplyMethod, SupportedEngineModes}. The recorded cluster-parameter listing carries AllowedValues, ApplyType, DataType, Description, IsModifiable and SupportedEngineModes on every Parameter.
Emulator: returns Parameter entries with only the name/value-ish members; AllowedValues, ApplyType, DataType, Description, IsModifiable and SupportedEngineModes are absent on every element, and the element set itself differs from index 2 onward.
Why: tooling that validates a candidate ModifyDBClusterParameterGroup value against AllowedValues/IsModifiable, or that renders a parameter reference, has nothing to validate against and either accepts invalid values or refuses valid ones.
Fix: the cluster-parameter catalog seed and the Parameter serializer used by DescribeDBClusterParameters.
Traces: rds-cluster-parameter-group-mutation-envelopes#task#6.

Cause 6 of 10: DescribeDBParameters returns a different parameter set from the engine's defaults, again without the descriptive members.
AWS: as Cause 5 for the Parameter shape. The recorded response for a freshly created mysql8.0-family group lists the engine's full default parameter set in name order starting at activate_all_roles_on_login, allow-suspicious-udfs, autocommit, …, each with ApplyMethod=pending-reboot and Source=engine-default.
Emulator: starts at autocommit, then general_log, innodb_file_per_table …, i.e. a ~20-entry hand-picked subset in a different order, and omits ApplyMethod. All 20 returned elements diverge.
Why: a client reading a new parameter group's contents sees parameters that do not exist for the family and misses the ones that do, so parameter-drift and baseline-comparison logic is wrong.
Fix: the engine-default parameter seed consumed by DescribeDBParameters.
Traces: rds-db-parameter-group-lifecycle#task#3.

Cause 7 of 10: DescribeEngineDefaultParameters serves the same truncated subset as the instance parameter listing.
AWS: Smithy: DescribeEngineDefaultParameters -> EngineDefaults {DBParameterGroupFamily, Marker, Parameters}; the recorded response for a family enumerates that family's engine defaults in name order (activate_all_roles_on_login, allow-suspicious-udfs, auto_generate_certs, …) with Source=engine-default.
Emulator: returns the same ~20 placeholder rows as DescribeDBParameters (autocommit, general_log, innodb_file_per_table, …), so all 20 elements diverge.
Why: the engine defaults are the baseline a client diffs a customer parameter group against; a wrong baseline makes every computed modification set wrong.
Fix: the engine-default parameter catalog behind DescribeEngineDefaultParameters.
Traces: rds-db-parameter-group-lifecycle#task#4.

Cause 8 of 10: DescribeEngineDefaultParameters' second page is also wrong, and EngineDefaults.Marker is omitted entirely.
AWS: EngineDefaults.Marker is the operation's outputToken (Smithy paginated: inputToken=Marker, outputToken=EngineDefaults.Marker, items=EngineDefaults.Parameters). When more defaults remain, the recorded response carries an opaque Marker and a page of real parameters whose ApplyType/IsModifiable/Description reflect the engine (e.g. allow-suspicious-udfs: static, IsModifiable false).
Emulator: omits EngineDefaults.Marker (so the page cannot be continued) and serves the placeholder rows again (autocommit/general_log) with ApplyType dynamic and IsModifiable true where the service reports static/false.
Why: a paginating client terminates after one page and never sees the rest of the engine defaults; the inverted IsModifiable/ApplyType also makes modifiable-vs-static classification wrong.
Fix: the pagination and projection in the DescribeEngineDefaultParameters handler, over the corrected catalog.
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#10.

Cause 9 of 10: DescribeEngineDefaultClusterParameters serves non-cluster parameters, omits MinimumEngineVersion/SupportedEngineModes and omits the Marker.
AWS: same EngineDefaults envelope; the recorded aurora-mysql cluster defaults start at aurora_binlog_read_buffer_size (integer, AllowedValues 8192-536870912, dynamic, ParameterValue 5242880, MinimumEngineVersion 5.7.mysql_aurora.2.04.5, SupportedEngineModes [provisioned]) and carry a Marker for the next page.
Emulator: starts at binlog_format (string, AllowedValues ROW,STATEMENT,MIXED,OFF, static, value OFF), omits MinimumEngineVersion and SupportedEngineModes on every element, and omits EngineDefaults.Marker.
Why: Aurora cluster parameters are a different namespace from instance parameters; serving instance-ish rows means a client configuring a cluster parameter group cannot find the parameters it needs and cannot page past the first chunk.
Fix: the cluster engine-default catalog and the DescribeEngineDefaultClusterParameters handler's projection/pagination.
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#11.

Cause 10 of 10: the single-source-type DescribeEventCategories projection alphabetises the category list.
AWS: for a SourceType-filtered DescribeEventCategories the recorded EventCategories list is in the service's own order (backup, deletion, availability, creation, low storage, restoration, configuration change, failover, maintenance, failure, …), not sorted.
Emulator: returns the same categories sorted alphabetically (availability, backtrack, backup, configuration change, creation, deletion, failover, failure, low storage, maintenance, …), so 12 positions diverge.
Why: clients that compare the list positionally or snapshot it see a permanent diff; this is the same seed/ordering defect as Cause 3 observed through the filtered path.
Fix: preserve the catalog's declared order in the DescribeEventCategories projection instead of sorting.
Traces: rds-event-subscription-lifecycle#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true; this service answers, so these are fidelity divergences, not a bootstrap gap).

Cause 1 - rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#4, DescribeDBEngineVersions, 200 vs 200: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions type_mismatch - expected the published sqlserver-dev-se 17.00 entries (DBEngineVersionDescription 'SQL Server 2025 17.00.4045.5.v1', DBParameterGroupFamily sqlserver-dev-se-17.0, ValidUpgradeTarget listing 17.00.4055.5.v1/17.00.4065.4.v1/17.00.4075.5.v1), got the emulator's synthesized list.

Cause 2 - rds-catalog-describe-db-major-engine-versions#task#1, 200 vs 200: 39 DBMajorEngineVersion entries differ; e.g. aurora-mysql 5.7 expected standard window 2018-02-06T00:00:00Z..2024-10-31T23:59:59.999Z and extended ..2029-06-30T23:59:59.999Z, got 2017-10-24T00:00:00Z..2024-10-31T00:00:00Z and extended ..2027-10-31T00:00:00Z.

Cause 3 - rds-catalog-describe-event-categories#task#1, 200 vs 200: EventCategoriesMap[0] expected SourceType db-snapshot {notification, creation, deletion, restoration}, got db-instance with an alphabetised 16-item list; [2] expected db-instance, got db-parameter-group; [5] expected db-cluster, got db-cluster-snapshot (9 entries diverge).

Cause 4 - rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1, 200 vs 200: OrderableDBInstanceOptions type_mismatch; 269 steps in the cluster are affected. Expected rows such as {Engine mysql, EngineVersion 8.4.9, DBInstanceClass db.c6gd.12xlarge, StorageType gp2, MinStorageSize 5, MaxStorageSize 65536, AvailabilityZones us-west-2a..d}.

Cause 5 - rds-cluster-parameter-group-mutation-envelopes#task#6, DescribeDBClusterParameters, 200 vs 200: Parameter[0].AllowedValues ('-1-2147483647'), .ApplyType ('dynamic'), .DataType ('integer'), .Description ('(ms) Sets the minimum execution time above which statements will be logged.'), .IsModifiable ('true'), .SupportedEngineModes ({member: provisioned}) all missing (got None); same for Parameter[1]; Parameter[2] missing entirely.

Cause 6 - rds-db-parameter-group-lifecycle#task#3, DescribeDBParameters, 200 vs 200: Parameter[0] expected {ParameterName activate_all_roles_on_login, ApplyMethod pending-reboot, Source engine-default, …}, got {ParameterName autocommit, …}; Parameter[1] expected allow-suspicious-udfs (static, IsModifiable false), got general_log; Parameter[2] expected autocommit, got innodb_file_per_table; all 20 elements diverge.

Cause 7 - rds-db-parameter-group-lifecycle#task#4, DescribeEngineDefaultParameters, 200 vs 200: EngineDefaults.Parameters.Parameter[0] expected activate_all_roles_on_login, got autocommit; [1] expected allow-suspicious-udfs, got general_log; [2] expected auto_generate_certs, got innodb_file_per_table; all 20 diverge.

Cause 8 - rds-db-parameter-group-lifecycle-and-engine-defaults#task#10, DescribeEngineDefaultParameters, 200 vs 200: EngineDefaults.Marker missing (expected an opaque continuation token, got None); Parameter[0].ParameterName expected allow-suspicious-udfs got autocommit, .ApplyType expected static got dynamic, .IsModifiable expected false got true; Parameter[1].ParameterName expected auto_generate_certs got general_log (same inversion).

Cause 9 - rds-db-parameter-group-lifecycle-and-engine-defaults#task#11, DescribeEngineDefaultClusterParameters, 200 vs 200: EngineDefaults.Marker missing; Parameter[0] expected aurora_binlog_read_buffer_size / integer / dynamic / AllowedValues 8192-536870912 / ParameterValue 5242880 / MinimumEngineVersion 5.7.mysql_aurora.2.04.5 / SupportedEngineModes {member: provisioned}, got binlog_format / string / static / ROW,STATEMENT,MIXED,OFF / OFF with MinimumEngineVersion and SupportedEngineModes absent.

Cause 10 - rds-event-subscription-lifecycle#task#1, DescribeEventCategories, 200 vs 200: EventCategory[0] expected 'backup' got 'availability'; [1] expected 'deletion' got 'backtrack'; [2] expected 'availability' got 'backup'; … 12 positions diverge (emulator list is alphabetised).

Smithy consulted: DescribeDBParameters (paginated inputToken=Marker/outputToken=Marker/items=Parameters), DescribeEngineDefaultParameters (outputToken=EngineDefaults.Marker), shapes Parameter, EngineDefaults, EventCategoriesMap. No value constraints are modeled for these catalogs, so the recorded responses are the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an rds client at its data listener.
2. Cause 1: DescribeDBEngineVersions(Engine='sqlserver-dev-se') and compare the DBEngineVersions entries (descriptions, DBParameterGroupFamily, ValidUpgradeTarget) with the recorded response.
3. Cause 2: DescribeDBMajorEngineVersions() and compare SupportedEngineLifecycles windows for aurora-mysql 5.7/8.0.
4. Cause 3: DescribeEventCategories() with no filter and compare the order and content of EventCategoriesMapList.
5. Cause 4: DescribeOrderableDBInstanceOptions(Engine='mysql', DBInstanceClass='db.c6gd.12xlarge') and page through; compare each OrderableDBInstanceOption.
6. Cause 5: CreateDBClusterParameterGroup then DescribeDBClusterParameters and observe Parameter entries without AllowedValues/ApplyType/DataType/Description/IsModifiable/SupportedEngineModes.
7. Causes 6-7: CreateDBParameterGroup(family mysql8.0) then DescribeDBParameters and DescribeEngineDefaultParameters(DBParameterGroupFamily='mysql8.0'); observe the placeholder subset starting at autocommit.
8. Causes 8-9: DescribeEngineDefaultParameters / DescribeEngineDefaultClusterParameters with MaxRecords set; observe the missing EngineDefaults.Marker and the wrong first page.
9. Cause 10: DescribeEventCategories(SourceType='db-instance') and observe the alphabetised category list.
10. Re-replay the listed fixtures/steps to confirm.
