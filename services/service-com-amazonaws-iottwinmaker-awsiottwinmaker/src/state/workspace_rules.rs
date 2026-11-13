// SPDX-License-Identifier: CC0-1.0

//! The rules a WORKSPACE decides for the resources it holds.
//!
//! One concern of [`crate::state`], kept in its own file: a caller reaches these
//! through the parent module, which holds the [`Workspace`] record itself. Every
//! rule here is asked of one workspace's own component types and entities — how
//! a property definition resolves through an inheritance chain, which component
//! type a component instantiates, whether a value is a value of the property's
//! `dataType` — so every writer and every reader of a component decides it the
//! same way.

use super::*;

impl Workspace {
    /// The services linked into this workspace — what `GetWorkspace` and
    /// `ListWorkspaces` report as `linkedServices`, documented as "a list of
    /// services that are linked to the workspace".
    ///
    /// DERIVED from the workspace's sync jobs rather than stored beside them:
    /// what links another service into a workspace is a sync job against that
    /// service as its source (the sync role "is used by the SyncJob to read from
    /// the syncSource, and create, update, or delete the corresponding
    /// resources"), so the linked services ARE the sources of the jobs the
    /// workspace holds — and deriving them means a job created or deleted after
    /// a read cannot leave a stored list stale, the same choice
    /// [`has_child_entities`](Self::has_child_entities) makes.
    ///
    /// In sync-source order, because the map is keyed by source: two reads of an
    /// unchanged workspace report the same list.
    pub fn linked_services(&self) -> Vec<String> {
        self.sync_jobs.keys().cloned().collect()
    }

    /// The component types a CALLER created in this workspace: the stored ones
    /// whose ids lie outside the service's reserved namespace, which is exactly
    /// what `CreateComponentType` wrote.
    ///
    /// The built-ins every workspace is provisioned with are the SERVICE's own
    /// ([`built_in_component_types`]), not the caller's contents, so a caller
    /// that created none has an empty workspace however many built-ins it
    /// holds: this is what the per-workspace component-type ceiling counts
    /// against and what a `DeleteWorkspace` that requires an emptied workspace
    /// asks about. ONE definition of "the caller's component types", so a
    /// quota, a delete guard and any later reader cannot disagree.
    pub fn created_component_types(&self) -> impl Iterator<Item = &ComponentType> {
        self.component_types
            .values()
            .filter(|component_type| !built_in_component_type_id(&component_type.component_type_id))
    }

    /// Whether any stored entity names `entity_id` as its parent.
    ///
    /// Derived on every read rather than kept as a flag beside the entity, so a
    /// child created or deleted after the parent cannot leave it stale.
    pub fn has_child_entities(&self, entity_id: &str) -> bool {
        self.entities
            .values()
            .any(|entity| entity.parent_entity_id == entity_id)
    }

    /// The ids of the entities whose parent is `entity_id`, in id order.
    pub fn children_of(&self, entity_id: &str) -> Vec<String> {
        self.entities
            .values()
            .filter(|entity| entity.parent_entity_id == entity_id)
            .map(|entity| entity.entity_id.clone())
            .collect()
    }

    /// Everything stored in this workspace that still REFERENCES the component
    /// type `component_type_id`, as the IDS of the resources the reference lives
    /// on, each listed once and in id order.
    ///
    /// A stored reference to a component type is one of:
    ///
    /// * an entity's component, through `ComponentRequest::componentTypeId` —
    ///   the component IS an instance of the type, so the type's definitions are
    ///   what give the component's properties their meaning;
    /// * an entity's COMPOSITE component, through
    ///   [`CompositeComponentRecord::component_type_id`] — a composite
    ///   component is a component of the entity too (`GetEntity` reports it as
    ///   one) and is an instance of the type the declaration its path resolved
    ///   to named, so the type's definitions are equally what give its
    ///   properties their meaning, whatever depth it sits at and whether or not
    ///   the declaration that produced it is still on the holding component
    ///   type;
    /// * another component type that `extendsFrom` it — the child inherits its
    ///   property definitions;
    /// * another component type that names it in its `compositeComponentTypes`
    ///   — the composite type is built out of it.
    ///
    /// A `RELATIONSHIP` property definition's `targetComponentTypeId` is NOT
    /// counted: it is a declared target no writer in this service requires to
    /// resolve (a create stores it without checking, and it may name a built-in
    /// type of the real service this emulator does not model), so
    /// counting it would refuse deletes the real service allows.
    ///
    /// Derived on every call rather than kept as a count beside the component
    /// type, so a reference created or dropped afterwards cannot leave it stale
    /// — the same choice [`has_child_entities`](Self::has_child_entities) makes.
    pub fn component_type_references(&self, component_type_id: &str) -> ComponentTypeReferences {
        let mut entity_ids = BTreeSet::new();
        let mut referencing_type_ids = BTreeSet::new();
        for entity in self.entities.values() {
            for component in entity.components.values() {
                if component.component_type_id.as_deref() == Some(component_type_id) {
                    entity_ids.insert(entity.entity_id.clone());
                }
                for composite in component.composite_components.values() {
                    if composite.component_type_id == component_type_id {
                        entity_ids.insert(entity.entity_id.clone());
                    }
                }
            }
        }
        for component_type in self.component_types.values() {
            // A component type referencing ITSELF is not something that keeps
            // it alive: the reference goes away with the record that carries it.
            if component_type.component_type_id == component_type_id {
                continue;
            }
            if component_type
                .extends_from
                .iter()
                .any(|parent| parent == component_type_id)
            {
                referencing_type_ids.insert(component_type.component_type_id.clone());
            }
            for composite in component_type.composite_component_types.values() {
                if composite.component_type_id.as_deref() == Some(component_type_id) {
                    referencing_type_ids.insert(component_type.component_type_id.clone());
                }
            }
        }
        ComponentTypeReferences {
            entity_ids: entity_ids.into_iter().collect(),
            component_type_ids: referencing_type_ids.into_iter().collect(),
        }
    }

