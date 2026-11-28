// SPDX-License-Identifier: CC0-1.0

//! A user's attributes: the plans an update or a deletion of them is decided by,
//! and the pool schema those plans are judged against.

use super::*;

/// Decide what `requested` does to `user` of `pool`, or the refusal it earns.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_update`]), so a request refused halfway
/// through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * An attribute the pool's schema does not declare cannot hold a value, and
///   one it declares immutable cannot change (`SchemaAttributeType.Mutable`),
///   so neither is an update to make — nor is blanking an attribute the schema
///   marks required, which has to keep a value, each reported as the schema
///   finding it is ([`schema_validation_report`]). A verified flag is immutable to
///   the user it vouches for, whatever the schema says
///   ([`VerifiedFlagAuthority::NotTheirs`]).
/// * One value per attribute: `AttributeType` is "the name and value of a user
///   attribute", so a list naming the same attribute twice asks for two values
///   of one attribute, and a list naming more attributes than the schema
///   declares asks for more than any user of the pool can hold.
/// * A value longer than an attribute value's own maximum
///   ([`MAX_ATTRIBUTE_VALUE_LENGTH`]) is no value an attribute can hold, so it is
///   refused ahead of every bound the pool's schema declares, with that ceiling
///   stated per attribute ([`attribute_value_too_long_message`]).
/// * "To delete an attribute from your user, submit the attribute in your API
///   request with a blank value", so a blank value REMOVES the stored
///   attribute rather than storing an empty one.
/// * A value whose attribute has a FORM of its own has to have that form
///   ([`attribute_value_defect`]), so a `phone_number` that is not a phone
///   number in E.164 form and an `email` that is not an email address are
///   refused rather than stored — and a `preferred_username`, being a NAME its
///   holder answers to, has to lie outside the namespace of every attribute the
///   pool aliases ([`UserPool::alias_format_username_defect`]).
/// * A value has to satisfy the constraints the pool's schema declares for its
///   attribute ([`schema_constraint_refusal`]), so a number outside the declared
///   `MinValue`/`MaxValue`, a string outside the declared
///   `MinLength`/`MaxLength` and a value of a `Number` attribute that is not a
///   number at all are refused rather than stored — this operation being an
///   UPDATE of a stored user, with that stage's wording.
/// * A new `email` or `phone_number` is no longer the value its verified flag
///   vouched for ([`verified_flag`]), so the flag goes unconfirmed unless the
///   request sets it itself, and a deleted one takes its flag with it — and a
///   flag set for an attribute that would hold no value at all vouches for
///   nothing, so it is refused
///   ([`verified_flag_needs_a_value_message`]).
/// * A pool that verifies the attribute itself (`AutoVerifiedAttributes`) or
///   requires verification before the update
///   (`AttributesRequireVerificationBeforeUpdate`) sends a verification
///   message, and the second holds the new value pending until the user
///   confirms it — unless the caller's own flag asserts it is already verified
///   ([`VerifiedFlagAuthority`]).
/// * An alias value names ONE user of the pool, so a value another user holds
///   as an active alias moves to this user and the other holder's verified flag
///   is demoted with it; an alias attribute that carries no such flag cannot be
///   moved that way, so taking its value is refused.
///
/// Every step below is bounded by the POOL's schema rather than by the length
/// of the submitted list: the count is capped and repeated names are refused
/// before any per-attribute work, so one request cannot drive work the caller
/// chose the size of.
pub fn plan_attribute_update(
    pool: &UserPool,
    user: &User,
    requested: &[AttributeType],
    authority: VerifiedFlagAuthority,
) -> Result<AttributeUpdatePlan, AttributeUpdateRefusal> {
    // A request cannot update more attributes than the pool declares: past
    // that it either names one the schema does not declare or names one twice,
    // both refused below. Checked FIRST, so nothing further is proportional to
    // a list length the caller chose.
    if requested.len() > pool.schema.len() {
        return Err(AttributeUpdateRefusal::Invalid(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be updated in one request",
            pool.schema.len()
        )));
    }
    // Every value this request submits has to be one an attribute value may
    // hold at all ([`MAX_ATTRIBUTE_VALUE_LENGTH`]), whatever attribute carries
    // it and whatever the pool's schema declares for that attribute: the
    // ceiling is the model's own, so it is judged ahead of every bound the
    // pool's operator declared, and this stage states it in its own
    // per-attribute sentence. Asked of the submitted list in the order it
    // arrived, so the attribute reported is the first value past the ceiling.
    if let Some(too_long) = requested.iter().find(|attribute| {
        attribute
            .value
            .as_deref()
            .unwrap_or_default()
            .chars()
            .count()
            > MAX_ATTRIBUTE_VALUE_LENGTH
    }) {
        return Err(AttributeUpdateRefusal::Invalid(
            attribute_value_too_long_message(&too_long.name, MAX_ATTRIBUTE_VALUE_LENGTH),
        ));
    }
    // Every attribute written has to be one the pool's schema declares. Asked
    // of the submitted list as it arrived, so an undeclared attribute is
    // reported as undeclared whatever else the list carries, and EVERY one of
    // them is reported: the schema validation states a line per attribute it
    // will not write.
    let names: Vec<String> = requested
        .iter()
        .map(|attribute| attribute.name.clone())
        .collect();
    let undeclared: Vec<&str> = names
        .iter()
        .filter(|name| !pool.declares_attribute(name))
        .map(String::as_str)
        .collect();
    if !undeclared.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            undeclared
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotInSchema)),
        )));
    }
    // A user holds ONE value per attribute, so a list naming the same
    // attribute twice names no single update to make. Keyed by name, so every
    // lookup below is a lookup rather than a scan of the request.
    let mut submitted: BTreeMap<&str, &str> = BTreeMap::new();
    for attribute in requested {
        let value = attribute.value.as_deref().unwrap_or_default();
        if submitted.insert(attribute.name.as_str(), value).is_some() {
            return Err(AttributeUpdateRefusal::Invalid(format!(
                "Invalid attributes given, {} was submitted more than once",
                attribute.name
            )));
        }
    }

    let mut updates = Vec::new();
    let mut verifications = Vec::new();
    let mut demotions: Vec<(String, &'static str)> = Vec::new();
    // A verified flag records THIS SERVICE's confirmation of the value beside
    // it, so it is not an attribute its own holder may write
    // ([`VerifiedFlagAuthority::NotTheirs`]): a caller with no authority over
    // the flag cannot modify it whatever the pool's schema says about it, and
    // Amazon Cognito reports that as the non-mutable attribute it is. Reported
    // for the first flag the request names, in request order.
    if authority == VerifiedFlagAuthority::NotTheirs {
        if let Some(flag) = requested
            .iter()
            .find(|attribute| verified_attribute_of(&attribute.name).is_some())
        {
            return Err(AttributeUpdateRefusal::Invalid(
                non_mutable_attribute_message(&flag.name),
            ));
        }
    }
    // `SchemaAttributeType.Mutable` is what says a stored value may change, so an
    // attribute the schema freezes cannot be updated at all — and EVERY one the
    // request names is reported, a line each, exactly as the undeclared ones above
    // are: the schema validation states a line per attribute it will not write.
    // Judged in request order, over the attributes this request actually writes.
    let frozen: Vec<&str> = requested
        .iter()
        .map(|attribute| attribute.name.as_str())
        .filter(|name| !settled_by_a_deletion(name, &submitted))
        .filter(|name| !pool.attribute_mutable(name))
        .collect();
    if !frozen.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            frozen
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotUpdatable)),
        )));
    }
    // An attribute the pool's schema marks required has to keep a value, so a
    // blank value is not one it may be given — "to delete an attribute from
    // your user, submit the attribute in your API request with a blank value"
    // (this service's model), and a required attribute is not one to delete.
    // That is a finding of the same schema validation as the two above, so it
    // is reported the same way: a line per attribute the request would blank,
    // in request order.
    let undeletable: Vec<&str> = requested
        .iter()
        .filter(|attribute| attribute.value.as_deref().unwrap_or_default().is_empty())
        .map(|attribute| attribute.name.as_str())
        .filter(|name| pool.attribute_required(name))
        .collect();
    if !undeletable.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            undeletable
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::RequiredNotDeletable)),
        )));
    }
    // Every value written has to satisfy the constraints the pool's schema
    // declares for its attribute ([`schema_constraint_refusal`]). Judged in the
    // order the request submitted the attributes, and decided before any attribute
    // is planned, so a request carrying such a value writes nothing.
    if let Some(refusal) = schema_constraint_refusal(
        pool,
        SchemaValidationStage::Update,
        requested.iter().map(|attribute| {
            (
                attribute.name.as_str(),
                attribute.value.as_deref().unwrap_or_default(),
            )
        }),
    ) {
        return Err(AttributeUpdateRefusal::Invalid(refusal));
    }
    // A submitted NAME has to lie in a namespace this pool admits, whichever
    // operation submits it ([`submitted_name_defect`]).
    if let Some(defect) = submitted_name_defect(
        pool,
        submitted.iter().map(|(name, value)| (*name, *value)),
    ) {
        return Err(AttributeUpdateRefusal::Invalid(defect));
    }
    // A verified flag vouches for the VALUE of its own attribute
    // ([`verified_flag`]), so there has to be one for it to vouch for: the
    // value this request stores for that attribute, or the one the user already
    // holds where the request says nothing about it. A flag set for an
    // attribute that would hold nothing — because the user holds nothing and
    // the request stores nothing, or because the request DELETES it — vouches
    // for no value at all. Judged in request order, over the two attributes
    // this API verifies, so the work is bounded by the flags the request names.
    for attribute in requested {
        let Some(described) = verified_attribute_of(&attribute.name) else {
            continue;
        };
        let vouched_for = match submitted.get(described) {
            Some(submitted_value) => *submitted_value,
            None => user
                .attributes
                .get(described)
                .map(String::as_str)
                .unwrap_or_default(),
        };
        if vouched_for.is_empty() {
            if let Some(message) = verified_flag_needs_a_value_message(described) {
                return Err(AttributeUpdateRefusal::Invalid(message));
            }
        }
    }
    for (name, value) in &submitted {
        let name = (*name).to_string();
        // A verified flag whose attribute this same request DELETES has
        // nothing left to record, so the deletion settles the flag (below)
        // rather than the value submitted for it.
        if settled_by_a_deletion(&name, &submitted) {
            continue;
        }
        let value = (*value).to_string();
        // A value whose attribute has a FORM of its own has to have that form
        // ([`attribute_value_defect`]), whatever else the request asks for.
        if let Some(defect) = attribute_value_defect(&name, &value) {
            return Err(AttributeUpdateRefusal::Invalid(defect.to_string()));
        }
        if value.is_empty() {
            // A deleted email address or phone number leaves its verified flag
            // vouching for a value the user no longer holds, so the flag goes
            // with it.
            let stale_verified_flag = verified_flag(&name);
            updates.push(AttributeUpdate {
                name,
                value: None,
                deferred: false,
                stale_verified_flag,
            });
            continue;
        }

        let unchanged = user
            .attributes
            .get(&name)
            .is_some_and(|stored| stored == &value);
        let verified_flag = verified_flag(&name);
        // A caller who may assert the value verified, and does, has nothing
        // left to verify: the value applies at once and no message goes out.
        let asserted_verified = authority == VerifiedFlagAuthority::Asserts
            && verified_flag.is_some_and(|flag| {
                submitted
                    .get(flag)
                    .is_some_and(|value| *value == FLAG_CONFIRMED)
            });
        // A pool that requires verification before the update holds the new
        // value pending instead of storing it over the confirmed one.
        let deferred = !asserted_verified
            && verified_flag.is_some()
            && pool.requires_verification_before_update(&name);
        if deferred && unchanged {
            // The user already holds this value, confirmed: there is nothing
            // to hold pending and nothing to confirm.
            continue;
        }
        // A value that changed is no longer the one the verified flag vouched
        // for — unless the request sets that flag itself, which is then the
        // value stored for it.
        let stale_verified_flag = verified_flag
            .filter(|flag| !deferred && !unchanged && !submitted.contains_key(*flag));

        // An alias value names ONE user of the pool, so this user taking
        // `value` is the other holders losing it. An `email` or a
        // `phone_number` is an alias only while its verified flag vouches for
        // it ([`verified_flag`]), so the holder's flag is what the write takes
        // away: the update is accepted and the previous holder's copy is
        // demoted out of verified status. An alias attribute that carries no
        // such flag cannot be moved that way, so a value the pool ALREADY
        // RESOLVES to another user of it — that user's username, a sign-in or
        // alias value it answers to, or its own id
        // ([`UserPool::names_another_user`]) — stays that user's and this update
        // is refused: the two would otherwise both answer to the one name. Only
        // an alias attribute of the pool is looked up, and each is looked up
        // once, so the pool's users are walked at most once per alias attribute
        // the pool configures.
        let becomes_active_alias = !deferred
            && verified_flag
                .is_none_or(|flag| flag_after_update(&submitted, user, flag, &stale_verified_flag));
        if becomes_active_alias && pool.is_alias_attribute(&name) {
            match verified_flag {
                Some(flag) => demotions.extend(
                    pool.alias_holder_keys(&name, &value, &user.username)
                        .into_iter()
                        .map(|key| (key, flag)),
                ),
                None if pool.names_another_user(&value, &user.username) => {
                    return Err(AttributeUpdateRefusal::AliasTaken(name));
                }
                None => {}
            }
        }

        // A verification message goes to a NEW email address or phone number
        // the pool verifies itself, and to one it holds pending until the user
        // confirms it. One entry per attribute at most, since the attributes
        // are the distinct ones submitted.
        if !unchanged
            && !asserted_verified
            && (deferred || pool.auto_verifies(&name))
            && delivery_medium(&name).is_some()
        {
            verifications.push((name.clone(), value.clone()));
        }

        updates.push(AttributeUpdate {
            name,
            value: Some(value),
            deferred,
            stale_verified_flag,
        });
    }

    Ok(AttributeUpdatePlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        updates,
        verifications,
        demotions,
    })
}

