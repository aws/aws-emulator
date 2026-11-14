// SPDX-License-Identifier: CC0-1.0

//! THE ACTIONS A REFACTOR'S PLAN CONCLUDES: "the stack refactor actions that will be taken
//! after calling the ExecuteStackRefactor action", which `ListStackRefactorActions` reports.
//!
//! An action "describes the stack and the action that CloudFormation will perform on it if you
//! execute the stack refactor", so the list is a property of the PLAN rather than of a read:
//! it is computed once, by the call that creates the refactor, from the refactor it was asked
//! for and the stacks the store held at that instant
//! ([`super::stack_refactors::plan`]) — the same conclusion the creation's `Status` and
//! `StackIds` come from. A read reports that record and recomputes nothing, so two reads of one
//! refactor report the same actions whatever happened to the stacks in between.
//!
//! ## What CloudFormation will DO
//!
//! Two things, which is exactly what the modelled `StackRefactorActionType` carries:
//!
//! * `CREATE` a stack the refactor names that does not exist yet, which is what
//!   `EnableStackCreation` ("determines if a new stack is created with the refactor") allows —
//!   an action on a `STACK`; and
//! * `MOVE` a resource out of the stack that holds it into the stack that is to hold it — an
//!   action on a `RESOURCE`, carrying the `ResourceMapping` of that move.
//!
//! A stack has to exist before a resource can be moved into it, so the actions are reported in
//! the order CloudFormation carries them out: every stack it creates, then every move.
//!
//! ## How a move is ARRIVED AT
//!
//! `Detection` is "the detection type", and the model gives both of its values a sentence:
//! "Manual: The customer provided the mapping in the `ResourceMapping` parameter" and "Auto:
//! CloudFormation figured out the mapping on its own". So a move the request named is
//! `MANUAL`, and a move the plan worked out from the final states the request submitted is
//! `AUTO`: a resource a stack holds that its submitted template no longer declares has to go
//! somewhere, and the stack whose submitted template newly declares a resource of that type is
//! where it goes. That is the one inference this planner makes, and it makes it only where the
//! request named no mapping for either end — a mapping the request DID name is the customer's,
//! and nothing is inferred over it.
//!
//! ## Bounds
//!
//! A caller chooses how many stacks a refactor spans and how large each one's template is, so
//! what one plan may record is bounded ([`MAX_ACTIONS_PER_REFACTOR`]), and the two ends of a
//! detected move are INDEXED rather than compared pairwise, so working the moves out costs what
//! the request submitted rather than the product of its parts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::stack_refactors::{CoveredStack, RefactorResourceLocation, RefactorResourceMapping};

/// The most actions ONE plan records.
///
/// A plan carries one action per stack the refactor creates and one per resource it moves, and
/// a caller chooses both: the moves it NAMES are already held to
/// [`super::stack_refactors::MAX_RESOURCE_MAPPINGS_PER_REFACTOR`] by the submitting operation,
/// while the ones the plan works out itself are a function of the templates it submitted, so
/// this is what bounds those. Chosen as the two bounds the request is already held to, since a
/// refactor cannot usefully carry more actions than the stacks and moves one record may name;
/// a plan that could see more stops at this many rather than growing the store without limit.
pub const MAX_ACTIONS_PER_REFACTOR: usize = super::stack_refactors::MAX_STACKS_PER_REFACTOR
    + super::stack_refactors::MAX_RESOURCE_MAPPINGS_PER_REFACTOR;

/// The modelled `StackRefactorActionType` values
/// (`smithy_query(query="shape", shape="StackRefactorActionType")`): "the action that
/// CloudFormation takes on the stack".
pub mod action {
    pub const MOVE: &str = "MOVE";
    pub const CREATE: &str = "CREATE";
}

/// The modelled `StackRefactorActionEntity` values
/// (`smithy_query(query="shape", shape="StackRefactorActionEntity")`): "the type that will be
/// evaluated in the `StackRefactorAction`".
pub mod entity {
    pub const RESOURCE: &str = "RESOURCE";
    pub const STACK: &str = "STACK";
}

