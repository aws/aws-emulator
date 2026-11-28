// SPDX-License-Identifier: CC0-1.0

//! An identifier this account really holds for ANOTHER resource kind, submitted where a
//! field-level encryption profile's belongs, end to end over HTTP.
//!
//! The rule under test: an operation addressed by a field-level encryption profile `Id` decides
//! from the VALUE ALONE, before it consults the store, whether the value could ever be an
//! identifier CloudFront issued for a profile. These routes model one not-found code and no
//! argument error
//! (`smithy_query(query="operation",operation="GetFieldLevelEncryptionProfile,GetFieldLevelEncryptionProfileConfig,UpdateFieldLevelEncryptionProfile,DeleteFieldLevelEncryptionProfile")`
//! reports `NoSuchFieldLevelEncryptionProfile` and `AccessDenied` and nothing else, over an `Id`
//! that is a plain `string` `@httpLabel` carrying no `@pattern` and no `@length`), so the
//! MESSAGE is the whole of what tells the two conditions apart:
//!
//! * a value written exactly the way an identifier is ISSUED — the identifier alphabet (decimal
//!   digits and uppercase letters, no separator and no lower case) at the width this service
//!   issues one at — under the NAMESPACE LETTER of another kind this service issues identifiers
//!   under IS that other kind's identifier, so no profile could ever answer to it and the route
//!   reports the identifier it cannot READ ("Id format is invalid"). That the account really
//!   holds a live resource of that other kind under the value changes nothing: the refusal is a
//!   property of the value, not of the store;
//! * every other value is carried through to the store — one under the profile kind's OWN
//!   letter, since WHICH profile identifier was ever issued is a property of the store, one
//!   under a letter no kind of this service issues an identifier under, and one of the alphabet
//!   at any width OTHER than the issued one, on either side of it, since that width is one width
//!   CloudFront writes an identifier in
//!   rather than the only one — and the profile the store holds none under is the profile that
//!   is simply ABSENT ("The specified field level encryption profile does not exist.").
//!
//! The rows vary what the rule generalises over:
//!
//! * the KIND the submitted identifier was really issued for: a public key, a field-level
//!   encryption configuration, a distribution and an origin access control, each created
//!   through its OWN API in this account and each confirmed LIVE through its own reader first,
//!   so every crossed value here is a working identifier of another kind rather than a value
//!   made up to look like one;
//! * the ROUTE addressed by a profile identifier: both of the profile's readers, its update and
//!   its delete — the update and the delete carrying a live profile's own version token in
//!   `If-Match`, so no refusal below can be the precondition's;
//! * the NAMESPACE LETTER and the WIDTH of values that are NOT another kind's identifier: the
//!   profile's own letter over a body never minted, a letter no kind of this service issues one
//!   under, and another kind's letter one character short of and one character past the issued
//!   width — each the profile reported ABSENT, which is what keeps the two sentences apart
//!   rather than making the refusal cover every value — against another kind's letter one
//!   character PAST that width, which is unreadable.
//!
//! The issued width is read off the identifiers these creates really answered with, so no width
//! is written out by hand. Afterwards every live resource is read back at its own identifier and
//! the live profile is served at its own, so none of these reporting calls changed the store.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, get_distribution, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{create_config, get_config, simple};
use crate::field_level_encryption_profile_lifecycle::{
    create_profile, get_profile, get_profile_config, upload_key, Entity, Profile,
};
use crate::origin_access_control_lifecycle::{create_control, get_control, table};
use crate::public_key_lifecycle::get_key;

/// The sentence a route addressed by a profile identifier answers a value that could never be
/// one, spelled out here rather than taken from the crate, so a reworded refusal fails this
/// suite.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The sentence the same code carries for a value the route carries through to the store and the
/// store holds no profile under — the OTHER condition this one code covers.
const NO_SUCH_PROFILE: &str = "The specified field level encryption profile does not exist.";

/// The modelled error code every refusal below carries (HTTP 404).
const NO_SUCH_PROFILE_CODE: &str = "NoSuchFieldLevelEncryptionProfile";

/// A namespace letter no kind of this service issues an identifier under, so a value carrying it
/// is in the identifier form without being any other kind's identifier.
const UNISSUED_NAMESPACE: char = 'W';

