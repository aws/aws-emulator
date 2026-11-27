// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins: **a registration seam stores a resource only under an ARN
//! that references a resource of that seam's OWN kind** — the same check every read path
//! of this registry applies to an ARN-carrying request member, applied where the entry is
//! written.
//!
//! A value whose resource type names another resource (or no resource at all) is no
//! reference to that kind, so no read path of this registry could ever resolve an entry
//! stored under it: the seam refuses the registration, exactly as it refuses one whose ARN
//! is empty, instead of occupying the key with an entry that is dead on arrival.
//!
//! Two observables follow, and both are asserted below:
//!
//! * the key stays FREE — a refused registration occupies nothing, so the seam that owns
//!   that resource type can still register the very same ARN afterwards;
//! * the registry's two readers AGREE about what exists — the seam-side reader
//!   (`state::campaign` / `state::recommender` / `state::stored_filter`) answers `None` for
//!   a value the handler-side read path refuses as an invalid reference, so no
//!   emulator-internal caller can see a resource the inference data plane says is not a
//!   reference to that kind at all.
//!
//! The table spans the class: every seam, every other Personalize resource type a caller
//! holds an ARN of (this registry's own two sibling kinds included), the near misses whose
//! resource type merely starts with or pluralises the seam's own, and the values that are
//! no resource ARN at all. The case the issue cited — the campaign seam handed a
//! recommender ARN — is one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{items, Emulator, RECOMMENDATIONS};

/// The ARN prefix every well-formed value below is built on, so a row varies only the
/// resource part.
const ARN_PREFIX: &str = "arn:aws:personalize:us-east-1:000000000000:";

/// How many entries a seeded catalog holds.
const CATALOG_SIZE: usize = 6;

/// An expression `parse` accepts, so only the ARN can decide a filter registration.
const FILTER_EXPRESSION: &str = "INCLUDE ItemID WHERE Items.YEAR >= 1900";

/// The sentence a value that references no resource of `kind` earns.
fn invalid_arn_message(kind: &str, given: &str) -> String {
    format!("The given {kind} ARN is invalid: {given}")
}

/// One of the three seams that make a resource exist in this registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seam {
    Campaign,
    Recommender,
    Filter,
}

impl Seam {
    /// Every seam, so a table row never has to list them.
    fn all() -> [Self; 3] {
        [Self::Campaign, Self::Recommender, Self::Filter]
    }

    /// The ARN resource type a reference to this seam's resource carries, which is also
    /// how a refusal naming the kind reads.
    fn kind(self) -> &'static str {
        match self {
            Self::Campaign => "campaign",
            Self::Recommender => "recommender",
            Self::Filter => "filter",
        }
    }

    /// Call the seam with `arn`. `Ok(true)` means it registered something; the filter
    /// seam's `Err` is reserved for an expression that does not parse, which never
    /// happens here.
    fn register(self, engine: &engine::InMemoryEngine, arn: &str) -> Result<bool, String> {
        match self {
            Self::Campaign => Ok(registry::register_campaign(
                engine,
                Campaign::new(
                    arn,
                    "demo",
                    RecipeType::UserPersonalization,
                    items(CATALOG_SIZE),
                ),
            )),
            Self::Recommender => Ok(registry::register_recommender(
                engine,
                Recommender::new(arn, "top-picks", "Top picks for you", items(CATALOG_SIZE)),
            )),
            Self::Filter => registry::register_filter(engine, arn, FILTER_EXPRESSION),
        }
    }

    /// Whether the seam's OWN reader — the read side of the seam — holds an entry under
    /// `arn`.
    fn holds(self, engine: &engine::InMemoryEngine, arn: &str) -> bool {
        match self {
            Self::Campaign => registry::campaign(engine, arn).is_some(),
            Self::Recommender => registry::recommender(engine, arn).is_some(),
            Self::Filter => registry::stored_filter(engine, arn).is_some(),
        }
    }

    /// An ARN that references a resource of this seam's own kind, named `name`.
    fn own_arn(self, name: &str) -> String {
        format!("{ARN_PREFIX}{}/{name}", self.kind())
    }
}

