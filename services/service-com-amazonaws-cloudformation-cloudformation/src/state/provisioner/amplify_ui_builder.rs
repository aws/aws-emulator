// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amplify UI Builder: the component, the form and the
//! theme an `AWS::AmplifyUIBuilder::{Component,Form,Theme}` declares. One file of [`super`]'s
//! per-type arms, dispatched to by its `Provisioner`.
//!
//! ## Why the entities are really created
//!
//! "Ref returns the ID", and that id is the handle every call on one of these entities takes
//! — `GetComponent`, `UpdateForm`, `DeleteTheme`, the export calls and the tagging calls all
//! address one by the `id` of its own resource shape, beside the app and the environment it
//! lives in. A template publishes that id through an output or another resource's property,
//! and the first thing a client does with the deployed stack is call that service with it. A
//! stack that reported the entity created while nothing created it publishes an id that
//! resolves to nothing, so that first call fails — the stack completed and the client is
//! broken. Creating the entity where it belongs is what makes the id the stack publishes one
//! the owning service answers for.
//!
//! ## The identifier is the service's, the name is CloudFormation's
//!
//! Each create binds the app and the environment to its URI
//! (`POST /app/{appId}/environment/{environmentName}/{collection}`), takes the entity's NAME
//! in its payload and no identifier at all, and answers the entity carrying the `Uuid` that
//! service assigned. No name CloudFormation decides could stand in for that id, so the
//! identifier in force is the one the create answered: it IS this resource's physical id
//! ([`super::super::template::identity_is_service_assigned`] names all three types), and the
//! uuid-shaped stand-in the naming rules computed is only what a reference was resolved
//! against until then. The NAME the entity is created under stays CloudFormation's — the
//! `Name` the declaration states, or the generated one ([`entity_name`]).
//!
//! ## What the declaration is
//!
//! A property of one of these types is the member of the same name with a lower-cased initial
//! ([`lower_initial`]), all the way down — with two members CloudFormation decides lifted out
//! of the payload: the app environment, which the URI carries, and the tag MAP, which carries
//! the tags CloudFormation gives the resource beside the ones the declaration asks for.
//!
//! The re-casing is NOT uniform over the document, because several of these members are maps
//! the CALLER keys: a component's `Properties`, `Overrides`, `BindingProperties`,
//! `CollectionProperties` and `Events`, a child's and an action's own such maps, a variant's
//! values, a form's `Fields` and `SectionalElements`, and a value map's binding properties are
//! each keyed by the author's own property, field or element name. A key of one of those maps
//! is DATA and travels verbatim; everything else is a member name and is re-cased. Which is
//! which is a function of WHERE in the document the map sits — a component's own
//! `BindingProperties` is such a map while a `ComponentProperty`'s is a structure of two
//! members — so the walk below follows the shapes rather than matching names
//! ([`Shape`]).
//!
//! ## No `clientToken` is sent
//!
//! That member is an `@idempotencyToken`. A token derived from the resource would have to be
//! derived from something a second stack cannot repeat, and the value these arms have — the
//! entity's name — is not: two stacks may each declare a form of one name in one app
//! environment, and that is the `ResourceConflictException` the owning service answers, not a
//! replay of the form the other stack created.

use super::*;

/// Amplify UI Builder's signing name, from its model (`sigv4Name: amplifyuibuilder`), which
/// speaks `restJson1` — so its calls ride on [`call_rest_json_named`], each on the URI its own
/// operation's `@http` binding declares, naming the operation so a refusal is reported the way
/// that service's own SDK raises it.
const AMPLIFYUIBUILDER_SIGNING_NAME: &str = "amplifyuibuilder";

/// The error code Amplify UI Builder answers for an entity it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of every read, update and delete of one).
const AMPLIFYUIBUILDER_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The three members of the create's payload that CloudFormation decides, as that payload
/// spells them: the entity's name, and the tag map the propagated tags travel in.
const NAME_MEMBER: &str = "name";
const TAGS_MEMBER: &str = "tags";

