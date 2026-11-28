Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 50e54e4605010b4ad8bf4d58feb6d67b4f503de3

# targetSnapshotName is only length-checked: the documented name form is unenforced and the name is matched case-sensitively

_Filed: 2026-11-28T02:40:43Z_
_Operation: CopyClusterSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `50e54e4605010b4ad8bf4d58feb6d67b4f503de3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CopyClusterSnapshotInput$targetSnapshotName` is the one snapshot-name member whose model documentation states a full name FORM and states that the name is case-insensitive. The handler enforces neither.

1. Form. The only check applied is `state::validate_snapshot_name`, which compares the character count against `SNAPSHOT_NAME_LEN` and nothing else — the same bound the member's own `length(min: 1, max: 63)` trait already imposes. So `targetSnapshotName` values the documentation forbids — `9lives` (first character not a letter), `nightly--1` (two consecutive hyphens), `nightly-` (trailing hyphen), `Nightly Backup` (a space, i.e. not a letter, number or hyphen) — are accepted and minted as snapshots. This crate ALREADY implements exactly this rule set: `state::validate_cluster_name` enforces the identical documented sentence for `clusterName` (character set, first character a letter, no trailing or doubled hyphen), and `CreateCluster`/`RestoreClusterFromSnapshot` call it. `CopyClusterSnapshot` has the same documented rule and calls only the length helper.

2. Case. The doc sentence "This parameter is not case sensitive" means two names differing only in case are ONE name. The duplicate-name index is keyed by the verbatim string (`guard.snapshot_names.get(&input.target_snapshot_name)`), so a copy named `NIGHTLY-1` is minted beside an existing snapshot `nightly-1` instead of colliding with it, and two snapshots end up holding what the model says is one name. The cluster side of this crate does it the other way round for its own equivalent sentence: `CreateCluster` lower-cases `clusterName` before indexing it, which is why a differently cased duplicate cluster name conflicts.

Note for the fixer: `tests/all/duplicate_name_conflict_wording.rs:260-275` (`snapshot_name_rows`) feeds one shared name list to BOTH the create path (`snapshotName`, which carries no documented form) and the copy path (`targetSnapshotName`), and two of its rows — `"Nightly Backup"` and `"sauvegarde-de-nuit-é"` — are outside the documented form. That test pins refusal WORDING, not the legality of those names, so its copy row needs a well-formed name once the form is enforced; the case-insensitivity half needs no test change at all.

## Evidence

Smithy model, `CopyClusterSnapshotInput$targetSnapshotName` (smithy_query query="doc"):
"The identifier of the new elastic cluster snapshot to create from the source cluster snapshot. This parameter is not case sensitive." / "Constraints:" / "Must contain from 1 to 63 letters, numbers, or hyphens." / "The first character must be a letter." / "Cannot end with a hyphen or contain two consecutive hyphens."

Handler — the only check applied, src/ops/copy_cluster_snapshot.rs:183-184:
```
        state::validate_snapshot_name("targetSnapshotName", &input.target_snapshot_name)
            .map_err(CopyClusterSnapshotErrorModeled::ValidationException)?;
```
and the verbatim, case-sensitive name index, src/ops/copy_cluster_snapshot.rs:237:
```
        if let Some(existing_arn) = guard.snapshot_names.get(&input.target_snapshot_name) {
```

What that helper does — length and nothing else, src/state.rs:1306-1310:
```
pub fn validate_snapshot_name(member: &str, value: &str) -> Result<(), ValidationException> {
    let (min, max) = SNAPSHOT_NAME_LEN;
    if (min..=max).contains(&value.chars().count()) {
        return Ok(());
    }
```

The sibling in this same crate that DOES implement the identical documented sentence, src/state.rs:878-900:
```
/// Refuse a `clusterName` outside the documented form.
///
/// Source: `CreateClusterInput$clusterName` doc — "Must contain from 1 to 63 letters,
/// numbers, or hyphens. The first character must be a letter. Cannot end with a
/// hyphen or contain two consecutive hyphens".
pub fn validate_cluster_name(value: &str) -> Result<(), ValidationException> {
    let (min, max) = NAME_LEN;
    let well_formed = (min..=max).contains(&value.chars().count())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
```

The case-insensitive sibling treatment, src/ops/create_cluster.rs:248-249:
```
        // "This parameter is stored as a lowercase string" — CreateClusterInput$clusterName.
        let cluster_name = input.cluster_name.to_ascii_lowercase();
```

Test coverage of the member today: tests/all/cluster_snapshot_copy.rs:584-617 exercises only the two length bounds and the empty name ("`targetSnapshotName` is modelled length(1, 63): both published bounds pass and one step past each is refused"); no row exercises the first-character, hyphen, character-set or case rules. tests/all/cluster_snapshot_copy.rs:548-582 pins the taken-name conflict only for an exactly equal name.

## Steps to Reproduce

1. CreateCluster(clusterName='copysrc', ...) and note clusterArn.
2. CreateClusterSnapshot(clusterArn=<arn>, snapshotName='nightly-1') and note snapshotArn.
3. CopyClusterSnapshot(snapshotArn=<snapshotArn>, targetSnapshotName='NIGHTLY-1') -> expect the documented name to be the same name as 'nightly-1' ("This parameter is not case sensitive"); actual: HTTP 200 and a second snapshot is minted, so ListClusterSnapshots now reports two snapshots under what the model calls one name.
4. CopyClusterSnapshot(snapshotArn=<snapshotArn>, targetSnapshotName='9lives') -> expect a ValidationException ("The first character must be a letter"); actual: HTTP 200, snapshot minted.
5. CopyClusterSnapshot(snapshotArn=<snapshotArn>, targetSnapshotName='bad--name') -> expect a ValidationException ("Cannot end with a hyphen or contain two consecutive hyphens"); actual: HTTP 200, snapshot minted.
6. CopyClusterSnapshot(snapshotArn=<snapshotArn>, targetSnapshotName='trailing-') -> expect a ValidationException; actual: HTTP 200, snapshot minted.
7. Compare with CreateCluster(clusterName='9lives'), which this crate already refuses through validate_cluster_name for the very same documented sentence.
