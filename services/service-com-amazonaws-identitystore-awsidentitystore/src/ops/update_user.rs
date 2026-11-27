// SPDX-License-Identifier: CC0-1.0

//! @op UpdateUser
//!
//! Applies the request's `AttributeOperation`s to one stored user, recording the
//! calling principal as the user's `UpdatedBy`.
//!
//! ## How an update is applied
//!
//! The request carries a list of `AttributeOperation`s, each naming one attribute of
//! the stored user by `AttributePath`:
//!
//! * an operation WITH an `AttributeValue` sets that attribute to the value — a
//!   multi-valued attribute (`emails`, `phoneNumbers`, `addresses`, `photos`, `roles`)
//!   is replaced wholesale by the array it is given, since the modelled
//!   `AttributePath` admits only letters and dots and so cannot name one element of a
//!   list;
//! * an operation WITHOUT one (or with an explicit `null`) REMOVES it, which is the
//!   third thing `AttributeOperation` documents it "might" do ("these operations might
//!   add, replace, or remove an attribute"); an EMPTY array is not a removal — every
//!   multi-valued member is modelled with a minimum length of 1, so it is refused.
//!
//! A replaced element is the element the request supplied and nothing more: a
//! sub-attribute the `AttributeValue` does not name is absent from the stored element,
//! and so from every later response, rather than being merged in from the element it
//! replaced or taken from the model's `@default`.
//!
//! The operations apply in request order, so a later one naming the same path wins.
//! Paths and the member names inside an `AttributeValue` are matched without regard to
//! case (SCIM attribute names are case-insensitive), so `displayName`, `DisplayName`
//! and `displayname` all name one attribute. A path that names no attribute of a user,
//! or one the service mints rather than the caller (`userId`, `userArn`, `revision`,
//! the timestamps), is refused with the modelled `ValidationException`, as is a value of
//! the wrong JSON type or one the modelled shape of the member being written does not
//! admit — its `@length` and its `@pattern` are applied here, because an `AttributeValue`
//! is an untyped document the request boundary checks nothing about, so a value a create
//! of that member refuses an update of it refuses too.
//!
//! Nothing is written until EVERY operation has been decoded, so a request refused
//! half-way leaves the stored user exactly as it was.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateUser`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-identitystore-awsidentitystore/smithy-model/com-amazonaws-identitystore-awsidentitystore.json")
//! ```
//!
//! Updates the specified user metadata and attributes in the specified identity store.
//!
//! ## Input
//!
//! * `IdentityStoreId` (IdentityStoreId, len 1..93, pattern
//!   ^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[
//!   …) **required** — The globally unique identifier for the identity store. You can specify
//!   the identity store by ID or by Amazon Resource Name (ARN). For example, identity store
//!   ID d-1234567890 or identity store ARN
//!   arn:aws:identitystore::111122223333:identitystore/d-1234567890.
//! * `Operations` (AttributeOperations, len 1..100) **required** — A list of
//!   AttributeOperation objects to apply to the requested user. These operations might add,
//!   replace, or remove an attribute. For more information on the attributes that can be
//!   added, replaced, or removed, see User.
//! * `Revision` (ResourceRevision, len 1..64, pattern ^[0-9]+$) — The expected current
//!   revision of the user. When you provide this value, the update is applied only if it
//!   matches the current revision of the user in the identity store, which prevents you from
//!   overwriting concurrent changes. If the value doesn't match, the operation fails with a
//!   ConflictException. If you don't provide this value, the update is applied
//!   unconditionally.
//! * `UserId` (ResourceId, len 1..100, pattern
//!   ^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-
//!   …) **required** — The identifier for a user in the identity store. You can specify the
//!   user by ID or by Amazon Resource Name (ARN). For example, user ID
//!   a1b2c3d4-5678-90ab-cdef-EXAMPLE11111 or user ARN
//!   arn:aws:identitystore:::user/a1b2c3d4-5678-90ab-cdef-EXAMPLE11111.
//!
//! ## Output
//!
//! * `IdentityStoreId` (IdentityStoreId, len 1..93, pattern
//!   ^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[
//!   …) **required** — The globally unique identifier for the identity store.
//! * `Revision` (ResourceRevision, len 1..64, pattern ^[0-9]+$) **required** — The revision
//!   of the user after the requested update is applied.
//! * `UserArn` (ResourceArn, len 60..100, pattern
//!   ^arn:aws[a-z-]*:identitystore:::(user|group|membership)/([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-
//!   …) **required** — The Amazon Resource Name (ARN) of the user in the identity store. For
//!   example, arn:aws:identitystore:::user/a1b2c3d4-5678-90ab-cdef-EXAMPLE11111.
//! * `UserId` (ResourceId, len 1..100, pattern
//!   ^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-
//!   …) **required** — The identifier for a user in the identity store.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — This request cannot be completed for one of the
//!   following reasons: Performing the requested operation would violate an existing
//!   uniqueness claim in the identity store. Resolve the conflict before retrying this
//!   request. The requested resource was being concurrently modified by another request.
//! * `ResourceNotFoundException` (HTTP 404) — Indicates that a requested resource is not
//!   found.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would cause the number of users
//!   or groups in the identity store to exceed the maximum allowed.
//! * `ValidationException` (HTTP 400) — The request failed because it contains a syntax
//!   error.

