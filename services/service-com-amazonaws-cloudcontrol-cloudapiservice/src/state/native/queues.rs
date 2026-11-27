// SPDX-License-Identifier: CC0-1.0

//! `AWS::SQS::Queue`, as it lives in the Amazon SQS service's own store.
//!
//! A queue is a resource of the Amazon SQS service: `sqs:CreateQueue` creates it,
//! `sqs:GetQueueAttributes` and `sqs:ListQueues` report it, `sqs:SetQueueAttributes`
//! changes it and `sqs:DeleteQueue` removes it, and the record of it lives in that
//! service's state in the same shared engine. So this provider holds a handle to THAT
//! store and nothing of its own: a queue this emulator holds is one Cloud Control API
//! resolves, whichever API created it, and a queue Amazon SQS no longer holds is one Cloud
//! Control API reports as not found.
//!
//! Queues are held one namespace per Region ("One namespace per region" in that service's
//! own state module), so every answer here is about the Region the call was made in: a
//! reference resolves to that Region's queue of the name it carries, exactly as the owning
//! service resolves a `QueueUrl` a request supplies ([`queue_name`]).
//!
//! The resource model is the type's registry schema. Catalog source: the `AWS::SQS::Queue`
//! resource provider schema's `properties`, whose `primaryIdentifier` is
//! `/properties/QueueUrl`, whose `additionalIdentifiers` are `/properties/Arn` and
//! `/properties/QueueName` (the three references [`queue_name`] resolves), and whose
//! `readOnlyProperties` are `/properties/Arn` and `/properties/QueueUrl`:
//!
//! * `QueueName`, the queue's name, which its url and its ARN are built from;
//! * every property that IS one of the queue's own attributes, named identically on both
//!   sides (`VisibilityTimeout`, `DelaySeconds`, `MessageRetentionPeriod`, `FifoQueue`,
//!   `RedrivePolicy`, …) — which is every attribute Amazon SQS publishes as settable
//!   except the queue's `Policy`, because this type's schema declares no such property: a
//!   queue's access policy is carried by resource types of its own
//!   (`AWS::SQS::QueuePolicy`, `AWS::SQS::QueueInlinePolicy`);
//! * `Tags`, the queue's cost allocation tags, as a list of `{ Key, Value }` pairs;
//! * the read-only `Arn` and `QueueUrl`.
//!
//! A property is carried in the form the schema declares — a count as a number, a flag as
//! a boolean, a redrive document as an object — while an attribute VALUE is text on the
//! owning service's wire, so the two are converted by that service's own spelling of the
//! rule ([`property_value`] one way, [`sqs::attribute_text`] the other) and every rule about
//! what an attribute accepts stays that service's own ([`DesiredModel::attributes`]).
//!
//! An attribute of the queue this type's schema does NOT declare is none of this type's
//! business either way: a read does not report it, and a write of this type leaves it
//! exactly as the owning store holds it.

use ::std::collections::{BTreeMap, HashMap};
use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_sqs_amazonsqs::state as sqs;

use super::{require_value, Announced, NativeRefusal, NativeResourceProvider, SqsState};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::SQS::Queue";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/QueueUrl`): the queue's url, which Amazon
/// SQS assigns rather than the caller naming it.
const IDENTIFIER_PROPERTY: &str = "QueueUrl";

/// The type's other read-only property: the queue's ARN
/// (`readOnlyProperties: ["/properties/Arn", "/properties/QueueUrl"]`), which the owning
/// service reports as the `QueueArn` attribute.
const ARN: &str = "Arn";

/// The queue's name (`QueueName`), which the schema declares create-only and which its url
/// and ARN are built from.
const QUEUE_NAME: &str = "QueueName";

/// The queue's tag set (`Tags`), a list of `{ Key, Value }` pairs — read and rendered by
/// the shared reading of a tag set every provider uses.
const TAGS: &str = "Tags";