/// Every form of value that references no resource of `kind`, as a (form, value) pair.
///
/// The other Personalize resource types a caller holds ARNs of are the ones the sibling
/// control-plane crate's own ARN helpers mint — `schema_arn`, `dataset_group_arn` and
/// `dataset_arn` in
/// `services/service-com-amazonaws-personalize-amazonpersonalize/src/state.rs`, and
/// `solution_arn`, `solution_version_arn`, `dataset_import_job_arn`, `campaign_arn` and
/// `filter_arn` in that crate's `src/state/pipeline.rs`. This registry's own two sibling
/// kinds are in the list too, so each seam is offered its siblings' ARNs; the issue cited
/// only the campaign seam handed a recommender ARN.
fn values_of_another_kind(kind: &str) -> Vec<(String, String)> {
    let mut forms: Vec<(String, String)> = Vec::new();
    for (form, resource) in [
        ("a schema", "schema/sim-schema"),
        ("a dataset group", "dataset-group/sim-group"),
        ("a dataset", "dataset/sim-group/INTERACTIONS"),
        ("a solution", "solution/sim-solution"),
        ("a solution version", "solution/sim-solution/00000001"),
        ("a dataset import job", "dataset-import-job/sim-import"),
        ("a campaign", "campaign/sim-campaign"),
        ("a recommender", "recommender/sim-recommender"),
        ("a filter", "filter/sim-filter"),
    ] {
        // Skip the row that IS this seam's own kind.
        if resource.starts_with(&format!("{kind}/")) {
            continue;
        }
        forms.push((form.to_string(), format!("{ARN_PREFIX}{resource}")));
    }
    // The whole resource-type segment identifies the kind, so a type the seam's own type
    // is only a prefix of — or a pluralised spelling of it — references nothing of it.
    forms.push((
        format!("a type the {kind} type is a prefix of"),
        format!("{ARN_PREFIX}{kind}-group/sim-near-miss"),
    ));
    forms.push((
        format!("the {kind} type pluralised"),
        format!("{ARN_PREFIX}{kind}s/sim-near-miss"),
    ));
    // The type is read out of the resource TYPE segment, never out of the id.
    forms.push((
        format!("another type whose id is {kind:?}"),
        format!("{ARN_PREFIX}dataset-group/{kind}"),
    ));
    // A resource written in the `<type>:<id>` form AWS also uses.
    forms.push((
        "another type in the colon resource form".to_string(),
        format!("{ARN_PREFIX}solution:sim-solution"),
    ));
    // Values that are no resource ARN at all, so they reference nothing of any kind.
    forms.push(("an empty value".to_string(), String::new()));
    forms.push(("a bare resource name".to_string(), format!("sim-{kind}")));
    forms.push((
        "an ARN whose resource field names no type".to_string(),
        format!("{ARN_PREFIX}/sim-{kind}"),
    ));
    forms.push((
        "a value with the resource part but no ARN fields".to_string(),
        format!("{kind}/sim-{kind}"),
    ));
    forms
}

