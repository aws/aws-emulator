// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SHAPE of what this service's success bodies carry,
//! and for where the `external` flag of an association comes from.
//!
//! The three rules under test, each asserted over a table rather than one case:
//!
//! * **A tag MUTATION answers with an empty body; a tag reported as part of a
//!   resource's state carries the modelled pair.** `TagResourceResponse` and
//!   `UntagResourceResponse` declare no members
//!   (`smithy_query(query="shape",shape="TagResourceResponse")`), so a successful
//!   mutation carries nothing, and `ResourceShare$tags` reports every stored pair with
//!   exactly the members `Tag` declares (`smithy_query(query="shape",shape="Tag")`:
//!   exactly `key` and `value`) and no structure discriminator.
//! * **Every managed-permission summary a response carries reports exactly the members
//!   `ResourceSharePermissionSummary` declares, from the stored permission.** The
//!   summary's members are the ones
//!   `smithy_query(query="shape",shape="ResourceSharePermissionSummary")` declares and no
//!   other, and each value is read off the stored permission, so every operation that
//!   projects a summary reports the same values for one permission and a read never moves
//!   them.
//! * **`external` is derived from the entity and the share's owning account, and the
//!   teardown report carries false.** `ResourceShareAssociation$external` "indicates
//!   whether the principal belongs to the same organization in Organizations as the
//!   Amazon Web Services account that owns the resource share", so for every operation
//!   that reports an association still participating in the share
//!   (`AssociateResourceShare`, `GetResourceShareAssociations`) it follows the entity:
//!   the owning account and anything inside the owner's Organizations hierarchy are
//!   in-organization, any other account is external.
//!   `DisassociateResourceShare` reports the association it is REMOVING — the call
//!   leaves no principal participating in the share, so there is none to place in the
//!   owner's organization and the flag on its records is false whatever the entity,
//!   while the stored derivation is untouched and a later read reports it again. The
//!   second `external` test pins the other half of the rule — the stored flag is a
//!   property of the ENTITY and the OWNER alone, so neither the share's
//!   `allowExternalPrincipals` switch (before or after an update flips it) nor a repeat
//!   of the association moves it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, AWS_MANAGED_PERMISSIONS,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

// ---------------------------------------------------------------------------
// A success body carries exactly the modelled members
// ---------------------------------------------------------------------------

/// A row of the tag-serialization table: the tags a share is created with, a tag
/// mutation to report, and a key to detach.
struct TagShapeCase {
    case: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    add: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
}

/// Every member name `value` carries, at any depth.
fn member_names(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                into.insert(name.clone());
                member_names(child, into);
            }
        }
        Value::Array(items) => {
            for item in items {
                member_names(item, into);
            }
        }
        _ => {}
    }
}

/// Assert no member of a body reporting a resource's own state is a type
/// discriminator: a member whose name starts with `__`. Every body this service answers
/// carries modelled members alone.
fn assert_no_discriminator(label: &str, body: &Value) {
    let mut names = BTreeSet::new();
    member_names(body, &mut names);
    let discriminators: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("__"))
        .collect();
    assert!(
        discriminators.is_empty(),
        "case {label:?}: a body reporting stored state must carry only modelled members, \
         got the discriminator(s) {discriminators:?} in {body:?}"
    );
}

/// The member names of one reported entry, as the wire carried them.
fn members_of(label: &str, entry: &Value) -> BTreeSet<String> {
    entry
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {entry:?}"))
        .keys()
        .cloned()
        .collect()
}

/// The entries of a reported tag list.
fn list_of(label: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: response carried no {member}, got {body:?}"))
}

/// The `key`/`value` members the modelled `Tag` structure declares
/// (`smithy_query(query="shape",shape="Tag")`), as a set.
fn modelled_tag_members() -> BTreeSet<String> {
    ["key", "value"].iter().map(|m| (*m).to_string()).collect()
}

