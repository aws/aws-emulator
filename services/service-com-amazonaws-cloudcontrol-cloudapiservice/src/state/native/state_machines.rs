// SPDX-License-Identifier: CC0-1.0

//! `AWS::StepFunctions::StateMachine`, as it lives in the Step Functions service's own
//! store.
//!
//! A state machine is a resource of the Step Functions service: `states:CreateStateMachine`
//! creates it, `states:DescribeStateMachine` and `states:ListStateMachines` report it,
//! `states:UpdateStateMachine` changes it and `states:DeleteStateMachine` removes it, and
//! the record of it lives in that service's state in the same shared engine. So this
//! provider holds a handle to THAT store and nothing of its own: a state machine this
//! emulator holds is one Cloud Control API resolves whichever API created it, one Step
//! Functions no longer holds is one Cloud Control API reports as not found, and one a
//! `CreateResource` provisioned is one `states:StartExecution` can be called on — which is
//! what makes the identifier Cloud Control hands back a usable handle rather than a name.
//!
//! State machines are regional: a state machine lives in the Region its own ARN carries
//! ([`sfn::state_machine_arn`] is a total function of the Region and the name), so every
//! answer here is about the Region the call was made in.
//!
//! The resource model is the type's registry schema. Catalog source: the
//! `AWS::StepFunctions::StateMachine` resource provider schema's `properties`, whose
//! `primaryIdentifier` is `/properties/Arn` — the ARN Step Functions assigns, which is
//! therefore also one of its `readOnlyProperties`. The properties this path APPLIES are the
//! identity and the configuration the owning record carries:
//!
//! * `StateMachineName`, the name the create chooses and the ARN is built from, which the
//!   schema declares create-only; a model naming none is a resource whose name the provider
//!   picks, the same way a template that declares none leaves the name to CloudFormation;
//! * `Definition` and `DefinitionString`, the two forms the schema carries the Amazon States
//!   Language definition in — `Definition` as the JSON object a template writes inline
//!   (write-only, so a read reports the definition as `DefinitionString`), and a model
//!   carrying both asks for the one it WROTE;
//! * `RoleArn`, the IAM role the state machine runs as;
//! * `StateMachineType`, create-only, which the owning service settles when the state
//!   machine is created;
//! * `Tags`, as a list of `{ Key, Value }` pairs, written through the owning service's own
//!   tag writes;
//! * the read-only `Arn`, `Name` and `StateMachineRevisionId`, which a read reports beside
//!   them.
//!
//! Every rule about what a name, a definition, a role ARN or a workflow type is stays the
//! OWNING service's own ([`sfn::is_valid_name`], [`sfn::asl::validate`],
//! [`sfn::invalid_role_arn_reason`]), so nothing is created here that service's own API
//! would refuse.
//!
//! The schema's remaining properties are ones THIS path cannot apply
//! ([`UNAPPLIED_PROPERTIES`]), so a model that asks for one is refused rather than accepted
//! and dropped — a property a caller sets has to be one a later read reports:
//!
//! * `DefinitionS3Location`, because the definition would have to be fetched from an object
//!   this handler does not read;
//! * `DefinitionSubstitutions`, because substituting into the definition is work this
//!   handler does not perform;
//! * `LoggingConfiguration`, `TracingConfiguration` and `EncryptionConfiguration`, each of
//!   which the owning service accepts in its own shape and under rules of its own that
//!   nothing here applies.

use ::std::collections::BTreeMap;
use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_sfn_awsstepfunctions::ops::types::{Tag, STATE_MACHINE_TYPE_VALUES};
use ::service_com_amazonaws_sfn_awsstepfunctions::state as sfn;

use super::{
    generated_name, require_held, require_value, tag_entries, tag_list, text, unsupported,
    NativeRefusal, NativeResourceProvider, SfnState, TAGS,
};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::StepFunctions::StateMachine";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/Arn`): the state machine's ARN, which Step
/// Functions assigns rather than the caller naming it.
const IDENTIFIER_PROPERTY: &str = "Arn";

/// The state machine's name as the schema reports it read-only (`Name`).
const NAME: &str = "Name";

/// The name the create chooses (`StateMachineName`), which the schema declares create-only
/// and which the ARN is built from.
const STATE_MACHINE_NAME: &str = "StateMachineName";

/// The Amazon States Language definition as the JSON object a template writes inline
/// (`Definition`), which the schema declares write-only.
const DEFINITION: &str = "Definition";

