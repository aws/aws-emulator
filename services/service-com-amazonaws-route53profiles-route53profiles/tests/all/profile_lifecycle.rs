// SPDX-License-Identifier: CC0-1.0

//! The Route 53 Profile lifecycle, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises
//! the same restJson1 path a real SDK client takes: routing, deserialization,
//! boundary constraint validation, the handler, and error rendering.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a created Profile is persisted, and `GetProfile` / `ListProfiles` report exactly
//!   what was stored — for every name the modelled `Name` accepts, with or without
//!   `Tags`;
//! * a Profile's `Arn` is
//!   `arn:aws:route53profiles:<region>:<account>:profile/<Id>` for whatever region the
//!   request is signed for;
//! * a repeated `ClientToken` reports the Profile that token already minted instead of
//!   creating another;
//! * `ListProfiles` and `ListProfileAssociations` return every stored record exactly
//!   once across pages, for every page size, and emit a resumable token only while
//!   items remain; a token neither issued is `InvalidNextTokenException`;
//! * `ListProfileAssociations` narrows by `profileId` and `resourceId`;
//! * `AssociateProfile` persists an association readable by `GetProfileAssociation`,
//!   refuses a (ProfileId, ResourceId) pair that is already associated, refuses an
//!   associate that would rename or re-prioritise one that already exists, places
//!   each association in its VPC's evaluation order by `Priority`, and refuses a
//!   `ProfileId` that names nothing;
//! * a VPC's Profile associations are evaluated in priority order: the first one may
//!   be made without a `Priority` and gets 100, a further one has to state a priority
//!   no other association of that VPC holds, and a VPC past its allowance is refused;
//! * `UpdateProfileAssociation` applies the `Name` and `Priority` the request names
//!   and nothing else, reports a changed priority as propagating until it settles,
//!   and refuses a priority another association of the same VPC holds;
//! * an association's `StatusMessage` states the lifecycle step the response is
//!   about — the creation the associate call performed, the created association
//!   every read reports, the removal the disassociate call performed — and both its
//!   lifecycle timestamps are carried as epoch-seconds numbers at millisecond
//!   resolution;
//! * `DisassociateProfile` removes the association and reports it `DELETING`;
//! * `DeleteProfile` refuses with `ConflictException` while the Profile still has
//!   associations, and otherwise removes it and reports it `DELETING`;
//! * an id that names nothing is `ResourceNotFoundException`, and a `MaxResults`
//!   outside the modelled range is refused;
//! * the tag slice holds one key-to-value set per resource, addressed by that
//!   resource's ARN: `TagResource` upserts the keys it names, `UntagResource` removes
//!   exactly the keys it names, and `ListTagsForResource` reports what is held — for
//!   every resource kind of this service that has an ARN, over tag sets spanning the
//!   modelled `TagKey` / `TagValue` bounds, and up to the modelled ceiling of 200 tags
//!   per resource;
//! * every tag operation refuses an ARN this service holds no resource under with
//!   `ResourceNotFoundException`, and one that names no taggable resource of this
//!   service with `ValidationException`, persisting nothing either way.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::harness::{assert_timestamp_form, Emulator, ACCOUNT_ID};

/// The names in this table are the classes the modelled `Name` shape accepts —
/// `@length(0..64)` with pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`: a single
/// character, the plain alphanumeric case, a hyphen, an underscore with an interior
/// space, an apostrophe, a digit-leading name that is not all digits, and exactly the
/// maximum length.
fn accepted_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "testprofile".to_string(),
        "test-profile".to_string(),
        "My_Profile 1".to_string(),
        "o'brien".to_string(),
        "0a".to_string(),
        "A".repeat(64),
    ]
}

/// Whether `id` is `<prefix>-` followed by 16 lowercase hex digits — the id shape
/// this emulator mints, inside `ResourceId`'s modelled 1..64 length.
fn is_minted_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('-')) {
        Some(hex) => hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

#[tokio::test]
async fn a_created_profile_is_persisted_and_read_back_whole() {
    // Two regions, because the ARN is built from the request's credential scope.
    for region in ["us-east-1", "eu-west-2"] {
        let em = Emulator::in_region(region);

        for name in accepted_names() {
            for tags in [
                None,
                Some(json!([])),
                Some(json!([{ "Key": "env", "Value": "test" }])),
                Some(json!([{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }])),
            ] {
                let token = format!("tok-{region}-{name}-{}", tags.is_some());
                let mut body = json!({ "Name": name, "ClientToken": token });
                if let Some(tags) = tags.clone() {
                    body["Tags"] = tags;
                }
                let case =
                    format!("region={region} name={name:?} tags={tags:?}");

                let created = em.create_profile(body).await;
                assert_eq!(
                    created.status, 200,
                    "case {case}: expected 200, got {} / {:?}",
                    created.status, created.body
                );
                let profile = created.member("Profile").clone();
                let id = created.nested_string("Profile", "Id");

                assert!(
                    is_minted_id(&id, "rp"),
                    "case {case}: expected a minted `rp-<16 hex>` Id, got {id:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Name"),
                    name,
                    "case {case}: the created Name must be the one requested, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Arn"),
                    format!("arn:aws:route53profiles:{region}:{ACCOUNT_ID}:profile/{id}"),
                    "case {case}: unexpected Arn in {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "OwnerId"),
                    ACCOUNT_ID,
                    "case {case}: unexpected OwnerId in {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Status"),
                    "COMPLETE",
                    "case {case}: a created Profile is COMPLETE, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "ShareStatus"),
                    "NOT_SHARED",
                    "case {case}: a created Profile is NOT_SHARED, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "ClientToken"),
                    token,
                    "case {case}: ClientToken must be echoed, got {profile:?}"
                );
                assert_eq!(
                    profile["CreationTime"], profile["ModificationTime"],
                    "case {case}: a Profile nothing has changed was modified when created, \
                     got {profile:?}"
                );
                assert!(
                    !profile["CreationTime"].is_null(),
                    "case {case}: CreationTime must be reported, got {profile:?}"
                );
                // Every lifecycle timestamp of this service is carried the same way,
                // whichever shape reports it.
                for member in ["CreationTime", "ModificationTime"] {
                    assert_timestamp_form(&case, member, &profile[member]);
                }

                // The read-back is the same record, member for member.
                let fetched = em.get_profile(&id).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case}: GetProfile expected 200, got {} / {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    *fetched.member("Profile"),
                    profile,
                    "case {case}: GetProfile must report the created record, got {:?}",
                    fetched.body
                );

                // And the summary the list reports is that record's subset.
                let listed = em.list_profiles("").await;
                let summary = listed
                    .list("ProfileSummaries")
                    .iter()
                    .find(|entry| entry["Id"] == profile["Id"])
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: the created Profile must appear in ListProfiles, got {:?}",
                            listed.body
                        )
                    })
                    .clone();
                assert_eq!(
                    summary,
                    json!({
                        "Id": profile["Id"],
                        "Arn": profile["Arn"],
                        "Name": profile["Name"],
                        "ShareStatus": profile["ShareStatus"],
                    }),
                    "case {case}: the summary must be the stored record's subset, got {summary:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn repeating_a_client_token_reports_the_first_profile() {
    let em = Emulator::new();

    // The token values differ in form (short, long, punctuated, exactly the modelled
    // 255-character maximum of `CreatorRequestId`); every one must be honoured the
    // same way.
    for token in [
        "t".to_string(),
        "tok-1".to_string(),
        "9f8e7d6c-5b4a-3210-9f8e-7d6c5b4a3210".to_string(),
        "z".repeat(255),
    ] {
        let first = em
            .create_profile(json!({ "Name": "first-name", "ClientToken": token }))
            .await;
        assert_eq!(
            first.status, 200,
            "case token={token:?}: expected 200, got {} / {:?}",
            first.status, first.body
        );
        let before = em.list_profiles("").await.list("ProfileSummaries").len();

        // A repeat under the same token, even naming something else, reports the
        // Profile that token already minted and creates nothing.
        let repeat = em
            .create_profile(json!({ "Name": "second-name", "ClientToken": token }))
            .await;
        assert_eq!(
            repeat.status, 200,
            "case token={token:?}: a repeat expected 200, got {} / {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            *repeat.member("Profile"),
            *first.member("Profile"),
            "case token={token:?}: a repeated ClientToken must report the first Profile, \
             got {:?}",
            repeat.body
        );
        let after = em.list_profiles("").await.list("ProfileSummaries").len();
        assert_eq!(
            after, before,
            "case token={token:?}: a repeated ClientToken must not store a second Profile"
        );
    }
}

