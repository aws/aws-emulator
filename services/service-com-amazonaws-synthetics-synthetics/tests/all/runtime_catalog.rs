// SPDX-License-Identifier: CC0-1.0

//! The CONTENTS of the published canary runtime-version catalog, over the wire through the
//! same `MultiServiceRouter` the emulator boots (the harness in `canary_lifecycle`).
//!
//! The rule pinned here: `DescribeRuntimeVersions` reports the service's ONE published
//! catalog of canary runtime versions, newest first, where each entry carries the version's
//! name, the language canaries on it are written in, a description listing what that version
//! bundles, its release date and a deprecation date only once that version is deprecated —
//! and every version the catalog reports is a version a canary can be created on, while a
//! version it does not report is refused.
//!
//! The rows below therefore span what the rule generalises over rather than one recorded
//! call: every runtime family the service publishes (Node.js with Playwright, Node.js with
//! Puppeteer, Python with Selenium and the retired families it still names), both languages,
//! `.0` releases and patch releases, the newest and the oldest entry, and names the catalog
//! does not publish.
//!
//! The modelled shape quoted here is the model's own, read with
//! `smithy_query(query="shape",shape="RuntimeVersion",model_path="services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json")`.
//!
//! The last test pins the LOCAL DECLARATION the `Language` member needs: the published
//! model snapshot declares no such member, so the shape that carries it is generated from
//! the overlay model `smithy-model/local-additions.json` (see
//! `smithy-model/LOCAL-ADDITIONS.md`). Everything that overlay restates of the snapshot is
//! compared against it, shape by shape, so a refreshed snapshot that declares the member,
//! adds one the overlay does not restate, or changes a target or constraint fails here
//! rather than silently changing what this service reports.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::canary_lifecycle::{create_body, Answer, Synthetics};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declaration is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-synthetics-synthetics.json"
));

/// The overlay model the local `RuntimeVersion` declaration is generated from, embedded for
/// the same reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The member this crate declares locally, which the snapshot does not.
const LOCAL_MEMBER: &str = "Language";

/// The shape the overlay adds for that member, which the snapshot declares nowhere.
const LOCAL_SHAPE: &str = "com.amazonaws.synthetics#RuntimeVersionLanguage";

/// The shape the overlay restates in order to add [`LOCAL_MEMBER`] to it.
const CATALOG_ENTRY_SHAPE: &str = "com.amazonaws.synthetics#RuntimeVersion";

/// The two values `RuntimeVersion$Language` takes: a canary script is written either in
/// Node.js or in Python, which is what the runtime families the service publishes are split
/// into (the CloudWatch User Guide's canary runtime versions page, the page the model's
/// `RuntimeVersion` doc links to).
const LANGUAGES: [&str; 2] = ["nodejs", "python"];

/// What the name of a Python runtime family carries, and nothing else does.
const PYTHON_FAMILY_SEGMENT: &str = "-python-";

/// How the description of a runtime version introduces what that version bundles.
const DEPENDENCIES_LABEL: &str = "Dependencies: ";

/// How the description of a PATCH release of an already published family opens.
const SECURITY_FIX_SENTENCE: &str = "Security fixes. ";

/// The runtime families the service publishes, as a `VersionName` prefix. Every one of them
/// has to appear in the answer: the catalog is the whole published list, not one family of
/// it. None of these prefixes is quoted from a single recorded entry.
const PUBLISHED_FAMILIES: [&str; 4] = [
    "syn-nodejs-playwright",
    "syn-nodejs-puppeteer",
    "syn-python-selenium",
    "syn-nodejs",
];

/// Names that are not runtime versions this service publishes, differing along what the rule
/// generalises over: an unreleased patch of a published family, an unreleased major of
/// another, a family the service has none of, and a published family with no version.
const UNPUBLISHED: [(&str, &str); 4] = [
    ("an unreleased patch of a published family", "syn-nodejs-puppeteer-9.7"),
    ("an unreleased major of a published family", "syn-python-selenium-42.0"),
    ("a language this service has no family for", "syn-ruby-capybara-1.0"),
    ("a published family with no version", "syn-nodejs-playwright"),
];

