// SPDX-License-Identifier: CC0-1.0

//! Re-pointing a deployment's stand-ins at the identifiers the owning services MINTED.
//!
//! A declaration is resolved before any of its resources exist, so a reference to a resource
//! whose identity its owning service assigns resolves to the STAND-IN the naming rules
//! computed ([`crate::state::template`]). As [`super::reconcile`] walks the deployment, each
//! create answers the identifier that service really assigned, and this module is what brings
//! the rest of the deployment into step with it: the properties of a resource not created yet
//! ([`repoint_references`]) and that resource's own identity where its name is composed out of
//! such an identifier ([`repoint_identity`]).
//!
//! It lives in its own file beside the walk that drives it so neither grows past the length a
//! change to this repository may touch; [`Minted`] is the pair the walk collects, [`pairs_for`]
//! decides WHICH of those pairs one declaration may be re-pointed with, and [`retokenised`] is
//! the one place a stand-in may be matched inside a longer value.

use super::*;

/// How deep [`repoint_references`] walks a property document.
///
/// The document's nesting is the REQUESTER's, and a recursive walk bounded by its input
/// exhausts the native stack — which aborts the process rather than failing one stack. Past
/// this depth the values are left exactly as they are: a reference to another resource is a
/// property of it, never a hundred levels inside one.
pub(super) const MAX_REPOINTED_NESTING: usize = 64;

/// The minted pairs that may re-point the declaration of `logical_id`.
///
/// The rule: a value another declaration STATES is that declaration's own, so adopting an
/// identifier an owning service minted may only change the values a REFERENCE to that
/// resource put there — and a declaration outside the reference relation
/// ([`crate::state::template::direct_dependencies`]) reads nothing of the minted resource, so
/// every value of its own stands exactly as written. (That relation is what the deployment is
/// ORDERED by, so it counts a `DependsOn` beside the `Ref`, the `Fn::GetAtt` and the `Fn::Sub`
/// variable a declaration really reads the resource through: a declaration that only waits for
/// the resource is inside it too, which is as far as one relation can tell the two apart.)
/// That holds whatever the stand-in IS: a
/// stand-in an owning service's identifier is substituted into is often a value a template can
/// compose for itself out of public parts — the account id, the Region, a name the declaration
/// states — and two declarations that write the same string are indistinguishable once both are
/// resolved, so whether the declaration reads the resource at all is the only thing that tells
/// a reference from a value of a declaration's own. Without it, a template that writes such a
/// string out itself (a container image on the registry host of this account and Region) is
/// deployed carrying an address it never asked for.
///
/// HOW FAR along that relation a pair reaches follows from what the stand-in is, the same
/// question [`Minted::composed`] answers for where inside a value it may be matched:
///
/// * a stand-in CloudFormation itself COMPUTED, or an attribute value the resource's own
///   declaration does not state, is no value another declaration writes for itself, and a
///   reference resolves to a value that was ITSELF composed — a metric filter naming the log
///   group of the same stack carries the group's name, which was composed out of an identifier
///   the API's own service minted. Such a pair therefore reaches every declaration that reads
///   the resource, directly or through another resource of the same stack
///   (`super::needed_by`, the one walk of that relation this module tree keeps);
/// * a stand-in that is the NAME the declaration asked for is a value ANY declaration may
///   state — the stack name a bucket is named after, which the user pool of the same stack was
///   also created under — so it reaches only the declarations that name that resource
///   THEMSELVES. Otherwise such a bucket's policy is written for the identifier Amazon Cognito
///   minted for the pool, which is a bucket that does not exist.
pub(super) fn pairs_for(
    logical_id: &str,
    dependencies: &Dependencies,
    minted: &[Minted],
) -> Vec<Minted> {
    if minted.is_empty() {
        return Vec::new();
    }
    let named = dependencies.get(logical_id);
    // The closure is walked only where a pair asks for it, which most walks carry none of; a
    // resource never reads ITSELF, whatever a cycle in the relation says.
    let mut reads: BTreeSet<String> = BTreeSet::new();
    if minted.iter().any(|minted| minted.composed) {
        reads = super::needed_by(dependencies, logical_id);
        reads.remove(logical_id);
    }
    minted
        .iter()
        .filter(|minted| match minted.composed {
            true => reads.contains(&minted.of),
            false => named.is_some_and(|named| named.contains(&minted.of)),
        })
        .cloned()
        .collect()
}

