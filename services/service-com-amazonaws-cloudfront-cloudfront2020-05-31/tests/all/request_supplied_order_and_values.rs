// SPDX-License-Identifier: CC0-1.0

//! What a request SUPPLIES is what every later answer reports — same members, same values,
//! same sequence — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test is ONE rule, read off the model
//! (`smithy_query(query="shape",shape="Paths,PathList,FieldPatterns,FieldPatternList,FieldLevelEncryptionConfig,QueryArgProfileConfig,ContentTypeProfileConfig")`):
//!
//! * a member documented as carrying the submitted document — `Invalidation$InvalidationBatch`
//!   ("The current invalidation information for the batch request"),
//!   `FieldLevelEncryptionProfile$FieldLevelEncryptionProfileConfig`,
//!   `FieldLevelEncryption$FieldLevelEncryptionConfig`, and the members a listing's summary
//!   repeats — reports the document the request supplied and nothing else: a REQUIRED member
//!   carries the value submitted for it, and an OPTIONAL member the request left out stays
//!   absent rather than being filled in by the read, save for the members the SERVICE itself
//!   supplies for a configuration that states none (`server_supplied_config_members.rs`
//!   states that rule, and `field_level_encryption_query_arg_forwarding.rs` the one member a
//!   field-level encryption configuration answers from its own mapping);
//! * `Paths$Items` (`PathList`), `FieldPatterns$Items` (`FieldPatternList`) and
//!   `EncryptionEntities$Items` (`EncryptionEntityList`) are Smithy LISTS, i.e. ordered
//!   sequences, so the SEQUENCE is part of the value: the entries are reported at the
//!   positions the request put them in, on the create and on every later read or listing — an
//!   invalidation batch being the SET of objects to invalidate, where an exact repeat of a
//!   path collapses onto its first position (`invalidation_path_set.rs` states that rule).
//!
//! Neither half of the rule is a property of one request document's FORM, so each suite below
//! drives it from a table whose rows differ along what the rule generalises over: how many
//! entries a list carries (one, two, three, five), whether the submitted sequence is in
//! ascending order, descending order or neither, whether an entry repeats, how many entities
//! carry those lists, which optional sections a configuration carries at all (both, one, the
//! other, neither), and which value each required flag is submitted with (both settings).
//!
//! The harness (the router, the request helpers and the element scanner), the profile rows and
//! the configuration rows are the sibling suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    batch_xml, call, config, create_distribution, create_invalidation, emulator, get_invalidation,
    Answer, Node, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_list::list_configs;
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, store_profile, upload_key, Entity, Profile,
};