#[tokio::test]
async fn list_profiles_pages_every_profile_exactly_once() {
    let em = Emulator::new();

    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..7 {
        let reply = em.create_named(&format!("paged-{index}")).await;
        assert_eq!(
            reply.status, 200,
            "setup: create paged-{index} expected 200, got {:?}",
            reply.body
        );
        created.insert(reply.nested_string("Profile", "Id"));
    }

    // Every page size from one item to more than the whole store, plus the absent
    // `MaxResults`, must walk the same set exactly once.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(100)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match page_size {
                Some(size) => format!("?maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = next.as_deref() {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("nextToken={token}"));
            }
            let reply = em.list_profiles(&query).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: ListProfiles expected 200, got {} / {:?}",
                reply.status, reply.body
            );
            let page = reply.list("ProfileSummaries");
            assert!(
                page.len() <= page_size.unwrap_or(100) as usize,
                "case {case}: a page must hold at most maxResults items, got {}",
                page.len()
            );
            for entry in page {
                seen.push(
                    entry["Id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: a summary without an Id"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match reply.body["NextToken"].as_str() {
                Some(token) => next = Some(token.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no Profile may appear twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case}: every stored Profile must appear exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_the_operation_did_not_issue_is_refused() {
    let em = Emulator::new();
    for index in 0..3 {
        em.create_named(&format!("tokened-{index}")).await;
        em.associate(
            &em.create_named(&format!("assoc-owner-{index}"))
                .await
                .nested_string("Profile", "Id"),
            &format!("vpc-0000000{index}"),
            &format!("assoc-{index}"),
        )
        .await;
    }

    // A token one list operation issued resumes only that operation: the marker it
    // carries names the issuer, so the other list refuses it rather than reading it
    // as some unrelated resume point.
    let profiles_token = em
        .list_profiles("?maxResults=1")
        .await
        .body["NextToken"]
        .as_str()
        .expect("a resumable ListProfiles token while items remain")
        .to_string();
    let associations_token = em
        .list_associations("?maxResults=1")
        .await
        .body["NextToken"]
        .as_str()
        .expect("a resumable ListProfileAssociations token while items remain")
        .to_string();

    let bogus = [
        String::new(),
        "not-hex".to_string(),
        hex::encode("ListProfiles/1/"),
        hex::encode("some-other-marker/1/rp-0000000000000001"),
    ];

    for token in bogus
        .iter()
        .cloned()
        .chain([associations_token.clone()])
        .collect::<Vec<_>>()
    {
        let reply = em.list_profiles(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("InvalidNextTokenException"),
            "case nextToken={token:?}: ListProfiles must refuse a token it did not issue, \
             got {} / {:?}",
            reply.status,
            reply.body
        );
    }

    for token in bogus
        .iter()
        .cloned()
        .chain([profiles_token.clone()])
        .collect::<Vec<_>>()
    {
        let reply = em.list_associations(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("InvalidNextTokenException"),
            "case nextToken={token:?}: ListProfileAssociations must refuse a token it did not \
             issue, got {} / {:?}",
            reply.status,
            reply.body
        );
    }

    // The operation's own token still resumes it.
    let resumed = em
        .list_profiles(&format!("?maxResults=1&nextToken={profiles_token}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "a token ListProfiles issued must resume it, got {} / {:?}",
        resumed.status, resumed.body
    );
}

#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_outside_them() {
    let em = Emulator::new();
    em.create_named("bounded").await;

    // `MaxResults` is modelled `@range(min: 1, max: 100)` on both list operations.
    for (value, expected_ok) in [(1, true), (100, true), (0, false), (101, false)] {
        for (operation, reply) in [
            (
                "ListProfiles",
                em.list_profiles(&format!("?maxResults={value}")).await,
            ),
            (
                "ListProfileAssociations",
                em.list_associations(&format!("?maxResults={value}")).await,
            ),
        ] {
            if expected_ok {
                assert_eq!(
                    reply.status, 200,
                    "case {operation} maxResults={value}: a value on the modelled bound must be \
                     accepted, got {} / {:?}",
                    reply.status, reply.body
                );
            } else {
                assert_eq!(
                    reply.status, 400,
                    "case {operation} maxResults={value}: a value outside the modelled range must \
                     be refused, got {} / {:?}",
                    reply.status, reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_id_that_names_nothing_is_not_found() {
    let em = Emulator::new();
    // Forms differing in shape: a well-formed but unused minted id, a bare word, and
    // exactly the modelled 64-character maximum of `ResourceId`.
    for id in [
        "rp-0123456789abcdef".to_string(),
        "nosuchprofile".to_string(),
        "z".repeat(64),
    ] {
        for (operation, reply) in [
            ("GetProfile", em.get_profile(&id).await),
            ("DeleteProfile", em.delete_profile(&id).await),
            ("GetProfileAssociation", em.get_association(&id).await),
            ("DisassociateProfile", em.disassociate(&id, "vpc-00000001").await),
        ] {
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation} id={id:?}: an unknown id must be ResourceNotFoundException, \
                 got {} / {:?}",
                reply.status,
                reply.body
            );
        }
    }

    // Associating with an unknown Profile is the same refusal.
    let reply = em.associate("rp-0123456789abcdef", "vpc-00000001", "a1").await;
    assert_eq!(
        reply.error.as_deref(),
        Some("ResourceNotFoundException"),
        "AssociateProfile with an unknown ProfileId must be ResourceNotFoundException, \
         got {} / {:?}",
        reply.status,
        reply.body
    );
}

#[tokio::test]
async fn the_association_lifecycle_is_persisted_and_reversible() {
    let em = Emulator::new();
    let profile = em.create_named("assoc-host").await;
    let profile_id = profile.nested_string("Profile", "Id");
    // A second Profile, so the "one Profile per resource" rule has a rival to refuse.
    let rival_id = em
        .create_named("assoc-rival")
        .await
        .nested_string("Profile", "Id");

    // Rows differing in the VPC id's form and in the association name's class.
    let rows = [
        ("vpc-12345678", "assoc-1"),
        ("vpc-0a1b2c3d4e5f67890", "My_Assoc 2"),
        ("v", "a"),
    ];

    for (resource_id, name) in rows {
        let case = format!("resourceId={resource_id:?} name={name:?}");

        let created = em.associate(&profile_id, resource_id, name).await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        let association = created.member("ProfileAssociation").clone();
        let association_id = created.nested_string("ProfileAssociation", "Id");
        assert!(
            is_minted_id(&association_id, "rpassoc"),
            "case {case}: expected a minted `rpassoc-<16 hex>` Id, got {association_id:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "Name"),
            name,
            "case {case}: the created Name must be the one requested, got {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "ProfileId"),
            profile_id,
            "case {case}: unexpected ProfileId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "ResourceId"),
            resource_id,
            "case {case}: unexpected ResourceId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "OwnerId"),
            ACCOUNT_ID,
            "case {case}: unexpected OwnerId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "Status"),
            "COMPLETE",
            "case {case}: a created association is COMPLETE, got {association:?}"
        );

        // Readable back whole, and listable under either filter. The read reports the
        // STORED record, whose `StatusMessage` states the created association, where
        // the associate response states the creation that call performed — so the
        // record every read and listing projects is compared against that one.
        let stored = {
            let mut stored = association.clone();
            stored["StatusMessage"] = json!("Created Profile Association");
            stored
        };
        let fetched = em.get_association(&association_id).await;
        assert_eq!(
            *fetched.member("ProfileAssociation"),
            stored,
            "case {case}: GetProfileAssociation must report the created record, got {:?}",
            fetched.body
        );
        for query in [
            format!("?profileId={profile_id}"),
            format!("?resourceId={resource_id}"),
            format!("?profileId={profile_id}&resourceId={resource_id}"),
        ] {
            let listed = em.list_associations(&query).await;
            assert!(
                listed.list("ProfileAssociations").contains(&stored),
                "case {case}: {query} must report the association, got {:?}",
                listed.body
            );
        }

        // The same (ProfileId, ResourceId) pair cannot be associated twice: a repeat
        // that asks for the association exactly as it stands reports it as existing,
        // while one naming it differently asks for a CHANGE this operation does not
        // make.
        let duplicate = em.associate(&profile_id, resource_id, name).await;
        assert_eq!(
            duplicate.error.as_deref(),
            Some("ResourceExistsException"),
            "case {case}: a duplicate pair must be refused, got {} / {:?}",
            duplicate.status,
            duplicate.body
        );
        let renamed = em
            .associate(&profile_id, resource_id, "a-different-name")
            .await;
        assert_eq!(
            renamed.error.as_deref(),
            Some("ValidationException"),
            "case {case}: an associate that renames an existing association must be \
             refused, got {} / {:?}",
            renamed.status,
            renamed.body
        );

        // A VPC's associations are evaluated in priority order, so a further one has
        // to state a priority of its own — and the first association is untouched.
        let taken = em.associate(&rival_id, resource_id, "rival-assoc").await;
        assert_eq!(
            taken.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a second Profile over the same ResourceId with no Priority \
             must be refused, got {} / {:?}",
            taken.status,
            taken.body
        );
        assert_eq!(
            *em.get_association(&association_id)
                .await
                .member("ProfileAssociation"),
            stored,
            "case {case}: a refused association must not have changed the stored one"
        );
        assert!(
            em.list_associations(&format!("?profileId={rival_id}"))
                .await
                .list("ProfileAssociations")
                .is_empty(),
            "case {case}: a refused association must not have been stored"
        );

        // The Profile cannot be deleted while this association stands.
        let blocked = em.delete_profile(&profile_id).await;
        assert_eq!(
            blocked.error.as_deref(),
            Some("ConflictException"),
            "case {case}: DeleteProfile must refuse while associations remain, got {} / {:?}",
            blocked.status,
            blocked.body
        );

        // Disassociating reports the removed record as DELETING, and it is gone.
        let removed = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            removed.status, 200,
            "case {case}: DisassociateProfile expected 200, got {} / {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileAssociation", "Id"),
            association_id,
            "case {case}: the removed association must be the stored one, got {:?}",
            removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileAssociation", "Status"),
            "DELETING",
            "case {case}: a removed association is DELETING, got {:?}",
            removed.body
        );
        assert_eq!(
            em.get_association(&association_id).await.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a removed association must no longer be readable"
        );
        let listed = em
            .list_associations(&format!("?profileId={profile_id}"))
            .await;
        assert!(
            listed.list("ProfileAssociations").is_empty(),
            "case {case}: a removed association must no longer be listed, got {:?}",
            listed.body
        );
        assert_eq!(
            em.disassociate(&profile_id, resource_id).await.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a pair that is not associated must be ResourceNotFoundException"
        );
    }

    // With every association gone the Profile deletes, reports DELETING, and vanishes
    // from both the read and the list.
    let deleted = em.delete_profile(&profile_id).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteProfile expected 200, got {} / {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.nested_string("Profile", "Id"),
        profile_id,
        "the removed Profile must be the stored one, got {:?}",
        deleted.body
    );
    assert_eq!(
        deleted.nested_string("Profile", "Status"),
        "DELETING",
        "a removed Profile is DELETING, got {:?}",
        deleted.body
    );
    assert_eq!(
        em.get_profile(&profile_id).await.error.as_deref(),
        Some("ResourceNotFoundException"),
        "a deleted Profile must no longer be readable"
    );
    let listed = em.list_profiles("").await;
    assert!(
        listed
            .list("ProfileSummaries")
            .iter()
            .all(|entry| entry["Id"] != json!(profile_id)),
        "a deleted Profile must no longer be listed, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_vpc_associations_status_message_states_the_lifecycle_step_being_reported() {
    // The rule over a SET of associations: a VPC association exists the moment it is
    // recorded, so its `Status` is the terminal one from the associate response
    // onwards, and its `StatusMessage` states the lifecycle step the response is
    // about — the creation the associate call performed, the created association
    // every read reports, and the removal the disassociate call performed. The three
    // sentences are transcribed here, so this suite states them independently of the
    // code that renders them.
    let em = Emulator::new();
    let profile_id = em
        .create_named("assoc-phases")
        .await
        .nested_string("Profile", "Id");

    // The rows differ in the name's character classes and in the VPC id's length, so
    // the pair is read off records differing in more than their id.
    for (name, resource_id) in [
        ("a", "vpc-1"),
        ("phase-assoc", "vpc-0a1b2c3d4e5f60718"),
        ("My_Assoc 1", "vpc-00000000000000001"),
        ("o'brien", "vpc-0f1e2d3c"),
    ] {
        let case = format!("name={name:?} resource={resource_id}");

        let created = em.associate(&profile_id, resource_id, name).await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        let association_id = created.nested_string("ProfileAssociation", "Id");
        assert_eq!(
            created.nested_string("ProfileAssociation", "Status"),
            "COMPLETE",
            "case {case}: a recorded association is COMPLETE, got {:?}",
            created.body
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "StatusMessage"),
            "Creating Profile Association",
            "case {case}: unexpected StatusMessage in {:?}",
            created.body
        );
        for member in ["CreationTime", "ModificationTime"] {
            assert_timestamp_form(&case, member, &created.member("ProfileAssociation")[member]);
        }
        let created_at = association_instant(&case, &created, "CreationTime");
        assert_eq!(
            created_at,
            association_instant(&case, &created, "ModificationTime"),
            "case {case}: an association nothing has changed was modified when created, got {:?}",
            created.body
        );

        // Every read reports the stored record — the created association — and keeps
        // reporting the same thing, with the creation instant it was recorded at.
        for read in 0..2 {
            let fetched = em.get_association(&association_id).await;
            assert_eq!(
                fetched.nested_string("ProfileAssociation", "Status"),
                "COMPLETE",
                "case {case}: read {read} must report COMPLETE, got {:?}",
                fetched.body
            );
            assert_eq!(
                fetched.nested_string("ProfileAssociation", "StatusMessage"),
                "Created Profile Association",
                "case {case}: read {read} reported an unexpected StatusMessage in {:?}",
                fetched.body
            );
            for member in ["CreationTime", "ModificationTime"] {
                assert_timestamp_form(&case, member, &fetched.member("ProfileAssociation")[member]);
            }
            assert_eq!(
                association_instant(&case, &fetched, "CreationTime"),
                created_at,
                "case {case}: read {read} must report the creation instant unchanged, got {:?}",
                fetched.body
            );
        }

        // A listing projects the same record as the read.
        let listed = em
            .list_associations(&format!("?resourceId={resource_id}"))
            .await;
        assert_eq!(
            listed.list("ProfileAssociations").len(),
            1,
            "case {case}: the VPC must be listed once, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.list("ProfileAssociations")[0]["StatusMessage"],
            json!("Created Profile Association"),
            "case {case}: a listed association must report the same sentence as the read, \
             got {:?}",
            listed.body
        );

        // The removal reports the step it performed, with a modification instant no
        // earlier than the creation one.
        let removed = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            removed.nested_string("ProfileAssociation", "Status"),
            "DELETING",
            "case {case}: a removed association is DELETING, got {:?}",
            removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileAssociation", "StatusMessage"),
            "Deleting Profile Association",
            "case {case}: unexpected removal StatusMessage in {:?}",
            removed.body
        );
        assert!(
            association_instant(&case, &removed, "ModificationTime") >= created_at,
            "case {case}: the removal must report when the record last changed, got {:?}",
            removed.body
        );
    }
}

/// One timestamp member of the `ProfileAssociation` a reply reports, as the epoch
/// seconds the wire carries. Panics with the whole body when the member is absent or
/// is no number.
fn association_instant(case: &str, reply: &super::harness::Reply, member: &str) -> f64 {
    reply.member("ProfileAssociation")[member]
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: ProfileAssociation.{member} is no number in {:?}",
                reply.body
            )
        })
}

#[tokio::test]
async fn list_profile_associations_narrows_by_filter_and_pages_every_record_once() {
    let em = Emulator::new();

    // Two Profiles, each associated with a different number of VPCs, plus one VPC
    // shared between them — so each filter has a distinguishable answer.
    let mut owners: Vec<String> = Vec::new();
    for index in 0..2 {
        owners.push(
            em.create_named(&format!("filter-owner-{index}"))
                .await
                .nested_string("Profile", "Id"),
        );
    }
    let mut expected: Vec<(String, String, String)> = Vec::new();
    for (owner_index, profile_id) in owners.iter().enumerate() {
        // Every VPC here belongs to exactly one owner; the two owners hold different
        // numbers of them, so a `profileId` filter has a distinguishable answer.
        for vpc_index in 0..=owner_index + 1 {
            let resource_id = format!("vpc-{owner_index}{vpc_index}");
            let reply = em
                .associate(profile_id, &resource_id, &format!("a-{owner_index}-{vpc_index}"))
                .await;
            assert_eq!(
                reply.status, 200,
                "setup: associate {profile_id}/{resource_id} expected 200, got {:?}",
                reply.body
            );
            expected.push((
                reply.nested_string("ProfileAssociation", "Id"),
                profile_id.clone(),
                resource_id,
            ));
        }
    }

    // A VPC one Profile already holds admits another only at a priority of its own.
    let (_, held_by, held_vpc) = expected[0].clone();
    let rival = owners
        .iter()
        .find(|owner| **owner != held_by)
        .expect("a second owner Profile");
    let taken = em.associate(rival, &held_vpc, "rival-assoc").await;
    assert_eq!(
        taken.error.as_deref(),
        Some("ValidationException"),
        "a second association with a VPC and no Priority must be refused, got {} / {:?}",
        taken.status,
        taken.body
    );

    // Each filter combination, against the set it should select.
    let unique_vpc = expected[0].2.clone();
    let unique_vpc_owner = expected[0].1.clone();
    let other_owner = rival.clone();
    let filters: Vec<(String, Vec<String>)> = {
        let mut cases = vec![(
            String::new(),
            expected.iter().map(|(id, _, _)| id.clone()).collect(),
        )];
        for profile_id in &owners {
            cases.push((
                format!("profileId={profile_id}"),
                expected
                    .iter()
                    .filter(|(_, owner, _)| owner == profile_id)
                    .map(|(id, _, _)| id.clone())
                    .collect(),
            ));
        }
        // `resourceId` alone selects the single association holding that VPC.
        cases.push((
            format!("resourceId={unique_vpc}"),
            expected
                .iter()
                .filter(|(_, _, resource)| *resource == unique_vpc)
                .map(|(id, _, _)| id.clone())
                .collect(),
        ));
        // Both filters together: the matching pair selects it, a mismatched pair
        // selects nothing even though each half matches something.
        cases.push((
            format!("profileId={unique_vpc_owner}&resourceId={unique_vpc}"),
            expected
                .iter()
                .filter(|(_, owner, resource)| {
                    *owner == unique_vpc_owner && *resource == unique_vpc
                })
                .map(|(id, _, _)| id.clone())
                .collect(),
        ));
        cases.push((
            format!("profileId={other_owner}&resourceId={unique_vpc}"),
            Vec::new(),
        ));
        cases.push(("resourceId=vpc-nosuchvpc".to_string(), Vec::new()));
        cases.push(("profileId=rp-0123456789abcdef".to_string(), Vec::new()));
        cases
    };

    for (filter, wanted) in filters {
        let wanted: BTreeSet<String> = wanted.into_iter().collect();
        for page_size in [None, Some(1), Some(2), Some(100)] {
            let case = format!("filter={filter:?} maxResults={page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut params: Vec<String> = Vec::new();
                if !filter.is_empty() {
                    params.push(filter.clone());
                }
                if let Some(size) = page_size {
                    params.push(format!("maxResults={size}"));
                }
                if let Some(token) = next.as_deref() {
                    params.push(format!("nextToken={token}"));
                }
                let query = if params.is_empty() {
                    String::new()
                } else {
                    format!("?{}", params.join("&"))
                };
                let reply = em.list_associations(&query).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case}: expected 200, got {} / {:?}",
                    reply.status, reply.body
                );
                for entry in reply.list("ProfileAssociations") {
                    seen.push(
                        entry["Id"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case}: an association without an Id")
                            })
                            .to_string(),
                    );
                }
                pages += 1;
                assert!(
                    pages <= 16,
                    "case {case}: pagination did not terminate after {pages} pages"
                );
                match reply.body["NextToken"].as_str() {
                    Some(token) => next = Some(token.to_string()),
                    None => break,
                }
            }
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case}: no association may appear twice across pages, got {seen:?}"
            );
            assert_eq!(
                unique, wanted,
                "case {case}: the filter must select exactly its own associations, got {seen:?}"
            );
        }
    }
}