#[tokio::test]
async fn a_tag_mutation_answers_empty_while_a_share_reports_plain_pairs() {
    let router = emulator();

    // The rule generalises over the tag PAIR, so the rows differ along the form of the
    // key and the value: a plain pair, an empty value ("The value can be an empty
    // string" — `Tag$value`), two keys differing only in case ("Key names are case
    // sensitive" — `Tag$key`), several pairs at once, and a pair whose characters JSON
    // must escape or encode.
    let cases = vec![
        TagShapeCase {
            case: "one plain pair on an untagged share",
            initial: vec![],
            add: vec![("cost-center", "42")],
            remove: vec!["cost-center"],
        },
        TagShapeCase {
            case: "an empty value",
            initial: vec![("team", "platform")],
            add: vec![("owner", "")],
            remove: vec!["owner"],
        },
        TagShapeCase {
            case: "keys differing only in case",
            initial: vec![("Team", "platform")],
            add: vec![("team", "network")],
            remove: vec!["Team"],
        },
        TagShapeCase {
            case: "several pairs in one call",
            initial: vec![("env", "test")],
            add: vec![("a", "1"), ("b", "2"), ("c", "3")],
            remove: vec!["a", "c"],
        },
        TagShapeCase {
            case: "characters JSON must escape or encode",
            initial: vec![],
            add: vec![
                ("quote\"and\\slash", "line\nbreak"),
                ("ключ", "значение"),
                ("emoji-🙂", "值"),
            ],
            remove: vec!["ключ"],
        },
    ];

    for TagShapeCase {
        case,
        initial,
        add,
        remove,
    } in cases
    {
        let initial_tags: Vec<Value> = initial
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let (status, created_body) = create(
            &router,
            &json!({
                "name": format!("tag-shape-{}", case.replace(' ', "-")),
                "tags": initial_tags,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with {created_body:?}"
        );
        assert_no_discriminator(case, &created_body);
        let created = created_body
            .get("resourceShare")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: create reported no share: {created_body:?}"));
        for entry in list_of(case, &created, "tags") {
            assert_eq!(
                members_of(case, &entry),
                modelled_tag_members(),
                "case {case:?}: a tag the create reported carries unmodelled members: {entry:?}"
            );
        }

        let arn = arn_of(case, &created);

        // TagResource answers with nothing; what it attached is the share's own tag set,
        // where the SAME pair is reported with the modelled members alone.
        let tags: Vec<Value> = add
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let (status, tagged) =
            tag_resource(&router, &json!({ "resourceShareArn": arn, "tags": tags })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} with {tagged:?}"
        );
        assert_eq!(
            tagged,
            json!({}),
            "case {case:?}: TagResource declares no response members, so the body must be \
             empty, got {tagged:?}"
        );

        let (status, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShares expected 200, got {status} with {listed:?}"
        );
        assert_no_discriminator(case, &listed);
        let share = only_share(case, &listed);
        let share_entries = list_of(case, &share, "tags");
        for entry in &share_entries {
            assert_eq!(
                members_of(case, entry),
                modelled_tag_members(),
                "case {case:?}: a tag inside the share carries unmodelled members: {entry:?}"
            );
        }
        for (key, value) in &add {
            assert!(
                share_entries.contains(&json!({ "key": key, "value": value })),
                "case {case:?}: the pair {key:?}={value:?} TagResource attached is not the \
                 object the share carries: {share_entries:?}"
            );
        }

        // UntagResource answers with nothing either, and the keys it named are gone from
        // the share while every other pair stays.
        let kept: Vec<Value> = share_entries
            .iter()
            .filter(|entry| {
                entry
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|key| !remove.contains(&key))
            })
            .cloned()
            .collect();
        let (status, untagged) = untag_resource(
            &router,
            &json!({ "resourceShareArn": arn, "tagKeys": remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} with {untagged:?}"
        );
        assert_eq!(
            untagged,
            json!({}),
            "case {case:?}: UntagResource declares no response members, so the body must be \
             empty, got {untagged:?}"
        );
        let (status, after) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read after the removal expected 200, got {status} with \
             {after:?}"
        );
        assert_eq!(
            list_of(case, &only_share(case, &after), "tags"),
            kept,
            "case {case:?}: expected the pairs the removal left behind, got {after:?}"
        );
    }

    // The other side of the same rule: a REFUSED tag mutation answers in the restJson1
    // error envelope rather than with a success body.
    let (status, refused) = tag_resource(
        &router,
        &json!({
            "resourceShareArn": format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-0000-0000-000000000000"),
            "tags": [{ "key": "cost-center", "value": "42" }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"tagging an unknown share\": expected 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case \"tagging an unknown share\": wrong error code in {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// `external` is one derived value per association record
// ---------------------------------------------------------------------------

/// A row of the `external` table: one entity, and whether the share's owner can place
/// it inside its own organization.
struct ExternalCase {
    case: &'static str,
    association_type: &'static str,
    entity: &'static str,
    external: bool,
}

/// The entity forms `ResourceShareAssociation$associatedEntity` names, each with the
/// value the documented rule gives `external` for a share owned by [`ACCOUNT`]: the
/// owning account and anything inside its Organizations hierarchy are in-organization,
/// and an entity this service cannot place there is external.
fn external_cases() -> Vec<ExternalCase> {
    vec![
        ExternalCase {
            case: "the owning account itself",
            association_type: "PRINCIPAL",
            entity: ACCOUNT,
            external: false,
        },
        ExternalCase {
            case: "another account",
            association_type: "PRINCIPAL",
            entity: "111122223333",
            external: true,
        },
        ExternalCase {
            case: "a role in the owning account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::000000000000:role/local-consumer",
            external: false,
        },
        ExternalCase {
            case: "a role in another account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::777788889999:role/consumer",
            external: true,
        },
        ExternalCase {
            case: "an organization id",
            association_type: "PRINCIPAL",
            entity: "o-abc123exam",
            external: false,
        },
        ExternalCase {
            case: "an organizational unit id",
            association_type: "PRINCIPAL",
            entity: "ou-ab12-cd34efgh",
            external: false,
        },
        ExternalCase {
            case: "an organization ARN in the management account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:organization/o-abc123exam",
            external: false,
        },
        ExternalCase {
            case: "an organizational-unit ARN in the management account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:ou/o-abc123exam/ou-ab12-cd34efgh",
            external: false,
        },
        ExternalCase {
            case: "an Organizations account ARN",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:account/o-abc123exam/999988887777",
            external: false,
        },
        // The third part of an Organizations hierarchy a RAM principal may name, beside
        // the organization and the organizational unit: a ROOT, by its bare `r-` id or
        // by its `organizations` ARN (`state::ORGANIZATIONS_ID_PREFIXES`, which lists
        // `ou-`, `o-` and `r-`). Inside the owner's hierarchy, hence in-organization.
        ExternalCase {
            case: "an organization root id",
            association_type: "PRINCIPAL",
            entity: "r-ab12",
            external: false,
        },
        ExternalCase {
            case: "an organization root ARN in the management account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:root/o-abc123exam/r-ab12",
            external: false,
        },
        ExternalCase {
            case: "a service principal",
            association_type: "PRINCIPAL",
            entity: "lambda.amazonaws.com",
            external: true,
        },
        ExternalCase {
            case: "a resource in the owning account",
            association_type: "RESOURCE",
            entity: LOCAL_RESOURCE,
            external: false,
        },
        ExternalCase {
            case: "a resource in another account",
            association_type: "RESOURCE",
            entity: FOREIGN_RESOURCE,
            external: true,
        },
        ExternalCase {
            case: "a source account inside the owning account",
            association_type: "SOURCE",
            entity: ACCOUNT,
            external: false,
        },
        ExternalCase {
            case: "a source account elsewhere",
            association_type: "SOURCE",
            entity: "111122223333",
            external: true,
        },
    ]
}

/// A request body naming `entity` under the list its association type is taken from.
fn entity_request(association_type: &str, entity: &str, arn: &str) -> Value {
    let member = match association_type {
        "RESOURCE" => "resourceArns",
        "SOURCE" => "sources",
        _ => "principals",
    };
    json!({ "resourceShareArn": arn, member: [entity] })
}

/// The one association a response reports for `entity`.
fn association_for(label: &str, body: &Value, entity: &str) -> Value {
    associations_of(label, body)
        .into_iter()
        .find(|association| {
            association.get("associatedEntity").and_then(Value::as_str) == Some(entity)
        })
        .unwrap_or_else(|| panic!("case {label:?}: no association reported for {entity}: {body:?}"))
}

/// The `external` member of a reported association.
fn external_of(label: &str, association: &Value) -> bool {
    association
        .get("external")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {label:?}: association reported no external: {association:?}"))
}

#[tokio::test]
async fn external_is_derived_from_the_owning_organization_and_the_teardown_reports_false() {
    let router = emulator();

    for (index, row) in external_cases().into_iter().enumerate() {
        let ExternalCase {
            case,
            association_type,
            entity,
            external,
        } = row;

        let created = create_ok(
            &router,
            case,
            &json!({ "name": format!("external-{index}") }),
        )
        .await;
        let arn = arn_of(case, &created);
        let request = entity_request(association_type, entity, &arn);

        // 1. The call that makes the association.
        let (status, associated) = associate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateResourceShare expected 200, got {status} with {associated:?}"
        );
        let on_associate = external_of(case, &association_for(case, &associated, entity));
        assert_eq!(
            on_associate, external,
            "case {case:?}: AssociateResourceShare expected external {external}, \
             got {on_associate} for {entity}"
        );

        // 2. The read of the stored record.
        let (status, read) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShareAssociations expected 200, got {status} with {read:?}"
        );
        let on_read = external_of(case, &association_for(case, &read, entity));
        assert_eq!(
            on_read, external,
            "case {case:?}: GetResourceShareAssociations expected external {external}, \
             got {on_read} for {entity}"
        );

        // 3. The call that REMOVES it: the entity stops participating in the share, so
        // there is no principal left to place in the owner's organization and this
        // report carries false for every row of the table.
        let (status, disassociated) = disassociate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateResourceShare expected 200, got {status} \
             with {disassociated:?}"
        );
        let on_disassociate = external_of(case, &association_for(case, &disassociated, entity));
        assert!(
            !on_disassociate,
            "case {case:?}: DisassociateResourceShare expected external false, \
             got {on_disassociate} for {entity}"
        );

        // And the teardown left the STORED flag where a later read reports it again:
        // the removal report does not rewrite the record.
        let (_, after) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        let after_teardown = external_of(case, &association_for(case, &after, entity));
        assert_eq!(
            after_teardown, external,
            "case {case:?}: the read after the teardown expected external {external}, \
             got {after_teardown}"
        );
    }
}

// ---------------------------------------------------------------------------
// …and of the entity and the owner ALONE
// ---------------------------------------------------------------------------

/// A row of the "nothing else moves the flag" table: an entity, the
/// `allowExternalPrincipals` the share is created with, and the value the documented
/// rule gives `external` for a share owned by [`ACCOUNT`].
///
/// `ResourceShare$allowExternalPrincipals` is the share-level switch for WHETHER
/// principals outside the owner's organization may be associated at all ("Indicates
/// whether principals outside your organization in Organizations can be associated
/// with a resource share"); it says nothing about where an already-associated
/// principal sits, so it cannot move `external`.
struct ExternalInvariantCase {
    case: &'static str,
    entity: &'static str,
    allow_external_principals: Option<bool>,
    external: bool,
}

/// Whether a reported share allows external principals.
fn allows_external_principals(label: &str, share: &Value) -> bool {
    share
        .get("allowExternalPrincipals")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            panic!("case {label:?}: share reported no allowExternalPrincipals: {share:?}")
        })
}