/// The whole deletion: which stored user it applies to and which of that
/// user's attributes it removes ([`plan_attribute_deletion`]).
#[derive(Debug)]
pub struct AttributeDeletionPlan {
    /// The pool holding the user this plan applies to.
    pub user_pool_id: String,
    /// The STORED username of that user, however the request named it.
    pub username: String,
    /// Every attribute name to remove, each once: the names the request asked
    /// for, plus the verified flag of each, which has nothing left to vouch for
    /// once the value it describes is gone.
    pub deletions: BTreeSet<String>,
}

/// Decide what deleting `requested` does to `user` of `pool`, or the message of
/// the `InvalidParameterException` the request earns — which the asking
/// operation carries in the modelled error IT declares.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_deletion`]), so a request refused
/// halfway through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * `AttributeNameListType` is a list of attribute NAMES and a user holds one
///   value per attribute, so naming an attribute twice asks for the same single
///   removal, and a list of more distinct names than the schema declares names
///   more attributes than any user of the pool can hold.
/// * An attribute the pool's schema does not declare cannot hold a value, so it
///   is not one a user can be asked to give up.
/// * An attribute the schema declares immutable (`SchemaAttributeType.Mutable`)
///   cannot change, and taking its value away changes it; one the schema marks
///   required has to keep a value, so neither is one this operation may remove —
///   the required one reported as the schema finding a blank value submitted for
///   it earns ([`schema_validation_report`]).
/// * A deleted `email` or `phone_number` leaves its verified flag
///   ([`verified_flag`]) vouching for a value the user no longer holds, so the
///   flag goes with it.
///
/// Every per-attribute step below is bounded by the POOL's schema rather than
/// by the length of the submitted list: the names are reduced to the distinct
/// ones and their count is capped before any per-attribute work, so one request
/// cannot drive work the caller chose the size of.
pub fn plan_attribute_deletion(
    pool: &UserPool,
    user: &User,
    requested: &[String],
) -> Result<AttributeDeletionPlan, String> {
    let names: Vec<String> = requested
        .iter()
        .collect::<BTreeSet<&String>>()
        .into_iter()
        .cloned()
        .collect();
    // Checked FIRST, so nothing further is proportional to a list length the
    // caller chose.
    if names.len() > pool.schema.len() {
        return Err(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be deleted in one request",
            pool.schema.len()
        ));
    }
    if let Some(unknown) = pool.unknown_attribute(&names) {
        return Err(undetermined_attribute_type_message(&unknown));
    }

    let mut deletions = BTreeSet::new();
    for name in names {
        if !pool.attribute_mutable(&name) {
            return Err(non_mutable_attribute_message(&name));
        }
        if pool.attribute_required(&name) {
            // An attribute the schema marks required keeps a value, and taking it
            // away is the same finding a blank value submitted for it earns, so
            // it is reported the same way ([`SchemaValidationFinding::RequiredNotDeletable`]).
            return Err(schema_validation_report([(
                name.as_str(),
                SchemaValidationFinding::RequiredNotDeletable,
            )]));
        }
        // Exactly as a blank value submitted to an attribute update takes it.
        if let Some(flag) = verified_flag(&name) {
            deletions.insert(flag.to_string());
        }
        deletions.insert(name);
    }

    Ok(AttributeDeletionPlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        deletions,
    })
}

/// The message `InvalidParameterException` carries for a `phone_number` value
/// that is not a phone number: the fixed sentence Amazon Cognito answers, which
/// names neither the attribute nor the value the request submitted.
pub const INVALID_PHONE_NUMBER_MESSAGE: &str = "Invalid phone number format.";

/// The message `InvalidParameterException` carries for an `email` value that is
/// not an email address: the fixed sentence Amazon Cognito answers, worded about
/// the form the value does not have and naming neither the attribute nor the
/// value the request submitted — the counterpart of
/// [`INVALID_PHONE_NUMBER_MESSAGE`] for the other addressing attribute.
pub const INVALID_EMAIL_MESSAGE: &str = "Invalid email address format.";

/// The message `AliasExistsException` carries for a write refused because the
/// value it would store already names another account of the pool as that
/// account's alias ([`AttributeUpdateRefusal::AliasTaken`]).
///
/// Amazon Cognito states that an account already answers to the alias, in one
/// fixed sentence: it names neither the attribute the request submitted, the
/// value it carried nor the account that holds it, so the refusal discloses
/// nothing about the pool's other users.
pub const ALIAS_EXISTS_MESSAGE: &str = "User account already exists for the alias.";