/// The region the shared harness signs for, used to build the ARNs below.
const REGION: &str = "us-east-1";

/// One tag set as the modelled `TagMap` the wire carries: a JSON object of key to
/// value (`smithy_query(query="shape",shape="TagMap")`).
fn tag_map(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The tag sets the slice has to round-trip, as `(case, tags)`.
///
/// The rows differ along what the rule generalises over — how many tags a write
/// carries, and the forms a key and a value take: `TagKey` is `length 1..128` and
/// `TagValue` `length 0..256` (`smithy_query(query="shape",shape="TagMap")`), so
/// both published bounds of each appear, as do characters a path or a query string
/// gives its own meaning and characters outside ASCII, since neither member
/// constrains its alphabet.
fn tag_cases() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        ("one", vec![("env".to_string(), "test".to_string())]),
        (
            "several",
            vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "net".to_string()),
                ("owner".to_string(), "dns".to_string()),
            ],
        ),
        (
            "modelled-bounds",
            vec![
                ("k".to_string(), String::new()),
                ("K".repeat(128), "v".repeat(256)),
            ],
        ),
        (
            "punctuation-and-non-ascii",
            vec![
                ("a/b c".to_string(), "x=y&z?#".to_string()),
                ("clé".to_string(), "日本語".to_string()),
            ],
        ),
    ]
}