/// The properties the declaration states the app environment the entity belongs to in, which
/// are also the names the type publishes those two values under.
const APP_ID_PROPERTY: &str = "AppId";
const ENVIRONMENT_NAME_PROPERTY: &str = "EnvironmentName";

/// The property the declaration states the entity's name in.
const NAME_PROPERTY: &str = "Name";

/// The attribute carrying the id Amplify UI Builder minted, which is the last part of the
/// identity the type is addressed by.
const ID_ATTRIBUTE: &str = "Id";

/// The property the declared tag map lives in, which is lifted out of the payload.
const TAGS_PROPERTY: &str = "Tags";

/// The members an answered entity reports, as `(attribute, member)`: the values each of these
/// types publishes ([`super::super::template::published_attribute_names`]), read off what that
/// service answered rather than recomputed here.
///
/// The two instants are a COMPONENT's and a THEME's; a form's structure carries neither, so
/// nothing is reported for one ([`reported_attributes`] contributes only what the answer
/// really carries).
const ENTITY_ATTRIBUTES: &[(&str, &str)] = &[
    ("AppId", "appId"),
    ("EnvironmentName", "environmentName"),
    ("Id", "id"),
    ("CreatedAt", "createdAt"),
    ("ModifiedAt", "modifiedAt"),
];

/// Which entity of Amplify UI Builder one of these three types declares.
///
/// The three have one lifecycle between them — the same create, read, update and delete,
/// differing only in the COLLECTION their URIs name, the payload member the create takes and
/// which properties the entity's own read reports back — so each arm below takes the kind and
/// the one dispatch in [`super::Provisioner`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiEntity {
    Component,
    Form,
    Theme,
}

impl UiEntity {
    /// The collection the entity's URIs name, which is the element its own ARN template and
    /// every one of its `@http` bindings carry
    /// (`/app/{appId}/environment/{environmentName}/components`).
    fn collection(self) -> &'static str {
        match self {
            Self::Component => "components",
            Self::Form => "forms",
            Self::Theme => "themes",
        }
    }

    /// The operations this module calls for the entity, as that service's own model names
    /// them: `(create, read, update, delete)`.
    fn operations(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Self::Component => (
                "CreateComponent",
                "GetComponent",
                "UpdateComponent",
                "DeleteComponent",
            ),
            Self::Form => ("CreateForm", "GetForm", "UpdateForm", "DeleteForm"),
            Self::Theme => ("CreateTheme", "GetTheme", "UpdateTheme", "DeleteTheme"),
        }
    }

    /// The shape of the create's and the update's payload for the entity, which is what the
    /// re-casing walk follows ([`recased`]).
    fn payload(self) -> Shape {
        match self {
            Self::Component => Shape::ComponentData,
            Self::Form => Shape::FormData,
            // A theme's `Values` and `Overrides` are LISTS of `{Key, Value}` structures, so
            // every key of a theme's own document is a member name.
            Self::Theme => Shape::Members,
        }
    }

    /// The properties the entity's own read reports back under the same names, for drift
    /// detection, as `(property, member)`: the SCALAR members a declaration also states.
    ///
    /// The configuration documents are left out: what the read reports are that service's own
    /// member names, and a property the read does not report is not evidence of anything
    /// ([`super::drift`]).
    fn reported_properties(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Component => &[
                ("AppId", "appId"),
                ("EnvironmentName", "environmentName"),
                ("Name", "name"),
                ("ComponentType", "componentType"),
                ("SourceId", "sourceId"),
                ("SchemaVersion", "schemaVersion"),
            ],
            Self::Form => &[
                ("AppId", "appId"),
                ("EnvironmentName", "environmentName"),
                ("Name", "name"),
                ("FormActionType", "formActionType"),
                ("LabelDecorator", "labelDecorator"),
                ("SchemaVersion", "schemaVersion"),
            ],
            Self::Theme => &[
                ("AppId", "appId"),
                ("EnvironmentName", "environmentName"),
                ("Name", "name"),
            ],
        }
    }
}