/// One route addressed by a field-level encryption profile `Id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// `GET /2020-05-31/field-level-encryption-profile/{Id}`.
    Read,
    /// `GET /2020-05-31/field-level-encryption-profile/{Id}/config`.
    ReadConfig,
    /// `PUT /2020-05-31/field-level-encryption-profile/{Id}/config`.
    Update,
    /// `DELETE /2020-05-31/field-level-encryption-profile/{Id}`.
    Delete,
}

/// Every operation this service addresses by a profile identifier.
const ROUTES: [Route; 4] = [
    Route::Read,
    Route::ReadConfig,
    Route::Update,
    Route::Delete,
];

impl Route {
    /// Send this route at `id`. The two mutations carry `if_match` — a live profile's own
    /// version token — and the update the profile configuration `document`, so what they refuse
    /// is the identifier rather than a missing precondition or an unreadable body.
    async fn send(
        self,
        router: &MultiServiceRouter,
        id: &str,
        document: &str,
        if_match: &str,
    ) -> Answer {
        match self {
            Self::Read => get_profile(router, id).await,
            Self::ReadConfig => get_profile_config(router, id).await,
            Self::Update => {
                call(
                    router,
                    "PUT",
                    &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
                    Some(document),
                    Some(if_match),
                )
                .await
            }
            Self::Delete => {
                call(
                    router,
                    "DELETE",
                    &format!("/{API_VERSION}/field-level-encryption-profile/{id}"),
                    None,
                    Some(if_match),
                )
                .await
            }
        }
    }
}

/// One resource of another kind this account really holds: the identifier it was issued, and the
/// reader that confirms the identifier resolves for its OWN kind.
struct Live {
    /// What this row is a case of, named in every assertion it drives.
    case: &'static str,
    id: String,
}

/// Assert one refusal: its status, its code and — the point of this suite — its message.
fn assert_refusal(answer: &Answer, case: &str, message: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        NO_SUCH_PROFILE_CODE,
        "case {case}: expected code {NO_SUCH_PROFILE_CODE}, got {}",
        answer.raw
    );
    let actual = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case}: expected the sentence {message:?}, got {actual:?}"
    );
}

/// Assert a reader served the resource at its own identifier.
fn assert_served(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
}

