Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7edee2d691bdf0e85063cccc4792b3c085a7344a

# The registration seams never apply the crate's own require_resource_kind, so a resource stored under an ARN of another resource type is permanently unresolvable while the seam's own reader still returns it

_Filed: 2026-11-20T21:03:59Z_
_Operation: GetRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `7edee2d691bdf0e85063cccc4792b3c085a7344a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE rule for which resource type an ARN-carrying member may name, stated once in `state::require_resource_kind` and applied by EVERY read path a handler takes (`State::campaign`, `State::recommender`, `State::filter`): an ARN whose resource part names another type "is not a reference to a `kind` the registry could hold", so the registry is not even consulted and the request is refused `InvalidInputException` 400 "The given <kind> ARN is invalid: <arn>".

The three registration seams — `register_campaign`, `register_recommender`, `register_filter` — never apply that helper. Each already refuses an ARN the registry could never resolve in its weakest form (`if campaign.arn.is_empty() { return false; }`), and the two sibling seams additionally refuse exactly what the runtime could not use (an unparseable filter expression; a use case not in `RECOMMENDER_USE_CASES`), with the crate stating that principle outright: "A filter's expression is validated when the filter is REGISTERED, exactly as `CreateFilter` validates it, so a stored filter is always one the runtime can apply." But an ARN of the WRONG resource type is accepted and inserted.

Two consequences, both inside this crate:

1. The entry is dead on arrival: `register_campaign` answers `true`, the ARN is occupied in `campaigns` (and `contains_key` makes any later, well-formed registration of the same ARN answer `false`), yet every inference call naming it is refused forever with "The given campaign ARN is invalid" — a condition that, unlike `Denied::unresolvable`, "can never be resolved by anything the caller waits for".
2. The crate's TWO readers of the same map disagree about whether the entry exists: the seam-side reader `state::campaign(engine, arn)` (no kind check) returns `Some(campaign)` while the handler-side reader `State::campaign(arn)` refuses it, so an emulator-internal caller that registered it and read it back sees a campaign the data plane says is not a campaign reference at all.

The fix is the check the crate already owns: apply `require_resource_kind(&arn, ResourceKind::Campaign/Recommender/Filter)` in each seam and answer `false` (as the seams already do for an empty ARN) instead of storing an entry no read path can ever resolve.

## Evidence

The rule, stated once — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:739-753`:
```
/// Check that `arn` references a resource of `kind` before anything is looked up.
///
/// A reference whose resource type is another one is not a reference to a `kind` the
/// registry could hold — no campaign can ever answer to a schema ARN — so existence is
/// never consulted for it and the refusal says what is wrong with the request instead
/// (see [`Denied::invalid_arn`]).
pub fn require_resource_kind(arn: &str, kind: ResourceKind) -> Result<(), Denied> {
    if ::service_common::arn::resource_type(arn) == Some(kind.as_str()) {
        return Ok(());
    }
    Err(Denied::invalid_arn(kind, arn))
}
```
Every READ path applies it — `src/state.rs:604-609`, `:616-621`, `:627-632`:
```
    pub fn campaign(&self, arn: &str) -> Result<&Campaign, Denied> {
        require_resource_kind(arn, ResourceKind::Campaign)?;
        self.campaigns
            .get(arn)
            .filter(|campaign| campaign.is_active(SystemTime::now()))
            .ok_or_else(|| Denied::unresolvable(arn))
    }
```
The WRITE path that skips it — `src/state.rs:982-1001`:
```
/// Register a campaign, one of the three seams that make a resource exist.
///
/// Returns `true` when the campaign was registered, and `false` without touching
/// anything when its ARN is empty, when the ARN is already registered (a campaign is
/// created once; re-registering must not silently swap a catalog out from under a
/// caller) or when `engine` does not carry this service's state.
pub fn register_campaign<Eng: Send + Sync + 'static>(engine: &Eng, campaign: Campaign) -> bool {
    ...
    if campaign.arn.is_empty() {
        return false;
    }
    let mut guard = state.lock();
    if guard.campaigns.contains_key(&campaign.arn) {
        return false;
    }
    guard.campaigns.insert(campaign.arn.clone(), campaign);
    true
}
```
(the same omission in `register_recommender`, `src/state.rs:1009-1027`, and `register_filter`, `src/state.rs:1035-1062`).