/// The Amazon SQS client this type's handler reaches the owning service through, named as
/// that client's rendering of an exception names the service.
///
/// This handler reports an error the owning service raised which it maps to no verdict of
/// its own as the exception its CLIENT raised, so that service's own diagnostic of the
/// refused call travels out with that client's rendering of the call around it
/// ([`NativeResourceProvider::relayed_client`]) — a caller of a failed request reads why
/// Amazon SQS would not make the queue, in Amazon SQS's own words.
const SDK_CLIENT: &str = "Sqs";

/// The Amazon SQS queues of one engine, as Cloud Control API resources.
pub struct QueueProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<SqsState>,
    /// The `host[:port]` the queue urls this provider reports name, read off what the
    /// owning service announces to this caller ([`Announced::sqs_endpoint`]); `None` for a
    /// reader with no request behind it, whose urls take that service's published form.
    announced: Option<String>,
}

impl QueueProvider {
    /// The provider for the Amazon SQS store THIS engine carries, or `None` when it
    /// carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(
        engine: &Eng,
        announced: &Announced,
    ) -> Option<Self> {
        sqs::service_state(engine).map(|state| Self {
            state,
            announced: announced.sqs_endpoint().map(str::to_string),
        })
    }

    /// The url THIS provider reports for the queue called `name` in `region`: the queue's
    /// `/<accountId>/<QueueName>` path on the endpoint the owning service announces to this
    /// caller, and that service's published form for a reader with no request behind it.
    ///
    /// The ONE place this provider composes a queue url, so the identifier it tracks, the
    /// identifier it lists and the `QueueUrl` property of the model it reports cannot name
    /// three different addresses for one queue.
    fn url(&self, region: &str, name: &str) -> String {
        match &self.announced {
            Some(authority) => {
                sqs::queue_url_at(authority, ::engine::emulator_account_id(), name)
            }
            None => sqs::queue_url(region, name),
        }
    }

    /// How a refusal states what the type's primary identifier has to be, for a create
    /// whose identifier names no queue: in the form of a url THIS reader reports, so the
    /// example a caller is shown is an address it can really use.
    fn queue_url_requirement(&self) -> String {
        format!(
            "must be the url of a queue of this account in the form {}, or the queue's ARN \
             or name",
            self.url("<region>", "<QueueName>")
        )
    }
}

