Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 58d3cdd8a54717975c7a5e45d2c60ee5b89f816e

# UpdateConfig can repoint a dataflowEndpointConfig at another group's endpoint, leaving a stored mission profile holding a graph no one dataflow endpoint group can serve

_Filed: 2026-10-29T14:46:19Z_
_Operation: UpdateConfig | Class: validation_

> _Note: this issue was identified against the code base at commit `58d3cdd8a54717975c7a5e45d2c60ee5b89f816e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConfig` replaces a stored config's `configData` wholesale. For a `dataflow-endpoint` config that means it replaces `dataflowEndpointConfig.dataflowEndpointName` — the very member every mission profile's dataflow graph is resolved through (`state::dataflow_endpoint_names_wired_by`). The handler's gates are: does the `(configType, configId)` pair exist; does the body select the same config type; are the `configData` numeric ranges/uplink approval satisfied; does the S3 recording bucket resolve; does an `uplinkEchoConfig` reference resolve. Not one of them asks what the mission profiles that already reference this config would then be holding.

So this sequence is accepted end to end:

1. Group `home` holds endpoints `alpha` and `beta`; group `other` holds `gamma`.
2. Config A = `dataflowEndpointConfig{dataflowEndpointName: "alpha"}`, config B = `dataflowEndpointConfig{dataflowEndpointName: "beta"}`.
3. `CreateMissionProfile` with edges ending at A and at B — accepted, because `home` holds both names.
4. `UpdateConfig` on B with `configData: {dataflowEndpointConfig: {dataflowEndpointName: "gamma"}}` — accepted (same config type, no numeric bound, no bucket, no echo reference).

The profile now holds a graph selecting `alpha` (only in `home`) and `gamma` (only in `other`). `state::dataflow_endpoint_group_fault` returns `Some(...)` for exactly that graph, so neither `CreateMissionProfile` nor `UpdateMissionProfile` would ever have stored it, and a contact reserved against the profile resolves its two dataflow endpoints out of two different groups — the state the model sentence "each `Config` must match a `DataflowEndpoint` in the same group" forbids and the state this crate's own guard and test exist to prevent.

This is NOT the forward-reference case the crate deliberately allows ("A name no group of this account holds yet constrains nothing"): both `alpha` and `gamma` are held by a group at the instant of the update, which is precisely the condition `dataflow_endpoint_group_fault` refuses on.

It is also the mirror of the inbound-reference guard `DeleteConfig` already has: a delete of config B while the profile references it is refused, but an update of B that breaks the same profile's graph is not.

## Evidence

The model sentence the state ends up violating — `smithy_query(query="doc", shape="com.amazonaws.groundstation#CreateDataflowEndpointGroup")`:

    When a contact uses multiple DataflowEndpointConfig objects, each Config must match a DataflowEndpoint in the same group.

The crate's single owner of that rule, and its own statement of the invariant — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:4824` and `:4840`:

    /// "When a contact uses multiple `DataflowEndpointConfig` objects, each `Config` must
    /// match a `DataflowEndpoint` in the same group" (`smithy_query(query="operation",
    /// operation="CreateDataflowEndpointGroup")`), so the endpoints a graph selects
    /// ([`Self::dataflow_endpoint_names_wired_by`]) have to be servable by a SINGLE group