#[tokio::test]
async fn external_is_a_property_of_the_entity_and_the_owner_alone() {
    let router = emulator();

    // The rows span the entity forms on one axis and the share's external-sharing
    // switch on the other — including the forms the cited case did not use (an
    // Organizations root, a service principal) and a share created with the switch
    // explicitly off, which the default (`true`, from `allowExternalPrincipals`'s own
    // documentation) never exercises.
    let cases = vec![
        ExternalInvariantCase {
            case: "the owning account on a share that allows external principals",
            entity: ACCOUNT,
            allow_external_principals: Some(true),
            external: false,
        },
        ExternalInvariantCase {
            case: "the owning account on a share that forbids them",
            entity: ACCOUNT,
            allow_external_principals: Some(false),
            external: false,
        },
        ExternalInvariantCase {
            case: "another account on a share that forbids external principals",
            entity: "111122223333",
            allow_external_principals: Some(false),
            external: true,
        },
        ExternalInvariantCase {
            case: "another account on a share created with the default switch",
            entity: "111122223333",
            allow_external_principals: None,
            external: true,
        },
        ExternalInvariantCase {
            case: "an organization id on a share that forbids external principals",
            entity: "o-abc123exam",
            allow_external_principals: Some(false),
            external: false,
        },
        ExternalInvariantCase {
            case: "an organization root id on a share that allows them",
            entity: "r-ab12",
            allow_external_principals: Some(true),
            external: false,
        },
        ExternalInvariantCase {
            case: "a role in another account on a share that forbids them",
            entity: "arn:aws:iam::777788889999:role/consumer",
            allow_external_principals: Some(false),
            external: true,
        },
        ExternalInvariantCase {
            case: "a service principal on a share created with the default switch",
            entity: "lambda.amazonaws.com",
            allow_external_principals: None,
            external: true,
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let ExternalInvariantCase {
            case,
            entity,
            allow_external_principals,
            external,
        } = row;

        let mut create_body = json!({ "name": format!("external-invariant-{index}") });
        if let Some(allow) = allow_external_principals {
            create_body["allowExternalPrincipals"] = json!(allow);
        }
        let created = create_ok(&router, case, &create_body).await;
        let arn = arn_of(case, &created);
        let created_allows = allows_external_principals(case, &created);
        assert_eq!(
            created_allows,
            allow_external_principals.unwrap_or(true),
            "case {case:?}: the share did not keep the external-sharing switch it was \
             created with, got {created_allows}"
        );
        let request = json!({ "resourceShareArn": arn, "principals": [entity] });

        let (status, associated) = associate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateResourceShare expected 200, got {status} with {associated:?}"
        );
        assert_no_discriminator(case, &associated);
        let on_associate = external_of(case, &association_for(case, &associated, entity));
        assert_eq!(
            on_associate, external,
            "case {case:?}: AssociateResourceShare expected external {external}, \
             got {on_associate} for {entity}"
        );

        // Flip the share's external-sharing switch. The share records the flip…
        let (status, updated) = update(
            &router,
            &json!({ "resourceShareArn": arn, "allowExternalPrincipals": !created_allows }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateResourceShare expected 200, got {status} with {updated:?}"
        );
        let after_flip = updated
            .get("resourceShare")
            .map(|share| allows_external_principals(case, share))
            .unwrap_or_else(|| panic!("case {case:?}: update reported no share: {updated:?}"));
        assert_eq!(
            after_flip, !created_allows,
            "case {case:?}: expected allowExternalPrincipals {} after the flip, got {after_flip}",
            !created_allows
        );
        // …and the association's own flag does not move with it.
        let read_request = json!({ "associationType": "PRINCIPAL", "resourceShareArns": [arn] });
        let (status, read) = get_associations(&router, &read_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShareAssociations expected 200, got {status} with {read:?}"
        );
        assert_no_discriminator(case, &read);
        let on_read = external_of(case, &association_for(case, &read, entity));
        assert_eq!(
            on_read, external,
            "case {case:?}: after flipping allowExternalPrincipals the read expected \
             external {external}, got {on_read} for {entity}"
        );

        // Re-associating the same entity re-derives the same flag from the same owner.
        let (status, again) = associate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated AssociateResourceShare expected 200, got {status} \
             with {again:?}"
        );
        let on_repeat = external_of(case, &association_for(case, &again, entity));
        assert_eq!(
            on_repeat, external,
            "case {case:?}: the repeated association expected external {external}, \
             got {on_repeat} for {entity}"
        );

        // And the teardown reports the record it tore down with the flag the removal
        // carries, whatever the entity and whatever the share's switch now says.
        let (status, disassociated) = disassociate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateResourceShare expected 200, got {status} \
             with {disassociated:?}"
        );
        assert_no_discriminator(case, &disassociated);
        let on_disassociate = external_of(case, &association_for(case, &disassociated, entity));
        assert!(
            !on_disassociate,
            "case {case:?}: DisassociateResourceShare expected external false, \
             got {on_disassociate} for {entity}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every managed-permission summary carries exactly the modelled members
// ---------------------------------------------------------------------------

/// The members `ResourceSharePermissionSummary` declares, from
/// `smithy_query(query="shape",shape="ResourceSharePermissionSummary")`. A summary may
/// leave a member out, but may carry no member outside this list.
const SUMMARY_MEMBERS: [&str; 14] = [
    "arn",
    "version",
    "defaultVersion",
    "name",
    "resourceType",
    "status",
    "creationTime",
    "lastUpdatedTime",
    "isResourceTypeDefault",
    "permissionType",
    "featureSet",
    "permissionTemplateParameters",
    "tags",
    "resourceOwner",
];

/// The resource type the share below shares, and the type whose default managed
/// permission the second test writes the restriction onto.
const SUBNET_RESOURCE_TYPE: &str = "ec2:Subnet";

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissions", Some(body)).await
}

async fn list_permission_versions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpermissionversions", Some(body)).await
}