/// The modelled `StackRefactorDetection` values
/// (`smithy_query(query="shape", shape="StackRefactorDetection")`): how the mapping an action
/// carries was arrived at.
pub mod detection {
    pub const AUTO: &str = "AUTO";
    pub const MANUAL: &str = "MANUAL";
}

/// `DetectionReason`: "the description of the detection type" — the sentence the model gives
/// the detection type this action carries, which is what makes the two values tell a reader
/// WHO decided the mapping.
///
/// The text is the `StackRefactorDetection` documentation's own sentence for each value, since
/// that is the only published description of the detection type; nothing fixes the sentence the
/// service itself writes here, so a recorded response is what would settle it.
pub fn detection_reason(detection: &str) -> Option<String> {
    match detection {
        detection::MANUAL => {
            Some("The customer provided the mapping in the ResourceMapping parameter".to_string())
        }
        detection::AUTO => Some("CloudFormation figured out the mapping on its own".to_string()),
        _ => None,
    }
}

/// The property a stack is identified BY, which is the one a `ResourceIdentifier` names for an
/// action on a `STACK`: a stack of this account and Region is addressed by "the name
/// associated with the stack" (`StackDefinition$StackName`, `ResourceLocation$StackName`).
const STACK_NAME_PROPERTY: &str = "StackName";

/// `StackRefactorAction$ResourceIdentifier`: "a key-value pair that identifies the target
/// resource. The key is an identifier property (for example, `BucketName` for
/// `AWS::S3::Bucket` resources) and the value is the actual property value (for example,
/// `MyS3Bucket`)".
///
/// The member is ONE string holding a pair, and nothing published fixes how the service spells
/// that pair — so it is written as the JSON object a client can read the key and the value out
/// of with the parser it already has, rather than as a separator only this emulator knows how
/// to take apart. A recorded response would settle the spelling; the KEY and the VALUE are
/// decided here, from the resource's type and the instance the store holds, either way.
fn resource_identifier(property: &str, value: &str) -> String {
    ::serde_json::json!({ property: value }).to_string()
}

/// One resource a stack holds right now, as the planner reads it: enough to say WHICH physical
/// instance a move acts on and how a client names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveredResource {
    pub logical_id: String,
    pub resource_type: String,
    /// "The name or unique identifier associated with the physical instance of the resource",
    /// which is what the action reports as its `PhysicalResourceId`.
    pub physical_resource_id: String,
}

/// The final state ONE stack definition submits, as the planner reads it: "the stack and the
/// template used by the stack".
///
/// `declared` is the resources that template declares, in declaration order, and `None` for a
/// definition that submitted no template at all — a stack whose own template the refactor does
/// not change, which therefore says nothing about what moves.
#[derive(Debug, Clone, Default)]
pub struct FinalState {
    pub stack_name: String,
    pub declared: Option<Vec<DeclaredFinalResource>>,
}

/// One resource a submitted final-state template declares: the logical id it is to be known
/// by in that stack, and its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredFinalResource {
    pub logical_id: String,
    pub resource_type: String,
}

/// One action of a refactor's plan, as this emulator persists it — the modelled
/// `StackRefactorAction`, member for member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRefactorAction {
    /// [`action`] — "the action that CloudFormation takes on the stack".
    pub action: String,
    /// [`entity`] — "the type that will be evaluated in the `StackRefactorAction`".
    pub entity: String,
    /// "The name or unique identifier associated with the physical instance of the resource",
    /// and `None` where nothing physical exists yet: a stack the refactor is to CREATE has no
    /// instance to name.
    pub physical_resource_id: Option<String>,
    /// [`resource_identifier`] — the pair that identifies the target of this action.
    pub resource_identifier: Option<String>,
    /// [`detection`], for an action carrying a mapping; `None` for one that carries none.
    pub detection: Option<String>,
    /// [`detection_reason`].
    pub detection_reason: Option<String>,
    /// "The mapping for the stack resource `Source` and stack resource `Destination`" — the
    /// move this action carries out, and `None` for an action on a stack.
    pub resource_mapping: Option<RefactorResourceMapping>,
}