/// The Amazon States Language definition as text (`DefinitionString`), which is the form a
/// read reports it in.
const DEFINITION_STRING: &str = "DefinitionString";

/// The IAM role the state machine runs as (`RoleArn`).
const ROLE_ARN: &str = "RoleArn";

/// The workflow type (`StateMachineType`), which the schema declares create-only.
const STATE_MACHINE_TYPE: &str = "StateMachineType";

/// The revision the state machine is at (`StateMachineRevisionId`), which the schema
/// declares read-only: the owning service mints it on every update.
const STATE_MACHINE_REVISION_ID: &str = "StateMachineRevisionId";

/// The most characters a state machine's name may carry (`name`, modelled length 1..80).
const MAX_NAME_CHARACTERS: usize = 80;

/// The properties of the type's schema THIS path does not apply — see this module's header.
///
/// Named here so a model asking for one is told it is a property of the type this handler
/// does not apply, rather than one the type does not declare at all: the two are different
/// faults and a caller fixes them differently.
const UNAPPLIED_PROPERTIES: &[&str] = &[
    "DefinitionS3Location",
    "DefinitionSubstitutions",
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "TracingConfiguration",
];

/// The Step Functions state machines of one engine, as Cloud Control API resources.
pub struct StateMachineProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<SfnState>,
}