/// RULE: every route addressed by a field-level encryption profile `Id` reports the identifier it
/// cannot read for a value issued as ANOTHER kind's identifier — whatever that kind is, and even
/// while the account really holds that resource — and keeps the absent-profile sentence for every
/// value it carries through to the store.
#[tokio::test]
async fn an_identifier_live_for_another_kind_is_refused_on_its_form_by_every_profile_route() {
    let router = emulator();

    // One public key, which a profile's encryption entities need anyway, and the live profile
    // whose version token the mutations below carry.
    let key = upload_key(&router, 0).await;
    let created_profile = create_profile(
        &router,
        &Profile::new(
            "the profile these rows cross",
            "fle-profile-crossed-live",
            None,
            vec![Entity::new(&key, "provider-one", &["CreditCard"])],
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_profile.status, 201,
        "case creating the profile: expected HTTP 201, got {} with {}",
        created_profile.status, created_profile.raw
    );
    let profile_id = created_profile
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let served_profile = get_profile(&router, &profile_id).await;
    assert_served(&served_profile, "the profile at its own identifier");
    let if_match = served_profile.header("ETag").to_string();
    assert!(
        !if_match.is_empty(),
        "case the profile at its own identifier: expected a version token, got {:?}",
        served_profile.raw
    );

    // The document the update submits. Well-formed, so nothing it carries can be the reason an
    // update below is refused.
    let document = Profile::new(
        "the update these rows cross",
        "fle-profile-crossed-update",
        None,
        vec![Entity::new(&key, "provider-two", &["CreditCard"])],
    )
    .xml();

    // One field-level encryption configuration naming that profile, one distribution and one
    // origin access control — each created through its own API.
    let created_config = create_config(
        &router,
        &simple(
            "the configuration these rows cross",
            "fle-config-crossed-live",
            &profile_id,
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_config.status, 201,
        "case creating the configuration: expected HTTP 201, got {} with {}",
        created_config.status, created_config.raw
    );
    let created_distribution = create_distribution(
        &router,
        &config(
            "the distribution these rows cross",
            "distribution-crossed-live",
        )
        .xml(),
    )
    .await;
    assert_eq!(
        created_distribution.status, 201,
        "case creating the distribution: expected HTTP 201, got {} with {}",
        created_distribution.status, created_distribution.raw
    );
    let created_control = create_control(
        &router,
        &table()[0]
            .with_name("the control these rows cross", "oac-crossed-live")
            .xml(),
    )
    .await;
    assert_eq!(
        created_control.status, 201,
        "case creating the control: expected HTTP 201, got {} with {}",
        created_control.status, created_control.raw
    );

    let live = [
        Live {
            case: "a live public key's identifier",
            id: key.clone(),
        },
        Live {
            case: "a live field-level encryption configuration's identifier",
            id: created_config
                .root()
                .text_at("Id")
                .unwrap_or_default()
                .to_string(),
        },
        Live {
            case: "a live distribution's identifier",
            id: created_distribution
                .root()
                .text_at("Id")
                .unwrap_or_default()
                .to_string(),
        },
        Live {
            case: "a live origin access control's identifier",
            id: created_control
                .root()
                .text_at("Id")
                .unwrap_or_default()
                .to_string(),
        },
    ];

    // The positive control the rule needs: every crossed value is an identifier this account
    // really resolves for its OWN kind, read back through that kind's own reader.
    assert_served(&get_key(&router, &live[0].id).await, live[0].case);
    assert_served(&get_config(&router, &live[1].id).await, live[1].case);
    assert_served(&get_distribution(&router, &live[2].id).await, live[2].case);
    assert_served(&get_control(&router, &live[3].id).await, live[3].case);

    // Every identifier this suite crosses, the profile's own included, was issued at one width
    // under one letter — read off the creates rather than written out here.
    let issued_width = profile_id.len();
    for row in &live {
        assert_eq!(
            row.id.len(),
            issued_width,
            "case {}: expected an identifier of the issued width {issued_width}, got {:?}",
            row.case,
            row.id
        );
        assert_ne!(
            row.id.chars().next(),
            profile_id.chars().next(),
            "case {}: expected a namespace letter other than the profile's, got {:?}",
            row.case,
            row.id
        );
    }

    // An identifier issued for another kind is the identifier no profile route can read, on
    // every route addressed by a profile identifier.
    for (case, id) in live
        .iter()
        .map(|row| (row.case.to_string(), row.id.clone()))
    {
        for route in ROUTES {
            let answer = route.send(&router, &id, &document, &if_match).await;
            assert_refusal(
                &answer,
                &format!("{route:?} with {case} ({id:?})"),
                UNREADABLE_IDENTIFIER,
            );
        }
    }

    // And the contrast: a value that is NOT another kind's identifier is carried through to the
    // store, so the profile it holds none under is the profile reported absent. The rows vary the
    // namespace letter and the width.
    let profile_letter = profile_id.chars().next().expect("a minted identifier");
    let other_letter = live[0].id.chars().next().expect("a minted identifier");
    let body = "A".repeat(issued_width - 1);
    let carried: Vec<(String, String)> = vec![
        (
            "the profile's own namespace over a body never minted".to_string(),
            format!("{profile_letter}{body}"),
        ),
        (
            "a namespace letter no kind of this service issues one under".to_string(),
            format!("{UNISSUED_NAMESPACE}{body}"),
        ),
        (
            "another kind's namespace one character short of the issued width".to_string(),
            format!("{other_letter}{}", "A".repeat(issued_width - 2)),
        ),
        (
            "another kind's namespace one character past the issued width".to_string(),
            format!("{other_letter}{body}A"),
        ),
    ];
    for (case, id) in &carried {
        for route in ROUTES {
            let answer = route.send(&router, id, &document, &if_match).await;
            assert_refusal(
                &answer,
                &format!("{route:?} with {case} ({id:?})"),
                NO_SUCH_PROFILE,
            );
        }
    }

    // None of those calls touched the store: every live resource is still served at its own
    // identifier, and so is the profile the rows crossed.
    assert_served(&get_key(&router, &live[0].id).await, live[0].case);
    assert_served(&get_config(&router, &live[1].id).await, live[1].case);
    assert_served(&get_distribution(&router, &live[2].id).await, live[2].case);
    assert_served(&get_control(&router, &live[3].id).await, live[3].case);
    assert_served(
        &get_profile(&router, &profile_id).await,
        "the profile after every crossed call",
    );
    assert_served(
        &get_profile_config(&router, &profile_id).await,
        "the profile document after every crossed call",
    );
}