impl NativeResourceProvider for QueueProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    /// The owning service's calls this provider's handler makes: `sqs:CreateQueue`, which
    /// creates a queue, `sqs:SetQueueAttributes`, which changes one, and `sqs:DeleteQueue`,
    /// which removes one.
    fn handler_calls(&self, operation: &str) -> &'static [&'static str] {
        // The create.
        const CREATE: &[&str] = &["sqs:CreateQueue"];
        // The change.
        const CHANGE: &[&str] = &["sqs:SetQueueAttributes"];
        // The removal.
        const REMOVE: &[&str] = &["sqs:DeleteQueue"];
        match operation {
            crate::state::OPERATION_CREATE => CREATE,
            crate::state::OPERATION_UPDATE => CHANGE,
            crate::state::OPERATION_DELETE => REMOVE,
            _ => &[],
        }
    }

    /// The exception this type's handler relays is its Amazon SQS client's ([`SDK_CLIENT`]).
    fn relayed_client(&self) -> Option<&'static str> {
        Some(SDK_CLIENT)
    }

    /// The queue's own ARN, as the OWNING service builds it from the Region and the name the
    /// reference this type is identified by resolves to ([`queue_name`]).
    fn resource_arn(&self, region: &str, identifier: &str) -> Option<String> {
        Some(sqs::queue_arn(region, &queue_name(region, identifier)?))
    }

    /// The url the queue a create describes will answer to.
    ///
    /// The type's primary identifier is read-only in its schema, so no desired state
    /// carries it: the queue's url is the owning service's to assign, and this is the url
    /// that service reports for it to this caller ([`QueueProvider::url`]). A model that
    /// names no name is a resource whose name the provider picks ([`generated_name`]), the
    /// same way a template that declares none leaves the name to CloudFormation.
    fn assigned_identifier(&self, region: &str, properties: &Value) -> Option<String> {
        let name = properties
            .get(QUEUE_NAME)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| generated_name(properties));
        Some(self.url(region, &name))
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        let name = queue_name(region, identifier)?;
        let url = self.url(region, &name);
        self.state
            .lock()
            .queue(region, &name)
            .map(|queue| model(queue, &url))
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        // The owning store holds one Region's queues by name, in name order, and every url
        // of one Region shares the same prefix — so that order IS the url order a listing
        // pages over, and the window below is the page it is about to report.
        let store = self.state.lock();
        store
            .queues_in(region)
            .map(|queue| {
                let url = self.url(region, &queue.name);
                (url.clone(), model(queue, &url))
            })
            .filter(|(url, _)| after.is_none_or(|after| url.as_str() > after))
            .take(limit)
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let Some(name) = queue_name(region, identifier) else {
            return Err(NativeRefusal::Invalid(format!(
                "{IDENTIFIER_PROPERTY} {}",
                self.queue_url_requirement()
            )));
        };
        // The name a queue is created under is the OWNING service's to accept, so nothing
        // is created here under a name that service's own API would refuse.
        let arn = sqs::queue_arn(region, &name);
        // The url the queue will answer to, whichever way the reference that reached this
        // call spelled it: the one the owning service reports for it to this caller.
        let url = self.url(region, &name);
        let desired = DesiredModel::read(&name, &arn, &url, properties)?;
        let fifo = desired.designates_fifo();
        if let Some(why) = sqs::queue_name_refusal(&name, fifo) {
            return Err(NativeRefusal::Rejected(why));
        }
        if let Some(why) = sqs::tag_refusal(&name, &BTreeMap::new(), &desired.tag_map()) {
            return Err(NativeRefusal::Rejected(why));
        }
        let attributes = desired.attributes(fifo)?;
        redrive_documents_accepted(region, &desired)?;

        let mut store = self.state.lock();
        // The queue is created in the owning service's store under its own rules: a name
        // it already holds is that service's own already-exists verdict, a name it deleted
        // too recently is not available again yet, and its quota bounds what this create
        // may add.
        if store.holds_queue(region, &name) {
            return Err(NativeRefusal::AlreadyExists);
        }
        store.forget_lapsed_deletions(at);
        if store.deleted_recently(region, &name, at).is_some() {
            return Err(NativeRefusal::Rejected(
                sqs::queue_deleted_recently_message(),
            ));
        }
        if store.queues.len() >= sqs::MAX_QUEUES {
            return Err(NativeRefusal::AtCapacity(sqs::queues_quota_message(&name)));
        }
        if let Some(why) = redrive_refusal(&store, region, &name, fifo, &desired) {
            return Err(NativeRefusal::Rejected(why));
        }
        let queue = sqs::Queue::created(region, &name, attributes, desired.tags, at);
        let created = model(&queue, &url);
        store.insert_queue(queue);
        Ok(created)
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let name = queue_name(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        let held = store.queue(region, &name).ok_or(NativeRefusal::NotFound)?;
        let (arn, url, was_fifo) = (held.arn.clone(), self.url(region, &name), held.is_fifo());
        let (held_attributes, held_tags) = (held.attributes.clone(), held.tags.clone());
        let desired = DesiredModel::read(&name, &arn, &url, properties)?;
        // "You can provide this attribute only during queue creation. You can't change it
        // for an existing queue": the queue's type is settled, so a model that designates
        // the other one asks the owning service for a change it does not make.
        if desired.designates_fifo() != was_fifo {
            return Err(NativeRefusal::Rejected(
                sqs::QUEUE_TYPE_NOT_MODIFIABLE.to_string(),
            ));
        }
        if let Some(why) = sqs::tag_refusal(&name, &held_tags, &desired.tag_map()) {
            return Err(NativeRefusal::Rejected(why));
        }
        // A FIFO queue's own attributes stay applicable, and the model carries the queue's
        // settled type — held to the queue's own above — so the map is judged exactly as a
        // create's is.
        let mut attributes = desired.attributes(was_fifo)?;
        // An attribute of the queue this type's schema does NOT declare is none of this
        // type's business: a queue's access policy belongs to `AWS::SQS::QueuePolicy` /
        // `AWS::SQS::QueueInlinePolicy` (see this module's header), so a write of THIS type
        // leaves it exactly as the owning store holds it rather than resetting it with a
        // model that could never have carried it.
        for (attribute, value) in &held_attributes {
            if attribute_property(attribute).is_none() {
                attributes.insert(attribute.clone(), value.clone());
            }
        }
        redrive_documents_accepted(region, &desired)?;
        if let Some(why) = redrive_refusal(&store, region, &name, was_fifo, &desired) {
            return Err(NativeRefusal::Rejected(why));
        }
        let queue = store
            .queue_mut(region, &name)
            .ok_or(NativeRefusal::NotFound)?;
        let delay_changed =
            queue.attributes.get(sqs::DELAY_SECONDS) != attributes.get(sqs::DELAY_SECONDS);
        queue.attributes = attributes;
        queue.tags = desired.tags;
        queue.last_modified_at = at;
        // What the new values make the clock owe, applied at once rather than left for the
        // next reader to discover, exactly as the owning service's own attribute write
        // applies it: a `DelaySeconds` change is retroactive on a FIFO queue, and a
        // shortened `MessageRetentionPeriod` expires every message already older than it.
        if delay_changed {
            queue.reapply_delay_seconds();
        }
        queue.materialise_due(at);
        Ok(model(queue, &url))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let name = queue_name(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        if !store.holds_queue(region, &name) {
            return Err(NativeRefusal::NotFound);
        }
        // Removed through the owning store's own removal, so the queue goes with every
        // message and handle it held and its NAME stays unavailable for as long as that
        // service keeps a deleted name deferred. Addressed by the url that service assigned
        // the queue, which is what it resolves a removal by.
        store
            .remove_queue_by_url(region, &sqs::queue_url(region, &name), at)
            .map(|_| ())
            .ok_or(NativeRefusal::NotFound)
    }
}