The registration-time principle the crate states for the OTHER two seams — `src/state.rs:63-66`:
```
//! * A filter's expression is validated when the filter is REGISTERED, exactly as
//!   `CreateFilter` validates it, so a stored filter is always one the runtime can
//!   apply.
```
and `src/state.rs:1003-1008`:
```
/// Register a domain recommender.
///
/// Returns `false` without touching anything when the ARN is empty or already
/// registered, when `use_case` is not one of [`RECOMMENDER_USE_CASES`] (the control
/// plane refuses an unknown use case at `CreateRecommender`, so the runtime never
/// sees one), ...
```
pinned by this crate's own suite — `tests/all/inference.rs:1516-1527`:
```
    assert!(
        !registry::register_recommender(
            engine,
            Recommender::new(
                "arn:aws:personalize:us-east-1:000000000000:recommender/unknown-use-case",
                "unknown",
                "No such use case",
                items(4)
            )
        ),
        "the seam must refuse a use case the control plane would never create"
    );
```
The second reader of the same map, with no kind check — `src/state.rs:1064-1068`:
```
/// The registered campaign under `arn`, cloned — the read side of the seam, for an
/// emulator-internal caller that wants to see what it registered.
pub fn campaign<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> Option<Campaign> {
    service_state(engine)?.lock().campaigns.get(arn).cloned()
}
```
Why the two refusals are not interchangeable — `src/state.rs:689-699`:
```
    /// Deliberately NOT [`Denied::unresolvable`]: both are `InvalidInputException`
    /// (HTTP 400), ... An ARN of the wrong resource type can never be resolved by
    /// anything the caller waits for, while "does not exist or is not active yet" is a
    /// condition that changes by itself once the create the caller started completes.
```
Test coverage: `tests/all/arn_resource_type.rs` spans the READ side exhaustively ("every operation, every member that references a registry resource ... and every other Personalize resource type a caller holds an ARN of", `tests/all/arn_resource_type.rs:17-20`) and every seam call in the suite passes a correctly typed ARN (e.g. `tests/all/arn_resource_type.rs:67-97`); no test registers a resource under an ARN of another kind, so nothing pins the seam's acceptance of one.

## Steps to Reproduce

1. Build an emulator carrying this service's state, as `tests/all/arn_resource_type.rs:63-66` does.
2. Call the seam with a well-formed Personalize ARN of ANOTHER resource type: `registry::register_campaign(engine, Campaign::new("arn:aws:personalize:us-east-1:000000000000:recommender/x", "demo", RecipeType::UserPersonalization, items(6)))`. It answers `true` (src/state.rs:988-1001 checks only emptiness and prior occupancy) and the entry is inserted into `campaigns`.
3. Read it back through the seam's own reader: `registry::campaign(engine, "arn:aws:personalize:us-east-1:000000000000:recommender/x")` returns `Some(..)` (src/state.rs:1066-1068).
4. POST `/recommendations` with that value as `campaignArn`. The handler's read path refuses with HTTP 400 `InvalidInputException` "The given campaign ARN is invalid: arn:aws:personalize:us-east-1:000000000000:recommender/x" (src/ops/get_recommendations.rs resolve -> `state.campaign(arn)` -> src/state.rs:605), and will do so for every future call: the stored campaign is unreachable, and its ARN is now occupied so a later well-formed registration of the same ARN answers `false`.
5. Compare with the sibling seams, which refuse at registration what the runtime could never use: `registry::register_recommender` with an unknown use case answers `false` (tests/all/inference.rs:1516-1527) and `registry::register_filter` with an unparseable expression answers `Err` (src/state.rs:1029-1042).