    /// Whether a declared reference to the component type `component_type_id`
    /// RESOLVES in this workspace — whether the service holds the component
    /// type the reference names.
    ///
    /// A reference resolves when this workspace stores a component type under
    /// the id — which includes the built-ins the workspace was provisioned with
    /// ([`built_in_component_types`]), since a component type is addressed
    /// inside its workspace and a reference never reaches across one — or when
    /// the id lies in the service's reserved namespace
    /// ([`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]) without being one of the
    /// built-ins this emulator models: the real service ships more of them than
    /// are modelled here, and refusing such an id would reject inheritance the
    /// real service accepts, which is the opposite error from the dangling
    /// reference this rule exists to catch.
    pub fn resolves_component_type(&self, component_type_id: &str) -> bool {
        self.component_types.contains_key(component_type_id)
            || built_in_component_type_id(component_type_id)
    }

    /// The transitive `extendsFrom` closure of `seeds`, in ONE walk: the stored
    /// component types it reaches (breadth-first, nearest first, each at most
    /// once), every id it reached whether stored or not, and whether any of
    /// those ids is NOT stored in this workspace — a built-in type of the real
    /// service that this emulator does not model, whose own definitions it
    /// therefore does not hold.
    ///
    /// The single walk over `extendsFrom` every reader of inherited structure
    /// shares, so an inheritance chain is resolved ONE way:
    /// [`component_type_chain`](Self::component_type_chain) takes the component
    /// types it reached,
    /// [`ancestor_component_type_ids`](Self::ancestor_component_type_ids) their
    /// ids.
    ///
    /// TERMINATION is structural, the same way [`subtree_of`](Self::subtree_of)
    /// gets it: an id is expanded only the first time it is seen and the ids
    /// come from this workspace's own component-type map, so the walk visits at
    /// most one id per stored component type plus the declared parents however
    /// the stored `extendsFrom` values point — a self-extending component type
    /// or an already-stored cycle cannot make it loop.
    pub(super) fn component_type_closure(
        &self,
        seeds: impl IntoIterator<Item = String>,
    ) -> (Vec<&ComponentType>, BTreeSet<String>, bool) {
        component_type_closure_in(&self.component_types, seeds)
    }

    /// Every component type `extends_from` reaches — the ids it names plus,
    /// transitively, the ids those stored component types themselves extend —
    /// and whether any of them is NOT stored in this workspace, i.e. is a
    /// built-in type of the real service this emulator does not model, whose
    /// own definitions it therefore does not hold.
    pub fn ancestor_component_type_ids(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (_, reached, holds_unstored) =
            self.component_type_closure(extends_from.iter().cloned());
        (reached, holds_unstored)
    }

    /// The property names a component type extending `extends_from` INHERITS —
    /// the own definitions of every component type the inheritance chain
    /// reaches — and whether some ancestor's definitions are unknown to this
    /// emulator (a built-in type), in which case a name the set does not carry
    /// cannot be called undefined.
    pub fn inherited_property_names(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (ancestors, holds_unstored) = self.ancestor_component_type_ids(extends_from);
        let mut names: BTreeSet<String> = BTreeSet::new();
        for ancestor in &ancestors {
            if let Some(stored) = self.component_types.get(ancestor) {
                names.extend(stored.property_definitions.keys().cloned());
            }
        }
        (names, holds_unstored)
    }