/// Check the redrive documents a model carries against the owning service's own rules for
/// the DOCUMENT — the ones that need no store, so they are judged before one is locked.
fn redrive_documents_accepted(region: &str, desired: &DesiredModel) -> Result<(), NativeRefusal> {
    // A dead-letter link never crosses an account or a Region boundary, so no queue
    // carries a `RedrivePolicy` whose target is some other namespace's queue.
    if let Some(why) = desired
        .supplied
        .get(sqs::REDRIVE_POLICY)
        .and_then(|value| sqs::redrive_policy_refusal(region, value))
    {
        return Err(NativeRefusal::Rejected(why));
    }
    // The same boundary from the other end of the link, and the rest of the permission
    // document's own shape.
    if let Some(refusal) = desired
        .supplied
        .get(sqs::REDRIVE_ALLOW_POLICY)
        .and_then(|value| sqs::redrive_allow_policy_document_refusal(region, value))
    {
        return Err(NativeRefusal::Rejected(match refusal {
            sqs::RedriveAllowPolicyRefusal::Document(why) => why,
            sqs::RedriveAllowPolicyRefusal::Permission(error) => detail(error.message),
        }));
    }
    Ok(())
}

/// Check the redrive association a model asks for against the queues the owning store
/// HOLDS: the dead-letter target has to be there and be of this queue's own type, and that
/// target's own permission has to admit this queue as a source.
fn redrive_refusal(
    store: &SqsState,
    region: &str,
    name: &str,
    fifo: bool,
    desired: &DesiredModel,
) -> Option<String> {
    let policy = desired.supplied.get(sqs::REDRIVE_POLICY)?;
    store
        .dead_letter_target_refusal(region, fifo, policy)
        .or_else(|| {
            store.redrive_allow_policy_refusal(region, &sqs::queue_arn(region, name), policy)
        })
}