/// The ATTRIBUTE whose value is a phone number, and so has a form of its own to
/// satisfy ([`attribute_value_defect`]).
const PHONE_NUMBER_ATTRIBUTE: &str = "phone_number";

/// The ATTRIBUTE whose value is an email address, and so has a form of its own
/// to satisfy ([`attribute_value_defect`]).
const EMAIL_ATTRIBUTE: &str = "email";

/// The most digits a phone number in E.164 form carries, country calling code
/// included (the form `AdminCreateUser.UserAttributes` requires: "a phone number
/// must be in E.164 format", this service's model).
const E164_MAX_DIGITS: usize = 15;

/// The most characters one user attribute's value may carry, whichever attribute
/// holds it: `AttributeType.Value` targets `AttributeValueType`, which is
/// `@length(max: 2048)`
/// (`smithy_query(query="shape",shape="AttributeType,AttributeValueType")`).
///
/// A MODEL constant rather than a pool's own bound: the constraints a pool's
/// schema declares for an attribute ([`schema_constraint_refusal`]) sit inside
/// this one, and an attribute whose schema entry declares no maximum is bounded
/// by this alone.
pub const MAX_ATTRIBUTE_VALUE_LENGTH: usize = 2048;

/// The message an UPDATE of a stored user's attributes is refused with when the
/// value submitted for `name` is longer than `maximum` characters.
///
/// Amazon Cognito states that ceiling per ATTRIBUTE: the sentence's variable
/// parts are the attribute the request named and the maximum its value may have
/// ([`MAX_ATTRIBUTE_VALUE_LENGTH`]), and the rejected value is not quoted back —
/// a value past a maximum is longer than the maximum, so quoting it would answer
/// a request with the request.
pub fn attribute_value_too_long_message(name: &str, maximum: usize) -> String {
    format!(
        "The value of the attribute {name} must have a length less than or equal to {maximum} \
         characters"
    )
}

/// Why one of the `submitted` `(attribute, value)` pairs stores a NAME no user of
/// `pool` may answer to, or `None` when every one of them does.
///
/// `preferred_username` is "the user's preferred name" and an alias attribute of
/// its own ([`PREFERRED_USERNAME_ATTRIBUTE`]), so a value submitted for it is a
/// NAME its holder answers to rather than an address — and is held to the same
/// namespace rule as a username: a value of the format of an attribute the pool
/// ALIASES belongs to that alias's namespace
/// ([`UserPool::alias_format_username_defect`]). The ONE statement of that rule
/// for every writer of a user attribute — the two attribute updates and the two
/// registrations — so a name one of them refuses is not stored by another.
///
/// Bounded by the submitted list, with no allocation for a list that names no
/// such attribute.
pub fn submitted_name_defect<'submitted>(
    pool: &UserPool,
    submitted: impl IntoIterator<Item = (&'submitted str, &'submitted str)>,
) -> Option<String> {
    submitted
        .into_iter()
        .filter(|(name, _)| *name == PREFERRED_USERNAME_ATTRIBUTE)
        .find_map(|(_, value)| pool.alias_format_username_defect(value))
}

/// The message `InvalidParameterException` carries for a verified flag a request
/// sets where the attribute it vouches for would hold no value, or `None` for an
/// attribute that carries no such flag.
///
/// A verified flag records whether the value of its own attribute was confirmed
/// ([`verified_flag`]), so confirming or unconfirming one needs that value to
/// exist. Amazon Cognito states it as the value the flag needs and what the flag
/// does, naming the attribute and nothing the request carried.
fn verified_flag_needs_a_value_message(attribute: &str) -> Option<String> {
    let (needed, vouched_for) = match attribute {
        EMAIL_ATTRIBUTE => ("Email", "an email"),
        PHONE_NUMBER_ATTRIBUTE => ("Phone number", "a phone number"),
        _ => return None,
    };
    Some(format!(
        "{needed} is required to verify/un-verify {vouched_for}"
    ))
}

/// Why `value` is not a value the attribute `name` can hold, or `None` when the
/// value has the form that attribute's values take.
///
/// A user attribute whose values have a FORM is checked against that form before
/// it is stored, whichever operation submitted it: "a phone number must be in
/// E.164 format" (`AdminCreateUser.UserAttributes` in this service's model),
/// which is a `+` followed by the number's digits, and an `email` holds an email
/// ADDRESS — the value a verification message to that attribute is addressed to
/// (`VerifiedAttributeType`, `DeliveryMediumType.EMAIL`) — so a value of either
/// attribute that is not one is a bad parameter rather than a value a user may
/// hold. Every other attribute's values are judged by the pool's schema alone, so
/// none is refused here.
///
/// Whether a value is an address at all is the workspace's one reading of that
/// form ([`is_email_address`]), so no two services disagree about it.
///
/// A BLANK value is no value at all — "to delete an attribute from your user,
/// submit the attribute in your API request with a blank value" (this service's
/// model) — so it is not refused for the form it does not have.
pub fn attribute_value_defect(name: &str, value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return None;
    }
    match name {
        PHONE_NUMBER_ATTRIBUTE if !is_e164(value) => Some(INVALID_PHONE_NUMBER_MESSAGE),
        EMAIL_ATTRIBUTE if !is_email_address(value) => Some(INVALID_EMAIL_MESSAGE),
        _ => None,
    }
}

/// Whether `value` is an email address: the workspace's one reading of that form
/// (`service_common::email_address`), so this service cannot disagree with another
/// about what an address is.
fn is_email_address(value: &str) -> bool {
    ::service_common::email_address::is_email_address(value)
}

/// Whether `value` is a phone number in E.164 form: a `+`, then between one and
/// [`E164_MAX_DIGITS`] decimal digits and nothing else.
///
/// The digit count is read off the bytes before they are examined, so a long
/// value is rejected on its length rather than scanned.
fn is_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    !digits.is_empty()
        && digits.len() <= E164_MAX_DIGITS
        && digits.bytes().all(|digit| digit.is_ascii_digit())
}

/// WHICH of Amazon Cognito's two schema validations of a user's submitted
/// attributes judged them, which is what words the refusal.
///
/// The pool's schema is one boundary, but the stage that applies it is not: a
/// REGISTRATION of a new account and an UPDATE of a stored user's attributes are
/// validated at different stages of this service, and each states its finding in
/// its own sentence. Both wordings are the service's own, and neither is derived
/// from the other: a caller picks the stage its operation is, and the message
/// follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaValidationStage {
    /// A registration of a new account (`SignUp`, `AdminCreateUser`): the refusal
    /// opens with [`SCHEMA_VALIDATION_PREFIX`] and names the attribute by its
    /// SCHEMA PATH ([`schema_path`]) and the validation its value did not satisfy.
    Registration,
    /// An update of the attributes of a user the pool already holds: the refusal
    /// is the per-attribute report ([`schema_validation_report`]), a line per
    /// offending attribute naming it under the `user.` prefix and the bound it
    /// missed as the schema declares it.
    Update,
}

impl SchemaValidationStage {
    /// How this stage states that the value submitted for the attribute `name`
    /// misses `bound`.
    fn bound_message(self, name: &str, bound: DeclaredBound<'_>) -> String {
        match self {
            Self::Registration => format!(
                "{SCHEMA_VALIDATION_PREFIX}{}: This attribute does not satisfy the {} validation \
                 defined in the schema.",
                schema_path(name),
                bound.validation()
            ),
            Self::Update => schema_validation_report([(
                name,
                SchemaValidationFinding::OutsideDeclaredBound(bound.sentence()),
            )]),
        }
    }

    /// How this stage states that the value submitted for the attribute `name` is
    /// not of the FORM its data type's values take.
    ///
    /// The REGISTRATION stage judges the submitted value against the pool's schema
    /// before an account exists, and reports a value of no numeric form in the
    /// envelope this service states a member's pattern constraint in, naming the
    /// value's position by the attribute's schema path, behind that stage's own
    /// prefix. The UPDATE stage reads the value as the number the attribute holds,
    /// so a value it cannot read a number out of is reported as the per-attribute
    /// finding it is ([`SchemaValidationFinding::UnparseableNumber`]): the attribute
    /// under the `user.` prefix and that nothing could be parsed, in that stage's
    /// own report.
    ///
    /// An attribute whose values are POINTS IN TIME is reported in its own
    /// sentence instead ([`timestamp_form_message`]), which both stages state the
    /// same way: what refused the value is the attribute's own meaning rather than
    /// the schema the pool's operator declared.
    fn number_form_message(self, name: &str) -> String {
        if let Some(message) = timestamp_form_message(name) {
            return message;
        }
        match self {
            Self::Registration => format!(
                "{SCHEMA_VALIDATION_PREFIX}1 validation error detected: Value at '{}' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {NUMBER_VALUE_PATTERN}",
                schema_path(name)
            ),
            Self::Update => {
                schema_validation_report([(name, SchemaValidationFinding::UnparseableNumber)])
            }
        }
    }

    /// How this stage states that the value submitted for the `Boolean` attribute
    /// `name` is not a boolean.
    ///
    /// A `Boolean` attribute's values are the two a boolean has, so a value that is
    /// neither is not a value of that attribute at all — reported as the schema
    /// finding it is: the update stage states its own report line, and the
    /// registration stage the same sentence under its envelope, naming the
    /// attribute by its schema path as every finding of that stage does.
    fn boolean_form_message(self, name: &str) -> String {
        match self {
            Self::Registration => format!(
                "{SCHEMA_VALIDATION_PREFIX}{}: {}",
                schema_path(name),
                SchemaValidationFinding::NotABoolean.sentence()
            ),
            Self::Update => {
                schema_validation_report([(name, SchemaValidationFinding::NotABoolean)])
            }
        }
    }
}

