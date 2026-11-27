Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aff844956fc3a746e6f61af85746cbd7333fe795

# preferredMaintenanceWindow is accepted with a span shorter than the documented 30-minute minimum

_Filed: 2026-11-27T17:47:17Z_
_Operation: CreateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `aff844956fc3a746e6f61af85746cbd7333fe795`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::validate_maintenance_window` checks only the SHAPE of `preferredMaintenanceWindow` (`ddd:hh24:mi-ddd:hh24:mi`, with a known day abbreviation and two wall-clock times) and never the SPAN between the two times. The model states a constraint on the span, and the same crate honours that constraint where it MINTS a window, so a request-supplied window is held to a weaker rule than a service-derived one.

As a result `CreateCluster` (and `UpdateCluster`, which calls the same validator) accepts windows of 10 minutes, of 1 minute and of zero minutes (`Mon:04:00-Mon:04:00`), stores them verbatim on the record and echoes them from every later `GetCluster`/`ListClusters` read. Nothing in the crate — no comment, no test — states a reason for accepting a sub-30-minute window; the only mention of the duration in the crate asserts the opposite.

## Evidence

Smithy, `CreateClusterInput$preferredMaintenanceWindow` (identical text on `UpdateClusterInput$preferredMaintenanceWindow`), via `smithy_query(query="doc", shape="CreateClusterInput", member="preferredMaintenanceWindow")`:
  "<i>Format</i>: <code>ddd:hh24:mi-ddd:hh24:mi</code> ... <i>Constraints</i>: Minimum 30-minute window."

The validator never looks at the span — src/state.rs:1370-1386:
```
pub fn validate_maintenance_window(
    member: &str,
    value: &str,
) -> Result<String, ValidationException> {
    if let Some(normalized) = normalized_maintenance_window(value) {
        Ok(normalized)
```
and all `normalized_maintenance_window` asks is the form — src/state.rs:1412-1428:
```
fn normalized_maintenance_window(value: &str) -> Option<String> {
    let (start, end) = value.split_once('-')?;
    Some(format!("{}-{}", normalized_day_time(start)?, normalized_day_time(end)?))
```
```
    is_time_of_day(time).then(|| format!("{day}:{time}"))
```

The same crate states and honours the rule where it DERIVES a window — src/state.rs:1441-1453:
```
/// The `preferredMaintenanceWindow` a cluster created without one reports.
...
/// the shape is the documented
/// `ddd:hh24:mi-ddd:hh24:mi` and the span is the documented 30-minute minimum.
pub fn default_maintenance_window(cluster_name: &str) -> String {
```

Call sites that accept the unchecked value: src/ops/create_cluster.rs:236-242 and src/ops/update_cluster.rs:220-226 (`state::validate_maintenance_window("preferredMaintenanceWindow", window)`), whose results are stored at src/ops/create_cluster.rs:317-318 and src/ops/update_cluster.rs:269-272.

Test coverage: every window the suites exercise is exactly 30 minutes (tests/all/create_cluster_service_rules.rs:361-368 "A different 30-minute window per row", tests/all/cluster_lifecycle.rs:277, 679, 699, 1096), and the refusal table at tests/all/create_cluster_service_rules.rs:445-452 contains only bad days and a bad hour — no test pins acceptance of a shorter span, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. POST /cluster with a valid body plus "preferredMaintenanceWindow": "Mon:04:00-Mon:04:10" (a 10-minute window).
2. Observe HTTP 200 and `cluster.preferredMaintenanceWindow == "Mon:04:00-Mon:04:10"`, where the model states "Constraints: Minimum 30-minute window."
3. GET /cluster/{clusterArn} — the sub-minimum window is persisted and echoed.
4. Repeat with "Mon:04:00-Mon:04:00" (zero-length window) and with PUT /cluster/{clusterArn} carrying the same member: both are accepted too.