/// The name of the queue `identifier` names in `region`, or `None` for a value that names
/// no queue of this type at all.
///
/// The references a resource of this type answers to are the ones the type's schema
/// DECLARES: its `primaryIdentifier` `/properties/QueueUrl` — a url of this account, which
/// names its queue by the `/<accountId>/<QueueName>` path it carries and not by its
/// authority, exactly as the owning service resolves one ([`sqs::queue_name_from_url`]) —
/// and its `additionalIdentifiers`, `/properties/Arn` and `/properties/QueueName`. Each is
/// resolved to the one queue it names, and whichever of them addressed it, every answer
/// reports the url the owning service announces for that queue.
///
/// Resolving a url by its path rather than by the whole string is what lets a client hand
/// back a handle it was given at one endpoint of this emulator through another: the two
/// strings name one queue.
///
/// A value that is none of the three — a url or an ARN of another account, a url whose path
/// is empty, a name no queue carries — names no queue, which is what a read of it reports as
/// not found.
fn queue_name(region: &str, identifier: &str) -> Option<String> {
    // The url: resolved by the `/<accountId>/<QueueName>` pair the owning service resolves
    // one by, so only a value carrying a path reaches it — an ARN carries none.
    if identifier.contains('/') {
        return sqs::queue_name_from_url(identifier).map(str::to_string);
    }
    // The ARN, which the owning service builds from the same Region, account and name.
    if let Some(name) = identifier
        .rsplit_once(':')
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty() && sqs::queue_arn(region, name) == identifier)
    {
        return Some(name.to_string());
    }
    // And the name itself, which carries none of the separators either of the two above
    // is built with.
    (!identifier.is_empty() && !identifier.contains([':', '/'])).then(|| identifier.to_string())
}

/// The name a create picks for a queue whose model names none — the shared rule, carrying
/// the `.fifo` suffix a FIFO queue's name must end with.
fn generated_name(properties: &Value) -> String {
    super::generated_name(
        designates_fifo(properties.get(sqs::FIFO_QUEUE_ATTRIBUTE)),
        sqs::FIFO_QUEUE_NAME_SUFFIX,
    )
}

/// Whether a model's `FifoQueue` value designates a FIFO queue.
///
/// "If you don't specify the FifoQueue attribute, Amazon SQS creates a standard queue", and
/// the schema declares the property a boolean, so a model carrying `true` for it asks for a
/// FIFO queue and anything else — including no such property — asks for a standard one.
fn designates_fifo(value: Option<&Value>) -> bool {
    value.is_some_and(|value| sqs::attribute_text(value) == sqs::DESIGNATES_FIFO_QUEUE)
}

/// The spec of the attribute a property of this type IS, or `None` for a property that is
/// not one of the queue's attributes.
///
/// The properties of this type are the queue's own attributes under their own names, which
/// is every attribute the owning service publishes as settable EXCEPT the queue's `Policy`:
/// this type's schema declares no such property, because a queue's access policy is carried
/// by resource types of its own.
fn attribute_property(property: &str) -> Option<&'static sqs::AttributeSpec> {
    if property == sqs::POLICY {
        return None;
    }
    sqs::settable_attribute(property)
}