/// Re-point every reference inside one property document from the stand-in identifier a
/// resource was given before its create to the identifier its owning service MINTED.
///
/// `minted` is the set this document may be re-pointed with ([`pairs_for`]), not every pair
/// the walk has collected: a declaration that reads nothing of a resource states its own
/// values, whatever another resource's stand-in happens to be.
///
/// The rule: a reference to a resource resolves to the identifier that resource really has,
/// and for a type whose identity its owning service assigns
/// ([`crate::state::template::identity_is_service_assigned`]) that identifier only exists once
/// the create has answered. A declaration is resolved before any of them have, so a property
/// naming such a resource (`ResolverRuleId: !Ref Rule`) holds the stand-in the naming rules
/// computed; `minted` pairs each stand-in with what its service answered, and a property
/// whose value IS one of those stand-ins becomes the minted identifier.
///
/// The same holds for an ATTRIBUTE the owning service reports differently from the computed
/// one (`KmsMasterKeyId: !GetAtt [Key, Arn]`, where the ARN carries the key id KMS minted):
/// [`super::reconcile`] pairs those too, so a property reaches its own service as the value the
/// resource it names really publishes.
///
/// Matched WHOLE value — or, where the stand-in is an identifier CloudFormation computed
/// rather than a name the declaration asked for ([`Minted::composed`]), as one whole TOKEN
/// of a longer value ([`retokenised`]), which is how a template composes an identifier into
/// one of its own strings (`integrations/<IntegrationId>`, or the ARN an `Fn::Sub` builds
/// around a `Ref`). Never as any shorter run of characters: a stand-in is often the NAME the
/// declaration asked for, so a longer value that merely contains one (an association named
/// `<rule name>-assoc`) is a value of its own and is left exactly as written.
///
/// Called by [`super::reconcile`] for the identifiers minted in the walk it is driving. A
/// resource is never re-pointed with its OWN pair — it cannot reference itself, and for a type
/// whose stand-in is the name it declares that pair would rewrite that very property. The
/// identifiers a stack ALREADY has are not substituted at all:
/// [`crate::state::StoredStack::deploy_template`] resolves the declarations against them
/// instead.
pub(super) fn repoint_references(properties: &mut Value, minted: &[Minted]) {
    repoint(properties, minted, 0);
}

/// Re-point the IDENTITY of the resource a deployment is about to create: the physical id the
/// naming rules computed for it, and the attributes they read off that id.
///
/// The rule: a resource is created in its owning service under the name its declaration
/// resolves to once the resources it depends on carry the identifiers THEIR services minted.
/// A declaration that composes its name out of such an identifier
/// (`LogGroupName: !Join ['', ['/aws/<service>/<kind>/', !GetAtt Api.ApiId]]`) was resolved
/// before any of them had answered, so the name — and the id the naming rules took from it,
/// and the ARN they built on it — carry the stand-in. [`repoint_references`] has just brought
/// the declaration itself into step with what those services really assigned; this brings the
/// identity along, so the create addresses the resource under the name the declaration now
/// states, `Ref` resolves to that name, and a reader that builds the same name from the same
/// dependency reaches the resource rather than a name nothing holds.
///
/// A name no minted identifier is part of is left exactly as it is, which is every resource
/// whose declaration names it outright.
///
/// A type whose primary identifier is COMPOSITE is brought along the same way, from the
/// declaration this has just re-pointed: every part of such an identifier is a value the
/// declaration states ([`crate::state::template::composite_identity`]), so a part naming a
/// resource whose identity its owning service mints was composed out of that resource's
/// stand-in. Recomposing it is what makes the binding's `Ref` — and the record the stack's own
/// delete addresses the binding by — carry the identifiers its parts really have.
pub(super) fn repoint_identity(resource: &mut StoredResource, minted: &[Minted]) {
    if minted.is_empty() {
        return;
    }
    for value in resource.attributes.values_mut() {
        if let Some(repointed) = repointed_text(value, minted) {
            *value = repointed;
        }
    }
    if let Some(repointed) = repointed_text(&resource.physical_resource_id, minted) {
        // Through the one step that takes an identifier over, so the `Id`/`<Type>Id`
        // attributes and the id a `Ref` resolves to cannot drift apart.
        crate::state::template::adopt_identity(resource, repointed);
    }
    // The same step for a composite identifier, recomposed out of the re-pointed document by
    // the one function that computes one ([`crate::state::template::resolve_properties`] runs
    // it before any create has answered).
    crate::state::template::adopt_composite_identity(resource);
}