#[tokio::test]
async fn a_seam_refuses_an_arn_that_references_no_resource_of_its_own_kind() {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for seam in Seam::all() {
        for (form, value) in values_of_another_kind(seam.kind()) {
            let case = format!("the {} seam given {form}", seam.kind());
            let outcome = seam.register(engine, &value);
            assert_eq!(
                outcome,
                Ok(false),
                "case {case} ({value:?}): the seam must refuse the registration without \
                 touching anything, got {outcome:?}"
            );
            assert!(
                !seam.holds(engine, &value),
                "case {case} ({value:?}): a refused registration must store nothing, yet the \
                 seam's own reader holds an entry"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_registration_leaves_the_arn_free_for_the_seam_that_owns_its_kind() {
    // The consequence that makes a stored wrong-kind entry worse than a refusal: an
    // occupied key can never be registered again. Every ordered pair of distinct seams:
    // the wrong seam is handed the right seam's ARN, and the right seam then registers
    // that very ARN.
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for owner in Seam::all() {
        for intruder in Seam::all() {
            if intruder == owner {
                continue;
            }
            let arn = owner.own_arn(&format!("shared-{}-{}", owner.kind(), intruder.kind()));
            let case = format!(
                "the {} seam given a {} ARN, then the {} seam",
                intruder.kind(),
                owner.kind(),
                owner.kind()
            );
            let refused = intruder.register(engine, &arn);
            assert_eq!(
                refused,
                Ok(false),
                "case {case}: the intruding seam must refuse the ARN, got {refused:?}"
            );
            assert!(
                !intruder.holds(engine, &arn),
                "case {case}: the intruding seam must hold nothing under {arn}"
            );

            let accepted = owner.register(engine, &arn);
            assert_eq!(
                accepted,
                Ok(true),
                "case {case}: the refused registration must leave the ARN free, got {accepted:?}"
            );
            assert!(
                owner.holds(engine, &arn),
                "case {case}: the owning seam's registration must be readable back under {arn}"
            );
        }
    }
}

/// How a `GetRecommendations` request carries the value under test, per seam.
fn request_naming(seam: Seam, value: &str, campaign: &str) -> Value {
    match seam {
        Seam::Campaign => json!({ "campaignArn": value, "userId": "user-1", "numResults": 3 }),
        Seam::Recommender => json!({ "recommenderArn": value, "userId": "user-1" }),
        Seam::Filter => json!({ "campaignArn": campaign, "userId": "user-1", "filterArn": value }),
    }
}

#[tokio::test]
async fn the_registry_two_readers_agree_about_an_arn_of_another_kind() {
    // The second consequence: the seam-side reader and the handler-side read path must
    // never disagree about whether an entry exists. With the registration refused, both
    // say it does not — the reader with `None`, the data plane with the invalid-reference
    // refusal naming the member's own kind.
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    // A resolvable campaign, so a row testing `filterArn` fails on the filter alone.
    let campaign = Seam::Campaign.own_arn("personalization");
    assert_eq!(
        Seam::Campaign.register(engine, &campaign),
        Ok(true),
        "the campaign the filter rows reference must be registered"
    );

    for seam in Seam::all() {
        // One well-formed Personalize ARN of each OTHER kind this registry holds: the
        // boundary admits it (the modelled `Arn` pattern matches any Personalize ARN), so
        // the request reaches the registry's own check.
        for other in Seam::all() {
            if other == seam {
                continue;
            }
            let value = other.own_arn("sim-resource");
            let case = format!("a {} ARN in the {} member", other.kind(), seam.kind());

            let outcome = seam.register(engine, &value);
            assert_eq!(
                outcome,
                Ok(false),
                "case {case}: the seam must refuse the registration, got {outcome:?}"
            );
            assert!(
                !seam.holds(engine, &value),
                "case {case}: the seam's own reader must hold nothing under {value}"
            );

            let refusal = emulator
                .refusal(RECOMMENDATIONS, request_naming(seam, &value, &campaign))
                .await;
            assert_eq!(
                (refusal.status, refusal.code.as_str()),
                (400, "InvalidInputException"),
                "case {case}: expected 400 InvalidInputException, got {refusal:?}"
            );
            assert_eq!(
                refusal.message,
                invalid_arn_message(seam.kind(), &value),
                "case {case}: expected the invalid-reference refusal naming the member's kind, \
                 got {:?}",
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn a_seam_still_registers_an_arn_of_its_own_kind_and_the_data_plane_serves_it() {
    // The complement: the check refuses only what references another kind. Each seam's
    // own ARN registers, reads back through the seam's reader, and is served end to end.
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for seam in Seam::all() {
        for name in ["served", "s", "a-longer-resource-name"] {
            let arn = seam.own_arn(name);
            let case = format!("the {} seam given its own ARN {arn}", seam.kind());
            let outcome = seam.register(engine, &arn);
            assert_eq!(
                outcome,
                Ok(true),
                "case {case}: the seam must register it, got {outcome:?}"
            );
            assert!(
                seam.holds(engine, &arn),
                "case {case}: the registration must be readable back through the seam"
            );
        }
    }

    // And the data plane serves every one of them: a campaign, a recommender, and a
    // filter named beside a campaign.
    let campaign = Seam::Campaign.own_arn("served");
    for seam in Seam::all() {
        let value = seam.own_arn("served");
        let body = request_naming(seam, &value, &campaign);
        let response = emulator.ok(RECOMMENDATIONS, body).await;
        assert!(
            response.get("itemList").is_some(),
            "case the {} seam's own ARN: expected a served itemList, got {response}",
            seam.kind()
        );
    }
}