    /// The first property, in property-NAME order, whose definition a component
    /// type that `extends_from` these parents would hold with no `dataType` at
    /// all — `None` when every definition it would hold is one a read can
    /// report.
    ///
    /// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
    /// `PropertyDefinitionResponse` a read reports (see
    /// [`property_definition_response`]), so a declaration may omit it only
    /// where it REFINES one it inherits: some component type the `extendsFrom`
    /// chain reaches declares the SAME property name with a `dataType`, which is
    /// the value [`refined_property_definition`] would fold out for it (that
    /// fold takes `dataType` from the nearest declaration that sets one, so some
    /// declaration setting one is exactly the question). A component type that
    /// extends nothing inherits nothing, and one that extends something
    /// inherits nothing for a name no component type on its chain declares with
    /// a `dataType`: in either case a writer refuses the request rather than
    /// storing a definition a reader would have to drop. Where the chain
    /// reaches a built-in type this emulator does not model, it does not hold
    /// that type's declarations, so a `dataType` it cannot find there cannot be
    /// called uninherited and every definition stands — the same stand-down the
    /// sibling rule [`property_groups_violation`] makes on the same unknowable
    /// chain.
    ///
    /// Asked of the WHOLE set of definitions the component type would hold
    /// rather than of one of them, because `extends_from` is the other input and
    /// it is the same for every definition in the set: the inheritance chain is
    /// therefore resolved ONCE per request, by the single walk every reader of
    /// inherited structure shares, and each definition is then answered by a
    /// lookup in what that walk reached. Walked in name order, the way
    /// every writer of a requested map walks one ([`requested_in_key_order`]),
    /// so a record holding more than one such definition is always refused for
    /// the same one. Shared by every writer of a COMPONENT TYPE's definitions so
    /// a create and an update hold one definition to one rule; the same question
    /// about a COMPONENT's definition, whose inheritance runs through the
    /// component type it instantiates rather than through `extendsFrom`, is
    /// [`stranded_component_property_definition`](Self::stranded_component_property_definition).
    pub fn stranded_property_definition<'definitions>(
        &self,
        extends_from: &[String],
        definitions: impl IntoIterator<
            Item = (
                &'definitions str,
                &'definitions crate::ops::types::PropertyDefinitionRequest,
            ),
        >,
    ) -> Option<&'definitions str> {
        let (chain, _, inherits_unknown_definitions) =
            self.component_type_closure(extends_from.iter().cloned());
        if inherits_unknown_definitions {
            return None;
        }
        definitions
            .into_iter()
            .find_map(|(property_name, definition)| {
                if definition.data_type.is_some() {
                    return None;
                }
                let inherits_data_type = chain.iter().any(|ancestor| {
                    ancestor
                        .property_definitions
                        .get(property_name)
                        .is_some_and(|declared| declared.data_type.is_some())
                });
                (!inherits_data_type).then_some(property_name)
            })
    }

    /// Why the `defaultValue` one of the `definitions` a component type that
    /// `extends_from` these parents would hold declares cannot be stored —
    /// `None` when every one of them can.
    ///
    /// The rule is [`default_value_type_violation`], asked of each definition as
    /// a read would REPORT it: the declaration this writer is about to store
    /// folded over the ones the declared inheritance brings for the same property
    /// ([`refined_property_definition`]), which is the same resolution
    /// `GetComponentType` reports a definition through. A declaration may
    /// therefore refine an inherited `dataType` with a default of its own, or an
    /// inherited default with a `dataType` of its own, and it is the resolved
    /// pair that has to agree — so a patch that re-points `extendsFrom` away
    /// from the `dataType` an older default was written against is refused by
    /// the same walk that refuses a mistyped default in the request itself.
    ///
    /// Asked of the WHOLE set of definitions the component type would hold, over
    /// ONE walk of the declared chain, exactly as its sibling
    /// [`stranded_property_definition`](Self::stranded_property_definition) is,
    /// and in property-NAME order, so a record holding more than one such
    /// definition is always refused for the same one. Shared by every writer of a
    /// COMPONENT TYPE's definitions; the same question about a COMPONENT's
    /// definition is
    /// [`component_property_default_violation`](Self::component_property_default_violation).
    pub fn property_definition_default_violation<'definitions>(
        &self,
        extends_from: &[String],
        definitions: impl IntoIterator<
            Item = (
                &'definitions str,
                &'definitions crate::ops::types::PropertyDefinitionRequest,
            ),
        >,
    ) -> Option<String> {
        let (chain, _, _) = self.component_type_closure(extends_from.iter().cloned());
        definitions
            .into_iter()
            .find_map(|(property_name, definition)| {
                let reported = refined_property_definition(
                    ::std::iter::once(definition).chain(
                        chain
                            .iter()
                            .filter_map(|ancestor| ancestor.property_definitions.get(property_name)),
                    ),
                );
                default_value_type_violation(property_name, reported.as_ref())
            })
    }

    /// Every id that some stored entity names as its parent, in ONE pass.
    ///
    /// A listing needs `hasChildEntities` for each entity it reports; asking
    /// [`has_child_entities`](Self::has_child_entities) per reported entity would
    /// make the page cost quadratic in the workspace's size, so a list builds
    /// this set once and looks each id up in it.
    pub fn parent_ids(&self) -> std::collections::BTreeSet<&str> {
        self.entities
            .values()
            .map(|entity| entity.parent_entity_id.as_str())
            .collect()
    }

    /// `entity_id` plus every entity below it, parents before children.
    ///
    /// TERMINATION is structural, not assumed: an id is expanded only the first
    /// time it is seen, and the ids come from this workspace's own entity map,
    /// so the walk visits at most `entities.len() + 1` of them however the
    /// stored `parent_entity_id` values point — a self-parenting entity or a
    /// cycle cannot make it loop or grow without bound. The explicit cap below
    /// makes that bound local to this function rather than a property a reader
    /// has to derive.
    pub fn subtree_of(&self, entity_id: &str) -> Vec<String> {
        let cap = self.entities.len() + 1;
        let mut seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from([entity_id.to_string()]);
        let mut queue: std::collections::VecDeque<String> =
            std::collections::VecDeque::from([entity_id.to_string()]);
        let mut ordered = Vec::new();
        while let Some(parent) = queue.pop_front() {
            if ordered.len() >= cap {
                break;
            }
            ordered.push(parent.clone());
            for child in self.children_of(&parent) {
                if seen.insert(child.clone()) {
                    queue.push_back(child);
                }
            }
        }
        ordered
    }

    /// The component type `component_type_id` and every component type it
    /// extends, nearest first.
    ///
    /// `extendsFrom` is where a property definition a component type does not
    /// declare itself comes from, so a reader that has to resolve one walks this
    /// chain rather than a single map. The walk is
    /// [`component_type_closure`](Self::component_type_closure)'s, seeded with
    /// the component type itself; a parent this workspace does not hold (a
    /// built-in type of the real service this emulator does not model)
    /// contributes nothing rather than failing the walk.
    pub fn component_type_chain(&self, component_type_id: &str) -> Vec<&ComponentType> {
        let (chain, _, _) = self.component_type_closure([component_type_id.to_string()]);
        chain
    }

    /// Every component type an instance of `component_type_id` may name: the id
    /// itself plus every stored component type that extends it, directly or
    /// through another.
    ///
    /// The scope of a query addressed to a component TYPE, resolved in ONE pass
    /// over this workspace's component types (their `extendsFrom` edges,
    /// reversed) plus a walk of the ids those edges reach, so a scope is
    /// resolved once per request rather than once per component tested against
    /// it. Empty when this workspace holds no such component type, which is
    /// refused before a scope is wanted.
    ///
    /// TERMINATION is structural, as in
    /// [`component_type_closure`](Self::component_type_closure): an id is
    /// expanded only the first time it is seen and every id comes from this
    /// workspace's own component-type map.
    pub fn component_type_instance_ids(&self, component_type_id: &str) -> BTreeSet<&str> {
        let Some(named) = self.component_types.get(component_type_id) else {
            return BTreeSet::new();
        };
        let mut extenders: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for component_type in self.component_types.values() {
            for parent in &component_type.extends_from {
                extenders
                    .entry(parent.as_str())
                    .or_default()
                    .push(component_type.component_type_id.as_str());
            }
        }
        let mut reached: BTreeSet<&str> = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([named.component_type_id.as_str()]);
        while let Some(id) = queue.pop_front() {
            if !reached.insert(id) {
                continue;
            }
            if let Some(children) = extenders.get(id) {
                queue.extend(children.iter().copied());
            }
        }
        reached
    }

    /// The definition that gives one property of a component its meaning,
    /// RESOLVED over every declaration that reaches the property: the
    /// component's own declaration where it has one, then the declarations of
    /// the component type it instantiates and of the types that type extends,
    /// nearest first.
    ///
    /// A component may carry a property its entity's create never restated —
    /// naming the component's `componentTypeId` is what gives the component that
    /// type's properties — so a reader of a property's definition, its default
    /// value or its data type resolves it through here rather than off the
    /// component alone. A nearer declaration may also REFINE a further one
    /// rather than replace it (every member of `PropertyDefinitionRequest` is
    /// optional, so a declaration can restate only the members it changes),
    /// which is why the whole set of declarations is folded
    /// ([`refined_property_definition`]) instead of the nearest one being taken
    /// whole. `None` when no declaration reaches the property at all.
    pub fn effective_property_definition(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
        property_name: &str,
    ) -> Option<crate::ops::types::PropertyDefinitionRequest> {
        self.refined_component_property_definition(
            component_type_id,
            properties
                .get(property_name)
                .and_then(|property| property.definition.as_ref()),
            property_name,
        )
    }

    /// [`effective_property_definition`](Self::effective_property_definition)
    /// with the component's OWN declaration named explicitly: `own` is folded
    /// over the declarations the component type `component_type_id` and the
    /// types it extends make for `property_name`, nearest first.
    ///
    /// What a WRITER needs, because the declaration it is about to store is not
    /// on the record yet: the definition a property ends up with is the one the
    /// request carries where it carries one (a definition a request names
    /// REPLACES the stored one) and the stored one otherwise, resolved over the
    /// same inheritance chain a read resolves. A reader of the stored record
    /// goes through
    /// [`effective_property_definition`](Self::effective_property_definition),
    /// which is this with the record's own declaration, so a writer and a
    /// reader resolve one property one way.
    pub fn refined_component_property_definition(
        &self,
        component_type_id: Option<&str>,
        own: Option<&crate::ops::types::PropertyDefinitionRequest>,
        property_name: &str,
    ) -> Option<crate::ops::types::PropertyDefinitionRequest> {
        let chain = component_type_id
            .map(|component_type_id| self.component_type_chain(component_type_id))
            .unwrap_or_default();
        refined_property_definition(own.into_iter().chain(
            chain
                .iter()
                .filter_map(|component_type| component_type.property_definitions.get(property_name)),
        ))
    }

    /// Whether storing `definition` for `property_name` on a component that
    /// instantiates `component_type_id` would leave the component holding a
    /// definition no read can report.
    ///
    /// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
    /// `PropertyDefinitionResponse` a read reports (see
    /// [`property_definition_response`]), so a declaration may omit it only
    /// where it REFINES one that reaches the property anyway — for a component,
    /// a declaration of the component type it instantiates or of a type that
    /// type extends, which is exactly what
    /// [`effective_property_definition`](Self::effective_property_definition)
    /// folds in with no declaration of the component's own to fold over. A
    /// property nothing on that chain declares with a `dataType` — a component
    /// naming no component type, naming one this workspace does not hold, or
    /// declaring a property its chain never mentions — inherits none, so a
    /// writer refuses the request rather than storing a definition a reader
    /// would have to drop.
    ///
    /// The component-level counterpart of
    /// [`stranded_property_definition`](Self::stranded_property_definition),
    /// which asks the same question of a component TYPE's own definitions;
    /// shared by every writer of a component's definition so a create and an
    /// update hold one definition to one rule.
    pub fn component_property_definition_needs_data_type(
        &self,
        component_type_id: Option<&str>,
        property_name: &str,
        definition: &crate::ops::types::PropertyDefinitionRequest,
    ) -> bool {
        definition.data_type.is_none()
            && self
                .effective_property_definition(component_type_id, &BTreeMap::new(), property_name)
                .and_then(|inherited| inherited.data_type)
                .is_none()
    }

    /// The first property, in property-NAME order, whose definition a component
    /// instantiating `component_type_id` would hold with no `dataType` at all —
    /// `None` when every definition the component holds is one a read can
    /// report.
    ///
    /// [`component_property_definition_needs_data_type`](Self::component_property_definition_needs_data_type)
    /// asked of the definitions a component ALREADY HOLDS rather than of one a
    /// request carries, because the rule's other input is the component type
    /// those declarations inherit a `dataType` from: a writer that re-points it
    /// is the call that takes the inheritance away, so it answers the rule over
    /// the WHOLE record it would store instead of over the entries the request
    /// happened to name. Walked in name order, the way every writer of a
    /// requested map walks one ([`requested_in_key_order`]), so a record holding
    /// more than one such definition is always refused for the same one.
    pub fn stranded_component_property_definition<'properties>(
        &self,
        component_type_id: Option<&str>,
        properties: &'properties BTreeMap<String, PropertyRecord>,
    ) -> Option<&'properties str> {
        properties.iter().find_map(|(property_name, property)| {
            let definition = property.definition.as_ref()?;
            self.component_property_definition_needs_data_type(
                component_type_id,
                property_name,
                definition,
            )
            .then_some(property_name.as_str())
        })
    }

    /// Why one of the properties a component instantiating `component_type_id`
    /// HOLDS would report a `defaultValue` the `dataType` beside it does not
    /// declare — `None` when none of them would.
    ///
    /// [`component_property_default_violation`](Self::component_property_default_violation)
    /// asked of the definitions a component ALREADY HOLDS rather than of one a
    /// request carries, for the reason its sibling
    /// [`stranded_component_property_definition`](Self::stranded_component_property_definition)
    /// is: the component type is the other input of the rule — the declarations
    /// a component holds resolve both their `dataType` and their default through
    /// it — so a writer that re-points it answers the rule over the WHOLE record
    /// it would store instead of over the entries the request happened to name.
    /// Walked in name order, the way every writer of a requested map walks one
    /// ([`requested_in_key_order`]), so a record holding more than one such
    /// definition is always refused for the same one.
    pub fn component_property_defaults_violation(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
    ) -> Option<String> {
        properties.iter().find_map(|(property_name, property)| {
            self.component_property_default_violation(
                component_type_id,
                property_name,
                property.definition.as_ref(),
            )
        })
    }

    /// Why `value` cannot be written to the property `property_name` of a
    /// component that instantiates `component_type_id` and declares
    /// `definition` for it — `None` when it can.
    ///
    /// The rule is [`data_value_type_violation`], asked of the definition the
    /// property EFFECTIVELY carries: the declaration the write itself holds,
    /// refined over the ones it INHERITS from the component type the holder
    /// instantiates and from the types that type extends
    /// ([`refined_component_property_definition`](Self::refined_component_property_definition))
    /// — the same resolution every reader of the property's definition makes
    /// ([`effective_property_definition`](Self::effective_property_definition)).
    /// A value is therefore judged against the `dataType` a read REPORTS for
    /// the property, whether the property declares it itself or inherits it,
    /// and a write of another type is refused rather than stored.
    ///
    /// The component-level counterpart of the question the time-series data
    /// plane asks of every value it writes, shared by every writer of a
    /// component's property value so one value is held to one rule wherever it
    /// is written.
    pub fn component_property_value_violation(
        &self,
        component_type_id: Option<&str>,
        property_name: &str,
        definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
        value: &crate::ops::types::DataValue,
    ) -> Option<String> {
        data_value_type_violation(
            property_name,
            self.refined_component_property_definition(component_type_id, definition, property_name)
                .as_ref(),
            value,
        )
    }

    /// Why the `defaultValue` the definition `definition` declares for the
    /// property `property_name` of a component that instantiates
    /// `component_type_id` cannot be stored — `None` when it can.
    ///
    /// The rule is [`default_value_type_violation`], asked of the definition the
    /// property EFFECTIVELY carries: the declaration the write itself holds,
    /// refined over the ones it INHERITS from the component type the holder
    /// instantiates and from the types that type extends
    /// ([`refined_component_property_definition`](Self::refined_component_property_definition))
    /// — the same resolution every reader of the property's definition makes
    /// ([`effective_property_definition`](Self::effective_property_definition)).
    /// A default is therefore judged against the `dataType` a read REPORTS for
    /// the property, whether the property declares it itself or inherits it.
    ///
    /// The component-level counterpart of
    /// [`property_definition_default_violation`](Self::property_definition_default_violation),
    /// which asks the same question of a component TYPE's own definitions, and
    /// the definition-side counterpart of
    /// [`component_property_value_violation`](Self::component_property_value_violation):
    /// a default is a value of the property, so one rule holds every value a
    /// component's property comes to report, whichever member it arrived in.
    pub fn component_property_default_violation(
        &self,
        component_type_id: Option<&str>,
        property_name: &str,
        definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
    ) -> Option<String> {
        default_value_type_violation(
            property_name,
            self.refined_component_property_definition(component_type_id, definition, property_name)
                .as_ref(),
        )
    }

    /// The component types one component type INHERITS from: the `extendsFrom`
    /// chain behind it, nearest first, without the component type itself.
    ///
    /// What a reader of a component type's own schema needs in order to report
    /// the schema the type effectively has: the declarations these contribute
    /// are the inherited ones. The walk is
    /// [`component_type_closure`](Self::component_type_closure)'s, seeded with
    /// the type's declared parents rather than with the type, so a parent this
    /// workspace does not hold (a built-in of the real service this emulator
    /// does not model) contributes nothing rather than failing the walk.
    pub fn inherited_component_types(&self, component_type: &ComponentType) -> Vec<&ComponentType> {
        let (chain, _, _) = self.component_type_closure(component_type.extends_from.iter().cloned());
        chain
    }

    /// The property group `group_name` as it applies to a component, RESOLVED
    /// over every declaration that reaches it: the component's own where it has
    /// one, then the declarations of the component type it instantiates and of
    /// the types that type extends, nearest first — the same resolution
    /// [`effective_property_definition`](Self::effective_property_definition)
    /// makes for a definition.
    ///
    /// A nearer declaration may REFINE a further one rather than replace it
    /// (every member of `ComponentPropertyGroupRequest` is optional, so a
    /// declaration can restate only the members it changes), which is why the
    /// whole set of declarations is folded ([`refined_property_group`]) instead
    /// of the nearest one being taken whole. `None` when no declaration reaches
    /// the group at all.
    pub fn effective_property_group(
        &self,
        component_type_id: Option<&str>,
        property_groups: &BTreeMap<String, PropertyGroupRecord>,
        group_name: &str,
    ) -> Option<PropertyGroupRecord> {
        let own = property_groups.get(group_name);
        let chain = component_type_id
            .map(|component_type_id| self.component_type_chain(component_type_id))
            .unwrap_or_default();
        refined_property_group(own.into_iter().chain(
            chain
                .iter()
                .filter_map(|component_type| component_type.property_groups.get(group_name)),
        ))
    }

    /// Whether some component type the component's `component_type_id` reaches
    /// — the type itself or one it extends — declares an entry named `name` in
    /// the part of its schema `keyed` selects.
    ///
    /// The one test both of a component's `isInherited` flags
    /// ([`property_definition_is_inherited`](Self::property_definition_is_inherited),
    /// [`property_group_is_inherited`](Self::property_group_is_inherited)) are
    /// decided by, so a definition and a group answer the question the same way,
    /// over the same walk every reader of inherited structure shares
    /// ([`component_type_chain`](Self::component_type_chain)). `false` when the
    /// component instantiates no component type, or names one this workspace
    /// does not hold: then nothing reaches it through a type.
    pub(super) fn component_type_chain_declares<Entry>(
        &self,
        component_type_id: Option<&str>,
        name: &str,
        keyed: impl Fn(&ComponentType) -> &BTreeMap<String, Entry>,
    ) -> bool {
        component_type_id.is_some_and(|component_type_id| {
            self.component_type_chain(component_type_id)
                .iter()
                .any(|component_type| keyed(component_type).contains_key(name))
        })
    }

    /// Whether the definition
    /// [`effective_property_definition`](Self::effective_property_definition)
    /// resolves for `property_name` is INHERITED: the component type the
    /// component instantiates, or a component type that type extends, declares
    /// the property, so the definition reaches the component through the type it
    /// instantiates whether or not the component's own record also REFINES it.
    ///
    /// A component's own declaration need not REPLACE the inherited definition:
    /// every member of `PropertyDefinitionRequest` is optional, so a declaration
    /// can restate only the members it changes and the definition reported then
    /// carries the inherited values of every member it left alone
    /// ([`refined_property_definition`]). `isInherited` reports where the
    /// reported definition comes from, and for such a refinement it still comes
    /// through the inheritance chain — the same rule `GetComponentType` applies
    /// to a component type that refines a definition its `extendsFrom` chain
    /// declares. A property no component type on the chain declares is the
    /// component's own and nothing else's; one whose record carries a value but
    /// no definition is the type's, the value being the component's.
    pub fn property_definition_is_inherited(
        &self,
        component_type_id: Option<&str>,
        property_name: &str,
    ) -> bool {
        self.component_type_chain_declares(component_type_id, property_name, |component_type| {
            &component_type.property_definitions
        })
    }

    /// Whether the group
    /// [`effective_property_group`](Self::effective_property_group) resolves for
    /// `group_name` is INHERITED — the same question
    /// [`property_definition_is_inherited`](Self::property_definition_is_inherited)
    /// answers for a definition, asked of a component's property groups, and
    /// answered by the same test: a group the inheritance chain declares is
    /// inherited whether or not the component also refines it.
    pub fn property_group_is_inherited(
        &self,
        component_type_id: Option<&str>,
        group_name: &str,
    ) -> bool {
        self.component_type_chain_declares(component_type_id, group_name, |component_type| {
            &component_type.property_groups
        })
    }

    /// Every property a component HAS: the ones it stores itself, plus the ones
    /// the component type it instantiates — or a type that type extends —
    /// defines and it never restated.
    ///
    /// Naming a `componentTypeId` is what gives a component that type's
    /// properties, so a component commonly stores only the VALUE of a property
    /// whose definition lives on its type, and a property whose definition it
    /// inherits is as much a property of the component as one it declares
    /// inline. In property-name order, so a reader that pages over the set
    /// resumes at a name. Shared by every reader that has to enumerate a
    /// component's properties rather than resolve one of them
    /// ([`effective_property_definition`](Self::effective_property_definition)).
    pub fn component_property_names(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
    ) -> BTreeSet<String> {
        let mut property_names: BTreeSet<String> = properties.keys().cloned().collect();
        if let Some(component_type_id) = component_type_id {
            for component_type in self.component_type_chain(component_type_id) {
                property_names.extend(component_type.property_definitions.keys().cloned());
            }
        }
        property_names
    }

    /// Every property group a component HAS: the ones it declares itself, plus
    /// the ones the component type it instantiates — or a type that type
    /// extends — declares and it never restated.
    ///
    /// The same resolution [`component_property_names`](Self::component_property_names)
    /// makes for a component's properties, applied to its groups, so a reader
    /// that enumerates either reports the component's EFFECTIVE schema rather
    /// than only the part its entity's create restated. In group-name order.
    pub fn component_property_group_names(
        &self,
        component_type_id: Option<&str>,
        property_groups: &BTreeMap<String, PropertyGroupRecord>,
    ) -> BTreeSet<String> {
        let mut group_names: BTreeSet<String> = property_groups.keys().cloned().collect();
        if let Some(component_type_id) = component_type_id {
            for component_type in self.component_type_chain(component_type_id) {
                group_names.extend(component_type.property_groups.keys().cloned());
            }
        }
        group_names
    }

    /// Where the definition set a component's inherited schema comes from: the
    /// component type the component instantiates, when this workspace holds it.
    ///
    /// Reported as `definedIn` ("the name of the property definition set"),
    /// which is what tells a caller that the definitions it is being shown were
    /// not declared on the entity. The INSTANTIATED type is the name, not
    /// whichever type on its `extendsFrom` chain happens to declare a given
    /// definition: `definedIn` is one string per component while the component's
    /// definitions may come from several types at once, and the instantiated
    /// type is the definition set the component was given — the chain behind it
    /// is that type's own structure, which `GetComponentType` reports. It is
    /// named whenever it resolves, including for a type that declares nothing:
    /// what resolved is a definition set, not a non-empty one. `None` when the
    /// component instantiates no component type, or names one this workspace
    /// does not hold — then nothing was resolved and there is no definition set
    /// to name.
    pub fn component_defined_in(&self, component_type_id: Option<&str>) -> Option<String> {
        let component_type_id = component_type_id?;
        self.component_types
            .get(component_type_id)
            .map(|component_type| component_type.component_type_id.clone())
    }

    /// The external ids a component carries: the value of every property of it
    /// whose EFFECTIVE definition marks the property as the external id — "the
    /// primary key of an external storage system".
    ///
    /// `isExternalId` is a member of the property DEFINITION, and a definition
    /// normally lives on the component TYPE the component instantiates — naming
    /// a `componentTypeId` is what gives a component that type's properties, so
    /// a component usually supplies only the VALUE of its external-id property.
    /// Which property is the external id is therefore resolved through
    /// [`effective_property_definition`](Self::effective_property_definition),
    /// exactly as every other member of a definition is: the component's own
    /// inline definition where it declares one, else the one it inherits from
    /// the component type it instantiates or from a type that type extends. The
    /// value is the one a READ of the property reports
    /// ([`latest_property_value`]), so a property whose value comes from its
    /// definition's `defaultValue` carries the same external id a read of it
    /// shows.
    ///
    /// `properties` are the component's stored properties and
    /// `component_type_id` the type it instantiates, so one component and one
    /// composite component — which carry the same property records — resolve
    /// their external ids the same way. `ListEntities`' `externalId` filter
    /// selects on this.
    ///
    /// The cost is bounded by the component's own properties plus the
    /// definitions on its inheritance chain, which
    /// [`component_type_chain`](Self::component_type_chain) walks at most once
    /// per component type reached.
    pub fn component_external_ids(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
    ) -> BTreeSet<String> {
        // Every property the component HAS: the ones it stores itself plus the
        // ones its component type gives it and that it never restated — the
        // external-id property is normally one of the latter.
        let property_names = self.component_property_names(component_type_id, properties);
        let mut external_ids = BTreeSet::new();
        for property_name in &property_names {
            let Some(definition) =
                self.effective_property_definition(component_type_id, properties, property_name)
            else {
                continue;
            };
            if !definition.is_external_id.unwrap_or(false) {
                continue;
            }
            if let Some(value) =
                latest_property_value(properties.get(property_name), Some(&definition))
            {
                // Only a STRING value is an external id: `externalId` on the
                // filter is a string, so a property of any other data type
                // carries nothing to match against.
                if let Some(string_value) = value.string_value {
                    external_ids.insert(string_value);
                }
            }
        }
        external_ids
    }

    /// Why the `properties` a component instantiating `component_type_id` would
    /// come to hold cannot be stored, as the message a `ValidationException`
    /// carries — `None` when every one of them can.
    ///
    /// The four rules every writer of a component's properties meets, asked of
    /// each requested property in property-NAME order so a request carrying
    /// more than one refusable property is always refused for the first of them:
    ///
    /// * a definition whose `dataType` carries a `type` outside the modelled
    ///   `Type` value list ([`property_definition_violation`]), which a read
    ///   would otherwise report verbatim on a required member the model does
    ///   not admit it on;
    /// * a definition that would leave the property with no `dataType` at all
    ///   ([`component_property_definition_needs_data_type`](Self::component_property_definition_needs_data_type)),
    ///   which `PropertyDefinitionResponse` requires a read to report;
    /// * a VALUE that is not a value of the `dataType` the property has
    ///   ([`component_property_value_violation`](Self::component_property_value_violation));
    /// * a `defaultValue` that is not one either
    ///   ([`component_property_default_violation`](Self::component_property_default_violation)),
    ///   since a read reports it as the property's value while nothing has been
    ///   written to the property.
    ///
    /// `member_path` is the member the properties hang off in the caller's own
    /// terms (`components.<name>`, `compositeComponents.<path>`, or the entry of
    /// a metadata document being transferred), so the refusal names the exact
    /// place that has to be fixed. Shared by `CreateEntity`, `UpdateEntity` and
    /// the writer that lands a transferred entity, so a component the control
    /// plane would refuse is not one a transfer stores.
    pub fn component_property_violation<'properties>(
        &self,
        component_type_id: Option<&str>,
        member_path: &str,
        properties: impl IntoIterator<
            Item = (
                &'properties str,
                &'properties crate::ops::types::PropertyRequest,
            ),
        >,
    ) -> Option<String> {
        for (property_name, property) in properties {
            // Asked of the definition the request LITERALLY carries, before any
            // resolution: a `dataType` outside the modelled value list is one
            // no read could report and no `DataValue` could ever satisfy, so it
            // is refused rather than stored.
            if let Some(definition) = property.definition.as_ref() {
                if let Some(violation) = property_definition_violation(
                    &format!("{member_path}.properties.{property_name}.definition"),
                    definition,
                ) {
                    return Some(violation);
                }
            }
            if property.definition.as_ref().is_some_and(|definition| {
                self.component_property_definition_needs_data_type(
                    component_type_id,
                    property_name,
                    definition,
                )
            }) {
                return Some(format!(
                    "{member_path}.properties.{property_name}.definition.dataType is required"
                ));
            }
            // Asked of the value the write would STORE, before anything is
            // stored: a property with no declaration of its own is still judged
            // against the definition it inherits.
            if let Some(value) = property.value.as_ref() {
                if let Some(violation) = self.component_property_value_violation(
                    component_type_id,
                    property_name,
                    property.definition.as_ref(),
                    value,
                ) {
                    return Some(violation);
                }
            }
            if let Some(violation) = self.component_property_default_violation(
                component_type_id,
                property_name,
                property.definition.as_ref(),
            ) {
                return Some(violation);
            }
        }
        None
    }
}
