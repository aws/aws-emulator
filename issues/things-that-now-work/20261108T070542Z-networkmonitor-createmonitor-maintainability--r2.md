Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8f0fb02f51144bcbea286868c1e4acfa8955758b

# The source-subnet ARN is split by hand in state.rs although the crate already depends on service_common::arn, which declares itself the one ARN parse

_Filed: 2026-11-08T07:05:42Z_
_Operation: CreateMonitor | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8f0fb02f51144bcbea286868c1e4acfa8955758b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::subnet_id_of` — the structural half of resolving a probe's `sourceArn`, reached by `CreateMonitor` (both for the EC2 resolution and for the per-probe rule), `CreateProbe` and `UpdateProbe` — re-implements a six-field ARN parse by hand: it splits the value with `splitn(6, ':')`, checks the literal `arn` field, skips the partition and region, compares the account, then splits the resource on '/'. The workspace already carries exactly that parse in `service-common` (`arn::parse` / `arn::parse_of` for the six fields, `arn::resource` / `arn::resource_id_of_type` for the `<type>/<id>` split), whose own documentation states that a service needing more than the vendor reads it from there "rather than splitting the value again". That crate is already a dependency of THIS crate, and this very file already calls the same module (`::service_common::arn::region_label`) for the region grammar, with the Cargo.toml comment calling it "the ONE statement of the Region grammar". So one crate reads one ARN two ways: the region through the shared parse, the vendor/account/resource through a private copy that can drift from it.

The defect is the duplicated parse, not the service's own rules on top of it: which account must own the subnet, that the resource type is `subnet`, and the EC2 id grammar (`EC2_ID_SUFFIX_LENGTHS`) stay in this crate. For every `sourceArn` form this crate's own suites and docs describe (`arn:<partition>:ec2:<region>:<account>:subnet/<subnetId>`, including the empty-region and other-partition spellings the suites exercise) the shared parse yields the same subnet id, so the responses those tests pin are unchanged. Note for the reviewer: the shared `parse` additionally requires a NON-EMPTY partition field, which the hand-rolled copy does not — the one spelling where the private copy is looser than the tree's single statement of what an ARN is, and no test exercises it.

## Evidence

Hand-rolled copy — services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:374-392:
```
pub fn subnet_id_of(source_arn: &str) -> Option<&str> {
    // arn : partition : service : region : account : resource
    let mut segments = source_arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let _partition = segments.next()?;
    if segments.next()? != SUBNET_ARN_SERVICE {
        return None;
    }
    let _region = segments.next()?;
    if segments.next()? != ::engine::emulator_account_id() {
        return None;
    }
    let (resource_type, subnet_id) = segments.next()?.split_once('/')?;
```

The crate already in Cargo.lock that replaces it — services/common/src/arn.rs:82-92:
```
/// `value`'s six fields, or `None` when `value` is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The ONE parse: a
/// service that needs more than the vendor (the region an ARN names, the resource type
/// it addresses) reads it from here rather than splitting the value again, so no two
/// services can disagree about what an ARN is.
pub fn parse(value: &str) -> Option<Arn<'_>> {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
```
and services/common/src/arn.rs:174-177:
```
pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str> {
    let rest = resource(value)?.strip_prefix(resource_type)?;
    rest.strip_prefix('/').or_else(|| rest.strip_prefix(':'))
}
```

The dependency is already declared and already used from this same file — services/service-com-amazonaws-networkmonitor-networkmonitor/Cargo.toml:14-21:
```
# `arn::region_label`: the ONE statement of the Region grammar, applied to the
# caller-supplied signing region before it reaches the AUTHORITY of the endpoint the
# in-process EC2 read is addressed at (`src/state.rs`, `ec2_client`).
service-common = { path = "../common" }
```
and state.rs:459: `let Some(region) = ::service_common::arn::region_label(region) else {`.

Callers of the hand-rolled parse: state.rs:515 (`resolve_source_subnet`, reached from create_monitor.rs:327 and create_probe.rs:277), state.rs:1028 (`validate_probe_spec`, reached from create_monitor.rs:383, create_probe.rs:310 and update_probe.rs:333) and state.rs:1068 (`probe_subnet_key`).

Existing tests that cover these paths and must stay green unchanged: tests/all/probe_source_subnet_resolution.rs — `a_held_subnet_is_accepted_and_reports_the_vpc_it_sits_in`, `a_source_the_account_holds_no_subnet_for_is_refused`, `create_monitor_resolves_every_entry_of_its_inline_list`, `an_update_names_no_source_subnet_and_resolves_none`, `with_no_ec2_to_ask_a_source_is_refused_on_nothing` (its `arn_spelled(partition, region, subnet_id)` helper, lines 190-192); tests/all/probe_subnet_identity.rs — `SPELLINGS` (lines 43-49: partitions `aws`, `aws-us-gov`, `aws-cn`, another region and an EMPTY region segment) with `create_probe_counts_every_spelling_of_one_subnet_together`, `create_monitor_counts_every_spelling_of_one_subnet_together`, `no_spelling_of_a_full_subnet_adds_capacity`, `subnets_that_differ_in_id_keep_separate_counts`; tests/all/probes_per_subnet_limit.rs.

(This is not the already-fixed CreateMonitor maintainability issue 20261025T040639Z, which was about `Monitor.client_token` being a second copy of the idempotency token.)

## Steps to Reproduce

1. Open services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:374-399 and read `subnet_id_of`: it splits the ARN itself with `splitn(6, ':')` and `split_once('/')`.
2. Open services/common/src/arn.rs:82-108 and 174-177: `parse` performs the identical `splitn(6, ':')` field read and documents itself as "The ONE parse: a service that needs more than the vendor ... reads it from here rather than splitting the value again"; `resource_id_of_type` performs the `<type>/<id>` split.
3. Confirm the dependency already exists and is already used by the same file: Cargo.toml:21 (`service-common = { path = "../common" }`) and state.rs:459 (`::service_common::arn::region_label`).
4. Observe the drift surface: a change to what the tree considers a well-formed ARN lands in `service_common::arn` for the region read on line 459 but not in `subnet_id_of` on line 376, so one crate holds two definitions of the same ARN.