#[allow(unused_imports)]
use super::types::*;

// Every string attribute is read through the service's ONE text reader, in
// `crate::state`, so the modelled constraints of the member being written — its type, its
// `@length` and its `@pattern` — have one refusal; `optional_text` is the name the
// decoders below call it by.
use crate::state::attribute_text_value as optional_text;
use crate::state::attribute_value_kind as kind;
use crate::state::AttributeTextShape;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateUserError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-identitystore-awsidentitystore.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateUserError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.identitystore#UpdateUser",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum UpdateUserErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ValidationException(ValidationException),
    /// The service-common server error, widened in because the model declares it on
    /// the service rather than on this operation. Answered when the store is
    /// unreachable.
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_user::UpdateUserError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateUserError = crate::scaffolded::Scaffolded<UpdateUserErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateUserErrorModeled> for UpdateUserError {
    fn from(modeled: UpdateUserErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateUser` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateUser<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateUser<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateUser<Eng>
{
    type Input = UpdateUserInput;
    type Output = UpdateUserOutput;
    type Error = UpdateUserError;

    const NAME: &'static str = "UpdateUser";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_USER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_USER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Who is making the change: recorded as the user's `UpdatedBy`.
        let updated_by = crate::state::caller_principal(&*self.engine, ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateUserErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        // The store has to be one this account HAS, and the USER it names must exist in
        // it: an update stores no new resource, so a store this emulator holds no record
        // of holds no such user and the USER is what is missing. An identifier naming no
        // identity store of this account is refused for the store's own sake.
        let store_key =
            crate::state::addressed_store_key(&*self.engine, &input.identity_store_id)
                .map_err(UpdateUserErrorModeled::ResourceNotFoundException)?;
        let mut state = state.lock();
        let user_not_found = || {
            UpdateUserErrorModeled::ResourceNotFoundException(crate::state::not_found(
                crate::state::ResourceKind::User,
            ))
        };
        let store = state
            .resolve_recorded_store_mut(&store_key)
            .ok_or_else(user_not_found)?;
        let user_key = crate::state::resource_key(&input.user_id);
        let stored = store
            .users
            .get(&user_key)
            .cloned()
            .ok_or_else(user_not_found)?;

        // `Revision` is the caller's precondition: "the update is applied only if it
        // matches the current revision of the user … If the value doesn't match, the
        // operation fails with a ConflictException." The member is a decimal string
        // (`^[0-9]+$`), so what it names is the NUMBER it spells — `01` and `1` name one
        // revision — which `revision_precondition_holds` is the one comparison for.
        if let Some(expected) = input.revision.as_deref() {
            if !crate::state::revision_precondition_holds(expected, stored.revision) {
                return Err(UpdateUserErrorModeled::ConflictException(
                    crate::state::revision_conflict(expected, &stored.revision()),
                )
                .into());
            }
        }

        // The operations are applied to a COPY and committed only once every one of
        // them held, so a request that names an attribute path this service does not
        // update leaves the stored user exactly as it was.
        let mut updated = stored.clone();
        for operation in &input.operations {
            apply_attribute_operation(
                &mut updated,
                &operation.attribute_path,
                operation.attribute_value.as_ref(),
            )
            .map_err(UpdateUserErrorModeled::ValidationException)?;
        }

        // `UserName` is a uniqueness claim within the identity store, so an update
        // taking one another user already holds conflicts instead of storing a
        // duplicate. The conflict names the CLAIM that was violated and nothing else:
        // neither the `@sensitive` name nor the id of the user that already holds it.
        if updated.user_name != stored.user_name
            && updated
                .user_name
                .as_deref()
                .and_then(|user_name| store.user_name_index.get(user_name))
                .is_some_and(|taken_by| *taken_by != updated.user_id)
        {
            return Err(UpdateUserErrorModeled::ConflictException(
                crate::state::uniqueness_conflict(crate::state::UniquenessClaim::UserName),
            )
            .into());
        }

        // The update happened: a new revision, a new `UpdatedAt` and the principal that
        // made it.
        updated.record_update(::std::time::SystemTime::now());
        updated.updated_by = Some(updated_by);

        let output = UpdateUserOutput {
            identity_store_id: store.identity_store_id.clone(),
            user_id: updated.user_id.clone(),
            user_arn: updated.user_arn.clone(),
            // "The revision of the user after the requested update is applied."
            revision: updated.revision(),
        };
        // The `UserName` index follows the name: the old claim is released and the new
        // one taken, so a later create or update can reuse the name this user dropped.
        if updated.user_name != stored.user_name {
            if let Some(released) = stored.user_name.as_deref() {
                store.user_name_index.remove(released);
            }
            if let Some(claimed) = updated.user_name.clone() {
                store.user_name_index.insert(claimed, user_key.clone());
            }
        }
        store.users.insert(user_key, updated);
        Ok(output)
    }
}

/// The modelled string shape every string attribute of a `User` but `UserName` targets:
/// `SensitiveStringType`, whose `@length` is 1..1024 and whose `@pattern` admits the three
/// space characters the model lists (`smithy_query(query="shape",shape="User")`).
const TEXT: AttributeTextShape = AttributeTextShape::SensitiveText;

/// The modelled string shape `User$UserName` targets: `UserName`, whose `@length` is 1..128
/// and whose `@pattern` admits no whitespace at all.
const USER_NAME: AttributeTextShape = AttributeTextShape::UserName;

/// The most entries `User$Emails`, `User$Addresses`, `User$PhoneNumbers` and
/// `User$Roles` admit: each of those members is modelled `length 1..1`.
const SINGLE_ENTRY_MAX: usize = 1;

/// The most entries `User$Photos` admits: modelled `length 1..3` ("Users can have up
/// to 3 photos").
const PHOTOS_MAX: usize = 3;

/// The members of a modelled `Email` / `PhoneNumber` / `Role`, from
/// `smithy_query(query="shape",shape="Email,PhoneNumber,Role")`.
const CONTACT_MEMBERS: &[&str] = &["Value", "Type", "Primary"];

/// The members of a modelled `Address`.
const ADDRESS_MEMBERS: &[&str] = &[
    "StreetAddress",
    "Locality",
    "Region",
    "PostalCode",
    "Country",
    "Formatted",
    "Type",
    "Primary",
];

/// The members of a modelled `Photo`.
const PHOTO_MEMBERS: &[&str] = &["Value", "Type", "Display", "Primary"];

/// The members of a modelled `Name`.
const NAME_MEMBERS: &[&str] = &[
    "Formatted",
    "FamilyName",
    "GivenName",
    "MiddleName",
    "HonorificPrefix",
    "HonorificSuffix",
];

/// Apply one `AttributeOperation` to `user`, or say why the request is invalid.
///
/// `value` is `None` for an operation that carries no `AttributeValue`, which removes
/// the attribute; an explicit JSON `null` means the same thing.
///
/// A path this operation does not own — one the service mints or an identity provider
/// owns (`userId`, `userArn`, `identityStoreId`, `revision`, `externalIds`, the audit
/// members), an attribute a user does not have, or a path INTO a multi-valued attribute,
/// which AWS resolves with JMESPath over the stored entries and this slice does not
/// evaluate — is refused with the message AWS builds from the requested path, and
/// nothing is applied.
fn apply_attribute_operation(
    user: &mut crate::state::User,
    path: &str,
    value: Option<&::aws_smithy_types::Document>,
) -> Result<(), ValidationException> {
    apply(user, path, value).map_err(crate::state::validation)
}

/// [`apply_attribute_operation`]'s body, whose `Err` is the message the caller gets.
fn apply(
    user: &mut crate::state::User,
    path: &str,
    value: Option<&::aws_smithy_types::Document>,
) -> Result<(), String> {
    // SCIM attribute names are case-insensitive, and the modelled `AttributePath`
    // pattern admits only letters and dots, so folding the path is enough to normalise
    // it.
    let key = path.to_ascii_lowercase();
    match key.as_str() {
        "username" => {
            let user_name = optional_text(value, path, USER_NAME)?;
            // `UserName` NAMES the user, so an update that would give it a name this API
            // reserves is refused exactly as a create carrying one is, and the user keeps
            // the name it had.
            if user_name
                .as_deref()
                .is_some_and(crate::state::is_reserved_identity_name)
            {
                return Err(crate::state::reserved_identity_name_message());
            }
            user.user_name = user_name;
        }
        "displayname" => user.display_name = optional_text(value, path, TEXT)?,
        "nickname" => user.nick_name = optional_text(value, path, TEXT)?,
        "profileurl" => user.profile_url = optional_text(value, path, TEXT)?,
        "usertype" => user.user_type = optional_text(value, path, TEXT)?,
        "title" => user.title = optional_text(value, path, TEXT)?,
        "preferredlanguage" => user.preferred_language = optional_text(value, path, TEXT)?,
        "locale" => user.locale = optional_text(value, path, TEXT)?,
        "timezone" => user.timezone = optional_text(value, path, TEXT)?,
        "website" => user.website = optional_text(value, path, TEXT)?,
        "birthdate" => user.birthdate = optional_text(value, path, TEXT)?,
        "userstatus" => user.user_status = user_status(value, path)?,
        "name" => user.name = optional_name(value, path)?,
        "name.formatted" => set_name_part(user, |name| &mut name.formatted, value, path)?,
        "name.familyname" => set_name_part(user, |name| &mut name.family_name, value, path)?,
        "name.givenname" => set_name_part(user, |name| &mut name.given_name, value, path)?,
        "name.middlename" => set_name_part(user, |name| &mut name.middle_name, value, path)?,
        "name.honorificprefix" => {
            set_name_part(user, |name| &mut name.honorific_prefix, value, path)?
        }
        "name.honorificsuffix" => {
            set_name_part(user, |name| &mut name.honorific_suffix, value, path)?
        }
        "emails" => {
            user.emails = entries(value, path, SINGLE_ENTRY_MAX, "Email", CONTACT_MEMBERS, email)?
        }
        "phonenumbers" => {
            user.phone_numbers = entries(
                value,
                path,
                SINGLE_ENTRY_MAX,
                "PhoneNumber",
                CONTACT_MEMBERS,
                phone,
            )?
        }
        "addresses" => {
            user.addresses = entries(
                value,
                path,
                SINGLE_ENTRY_MAX,
                "Address",
                ADDRESS_MEMBERS,
                address,
            )?
        }
        "roles" => {
            user.roles = entries(value, path, SINGLE_ENTRY_MAX, "Role", CONTACT_MEMBERS, role)?
        }
        "photos" => {
            user.photos = entries(value, path, PHOTOS_MAX, "Photo", PHOTO_MEMBERS, photo)?
        }
        // Every other path is one an update does not own: the attributes the service
        // mints or an identity provider owns, a path into a multi-valued attribute, and
        // anything that is not an attribute of a user at all. AWS refuses each with one
        // message built from the path it was given, which `crate::state` states once
        // for both update operations.
        _ => return Err(crate::state::unsupported_attribute_path_message(path)),
    }
    Ok(())
}

/// A required string member of an entry inside a multi-valued attribute.
fn required_text(
    value: Option<&::aws_smithy_types::Document>,
    path: &str,
    member: &str,
) -> Result<String, String> {
    optional_text(value, path, TEXT)?
        .ok_or_else(|| format!("every {member} entry of {path} must carry a Value"))
}

/// The `Primary` sub-attribute an entry carries, or `None` when it carries none.
///
/// A replaced element is the element the request supplied and nothing more, so a
/// `Primary` the request did not name stays ABSENT rather than taking the model's
/// `@default` — the reported element then says what the caller set, and a caller can
/// still tell "never set" from an explicit `false`.
fn flag(value: Option<&::aws_smithy_types::Document>, path: &str) -> Result<Option<bool>, String> {
    use ::aws_smithy_types::Document;
    match value {
        None | Some(Document::Null) => Ok(None),
        Some(Document::Bool(flag)) => Ok(Some(*flag)),
        Some(other) => Err(format!(
            "Primary inside the AttributeValue for {path} must be a boolean, got {}",
            kind(other)
        )),
    }
}

/// The `UserStatus` an operation sets: the modelled enum admits exactly
/// `ENABLED | DISABLED` (`USER_STATUS_VALUES`), and every stored user HAS a status, so
/// neither another value nor a removal names a status a user can be in.
fn user_status(
    value: Option<&::aws_smithy_types::Document>,
    path: &str,
) -> Result<String, String> {
    let expected = || {
        format!(
            "the AttributeValue for {path} must be one of {}",
            USER_STATUS_VALUES.join(", ")
        )
    };
    // `UserStatus` is a modelled ENUM, so its value set is the constraint on it and it
    // carries neither the `@length` nor the `@pattern` a text attribute does: the string is
    // read for its JSON type alone and then checked against the enum.
    let status = crate::state::attribute_string_value(value, path)?.ok_or_else(expected)?;
    if !USER_STATUS_VALUES.contains(&status.as_str()) {
        return Err(expected());
    }
    Ok(status)
}

/// The members of an object `AttributeValue`, keyed without regard to case.
fn object<'a>(
    value: &'a ::aws_smithy_types::Document,
    path: &str,
    shape: &str,
    members: &[&str],
) -> Result<::std::collections::HashMap<String, &'a ::aws_smithy_types::Document>, String> {
    use ::aws_smithy_types::Document;
    let Document::Object(entries) = value else {
        return Err(format!(
            "the AttributeValue for {path} must be a {shape} object, got {}",
            kind(value)
        ));
    };
    let mut keyed = ::std::collections::HashMap::with_capacity(entries.len());
    for (name, member) in entries {
        let Some(modeled) = members
            .iter()
            .find(|modeled| modeled.eq_ignore_ascii_case(name))
        else {
            return Err(format!(
                "the AttributeValue for {path} carries {name:?}, which is not a member \
                 of {shape} (expected one of {})",
                members.join(", ")
            ));
        };
        // Two spellings of one member (`{"Value":.., "value":..}`) would otherwise
        // leave the entry to depend on map iteration order, so the request is refused
        // rather than resolved arbitrarily.
        if keyed.insert((*modeled).to_string(), member).is_some() {
            return Err(format!(
                "the AttributeValue for {path} carries {modeled} of {shape} more than \
                 once"
            ));
        }
    }
    Ok(keyed)
}

/// The modelled `Name` an operation sets, or `None` when it removes the whole name.
fn optional_name(
    value: Option<&::aws_smithy_types::Document>,
    path: &str,
) -> Result<Option<Name>, String> {
    use ::aws_smithy_types::Document;
    let Some(value) = value.filter(|value| !matches!(value, Document::Null)) else {
        return Ok(None);
    };
    let members = object(value, path, "Name", NAME_MEMBERS)?;
    let name = Name {
        formatted: optional_text(members.get("Formatted").copied(), path, TEXT)?,
        family_name: optional_text(members.get("FamilyName").copied(), path, TEXT)?,
        given_name: optional_text(members.get("GivenName").copied(), path, TEXT)?,
        middle_name: optional_text(members.get("MiddleName").copied(), path, TEXT)?,
        honorific_prefix: optional_text(
            members.get("HonorificPrefix").copied(),
            path,
            TEXT,
        )?,
        honorific_suffix: optional_text(
            members.get("HonorificSuffix").copied(),
            path,
            TEXT,
        )?,
    };
    // A name every member of which was removed is no name at all, so it is reported
    // absent rather than as an empty object.
    Ok((!empty_name(&name)).then_some(name))
}

/// Whether every member of `name` is absent.
fn empty_name(name: &Name) -> bool {
    [
        &name.formatted,
        &name.family_name,
        &name.given_name,
        &name.middle_name,
        &name.honorific_prefix,
        &name.honorific_suffix,
    ]
    .iter()
    .all(|member| member.is_none())
}

/// Set one sub-attribute of the user's `Name`, leaving its siblings alone.
///
/// The `Name` object springs into being when a sub-attribute is set on a user that has
/// none, and goes away again once its last member is removed.
fn set_name_part(
    user: &mut crate::state::User,
    part: fn(&mut Name) -> &mut Option<String>,
    value: Option<&::aws_smithy_types::Document>,
    path: &str,
) -> Result<(), String> {
    let text = optional_text(value, path, TEXT)?;
    if text.is_none() && user.name.is_none() {
        return Ok(());
    }
    let name = user.name.get_or_insert_with(Name::default);
    *part(name) = text;
    if empty_name(name) {
        user.name = None;
    }
    Ok(())
}

/// The entries of a multi-valued attribute: a full replacement, since the modelled
/// `AttributePath` cannot name one element of a list.
///
/// An absent value or a `null` leaves the attribute with no entries, which is how it
/// is reported absent. `max` is the modelled maximum length of the member being set,
/// and the model gives each of them a MINIMUM of 1, so an array must carry between one
/// and `max` entries: an empty one is refused, and removal is what the valueless
/// operation is for.
fn entries<T>(
    value: Option<&::aws_smithy_types::Document>,
    path: &str,
    max: usize,
    shape: &str,
    members: &[&str],
    decode: fn(
        &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
        &str,
    ) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    use ::aws_smithy_types::Document;
    let items: &[Document] = match value {
        None | Some(Document::Null) => return Ok(Vec::new()),
        Some(Document::Array(items)) => items,
        // The model types these members as LISTS, so a lone entry still needs its
        // array: anything else is refused rather than repaired.
        Some(other) => {
            return Err(format!(
                "the AttributeValue for {path} must be an array of {shape} objects, got {}",
                kind(other)
            ))
        }
    };
    if !(1..=max).contains(&items.len()) {
        return Err(format!(
            "the AttributeValue for {path} carries {} entries, outside the 1..{max} \
             the model allows: omit the AttributeValue to remove the attribute",
            items.len()
        ));
    }
    items
        .iter()
        .map(|item| decode(&object(item, path, shape, members)?, path))
        .collect()
}

/// One `Email` entry.
fn email(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<Email, String> {
    Ok(Email {
        value: optional_text(members.get("Value").copied(), path, TEXT)?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}

/// One `PhoneNumber` entry.
fn phone(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<PhoneNumber, String> {
    Ok(PhoneNumber {
        value: optional_text(members.get("Value").copied(), path, TEXT)?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}

/// One `Role` entry.
fn role(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<Role, String> {
    Ok(Role {
        value: optional_text(members.get("Value").copied(), path, TEXT)?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}

/// One `Address` entry.
fn address(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<Address, String> {
    Ok(Address {
        street_address: optional_text(members.get("StreetAddress").copied(), path, TEXT)?,
        locality: optional_text(members.get("Locality").copied(), path, TEXT)?,
        region: optional_text(members.get("Region").copied(), path, TEXT)?,
        postal_code: optional_text(members.get("PostalCode").copied(), path, TEXT)?,
        country: optional_text(members.get("Country").copied(), path, TEXT)?,
        formatted: optional_text(members.get("Formatted").copied(), path, TEXT)?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}

/// One `Photo` entry. `Photo$Value` is the shape's only required member ("this field
/// is required for all photo entries"), so an entry without one is refused.
fn photo(
    members: &::std::collections::HashMap<String, &::aws_smithy_types::Document>,
    path: &str,
) -> Result<Photo, String> {
    Ok(Photo {
        value: required_text(members.get("Value").copied(), path, "Photo")?,
        r#type: optional_text(members.get("Type").copied(), path, TEXT)?,
        display: optional_text(members.get("Display").copied(), path, TEXT)?,
        primary: flag(members.get("Primary").copied(), path)?,
    })
}