impl StoredRefactorAction {
    /// The action that CREATES one stack the refactor names and this Region holds none of.
    fn creating_stack(stack_name: &str) -> Self {
        Self {
            action: action::CREATE.to_string(),
            entity: entity::STACK.to_string(),
            // The stack does not exist yet, so there is no physical instance to name: the
            // member is left out rather than filled with the name of something that is still
            // to be created.
            physical_resource_id: None,
            resource_identifier: Some(resource_identifier(STACK_NAME_PROPERTY, stack_name)),
            detection: None,
            detection_reason: None,
            resource_mapping: None,
        }
    }

    /// The action that MOVES one resource, from the stack that holds it to the stack that is
    /// to hold it.
    ///
    /// `held` is the resource as the source stack holds it right now, where the source
    /// resolves to one: the physical instance the move acts on, and the property a client
    /// names it by ([`super::primary_identifier`]). A mapping whose source resolves to no
    /// resource reports neither, rather than a physical id nothing stands behind.
    fn moving_resource(mapping: RefactorResourceMapping, detection: &str, held: Option<&CoveredResource>) -> Self {
        Self {
            action: action::MOVE.to_string(),
            entity: entity::RESOURCE.to_string(),
            physical_resource_id: held.map(|resource| resource.physical_resource_id.clone()),
            resource_identifier: held.and_then(|resource| {
                super::primary_identifier::properties(&resource.resource_type)
                    .first()
                    .map(|property| {
                        resource_identifier(property, &resource.physical_resource_id)
                    })
            }),
            detection: Some(detection.to_string()),
            detection_reason: detection_reason(detection),
            resource_mapping: Some(mapping),
        }
    }
}

/// Work out what executing the refactor WILL DO, from the refactor that was asked for and the
/// stacks the store holds.
///
/// `covered` carries one entry per stack the refactor names, in the order it first named one,
/// and `final_states` the templates its definitions submitted. Called by
/// [`super::stack_refactors::plan`] for a refactor that can be carried out: a creation that
/// concludes `CREATE_FAILED` has no plan to execute, so it has no actions either.
///
/// Every end of every move is reported as [`reported_location`] resolves it, so a move names
/// the stack it acts on by the stack's own identity rather than by whichever of the spellings
/// `StackName` admits the request happened to use.
pub fn plan_actions(
    mappings: &[RefactorResourceMapping],
    covered: &[CoveredStack],
    final_states: &[FinalState],
) -> Vec<StoredRefactorAction> {
    let mut actions: Vec<StoredRefactorAction> = Vec::new();

    // A stack has to exist before a resource can be moved into it, so the stacks the refactor
    // creates come first — in the order the refactor named them.
    for stack in covered.iter().filter(|stack| stack.stack_id.is_none()) {
        actions.push(StoredRefactorAction::creating_stack(&stack.stack_name));
    }

    // The moves the CUSTOMER named, in the order the request named them.
    let named: Vec<RefactorResourceMapping> = mappings
        .iter()
        .map(|mapping| RefactorResourceMapping {
            source: reported_location(covered, &mapping.source),
            destination: reported_location(covered, &mapping.destination),
        })
        .collect();
    for (mapping, named) in mappings.iter().zip(&named) {
        let held = resource_of(covered, &mapping.source);
        actions.push(StoredRefactorAction::moving_resource(
            named.clone(),
            detection::MANUAL,
            held,
        ));
    }

    // Then the moves the plan works out itself from the final states the request submitted,
    // up to what one plan may record ([`MAX_ACTIONS_PER_REFACTOR`]).
    let budget = MAX_ACTIONS_PER_REFACTOR.saturating_sub(actions.len());
    for mapping in detected_moves(&named, covered, final_states, budget) {
        let held = resource_of(covered, &mapping.source);
        actions.push(StoredRefactorAction::moving_resource(
            mapping,
            detection::AUTO,
            held,
        ));
    }

    // The bound holds for the WHOLE plan, not only for the moves it worked out: the stacks a
    // refactor may name and the moves it may name are each already held to their own bound by
    // the submitting operation, so this cannot drop one of those — it is what keeps the
    // statement "a plan records at most this many actions" true of every action.
    actions.truncate(MAX_ACTIONS_PER_REFACTOR);
    actions
}