/// The one `Format` the model's `Format` enum admits for a content type-profile mapping
/// (`smithy_query(query="shape",shape="Format")` lists `URLEncoded` and nothing else).
const URL_ENCODED: &str = "URLEncoded";

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The text of every `member` element under the element at `path`, in serialised order.
fn sequence_at(node: &Node, path: &str, member: &str) -> Vec<String> {
    node.at(path)
        .map(|items| {
            items
                .children_named(member)
                .into_iter()
                .map(|child| child.text.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /2020-05-31/field-level-encryption-profile`, from the operation's `http` binding.
async fn list_profiles(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption-profile"),
        None,
        None,
    )
    .await
}

/// The `member` summary a listing page carries for the resource whose `Id` is `id`.
fn summary_of<'a>(page: &'a Answer, member: &str, id: &str) -> &'a Node {
    page.root()
        .at("Items")
        .map(|items| items.children_named(member))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id))
        .unwrap_or_else(|| panic!("no {member} for {id} in {}", page.raw))
}

// ---------------------------------------------------------------------------
// An invalidation batch's paths
// ---------------------------------------------------------------------------

/// `InvalidationBatch$Paths$Items` is a `PathList` — an ordered sequence — and the
/// `Invalidation` every invalidation operation reports carries "the current invalidation
/// information for the batch request", so the paths come back at the positions the request put
/// them in, both from the create and from a later read.
#[tokio::test]
async fn an_invalidation_reports_its_paths_in_the_order_submitted() {
    let router = emulator();
    let created = create_distribution(&router, &config("order", "ref-order").xml()).await;
    let distribution = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    // Rows differing in how many paths a batch names and in where the submitted sequence sits
    // relative to a sorted one — descending, ascending, neither — plus a repeated entry, which
    // the SET a batch describes keeps once, at the position the request first named it
    // (`invalidation_path_set.rs` asserts that rule over the class).
    let rows: &[(&str, &[&str])] = &[
        ("one path", &["/*"]),
        (
            "two paths, submitted descending",
            &["/index.html", "/images/*"],
        ),
        (
            "two paths, submitted ascending",
            &["/images/*", "/index.html"],
        ),
        (
            "three paths, the middle one first",
            &["/b.html", "/a.html", "/c.html"],
        ),
        ("five paths in no order at all", &["/e", "/a", "/d", "/c", "/b"]),
        (
            "a repeated path",
            &["/dup.html", "/other.html", "/dup.html"],
        ),
    ];

    for (case, paths) in rows {
        let reference = format!("inv-{}", case.replace(' ', "-"));
        let answer =
            create_invalidation(&router, &distribution, &batch_xml(&reference, paths)).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        // The batch is the SET of objects to invalidate, so the paths come back in the order
        // submitted with an exact repeat collapsed onto its first position.
        let mut expected: Vec<String> = Vec::new();
        for path in *paths {
            let path = (*path).to_string();
            if !expected.contains(&path) {
                expected.push(path);
            }
        }
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        let read = get_invalidation(&router, &distribution, &id).await;

        for (context, answer) in [("CreateInvalidation", &answer), ("GetInvalidation", &read)] {
            let document = answer.root();
            let actual = sequence_at(document, "InvalidationBatch/Paths/Items", "Path");
            assert_eq!(
                actual, expected,
                "case {case:?} ({context}): expected paths {expected:?}, got {actual:?} in {}",
                answer.raw
            );
            let quantity = document.text_at("InvalidationBatch/Paths/Quantity");
            assert_eq!(
                quantity,
                Some(expected.len().to_string().as_str()),
                "case {case:?} ({context}): expected Paths.Quantity {}, got {quantity:?} in {}",
                expected.len(),
                answer.raw
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A profile's encryption entities and field patterns
// ---------------------------------------------------------------------------

/// `FieldPatterns$Items` is a `FieldPatternList` and `EncryptionEntities$Items` an
/// `EncryptionEntityList` — ordered sequences both — so a profile reports its entities, and
/// each entity its field patterns, at the positions the request put them in: from the create,
/// from either read of the profile, and from the summary a listing carries.
#[tokio::test]
async fn a_profile_reports_its_field_patterns_in_the_order_submitted() {
    let router = emulator();
    let keys: Vec<String> = vec![
        upload_key(&router, 0).await,
        upload_key(&router, 1).await,
        upload_key(&router, 2).await,
    ];

    // Rows differing in how many entities carry the patterns, how many patterns an entity
    // carries, and where the submitted sequence sits relative to a sorted one.
    let rows: Vec<Profile> = vec![
        Profile::new(
            "one entity, two patterns submitted descending",
            "ref-order-descending",
            None,
            vec![Entity::new(
                &keys[0],
                "provider-one",
                &["ssn", "credit-card"],
            )],
        ),
        Profile::new(
            "one entity, two patterns submitted ascending",
            "ref-order-ascending",
            None,
            vec![Entity::new(&keys[0], "provider-one", &["dob", "ssn"])],
        ),
        Profile::new(
            "one entity, three patterns in no order at all",
            "ref-order-three",
            Some("three patterns"),
            vec![Entity::new(
                &keys[1],
                "provider-two",
                &["ssn", "dob", "account-number"],
            )],
        ),
        Profile::new(
            "three entities, each with its own pattern order",
            "ref-order-entities",
            None,
            vec![
                Entity::new(&keys[2], "provider-three", &["ssn", "credit-card"]),
                Entity::new(&keys[0], "provider-one", &["dob"]),
                Entity::new(&keys[1], "provider-two", &["postcode", "account-number"]),
            ],
        ),
    ];

    for row in &rows {
        let case = row.case.as_str();
        let answer = create_profile(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();

        let read = get_profile(&router, &id).await;
        let read_config = get_profile_config(&router, &id).await;
        let page = list_profiles(&router).await;

        let documents: Vec<(&str, &Node, &str)> = vec![
            (
                "CreateFieldLevelEncryptionProfile",
                answer
                    .root()
                    .at("FieldLevelEncryptionProfileConfig")
                    .unwrap_or_else(|| panic!("case {case:?}: no config in {}", answer.raw)),
                answer.raw.as_str(),
            ),
            (
                "GetFieldLevelEncryptionProfile",
                read.root()
                    .at("FieldLevelEncryptionProfileConfig")
                    .unwrap_or_else(|| panic!("case {case:?}: no config in {}", read.raw)),
                read.raw.as_str(),
            ),
            (
                "GetFieldLevelEncryptionProfileConfig",
                read_config.root(),
                read_config.raw.as_str(),
            ),
            (
                "ListFieldLevelEncryptionProfiles",
                summary_of(&page, "FieldLevelEncryptionProfileSummary", &id),
                page.raw.as_str(),
            ),
        ];

        for (context, document, raw) in documents {
            let entities = document
                .at("EncryptionEntities/Items")
                .map(|items| items.children_named("EncryptionEntity"))
                .unwrap_or_default();
            assert_eq!(
                entities.len(),
                row.entities.len(),
                "case {case:?} ({context}): expected {} entities, got {} in {raw}",
                row.entities.len(),
                entities.len()
            );
            for (position, (expected, element)) in row.entities.iter().zip(entities).enumerate() {
                let provider = element.text_at("ProviderId").map(str::to_string);
                assert_eq!(
                    provider.as_deref(),
                    Some(expected.provider_id.as_str()),
                    "case {case:?} ({context}): expected entity {position} to be {:?}, got \
                     {provider:?} in {raw}",
                    expected.provider_id
                );
                let actual = sequence_at(element, "FieldPatterns/Items", "FieldPattern");
                assert_eq!(
                    actual, expected.field_patterns,
                    "case {case:?} ({context}): expected entity {position} patterns {:?}, got \
                     {actual:?} in {raw}",
                    expected.field_patterns
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A field-level encryption configuration's submitted values
// ---------------------------------------------------------------------------

/// `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` and
/// `ContentTypeProfileConfig$ForwardWhenContentTypeIsUnknown` are REQUIRED booleans, and both
/// sections are OPTIONAL on `FieldLevelEncryptionConfig`: so each stated section's flag and
/// its mappings are reported with the values the request submitted for them, and a section
/// the request left out is reported as the mapping the service supplies for it — on the
/// create, on both reads, and in the summary a listing carries.
#[tokio::test]
async fn a_configuration_reports_the_values_submitted() {
    let router = emulator();
    let profile = store_profile(&router, "ref-config-values-profile").await;

    let query_arg = |forward: bool| {
        Section::new(
            forward,
            Some(vec![QueryArgMapping::new("fle-profile", &profile)]),
        )
    };
    let content_type = |forward: bool, names_profile: bool| {
        Section::new(
            forward,
            Some(vec![ContentTypeMapping::new(
                "application/x-www-form-urlencoded",
                names_profile.then_some(profile.as_str()),
            )]),
        )
    };

    // Rows differing in which optional sections the configuration carries at all, in the value
    // each required flag is submitted with, and in whether a content type mapping names a
    // profile.
    let rows: Vec<Config> = vec![
        Config::new(
            "both sections, both flags false",
            "ref-values-false",
            Some("both sections"),
            Some(query_arg(false)),
            Some(content_type(false, true)),
        ),
        Config::new(
            "both sections, both flags true",
            "ref-values-true",
            None,
            Some(query_arg(true)),
            Some(content_type(true, true)),
        ),
        Config::new(
            "both sections, the flags disagreeing",
            "ref-values-mixed",
            None,
            Some(query_arg(false)),
            Some(content_type(true, false)),
        ),
        Config::new(
            "the query argument section only",
            "ref-values-query-only",
            None,
            Some(query_arg(false)),
            None,
        ),
        Config::new(
            "the content type section only",
            "ref-values-content-only",
            None,
            None,
            Some(content_type(false, true)),
        ),
    ];

    for row in &rows {
        let case = row.case.as_str();
        let answer = create_config(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();

        // The whole submitted document, as the create and both reads report it.
        row.assert_matches(
            answer
                .root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no config in {}", answer.raw)),
            "CreateFieldLevelEncryptionConfig",
            &answer.raw,
        );
        let read = get_config(&router, &id).await;
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no config in {}", read.raw)),
            "GetFieldLevelEncryption",
            &read.raw,
        );
        let read_config = get_config_document(&router, &id).await;
        row.assert_matches(
            read_config.root(),
            "GetFieldLevelEncryptionConfig",
            &read_config.raw,
        );

        // The summary repeats the same two sections beside the minted identity, so both flags
        // and the mappings are asserted there too — `CallerReference` is not a summary member,
        // which is why the summary goes through the assertions below rather than the whole
        // document's.
        let page = list_configs(&router, "").await;
        let summary = summary_of(&page, "FieldLevelEncryptionSummary", &id);
        assert_sections(
            row,
            summary,
            case,
            "ListFieldLevelEncryptionConfigs",
            &page.raw,
        );
    }
}

/// Assert that one serialised element reports each section as the stored configuration
/// carries it: `QueryArgProfileConfig` always — a configuration that submits none carries
/// the mapping that maps nothing — `ContentTypeProfileConfig` always too, the section the
/// request carried or the mapping the service supplies for a request that carried none, each
/// required flag at the value the request submitted for it, and each content type
/// mapping's members as submitted.
fn assert_sections(row: &Config, node: &Node, case: &str, context: &str, raw: &str) {
    let query_arg = row.effective_query_arg();
    let flag = node.text_at("QueryArgProfileConfig/ForwardWhenQueryArgProfileIsUnknown");
    assert_eq!(
        flag,
        Some(query_arg.forward_when_unknown.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenQueryArgProfileIsUnknown {}, got \
         {flag:?} in {raw}",
        query_arg.forward_when_unknown
    );

    let section = row.effective_content_type();
    let flag = node.text_at("ContentTypeProfileConfig/ForwardWhenContentTypeIsUnknown");
    assert_eq!(
        flag,
        Some(section.forward_when_unknown.to_string().as_str()),
        "case {case:?} ({context}): expected ForwardWhenContentTypeIsUnknown {}, got \
         {flag:?} in {raw}",
        section.forward_when_unknown
    );
    let mappings = section.mappings.as_deref().unwrap_or_default();
    let elements = node
        .at("ContentTypeProfileConfig/ContentTypeProfiles/Items")
        .map(|items| items.children_named("ContentTypeProfile"))
        .unwrap_or_default();
    assert_eq!(
        elements.len(),
        mappings.len(),
        "case {case:?} ({context}): expected {} ContentTypeProfile entries, got {} in {raw}",
        mappings.len(),
        elements.len()
    );
    for (expected, element) in mappings.iter().zip(elements) {
        for (member, want) in [
            ("Format", Some(URL_ENCODED.to_string())),
            ("ContentType", Some(expected.content_type.clone())),
            // An optional member the create left out stays absent.
            ("ProfileId", expected.profile_id.clone()),
        ] {
            let actual = element.text_at(member).map(str::to_string);
            assert_eq!(
                actual, want,
                "case {case:?} ({context}): expected ContentTypeProfile {member} \
                 {want:?}, got {actual:?} in {raw}"
            );
        }
    }
}