/// Every resource kind of this service a tag operation can address, as
/// `(kind, ResourceArn)`, each created fresh and carrying no tags.
///
/// A Profile reports its own `Arn`. The `ProfileAssociation` shape reports no `Arn`
/// member, so an association's ARN is built here the way the service's IAM resource
/// type `profile-association` spells it —
/// `arn:aws:route53profiles:<region>:<account>:profile-association/<Id>`.
async fn taggable_arns(em: &Emulator, label: &str) -> Vec<(&'static str, String)> {
    let profile = em.create_named(&format!("{label}-profile")).await;
    assert_eq!(
        profile.status, 200,
        "setup {label}: CreateProfile expected 200, got {} / {:?}",
        profile.status, profile.body
    );
    let profile_id = profile.nested_string("Profile", "Id");
    let association = em
        .associate(&profile_id, &format!("vpc-{label}"), &format!("{label}-assoc"))
        .await;
    assert_eq!(
        association.status, 200,
        "setup {label}: AssociateProfile expected 200, got {} / {:?}",
        association.status, association.body
    );
    let association_id = association.nested_string("ProfileAssociation", "Id");
    vec![
        ("profile", profile.nested_string("Profile", "Arn")),
        (
            "profile-association",
            format!(
                "arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile-association/{association_id}"
            ),
        ),
    ]
}