/// The standard attributes whose values are POINTS IN TIME rather than plain
/// numbers: `updated_at` is the OpenID Connect standard claim for the "time the
/// user's information was last updated", counted in seconds since the epoch,
/// which the standard schema declares as a `Number` (this service's own standard
/// schema entry for it). A value of such an
/// attribute is a unix timestamp, so one that is no number at all is reported for
/// the timestamp it is not rather than as a bad number.
const TIMESTAMP_ATTRIBUTES: [&str; 1] = ["updated_at"];

/// The message a value submitted for a TIMESTAMP attribute that is no timestamp
/// is refused with, or `None` for an attribute whose values are not points in
/// time ([`TIMESTAMP_ATTRIBUTES`]).
///
/// Amazon Cognito names the attribute the request submitted and the form its
/// values take, and quotes the rejected value nowhere.
fn timestamp_form_message(name: &str) -> Option<String> {
    TIMESTAMP_ATTRIBUTES.contains(&name).then(|| {
        format!("Invalid {name} timestamp format. It should be a unix timestamp.")
    })
}

/// The sentence every refusal of the REGISTRATION stage's schema validation opens
/// with: the submitted attributes were judged against the pool's schema, and one
/// of them did not pass it.
pub const SCHEMA_VALIDATION_PREFIX: &str = "Attributes did not conform to the schema: ";

/// The prefix the SCHEMA PATH of a custom attribute carries, under which a pool's
/// schema holds the attributes its operator declared for its users.
///
/// A registration-stage finding names the attribute by that path rather than by
/// the name the request wrote: "when you add an attribute with a `Name` value of
/// `MyAttribute`, Amazon Cognito creates the custom attribute
/// `custom:MyAttribute`. When `DeveloperOnlyAttribute` is `true`, Amazon Cognito
/// creates your attribute as `dev:MyAttribute`" (`SchemaAttributeType.Name`, this
/// service's model) — so the wire name is the declared name under one of those
/// prefixes, and the path is the declared name under this one.
const CUSTOM_SCHEMA_PATH: &str = "Custom.User.";

/// The wire prefixes that name a CUSTOM attribute of a pool, each of which
/// [`CUSTOM_SCHEMA_PATH`] replaces in a schema path.
const CUSTOM_ATTRIBUTE_PREFIXES: [&str; 2] = ["custom:", "dev:"];

/// The FORM the values of a `Number` attribute take, as the constraint a value
/// that is not of that form is refused against states it: an optionally signed
/// decimal number, written with digits of any script, carrying an optional
/// fraction and an optional exponent — or nothing at all, a blank value being no
/// value to store.
const NUMBER_VALUE_PATTERN: &str =
    r"$|^[+-]?((\p{Nd}+(\.\p{Nd}*)?)|(\.\p{Nd}+))([eE][+-]?\p{Nd}+)?";

/// [`NUMBER_VALUE_PATTERN`] as a matcher of a WHOLE submitted value: the pattern
/// is a constraint on the value, so what has to satisfy it is the value entire
/// rather than some run inside it.
static NUMBER_VALUE: ::std::sync::LazyLock<::regex::Regex> = ::std::sync::LazyLock::new(|| {
    ::regex::Regex::new(&format!("^(?:{NUMBER_VALUE_PATTERN})$"))
        .expect("the number attribute's own value pattern compiles")
});

/// The bound a submitted value missed, as the pool's schema declares it.
///
/// A schema attribute carries the constraints of its OWN data type —
/// "specifies the constraints for an attribute of the number type"
/// (`SchemaAttributeType.NumberAttributeConstraints`, whose `MinValue` is "the
/// minimum length of a number attribute value" and `MaxValue` the maximum) and
/// "... of the string type" (`StringAttributeConstraints`, with `MinLength` and
/// `MaxLength`), this service's model — so a value stored for that attribute has
/// to be within them. Which bound was missed is what a refusal states, as the
/// VALIDATION that bound is checked by ([`DeclaredBound::validation`]) or as the
/// bound the schema declares ([`DeclaredBound::sentence`]), one wording per
/// validation stage ([`SchemaValidationStage`]); the submitted value itself is
/// never quoted back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclaredBound<'schema> {
    /// The value is a number above the declared `MaxValue`.
    NumberAbove(&'schema str),
    /// The value is a number below the declared `MinValue`.
    NumberBelow(&'schema str),
    /// The value is longer than the declared `MaxLength`.
    StringLonger(&'schema str),
    /// The value is shorter than the declared `MinLength`.
    StringShorter(&'schema str),
}

impl DeclaredBound<'_> {
    /// The sentence this violation is reported with where the bound itself is
    /// stated: the bound that was missed, in the units its data type is bounded in
    /// — a number against the value it may not pass, a string against the number
    /// of characters it may not pass.
    fn sentence(self) -> String {
        match self {
            Self::NumberAbove(max_value) => format!("Number must be less than {max_value}"),
            Self::NumberBelow(min_value) => format!("Number must be greater than {min_value}"),
            Self::StringLonger(max_length) => {
                format!("String must be no longer than {max_length} characters")
            }
            Self::StringShorter(min_length) => {
                format!("String must be no shorter than {min_length} characters")
            }
        }
    }

    /// The VALIDATION this bound is checked by, which is what a refusal that names
    /// the validation rather than the bound states: the constraint member that
    /// refused the value, as this service's schema validation names it.
    fn validation(self) -> &'static str {
        match self {
            Self::NumberAbove(_) => "MAX_VALUE_VALIDATION",
            Self::NumberBelow(_) => "MIN_VALUE_VALIDATION",
            Self::StringLonger(_) => "MAX_LENGTH_VALIDATION",
            Self::StringShorter(_) => "MIN_LENGTH_VALIDATION",
        }
    }
}

/// The constraint of its schema attribute that a submitted value does not satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaConstraint<'schema> {
    /// A BOUND the schema declares for the attribute.
    Bound(DeclaredBound<'schema>),
    /// The form a `Number` attribute's values take
    /// ([`NUMBER_VALUE_PATTERN`]), which a value that is not a number at all
    /// does not have.
    NumberForm,
    /// The form a `Boolean` attribute's values take ([`BOOLEAN_VALUES`]), which a
    /// value that is not a boolean does not have.
    BooleanForm,
}

/// The values a `Boolean` schema attribute can hold, carried as the strings every
/// attribute value is carried as: the two a boolean has, which are the two this
/// service itself stores for a verified flag ([`FLAG_CONFIRMED`],
/// [`FLAG_UNCONFIRMED`]).
const BOOLEAN_VALUES: [&str; 2] = [FLAG_CONFIRMED, FLAG_UNCONFIRMED];

/// The `AttributeDataType` whose values are booleans
/// ([`crate::ops::types::ATTRIBUTE_DATA_TYPE_VALUES`]).
const BOOLEAN_ATTRIBUTE_DATA_TYPE: &str = "Boolean";

/// Whether `pool`'s schema declares the attribute `name` as one whose values are
/// BOOLEANS (`SchemaAttributeType.AttributeDataType` of
/// [`BOOLEAN_ATTRIBUTE_DATA_TYPE`]), so the only values a user of the pool can
/// hold for it are the two a boolean has ([`BOOLEAN_VALUES`]).
///
/// Asked by a caller that reads a boolean out of text written in some other
/// spelling than the one this service stores it in, so that it can store the
/// stored spelling instead of refusing the value. An attribute the schema does
/// not declare, or declares without a data type, declares no booleans: the type
/// an entry defaults to is [`DEFAULT_ATTRIBUTE_DATA_TYPE`].
#[must_use]
pub fn declares_boolean_attribute(pool: &UserPool, name: &str) -> bool {
    pool.schema_attribute(name)
        .and_then(|entry| entry.attribute_data_type.as_deref())
        .is_some_and(|data_type| data_type == BOOLEAN_ATTRIBUTE_DATA_TYPE)
}

/// The value this service STORES for the boolean `flag`, as the strings every
/// attribute value is carried as ([`FLAG_CONFIRMED`], [`FLAG_UNCONFIRMED`]).
#[must_use]
pub fn stored_boolean(flag: bool) -> &'static str {
    if flag { FLAG_CONFIRMED } else { FLAG_UNCONFIRMED }
}

/// The path `pool`'s schema holds the attribute `name` under: a custom attribute
/// under [`CUSTOM_SCHEMA_PATH`], carrying the name it was declared with rather
/// than the prefixed name a request writes it by, and a standard attribute under
/// its own name, which is the name its schema entry is declared with.
fn schema_path(name: &str) -> String {
    match CUSTOM_ATTRIBUTE_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    {
        Some(declared) => format!("{CUSTOM_SCHEMA_PATH}{declared}"),
        None => name.to_string(),
    }
}