impl StateMachineProvider {
    /// The provider for the Step Functions store THIS engine carries, or `None` when it
    /// carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        sfn::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for StateMachineProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    /// The ARN the state machine a create describes will answer to.
    ///
    /// The type's primary identifier is read-only in its schema, so no desired state carries
    /// it: the ARN is the owning service's to assign, and it assigns the one built from the
    /// partition, this Region, this account and the state machine's name. A model that names
    /// no name is a resource whose name the provider picks ([`generated_name`]), the same way
    /// a template that declares none leaves the name to CloudFormation.
    fn assigned_identifier(&self, region: &str, properties: &Value) -> Option<String> {
        let name = properties
            .get(STATE_MACHINE_NAME)
            .and_then(Value::as_str)
            .map(str::to_string)
            // A state machine name carries no suffix any kind of it requires, so the
            // generated one is the bare minted token.
            .unwrap_or_else(|| generated_name(false, ""));
        Some(sfn::state_machine_arn(region, &name))
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        let arn = own_arn(region, identifier)?;
        let mut store = self.state.lock();
        // A state machine whose deletion the clock has already made due is gone, whether or
        // not anybody called the owning service: materialising that is what keeps this read
        // and that service's own reads from disagreeing about what exists.
        store.settle(SystemTime::now());
        let machine = store.state_machines.get(&arn)?;
        Some(model(machine))
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let mut store = self.state.lock();
        store.settle(SystemTime::now());
        // The owning service's own reader of one Region's state machines answers in ARN
        // order, which is the order a listing pages over, so the window is that order
        // resumed after `after` and nothing is sorted here.
        store
            .state_machines_in_region(region)
            .into_iter()
            .filter(|machine| after.is_none_or(|after| machine.arn.as_str() > after))
            .take(limit)
            .map(|machine| (machine.arn.clone(), model(machine)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let arn = own_arn(region, identifier).ok_or_else(arn_requirement)?;
        let name = name_of(region, &arn).ok_or_else(arn_requirement)?;
        // The name a state machine is created under is the OWNING service's to accept: the
        // character rule it reads, and the length its `name` member is modelled with
        // (1..80), which no model-level walk reaches on this path.
        if !(1..=MAX_NAME_CHARACTERS).contains(&name.chars().count()) || !sfn::is_valid_name(&name)
        {
            return Err(NativeRefusal::Invalid(sfn::resource_error_message(
                sfn::INVALID_NAME,
                &name,
            )));
        }
        let desired = DesiredModel::read(
            properties,
            &Identity {
                arn: &arn,
                name: &name,
                revision_id: None,
            },
        )?;
        let definition = desired.definition()?;
        let role_arn = desired.role_arn()?;
        // "Determines whether a Standard or Express state machine is created. The default is
        // STANDARD" — the owning operation's own rule for an absent workflow type.
        let sm_type = desired
            .sm_type
            .clone()
            .unwrap_or_else(|| sfn::STATE_MACHINE_TYPE_STANDARD.to_string());

        let mut store = self.state.lock();
        store.settle(at);
        // An ARN the owning store already holds is that service's own already-exists
        // verdict: the ARN IS the name index, so a name in use in this Region is exactly a
        // key the store carries.
        if store.state_machines.contains_key(&arn) {
            return Err(NativeRefusal::AlreadyExists);
        }
        let candidate = sfn::StateMachine {
            arn: arn.clone(),
            name,
            definition,
            role_arn,
            sm_type,
            // `StateMachineStatus` is ACTIVE | DELETING: the owning service reports a newly
            // created state machine ready, so it is stored ready.
            status: sfn::STATE_MACHINE_STATUS_ACTIVE.to_string(),
            creation_date: at,
            update_date: at,
            // Configurations nothing here applies (see this module's header).
            logging_configuration: None,
            tracing_configuration: None,
            encryption_configuration: None,
            // A state machine nothing has updated is at no revision yet.
            revision_id: None,
            description: None,
            tags: desired.tags.clone(),
            // A version is published by a call of its own, which this create does not make.
            publish: false,
            published_version_arn: None,
            version_counter: 0,
            delete_due_at: None,
            // Measured and counted by `insert_state_machine`.
            retained_bytes: 0,
        };
        // The owning store's caps are on the WHOLE store, in records AND in bytes, and the
        // definition is caller-sized: a record this create cannot be admitted for is that
        // service's own quota refusal.
        if let Some(why) = store.state_machine_rejection(candidate.measure()) {
            return Err(NativeRefusal::AtCapacity(why));
        }
        store.insert_state_machine(candidate);
        let created = store
            .state_machines
            .get(&arn)
            // Unreachable: the insert above recorded it, under this same guard.
            .ok_or(NativeRefusal::NotFound)?;
        Ok(model(created))
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let arn = own_arn(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        store.settle(at);
        let held = store
            .state_machines
            .get(&arn)
            .ok_or(NativeRefusal::NotFound)?;
        // A state machine the owning service is already deleting is one it will not change,
        // and it states that about the resource as it is.
        if held.is_deleting() {
            return Err(NativeRefusal::Rejected(sfn::resource_error_message(
                sfn::STATE_MACHINE_DELETING,
                &arn,
            )));
        }
        let name = held.name.clone();
        let held_type = held.sm_type.clone();
        let revision_id = held.revision_id.clone();
        let held_keys: Vec<String> = held
            .tags
            .iter()
            .filter_map(|tag| tag.key.clone())
            .collect();
        let desired = DesiredModel::read(
            properties,
            &Identity {
                arn: &arn,
                name: &name,
                revision_id: revision_id.as_deref(),
            },
        )?;
        // "Determines whether a Standard or Express state machine is created": the workflow
        // type is settled when the state machine is created, so a model that names the other
        // one asks the owning service for a change it does not make.
        if desired
            .sm_type
            .as_ref()
            .is_some_and(|wanted| *wanted != held_type)
        {
            return Err(NativeRefusal::Invalid(format!(
                "{STATE_MACHINE_TYPE} of this resource is {held_type}"
            )));
        }
        let definition = desired.definition()?;
        let role_arn = desired.role_arn()?;
        let update = sfn::StateMachineUpdate {
            definition: Some(definition),
            role_arn: Some(role_arn),
            // Configurations nothing here applies, which the owning write leaves exactly as
            // the state machine holds them.
            logging_configuration: None,
            tracing_configuration: None,
            encryption_configuration: None,
            revision_id: store.fresh_revision_id(),
            now: at,
        };
        store
            .apply_state_machine_update(&arn, update)
            .map_err(|rejection| match rejection {
                sfn::StateMachineUpdateRejection::NotFound => NativeRefusal::NotFound,
                sfn::StateMachineUpdateRejection::Deleting => NativeRefusal::Rejected(
                    sfn::resource_error_message(sfn::STATE_MACHINE_DELETING, &arn),
                ),
                sfn::StateMachineUpdateRejection::NoRoom => {
                    NativeRefusal::AtCapacity(sfn::store_bytes_exhausted())
                }
            })?;
        // The model describes the resource as a whole, so the tag set it carries is the one
        // in force afterwards: a key it no longer names is removed and the rest are written,
        // both through the owning service's own tag writes.
        let dropped: Vec<String> = held_keys
            .into_iter()
            .filter(|key| {
                !desired
                    .tags
                    .iter()
                    .any(|tag| tag.key.as_deref() == Some(key.as_str()))
            })
            .collect();
        if !dropped.is_empty() && !store.remove_tags(&arn, &dropped) {
            return Err(NativeRefusal::NotFound);
        }
        if !desired.tags.is_empty() {
            store
                .merge_tags(&arn, &desired.tags)
                .map_err(tag_write_refusal)?;
        }
        let updated = store
            .state_machines
            .get(&arn)
            .ok_or(NativeRefusal::NotFound)?;
        Ok(model(updated))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let arn = own_arn(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        store.settle(at);
        // The removal takes the state machine's versions, aliases and executions with it,
        // which is the owning service's own rule about what a deleted state machine leaves
        // behind. A Cloud Control API delete reports completion only once the resource is
        // gone, so it is removed rather than left reporting `DELETING`.
        store
            .remove_state_machine(&arn)
            .map(|_| ())
            .ok_or(NativeRefusal::NotFound)
    }
}

/// The ARN of the state machine `reference` names in `region`, or `None` when it is not an
/// ARN of a state machine of this Region and account.
///
/// The type's primary identifier is the state machine's own ARN, so that is the reference
/// every answer here resolves: it is read through the OWNING service's own reader of a state
/// machine ARN, so this provider and that service cannot disagree about which ARNs name a
/// state machine. A QUALIFIED ARN — one carrying a version, an alias or a Map Run label —
/// names something other than the state machine itself and so names no resource of this
/// type.
fn own_arn(region: &str, reference: &str) -> Option<String> {
    let parsed = sfn::read_state_machine_arn(reference, region).ok()?;
    if parsed.qualifier.is_some() || parsed.label().is_some() {
        return None;
    }
    (reference == sfn::state_machine_arn(region, &parsed.name)).then(|| reference.to_string())
}

/// The name the state machine ARN `arn` carries, read through the same reader [`own_arn`]
/// resolves it with.
fn name_of(region: &str, arn: &str) -> Option<String> {
    sfn::read_state_machine_arn(arn, region)
        .ok()
        .map(|parsed| parsed.name)
}

/// What the identifier of a resource of this type has to be, for a reference that is not one.
fn arn_requirement() -> NativeRefusal {
    NativeRefusal::Invalid(format!(
        "{IDENTIFIER_PROPERTY} must be the ARN of a state machine of this Region and account"
    ))
}

/// The owning service's verdict on a tag write, as this provider reports it.
fn tag_write_refusal(rejection: sfn::TagWriteRejection) -> NativeRefusal {
    match rejection {
        sfn::TagWriteRejection::NotFound => NativeRefusal::NotFound,
        sfn::TagWriteRejection::TooManyTags(_) => {
            NativeRefusal::Invalid(sfn::TOO_MANY_TAGS_MESSAGE.to_string())
        }
        sfn::TagWriteRejection::NoRoom => NativeRefusal::AtCapacity(sfn::store_bytes_exhausted()),
    }
}

/// The resource model one state machine renders as — the schema's properties the owning
/// record carries, and only those it HAS: a state machine nothing has updated carries no
/// revision and an untagged one carries no tag set, the same way the owning service's own
/// readers report neither.
fn model(machine: &sfn::StateMachine) -> Value {
    let mut model = Map::new();
    model.insert(
        IDENTIFIER_PROPERTY.to_string(),
        Value::from(machine.arn.clone()),
    );
    model.insert(NAME.to_string(), Value::from(machine.name.clone()));
    model.insert(
        STATE_MACHINE_NAME.to_string(),
        Value::from(machine.name.clone()),
    );
    model.insert(
        DEFINITION_STRING.to_string(),
        Value::from(machine.definition.clone()),
    );
    model.insert(ROLE_ARN.to_string(), Value::from(machine.role_arn.clone()));
    model.insert(
        STATE_MACHINE_TYPE.to_string(),
        Value::from(machine.sm_type.clone()),
    );
    if let Some(revision_id) = &machine.revision_id {
        model.insert(
            STATE_MACHINE_REVISION_ID.to_string(),
            Value::from(revision_id.clone()),
        );
    }
    let tags: BTreeMap<String, String> = machine
        .tags
        .iter()
        .filter_map(|tag| Some((tag.key.clone()?, tag.value.clone().unwrap_or_default())))
        .collect();
    if !tags.is_empty() {
        model.insert(TAGS.to_string(), tag_list(tags.iter()));
    }
    Value::Object(model)
}

/// What the resource a model describes already answers to: the values its identity and its
/// read-only properties may be stated back with, and nothing else.
struct Identity<'a> {
    /// The state machine's own ARN.
    arn: &'a str,
    /// Its name.
    name: &'a str,
    /// The revision it is at, or `None` for one nothing has updated.
    revision_id: Option<&'a str>,
}

/// What a resource model asks a state machine to be: the properties this path APPLIES,
/// having been checked against the owning service's own rules.
struct DesiredModel {
    /// The Amazon States Language definition, in the text form the owning record holds.
    definition: Option<String>,
    /// The IAM role the state machine is to run as.
    role_arn: Option<String>,
    /// The workflow type the model names, or `None` where it names none.
    sm_type: Option<String>,
    /// The tag set the state machine is to carry.
    tags: Vec<Tag>,
}

impl DesiredModel {
    /// Read `properties` as the model of the state machine `identity` names — or state why
    /// the owning service would not take it.
    ///
    /// The identity and the read-only properties may be CARRIED (a read reports them, so a
    /// model a client round-trips carries them) but only with the value the state machine
    /// already answers to: a value accepted here and never applied would be one no later
    /// read reports.
    fn read(properties: &Value, identity: &Identity<'_>) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            definition: None,
            role_arn: None,
            sm_type: None,
            tags: Vec::new(),
        };
        let mut definition_object: Option<String> = None;
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, identity.arn)?,
                NAME | STATE_MACHINE_NAME => require_value(property, value, identity.name)?,
                STATE_MACHINE_REVISION_ID => {
                    require_held(TYPE_NAME, property, value, identity.revision_id)?;
                }
                DEFINITION => definition_object = Some(definition_text(value)?),
                DEFINITION_STRING => desired.definition = Some(text(property, value)?),
                ROLE_ARN => desired.role_arn = Some(text(property, value)?),
                STATE_MACHINE_TYPE => desired.sm_type = Some(text(property, value)?),
                TAGS => desired.tags = tags_of(value)?,
                other if UNAPPLIED_PROPERTIES.contains(&other) => {
                    return Err(unsupported(TYPE_NAME, other));
                }
                other => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )));
                }
            }
        }
        // The schema carries the definition in two forms, which are two spellings of ONE
        // property of the resource: `Definition` is the WRITE-ONLY one a request supplies,
        // and `DefinitionString` the form a read reports. So a model carrying both is one a
        // caller built by writing `Definition` onto a model it read back, and the definition
        // it asks for is the one it wrote.
        desired.definition = definition_object.or(desired.definition);
        // What a definition, a role ARN and a workflow type ARE stays the owning service's
        // own judgement, asked before anything is written.
        if let Some(definition) = &desired.definition {
            if let Err(diagnostic) = sfn::asl::validate(definition) {
                return Err(NativeRefusal::Invalid(sfn::resource_error_message(
                    sfn::INVALID_STATE_MACHINE_DEFINITION,
                    &diagnostic,
                )));
            }
        }
        if let Some(role_arn) = &desired.role_arn {
            if let Some(why) = sfn::invalid_role_arn_reason(role_arn) {
                return Err(NativeRefusal::Invalid(why));
            }
        }
        if let Some(sm_type) = &desired.sm_type {
            if !STATE_MACHINE_TYPE_VALUES.contains(&sm_type.as_str()) {
                return Err(NativeRefusal::Invalid(format!(
                    "{STATE_MACHINE_TYPE} must be one of {STATE_MACHINE_TYPE_VALUES:?}"
                )));
            }
        }
        Ok(desired)
    }

    /// The definition the model names, which a state machine cannot be without.
    fn definition(&self) -> Result<String, NativeRefusal> {
        self.definition.clone().ok_or_else(|| {
            NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires {DEFINITION_STRING} or {DEFINITION}"
            ))
        })
    }

    /// The role ARN the model names, which a state machine cannot be without.
    fn role_arn(&self) -> Result<String, NativeRefusal> {
        self.role_arn
            .clone()
            .ok_or_else(|| NativeRefusal::Invalid(format!("{TYPE_NAME} requires {ROLE_ARN}")))
    }
}

/// The definition text the `Definition` `value` carries: the schema types it as the JSON
/// object a template writes the Amazon States Language inline as, and the owning record
/// holds that definition as text.
fn definition_text(value: &Value) -> Result<String, NativeRefusal> {
    if !value.is_object() {
        return Err(NativeRefusal::Invalid(format!(
            "{DEFINITION} must be a JSON object carrying the state machine's definition"
        )));
    }
    Ok(value.to_string())
}

/// The tag set `value` carries, in the owning service's own tag shape.
fn tags_of(value: &Value) -> Result<Vec<Tag>, NativeRefusal> {
    Ok(tag_entries(value)?
        .into_iter()
        .map(|(key, tag_value)| Tag {
            key: Some(key),
            value: Some(tag_value),
        })
        .collect())
}