/// Create the component, form or theme the declaration asks for, in Amplify UI Builder — so a
/// client that deployed the stack reads the entity back through that service's own
/// `GetComponent`/`GetForm`/`GetTheme` for the id the stack publishes, and finds it in the
/// app environment's listing, rather than a `ResourceNotFoundException` for an entity nothing
/// created.
///
/// The declaration IS the payload once its member names are re-cased ([`entity_payload`]),
/// under the NAME CloudFormation decided, with the declared tags beside the ones
/// CloudFormation gives the resource ([`tags_of`]) as the map that call's own `tags` member
/// takes.
///
/// The identity is that service's: the TRIPLE the type is identified by — the app, the
/// environment and the `id` Amplify UI Builder minted — becomes this resource's physical id
/// ([`template::amplifyuibuilder_identity`]), and the values the type publishes are read off
/// the entity it answered.
pub(super) async fn create_ui_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: UiEntity,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    // The app environment is read off the DECLARATION here, not off the identity: the
    // declaration is the resolved request (a reference to the app resource carries the id
    // that service minted by now), while the identity the naming rules computed is only a
    // stand-in until this create answers.
    let app_id = declared_scalar(&resource.properties, APP_ID_PROPERTY);
    let environment_name = declared_scalar(&resource.properties, ENVIRONMENT_NAME_PROPERTY);
    let mut payload = entity_payload(entity, &resource.properties)?;
    payload.insert(
        NAME_MEMBER.to_string(),
        Value::String(entity_name(&resource.properties, resource)),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        payload.insert(TAGS_MEMBER.to_string(), Value::Object(tags_as_map(&tags)));
    }
    let (create, _, _, _) = entity.operations();
    let answered = call_rest_json_named(
        ctx,
        AMPLIFYUIBUILDER_SIGNING_NAME,
        region,
        create,
        "POST",
        &collection_uri(entity, &app_id, &environment_name),
        Some(&Value::Object(payload)),
    )
    .await?;
    let attributes = entity_attributes(entity, &answered)?;
    Ok(Created {
        // Every part of the identity is read off the entity that service answered, which is
        // where the app and the environment it really belongs to are reported too.
        physical_resource_id: Some(template::amplifyuibuilder_identity(
            attributes
                .get(APP_ID_PROPERTY)
                .map(String::as_str)
                .unwrap_or(&app_id),
            attributes
                .get(ENVIRONMENT_NAME_PROPERTY)
                .map(String::as_str)
                .unwrap_or(&environment_name),
            attributes
                .get(ID_ATTRIBUTE)
                .map(String::as_str)
                .unwrap_or_default(),
        )),
        attributes,
    })
}

/// Apply a changed declaration to the entity the stack created, through that service's own
/// `UpdateComponent`/`UpdateForm`/`UpdateTheme` — the one call that changes one that exists.
///
/// The call takes the entity's id in its URI and the whole declared configuration in its
/// payload, so a member the declaration no longer states leaves the entity without it rather
/// than keeping the one an earlier deployment gave it. The entity keeps the id that service
/// minted, so the handle the stack publishes is unchanged.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a changed `Tags` map, which no update payload
/// carries — an entity's tags are that service's own tagging calls.
pub(super) async fn update_ui_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: UiEntity,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared.get(TAGS_PROPERTY) != resource.properties.get(TAGS_PROPERTY) {
        return Ok(None);
    }
    let mut payload = entity_payload(entity, declared)?;
    payload.insert(
        NAME_MEMBER.to_string(),
        Value::String(entity_name(declared, resource)),
    );
    let (_, _, update, _) = entity.operations();
    let answered = call_rest_json_named(
        ctx,
        AMPLIFYUIBUILDER_SIGNING_NAME,
        region,
        update,
        "PATCH",
        &entity_uri(entity, resource),
        Some(&Value::Object(payload)),
    )
    .await?;
    Ok(Some(Created::attributes(entity_attributes(
        entity, &answered,
    )?)))
}