/// The refusal `stage` answers the `submitted` `(attribute, value)` pairs whose
/// values do not satisfy a constraint `pool`'s schema declares for their
/// attribute, or `None` when every submitted value satisfies the constraints of
/// its attribute.
///
/// The pool's schema is the boundary every write of a user attribute is judged
/// against, whichever operation submitted it: the entry for an attribute declares
/// the constraints its values take ([`SchemaConstraint`]), so a value that misses
/// one is a value no user of the pool can hold and the write is refused. WHICH
/// sentence states that is the stage's ([`SchemaValidationStage`]), and so is how
/// many findings it carries: the registration stage's message names one attribute
/// and one validation, so the finding reported is the first the submitted
/// attributes produce; the update stage's report states a line per offending
/// attribute, in the order they were submitted.
///
/// A value that is not of the FORM its data type's values take is judged ahead of
/// every declared bound, whatever attribute carries it: such a value is outside no
/// bound, there being nothing to compare.
///
/// A BLANK value is no value at all — "to delete an attribute from your user,
/// submit the attribute in your API request with a blank value" (this service's
/// model) — so no constraint applies to it.
///
/// Bounded by the submitted list, with one lookup of the pool's schema per entry
/// and nothing allocated for a value its attribute admits.
pub fn schema_constraint_refusal<'submitted>(
    pool: &UserPool,
    stage: SchemaValidationStage,
    submitted: impl IntoIterator<Item = (&'submitted str, &'submitted str)>,
) -> Option<String> {
    let findings: Vec<(&'submitted str, SchemaConstraint<'_>)> = submitted
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .filter_map(|(name, value)| {
            schema_constraint_missed(pool, name, value).map(|missed| (name, missed))
        })
        .collect();
    if let Some((name, _)) = findings
        .iter()
        .find(|(_, missed)| matches!(missed, SchemaConstraint::NumberForm))
    {
        return Some(stage.number_form_message(name));
    }
    if let Some((name, _)) = findings
        .iter()
        .find(|(_, missed)| matches!(missed, SchemaConstraint::BooleanForm))
    {
        return Some(stage.boolean_form_message(name));
    }
    let mut bounds = findings.iter().filter_map(|(name, missed)| match missed {
        SchemaConstraint::Bound(bound) => Some((*name, *bound)),
        SchemaConstraint::NumberForm | SchemaConstraint::BooleanForm => None,
    });
    match stage {
        SchemaValidationStage::Registration => bounds
            .next()
            .map(|(name, bound)| stage.bound_message(name, bound)),
        SchemaValidationStage::Update => {
            let report = schema_validation_report(bounds.map(|(name, bound)| {
                (
                    name,
                    SchemaValidationFinding::OutsideDeclaredBound(bound.sentence()),
                )
            }));
            (!report.is_empty()).then_some(report)
        }
    }
}

/// Whether `value` satisfies every constraint `pool`'s schema declares for the
/// attribute `name`. [`schema_constraint_refusal`] states the refusal a caller
/// answers with; this answers the question alone, for a caller that reports no
/// message and so needs none built.
pub fn satisfies_schema_constraints(pool: &UserPool, name: &str, value: &str) -> bool {
    value.is_empty() || schema_constraint_missed(pool, name, value).is_none()
}

/// The constraint `pool`'s schema declares for the attribute `name` that `value`
/// does not satisfy, or `None` when the value satisfies every one of them.
///
/// Which constraints apply is the attribute's `AttributeDataType` — a `Number`
/// attribute's values have the form of a number and are bounded by value, a
/// `String` attribute's are bounded by length, a `Boolean` attribute's are the two
/// values a boolean has ([`BOOLEAN_VALUES`]), and an attribute of any other type
/// carries no structure, so nothing constrains its values. An
/// entry naming no data type is a `String` one, the type an attribute defaults to
/// ([`DEFAULT_ATTRIBUTE_DATA_TYPE`]). An attribute the schema does not declare has
/// no entry to read and is refused as undeclared instead
/// ([`plan_attribute_update`]).
///
/// A value's FORM is judged before the bounds of its own type: a value that is not
/// a number is outside no numeric bound, there being nothing to compare.
fn schema_constraint_missed<'pool>(
    pool: &'pool UserPool,
    name: &str,
    value: &str,
) -> Option<SchemaConstraint<'pool>> {
    let entry = pool.schema_attribute(name)?;
    match entry
        .attribute_data_type
        .as_deref()
        .unwrap_or(DEFAULT_ATTRIBUTE_DATA_TYPE)
    {
        NUMBER_ATTRIBUTE_DATA_TYPE => {
            if !NUMBER_VALUE.is_match(value) {
                return Some(SchemaConstraint::NumberForm);
            }
            let submitted: f64 = value.parse().ok()?;
            let constraints = entry.number_attribute_constraints.as_ref()?;
            if let Some((declared, max_value)) = declared_number(constraints.max_value.as_deref()) {
                if submitted > max_value {
                    return Some(SchemaConstraint::Bound(DeclaredBound::NumberAbove(declared)));
                }
            }
            if let Some((declared, min_value)) = declared_number(constraints.min_value.as_deref()) {
                if submitted < min_value {
                    return Some(SchemaConstraint::Bound(DeclaredBound::NumberBelow(declared)));
                }
            }
            None
        }
        BOOLEAN_ATTRIBUTE_DATA_TYPE => {
            (!BOOLEAN_VALUES.contains(&value)).then_some(SchemaConstraint::BooleanForm)
        }
        STRING_ATTRIBUTE_DATA_TYPE => {
            let constraints = entry.string_attribute_constraints.as_ref()?;
            // A length is counted in CHARACTERS, which is what the constraint
            // bounds: a value the caller wrote in a non-ASCII script is as long
            // as the characters it has.
            let submitted = value.chars().count();
            if let Some((declared, max_length)) = declared_length(constraints.max_length.as_deref())
            {
                if submitted > max_length {
                    return Some(SchemaConstraint::Bound(DeclaredBound::StringLonger(declared)));
                }
            }
            if let Some((declared, min_length)) = declared_length(constraints.min_length.as_deref())
            {
                if submitted < min_length {
                    return Some(SchemaConstraint::Bound(DeclaredBound::StringShorter(
                        declared,
                    )));
                }
            }
            None
        }
        _ => None,
    }
}

/// A declared numeric bound as `(the text the schema declares it with, the number
/// that text names)`, or `None` where the schema declares no such bound or
/// declares one that names no number — a bound nothing can be compared against
/// bounds nothing.
fn declared_number(bound: Option<&str>) -> Option<(&str, f64)> {
    let declared = bound.map(str::trim).filter(|text| !text.is_empty())?;
    Some((declared, declared.parse().ok()?))
}

/// A declared length bound as `(the text the schema declares it with, the length
/// that text names)`, by the same rule as [`declared_number`].
fn declared_length(bound: Option<&str>) -> Option<(&str, usize)> {
    let declared = bound.map(str::trim).filter(|text| !text.is_empty())?;
    Some((declared, declared.parse().ok()?))
}

/// The attribute whose confirmation `flag` records, or `None` when `flag` is
/// not one of the verified flags ([`verified_flag`]).
pub(super) fn verified_attribute_of(flag: &str) -> Option<&'static str> {
    ["email", "phone_number"]
        .into_iter()
        .find(|attribute| verified_flag(attribute) == Some(flag))
}

/// Whether `name` is a verified flag whose attribute the same request DELETES, so
/// the deletion settles the flag and the value submitted for it writes nothing
/// ([`plan_attribute_update`]).
pub(super) fn settled_by_a_deletion(name: &str, submitted: &BTreeMap<&str, &str>) -> bool {
    verified_attribute_of(name)
        .is_some_and(|described| submitted.get(described).is_some_and(|value| value.is_empty()))
}

/// Whether `flag` says "confirmed" once this request has been applied: the
/// value the request sets for it if it names it, else `false` where the change
/// makes the flag stale, else the value the user already holds.
pub(super) fn flag_after_update(
    submitted: &BTreeMap<&str, &str>,
    user: &User,
    flag: &str,
    stale_verified_flag: &Option<&'static str>,
) -> bool {
    if let Some(value) = submitted.get(flag) {
        return *value == FLAG_CONFIRMED;
    }
    if stale_verified_flag.is_some() {
        return false;
    }
    user.attributes
        .get(flag)
        .is_some_and(|value| value == FLAG_CONFIRMED)
}

/// The `UserMFASettingList` value for SMS message MFA: that member is "the MFA
/// options that are activated for the user. The possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model), and
/// SMS is the one factor an `MFAOptionType` configures.
pub const SMS_MFA_SETTING: &str = "SMS_MFA";

/// The `UserMFASettingList` value for email message MFA, the second of the three
/// values that member documents (see [`SMS_MFA_SETTING`]).
pub const EMAIL_OTP_MFA_SETTING: &str = "EMAIL_OTP";

/// The `UserMFASettingList` value for time-based one-time password MFA, the third
/// of the three values that member documents (see [`SMS_MFA_SETTING`]).
pub const SOFTWARE_TOKEN_MFA_SETTING: &str = "SOFTWARE_TOKEN_MFA";

/// `AuthFactorType`'s password factor, which "can only be used as a first
/// authentication factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const PASSWORD_AUTH_FACTOR: &str = "PASSWORD";

/// `AuthFactorType`'s email one-time-password factor.
pub const EMAIL_OTP_AUTH_FACTOR: &str = "EMAIL_OTP";

/// `AuthFactorType`'s SMS one-time-password factor.
pub const SMS_OTP_AUTH_FACTOR: &str = "SMS_OTP";

/// `AuthFactorType`'s passkey factor.
pub const WEB_AUTHN_AUTH_FACTOR: &str = "WEB_AUTHN";