/// The stack of `covered` a location names, by either spelling `StackName` admits: the name
/// the request used, or the stack's own id.
///
/// `covered` is keyed by the spelling the REQUEST used, while an action reports the stack's id
/// ([`reported_location`]), so both have to resolve to the one stack — otherwise an action
/// built from a reported location could not say which instance it acts on.
fn stack_of<'a>(covered: &'a [CoveredStack], stack_name: &str) -> Option<&'a CoveredStack> {
    covered.iter().find(|stack| {
        stack.stack_name == stack_name || stack.stack_id.as_deref() == Some(stack_name)
    })
}

/// One end of a move, as the ACTIONS report it: the stack's own unique id where this Region
/// holds the stack, and the name the request used where it does not.
///
/// `ResourceLocation$StackName` is "the name associated with the stack", and CloudFormation
/// accepts that member as either a stack's name or its id — so two requests naming one stack
/// two ways describe the SAME move, and the stack a move acts on is the one the refactor also
/// reports the id of (`StackIds`: "the unique ID for each stack"). Reporting that id is what
/// makes the two spellings one answer; a stack the refactor is still to CREATE has no id yet,
/// so it keeps the name the request gave it.
fn reported_location(
    covered: &[CoveredStack],
    location: &RefactorResourceLocation,
) -> RefactorResourceLocation {
    let stack_id = stack_of(covered, &location.stack_name).and_then(|stack| stack.stack_id.clone());
    RefactorResourceLocation {
        stack_name: stack_id.unwrap_or_else(|| location.stack_name.clone()),
        logical_resource_id: location.logical_resource_id.clone(),
    }
}

/// The resource one end of a move addresses, as the stack that holds it holds it right now;
/// `None` where this Region holds no such stack or the stack holds no such logical id.
fn resource_of<'a>(
    covered: &'a [CoveredStack],
    location: &RefactorResourceLocation,
) -> Option<&'a CoveredResource> {
    stack_of(covered, &location.stack_name)
        .and_then(|stack| stack.resource(&location.logical_resource_id))
}

