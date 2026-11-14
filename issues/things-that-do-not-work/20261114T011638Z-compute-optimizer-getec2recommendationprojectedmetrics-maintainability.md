Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 22d0a813c75ce3e8d355f134bff43e3d58146669

# Projected-metrics reader re-implements crate::state::architecture_admitted inline instead of calling it

_Filed: 2026-11-14T01:16:37Z_
_Operation: GetEC2RecommendationProjectedMetrics | Class: maintainability_

> _Note: this issue was identified against the code base at commit `22d0a813c75ce3e8d355f134bff43e3d58146669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::architecture_admitted` is the crate's single rule for which recommendation options a request's `cpuVendorArchitectures` preference admits, and its doc says so ("The ONE rule in this service"). The EC2 instance reader and the EC2 instance export both call it. `GetEC2RecommendationProjectedMetrics`, which selects the very same candidates of the very same `ObservedInstance`, hand-rolls the same two-clause test inline, so the two copies can drift — and if they ever do, the option set and the ranks the projected-metrics reader reports stop matching the options the recommendation reader reports under the same preference, which is exactly what the handler's own comment promises they will not do.

The fix is to call the existing helper; the boolean it computes is identical today (`requested.is_empty() || requested.iter().any(|value| value == architecture)` versus `requested_architectures.is_empty() || requested_architectures.contains(&candidate.architecture)`), so every response and every existing test stays unchanged.

## Evidence

The one rule, in the state module — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/state.rs:1356-1366:
```
/// Whether an option of `architecture` is one a request asking for `requested` CPU vendor
/// architectures admits: ...
/// The ONE rule in this service, so a recommendation READER and an EXPORT of the same
/// recommendations cannot admit different options for the same preference.
pub fn architecture_admitted(requested: &[String], architecture: &str) -> bool {
    requested.is_empty() || requested.iter().any(|value| value == architecture)
}
```

The paths that call it — src/ops/get_ec2_instance_recommendations.rs:506-512:
```
    let options: Vec<&crate::state::InstanceConfiguration> = instance
        .candidates
        .iter()
        .filter(|candidate| {
            crate::state::architecture_admitted(requested_architectures, &candidate.architecture)
        })
```
and src/state/export.rs:1046-1052:
```
            let options: Vec<&InstanceConfiguration> = instance
                .candidates
                .iter()
                .filter(|candidate| {
                    architecture_admitted(requested_architectures, &candidate.architecture)
                })
```

The copy — src/ops/get_ec2_recommendation_projected_metrics.rs:320-328:
```
        let options: Vec<RecommendedOptionProjectedMetric> = instance
            .candidates
            .iter()
            // The candidates the request's CPU vendor architecture preference admits; a
            // request that states no preference restricts nothing.
            .filter(|candidate| {
                requested_architectures.is_empty()
                    || requested_architectures.contains(&candidate.architecture)
            })
```
immediately above the comment that depends on the two agreeing — src/ops/get_ec2_recommendation_projected_metrics.rs:361-365:
```
                    // "The top recommendation option is ranked as 1", and "the projected
                    // metric rank correlates to the recommendation option rank": a
                    // candidate's rank is its position among the options REPORTED, the way
                    // the Auto Scaling group reader ranks the options one preference admits,
                    // so the two readers of one analysis agree under the same preference.
```

Existing tests that cover both copies: tests/all/ec2_recommendation_projected_metrics.rs:661 `the_requested_cpu_architecture_selects_the_candidates_projected` and tests/all/ec2_instance_recommendations.rs (the `cpuVendorArchitectures` rows), plus tests/all/ec2_instance_recommendations_export_file.rs for the export copy.

## Steps to Reproduce

1. Read src/state.rs:1356-1366 — `architecture_admitted` is declared as "The ONE rule in this service" for admitting an option under `cpuVendorArchitectures`.
2. Read src/ops/get_ec2_instance_recommendations.rs:506-512 and src/state/export.rs:1046-1052 — both call that helper for the candidates of an `ObservedInstance`.
3. Read src/ops/get_ec2_recommendation_projected_metrics.rs:320-328 — the same filter over the same `instance.candidates` is written out inline instead of calling the helper, so the rule now lives in two places.
4. Run `cargo test -p service-com-amazonaws-computeoptimizer-computeoptimizerservice` before and after replacing the inline filter with the helper: `the_requested_cpu_architecture_selects_the_candidates_projected` and the EC2 reader/export suites pass unchanged.