/// The resource model one queue renders as — the schema's properties the owning record
/// carries, and only those it HAS: a queue carries the attributes its create supplied and
/// the documented defaults of those that apply to it, so that is what a read reports, in
/// each property's own declared form.
///
/// `url` is the queue's `QueueUrl` property, which is its primary identifier, so it is
/// passed in from the ONE place this provider composes one ([`QueueProvider::url`]) rather
/// than built again here.
fn model(queue: &sqs::Queue, url: &str) -> Value {
    let mut model = Map::new();
    model.insert(IDENTIFIER_PROPERTY.to_string(), Value::from(url));
    model.insert(ARN.to_string(), Value::from(queue.arn.clone()));
    model.insert(QUEUE_NAME.to_string(), Value::from(queue.name.clone()));
    // The encryption a queue is under is reported by the owning service whether or not its
    // create named an attribute for it, so a read through this type reports the same.
    let mut carried = queue.attributes.clone();
    sqs::encryption::report(&queue.attributes, &mut carried);
    for (attribute, value) in &carried {
        if let Some(spec) = attribute_property(attribute) {
            model.insert(attribute.clone(), property_value(spec, value));
        }
    }
    if !queue.tags.is_empty() {
        model.insert(TAGS.to_string(), super::tag_list(queue.tags.iter()));
    }
    Value::Object(model)
}

/// One attribute value as the PROPERTY that carries it is declared: a count as a number, a
/// flag as a boolean, a redrive document as the object it is, and anything else as the text
/// the owning service holds.
///
/// Every value in that store was accepted by the owning service's own check of what the
/// attribute takes ([`sqs::validate_attributes`]), so each conversion is of a value already
/// known to be of that form; one that somehow is not is reported as the text it is rather
/// than dropped from the model.
fn property_value(spec: &sqs::AttributeSpec, value: &str) -> Value {
    let text = || Value::from(value.to_string());
    match spec.kind {
        sqs::AttributeKind::Integer { .. } => {
            value.parse::<i64>().map_or_else(|_| text(), Value::from)
        }
        sqs::AttributeKind::Boolean | sqs::AttributeKind::Designator(_) => {
            value.parse::<bool>().map_or_else(|_| text(), Value::from)
        }
        sqs::AttributeKind::Json => ::serde_json::from_str(value).unwrap_or_else(|_| text()),
        sqs::AttributeKind::OneOf(_) | sqs::AttributeKind::Text => text(),
    }
}