/// Delete the entity a stack created, by the id it was created under, in the app environment
/// it belongs to — the three parts of this resource's physical id. One that service no longer
/// holds (`ResourceNotFoundException`, judged on the CODE and never on a message that carries
/// the request's own values) is already in the state the delete asks for.
pub(super) async fn delete_ui_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: UiEntity,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (_, _, _, delete) = entity.operations();
    match call_rest_json_named(
        ctx,
        AMPLIFYUIBUILDER_SIGNING_NAME,
        region,
        delete,
        "DELETE",
        &entity_uri(entity, resource),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == AMPLIFYUIBUILDER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The entity's configuration as Amplify UI Builder reports it, for drift detection: its own
/// read of the entity, by the id that is this resource's physical id
/// ([`UiEntity::reported_properties`]). One that service no longer holds was deleted.
pub(super) async fn describe_ui_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: UiEntity,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    Ok(read_ui_entity(ctx, region, entity, resource)
        .await?
        .map(|reported| reported_as_properties(&reported, entity.reported_properties())))
}

/// The entity Amplify UI Builder holds under this resource's physical id, or `None` when it
/// holds none (`ResourceNotFoundException`, judged on the CODE and never on a message).
async fn read_ui_entity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    entity: UiEntity,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (_, read, _, _) = entity.operations();
    match call_rest_json_named(
        ctx,
        AMPLIFYUIBUILDER_SIGNING_NAME,
        region,
        read,
        "GET",
        &entity_uri(entity, resource),
        None,
    )
    .await
    {
        // Each of those reads binds the entity as its `@httpPayload`, so the answer document
        // IS the entity rather than an envelope carrying one.
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == AMPLIFYUIBUILDER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI the entity's COLLECTION is bound to: the app and the environment it belongs to,
/// each escaped as a single path segment ([`percent_encoded`]) so a value carrying URI
/// structure addresses one app environment rather than another path.
///
/// An empty app or environment — both are required properties of all three types — addresses
/// the empty label, which that operation's own validation refuses rather than this arm
/// inventing a refusal of its own.
fn collection_uri(entity: UiEntity, app_id: &str, environment_name: &str) -> String {
    format!(
        "/app/{}/environment/{}/{}",
        percent_encoded(app_id),
        percent_encoded(environment_name),
        entity.collection(),
    )
}

/// The URI ONE entity is addressed at: its collection plus the `id` `@httpLabel`, escaped the
/// same way — the three parts of this resource's identity ([`entity_identity`]).
fn entity_uri(entity: UiEntity, resource: &StoredResource) -> String {
    let (app_id, environment_name, id) = entity_identity(resource);
    format!(
        "{}/{}",
        collection_uri(entity, &app_id, &environment_name),
        percent_encoded(&id)
    )
}

/// The app, the environment and the id one resource record names: the three parts of the
/// identity the type is addressed by.
///
/// Read off the physical resource id where that is such an identity
/// ([`template::amplifyuibuilder_identity_parts`]) — which it is once the create has answered,
/// and for the stand-in the naming rules computed. A record that carries anything else falls
/// back to what the declaration states for the app environment and to the `Id` the resource
/// publishes, so a record written before this type carried a composite identity is still
/// addressed correctly.
fn entity_identity(resource: &StoredResource) -> (String, String, String) {
    let identity = template::amplifyuibuilder_identity_parts(&resource.physical_resource_id)
        // An identity whose app or id is EMPTY is the stand-in computed for a declaration
        // whose app environment was not resolved yet, which addresses no entity.
        .filter(|(app_id, _, id)| !app_id.is_empty() && !id.is_empty());
    if let Some((app_id, environment_name, id)) = identity {
        return (
            app_id.to_string(),
            environment_name.to_string(),
            id.to_string(),
        );
    }
    (
        declared_scalar(&resource.properties, APP_ID_PROPERTY),
        declared_scalar(&resource.properties, ENVIRONMENT_NAME_PROPERTY),
        resource
            .attributes
            .get(ID_ATTRIBUTE)
            .cloned()
            .unwrap_or_else(|| resource.physical_resource_id.clone()),
    )
}

/// One declared scalar property as the text it states, or the empty string where the
/// declaration states it as nothing or as a composite.
fn declared_scalar(properties: &Value, property: &str) -> String {
    properties
        .get(property)
        .and_then(scalar_text)
        .unwrap_or_default()
}

/// The NAME the entity is created under: the `Name` the declaration states, or — for a
/// declaration that states none, which that type's own contract marks required — the id
/// CloudFormation's naming rules computed for the resource, so the create carries a name that
/// member accepts rather than an empty one.
fn entity_name(declared: &Value, resource: &StoredResource) -> String {
    declared
        .get(NAME_PROPERTY)
        .and_then(scalar_text)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| entity_identity(resource).2)
}