/// One TEXT value re-pointed the way a property document's values are
/// ([`repoint_references`]), or `None` for a value no pair changes.
pub(super) fn repointed_text(value: &str, minted: &[Minted]) -> Option<String> {
    let mut carried = Value::String(value.to_string());
    repoint(&mut carried, minted, 0);
    match carried {
        Value::String(repointed) if repointed != value => Some(repointed),
        _ => None,
    }
}

/// Whether a resource's own declaration STATES `value` — as a property's value, or as one
/// inside a list or an object the properties carry.
///
/// What it decides is whether a value the owning service reported differently from the
/// computed one may be matched inside a longer value ([`Minted::composed`]): a value the
/// template itself wrote is a value of its own wherever it turns up, while one CloudFormation
/// derived is in another declaration's longer string only because a reference to this resource
/// put it there.
///
/// Walked to the same depth a re-pointing walks ([`MAX_REPOINTED_NESTING`]), for the same
/// reason: the nesting is the requester's.
pub(super) fn states_value(properties: &Value, value: &str, depth: usize) -> bool {
    if depth >= MAX_REPOINTED_NESTING {
        return false;
    }
    match properties {
        Value::Array(items) => items
            .iter()
            .any(|item| states_value(item, value, depth + 1)),
        Value::Object(members) => members
            .values()
            .any(|member| states_value(member, value, depth + 1)),
        // A scalar is read as the one text an API member takes it as, so a number a template
        // wrote as a number is the value a service reports as text.
        scalar => crate::state::scalar_text(scalar).is_some_and(|text| text == value),
    }
}

/// One identifier an owning service MINTED in a deployment walk, against the stand-in the
/// naming rules had computed for the same resource ([`repoint_references`]).
#[derive(Debug, Clone)]
pub(super) struct Minted {
    /// The LOGICAL ID of the resource whose identifier this is, which is what says whose
    /// declarations may be re-pointed with it ([`pairs_for`]).
    pub(super) of: String,
    /// What a reference to the resource was resolved to before its create.
    pub(super) stand_in: String,
    /// What that reference has to carry now.
    pub(super) assigned: String,
    /// Whether the stand-in may also be matched INSIDE a longer value, as one whole token
    /// of it ([`retokenised`]).
    ///
    /// True only where the stand-in is an identifier CloudFormation COMPUTED rather than
    /// the name the declaration asked for: such a value is not one a template composed out
    /// of its own strings, so a longer value carrying it carries it because a reference to
    /// this resource put it there. That is how an API Gateway v2 route names the
    /// integration it is wired to — `Target: !Join ['/', ['integrations', !Ref
    /// Integration]]`, which is the `integrations/<IntegrationId>` form that service
    /// accepts — and how a template builds the ARN of a resource it declares, with the
    /// reference inside a longer string (`!Sub 'arn:aws:execute-api:${AWS::Region}:\
    /// ${AWS::AccountId}:${Api}/*/*'`). Without this, such a route would be created
    /// pointing at an integration id nothing minted, and such a grant made for an api id
    /// nothing minted.
    ///
    /// The same question for an ATTRIBUTE's pair is whether the resource's own declaration
    /// STATES the computed value ([`states_value`]): one it does not state is a value
    /// CloudFormation derived, so another declaration carries it inside a longer value only
    /// because an `Fn::GetAtt` of this resource put it there — the api id a log-group name is
    /// built around.
    pub(super) composed: bool,
}