/// `AuthFactorType`'s time-based one-time-password factor, which "can only be
/// used as an MFA factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const SOFTWARE_TOKEN_AUTH_FACTOR: &str = "SOFTWARE_TOKEN";

/// Every `AuthFactorType` value, in the order the model DECLARES them in
/// (`smithy_query(query="shape",shape="AuthFactorType")`) rather than the
/// alphabetical order the generated `crate::ops::types::AUTH_FACTOR_TYPE_VALUES`
/// catalog carries them in. `ConfiguredUserAuthFactors` reports a user's factors
/// in that declared order, as its own example `["PASSWORD", "WEB_AUTHN"]` shows
/// (this service's model).
pub const AUTH_FACTOR_ORDER: [&str; 5] = [
    PASSWORD_AUTH_FACTOR,
    EMAIL_OTP_AUTH_FACTOR,
    SMS_OTP_AUTH_FACTOR,
    WEB_AUTHN_AUTH_FACTOR,
    SOFTWARE_TOKEN_AUTH_FACTOR,
];

/// The `WebAuthnFactorConfigurationType` value under which "passkey
/// authentication with user verification satisfies MFA requirements"
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model); its
/// only other value makes passkeys a single authentication factor.
pub const WEB_AUTHN_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";

/// The `AuthFactorType` value a `UserMFASettingList` setting activates as an MFA
/// factor, or `None` for a setting that names no `AuthFactorType` value. The one
/// place the two value sets are mapped to each other.
pub fn mfa_auth_factor_of(setting: &str) -> Option<&'static str> {
    match setting {
        SMS_MFA_SETTING => Some(SMS_OTP_AUTH_FACTOR),
        EMAIL_OTP_MFA_SETTING => Some(EMAIL_OTP_AUTH_FACTOR),
        SOFTWARE_TOKEN_MFA_SETTING => Some(SOFTWARE_TOKEN_AUTH_FACTOR),
        _ => None,
    }
}

/// The only `DeliveryMediumType` an `MFAOptionType` may set:
/// `MFAOptionType.DeliveryMedium` is "the delivery medium to send the MFA code.
/// You can use this parameter to set only the `SMS` delivery medium value" (this
/// service's model).
pub const MFA_OPTION_DELIVERY_MEDIUM: &str = "SMS";

/// The only attribute an `MFAOptionType` may name:
/// `MFAOptionType.AttributeName` is "the attribute name of the MFA option type.
/// The only valid value is `phone_number`" (this service's model).
pub const MFA_OPTION_ATTRIBUTE: &str = "phone_number";

/// The one SMS MFA option a user holds while SMS message MFA is activated for
/// it: an option sets the only delivery medium ([`MFA_OPTION_DELIVERY_MEDIUM`])
/// and names the only attribute ([`MFA_OPTION_ATTRIBUTE`]) an `MFAOptionType`
/// may carry, so a user holds either this option or none.
pub fn sms_mfa_option() -> MFAOptionType {
    MFAOptionType {
        delivery_medium: Some(MFA_OPTION_DELIVERY_MEDIUM.to_string()),
        attribute_name: Some(MFA_OPTION_ATTRIBUTE.to_string()),
    }
}

/// What one submitted MFA settings structure asks for one factor
/// ([`User::set_mfa_preference`]): the `UserMFASettingList` setting the factor is
/// reported under, whether the request activates it, and whether the request
/// makes it the user's preferred factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfaSettingRequest<'a> {
    pub setting: &'a str,
    pub enabled: bool,
    pub preferred: bool,
}

impl MfaSettingRequest<'_> {
    /// Whether this setting asks to ACTIVATE the factor, which is what needs the
    /// delivery configuration the factor reaches the user through. A setting that
    /// does not turns the factor off, which needs nothing.
    pub fn activates(&self) -> bool {
        self.enabled
    }
}

/// Which MFA factors the POOL a user belongs to turns on
/// ([`UserPool::mfa_factors_offered`]), which is what an activation the user
/// cannot receive is judged against: the pool's own configuration of a factor and
/// the user's enrolment in it are two different preconditions, and the refusal
/// reports the one that is missing ([`User::set_mfa_preference`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MfaFactorsOffered {
    /// Whether the pool turns TOTP MFA on: `SoftwareTokenMfaConfigType.Enabled`
    /// is "the activation state of TOTP MFA" for a user pool (this service's
    /// model), so a pool no `SetUserPoolMfaConfig` turned it on for offers the
    /// factor to nobody.
    pub software_token: bool,
}

/// Why a submitted MFA preference is refused ([`User::set_mfa_preference`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaPreferenceRejection {
    /// The request marks more than one factor as the user's preferred one, which
    /// is a conflict within the request itself.
    TwoPreferred,
    /// The `UserMFASettingList` setting the request asks to ACTIVATE while no
    /// delivery configuration the factor would reach the user through is in
    /// place.
    NoDeliveryConfig(String),
    /// The request asks to ACTIVATE software token MFA in a pool that DOES turn
    /// that factor on, for a user that has not verified an authenticator for it:
    /// the configuration the factor is turned on with is in place and it is the
    /// user's own enrolment that is missing.
    SoftwareTokenNotVerified,
    /// The request asks for a factor to be the user's PREFERRED one while not
    /// activating it: a factor that is off is not one a user prefers.
    PreferredWhileDisabled,
}

impl MfaPreferenceRejection {
    /// Which precondition of ACTIVATING the `UserMFASettingList` setting
    /// `setting` a user that holds no delivery configuration for it is missing,
    /// given what its pool offers ([`MfaFactorsOffered`]).
    ///
    /// Activating a factor needs TWO things: the pool's own configuration of that
    /// factor, which `SetUserPoolMfaConfig` writes, and the user's enrolment in
    /// it ([`User::holds_mfa_delivery_config`]). Where the pool turns TOTP MFA on
    /// and the user has registered no authenticator, the configuration the factor
    /// is turned on with is in place and the step the USER still owes is its own
    /// verification, which is what the refusal reports
    /// ([`SOFTWARE_TOKEN_NOT_VERIFIED_MESSAGE`]); every other missing
    /// precondition is reported as the delivery configuration the factor would
    /// reach the user through, naming the setting the request asked for.
    ///
    /// A pure function of the two arguments: it decides what the refusal reports
    /// and reads or writes nothing stored.
    pub fn for_activation(setting: &str, offered: MfaFactorsOffered) -> Self {
        if setting == SOFTWARE_TOKEN_MFA_SETTING && offered.software_token {
            return Self::SoftwareTokenNotVerified;
        }
        Self::NoDeliveryConfig(setting.to_string())
    }

    /// What the refusal reports.
    ///
    /// A request marking two factors preferred reports the conflict and the one
    /// preference a user can hold, and names neither factor: "only one factor can
    /// be set as preferred" (`SetUserMFAPreference` in this service's model), so
    /// what is refused is the pair of preferences rather than either factor.
    ///
    /// An activation no delivery configuration is in place for reports that
    /// configuration, naming the factor the request asked for — the same sentence
    /// for every factor, whether what is missing is a message medium or the
    /// pool's activation of TOTP MFA — and the only part taken from the request
    /// is the `UserMFASettingList` setting it named. Where the pool DOES turn
    /// software token MFA on and only the user's own authenticator is
    /// unverified, the refusal reports that step instead
    /// ([`SOFTWARE_TOKEN_NOT_VERIFIED_MESSAGE`]), so the two preconditions a
    /// caller has to satisfy are told apart: an administrator's pool
    /// configuration and the user's own enrolment.
    ///
    /// A preference over a factor the request leaves off reports that alone and
    /// names NO factor, a factor that is off not being one a user prefers, so
    /// the refusal is about the preference rather than about any one factor.
    /// Nothing stored is disclosed by any of them.
    pub fn message(&self) -> String {
        match self {
            Self::TwoPreferred => TWO_PREFERRED_MFA_TYPES_MESSAGE.to_string(),
            Self::NoDeliveryConfig(setting) => {
                format!("User does not have delivery config set to turn on {setting}")
            }
            Self::SoftwareTokenNotVerified => SOFTWARE_TOKEN_NOT_VERIFIED_MESSAGE.to_string(),
            Self::PreferredWhileDisabled => "Can't mark a disabled mfa as preferred".to_string(),
        }
    }
}

/// The sentence a request marking more than one MFA factor as the user's
/// preferred one is refused with: "only one factor can be set as preferred"
/// (`SetUserMFAPreference` in this service's model), so a request expressing two
/// preferences expresses one the user cannot hold.
///
/// Entirely fixed text, with no part taken from the request: which two factors
/// were marked does not change that only one preference can be held.
pub const TWO_PREFERRED_MFA_TYPES_MESSAGE: &str =
    "Two MFA types marked as preferred, only one is allowed.";

/// The sentence an activation of software token MFA is refused with when the pool
/// turns that factor on but the user has not VERIFIED an authenticator for it.
///
/// "Users must register a TOTP authenticator before they set this as their
/// preferred MFA method" (`SetUserMFAPreference.SoftwareTokenMfaSettings` in this
/// service's model) and a verification "marks the user's software token MFA
/// status as verified" (`VerifySoftwareToken`), so the step this user still owes
/// is its own verification rather than anything an administrator configures for
/// the pool.
///
/// Entirely fixed text, with no part taken from the request or from state:
/// neither the user, nor the pool, nor the factor's setting name appears in it.
pub const SOFTWARE_TOKEN_NOT_VERIFIED_MESSAGE: &str = "User has not verified software token mfa";