/// One declared property document as the payload Amplify UI Builder takes: every member name
/// re-cased along the payload's own shape ([`recased`]), with the app environment and the tags
/// left out — the URI carries the first, the create adds the second as the map that call takes
/// and no update takes them at all — so one reader serves both.
fn entity_payload(entity: UiEntity, declared: &Value) -> Result<Map<String, Value>, Refusal> {
    let mut payload = match recased(declared, entity.payload(), 0)? {
        Value::Object(members) => members,
        // A declaration that is not an object states no member of the payload.
        _ => Map::new(),
    };
    for member in [
        lower_initial(APP_ID_PROPERTY),
        lower_initial(ENVIRONMENT_NAME_PROPERTY),
        TAGS_MEMBER.to_string(),
    ] {
        payload.remove(&member);
    }
    Ok(payload)
}

/// The attributes one answered entity reports ([`ENTITY_ATTRIBUTES`]).
///
/// An answer carrying no id is reported as a refusal rather than recorded as a created
/// resource: there would be no handle a client could call Amplify UI Builder with, and nothing
/// for the stack to publish.
fn entity_attributes(
    entity: UiEntity,
    answered: &Value,
) -> Result<BTreeMap<String, String>, Refusal> {
    let attributes = reported_attributes(answered, ENTITY_ATTRIBUTES);
    if !attributes.contains_key(ID_ATTRIBUTE) {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: format!(
                "the Amplify UI Builder control plane answered a created {} without an id",
                entity.collection(),
            ),
        });
    }
    Ok(attributes)
}

// ---------------------------------------------------------------------------
// Re-casing a declared document along the payload's own shape
// ---------------------------------------------------------------------------

/// What a document at one position of a payload IS, as far as re-casing its keys goes.
///
/// A CloudFormation property is the API member of the same name with a lower-cased initial, so
/// re-casing a STRUCTURE means re-casing every key of it. A map the CALLER keys is the
/// exception: its keys are the author's own property, field or element names, which travel as
/// written. Which of the two a document is cannot be read off the key alone — `BindingProperties`
/// is a caller-keyed map on a component and a two-member structure on a `ComponentProperty` —
/// so each variant below is one SHAPE of that service's model and [`member_shape`] says what
/// each of its members is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// A structure every key of which is a member name, at this level and below.
    Members,
    /// A map the caller keys whose values are structures of member names (a tag map, a
    /// component's `BindingProperties`, a form's `SectionalElements`).
    KeyedMembers,
    /// A map the caller keys whose values are themselves such a map: a variant's `Overrides`,
    /// which is `map<String, map<String, String>>`.
    KeyedKeyed,
    /// A map the caller keys whose values are `ComponentProperty` documents (a component's
    /// `Properties`, a child's, an action's `Fields`).
    KeyedProperties,
    /// A map the caller keys whose values are `ComponentEvent` documents.
    KeyedEvents,
    /// A map the caller keys whose values are `FieldConfig` documents (a form's `Fields`).
    KeyedFields,
    /// `CreateComponentData`/`UpdateComponentData`.
    ComponentData,
    /// `ComponentProperty`.
    ComponentProperty,
    /// `ComponentConditionProperty`, whose `Then` and `Else` are properties of their own.
    ComponentCondition,
    /// `ComponentChild`.
    ComponentChild,
    /// `ComponentEvent`.
    ComponentEvent,
    /// `ActionParameters`.
    ActionParameters,
    /// `MutationActionSetStateParameter`.
    MutationState,
    /// `ComponentVariant`.
    ComponentVariant,
    /// `CreateFormData`/`UpdateFormData`.
    FormData,
    /// `FieldConfig`.
    FieldConfig,
    /// `FieldInputConfig`.
    FieldInputConfig,
    /// `ValueMappings`.
    ValueMappings,
}

