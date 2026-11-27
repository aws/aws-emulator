// SPDX-License-Identifier: CC0-1.0

//! A stored configuration reports the members the SERVICE supplies, over HTTP.
//!
//! The rule every case here asserts: a configuration member a request leaves out is not
//! unset on the resource — it carries the value CloudFront applies to it, and the create,
//! every later read, every listing's summary and every read-modify-write round trip report
//! that resolved configuration rather than the request's literal members. Two kinds of
//! member are resolved:
//!
//! * one with a DOCUMENTED default takes that default — `CachePolicyConfig$DefaultTTL`
//!   ("86400 seconds (one day). If the value of `MinTTL` is more than 86400 seconds, then
//!   the default value for this field is the same as the value of `MinTTL`"),
//!   `CachePolicyConfig$MaxTTL` ("31536000 seconds (one year). If the value of `MinTTL` or
//!   `DefaultTTL` is more than 31536000 seconds, then the default value for this field is
//!   the same as the value of `DefaultTTL`") and
//!   `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli` (the flag's off
//!   position, "If both of these fields are `false`, then CloudFront treats the
//!   `Accept-Encoding` header the same as any other HTTP header in the viewer request");
//! * one whose absence has exactly one meaning takes that meaning — an optional free-text
//!   comment is the empty text (`FieldLevelEncryptionConfig$Comment`,
//!   `FieldLevelEncryptionProfileConfig$Comment`, `PublicKeyConfig$Comment`), and
//!   `FieldLevelEncryptionConfig$QueryArgProfileConfig`, the mapping every stored
//!   configuration carries (an update of one cannot leave it out), is the mapping that maps
//!   nothing: a `QueryArgProfiles` wrapper stating `Quantity` 0, which forwards a request
//!   naming a profile it cannot resolve since it designates none;
//! * one the service DECIDES from the rest of the configuration takes that value whatever
//!   the request stated — `QueryArgProfileConfig$ForwardWhenQueryArgProfileIsUnknown` says of
//!   the stored configuration whether its mapping maps NO query argument to a profile
//!   (`field_level_encryption_query_arg_forwarding.rs` states that rule over its own table).
//!
//! Each table spans what the rule generalises over: which members the request states at all,
//! the magnitude of each TTL (zero, one, each documented bound and one past it), both
//! settings of each flag, and the comment forms (absent, empty, ASCII, non-ASCII, XML
//! metacharacters). Every row is asserted on the create answer, on each read of the
//! resource, on the listing's summary where the resource has one, and on the answer of an
//! update that leaves the same members out again — so the resolution is also shown to be
//! IDEMPOTENT.
//!
//! The member docs quoted above come from
//! `smithy_query(query="shape",shape="CachePolicyConfig,ParametersInCacheKeyAndForwardedToOrigin,FieldLevelEncryptionConfig,FieldLevelEncryptionProfileConfig,PublicKeyConfig,QueryArgProfileConfig")`.

use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{
    create_policy, get_policy, get_policy_config, Parameters, Policy, DOCUMENTED_DEFAULT_TTL,
    DOCUMENTED_MAX_TTL,
};
use crate::cache_policy_update::update_policy;
use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, Config, ContentTypeMapping, QueryArgMapping,
    Section,
};
use crate::field_level_encryption_config_list::list_configs;
use crate::field_level_encryption_config_update::update_config;
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, store_profile, upload_key, Entity, Profile,
};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};