/// The moves CloudFormation "figured out on its own", from the final states the request
/// submitted and the resources the stacks hold, at most `budget` of them.
///
/// A resource a stack HOLDS that the final state submitted for that stack no longer declares
/// is a resource leaving it; a resource a final state declares that its stack does not hold
/// yet is a place one arrives at. A leaving resource is matched with the first arrival of the
/// same TYPE that is still unclaimed, which is the move that leaves both submitted templates
/// satisfied — and a definition that submitted no template declares nothing, so it offers
/// neither end.
///
/// Neither end of a mapping the request already named takes part: that move is the customer's,
/// and the resource it moves is accounted for by it. `named` therefore carries those mappings
/// as [`reported_location`] resolves them, which is the form every end here is compared in —
/// so a request that names one stack by its name in a definition and by its id in a mapping
/// still describes ONE move.
///
/// Both ends are INDEXED before they are matched — the named mappings as a set, the arrivals
/// by resource type — so the work is proportional to what the request submitted rather than to
/// the product of its two ends: both are caller-sized, and this runs while the store is held.
fn detected_moves(
    named: &[RefactorResourceMapping],
    covered: &[CoveredStack],
    final_states: &[FinalState],
    budget: usize,
) -> Vec<RefactorResourceMapping> {
    if budget == 0 {
        return Vec::new();
    }
    let named_ends = |end: fn(&RefactorResourceMapping) -> &RefactorResourceLocation| {
        named
            .iter()
            .map(|mapping| {
                let location = end(mapping);
                (
                    location.stack_name.as_str(),
                    location.logical_resource_id.as_str(),
                )
            })
            .collect::<BTreeSet<(&str, &str)>>()
    };
    let named_sources = named_ends(|mapping| &mapping.source);
    let named_destinations = named_ends(|mapping| &mapping.destination);
    let is_named = |ends: &BTreeSet<(&str, &str)>, location: &RefactorResourceLocation| {
        ends.contains(&(
            location.stack_name.as_str(),
            location.logical_resource_id.as_str(),
        ))
    };

    // Where a resource is leaving from, and where one can arrive, in the order the request's
    // own definitions and templates spell them. The arrivals are kept BY TYPE, since the type
    // is what decides which arrival takes a leaving resource.
    let mut leaving: Vec<(RefactorResourceLocation, String)> = Vec::new();
    let mut arriving: BTreeMap<String, VecDeque<RefactorResourceLocation>> = BTreeMap::new();
    for state in final_states {
        let Some(declared) = state.declared.as_ref() else {
            continue;
        };
        let declared_ids: BTreeSet<&str> = declared
            .iter()
            .map(|resource| resource.logical_id.as_str())
            .collect();
        let holder = stack_of(covered, &state.stack_name);
        for held in holder.map(|stack| stack.resources.as_slice()).unwrap_or(&[]) {
            if declared_ids.contains(held.logical_id.as_str()) {
                continue;
            }
            let location = reported_location(
                covered,
                &RefactorResourceLocation {
                    stack_name: state.stack_name.clone(),
                    logical_resource_id: held.logical_id.clone(),
                },
            );
            if !is_named(&named_sources, &location) {
                leaving.push((location, held.resource_type.clone()));
            }
        }
        for resource in declared {
            if holder.is_some_and(|stack| stack.holds(&resource.logical_id)) {
                continue;
            }
            let location = reported_location(
                covered,
                &RefactorResourceLocation {
                    stack_name: state.stack_name.clone(),
                    logical_resource_id: resource.logical_id.clone(),
                },
            );
            if !is_named(&named_destinations, &location) {
                arriving
                    .entry(resource.resource_type.clone())
                    .or_default()
                    .push_back(location);
            }
        }
    }

    let mut detected: Vec<RefactorResourceMapping> = Vec::new();
    for (source, resource_type) in leaving {
        if detected.len() == budget {
            break;
        }
        let Some(destination) = arriving
            .get_mut(&resource_type)
            .and_then(VecDeque::pop_front)
        else {
            // Nothing the submitted templates declare takes this resource, so there is no
            // move to report for it: the plan says what it can see, and invents nothing.
            continue;
        };
        detected.push(RefactorResourceMapping {
            source,
            destination,
        });
    }
    detected
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stack of the store, with the resources it holds.
    fn stack(stack_name: &str, stack_id: Option<&str>, held: &[(&str, &str, &str)]) -> CoveredStack {
        CoveredStack {
            stack_name: stack_name.to_string(),
            stack_id: stack_id.map(str::to_string),
            resources: held
                .iter()
                .map(|(logical_id, resource_type, physical)| CoveredResource {
                    logical_id: (*logical_id).to_string(),
                    resource_type: (*resource_type).to_string(),
                    physical_resource_id: (*physical).to_string(),
                })
                .collect(),
        }
    }

    /// One final state, as a definition that submitted a template spells it.
    fn final_state(stack_name: &str, declared: &[(&str, &str)]) -> FinalState {
        FinalState {
            stack_name: stack_name.to_string(),
            declared: Some(
                declared
                    .iter()
                    .map(|(logical_id, resource_type)| DeclaredFinalResource {
                        logical_id: (*logical_id).to_string(),
                        resource_type: (*resource_type).to_string(),
                    })
                    .collect(),
            ),
        }
    }

    fn mapping(source: (&str, &str), destination: (&str, &str)) -> RefactorResourceMapping {
        RefactorResourceMapping {
            source: RefactorResourceLocation {
                stack_name: source.0.to_string(),
                logical_resource_id: source.1.to_string(),
            },
            destination: RefactorResourceLocation {
                stack_name: destination.0.to_string(),
                logical_resource_id: destination.1.to_string(),
            },
        }
    }

    /// RULE: the plan reports every stack it must create before every move, and a move the
    /// request named carries the mapping it named with the physical instance it acts on.
    #[test]
    fn a_stack_is_created_before_a_resource_is_moved_into_it() {
        let covered = vec![
            stack(
                "holder",
                Some("arn:aws:cloudformation:us-east-1:000000000000:stack/holder/1"),
                &[("Topic", "AWS::SNS::Topic", "arn:aws:sns:us-east-1:000000000000:Topic")],
            ),
            stack("fresh", None, &[]),
        ];
        let mappings = vec![mapping(("holder", "Topic"), ("fresh", "Topic"))];
        let actions = plan_actions(&mappings, &covered, &[]);

        let shape: Vec<(&str, &str)> = actions
            .iter()
            .map(|reported| (reported.action.as_str(), reported.entity.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (action::CREATE, entity::STACK),
                (action::MOVE, entity::RESOURCE)
            ],
            "case \"a move into a stack that does not exist yet\": expected the stack to be \
             created first, got {actions:?}"
        );
        assert_eq!(
            actions[0].physical_resource_id, None,
            "case \"the stack to create\": expected no physical instance, got {:?}",
            actions[0]
        );
        assert_eq!(
            actions[1].physical_resource_id.as_deref(),
            Some("arn:aws:sns:us-east-1:000000000000:Topic"),
            "case \"the move\": expected the instance it acts on, got {:?}",
            actions[1]
        );
        assert_eq!(
            actions[1].detection.as_deref(),
            Some(detection::MANUAL),
            "case \"a move the request named\": expected it to be the customer's, got {:?}",
            actions[1]
        );
        assert_eq!(
            actions[1].resource_mapping.as_ref().map(|mapped| (
                mapped.source.stack_name.clone(),
                mapped.source.logical_resource_id.clone(),
                mapped.destination.stack_name.clone(),
                mapped.destination.logical_resource_id.clone()
            )),
            Some((
                // The stack that HOLDS the resource is reported by its own id, while the stack
                // the refactor is still to create has none and keeps the name the request
                // gave it.
                "arn:aws:cloudformation:us-east-1:000000000000:stack/holder/1".to_string(),
                "Topic".to_string(),
                "fresh".to_string(),
                "Topic".to_string()
            )),
            "case \"a move the request named\": expected the mapping it named, got {:?}",
            actions[1]
        );
    }

    /// RULE: a resource a stack holds that its submitted final state no longer declares moves
    /// to the stack whose final state newly declares one of that type, and that move is
    /// CloudFormation's own.
    #[test]
    fn a_move_the_submitted_templates_imply_is_detected() {
        let covered = vec![
            stack(
                "source",
                Some("arn:aws:cloudformation:us-east-1:000000000000:stack/source/1"),
                &[
                    ("Topic", "AWS::SNS::Topic", "arn:aws:sns:us-east-1:000000000000:Topic"),
                    ("Bucket", "AWS::S3::Bucket", "a-bucket"),
                ],
            ),
            stack(
                "target",
                Some("arn:aws:cloudformation:us-east-1:000000000000:stack/target/2"),
                &[],
            ),
        ];
        let final_states = vec![
            final_state("source", &[("Bucket", "AWS::S3::Bucket")]),
            final_state("target", &[("Renamed", "AWS::SNS::Topic")]),
        ];
        let actions = plan_actions(&[], &covered, &final_states);

        assert_eq!(
            actions.len(),
            1,
            "case \"one resource left its stack's final state\": expected one move, got {actions:?}"
        );
        let detected = &actions[0];
        assert_eq!(
            (
                detected.action.as_str(),
                detected.entity.as_str(),
                detected.detection.as_deref()
            ),
            (action::MOVE, entity::RESOURCE, Some(detection::AUTO)),
            "case \"a move nobody named\": expected CloudFormation's own move, got {detected:?}"
        );
        let mapped = detected
            .resource_mapping
            .as_ref()
            .expect("a move carries the mapping it carries out");
        assert_eq!(
            (
                mapped.source.stack_name.as_str(),
                mapped.source.logical_resource_id.as_str(),
                mapped.destination.stack_name.as_str(),
                mapped.destination.logical_resource_id.as_str()
            ),
            (
                "arn:aws:cloudformation:us-east-1:000000000000:stack/source/1",
                "Topic",
                "arn:aws:cloudformation:us-east-1:000000000000:stack/target/2",
                "Renamed"
            ),
            "case \"a move nobody named\": expected the resource of that type to move, got \
             {mapped:?}"
        );
    }

    /// RULE: nothing is inferred over a mapping the request named — the resource it moves is
    /// accounted for by the customer's own mapping, so it is reported once, whichever of the
    /// two spellings `StackName` admits each part of the request used for a stack.
    #[test]
    fn a_named_mapping_leaves_nothing_to_detect() {
        let source_id = "arn:aws:cloudformation:us-east-1:000000000000:stack/named-source/1";
        let target_id = "arn:aws:cloudformation:us-east-1:000000000000:stack/named-target/2";
        let covered = |source_spelling: &str, target_spelling: &str| {
            vec![
                stack(
                    source_spelling,
                    Some(source_id),
                    &[(
                        "Topic",
                        "AWS::SNS::Topic",
                        "arn:aws:sns:us-east-1:000000000000:Topic",
                    )],
                ),
                stack(target_spelling, Some(target_id), &[]),
            ]
        };
        // The definitions always name the stacks the way the request's own `StackDefinitions`
        // did; the mapping names them by NAME in one row and by ID in the other, which is a
        // stack id a client may put in a `ResourceLocation$StackName` just as well.
        let rows: Vec<(&str, Vec<RefactorResourceMapping>)> = vec![
            (
                "a mapping naming the stacks by name",
                vec![mapping(("named-source", "Topic"), ("named-target", "Topic"))],
            ),
            (
                "a mapping naming the same stacks by id",
                vec![mapping((source_id, "Topic"), (target_id, "Topic"))],
            ),
        ];
        for (case, mappings) in rows {
            let final_states = vec![
                final_state("named-source", &[]),
                final_state("named-target", &[("Topic", "AWS::SNS::Topic")]),
            ];
            let actions = plan_actions(&mappings, &covered("named-source", "named-target"), &final_states);

            assert_eq!(
                actions.len(),
                1,
                "case {case:?}: expected exactly the move the request named, got {actions:?}"
            );
            assert_eq!(
                actions[0].detection.as_deref(),
                Some(detection::MANUAL),
                "case {case:?}: expected the customer's mapping, got {:?}",
                actions[0]
            );
            let mapped = actions[0]
                .resource_mapping
                .as_ref()
                .expect("a move carries the mapping it carries out");
            assert_eq!(
                (
                    mapped.source.stack_name.as_str(),
                    mapped.destination.stack_name.as_str()
                ),
                (source_id, target_id),
                "case {case:?}: expected each end to name the stack by its own id, got \
                 {mapped:?}"
            );
            assert_eq!(
                actions[0].physical_resource_id.as_deref(),
                Some("arn:aws:sns:us-east-1:000000000000:Topic"),
                "case {case:?}: expected the instance the move acts on, got {:?}",
                actions[0]
            );
        }
    }

    /// RULE: a plan records at most [`MAX_ACTIONS_PER_REFACTOR`] actions, however many moves
    /// the templates it was submitted would imply.
    #[test]
    fn a_plan_records_no_more_actions_than_one_record_may_carry() {
        let implied = MAX_ACTIONS_PER_REFACTOR + 5;
        let covered = vec![
            CoveredStack {
                stack_name: "crowded".to_string(),
                stack_id: Some(
                    "arn:aws:cloudformation:us-east-1:000000000000:stack/crowded/1".to_string(),
                ),
                resources: (0..implied)
                    .map(|index| CoveredResource {
                        logical_id: format!("Topic{index}"),
                        resource_type: "AWS::SNS::Topic".to_string(),
                        physical_resource_id: format!(
                            "arn:aws:sns:us-east-1:000000000000:Topic{index}"
                        ),
                    })
                    .collect(),
            },
            stack(
                "roomy",
                Some("arn:aws:cloudformation:us-east-1:000000000000:stack/roomy/2"),
                &[],
            ),
        ];
        let final_states = vec![
            FinalState {
                stack_name: "crowded".to_string(),
                declared: Some(Vec::new()),
            },
            FinalState {
                stack_name: "roomy".to_string(),
                declared: Some(
                    (0..implied)
                        .map(|index| DeclaredFinalResource {
                            logical_id: format!("Moved{index}"),
                            resource_type: "AWS::SNS::Topic".to_string(),
                        })
                        .collect(),
                ),
            },
        ];
        let actions = plan_actions(&[], &covered, &final_states);
        assert_eq!(
            actions.len(),
            MAX_ACTIONS_PER_REFACTOR,
            "case \"more moves than one record may carry\": expected the plan to stop at \
             {MAX_ACTIONS_PER_REFACTOR} actions, got {}",
            actions.len()
        );
    }
}