/// [`repoint_references`] over one value, `depth` levels into the document.
pub(super) fn repoint(properties: &mut Value, minted: &[Minted], depth: usize) {
    if minted.is_empty() || depth >= MAX_REPOINTED_NESTING {
        return;
    }
    match properties {
        Value::String(text) => {
            if let Some(minted) = minted
                .iter()
                .find(|minted| minted.stand_in.as_str() == text.as_str())
            {
                *text = minted.assigned.clone();
                return;
            }
            for minted in minted.iter().filter(|minted| minted.composed) {
                if let Some(repointed) = retokenised(text, &minted.stand_in, &minted.assigned) {
                    *text = repointed;
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                repoint(item, minted, depth + 1);
            }
        }
        Value::Object(members) => {
            for value in members.values_mut() {
                repoint(value, minted, depth + 1);
            }
        }
        _ => {}
    }
}

/// Whether `character` can be part of one IDENTIFIER: an ASCII letter or digit, a hyphen or
/// an underscore, which is what the identifiers AWS services assign are made of.
///
/// It is the boundary [`retokenised`] matches a stand-in at: a stand-in whose neighbour in a
/// longer value is one of these is part of a longer token of that value, so it is not a
/// reference to the resource and is left exactly as written.
pub(super) fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

/// `text` with every WHOLE-TOKEN occurrence of `stand_in` replaced by `assigned`, or `None`
/// for a text that carries none.
///
/// This is how a reference COMPOSED INTO a longer string reaches the identifier its resource
/// really has: a template builds the ARN of a resource it declares out of its own text and
/// that reference (`!Sub 'arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${Api}/*/*'`),
/// so the identifier sits inside a longer value delimited by whatever the template wrote
/// around it — a colon, a slash, a quote, a space — rather than being the whole value or one
/// whole slash-separated segment of it.
///
/// A token is bounded by any character an identifier cannot carry
/// ([`is_identifier_character`]), so a stand-in that merely begins or ends a longer
/// identifier is left alone. Only a stand-in CloudFormation itself computed is matched this
/// way ([`Minted::composed`]): a value a declaration asked for is a value of its own
/// wherever it appears.
pub(super) fn retokenised(text: &str, stand_in: &str, assigned: &str) -> Option<String> {
    if stand_in.is_empty() {
        return None;
    }
    let mut repointed = String::new();
    let mut cursor = 0;
    let mut replaced = false;
    while let Some(offset) = text[cursor..].find(stand_in) {
        let at = cursor + offset;
        let end = at + stand_in.len();
        let bounded = text[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_character(character))
            && text[end..]
                .chars()
                .next()
                .is_none_or(|character| !is_identifier_character(character));
        repointed.push_str(&text[cursor..at]);
        if bounded {
            repointed.push_str(assigned);
            replaced = true;
        } else {
            repointed.push_str(stand_in);
        }
        cursor = end;
    }
    if !replaced {
        return None;
    }
    repointed.push_str(&text[cursor..]);
    Some(repointed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One resource as a deployment holds it just before its create: the identity the naming
    /// rules computed, and the declaration as it was resolved.
    fn deployed(resource_type: &str, physical_resource_id: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "ARes".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: physical_resource_id.to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// One minted pair, as the walk collects it for a resource whose identity its owning
    /// service assigned: the stand-in that resource's `Ref` had resolved to, and what that
    /// service answered. `composed` is false here, which is a stand-in that IS the name the
    /// declaration asked for — the case a whole-token match does not cover.
    fn minted(of: &str, stand_in: &str, assigned: &str) -> Minted {
        Minted {
            of: of.to_string(),
            stand_in: stand_in.to_string(),
            assigned: assigned.to_string(),
            composed: false,
        }
    }

    /// A type whose primary identifier is COMPOSITE is identified by the parts its declaration
    /// states, so the identity has to be the one those parts really carry: a part naming a
    /// resource whose identity its owning service minted is re-pointed, and the identity is
    /// recomposed around it — while a binding every part of which was already final keeps
    /// exactly the identity it had.
    ///
    /// The rows span the two directions of that rule and the types whose identity is a pair, a
    /// triple and a quadruple, so the recomposition is asserted over the forms
    /// [`crate::state::template::composite_identity_properties`] publishes rather than over one
    /// of them.
    #[test]
    fn a_composite_identity_is_recomposed_out_of_the_identifiers_its_parts_really_carry() {
        struct Row {
            case: &'static str,
            resource_type: &'static str,
            properties: Value,
            /// The identity the naming rules had computed before any create answered.
            computed: &'static str,
            /// The pair the walk minted for the resource this binding names, if any.
            minted: Vec<Minted>,
            expected: &'static str,
        }
        let rows = vec![
            // A session mapping one part of which names a Studio whose `es-…` id Amazon EMR
            // minted: the id the stack publishes has to carry that id, not the Studio's name.
            Row {
                case: "a triple whose first part is a minted identifier",
                resource_type:
                    crate::state::provisioner::emr_resources::EMR_STUDIO_SESSION_MAPPING,
                properties: serde_json::json!({
                    "StudioId": "es-0000000000001",
                    "IdentityType": "USER",
                    "IdentityName": "analyst",
                }),
                computed: "a-studio|USER|analyst",
                minted: vec![minted("Studio", "a-studio", "es-0000000000001")],
                expected: "es-0000000000001|USER|analyst",
            },
            // The same type with nothing minted for it at all: the parts the declaration
            // states are the identity.
            Row {
                case: "a triple with no minted part",
                resource_type:
                    crate::state::provisioner::emr_resources::EMR_STUDIO_SESSION_MAPPING,
                properties: serde_json::json!({
                    "StudioId": "es-0000000000002",
                    "IdentityType": "GROUP",
                    "IdentityName": "analysts",
                }),
                computed: "es-0000000000002|GROUP|analysts",
                minted: vec![minted("Other", "a-name", "another-name")],
                expected: "es-0000000000002|GROUP|analysts",
            },
            // A PAIR whose two parts are values the declaration itself states: the identity
            // stands exactly as it was computed.
            Row {
                case: "a pair every part of which was already final",
                resource_type: "AWS::EKS::AccessEntry",
                properties: serde_json::json!({
                    "ClusterName": "a-cluster",
                    "PrincipalArn": "arn:aws:iam::000000000000:role/a-role",
                }),
                computed: "a-cluster|arn:aws:iam::000000000000:role/a-role",
                minted: vec![minted("Other", "a-name", "another-name")],
                expected: "a-cluster|arn:aws:iam::000000000000:role/a-role",
            },
            // A QUADRUPLE, the longest form the catalogue publishes, likewise final.
            Row {
                case: "a quadruple every part of which was already final",
                resource_type: "AWS::Glue::TableOptimizer",
                properties: serde_json::json!({
                    "CatalogId": "000000000000",
                    "DatabaseName": "a-database",
                    "TableName": "a-table",
                    "Type": "compaction",
                }),
                computed: "000000000000|a-database|a-table|compaction",
                minted: vec![minted("Other", "a-name", "another-name")],
                expected: "000000000000|a-database|a-table|compaction",
            },
            // And a type with NO composite identifier keeps the name it was given, which is
            // the re-pointing this function did before composite identities took part in it.
            Row {
                case: "a type whose identity is its own name, re-pointed",
                resource_type: "AWS::Logs::LogGroup",
                properties: serde_json::json!({ "LogGroupName": "/aws/vendedlogs/es-0000000000001" }),
                computed: "a-studio",
                minted: vec![minted("Studio", "a-studio", "es-0000000000001")],
                expected: "es-0000000000001",
            },
        ];
        for row in rows {
            let case = row.case;
            let mut resource = deployed(row.resource_type, row.computed, row.properties.clone());
            repoint_identity(&mut resource, &row.minted);
            assert_eq!(
                resource.physical_resource_id, row.expected,
                "case {case:?}: expected the identity {:?}, got {:?}",
                row.expected, resource.physical_resource_id
            );
        }
    }

    /// WHICH declarations a minted pair may re-point ([`pairs_for`]): the ones that READ the
    /// resource it belongs to — directly for a stand-in that is the NAME the declaration asked
    /// for, and through another resource of the same stack as well for one CloudFormation
    /// itself computed.
    ///
    /// The rows span the two kinds of stand-in against the two directions of the rule and the
    /// chain in between: a declaration that names the resource itself, one that names a
    /// resource which names it, one further link along such a chain, and one that names
    /// neither — whose own values are its own however much they look like another resource's
    /// stand-in.
    #[test]
    fn only_a_declaration_that_reads_the_minted_resource_is_re_pointed_with_its_pair() {
        struct Row {
            case: &'static str,
            /// The declaration asking to be re-pointed.
            logical_id: &'static str,
            /// Whether the pair's stand-in is one CloudFormation computed
            /// ([`Minted::composed`]) rather than the name the declaration asked for.
            composed: bool,
            /// Whether the pair applies to that declaration.
            applies: bool,
        }
        // Api is read by Group, Group by Filter, Filter by Alarm; Image reads nothing at all.
        let dependencies: Dependencies = [
            ("Group", ["Api"]),
            ("Filter", ["Group"]),
            ("Alarm", ["Filter"]),
        ]
        .into_iter()
        .map(|(reader, reads)| {
            (
                reader.to_string(),
                reads.iter().map(ToString::to_string).collect(),
            )
        })
        .collect();
        let rows = [
            Row {
                case: "a computed stand-in, in the declaration that names the resource",
                logical_id: "Group",
                composed: true,
                applies: true,
            },
            Row {
                case: "a declared name, in the declaration that names the resource",
                logical_id: "Group",
                composed: false,
                applies: true,
            },
            Row {
                case: "a computed stand-in, one link along the chain",
                logical_id: "Filter",
                composed: true,
                applies: true,
            },
            Row {
                case: "a declared name, one link along the chain",
                logical_id: "Filter",
                composed: false,
                applies: false,
            },
            Row {
                case: "a computed stand-in, two links along the chain",
                logical_id: "Alarm",
                composed: true,
                applies: true,
            },
            Row {
                case: "a computed stand-in, in a declaration that reads nothing",
                logical_id: "Image",
                composed: true,
                applies: false,
            },
            Row {
                case: "a declared name, in a declaration that reads nothing",
                logical_id: "Image",
                composed: false,
                applies: false,
            },
            Row {
                case: "the minted resource's own declaration",
                logical_id: "Api",
                composed: true,
                applies: false,
            },
        ];
        for row in rows {
            let case = row.case;
            let pairs = vec![Minted {
                composed: row.composed,
                ..minted("Api", "an-api", "abcdefghijklmnopqrstuvwxyz")
            }];
            let applied = pairs_for(row.logical_id, &dependencies, &pairs);
            assert_eq!(
                !applied.is_empty(),
                row.applies,
                "case {case:?}: expected the pair to {} {:?}, got {applied:?}",
                match row.applies {
                    true => "apply to",
                    false => "be withheld from",
                },
                row.logical_id
            );
        }
    }
}