...
    /// Shared by the create and the update, so a profile cannot be left holding a graph a
    /// create would have refused.
    pub fn dataflow_endpoint_group_fault(

The member `UpdateConfig` rewrites is the one that graph resolution reads — `state.rs:4807`:

            .filter_map(|config| match &config.config_data {
                ConfigTypeData::dataflowEndpointConfig(endpoint_config) => {
                    Some(endpoint_config.dataflow_endpoint_name.as_str())
                }

And `UpdateConfig`'s gates enumerate themselves, with no inbound-holder check among them — `src/ops/update_config.rs:169`:

        // All three gates below are judged from this service's OWN store, under one short
        // lock, before the cross-service bucket resolution further down, and IN THIS
        // ORDER: the config the URI addresses has to be there first, and only then is the
        // body judged against it — the capability the `configData` selects, and what the
        // service and this account may use in it (the ranges the model documents for its
        // numeric members and the uplink frequencies this account is authorised to
        // transmit on) ...

followed by the unconditional write — `src/ops/update_config.rs:253`:

        let stored = guard.configs.get_mut(&key).ok_or_else(not_found)?;

        // Both members are required, so an update REPLACES the config's name and its
        // parameters; its id, type and ARN are its address and do not move.
        stored.name = input.name;
        stored.config_data = config_data;

The sibling that DOES apply the guard on its own write path — `src/ops/update_mission_profile.rs:231`:

        // A graph this update carries is held to the group rule the create's is held to:
        // the dataflows of a profile are flown as one contact, which draws its dataflow
        // endpoints from one dataflow endpoint group, so an update cannot leave a profile
        // holding a graph whose endpoint selections no single group can serve (see
        // `state::dataflow_endpoint_group_fault`).
        if let Some(edges) = input.dataflow_edges.as_deref() {
            if let Some(refusal) = guard.dataflow_endpoint_group_fault(edges) {

The sibling config write that DOES protect inbound holders — `src/ops/delete_config.rs:168`:

        // A config another resource this account still holds depends on is NOT deleted:
        // a holder names the config by ARN, and dropping it out from under the holder
        // would leave that holder pointing at a reference that no longer resolves — the
        // very state this crate's create and update gates refuse to let a caller build.

Test coverage: the invariant is pinned for the mission-profile write path only — `tests/all/mission_profile_endpoint_group_agreement.rs:339` `an_update_cannot_leave_a_profile_holding_a_graph_no_one_group_can_serve`, whose assertion is "case {case}: a refused update must leave the stored graph alone" (`:384`), and the read-side consequence by `:431` `a_contact_resolves_every_dataflow_endpoint_of_its_pass_out_of_one_group`. No test in the crate drives `UpdateConfig` against a config a profile references: `grep` for `dataflowEndpointName` across `tests/` shows no UpdateConfig case at all.

## Steps to Reproduce

1. Set GROUNDSTATION_GROUND_STATIONS so the account knows a ground station, then CreateDataflowEndpointGroup twice: group 'home' holding dataflow endpoints named "alpha" and "beta", group 'other' holding one named "gamma".
2. CreateConfig a trackingConfig; CreateConfig an antennaDownlinkConfig; CreateConfig config A = {"dataflowEndpointConfig": {"dataflowEndpointName": "alpha"}} and config B = {"dataflowEndpointConfig": {"dataflowEndpointName": "beta"}}.
3. CreateMissionProfile with trackingConfigArn and dataflowEdges [[downlinkArn, Aarn], [downlinkArn, Barn]] — expect HTTP 200 (both "alpha" and "beta" are in group 'home').
4. Confirm the guard is live: PUT /missionprofile/{id} with dataflowEdges [[downlinkArn, Aarn], [downlinkArn, Garn]] where G selects "gamma" is refused 431 InvalidParameterException naming "alpha" and "gamma" as endpoints of different dataflow endpoint groups.
5. Now PUT /config/dataflow-endpoint/{configIdOfB} with {"name": "B", "configData": {"dataflowEndpointConfig": {"dataflowEndpointName": "gamma"}}}. Expected: the same 431 the equivalent mission-profile update is refused with, since the profile that references B would be left holding a graph no one group can serve. Actual: HTTP 200 and the config is rewritten.
6. GET /missionprofile/{id} and GET /config/dataflow-endpoint/{configIdOfB}: the stored profile now selects "alpha" (group 'home') and "gamma" (group 'other').
7. ReserveContact against that profile, then GET /contact/{contactId}: the dataflowList resolves its two endpoints out of two different dataflow endpoint groups, the state tests/all/mission_profile_endpoint_group_agreement.rs:431 asserts can never happen.