/// What a resource model asks a queue to be: the attribute map the owning service's own
/// calls take, and the tag set the queue is to carry, each read off the model's properties.
struct DesiredModel {
    /// The queue's attributes, as the owning service takes them: one entry per attribute
    /// property the model carried, holding that value's text.
    supplied: HashMap<String, String>,
    /// The tag set the queue is to carry.
    tags: BTreeMap<String, String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the queue called `name`, whose ARN is `arn` and
    /// whose url is `url` — or state why the owning service would not take it.
    ///
    /// The identity and both read-only properties may be CARRIED (a read reports them, so a
    /// model a client round-trips carries them) but only with the value the queue already
    /// answers to: a resource never answers to an identifier other than the one it is
    /// stored under, and nothing here renames a queue.
    fn read(name: &str, arn: &str, url: &str, properties: &Value) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            supplied: HashMap::new(),
            tags: BTreeMap::new(),
        };
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, url)?,
                ARN => require_value(property, value, arn)?,
                QUEUE_NAME => require_value(property, value, name)?,
                // The shared reading of a tag set; its entries are then judged by the
                // owning service's own tagging rules, where the queue they are written on
                // is known ([`sqs::tag_refusal`]).
                TAGS => desired.tags = super::tag_set(value)?,
                other => match attribute_property(other) {
                    Some(_) => {
                        desired
                            .supplied
                            .insert(other.to_string(), sqs::attribute_text(value));
                    }
                    None => {
                        return Err(NativeRefusal::Invalid(format!(
                            "{other} is not a property of {TYPE_NAME}"
                        )));
                    }
                },
            }
        }
        Ok(desired)
    }

    /// Whether this model asks for a FIFO queue, which is what makes the FIFO-only
    /// attributes applicable to it.
    fn designates_fifo(&self) -> bool {
        self.supplied
            .get(sqs::FIFO_QUEUE_ATTRIBUTE)
            .is_some_and(|value| value == sqs::DESIGNATES_FIFO_QUEUE)
    }

    /// The tag set this model asks for, as the owning service's own tagging rules take one.
    fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The attribute set this model would leave the queue carrying, having been held to
    /// every rule the OWNING service holds an attribute map to — or why that service would
    /// not take it.
    ///
    /// A resource model is the WHOLE desired state of the resource, so the set is built the
    /// way a create builds one: the documented default of every attribute that applies to a
    /// queue of this type, overlaid with what the model carries. An attribute the model
    /// does not carry is therefore the default rather than whatever the queue held before,
    /// which is what makes a model a client round-trips describe the queue it reads back.
    ///
    /// The map is judged as a CREATE's is, because a model always states the queue's type:
    /// `FifoQueue` is the one attribute only a create may supply, and a model that carries
    /// it carries the type the queue already is (which [`NativeResourceProvider::update`]
    /// holds it to) rather than asking for a change.
    fn attributes(&self, fifo: bool) -> Result<BTreeMap<String, String>, NativeRefusal> {
        sqs::validate_attributes(&self.supplied, true, fifo).map_err(refusal)?;
        let mut attributes: BTreeMap<String, String> = sqs::SETTABLE_ATTRIBUTES
            .iter()
            .filter(|spec| fifo || !spec.fifo_only)
            .filter_map(|spec| {
                spec.default
                    .map(|value| (spec.name.to_string(), value.to_string()))
            })
            .collect();
        sqs::apply_attributes(&mut attributes, self.supplied.iter());
        // The two server-side encryption options are exclusive, and a model naming one
        // takes the queue off the other: a model naming BOTH asks for something no queue
        // can be.
        if let Some(error) = sqs::encryption::encryption_choice_refusal(&self.supplied) {
            return Err(NativeRefusal::Rejected(detail(error.message)));
        }
        sqs::encryption::apply_encryption_choice(&mut attributes, &self.supplied);
        // Two attributes can each be valid and still not hold together, so the pair is
        // judged over the set the queue would CARRY.
        if let Some(error) = sqs::attribute_combination_refusal(&attributes) {
            return Err(NativeRefusal::Rejected(detail(error.message)));
        }
        Ok(attributes)
    }
}

/// The refusal of an attribute map, as this path reports it — under the verdict that
/// follows from WHICH half of the attribute rules the map broke.
///
/// * A VALUE outside what the attribute accepts is a fault in the DESIRED STATE: this
///   type's schema publishes the values each of its attribute properties admits — the range
///   of a count, the two values of a flag, the set of a choice — in that property's own
///   documentation, which is the sentence this provider's catalogue of attributes carries
///   beside each one ([`sqs::SETTABLE_ATTRIBUTES`]). A value outside it describes no queue
///   of the type at all, decided from that one value with nothing of the store in it, so it
///   is the handler's own judgement of the model ([`NativeRefusal::Invalid`], reported as
///   `InvalidRequest`), stated in the sentence that names the property at fault.
/// * A NAME the call does not have for the queue it addresses is not about a value: whether
///   a queue HAS the attribute follows from the kind of queue it is, which is the owning
///   service's own answer about that queue ([`NativeRefusal::Rejected`]).
fn refusal(refused: sqs::AttributeRefusal) -> NativeRefusal {
    match refused {
        sqs::AttributeRefusal::Name(error) => NativeRefusal::Rejected(detail(error.message)),
        sqs::AttributeRefusal::Value(error) => NativeRefusal::Invalid(detail(error.message)),
    }
}

/// The sentence a modelled refusal of the owning service carries. Every one of them states
/// a message; a shape that somehow carries none states the type instead of an empty
/// diagnostic.
fn detail(message: Option<String>) -> String {
    message.unwrap_or_else(|| format!("{TYPE_NAME} was refused by Amazon SQS"))
}