// ---------------------------------------------------------------------------
// The calls this suite makes that its siblings do not expose
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/public-key/{Id}/config`.
async fn get_key_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/public-key/{Id}/config` with the `PublicKeyConfig` document `body`.
async fn update_key(router: &MultiServiceRouter, id: &str, body: &str, if_match: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// `GET /2020-05-31/public-key`.
async fn list_keys(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", &format!("/{API_VERSION}/public-key"), None, None).await
}

/// `PUT /2020-05-31/field-level-encryption-profile/{Id}/config` with the
/// `FieldLevelEncryptionProfileConfig` document `body`.
async fn update_profile(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption-profile`.
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

/// The summary of `id` in a listing's `Items`, by the element name the listed resource's
/// summary carries.
fn summary_of<'a>(page: &'a Answer, element: &str, id: &str, case: &str) -> &'a Node {
    page.root()
        .descendant("Items")
        .map(|items| items.children_named(element))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing carries no {element} for {id}: {}", page.raw)
        })
}

/// Assert that `node` reports `member` as `expected` — the one comparison every member of
/// every row below is checked by, so an absent element and an empty one are told apart.
fn assert_member(node: &Node, member: &str, expected: &str, case: &str, context: &str, raw: &str) {
    let actual = node.text_at(member).map(str::to_string);
    assert_eq!(
        actual.as_deref(),
        Some(expected),
        "case {case:?} ({context}): expected {member} {expected:?}, got {actual:?} in {raw}"
    );
}

// ---------------------------------------------------------------------------
// The cache policy's TTLs and its brotli flag
// ---------------------------------------------------------------------------

/// One cache policy row: what the request states, and nothing else — every expectation is
/// the documented rule applied to these values.
struct TtlRow {
    case: &'static str,
    name: &'static str,
    min_ttl: i64,
    default_ttl: Option<i64>,
    max_ttl: Option<i64>,
    brotli: Option<bool>,
}

impl TtlRow {
    /// This row as a `CachePolicyConfig` the sibling suite's one writer serialises.
    fn policy(&self) -> Policy {
        Policy {
            case: self.case,
            name: self.name.to_string(),
            comment: None,
            min_ttl: self.min_ttl,
            default_ttl: self.default_ttl,
            max_ttl: self.max_ttl,
            parameters: Some(Parameters {
                brotli: self.brotli,
                ..Parameters::default()
            }),
        }
    }

    /// The `DefaultTTL` the stored policy runs under, by the member's documented rule.
    fn expected_default_ttl(&self) -> i64 {
        self.default_ttl
            .unwrap_or_else(|| self.min_ttl.max(DOCUMENTED_DEFAULT_TTL))
    }

    /// The `MaxTTL` the stored policy runs under, by the member's documented rule.
    fn expected_max_ttl(&self) -> i64 {
        self.max_ttl
            .unwrap_or_else(|| self.expected_default_ttl().max(DOCUMENTED_MAX_TTL))
    }

    /// Assert one serialised `CachePolicyConfig` reports the three resolved members.
    fn assert_config(&self, config: &Node, context: &str, raw: &str) {
        assert_member(
            config,
            "MinTTL",
            &self.min_ttl.to_string(),
            self.case,
            context,
            raw,
        );
        assert_member(
            config,
            "DefaultTTL",
            &self.expected_default_ttl().to_string(),
            self.case,
            context,
            raw,
        );
        assert_member(
            config,
            "MaxTTL",
            &self.expected_max_ttl().to_string(),
            self.case,
            context,
            raw,
        );
        assert_member(
            config,
            "ParametersInCacheKeyAndForwardedToOrigin/EnableAcceptEncodingBrotli",
            &self.brotli.unwrap_or(false).to_string(),
            self.case,
            context,
            raw,
        );
    }
}

/// The cache policy rows: the TTL members stated and left out in every combination that
/// changes which documented default applies, at magnitudes spanning zero, one, each
/// documented bound and one past it, beside each setting of the optional brotli flag.
fn ttl_rows() -> Vec<TtlRow> {
    vec![
        TtlRow {
            case: "no TTL but the required minimum, and no brotli flag",
            name: "ttl-none",
            min_ttl: 0,
            default_ttl: None,
            max_ttl: None,
            brotli: None,
        },
        TtlRow {
            case: "a minimum of one second, the flag off",
            name: "ttl-one",
            min_ttl: 1,
            default_ttl: None,
            max_ttl: None,
            brotli: Some(false),
        },
        TtlRow {
            case: "a minimum exactly at the documented day, the flag on",
            name: "ttl-at-the-day",
            min_ttl: DOCUMENTED_DEFAULT_TTL,
            default_ttl: None,
            max_ttl: None,
            brotli: Some(true),
        },
        TtlRow {
            case: "a minimum one second past the documented day, so the default follows it",
            name: "ttl-past-the-day",
            min_ttl: DOCUMENTED_DEFAULT_TTL + 1,
            default_ttl: None,
            max_ttl: None,
            brotli: None,
        },
        TtlRow {
            case: "a minimum past the documented year, so both defaults follow it",
            name: "ttl-past-the-year",
            min_ttl: DOCUMENTED_MAX_TTL + 1,
            default_ttl: None,
            max_ttl: None,
            brotli: None,
        },
        TtlRow {
            case: "a default past the documented year, so the maximum follows the default",
            name: "ttl-default-past-the-year",
            min_ttl: 0,
            default_ttl: Some(DOCUMENTED_MAX_TTL + 2),
            max_ttl: None,
            brotli: Some(false),
        },
        TtlRow {
            case: "a default stated below the documented day, the maximum left out",
            name: "ttl-small-default",
            min_ttl: 0,
            default_ttl: Some(30),
            max_ttl: None,
            brotli: None,
        },
        TtlRow {
            case: "a maximum stated, the default left out",
            name: "ttl-max-only",
            min_ttl: 60,
            default_ttl: None,
            max_ttl: Some(DOCUMENTED_MAX_TTL),
            brotli: Some(true),
        },
        TtlRow {
            case: "every TTL and the flag stated, so nothing is supplied",
            name: "ttl-all-stated",
            min_ttl: 10,
            default_ttl: Some(20),
            max_ttl: Some(30),
            brotli: Some(true),
        },
        TtlRow {
            case: "every TTL stated and the flag left out, so only the flag is supplied",
            name: "ttl-stated-no-flag",
            min_ttl: 0,
            default_ttl: Some(DOCUMENTED_DEFAULT_TTL),
            max_ttl: Some(DOCUMENTED_MAX_TTL),
            brotli: None,
        },
    ]
}

/// A cache policy reports `DefaultTTL`, `MaxTTL` and `EnableAcceptEncodingBrotli` whether or
/// not the request stated them: the create, both reads and an update that states them no
/// more all answer with the values the service applies.
#[tokio::test]
async fn a_cache_policy_reports_the_ttls_and_the_brotli_flag_the_service_supplies() {
    let router = emulator();

    for row in ttl_rows() {
        let policy = row.policy();
        let created = create_policy(&router, &policy.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
            row.case, created.status, created.raw
        );
        let config = created
            .root()
            .descendant("CachePolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the create answers with the configuration: {}",
                    row.case, created.raw
                )
            });
        row.assert_config(config, "CreateCachePolicy", &created.raw);

        let id = created
            .root()
            .find_text("Id")
            .unwrap_or_default()
            .to_string();
        let etag = created.header("etag").to_string();

        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetCachePolicy, got {} with {}",
            row.case, read.status, read.raw
        );
        row.assert_config(
            read.root()
                .descendant("CachePolicyConfig")
                .unwrap_or_else(|| panic!("case {:?}: no configuration in {}", row.case, read.raw)),
            "GetCachePolicy",
            &read.raw,
        );

        let read_config = get_policy_config(&router, &id).await;
        assert_eq!(
            read_config.status, 200,
            "case {:?}: expected HTTP 200 from GetCachePolicyConfig, got {} with {}",
            row.case, read_config.status, read_config.raw
        );
        row.assert_config(
            read_config.root(),
            "GetCachePolicyConfig",
            &read_config.raw,
        );

        // The update resubmits the SAME document, which states those members no more than
        // the create did: the resolution is applied once more to the submitted payload
        // rather than to the already-resolved record, so the answer is the same.
        let updated = update_policy(&router, &id, &policy.xml(), Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateCachePolicy, got {} with {}",
            row.case, updated.status, updated.raw
        );
        row.assert_config(
            updated
                .root()
                .descendant("CachePolicyConfig")
                .unwrap_or_else(|| {
                    panic!("case {:?}: no configuration in {}", row.case, updated.raw)
                }),
            "UpdateCachePolicy",
            &updated.raw,
        );

        let after = get_policy_config(&router, &id).await;
        row.assert_config(
            after.root(),
            "GetCachePolicyConfig after the update",
            &after.raw,
        );
    }
}

// ---------------------------------------------------------------------------
// The field-level encryption configuration's comment and query argument mapping
// ---------------------------------------------------------------------------

/// What one field-level encryption configuration row states, and what the mapping the
/// service supplies for it looks like.
struct FleRow {
    case: &'static str,
    reference: &'static str,
    /// `Comment`, or `None` to leave the member out of the document.
    comment: Option<&'static str>,
    /// The forwarding flag of the `QueryArgProfileConfig` this request states, or `None` to
    /// leave the whole mapping out of the document.
    query_arg_flag: Option<bool>,
    /// The mappings that section's `QueryArgProfiles` wrapper carries, or `None` to leave
    /// the wrapper out of the stated section. Read only when `query_arg_flag` is `Some`.
    query_arg_mappings: Option<usize>,
}

impl FleRow {
    /// This row as the `FieldLevelEncryptionConfig` the sibling suite's one writer
    /// serialises, with a content type mapping naming `profile` so the configuration
    /// encrypts something whatever its query argument section says.
    fn config(&self, profile: &str) -> Config {
        let query_arg = self.query_arg_flag.map(|flag| {
            let mappings = self.query_arg_mappings.map(|count| {
                (0..count)
                    .map(|index| {
                        QueryArgMapping::new(&format!("fle-profile-{index}"), profile)
                    })
                    .collect()
            });
            Section::new(flag, mappings)
        });
        Config::new(
            self.case,
            self.reference,
            self.comment,
            query_arg,
            Some(Section::new(
                true,
                Some(vec![ContentTypeMapping::new(
                    "application/x-www-form-urlencoded",
                    Some(profile),
                )]),
            )),
        )
    }

    /// The `Comment` the stored configuration carries.
    fn expected_comment(&self) -> String {
        self.comment.unwrap_or_default().to_string()
    }

    /// The `ForwardWhenQueryArgProfileIsUnknown` the stored configuration carries: whether
    /// its mapping maps NO query argument to a profile — the value the stored configuration
    /// answers from its own mapping rather than from the request
    /// (`field_level_encryption_query_arg_forwarding.rs` states that rule), which for a
    /// configuration that states no section at all is the mapping that maps nothing.
    fn expected_flag(&self) -> bool {
        match (self.query_arg_flag, self.query_arg_mappings) {
            (None, _) => true,
            (Some(_), Some(mapped)) => mapped == 0,
            (Some(_), None) => true,
        }
    }

    /// How many mappings the stored `QueryArgProfiles` wrapper carries, or `None` when the
    /// stored configuration carries no wrapper: a section the request STATED is stored as
    /// stated, down to the wrapper it left out, while the mapping the service supplies for
    /// a configuration that states no section at all carries the empty wrapper.
    fn expected_wrapper(&self) -> Option<usize> {
        match self.query_arg_flag {
            None => Some(0),
            Some(_) => self.query_arg_mappings,
        }
    }

    /// Assert one serialised document — a whole `FieldLevelEncryptionConfig` or a
    /// `FieldLevelEncryptionSummary`, which repeats the same two members — reports the
    /// comment and the mapping the stored configuration carries.
    fn assert_members(&self, node: &Node, profile: &str, context: &str, raw: &str) {
        assert_member(
            node,
            "Comment",
            &self.expected_comment(),
            self.case,
            context,
            raw,
        );
        assert_member(
            node,
            "QueryArgProfileConfig/ForwardWhenQueryArgProfileIsUnknown",
            &self.expected_flag().to_string(),
            self.case,
            context,
            raw,
        );
        let Some(count) = self.expected_wrapper() else {
            assert!(
                node.at("QueryArgProfileConfig/QueryArgProfiles").is_none(),
                "case {:?} ({context}): expected no QueryArgProfiles wrapper, got {raw}",
                self.case
            );
            return;
        };
        assert_member(
            node,
            "QueryArgProfileConfig/QueryArgProfiles/Quantity",
            &count.to_string(),
            self.case,
            context,
            raw,
        );
        let mappings: Vec<&Node> = node
            .at("QueryArgProfileConfig/QueryArgProfiles/Items")
            .map(|items| items.children_named("QueryArgProfile"))
            .unwrap_or_default();
        assert_eq!(
            mappings.len(),
            count,
            "case {:?} ({context}): expected {count} QueryArgProfile entries, got {} in {raw}",
            self.case,
            mappings.len()
        );
        for mapping in mappings {
            assert_member(mapping, "ProfileId", profile, self.case, context, raw);
        }
    }
}

/// The field-level encryption configuration rows: the comment stated in each of its forms
/// or not at all, crossed with a query argument section left out entirely, stated with no
/// mapping wrapper, stated with an empty wrapper and stated with one mapping, under each
/// setting of its forwarding flag.
fn fle_rows() -> Vec<FleRow> {
    vec![
        FleRow {
            case: "neither a comment nor a query argument mapping",
            reference: "supplied-fle-bare",
            comment: None,
            query_arg_flag: None,
            query_arg_mappings: None,
        },
        FleRow {
            case: "an empty comment, no query argument mapping",
            reference: "supplied-fle-empty-comment",
            comment: Some(""),
            query_arg_flag: None,
            query_arg_mappings: None,
        },
        FleRow {
            case: "an ASCII comment, no query argument mapping",
            reference: "supplied-fle-ascii",
            comment: Some("the configuration the tenant runs under"),
            query_arg_flag: None,
            query_arg_mappings: None,
        },
        FleRow {
            case: "a non-ASCII comment with XML metacharacters, no query argument mapping",
            reference: "supplied-fle-non-ascii",
            comment: Some("プロダクション <a> & \"b\""),
            query_arg_flag: None,
            query_arg_mappings: None,
        },
        FleRow {
            case: "a mapping section stated with no wrapper, forwarding on",
            reference: "supplied-fle-section-no-wrapper",
            comment: None,
            query_arg_flag: Some(true),
            query_arg_mappings: None,
        },
        FleRow {
            case: "a mapping section stated with an empty wrapper, forwarding off",
            reference: "supplied-fle-section-empty-wrapper",
            comment: Some("no mapping"),
            query_arg_flag: Some(false),
            query_arg_mappings: Some(0),
        },
        FleRow {
            case: "a mapping section naming the profile, forwarding off",
            reference: "supplied-fle-one-mapping",
            comment: None,
            query_arg_flag: Some(false),
            query_arg_mappings: Some(1),
        },
        FleRow {
            case: "a mapping section naming the profile twice, forwarding on",
            reference: "supplied-fle-two-mappings",
            comment: Some("two mappings"),
            query_arg_flag: Some(true),
            query_arg_mappings: Some(2),
        },
    ]
}

/// A field-level encryption configuration reports `Comment` and `QueryArgProfileConfig`
/// whether or not the request stated them: the create, both reads, the listing's summary and
/// an update that states them no more all answer with what the service applies.
#[tokio::test]
async fn a_field_level_encryption_configuration_reports_the_comment_and_mapping_supplied() {
    let router = emulator();
    let profile = store_profile(&router, "supplied-fle-profile").await;

    for row in fle_rows() {
        let config = row.config(&profile);
        let created = create_config(&router, &config.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionConfig, got {} with {}",
            row.case, created.status, created.raw
        );
        let document = created
            .root()
            .descendant("FieldLevelEncryptionConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the create answers with the configuration: {}",
                    row.case, created.raw
                )
            });
        row.assert_members(
            document,
            &profile,
            "CreateFieldLevelEncryptionConfig",
            &created.raw,
        );

        let id = created.root().find_text("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        let read = get_config(&router, &id).await;
        row.assert_members(
            read.root()
                .descendant("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| panic!("case {:?}: no configuration in {}", row.case, read.raw)),
            &profile,
            "GetFieldLevelEncryption",
            &read.raw,
        );

        let read_config = get_config_document(&router, &id).await;
        row.assert_members(
            read_config.root(),
            &profile,
            "GetFieldLevelEncryptionConfig",
            &read_config.raw,
        );

        let page = list_configs(&router, "").await;
        row.assert_members(
            summary_of(&page, "FieldLevelEncryptionSummary", &id, row.case),
            &profile,
            "ListFieldLevelEncryptionConfigs",
            &page.raw,
        );

        // An update states a `QueryArgProfileConfig` on every path ("QueryArgProfileConfig
        // is required for updates"), so the update resubmits the document carrying the
        // section the service resolved into the record — for a row that stated none, the
        // flag beside the empty wrapper — and still states no comment of its own.
        let mut updated_row = row;
        updated_row.query_arg_mappings = updated_row.expected_wrapper();
        updated_row.query_arg_flag = Some(updated_row.expected_flag());
        let updated =
            update_config(&router, &id, &updated_row.config(&profile).xml(), Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateFieldLevelEncryptionConfig, got {} with {}",
            updated_row.case, updated.status, updated.raw
        );
        updated_row.assert_members(
            updated
                .root()
                .descendant("FieldLevelEncryptionConfig")
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: no configuration in {}",
                        updated_row.case, updated.raw
                    )
                }),
            &profile,
            "UpdateFieldLevelEncryptionConfig",
            &updated.raw,
        );
    }
}

// ---------------------------------------------------------------------------
// The field-level encryption profile's comment
// ---------------------------------------------------------------------------

/// The comment forms every row of the two comment tables below is run over: absent, empty,
/// ASCII, non-ASCII and one carrying XML metacharacters.
const COMMENT_FORMS: [(&str, Option<&str>); 5] = [
    ("no comment at all", None),
    ("an empty comment", Some("")),
    ("an ASCII comment", Some("the key the tenant signs with")),
    ("a non-ASCII comment", Some("プロダクション — 署名鍵")),
    ("a comment carrying XML metacharacters", Some("a <b> & \"c\"")),
];

/// A field-level encryption profile reports `Comment` whether or not the request stated it:
/// the create, both reads, the listing's summary and an update that states it no more all
/// answer with the comment the service applies.
#[tokio::test]
async fn a_field_level_encryption_profile_reports_the_comment_supplied() {
    let router = emulator();
    let key = upload_key(&router, 0).await;

    for (index, (case, comment)) in COMMENT_FORMS.into_iter().enumerate() {
        let reference = format!("supplied-profile-{index}");
        let row = Profile::new(
            case,
            &reference,
            comment,
            vec![Entity::new(&key, &format!("provider-{index}"), &["Card"])],
        );
        let expected = comment.unwrap_or_default();

        let created = create_profile(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateFieldLevelEncryptionProfile, got {} \
             with {}",
            created.status, created.raw
        );
        assert_member(
            created
                .root()
                .descendant("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no configuration in {}", created.raw)
                }),
            "Comment",
            expected,
            case,
            "CreateFieldLevelEncryptionProfile",
            &created.raw,
        );

        let id = created.root().find_text("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        let read = get_profile(&router, &id).await;
        assert_member(
            read.root()
                .descendant("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no configuration in {}", read.raw)),
            "Comment",
            expected,
            case,
            "GetFieldLevelEncryptionProfile",
            &read.raw,
        );

        let read_config = get_profile_config(&router, &id).await;
        assert_member(
            read_config.root(),
            "Comment",
            expected,
            case,
            "GetFieldLevelEncryptionProfileConfig",
            &read_config.raw,
        );

        let page = list_profiles(&router).await;
        assert_member(
            summary_of(&page, "FieldLevelEncryptionProfileSummary", &id, case),
            "Comment",
            expected,
            case,
            "ListFieldLevelEncryptionProfiles",
            &page.raw,
        );

        let updated = update_profile(&router, &id, &row.xml(), &etag).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateFieldLevelEncryptionProfile, got {} \
             with {}",
            updated.status, updated.raw
        );
        assert_member(
            updated
                .root()
                .descendant("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no configuration in {}", updated.raw)
                }),
            "Comment",
            expected,
            case,
            "UpdateFieldLevelEncryptionProfile",
            &updated.raw,
        );
    }
}

// ---------------------------------------------------------------------------
// The public key's comment
// ---------------------------------------------------------------------------

/// A public key reports `Comment` whether or not the request stated it: the create, both
/// reads, the listing's summary and an update that states it no more all answer with the
/// comment the service applies.
#[tokio::test]
async fn a_public_key_reports_the_comment_supplied() {
    let router = emulator();

    for (index, (case, comment)) in COMMENT_FORMS.into_iter().enumerate() {
        let row = Key {
            case: case.to_string(),
            caller_reference: format!("supplied-key-{index}"),
            name: format!("supplied-key-{index}"),
            encoded_key: pem(1 + index, 'K'),
            comment: comment.map(str::to_string),
        };
        let expected = comment.unwrap_or_default();

        let created = create_key(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreatePublicKey, got {} with {}",
            created.status, created.raw
        );
        assert_member(
            created
                .root()
                .descendant("PublicKeyConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no configuration in {}", created.raw)),
            "Comment",
            expected,
            case,
            "CreatePublicKey",
            &created.raw,
        );

        let id = created.root().find_text("Id").unwrap_or_default().to_string();
        let etag = created.header("etag").to_string();

        let read = get_key(&router, &id).await;
        assert_member(
            read.root()
                .descendant("PublicKeyConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no configuration in {}", read.raw)),
            "Comment",
            expected,
            case,
            "GetPublicKey",
            &read.raw,
        );

        let read_config = get_key_config(&router, &id).await;
        assert_member(
            read_config.root(),
            "Comment",
            expected,
            case,
            "GetPublicKeyConfig",
            &read_config.raw,
        );

        let page = list_keys(&router).await;
        assert_member(
            summary_of(&page, "PublicKeySummary", &id, case),
            "Comment",
            expected,
            case,
            "ListPublicKeys",
            &page.raw,
        );

        let updated = update_key(&router, &id, &row.xml(), &etag).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdatePublicKey, got {} with {}",
            updated.status, updated.raw
        );
        assert_member(
            updated
                .root()
                .descendant("PublicKeyConfig")
                .unwrap_or_else(|| panic!("case {case:?}: no configuration in {}", updated.raw)),
            "Comment",
            expected,
            case,
            "UpdatePublicKey",
            &updated.raw,
        );
    }
}