async fn list_share_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

/// The summaries a response carries: the `permissions` list of a read, or the single
/// `permission` a create reported.
fn summaries_of(case: &str, body: &Value) -> Vec<Value> {
    if let Some(list) = body.get("permissions").and_then(Value::as_array) {
        return list.clone();
    }
    body.get("permission")
        .cloned()
        .map(|permission| vec![permission])
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permission, got {body:?}"))
}

/// The ARN of a reported summary.
fn arn_of_summary(case: &str, summary: &Value) -> String {
    summary
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a summary carried no arn, got {summary:?}"))
        .to_string()
}

/// The restriction one reported summary carries. A summary that carries no such member,
/// or carries something that is not a boolean, fails the case: AWS sends a boolean on
/// every element.
fn resource_type_default_of(case: &str, summary: &Value) -> bool {
    let reported = summary.get("isResourceTypeDefault").unwrap_or_else(|| {
        panic!("case {case:?}: expected an isResourceTypeDefault member, got {summary:?}")
    });
    reported.as_bool().unwrap_or_else(|| {
        panic!(
            "case {case:?}: isResourceTypeDefault must be a boolean, got {reported:?} in \
             {summary:?}"
        )
    })
}

/// Every reported summary's `(arn, isResourceTypeDefault)` pair, asserting each one
/// carries only members the shape declares.
fn summary_flags_of(case: &str, body: &Value) -> Vec<(String, bool)> {
    let summaries = summaries_of(case, body);
    assert!(
        !summaries.is_empty(),
        "case {case:?}: expected at least one reported permission, got {body:?}"
    );
    let declared: BTreeSet<String> = SUMMARY_MEMBERS.iter().map(|m| (*m).to_string()).collect();
    summaries
        .iter()
        .map(|summary| {
            let members = members_of(case, summary);
            assert!(
                members.is_subset(&declared),
                "case {case:?}: a summary carries a member the shape does not declare: \
                 {summary:?}"
            );
            (
                arn_of_summary(case, summary),
                resource_type_default_of(case, summary),
            )
        })
        .collect()
}