/// Why a submitted `MFAOptionListType` configures no SMS MFA option
/// ([`sms_mfa_options`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaOptionRejection {
    /// The submitted delivery medium, which is not the one SMS MFA travels by,
    /// or `None` where the option set no medium at all.
    DeliveryMedium(Option<String>),
    /// The submitted attribute name, which is not the one an MFA option may
    /// name, or `None` where the option named no attribute at all.
    AttributeName(Option<String>),
}

impl MfaOptionRejection {
    /// What the refusal reports: the member of the submitted option that an SMS
    /// MFA configuration cannot carry, and the value submitted for it. Only
    /// submitted values are named, so nothing stored is disclosed.
    pub fn message(&self) -> String {
        match self {
            Self::DeliveryMedium(Some(medium)) => format!(
                "Invalid DeliveryMedium: {medium}, the only delivery medium an MFA option can \
                 set is {MFA_OPTION_DELIVERY_MEDIUM}"
            ),
            Self::DeliveryMedium(None) => format!(
                "Invalid MFAOptions: an MFA option must set {MFA_OPTION_DELIVERY_MEDIUM} as its \
                 DeliveryMedium"
            ),
            Self::AttributeName(Some(name)) => format!(
                "Invalid AttributeName: {name}, the only attribute name an MFA option can name \
                 is {MFA_OPTION_ATTRIBUTE}"
            ),
            Self::AttributeName(None) => format!(
                "Invalid MFAOptions: an MFA option must name {MFA_OPTION_ATTRIBUTE} as its \
                 AttributeName"
            ),
        }
    }
}

/// The SMS MFA configuration a submitted `MFAOptionListType` asks one user to
/// hold, or the refusal it earns.
///
/// `MFAOptions` is a parameter "you can use only to set an SMS configuration
/// that uses SMS for delivery" (`SetUserSettings` and `AdminSetUserSettings` in
/// this service's model), so every submitted option has to set SMS delivery of
/// the one attribute an option may name. A user holds ONE value per attribute,
/// so the same option submitted twice is still one configuration and the
/// DISTINCT options are what is stored.
///
/// A PURE function of the request: it decides the configuration without reading
/// or writing stored state, so a request refused here writes nothing. Each
/// submitted option costs one constant-time check and the configuration it
/// yields holds at most one option per attribute an option may name, so the
/// length of the submitted list bounds nothing that is stored.
pub fn sms_mfa_options(
    requested: &[MFAOptionType],
) -> Result<Vec<MFAOptionType>, MfaOptionRejection> {
    let mut stored: Vec<MFAOptionType> = Vec::new();
    for option in requested {
        match option.delivery_medium.as_deref() {
            Some(MFA_OPTION_DELIVERY_MEDIUM) => {}
            other => {
                return Err(MfaOptionRejection::DeliveryMedium(
                    other.map(str::to_string),
                ))
            }
        }
        match option.attribute_name.as_deref() {
            Some(MFA_OPTION_ATTRIBUTE) => {}
            other => return Err(MfaOptionRejection::AttributeName(other.map(str::to_string))),
        }
        if !stored
            .iter()
            .any(|kept| kept.attribute_name == option.attribute_name)
        {
            stored.push(option.clone());
        }
    }
    Ok(stored)
}

/// Where a password-reset message for one user goes
/// ([`UserPool::recovery_destination`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDestination {
    /// The attribute the message is addressed to, and the stored value it goes
    /// to.
    Delivered {
        attribute: String,
        destination: String,
    },
    /// This pool reserves password resets to administrators (`admin_only`), so
    /// no self-service message is sent at all.
    AdminOnly,
    /// The user holds no verified address or number for any mechanism this pool
    /// allows, so there is nowhere to send a reset code.
    None,
}

/// The `RecoveryOptionNameType` value that "prevents self-service account
/// recovery" (`RecoveryOptionType` in this service's model).
pub const RECOVERY_ADMIN_ONLY: &str = "admin_only";

/// The message a password reset is refused with where the pool reserves account
/// recovery to administrators ([`RecoveryDestination::AdminOnly`]): no
/// password-reset message is sent at all, so no reset can be begun through the
/// API that sends one.
pub const ADMIN_ONLY_RECOVERY_MESSAGE: &str = "Contact administrator to reset password.";

/// The message a password reset is refused with where the user holds no verified
/// address a reset message could go to ([`RecoveryDestination::None`]): "if
/// neither a verified phone number nor a verified email exists, Amazon Cognito
/// responds with an InvalidParameterException error" (`ForgotPassword` in this
/// service's model).
pub const NO_RECOVERY_DESTINATION_MESSAGE: &str =
    "Cannot reset password for the user as there is no registered/verified email or phone_number";

/// The recovery mechanisms a pool that configures none gets, in priority order:
/// Amazon Cognito's default account recovery is the verified email address,
/// falling back to the verified phone number (the two attribute-bearing
/// `RecoveryOptionNameType` values).
pub(super) const DEFAULT_RECOVERY_MECHANISMS: [&str; 2] = ["verified_email", "verified_phone_number"];

/// The `AccountRecoverySetting` a pool is STORED with, which is the one a
/// request carried or Amazon Cognito's own default.
///
/// A pool always has an effective account-recovery configuration — it is what
/// decides where a self-service reset message goes
/// ([`UserPool::recovery_destination`]) — so a request that configures none is
/// stored with the service default rather than with nothing, and
/// `DescribeUserPool` reports the configuration the pool actually applies.
/// The default is [`DEFAULT_RECOVERY_MECHANISMS`] in that order, each
/// mechanism's `Priority` being its rank from 1, since "groups ... take
/// precedence" by ascending priority ("if `verified_email` has a priority of 1
/// and `verified_phone_number` has a priority of 2, your user pool sends
/// account-recovery messages to a verified email address but falls back to an
/// SMS message", `RecoveryOptionType` in this service's model).
pub fn effective_account_recovery_setting(
    requested: Option<AccountRecoverySettingType>,
) -> AccountRecoverySettingType {
    requested.unwrap_or_else(|| AccountRecoverySettingType {
        recovery_mechanisms: Some(
            DEFAULT_RECOVERY_MECHANISMS
                .iter()
                .enumerate()
                .map(|(index, name)| RecoveryOptionType {
                    priority: i32::try_from(index).unwrap_or_default() + 1,
                    name: (*name).to_string(),
                })
                .collect(),
        ),
    })
}

/// The user attribute a `RecoveryOptionNameType` recovers through, or `None` for
/// a mechanism that names no attribute (`admin_only`).
pub(super) fn recovered_attribute(mechanism: &str) -> Option<&'static str> {
    match mechanism {
        "verified_email" => Some("email"),
        "verified_phone_number" => Some("phone_number"),
        _ => None,
    }
}

/// The recovered attribute whose reset message is delivered over SMS \u2014 the one
/// `RecoveryOptionNameType` mechanism that does not qualify for a user with SMS
/// MFA activated ([`UserPool::recovery_destination`]).
pub const SMS_RECOVERED_ATTRIBUTE: &str = "phone_number";

/// How many trailing characters of a masked phone number stay readable, which
/// is what identifies the number to the user who gave it.
pub(super) const UNMASKED_DESTINATION_SUFFIX: usize = 4;

/// `destination` as a `CodeDeliveryDetailsType.Destination`.
///
/// Amazon Cognito reports the address it sent the code to MASKED, so the reply
/// tells the caller which of the user's addresses was used without disclosing
/// an address the caller does not already hold. An email address keeps only the
/// FIRST character of its local part and the first character of its domain,
/// each followed by a fixed `***` run — nothing of the domain past that first
/// character is disclosed, not even its last label; anything else keeps a
/// leading `+` and its last [`UNMASKED_DESTINATION_SUFFIX`] characters.
pub fn masked_destination(destination: &str) -> String {
    if let Some((local, domain)) = destination.split_once('@') {
        let local_initial = local.chars().next().unwrap_or('*');
        let domain_initial = domain.chars().next().unwrap_or('*');
        return format!("{local_initial}***@{domain_initial}***");
    }
    let characters: Vec<char> = destination.chars().collect();
    let readable_from = characters
        .len()
        .saturating_sub(UNMASKED_DESTINATION_SUFFIX);
    characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            if index >= readable_from || (index == 0 && *character == '+') {
                *character
            } else {
                '*'
            }
        })
        .collect()
}

/// The delivery details a message to a SIMULATED destination is reported with,
/// for a request that named `username`.
///
/// `PreventUserExistenceErrors` set to `ENABLED` "suppresses messages that might
/// indicate a valid user exists" (this service's model): the account-confirmation
/// and password-recovery operations therefore answer a request naming a user
/// nobody registered exactly as they answer one naming a user that exists —
/// reporting a delivery — rather than with a refusal that tells the caller the
/// account does not exist. Nothing was sent and nothing is stored, so the
/// destination reported is a SIMULATED one, and it is derived from the request
/// that asked for it ([`simulated_destination`]) rather than being a literal:
/// a destination that is the same characters whatever the request named tells
/// the caller, by itself, that no such user exists.
pub fn simulated_code_delivery_details(username: &str) -> Option<CodeDeliveryDetailsType> {
    code_delivery_details(
        SIMULATED_DESTINATION_ATTRIBUTE,
        &simulated_destination(username),
    )
}