/// The shape of the VALUES of a caller-keyed map, or `None` for a shape whose own keys are
/// member names.
fn keyed_values(shape: Shape) -> Option<Shape> {
    match shape {
        Shape::KeyedMembers => Some(Shape::Members),
        Shape::KeyedKeyed => Some(Shape::KeyedMembers),
        Shape::KeyedProperties => Some(Shape::ComponentProperty),
        Shape::KeyedEvents => Some(Shape::ComponentEvent),
        Shape::KeyedFields => Some(Shape::FieldConfig),
        _ => None,
    }
}

/// The shape of one member of a structure, which for a LIST member is the shape of its
/// elements: every element of a list is read as the member's own shape.
///
/// Source: that service's own model, member by member
/// (`smithy_query(query="shape", shape="CreateComponentData,ComponentProperty,ComponentChild,
/// ComponentEvent,ActionParameters,ComponentVariant,CreateFormData,FieldConfig,
/// FieldInputConfig,ValueMappings")`). A member this says nothing about is a structure of
/// member names all the way down, which is [`Shape::Members`].
fn member_shape(shape: Shape, property: &str) -> Shape {
    match (shape, property) {
        (Shape::ComponentData, "Properties") => Shape::KeyedProperties,
        (Shape::ComponentData, "Overrides") => Shape::KeyedKeyed,
        (Shape::ComponentData, "BindingProperties" | "CollectionProperties") => {
            Shape::KeyedMembers
        }
        (Shape::ComponentData, "Events") => Shape::KeyedEvents,
        (Shape::ComponentData, "Children") => Shape::ComponentChild,
        (Shape::ComponentData, "Variants") => Shape::ComponentVariant,
        (Shape::ComponentProperty, "Bindings") => Shape::KeyedMembers,
        (Shape::ComponentProperty, "Concat") => Shape::ComponentProperty,
        (Shape::ComponentProperty, "Condition") => Shape::ComponentCondition,
        (Shape::ComponentCondition, "Then" | "Else") => Shape::ComponentProperty,
        (Shape::ComponentChild, "Properties") => Shape::KeyedProperties,
        (Shape::ComponentChild, "Events") => Shape::KeyedEvents,
        (Shape::ComponentChild, "Children") => Shape::ComponentChild,
        (Shape::ComponentEvent, "Parameters") => Shape::ActionParameters,
        (
            Shape::ActionParameters,
            "Type" | "Url" | "Anchor" | "Target" | "Global" | "Id",
        ) => Shape::ComponentProperty,
        (Shape::ActionParameters, "Fields") => Shape::KeyedProperties,
        (Shape::ActionParameters, "State") => Shape::MutationState,
        (Shape::MutationState, "Set") => Shape::ComponentProperty,
        (Shape::ComponentVariant, "VariantValues") => Shape::KeyedMembers,
        (Shape::ComponentVariant, "Overrides") => Shape::KeyedKeyed,
        (Shape::FormData, "Fields") => Shape::KeyedFields,
        (Shape::FormData, "SectionalElements") => Shape::KeyedMembers,
        (Shape::FieldConfig, "InputType") => Shape::FieldInputConfig,
        (Shape::FieldInputConfig, "ValueMappings") => Shape::ValueMappings,
        (Shape::ValueMappings, "BindingProperties") => Shape::KeyedMembers,
        _ => Shape::Members,
    }
}