/// A call that must answer 200, with the body it answered.
fn ok(case: &str, (status, body): (u16, Value)) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body
}

#[tokio::test]
async fn every_operation_that_reports_a_permission_summary_reports_modelled_members() {
    let router = emulator();

    // A customer managed permission beside the catalog, so the rows span both
    // populations the store holds rather than only the one the recording read.
    let minted = ok(
        "CreatePermission",
        create_permission(
            &router,
            &json!({
                "name": "RestrictionProbe",
                "resourceType": "codebuild:Project",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#,
            }),
        )
        .await,
    );
    let minted_arn = arn_of_summary("CreatePermission", &summaries_of("CreatePermission", &minted)[0]);

    // A share of a resource whose type the catalog publishes a default permission for,
    // so the share-scoped read has an attachment to project.
    let share = create_ok(
        &router,
        "a share to read the attached permission of",
        &json!({"name": "restriction-probe-share", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    let share_arn = arn_of("a share to read the attached permission of", &share);

    // The rows: every operation that projects a summary, over both populations — the
    // whole AWS managed catalog unfiltered, one catalog entry's versions, a created
    // permission and its versions, and the share-scoped read of an attachment.
    let catalog_arn = aws_managed_permission_arn(AWS_MANAGED_PERMISSIONS[0].name);
    let rows: Vec<(&str, Value)> = vec![
        (
            "CreatePermission reports the permission it minted",
            minted.clone(),
        ),
        (
            "ListPermissions over the whole store",
            ok(
                "ListPermissions over the whole store",
                list_permissions(&router, &json!({})).await,
            ),
        ),
        (
            "ListPermissions filtered to the AWS managed catalog",
            ok(
                "ListPermissions filtered to the AWS managed catalog",
                list_permissions(&router, &json!({"permissionType": "AWS_MANAGED"})).await,
            ),
        ),
        (
            "ListPermissions filtered to one resource type",
            ok(
                "ListPermissions filtered to one resource type",
                list_permissions(&router, &json!({"resourceType": SUBNET_RESOURCE_TYPE})).await,
            ),
        ),
        (
            "ListPermissionVersions of a catalog entry",
            ok(
                "ListPermissionVersions of a catalog entry",
                list_permission_versions(&router, &json!({"permissionArn": catalog_arn})).await,
            ),
        ),
        (
            "ListPermissionVersions of a created permission",
            ok(
                "ListPermissionVersions of a created permission",
                list_permission_versions(&router, &json!({"permissionArn": minted_arn.clone()}))
                    .await,
            ),
        ),
        (
            "ListResourceSharePermissions of a share",
            ok(
                "ListResourceSharePermissions of a share",
                list_share_permissions(&router, &json!({"resourceShareArn": share_arn})).await,
            ),
        ),
    ];

    for (case, body) in rows {
        assert_no_discriminator(case, &body);
        // A permission this account created is not the default for its resource type,
        // and a catalog entry is one exactly when the library publishes it as one, so
        // every reported flag is the stored permission's own.
        for (arn, is_default) in summary_flags_of(case, &body) {
            let expected = AWS_MANAGED_PERMISSIONS
                .iter()
                .find(|entry| aws_managed_permission_arn(entry.name) == arn)
                .map(|entry| entry.is_resource_type_default)
                .unwrap_or(false);
            assert_eq!(
                is_default, expected,
                "case {case:?}: expected isResourceTypeDefault {expected} for {arn}, got \
                 {is_default}"
            );
        }
    }

    // The unfiltered read covers every catalog entry, not only the first: each one
    // carries the member.
    let listed = ok(
        "ListPermissions covers the catalog",
        list_permissions(&router, &json!({})).await,
    );
    let reported: Vec<String> = summary_flags_of("ListPermissions covers the catalog", &listed)
        .into_iter()
        .map(|(arn, _)| arn)
        .collect();
    for entry in AWS_MANAGED_PERMISSIONS {
        let arn = aws_managed_permission_arn(entry.name);
        assert!(
            reported.contains(&arn),
            "case {:?}: the unfiltered read must report this catalog entry, got {reported:?}",
            entry.name
        );
    }
    assert!(
        reported.contains(&minted_arn),
        "case \"ListPermissions covers the catalog\": the created permission must be reported \
         too, got {reported:?}"
    );
}

#[tokio::test]
async fn the_reported_summary_follows_the_stored_permission() {
    let (router, engine) = emulator_with_engine();

    let minted = ok(
        "mint a permission to flag",
        create_permission(
            &router,
            &json!({
                "name": "RestrictedProbe",
                "resourceType": "codebuild:Project",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#,
            }),
        )
        .await,
    );
    let minted_arn = arn_of_summary(
        "mint a permission to flag",
        &summaries_of("mint a permission to flag", &minted)[0],
    );

    // The default managed permission of the type the share below shares, so the
    // share-scoped read projects a flagged permission too.
    let subnet_default = AWS_MANAGED_PERMISSIONS
        .iter()
        .find(|entry| entry.resource_type == SUBNET_RESOURCE_TYPE && entry.is_resource_type_default)
        .expect("the catalog publishes a default permission for the type the share shares");
    let subnet_default_arn = aws_managed_permission_arn(subnet_default.name);

    // The resource-type default is a property of the stored permission, so writing it
    // onto two records — one of each population — is what a read must then report.
    let defaulted = [minted_arn.clone(), subnet_default_arn.clone()];
    {
        let state = store(&engine);
        let mut locked = state.lock();
        for arn in &defaulted {
            let permission = locked
                .permissions
                .get_mut(arn)
                .unwrap_or_else(|| panic!("the store holds the permission {arn}"));
            permission.is_resource_type_default = true;
        }
    }

    let share = create_ok(
        &router,
        "a share governed by a flagged permission",
        &json!({"name": "restricted-probe-share", "resourceArns": [LOCAL_RESOURCE]}),
    )
    .await;
    let share_arn = arn_of("a share governed by a flagged permission", &share);

    // Each read reports true for exactly the two permissions carrying the flag and the
    // stored value for every other, so the value follows the state and not the caller.
    let reads: Vec<(&str, Value)> = vec![
        (
            "ListPermissions after the flag was written",
            ok(
                "ListPermissions after the flag was written",
                list_permissions(&router, &json!({})).await,
            ),
        ),
        (
            "ListPermissionVersions of the flagged catalog entry",
            ok(
                "ListPermissionVersions of the flagged catalog entry",
                list_permission_versions(
                    &router,
                    &json!({"permissionArn": subnet_default_arn.clone()}),
                )
                .await,
            ),
        ),
        (
            "ListPermissionVersions of the flagged created permission",
            ok(
                "ListPermissionVersions of the flagged created permission",
                list_permission_versions(&router, &json!({"permissionArn": minted_arn.clone()}))
                    .await,
            ),
        ),
        (
            "ListResourceSharePermissions of the share it governs",
            ok(
                "ListResourceSharePermissions of the share it governs",
                list_share_permissions(&router, &json!({"resourceShareArn": share_arn.clone()}))
                    .await,
            ),
        ),
    ];
    for (case, body) in &reads {
        for (arn, reported) in summary_flags_of(case, body) {
            let expected = expected_default(&defaulted, &arn);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected isResourceTypeDefault {expected} for {arn}, got \
                 {reported}"
            );
        }
    }

    // And reading it does not move it: a second read of the same permission reports the
    // same flag.
    let again = ok(
        "a repeated read",
        list_permissions(&router, &json!({"resourceType": SUBNET_RESOURCE_TYPE})).await,
    );
    for (arn, reported) in summary_flags_of("a repeated read", &again) {
        let expected = expected_default(&defaulted, &arn);
        assert_eq!(
            reported, expected,
            "case \"a repeated read\": expected isResourceTypeDefault {expected} for {arn}, \
             got {reported}"
        );
    }
}

/// What `isResourceTypeDefault` must report for `arn`: true for a record the test wrote
/// the flag onto, and otherwise the value the stored population carries — a catalog
/// entry's published flag, and false for a permission this account created.
fn expected_default(written: &[String], arn: &str) -> bool {
    written.iter().any(|flagged| flagged == arn)
        || AWS_MANAGED_PERMISSIONS
            .iter()
            .find(|entry| aws_managed_permission_arn(entry.name) == arn)
            .is_some_and(|entry| entry.is_resource_type_default)
}