/// The attribute a simulated delivery is reported against: an email address is
/// the destination every pool can address a message to, and the medium it
/// travels by is the one [`delivery_medium`] maps it to.
pub(super) const SIMULATED_DESTINATION_ATTRIBUTE: &str = "email";

/// The address [`simulated_code_delivery_details`] reports a code went to, read
/// out of the `Username` the request submitted — the one thing a request naming
/// a user nobody registered carries.
///
/// It is reported through the masking every real destination is reported through
/// ([`masked_destination`]), which discloses the first character of an address's
/// local part and the first character of its domain and nothing else. Both come
/// from the submitted username: a username already in address form gives each
/// from its own part, and one that carries no domain at all is read as the local
/// part alone, its first character standing for the domain's too. So the answer
/// for a user nobody registered has the shape AND the derivation of the answer
/// for one that exists, and differs between two usernames exactly as two real
/// addresses do.
pub(super) fn simulated_destination(username: &str) -> String {
    match username.split_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => username.to_string(),
        _ => format!("{username}@{username}"),
    }
}

/// The delivery details for a verification message sent to `value` for
/// `attribute`, or `None` for an attribute no message is sent for.
///
/// Rendered from the value the message goes to, masked as
/// [`masked_destination`] reports it.
pub fn code_delivery_details(attribute: &str, value: &str) -> Option<CodeDeliveryDetailsType> {
    Some(CodeDeliveryDetailsType {
        destination: Some(masked_destination(value)),
        delivery_medium: Some(delivery_medium(attribute)?.to_string()),
        attribute_name: Some(attribute.to_string()),
    })
}

/// The schema a new pool is created with: the standard attributes, with a
/// requested attribute of the same name overriding its properties, and every
/// other requested attribute added under the `custom:` prefix Amazon Cognito
/// gives custom attributes.
///
/// The stored ORDER is the pool's own, as created: "an array of attributes for
/// the new user pool. You can add custom attributes and modify the properties of
/// default attributes" (`CreateUserPool.Schema` in this service's model), so the
/// attributes the request names are stored in the order IT names them — a custom
/// one among the standard ones where the request put it — followed by the
/// standard attributes it does not name, in Amazon Cognito's standard claim
/// order. [`SUB_ATTRIBUTE`] leads the schema whatever the request says: the pool
/// mints it for every user rather than taking it from a caller. Every read of
/// the schema — `DescribeUserPool`, `GetCSVHeader` — reports that one order, and
/// a later `AddCustomAttributes` appends behind it.
///
/// The work is bounded and linear in the request: each requested attribute is
/// resolved through a name-keyed index rather than by scanning what the schema
/// already holds, the ordering reads each attribute's rank out of a map, and no
/// more attributes are taken than a pool may hold
/// ([`MAX_CUSTOM_ATTRIBUTES_PER_POOL`] beside the standard ones) — a bound
/// `CreateUserPool` refuses past before it calls this, so nothing a request can
/// reach is dropped here.
pub fn effective_schema(requested: Option<Vec<SchemaAttributeType>>) -> Vec<SchemaAttributeType> {
    let requested = requested.unwrap_or_default();
    let mut schema: Vec<SchemaAttributeType> =
        STANDARD_SCHEMA.iter().map(standard_schema_attribute).collect();

    // Where each stored attribute sits, so a requested one is resolved by name
    // instead of by scanning the schema.
    let mut position: BTreeMap<String, usize> = schema
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| entry.name.clone().map(|name| (name, at)))
        .collect();
    // The rank of each attribute the request names, in the order it names them.
    let mut requested_rank: BTreeMap<String, usize> = BTreeMap::new();

    let bound = STANDARD_SCHEMA.len().saturating_add(MAX_CUSTOM_ATTRIBUTES_PER_POOL);
    for attribute in requested.iter().take(bound) {
        let Some(name) = attribute.name.clone() else {
            continue;
        };
        if let Some(standard) = position.get(&name).and_then(|at| schema.get_mut(*at)) {
            standard.required = attribute.required.or(standard.required);
            standard.mutable = attribute.mutable.or(standard.mutable);
            if attribute.string_attribute_constraints.is_some() {
                standard.string_attribute_constraints =
                    attribute.string_attribute_constraints.clone();
            }
            if attribute.number_attribute_constraints.is_some() {
                standard.number_attribute_constraints =
                    attribute.number_attribute_constraints.clone();
            }
            let next = requested_rank.len();
            requested_rank.entry(name).or_insert(next);
            continue;
        }
        if let Some((stored_name, entry)) = custom_schema_attribute(attribute) {
            if position.contains_key(&stored_name) {
                continue;
            }
            position.insert(stored_name.clone(), schema.len());
            schema.push(entry);
            let next = requested_rank.len();
            requested_rank.insert(stored_name, next);
        }
    }

    // One attribute is stored under exactly one name, so ranking each entry and
    // sorting by that rank keeps the minted attribute first, then the requested
    // order, then the standard order a stable sort leaves the rest in.
    schema.sort_by_key(|entry| {
        let name = entry.name.as_deref().unwrap_or_default();
        if name == SUB_ATTRIBUTE {
            return (0, 0);
        }
        match requested_rank.get(name) {
            Some(rank) => (1, *rank),
            None => (2, 0),
        }
    });
    schema
}

/// The message a `CreateUserPool` whose `Schema` asks for more custom
/// attributes than a pool may hold is refused with: the per-pool quota is
/// [`MAX_CUSTOM_ATTRIBUTES_PER_POOL`], the same one `AddCustomAttributes`
/// enforces for a schema that grows after the pool exists, and the request
/// names no pool yet.
pub fn custom_attribute_quota_message() -> String {
    format!("A user pool holds at most {MAX_CUSTOM_ATTRIBUTES_PER_POOL} custom attributes.")
}

/// How many of `requested`'s attributes a pool would hold as CUSTOM ones: every
/// requested attribute that is not one of the standard attributes a pool already
/// carries, which are not counted against the quota.
pub fn requested_custom_attribute_count(requested: &[SchemaAttributeType]) -> usize {
    requested
        .iter()
        .filter(|attribute| {
            attribute
                .name
                .as_deref()
                .is_some_and(|name| !is_standard_attribute(name))
        })
        .count()
}

/// The `AttributeDataType` a requested custom attribute that names none is
/// stored with: `SchemaAttributeType.AttributeDataType` is "the data type of the
/// attribute" and `String` is the one every unqualified custom attribute reads
/// back as (this service's model). One of
/// [`crate::ops::types::ATTRIBUTE_DATA_TYPE_VALUES`].
pub const DEFAULT_ATTRIBUTE_DATA_TYPE: &str = "String";

/// The STORED NAME and the schema entry a requested CUSTOM attribute becomes,
/// with the defaults a custom attribute carries filled in.
///
/// Its stored name is the requested name under the prefix Amazon Cognito gives
/// it: "When you add an attribute with a `Name` value of `MyAttribute`, Amazon
/// Cognito creates the custom attribute `custom:MyAttribute`. When
/// `DeveloperOnlyAttribute` is `true`, Amazon Cognito creates your attribute as
/// `dev:MyAttribute`" (`SchemaAttributeType.Name`, this service's model). A name
/// that already carries one of those prefixes is stored as it is, so a schema
/// read and a re-add agree on the stored name.
///
/// `None` when the request names no attribute: an entry with no name could never
/// be found again. `CustomAttributeNameType` has a minimum length of 1, so only
/// a request the model does not admit reaches that answer.
pub fn custom_schema_attribute(
    requested: &SchemaAttributeType,
) -> Option<(String, SchemaAttributeType)> {
    let name = requested.name.clone()?;
    if name.is_empty() {
        return None;
    }
    let developer_only = requested.developer_only_attribute.unwrap_or(false);
    let prefix = if developer_only { "dev:" } else { "custom:" };
    let stored_name = if name.starts_with("custom:") || name.starts_with("dev:") {
        name
    } else {
        format!("{prefix}{name}")
    };
    let data_type = requested
        .attribute_data_type
        .clone()
        .unwrap_or_else(|| DEFAULT_ATTRIBUTE_DATA_TYPE.to_string());
    // The attribute reports the constraints its own data type has, holding the
    // bounds the request asked for and EMPTY where it asked for none — the same
    // rule the default schema's attributes report theirs by
    // ([`schema_attribute_constraints`]).
    let (min, max) = match data_type.as_str() {
        NUMBER_ATTRIBUTE_DATA_TYPE => (
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_value.as_deref()),
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_value.as_deref()),
        ),
        _ => (
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_length.as_deref()),
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_length.as_deref()),
        ),
    };
    let (number_attribute_constraints, string_attribute_constraints) =
        schema_attribute_constraints(&data_type, min, max);
    let entry = SchemaAttributeType {
        name: Some(stored_name.clone()),
        attribute_data_type: Some(data_type.clone()),
        developer_only_attribute: Some(developer_only),
        // A custom attribute is mutable unless the request says otherwise;
        // a custom attribute can never be required.
        mutable: Some(requested.mutable.unwrap_or(true)),
        required: Some(false),
        number_attribute_constraints,
        string_attribute_constraints,
    };
    Some((stored_name, entry))
}