/// The same document with every object key that is a MEMBER NAME re-cased to the member it is
/// ([`lower_initial`]), and every key of a caller-keyed map left as the author wrote it.
///
/// `depth` is how many levels are already above this one, so the outermost call passes `0`; a
/// document nested deeper than [`MAX_PROPERTY_NESTING`] is refused rather than walked, for the
/// reason that constant records — the nesting is the REQUESTER's choice.
fn recased(value: &Value, shape: Shape, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let element = keyed_values(shape);
            let mut converted = Map::new();
            for (key, member) in members {
                match element {
                    // A key of a caller-keyed map is data: it travels as written, and its
                    // value is read as that map's element shape.
                    Some(element) => {
                        converted.insert(key.clone(), recased(member, element, depth + 1)?);
                    }
                    None => {
                        converted.insert(
                            lower_initial(key),
                            recased(member, member_shape(shape, key), depth + 1)?,
                        );
                    }
                }
            }
            Ok(Value::Object(converted))
        }
        // Every element of a list is a document of the member's own shape.
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(recased(item, shape, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a declared property is the API member of the same name with a lower-cased
    /// initial, and a key of a map the CALLER keys is data that travels as written — so the
    /// same key is re-cased in one position and kept in another.
    #[test]
    fn a_declaration_is_recased_along_the_payloads_own_shape() {
        let rows: &[(&str, Shape, Value, Value)] = &[
            (
                "a theme's whole document, every key a member name",
                Shape::Members,
                ::serde_json::json!({
                    "Name": "a-theme",
                    "Values": [{"Key": "colors", "Value": {"Value": "blue"}}]
                }),
                ::serde_json::json!({
                    "name": "a-theme",
                    "values": [{"key": "colors", "value": {"value": "blue"}}]
                }),
            ),
            (
                "a component's caller-keyed maps, whose keys are the author's own names",
                Shape::ComponentData,
                ::serde_json::json!({
                    "Properties": {"Label": {"Value": "Hi", "Type": "String"}},
                    "Overrides": {"Button": {"Label": "Hi"}},
                    "BindingProperties": {"Item": {"Type": "Data", "DefaultValue": "x"}},
                    "Events": {"OnClick": {"Action": "Amplify.Navigation"}}
                }),
                ::serde_json::json!({
                    "properties": {"Label": {"value": "Hi", "type": "String"}},
                    "overrides": {"Button": {"Label": "Hi"}},
                    "bindingProperties": {"Item": {"type": "Data", "defaultValue": "x"}},
                    "events": {"OnClick": {"action": "Amplify.Navigation"}}
                }),
            ),
            (
                "the same key as a structure one level down, which IS re-cased",
                Shape::ComponentData,
                ::serde_json::json!({
                    "Properties": {
                        "Label": {"BindingProperties": {"Property": "item", "Field": "name"}}
                    }
                }),
                ::serde_json::json!({
                    "properties": {
                        "Label": {"bindingProperties": {"property": "item", "field": "name"}}
                    }
                }),
            ),
            (
                "a form's fields and sectional elements",
                Shape::FormData,
                ::serde_json::json!({
                    "Fields": {
                        "email": {
                            "Label": "Email",
                            "InputType": {
                                "Type": "TextField",
                                "ValueMappings": {
                                    "Values": [{"Value": {"Value": "a"}}],
                                    "BindingProperties": {"Model": {"Type": "Data"}}
                                }
                            }
                        }
                    },
                    "SectionalElements": {"Heading": {"Type": "Heading", "Level": 1}}
                }),
                ::serde_json::json!({
                    "fields": {
                        "email": {
                            "label": "Email",
                            "inputType": {
                                "type": "TextField",
                                "valueMappings": {
                                    "values": [{"value": {"value": "a"}}],
                                    "bindingProperties": {"Model": {"type": "Data"}}
                                }
                            }
                        }
                    },
                    "sectionalElements": {"Heading": {"type": "Heading", "level": 1}}
                }),
            ),
            (
                "a child's and a variant's own maps, nested under the lists that carry them",
                Shape::ComponentData,
                ::serde_json::json!({
                    "Children": [{
                        "ComponentType": "Text",
                        "Name": "label",
                        "Properties": {"Label": {"Value": "Hi"}},
                        "Children": [{
                            "ComponentType": "Text",
                            "Name": "inner",
                            "Properties": {"Label": {"Value": "Deep"}}
                        }]
                    }],
                    "Variants": [{
                        "VariantValues": {"Breakpoint": "large"},
                        "Overrides": {"Button": {"Label": "Hi"}}
                    }]
                }),
                ::serde_json::json!({
                    "children": [{
                        "componentType": "Text",
                        "name": "label",
                        "properties": {"Label": {"value": "Hi"}},
                        "children": [{
                            "componentType": "Text",
                            "name": "inner",
                            "properties": {"Label": {"value": "Deep"}}
                        }]
                    }],
                    "variants": [{
                        "variantValues": {"Breakpoint": "large"},
                        "overrides": {"Button": {"Label": "Hi"}}
                    }]
                }),
            ),
        ];

        for (case, shape, declared, expected) in rows {
            let converted = recased(declared, *shape, 0)
                .unwrap_or_else(|refusal| panic!("case {case:?}: refused with {refusal}"));
            assert_eq!(
                &converted, expected,
                "case {case:?}: expected {expected}, got {converted}"
            );
        }
    }

    /// The app environment and the tag map are NOT payload members: the URI carries the first
    /// and the create adds the second itself, so a declaration's own spelling of either is
    /// lifted out.
    #[test]
    fn the_payload_leaves_out_what_the_uri_and_the_tagging_carry() {
        for entity in [UiEntity::Component, UiEntity::Form, UiEntity::Theme] {
            let declared = ::serde_json::json!({
                "AppId": "d1234abcdefghi",
                "EnvironmentName": "staging",
                "Tags": {"owner": "studio"},
                "Name": "an-entity"
            });
            let payload = entity_payload(entity, &declared)
                .unwrap_or_else(|refusal| panic!("case {entity:?}: refused with {refusal}"));
            for member in ["appId", "environmentName", "tags"] {
                assert!(
                    !payload.contains_key(member),
                    "case {entity:?}: expected no {member} in the payload, got \
                     {payload:?}"
                );
            }
            assert_eq!(
                payload.get("name").and_then(Value::as_str),
                Some("an-entity"),
                "case {entity:?}: expected the declared name in the payload, got {payload:?}"
            );
        }
    }

    /// The URIs are the ones each operation's own `@http` binding declares, with every label
    /// escaped as a single path segment — and every label is read off the identity the record
    /// carries, whichever of its two forms that is.
    #[test]
    fn the_uris_are_the_bindings_of_the_entitys_own_operations() {
        const ID: &str = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";
        let declared = ::serde_json::json!({
            "AppId": "d1234abcdefghi",
            "EnvironmentName": "staging/dev",
        });
        // (case, the record's physical id, the attributes it publishes)
        let records: [(&str, String, BTreeMap<String, String>); 2] = [
            (
                "the composite identity the create answered",
                template::amplifyuibuilder_identity("d1234abcdefghi", "staging/dev", ID),
                BTreeMap::new(),
            ),
            (
                "a record carrying the id alone, addressed by its declaration",
                ID.to_string(),
                [("Id".to_string(), ID.to_string())].into_iter().collect(),
            ),
        ];
        let rows = [
            (UiEntity::Component, "components"),
            (UiEntity::Form, "forms"),
            (UiEntity::Theme, "themes"),
        ];
        for (case, physical_resource_id, attributes) in records {
            for (entity, collection) in rows {
                let resource = StoredResource {
                    logical_id: "AnEntity".to_string(),
                    resource_type: "AWS::AmplifyUIBuilder::Theme".to_string(),
                    physical_resource_id: physical_resource_id.clone(),
                    attributes: attributes.clone(),
                    metadata: None,
                    properties: declared.clone(),
                    materialised: true,
                    deletion_policy: None,
                };
                let (app_id, environment_name, id) = entity_identity(&resource);
                let collection_uri = collection_uri(entity, &app_id, &environment_name);
                assert_eq!(
                    collection_uri,
                    format!("/app/d1234abcdefghi/environment/staging%2Fdev/{collection}"),
                    "case {case:?} {entity:?}: expected the collection binding, got \
                     {collection_uri}"
                );
                assert_eq!(
                    id, ID,
                    "case {case:?} {entity:?}: expected the minted id, got {id}"
                );
                let entity_uri = entity_uri(entity, &resource);
                assert_eq!(
                    entity_uri,
                    format!("{collection_uri}/{ID}"),
                    "case {case:?} {entity:?}: expected one entity's binding, got {entity_uri}"
                );
            }
        }
    }
}