/// One `DescribeRuntimeVersions` call: `POST /runtime-versions`, as the model binds it.
async fn describe(aws: &Synthetics, max_results: Option<i64>, next_token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    aws.call("POST", "/runtime-versions", Some(body)).await
}

/// Every catalogued entry, in the order the service reports it, across as many pages as it
/// takes.
async fn whole_catalog(aws: &Synthetics) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    // More iterations than any catalog can need at this page size, so a non-terminating
    // paging loop fails instead of hanging.
    for _ in 0..1_000 {
        let answer = describe(aws, None, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case page after {token:?}: expected HTTP 200, got {} with body {:?}",
            answer.status, answer.body
        );
        let page = answer
            .at("RuntimeVersions")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case page after {token:?}: expected a RuntimeVersions array, got {:?}",
                    answer.body
                )
            })
            .clone();
        entries.extend(page);
        token = answer
            .at("NextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            assert!(
                !entries.is_empty(),
                "expected the published runtime catalog, got an empty list"
            );
            return entries;
        }
    }
    panic!("paging over the catalog did not terminate");
}

fn version_name(entry: &Value) -> String {
    entry["VersionName"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a VersionName string, got {entry:?}"))
        .to_string()
}

/// The family prefix and the `<major>.<minor>` version a published runtime version name
/// carries, e.g. `syn-nodejs-puppeteer-9.1` -> (`syn-nodejs-puppeteer`, (9, 1)).
fn family_and_version(name: &str) -> (String, (u32, u32)) {
    let (family, version) = name
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {name}: a published version name ends in -<major>.<minor>"));
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("case {name}: expected a numeric major version, got {version:?}"));
    let minor = parts
        .next()
        .and_then(|minor| minor.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("case {name}: expected a numeric minor version, got {version:?}"));
    (family.to_string(), (major, minor))
}

/// The language canaries on `name` are written in, read off the runtime family the name
/// states — the same rule the service's own catalog follows.
fn expected_language(name: &str) -> &'static str {
    if name.contains(PYTHON_FAMILY_SEGMENT) {
        "python"
    } else {
        "nodejs"
    }
}