/// The `Tags` map `ListTagsForResource` reports for `resource_arn`, asserting the
/// call succeeded.
async fn reported_tags(em: &Emulator, case: &str, resource_arn: &str) -> Value {
    let listed = em.list_tags(resource_arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case}: ListTagsForResource expected 200, got {} / {:?}",
        listed.status, listed.body
    );
    listed.member("Tags").clone()
}

#[tokio::test]
async fn the_tag_slice_round_trips_every_tag_set_for_every_taggable_resource() {
    let em = Emulator::new();

    for (kind, arn) in taggable_arns(&em, "round-trip").await {
        // A resource nobody has tagged reports an empty map rather than a refusal.
        assert_eq!(
            reported_tags(&em, kind, &arn).await,
            json!({}),
            "case kind={kind}: a resource carrying no tags must report none"
        );

        for (name, tags) in tag_cases() {
            let case = format!("kind={kind} tags={name}");

            let tagged = em.tag_resource(&arn, tag_map(&tags)).await;
            assert_eq!(
                tagged.status, 204,
                "case {case}: TagResource expected 204, got {} / {:?}",
                tagged.status, tagged.body
            );
            assert_eq!(
                reported_tags(&em, &case, &arn).await,
                tag_map(&tags),
                "case {case}: every tag written must be reported back"
            );

            // A key the resource already carries takes the value of the later write,
            // and the keys the write does not name are left alone.
            let (first_key, _) = tags[0].clone();
            let upserted = em
                .tag_resource(
                    &arn,
                    tag_map(&[(first_key.clone(), "rewritten".to_string())]),
                )
                .await;
            assert_eq!(
                upserted.status, 204,
                "case {case}: a repeated key expected 204, got {} / {:?}",
                upserted.status, upserted.body
            );
            let mut expected = tags.clone();
            expected[0].1 = "rewritten".to_string();
            assert_eq!(
                reported_tags(&em, &case, &arn).await,
                tag_map(&expected),
                "case {case}: a repeated key must take the new value and leave the others"
            );

            // Untagging removes exactly the keys named: a key the resource does not
            // carry removes nothing, and repeating the call changes nothing more.
            let remaining: Vec<(String, String)> = expected[1..].to_vec();
            for round in 0..2 {
                let untagged = em
                    .untag_resource(&arn, &[first_key.as_str(), "never-written"])
                    .await;
                assert_eq!(
                    untagged.status, 204,
                    "case {case} round={round}: UntagResource expected 204, got {} / {:?}",
                    untagged.status, untagged.body
                );
                assert_eq!(
                    reported_tags(&em, &case, &arn).await,
                    tag_map(&remaining),
                    "case {case} round={round}: untagging must remove exactly the keys named"
                );
            }

            // With every key removed the resource reports nothing at all, exactly
            // like one that was never tagged — so the next row starts clean.
            // `TagKeys` is a required query binding, so a call naming no key is not
            // a request this operation serves and none is made.
            let rest: Vec<&str> = remaining.iter().map(|(key, _)| key.as_str()).collect();
            if !rest.is_empty() {
                let cleared = em.untag_resource(&arn, &rest).await;
                assert_eq!(
                    cleared.status, 204,
                    "case {case}: clearing the rest expected 204, got {} / {:?}",
                    cleared.status, cleared.body
                );
            }
            assert_eq!(
                reported_tags(&em, &case, &arn).await,
                json!({}),
                "case {case}: a resource whose last tag was removed must report none"
            );
        }
    }
}

#[tokio::test]
async fn every_tag_operation_refuses_an_arn_that_names_no_resource_of_this_service() {
    let em = Emulator::new();

    // One tagged Profile that every refusal below is checked against: a refused
    // request must leave its tags exactly as they were.
    let held = em.create_named("tag-refusals").await;
    let held_arn = held.nested_string("Profile", "Arn");
    let held_tags = vec![("keep".to_string(), "me".to_string())];
    assert_eq!(
        em.tag_resource(&held_arn, tag_map(&held_tags)).await.status,
        204,
        "setup: tagging the held Profile must succeed"
    );

    // A Profile that was deleted takes its tags with it, so its ARN names nothing.
    let gone = em.create_named("tag-deleted").await;
    let gone_arn = gone.nested_string("Profile", "Arn");
    let gone_id = gone.nested_string("Profile", "Id");
    assert_eq!(
        em.tag_resource(&gone_arn, tag_map(&held_tags)).await.status,
        204,
        "setup: tagging the Profile to delete must succeed"
    );
    assert_eq!(
        em.delete_profile(&gone_id).await.status,
        200,
        "setup: DeleteProfile must succeed"
    );

    // An ARN naming a resource kind this service tags, whose id it holds nothing
    // for: the resource is absent, so every tag operation reports it missing and
    // names the id from the request.
    let absent: Vec<(String, String)> = vec![
        (
            format!("arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile/rp-0123456789abcdef"),
            "rp-0123456789abcdef".to_string(),
        ),
        (
            format!(
                "arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile-association/rpassoc-fedcba9876543210"
            ),
            "rpassoc-fedcba9876543210".to_string(),
        ),
        // Another region and another partition name no resource this store holds
        // either, however well-formed they are.
        (
            format!("arn:aws-cn:route53profiles:cn-north-1:{ACCOUNT_ID}:profile/rp-1111111111111111"),
            "rp-1111111111111111".to_string(),
        ),
        (gone_arn.clone(), gone_id.clone()),
    ];
    for (resource_arn, resource_id) in absent {
        for reply in [
            em.list_tags(&resource_arn).await,
            em.tag_resource(&resource_arn, tag_map(&held_tags)).await,
            em.untag_resource(&resource_arn, &["keep"]).await,
        ] {
            let case = format!("arn={resource_arn}");
            assert_eq!(
                reply.status, 400,
                "case {case}: expected 400, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: unexpected error code, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.message(),
                format!("[RSLVR-05007] Can't find the resource with ID \"{resource_id}\"."),
                "case {case}: unexpected refusal message"
            );
        }
    }

    // A value this service cannot read as naming a taggable resource of its own is
    // refused as an invalid ARN, whatever the store holds: a string that is no ARN,
    // another service's resource, and a resource kind this service does not tag.
    let invalid: Vec<String> = vec![
        "not-an-arn".to_string(),
        "arn:aws:s3:::a-bucket".to_string(),
        format!(
            "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-rule/rslvr-rr-1111111111111111"
        ),
        format!(
            "arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile-resource-association/rpr-1111111111111111"
        ),
        // A resource type with no id after it names no resource.
        format!("arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile/"),
        format!("arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile"),
    ];
    for resource_arn in invalid {
        for reply in [
            em.list_tags(&resource_arn).await,
            em.tag_resource(&resource_arn, tag_map(&held_tags)).await,
            em.untag_resource(&resource_arn, &["keep"]).await,
        ] {
            let case = format!("arn={resource_arn}");
            assert_eq!(
                reply.status, 400,
                "case {case}: expected 400, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case}: unexpected error code, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.message(),
                format!(
                    "[RSLVR-05022] The ARN in your request isn't valid:\"{resource_arn}\". \
                     Provide a valid ARN."
                ),
                "case {case}: unexpected refusal message"
            );
        }
    }

    // Not one refused request touched the resource that does exist.
    assert_eq!(
        reported_tags(&em, "held", &held_arn).await,
        tag_map(&held_tags),
        "a refused tag operation must leave every stored tag as it was"
    );
}