/// Every entry of the published catalog carries the modelled members with the values the
/// runtime family and the version they name imply: the language of the family, a description
/// listing what the version bundles, a release date, and no deprecation date while the
/// version is still offered.
#[tokio::test]
async fn every_catalogued_entry_reports_its_language_and_what_it_bundles() {
    let aws = Synthetics::new("us-west-2");
    let entries = whole_catalog(&aws).await;

    let mut languages_seen: BTreeSet<String> = BTreeSet::new();
    let mut patch_releases = 0_usize;
    let mut initial_releases = 0_usize;
    for entry in &entries {
        let case = version_name(entry);
        let (_, (_, minor)) = family_and_version(&case);

        // `RuntimeVersion$Language`: the language of the runtime family the version names,
        // so it cannot disagree with the name a caller reads beside it.
        let language = entry["Language"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Language string, got {entry:?}"));
        assert_eq!(
            language,
            expected_language(&case),
            "case {case}: expected the family's language, got {language:?}"
        );
        assert!(
            LANGUAGES.contains(&language),
            "case {case}: expected one of {LANGUAGES:?}, got {language:?}"
        );
        languages_seen.insert(language.to_string());

        // `RuntimeVersion$Description` — "A description of the runtime version, created by
        // Amazon": it lists what that version bundles, and says so after announcing the
        // security fixes when the version is a patch release of an already published family.
        let description = entry["Description"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Description string, got {entry:?}"));
        let dependencies = if minor > 0 {
            patch_releases += 1;
            let rest = description.strip_prefix(SECURITY_FIX_SENTENCE).unwrap_or_else(|| {
                panic!(
                    "case {case}: a patch release's description opens with \
                     {SECURITY_FIX_SENTENCE:?}, got {description:?}"
                )
            });
            rest
        } else {
            initial_releases += 1;
            assert!(
                !description.starts_with(SECURITY_FIX_SENTENCE),
                "case {case}: an initial release is not a security-fix release, got {description:?}"
            );
            description
        };
        let bundled = dependencies.strip_prefix(DEPENDENCIES_LABEL).unwrap_or_else(|| {
            panic!(
                "case {case}: a description lists what the version bundles after \
                 {DEPENDENCIES_LABEL:?}, got {description:?}"
            )
        });
        assert!(
            !bundled.trim().is_empty(),
            "case {case}: expected a non-empty dependency list, got {description:?}"
        );
        // The dependency list names the language runtime a canary on this version runs on,
        // which is the language the entry reports.
        let language_runtime = if language == "python" { "Python" } else { "Node JS" };
        assert!(
            bundled.contains(language_runtime),
            "case {case}: expected the dependency list to name {language_runtime:?}, got {bundled:?}"
        );
        // `RuntimeVersion$DeprecationDate` — "If this runtime version is deprecated, this
        // value is the date of deprecation": the service offers every version it reports, so
        // none of them carries one.
        assert!(
            entry["DeprecationDate"].is_null(),
            "case {case}: an offered runtime version has no DeprecationDate, got {:?}",
            entry["DeprecationDate"]
        );
    }

    // Both languages, and both kinds of release, are in the published catalog — the rows
    // above span them rather than one of them.
    for language in LANGUAGES {
        assert!(
            languages_seen.contains(language),
            "expected the catalog to publish {language:?} runtimes, got {languages_seen:?}"
        );
    }
    assert!(
        patch_releases > 0 && initial_releases > 0,
        "expected the catalog to publish both initial and patch releases, got {initial_releases} \
         initial and {patch_releases} patch"
    );
}

/// The catalog is reported newest first — across the whole list by release date, and within
/// every family by version number — and it spans every family the service publishes.
#[tokio::test]
async fn the_catalog_is_reported_newest_first_across_every_family() {
    let aws = Synthetics::new("eu-central-1");
    let entries = whole_catalog(&aws).await;

    let names: Vec<String> = entries.iter().map(version_name).collect();
    let mut families: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for name in &names {
        let (family, version) = family_and_version(name);
        families.entry(family).or_default().push(version);
    }

    for (family, versions) in &families {
        for pair in versions.windows(2) {
            assert!(
                pair[0] > pair[1],
                "case {family}: a family's versions are reported newest first, got {:?} before {:?}",
                pair[0],
                pair[1]
            );
        }
    }
    for published in PUBLISHED_FAMILIES {
        assert!(
            families.contains_key(published),
            "case {published}: expected the catalog to publish this family, got {:?}",
            families.keys().collect::<Vec<_>>()
        );
    }

    // The newest entry of the whole catalog is the newest entry of some family, never an
    // older one: this is the entry a client that pins "the current runtime" reads.
    let newest = &names[0];
    let (newest_family, newest_version) = family_and_version(newest);
    assert_eq!(
        families[&newest_family][0], newest_version,
        "case {newest}: the first entry reported must be its family's newest"
    );
}

/// The one-catalog rule, in both directions: every runtime version the catalog reports is a
/// version `CreateCanary` accepts and the store keeps, and a version the catalog does not
/// report is refused with the modelled `ValidationException`.
#[tokio::test]
async fn every_reported_version_can_create_a_canary_and_an_unreported_one_cannot() {
    let aws = Synthetics::new("ap-southeast-2");
    let names: Vec<String> = whole_catalog(&aws).await.iter().map(version_name).collect();

    // Rows spanning the catalog: its newest entry, its oldest, one in the middle, and the
    // newest version of every family it publishes — so both languages and both release kinds
    // are created on, not just the version the recordings happened to use.
    let mut rows: Vec<String> = vec![
        names[0].clone(),
        names[names.len() / 2].clone(),
        names[names.len() - 1].clone(),
    ];
    for family in PUBLISHED_FAMILIES {
        let newest = names
            .iter()
            .find(|name| family_and_version(name).0 == family)
            .unwrap_or_else(|| panic!("case {family}: the catalog publishes this family"));
        rows.push(newest.clone());
    }
    rows.sort();
    rows.dedup();

    for (index, runtime) in rows.iter().enumerate() {
        let canary = format!("reported-{index}");
        let mut body = create_body(&canary);
        body["RuntimeVersion"] = json!(runtime);
        let created = aws.create(body).await;
        assert_eq!(
            created.status, 200,
            "case {runtime}: a reported runtime version must be creatable, got {} with body {:?}",
            created.status, created.body
        );
        let read_back = aws.get(&canary).await;
        assert_eq!(
            read_back.string_at("Canary.RuntimeVersion"),
            *runtime,
            "case {runtime}: GetCanary must report the runtime version the canary was created on"
        );
    }

    for (index, (case, runtime)) in UNPUBLISHED.iter().enumerate() {
        assert!(
            !names.iter().any(|name| name == runtime),
            "case {case} ({runtime}): this row must not be a version the catalog reports"
        );
        let canary = format!("unreported-{index}");
        let mut body = create_body(&canary);
        body["RuntimeVersion"] = json!(runtime);
        let refused = aws.create(body).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({runtime}): expected HTTP 400, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case} ({runtime}): expected ValidationException, got {:?}",
            refused.error_code
        );
        let message = refused.at("Message").as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(runtime),
            "case {case} ({runtime}): expected the message to name the rejected version, got \
             {message:?}"
        );
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    Value::Object(
        parsed["shapes"]
            .as_object()
            .expect("a model document must carry a shapes object")
            .clone(),
    )
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types and
/// constraint traits — so comparing declarations has to ignore the one trait that carries no
/// behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_member_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if shape_id == LOCAL_SHAPE {
            assert!(
                snapshot[shape_id].is_null(),
                "case {shape_id}: the snapshot now declares this shape, so the local \
                 declaration should give way to the modelled one"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "case {shape_id}: the overlay restates a shape the snapshot no longer declares"
        );
        assert_eq!(
            declared["type"], modelled["type"],
            "case {shape_id}: expected the type the snapshot gives it ({}), got {}",
            modelled["type"], declared["type"]
        );

        // The traits the snapshot states for the shape itself — a constraint the boundary
        // validator enforces, the input/output marker — are what the overlay must restate
        // as written. A declaration whose only trait was documentation carries none here.
        let shape_traits = |declaration: &Value| -> Value {
            match without_docs(&declaration["traits"]) {
                Value::Object(traits) if traits.is_empty() => Value::Null,
                other => other,
            }
        };
        assert_eq!(
            shape_traits(declared),
            shape_traits(modelled),
            "case {shape_id}: expected the traits the snapshot states for it ({:?}), got {:?}",
            shape_traits(modelled),
            shape_traits(declared)
        );

        let modelled_members = modelled["members"].as_object().cloned().unwrap_or_default();
        let declared_members = declared["members"].as_object().cloned().unwrap_or_default();
        for (member, modelled_member) in &modelled_members {
            assert_eq!(
                without_docs(&declared_members[member]),
                without_docs(modelled_member),
                "case {shape_id}${member}: expected the declaration the snapshot gives it \
                 ({:?}), got {:?}",
                without_docs(modelled_member),
                declared_members.get(member)
            );
        }
        let added: Vec<&str> = declared_members
            .keys()
            .filter(|member| !modelled_members.contains_key(*member))
            .map(String::as_str)
            .collect();
        if shape_id == CATALOG_ENTRY_SHAPE {
            assert_eq!(
                added,
                vec![LOCAL_MEMBER],
                "case {shape_id}: expected exactly [{LOCAL_MEMBER:?}] added to the modelled \
                 members, got {added:?}"
            );
        } else {
            assert!(
                added.is_empty(),
                "case {shape_id}: this shape is restated only for the one above to target, so \
                 it may add no member; got {added:?}"
            );
        }
    }

    // The shape the local declaration is of is restated: left out of the overlay, the entry
    // would silently lose the member.
    assert!(
        restated.contains_key(CATALOG_ENTRY_SHAPE),
        "case {CATALOG_ENTRY_SHAPE}: the overlay must restate the shape the local declaration \
         is of, got {:?}",
        restated.keys().collect::<Vec<_>>()
    );
    assert!(
        snapshot[CATALOG_ENTRY_SHAPE]["members"]
            .as_object()
            .is_some_and(|members| !members.contains_key(LOCAL_MEMBER)),
        "case {CATALOG_ENTRY_SHAPE}: the snapshot now declares {LOCAL_MEMBER}, so the local \
         declaration should give way to the modelled member"
    );
}