#[tokio::test]
async fn tags_given_when_a_resource_is_created_are_reported_and_extended() {
    let em = Emulator::new();

    // `CreateProfile` and `AssociateProfile` both take a `Tags` list, and the tag
    // slice reports what they stored against the resource they created.
    let created = em
        .create_profile(json!({
            "Name": "seeded-profile",
            "ClientToken": "tok-seeded-profile",
            "Tags": [{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }],
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateProfile expected 200, got {} / {:?}",
        created.status, created.body
    );
    let profile_id = created.nested_string("Profile", "Id");
    let profile_arn = created.nested_string("Profile", "Arn");
    assert_eq!(
        reported_tags(&em, "created-profile", &profile_arn).await,
        json!({ "env": "test", "team": "net" }),
        "the tags CreateProfile stored must be reported back"
    );

    let association = em
        .call(
            "POST",
            "/profileassociation",
            Some(json!({
                "ProfileId": profile_id,
                "ResourceId": "vpc-seeded",
                "Name": "seeded-assoc",
                "Tags": [{ "Key": "owner", "Value": "dns" }],
            })),
        )
        .await;
    assert_eq!(
        association.status, 200,
        "AssociateProfile expected 200, got {} / {:?}",
        association.status, association.body
    );
    let association_arn = format!(
        "arn:aws:route53profiles:{REGION}:{ACCOUNT_ID}:profile-association/{}",
        association.nested_string("ProfileAssociation", "Id")
    );
    assert_eq!(
        reported_tags(&em, "created-association", &association_arn).await,
        json!({ "owner": "dns" }),
        "the tags AssociateProfile stored must be reported back"
    );

    // `TagResource` extends what the create stored rather than replacing it.
    for (resource_arn, expected) in [
        (
            profile_arn.clone(),
            json!({ "env": "test", "team": "core", "cost": "42" }),
        ),
        (
            association_arn.clone(),
            json!({ "owner": "dns", "team": "core", "cost": "42" }),
        ),
    ] {
        let case = format!("arn={resource_arn}");
        let tagged = em
            .tag_resource(&resource_arn, json!({ "team": "core", "cost": "42" }))
            .await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: TagResource expected 204, got {} / {:?}",
            tagged.status, tagged.body
        );
        assert_eq!(
            reported_tags(&em, &case, &resource_arn).await,
            expected,
            "case {case}: a tag write must add to the tags the create stored"
        );
    }
}

#[tokio::test]
async fn a_write_past_the_modelled_tag_ceiling_is_refused_and_changes_nothing() {
    let em = Emulator::new();
    let bulk = em.create_named("bulk-tags").await;
    let bulk_arn = bulk.nested_string("Profile", "Arn");

    // `TagMap` is `length 0..200`, so 200 tags is the most a resource can carry and
    // the most one request can hand over: a write at exactly the published maximum
    // is accepted.
    let full: Vec<(String, String)> = (0..200)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect();
    let filled = em.tag_resource(&bulk_arn, tag_map(&full)).await;
    assert_eq!(
        filled.status, 204,
        "case full: a write of 200 tags expected 204, got {} / {:?}",
        filled.status, filled.body
    );
    assert_eq!(
        reported_tags(&em, "full", &bulk_arn).await,
        tag_map(&full),
        "case full: every tag of a write at the maximum must be reported back"
    );

    // One more KEY would be the 201st, so the write is refused and the stored tags
    // are left exactly as they were.
    let over = em
        .tag_resource(&bulk_arn, json!({ "key-200": "one-too-many" }))
        .await;
    assert_eq!(
        over.status, 400,
        "case over: expected 400, got {} / {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error.as_deref(),
        Some("ValidationException"),
        "case over: unexpected error code, got {:?}",
        over.body
    );
    assert_eq!(
        over.message(),
        "A resource can have at most 200 tags.",
        "case over: unexpected refusal message"
    );
    assert_eq!(
        reported_tags(&em, "over", &bulk_arn).await,
        tag_map(&full),
        "case over: a refused write must persist nothing"
    );

    // A key the resource already carries is no new tag, so it is still an upsert at
    // the ceiling.
    let at_ceiling = em
        .tag_resource(&bulk_arn, json!({ "key-000": "rewritten" }))
        .await;
    assert_eq!(
        at_ceiling.status, 204,
        "case ceiling: rewriting a held key expected 204, got {} / {:?}",
        at_ceiling.status, at_ceiling.body
    );
    let mut rewritten = full.clone();
    rewritten[0].1 = "rewritten".to_string();
    assert_eq!(
        reported_tags(&em, "ceiling", &bulk_arn).await,
        tag_map(&rewritten),
        "case ceiling: a held key must take the new value even with the map full"
    );

    // And one request past the modelled map length is refused on a resource that
    // carries nothing at all, so the ceiling is the resource's, not the request's.
    let empty = em.create_named("bulk-tags-empty").await;
    let empty_arn = empty.nested_string("Profile", "Arn");
    let too_many: Vec<(String, String)> = (0..201)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect();
    let refused = em.tag_resource(&empty_arn, tag_map(&too_many)).await;
    assert_eq!(
        refused.status, 400,
        "case too-many: expected 400, got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        reported_tags(&em, "too-many", &empty_arn).await,
        json!({}),
        "case too-many: a refused write must persist nothing"
    );
}

/// The lowest `Priority` a Profile association may be evaluated at, as the model's
/// `Priority` shape declares its range (`smithy_query(query="shape",
/// shape="Priority")`: `range 100..9900`) and as the documentation of
/// `UpdateProfileAssociation` states it ("the range 100–9900"). Transcribed here, so
/// the suite states the published bound independently of the code that applies it.
const MIN_PRIORITY: i64 = 100;

/// The highest `Priority` a Profile association may be evaluated at, from that same
/// range.
const MAX_PRIORITY: i64 = 9900;

/// The message `AssociateProfile` refuses a further association with when the request
/// states no `Priority`, naming the VPC from the request and the bounds a priority
/// has to fall inside. Transcribed here for the same reason.
fn priority_required_message(resource_id: &str) -> String {
    format!(
        "VPC [{resource_id}] already has a Profile association. Specify a Priority between \
         {MIN_PRIORITY} and {MAX_PRIORITY} that no other Profile association for the VPC uses."
    )
}

/// The message a priority another association of the same VPC already holds is
/// refused with, naming the priority asked for, the VPC and the association already
/// evaluating at it.
fn priority_in_use_message(priority: i64, resource_id: &str, holder_id: &str) -> String {
    format!(
        "Priority {priority} is already used by Profile association \"{holder_id}\" for VPC \
         [{resource_id}]. Specify a priority that no other Profile association for the VPC uses."
    )
}

/// The message an associate that would rename or re-prioritise an association that
/// already exists is refused with, naming the ids from the request and the
/// association that holds the pair.
fn associate_cannot_change_message(
    profile_id: &str,
    resource_id: &str,
    association_id: &str,
) -> String {
    format!(
        "Profile [{profile_id}] is already associated with VPC [{resource_id}] by Profile \
         association \"{association_id}\". Use UpdateProfileAssociation to change the Name or \
         Priority of an existing Profile association."
    )
}

/// The `Priority` of the association a reply reports, which the model carries as an
/// integer.
fn reported_priority(reply: &super::harness::Reply) -> i64 {
    reply.member("ProfileAssociation")["Priority"]
        .as_i64()
        .unwrap_or_else(|| panic!("ProfileAssociation.Priority is not an integer in {:?}", reply.body))
}

#[tokio::test]
async fn an_association_is_placed_in_its_vpcs_evaluation_order_by_priority() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("priority-owner")
        .await
        .nested_string("Profile", "Id");

    // One row per class of `Priority` the model admits: no priority at all, which
    // `ProfileAssociation$Priority` documents as priority 100, each published bound of
    // the modelled range, and an interior value. Each row takes its OWN VPC, so the
    // rows say nothing about one another.
    let rows: [(Option<i64>, i64); 5] = [
        (None, MIN_PRIORITY),
        (Some(MIN_PRIORITY), MIN_PRIORITY),
        (Some(101), 101),
        (Some(5000), 5000),
        (Some(MAX_PRIORITY), MAX_PRIORITY),
    ];

    for (index, (requested, expected)) in rows.into_iter().enumerate() {
        let resource_id = format!("vpc-order{index}");
        let case = format!("requested={requested:?} resourceId={resource_id}");

        let created = em
            .associate_with_priority(&profile_id, &resource_id, &format!("order-{index}"), requested)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        assert_eq!(
            reported_priority(&created),
            expected,
            "case {case}: expected Priority {expected}, got {:?}",
            created.member("ProfileAssociation")
        );

        // Read back through the reader addressed by the minted id, and through the
        // listing narrowed to the VPC: both report the priority that was stored.
        let association_id = created.nested_string("ProfileAssociation", "Id");
        let fetched = em.get_association(&association_id).await;
        assert_eq!(
            reported_priority(&fetched),
            expected,
            "case {case}: GetProfileAssociation must report Priority {expected}, got {:?}",
            fetched.body
        );
        let listed = em
            .list_associations(&format!("?resourceId={resource_id}"))
            .await;
        let listed_priorities: Vec<&Value> = listed
            .list("ProfileAssociations")
            .iter()
            .filter(|association| association["Id"] == json!(association_id))
            .map(|association| &association["Priority"])
            .collect();
        assert_eq!(
            listed_priorities,
            vec![&json!(expected)],
            "case {case}: the listing must report Priority {expected} once, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_priority_outside_the_modelled_range_is_refused_and_persists_nothing() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("priority-bounds")
        .await
        .nested_string("Profile", "Id");

    // One past each published bound, and values far outside it on either side.
    for (index, priority) in [MIN_PRIORITY - 1, MAX_PRIORITY + 1, 0, -1, 1_000_000]
        .into_iter()
        .enumerate()
    {
        let resource_id = format!("vpc-bound{index}");
        let case = format!("priority={priority} resourceId={resource_id}");

        let refused = em
            .associate_with_priority(
                &profile_id,
                &resource_id,
                &format!("bound-{index}"),
                Some(priority),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: a priority outside the modelled range must be refused, got {} / {:?}",
            refused.status, refused.body
        );
        assert!(
            em.list_associations(&format!("?resourceId={resource_id}"))
                .await
                .list("ProfileAssociations")
                .is_empty(),
            "case {case}: a refused association must persist nothing"
        );
    }

    // And the same bound governs an update of an association that already exists.
    let standing = em
        .associate(&profile_id, "vpc-bound-standing", "standing")
        .await;
    let standing_id = standing.nested_string("ProfileAssociation", "Id");
    for priority in [MIN_PRIORITY - 1, MAX_PRIORITY + 1, 0] {
        let case = format!("update priority={priority}");
        let refused = em
            .update_association(&standing_id, json!({ "Priority": priority }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: a priority outside the modelled range must be refused, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            reported_priority(&em.get_association(&standing_id).await),
            MIN_PRIORITY,
            "case {case}: a refused update must leave the stored priority as it stood"
        );
    }
}

#[tokio::test]
async fn a_vpcs_associations_each_hold_a_priority_of_their_own() {
    let em = Emulator::new();
    let mut profiles: Vec<String> = Vec::new();
    for index in 0..3 {
        profiles.push(
            em.create_named(&format!("sharing-{index}"))
                .await
                .nested_string("Profile", "Id"),
        );
    }
    let resource_id = "vpc-shared01";

    // The first association with a VPC may be made without a priority.
    let first = em.associate(&profiles[0], resource_id, "first").await;
    assert_eq!(
        first.status, 200,
        "case first: AssociateProfile expected 200, got {} / {:?}",
        first.status, first.body
    );
    let first_id = first.nested_string("ProfileAssociation", "Id");
    assert_eq!(
        reported_priority(&first),
        MIN_PRIORITY,
        "case first: an association made without a Priority takes {MIN_PRIORITY}, got {:?}",
        first.member("ProfileAssociation")
    );

    // A further one has to say where in the VPC's evaluation order it goes.
    let silent = em.associate(&profiles[1], resource_id, "silent").await;
    assert_eq!(
        silent.error.as_deref(),
        Some("ValidationException"),
        "case silent: a further association with no Priority must be refused, got {} / {:?}",
        silent.status,
        silent.body
    );
    assert_eq!(
        silent.message(),
        priority_required_message(resource_id),
        "case silent: unexpected refusal message"
    );

    // And the priority it says has to be one no other association of that VPC holds.
    let taken = em
        .associate_with_priority(&profiles[1], resource_id, "taken", Some(MIN_PRIORITY))
        .await;
    assert_eq!(
        taken.error.as_deref(),
        Some("ValidationException"),
        "case taken: a priority another association of the VPC holds must be refused, \
         got {} / {:?}",
        taken.status,
        taken.body
    );
    assert_eq!(
        taken.message(),
        priority_in_use_message(MIN_PRIORITY, resource_id, &first_id),
        "case taken: unexpected refusal message"
    );

    // A free priority is accepted, and the VPC then reports both associations.
    let second = em
        .associate_with_priority(&profiles[1], resource_id, "second", Some(200))
        .await;
    assert_eq!(
        second.status, 200,
        "case second: AssociateProfile expected 200, got {} / {:?}",
        second.status, second.body
    );
    let second_id = second.nested_string("ProfileAssociation", "Id");
    let held = em
        .list_associations(&format!("?resourceId={resource_id}"))
        .await;
    let mut held_order: Vec<(i64, String)> = held
        .list("ProfileAssociations")
        .iter()
        .map(|association| {
            (
                association["Priority"].as_i64().unwrap_or_default(),
                association["Id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    held_order.sort();
    assert_eq!(
        held_order,
        vec![
            (MIN_PRIORITY, first_id.clone()),
            (200, second_id.clone())
        ],
        "case second: the VPC must hold both associations, each at its own priority, got {:?}",
        held.body
    );

    // Nothing but the refusals above touched the first association.
    assert_eq!(
        reported_priority(&em.get_association(&first_id).await),
        MIN_PRIORITY,
        "case second: a refused association must not have changed the stored one"
    );
}

#[tokio::test]
async fn an_associate_will_not_rename_or_re_prioritise_an_association_that_exists() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("repeat-owner")
        .await
        .nested_string("Profile", "Id");
    let resource_id = "vpc-repeat01";
    let created = em
        .associate_with_priority(&profile_id, resource_id, "original", Some(300))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: AssociateProfile expected 200, got {} / {:?}",
        created.status, created.body
    );
    let association_id = created.nested_string("ProfileAssociation", "Id");
    let stored = em
        .get_association(&association_id)
        .await
        .member("ProfileAssociation")
        .clone();

    // Rows differing in what the repeat asks for: the association exactly as it
    // stands, with its priority left unsaid, with another name, and with another
    // priority. Only a repeat that asks for no change reports the resource as
    // existing; one that asks for a change asks for the update operation's work.
    let rows: [(&str, Option<i64>, &str); 5] = [
        ("original", Some(300), "ResourceExistsException"),
        ("original", None, "ResourceExistsException"),
        ("renamed", Some(300), "ValidationException"),
        ("original", Some(400), "ValidationException"),
        ("renamed", None, "ValidationException"),
    ];

    for (name, priority, expected_error) in rows {
        let case = format!("name={name:?} priority={priority:?}");
        let repeat = em
            .associate_with_priority(&profile_id, resource_id, name, priority)
            .await;
        assert_eq!(
            repeat.error.as_deref(),
            Some(expected_error),
            "case {case}: expected {expected_error}, got {} / {:?}",
            repeat.status,
            repeat.body
        );
        if expected_error == "ValidationException" {
            assert_eq!(
                repeat.message(),
                associate_cannot_change_message(&profile_id, resource_id, &association_id),
                "case {case}: unexpected refusal message"
            );
        }
        assert_eq!(
            *em.get_association(&association_id)
                .await
                .member("ProfileAssociation"),
            stored,
            "case {case}: a refused repeat must leave the stored association as it stood"
        );
    }
}

#[tokio::test]
async fn an_updated_association_keeps_every_member_the_request_does_not_name() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("update-owner")
        .await
        .nested_string("Profile", "Id");

    // Rows differing in which members the update names, and in the value each names:
    // nothing at all, the name alone over the classes the modelled `Name` accepts,
    // the priority alone at each published bound, both together, and a priority the
    // association already evaluates at.
    let rows: [(Value, &str, i64, bool); 7] = [
        (json!({}), "original", 300, false),
        (json!({ "Name": "renamed" }), "renamed", 300, false),
        (json!({ "Name": "My_Assoc 2" }), "My_Assoc 2", 300, false),
        (json!({ "Name": "o'brien" }), "o'brien", 300, false),
        (json!({ "Priority": MIN_PRIORITY }), "original", MIN_PRIORITY, true),
        (json!({ "Priority": MAX_PRIORITY }), "original", MAX_PRIORITY, true),
        (
            json!({ "Name": "both", "Priority": 500 }),
            "both",
            500,
            true,
        ),
    ];

    for (index, (body, expected_name, expected_priority, changes_order)) in
        rows.into_iter().enumerate()
    {
        let resource_id = format!("vpc-update{index}");
        let case = format!("body={body} resourceId={resource_id}");
        let created = em
            .associate_with_priority(&profile_id, &resource_id, "original", Some(300))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        let association_id = created.nested_string("ProfileAssociation", "Id");

        let updated = em.update_association(&association_id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: UpdateProfileAssociation expected 200, got {} / {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.nested_string("ProfileAssociation", "Name"),
            expected_name,
            "case {case}: unexpected Name in {:?}",
            updated.member("ProfileAssociation")
        );
        assert_eq!(
            reported_priority(&updated),
            expected_priority,
            "case {case}: unexpected Priority in {:?}",
            updated.member("ProfileAssociation")
        );
        // Every member the request did not name is the one the record already held.
        for member in ["Id", "ProfileId", "ResourceId", "OwnerId", "CreationTime"] {
            assert_eq!(
                updated.member("ProfileAssociation")[member],
                created.member("ProfileAssociation")[member],
                "case {case}: {member} must be left as it stood, got {:?}",
                updated.member("ProfileAssociation")
            );
        }
        for member in ["CreationTime", "ModificationTime"] {
            assert_timestamp_form(&case, member, &updated.member("ProfileAssociation")[member]);
        }

        // A CHANGED priority has to propagate before it applies to every query from
        // the VPC, so the association is reported as updating until the move the clock
        // owes it falls due; anything else leaves the status where it stood.
        let (expected_status, expected_message) = if changes_order {
            ("UPDATING", "Updating Profile Association")
        } else {
            ("COMPLETE", "Created Profile Association")
        };
        assert_eq!(
            updated.nested_string("ProfileAssociation", "Status"),
            expected_status,
            "case {case}: unexpected Status in {:?}",
            updated.member("ProfileAssociation")
        );
        assert_eq!(
            updated.nested_string("ProfileAssociation", "StatusMessage"),
            expected_message,
            "case {case}: unexpected StatusMessage in {:?}",
            updated.member("ProfileAssociation")
        );

        // The reader reports the record the update left behind, propagation and all,
        // and once the owed move falls due the association is in force again with the
        // values the update stored.
        let read_back = em.get_association(&association_id).await;
        assert_eq!(
            read_back.nested_string("ProfileAssociation", "Status"),
            expected_status,
            "case {case}: the reader must report the stored phase, got {:?}",
            read_back.body
        );
        if changes_order {
            em.settle_association(&association_id);
        }
        let settled = em.get_association(&association_id).await;
        assert_eq!(
            settled.nested_string("ProfileAssociation", "Status"),
            "COMPLETE",
            "case {case}: a settled association is COMPLETE, got {:?}",
            settled.body
        );
        assert_eq!(
            settled.nested_string("ProfileAssociation", "Name"),
            expected_name,
            "case {case}: the settled record must keep the updated Name, got {:?}",
            settled.body
        );
        assert_eq!(
            reported_priority(&settled),
            expected_priority,
            "case {case}: the settled record must keep the updated Priority, got {:?}",
            settled.body
        );
    }
}

#[tokio::test]
async fn an_update_is_refused_a_priority_another_association_of_the_vpc_holds() {
    let em = Emulator::new();
    let holder_profile = em
        .create_named("update-holder")
        .await
        .nested_string("Profile", "Id");
    let rival_profile = em
        .create_named("update-rival")
        .await
        .nested_string("Profile", "Id");
    let resource_id = "vpc-reorder1";

    let first = em
        .associate_with_priority(&holder_profile, resource_id, "first", Some(MIN_PRIORITY))
        .await;
    let first_id = first.nested_string("ProfileAssociation", "Id");
    let second = em
        .associate_with_priority(&rival_profile, resource_id, "second", Some(200))
        .await;
    let second_id = second.nested_string("ProfileAssociation", "Id");
    assert_eq!(
        second.status, 200,
        "setup: the second association expected 200, got {} / {:?}",
        second.status, second.body
    );

    // The priority the OTHER association of this VPC holds is refused, and the record
    // is left exactly as it stood.
    let refused = em
        .update_association(&second_id, json!({ "Priority": MIN_PRIORITY }))
        .await;
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case taken: expected ValidationException, got {} / {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.message(),
        priority_in_use_message(MIN_PRIORITY, resource_id, &first_id),
        "case taken: unexpected refusal message"
    );
    let unchanged = em.get_association(&second_id).await;
    assert_eq!(
        reported_priority(&unchanged),
        200,
        "case taken: a refused update must leave the stored priority as it stood"
    );
    assert_eq!(
        unchanged.nested_string("ProfileAssociation", "Status"),
        "COMPLETE",
        "case taken: a refused update must leave the stored status as it stood, got {:?}",
        unchanged.body
    );

    // An association never conflicts with ITSELF, so re-stating the priority it
    // already evaluates at changes nothing and moves no status.
    let restated = em
        .update_association(&second_id, json!({ "Priority": 200 }))
        .await;
    assert_eq!(
        restated.status, 200,
        "case restated: UpdateProfileAssociation expected 200, got {} / {:?}",
        restated.status, restated.body
    );
    assert_eq!(
        restated.nested_string("ProfileAssociation", "Status"),
        "COMPLETE",
        "case restated: re-stating the held priority moves no status, got {:?}",
        restated.body
    );

    // The priority the other association gives up is then free for this one.
    let released = em
        .update_association(&first_id, json!({ "Priority": 300 }))
        .await;
    assert_eq!(
        released.status, 200,
        "case released: UpdateProfileAssociation expected 200, got {} / {:?}",
        released.status, released.body
    );
    let reordered = em
        .update_association(&second_id, json!({ "Priority": MIN_PRIORITY }))
        .await;
    assert_eq!(
        reordered.status, 200,
        "case reordered: a freed priority must be accepted, got {} / {:?}",
        reordered.status, reordered.body
    );
    assert_eq!(
        reported_priority(&reordered),
        MIN_PRIORITY,
        "case reordered: unexpected Priority in {:?}",
        reordered.member("ProfileAssociation")
    );

    // A `Name` outside the modelled pattern is refused, and persists nothing.
    em.settle_association(&second_id);
    let stored = em
        .get_association(&second_id)
        .await
        .member("ProfileAssociation")
        .clone();
    for name in ["", "12345", "no/slashes", "tab\there"] {
        let case = format!("name={name:?}");
        let refused = em
            .update_association(&second_id, json!({ "Name": name }))
            .await;
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a Name outside the modelled pattern must be refused, got {} / {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            *em.get_association(&second_id)
                .await
                .member("ProfileAssociation"),
            stored,
            "case {case}: a refused update must persist nothing"
        );
    }

    // An id that names no association names nothing to update, and the id of one that
    // has been removed is such an id.
    em.disassociate(&rival_profile, resource_id).await;
    for id in [
        second_id.as_str(),
        "rpassoc-0123456789abcdef",
        "nosuchassociation",
    ] {
        let case = format!("id={id:?}");
        let refused = em
            .update_association(id, json!({ "Priority": 400 }))
            .await;
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: an id that names nothing must be ResourceNotFoundException, \
             got {} / {:?}",
            refused.status,
            refused.body
        );
    }
}
