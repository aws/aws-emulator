// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The prefix of every parameter group name Amazon RDS provides ITSELF.
///
/// Alongside each engine-version family RDS creates a default DB parameter group and a
/// default DB cluster parameter group that no request creates, named `default.` followed by
/// that family ([`super::default_db_parameter_group_name`], `default.mysql8.0`). The dot is
/// outside the alphabet a created group's name may use
/// ([`super::validate_parameter_group_name`]), so a name of this form can only ever be one
/// of RDS's own groups and never a record this store holds.
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

/// Whether `name` names one of the parameter groups Amazon RDS provides itself.
///
/// It is the NAME that decides this, and only the name: RDS provides such a group for every
/// engine-version family, and what follows the prefix is not resolved against the
/// engine-version catalogue here, because the answer this predicate guards is "is this group
/// the account's to change or RDS's own" — a `default.`-prefixed name is RDS's whichever
/// family it names. A group of this form is therefore never answered as missing, and never
/// changed either: it is RDS's, not the account's, so a request that would change one is
/// refused ([`CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE`]). The name is compared without
/// case, the way a group name is stored.
pub fn is_default_parameter_group_name(name: &str) -> bool {
    name.to_ascii_lowercase()
        .starts_with(DEFAULT_PARAMETER_GROUP_PREFIX)
}

/// How Amazon RDS refuses a request that would change one of the parameter groups it
/// provides itself: ONE fixed sentence, which is why it is a constant — it names neither the
/// group nor the member, so the same text answers every such request, for an instance group
/// and a cluster group alike.
pub const CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE: &str =
    "Cannot modify a default parameter group.";

/// How Amazon RDS words the DB-cluster-parameter-group not-found fault: the `<type> not
/// found: <name>` form it uses for a resource a request NAMED, carrying the request's own
/// spelling of the name. Stated once here so every operation that resolves a cluster
/// parameter group answers the one text.
pub fn db_cluster_parameter_group_not_found_message(named: &str) -> String {
    format!("DBClusterParameterGroup not found: {named}")
}

/// How Amazon RDS refuses a copy whose SOURCE is one of the parameter groups it provides
/// itself: "you can't copy a default DB parameter group. Instead, create a new custom DB
/// parameter group, which copies the default parameters and values for the specified DB
/// parameter group family."
///
/// The one variable part is the value the request carried, so the caller can see which of
/// its own values broke the rule; the rest states the restriction and the way around it.
/// `subject` is the member the request named the source by, so the instance-level copy and
/// the cluster-level one read as the one wording with the one difference between them.
pub fn cannot_copy_a_default_parameter_group_message(
    member: &str,
    named: &str,
    subject: &str,
) -> String {
    format!(
        "invalid value '{named}' for {member}: you can't copy a default {subject}; create a \
         new custom group for the family instead"
    )
}

/// How Amazon RDS words the DB-parameter-group not-found fault; the instance-scoped sibling
/// of [`db_cluster_parameter_group_not_found_message`].
pub fn db_parameter_group_not_found_message(named: &str) -> String {
    format!("DBParameterGroup not found: {named}")
}

/// How Amazon RDS reports that the SOURCE of a parameter-group copy does not exist: ONE
/// fixed sentence, which is why it is a constant — it names neither the group nor the
/// member, so the same text answers whichever value the request carried.
///
/// A copy is the one parameter-group operation that answers this way: the reads and the
/// delete name the group they could not find
/// ([`db_parameter_group_not_found_message`]), while the copy reports only that the source
/// is not a group that exists.
pub const PARAMETER_GROUP_DOES_NOT_EXIST_MESSAGE: &str = "Parameter group does not exist";

/// Refuse a `DBParameterGroupFamily` that is not a parameter group family at all.
///
/// A family is one engine's major-version line, and a group "can be associated with one and
/// only one DB cluster parameter group family, and can be applied only to a DB cluster
/// running a database engine and engine version compatible with that … family"
/// (`CreateDBClusterParameterGroupMessage$DBParameterGroupFamily`). So the families that
/// exist are exactly the ones an engine version carries, which is what this front door's
/// engine-version catalogue publishes
/// ([`crate::ops::describe_db_engine_versions::published_parameter_group_families`], the same
/// list the default parameter groups of the account are derived from) — for every engine it
/// serves, the Amazon RDS, Amazon DocumentDB and Amazon Neptune ones alike — beside the
/// VERSIONLESS families the member's own documented value list names, whose family is the
/// engine's bare name (`crate::ops::describe_engine_default_parameters::versionless_family`).
/// A value outside both names a family no engine version of this front door belongs to, so no
/// group of it could ever be applied to a cluster or an instance, and it is refused instead of
/// stored.
///
/// This resolves the STATIC catalogue, which is every engine version except the account's own:
/// a CUSTOM engine version is an engine version this front door publishes too, and it names
/// the family RDS writes for it, so a create consults the stored CEVs
/// ([`ComAmazonawsRdsAmazonrdsv19State::publishes_custom_parameter_group_family`]) before
/// refusing on this answer.
///
/// The comparison is case-INSENSITIVE, as every other name this service resolves against a
/// published list is: AWS publishes each family lowercase, so a differently-cased value still
/// names the one family it can only have meant.
///
/// The refusal is
/// [`service_common::rds_cluster_parameters::parameter_group_family_not_valid_message`].
pub fn validate_parameter_group_family(value: &str) -> Result<(), String> {
    if crate::ops::describe_engine_default_parameters::versionless_family(value) {
        return Ok(());
    }
    let published = crate::ops::describe_db_engine_versions::published_parameter_group_families()
        .into_iter()
        .any(|family| family.eq_ignore_ascii_case(value));
    if published {
        return Ok(());
    }
    Err(parameter_group_family_not_valid_message(value))
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Whether the DB CLUSTER parameter group `named` is one a request may associate a
    /// cluster with in `region`: either a group this region holds, or one of the default
    /// groups RDS provides ([`is_default_parameter_group_name`]), which no request creates
    /// and this store therefore has no record of.
    pub fn db_cluster_parameter_group_exists(&self, region: &str, named: &str) -> bool {
        let folded = named.to_ascii_lowercase();
        is_default_parameter_group_name(&folded)
            || self
                .db_cluster_parameter_groups
                .contains_key(&(region.to_string(), folded))
    }

    /// Whether the DB parameter group `named` is one a request may associate an instance
    /// with in `region`; the instance-scoped sibling of
    /// [`Self::db_cluster_parameter_group_exists`].
    pub fn db_parameter_group_exists(&self, region: &str, named: &str) -> bool {
        let folded = named.to_ascii_lowercase();
        is_default_parameter_group_name(&folded)
            || self
                .db_parameter_groups
                .contains_key(&(region.to_string(), folded))
    }
}

/// One parameter value a caller set on a group.
#[derive(Debug, Clone)]
pub struct ParameterSetting {
    /// `Parameter$ParameterValue`, as the caller set it.
    pub value: String,
    /// `Parameter$ApplyMethod`, as the caller asked for it (see
    /// [`APPLY_METHOD_PENDING_REBOOT`] for what an absent one becomes).
    pub apply_method: String,
    /// When the setting was written.
    ///
    /// A change that waits for a reboot ("changes to static parameters require a reboot …
    /// before the change can take effect") is in force on an engine that has come up SINCE
    /// it was made and not on one that has not, so the engine data plane compares this
    /// against when the instance's engine last started
    /// ([`data_plane::pg_server::running_parameters`]). Without it a pending change would
    /// be indistinguishable from an applied one.
    pub set_at: ::aws_smithy_types::DateTime,
}

/// A persisted cluster parameter group.
///
/// The name is stored lowercased and is half of this record's key:
/// `CreateDBClusterParameterGroupMessage$DBClusterParameterGroupName` states "This value is
/// stored as a lowercase string."
#[derive(Debug, Clone)]
pub struct DbClusterParameterGroupRecord {
    /// The lowercased `DBClusterParameterGroupName`.
    pub name: String,
    /// `DBParameterGroupFamily`, as the caller named it.
    pub family: String,
    /// `Description`.
    pub description: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The parameters a caller has set, by `ParameterName`.
    pub parameters: BTreeMap<String, ParameterSetting>,
    /// The group's tags.
    pub tags: Vec<Tag>,
}

impl DbClusterParameterGroupRecord {
    /// `DBClusterParameterGroupArn`. The resource type is `cluster-pg`, the one AWS uses
    /// for a cluster parameter group under this service's `rds` ARN namespace.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-pg:{}", self.name),
        )
    }

    /// This record as the modelled `DBClusterParameterGroup`.
    pub fn to_wire(&self) -> DBClusterParameterGroup {
        DBClusterParameterGroup {
            db_cluster_parameter_group_name: Some(self.name.clone()),
            db_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            db_cluster_parameter_group_arn: Some(self.arn()),
        }
    }

    /// Whether the change to `name` is still waiting on a reboot of the cluster's
    /// instances; see `service_common::rds_cluster_parameters::change_is_pending` for the
    /// documented rule.
    pub fn parameter_is_pending(&self, name: &str) -> bool {
        let Some(setting) = self.parameters.get(name) else {
            return false;
        };
        let spec = published_cluster_parameter_at_any_level(&self.family, name)
            .map(|spec| published_spec_for_family(&self.family, spec));
        change_is_pending_for(spec.as_ref(), &setting.apply_method)
    }

    /// The names of the parameter changes this group is still waiting on a reboot for, in
    /// name order; see [`Self::parameter_is_pending`].
    pub fn pending_parameters(&self) -> Vec<String> {
        self.parameters
            .keys()
            .filter(|name| self.parameter_is_pending(name))
            .cloned()
            .collect()
    }

    /// The parameter `name` as the modelled `Parameter`: the value and apply method the
    /// caller set, plus the metadata the family publishes for it when it publishes any.
    pub fn parameter_to_wire(&self, name: &str) -> Parameter {
        let setting = self.parameters.get(name);
        let spec = published_cluster_parameter_at_any_level(&self.family, name)
            .map(|spec| published_spec_for_family(&self.family, spec));
        let spec = spec.as_ref();
        Parameter {
            parameter_name: Some(name.to_string()),
            parameter_value: setting.map(|setting| setting.value.clone()),
            description: spec.and_then(|spec| published_column(spec.description)),
            source: Some(PARAMETER_SOURCE_USER.to_string()),
            apply_type: spec.map(|spec| spec.apply_type.to_string()),
            data_type: spec.map(|spec| spec.data_type.to_string()),
            allowed_values: spec.and_then(|spec| published_column(spec.allowed_values)),
            is_modifiable: spec.map(|spec| spec.is_modifiable),
            // Answered about THIS group's family, and only where the catalog publishes the
            // version the parameter was introduced in; see
            // [`cluster_engine_default_parameter_to_wire`], which reports it by the same
            // rule, so the two cannot disagree about a parameter a describe reports either
            // way.
            minimum_engine_version: spec.and_then(|spec| {
                (!spec.minimum_engine_version.is_empty())
                    .then(|| minimum_engine_version_in_family(&self.family, spec))
            }),
            apply_method: setting.map(|setting| setting.apply_method.clone()),
            supported_engine_modes: Some(
                crate::ops::describe_engine_default_cluster_parameters::family_engine_modes(
                    &self.family,
                ),
            ),
        }
    }

    /// Every parameter of this group at once, which is what a describe that names no
    /// `Source` reports: each parameter the group's family publishes, under the value and
    /// apply method the GROUP sets for it where it sets one (`Source` `user`) and under the
    /// engine's own default where it does not (`Source` `engine-default`), plus any
    /// parameter the group carries that the family publishes nothing for.
    ///
    /// A group inherits its family's whole parameter list the moment it is created — "the
    /// detailed parameter list for a particular DB cluster parameter group" is that list,
    /// with each parameter's source saying where its current value came from — so a group
    /// nobody has modified reports the family's parameters at their engine defaults rather
    /// than nothing at all.
    ///
    /// Keyed by parameter name, so the report is in name order whichever half an entry came
    /// from — the order its pagination marker walks.
    pub fn every_parameter(&self) -> BTreeMap<String, Parameter> {
        let mut reported = self.published_defaults();
        for name in self.parameters.keys() {
            reported.insert(name.clone(), self.parameter_to_wire(name));
        }
        reported
    }

    /// The parameters this group's family publishes, each with the default value the engine
    /// runs with and `Source` `engine-default`: what a `Source` naming the engine reports,
    /// and the half of [`Self::every_parameter`] the group has set nothing for.
    pub fn published_defaults(&self) -> BTreeMap<String, Parameter> {
        // Each family publishes the cluster parameters its OWN engine release carries, out of
        // the one catalog that serves every family of the engine
        // ([`family_carries_parameter`]).
        family_parameters(
            &self.family,
            crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_catalog(
                &self.family,
            )
            .unwrap_or_default(),
        )
        .into_iter()
        .map(|spec| {
            (
                spec.name.to_string(),
                group_default_cluster_parameter_to_wire(&self.family, spec),
            )
        })
        .collect()
    }
}

/// A persisted DB (instance) parameter group.
///
/// The name is stored lowercased and is half of this record's key:
/// `CreateDBParameterGroupMessage$DBParameterGroupName` states "This value is stored as a
/// lowercase string."
#[derive(Debug, Clone)]
pub struct DbParameterGroupRecord {
    /// The lowercased `DBParameterGroupName`.
    pub name: String,
    /// `DBParameterGroupFamily`, as the caller named it.
    pub family: String,
    /// `Description`.
    pub description: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The tags `CreateDBParameterGroup$Tags` assigned to the group. The modelled
    /// `DBParameterGroup` shape carries no tag member, so nothing reports them yet —
    /// `ListTagsForResource`, which does, is still scaffolded — but the caller's input is
    /// kept rather than dropped, so it is here when that operation lands.
    pub tags: Vec<Tag>,
    /// The parameters a caller has set on the group, by `ParameterName`, exactly as the
    /// cluster-level groups above carry theirs: `ModifyDBParameterGroup` writes them and
    /// `DescribeDBParameters` reports them with `Source` [`PARAMETER_SOURCE_USER`]. A
    /// freshly created group carries NONE — the engine's own defaults belong to the FAMILY,
    /// which publishes them ([`published_db_parameters`]), and are not materialised into a
    /// group.
    pub parameters: BTreeMap<String, ParameterSetting>,
}

impl DbParameterGroupRecord {
    /// `DBParameterGroupArn`. The resource type is `pg`, the one AWS uses for a DB
    /// parameter group under this service's `rds` ARN namespace.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(ARN_NAMESPACE, &self.region, &format!("pg:{}", self.name))
    }

    /// This record as the modelled `DBParameterGroup`.
    pub fn to_wire(&self) -> DBParameterGroup {
        DBParameterGroup {
            db_parameter_group_name: Some(self.name.clone()),
            db_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            db_parameter_group_arn: Some(self.arn()),
        }
    }

    /// The parameter `name` as the modelled `Parameter`: the value and apply method the
    /// caller set, plus the metadata this group's family publishes for it when it publishes
    /// any ([`published_spec_for_family`], so a record the engine moved between versions is
    /// this family's) — the instance-level counterpart of
    /// [`DbClusterParameterGroupRecord::parameter_to_wire`].
    pub fn parameter_to_wire(&self, name: &str) -> Parameter {
        let setting = self.parameters.get(name);
        let spec = published_db_parameter(&self.family, name)
            .map(|spec| published_spec_for_family(&self.family, spec));
        let spec = spec.as_ref();
        Parameter {
            parameter_name: Some(name.to_string()),
            parameter_value: setting.map(|setting| setting.value.clone()),
            description: spec.and_then(|spec| published_column(spec.description)),
            source: Some(PARAMETER_SOURCE_USER.to_string()),
            apply_type: spec.map(|spec| spec.apply_type.to_string()),
            data_type: spec.map(|spec| spec.data_type.to_string()),
            allowed_values: spec.and_then(|spec| published_column(spec.allowed_values)),
            is_modifiable: spec.map(|spec| spec.is_modifiable),
            // Reported only where the catalog publishes the version the parameter was
            // introduced in, exactly as [`engine_default_parameter_to_wire`] reports it: "the
            // earliest engine version to which the parameter can apply" is a fact about the
            // engine's release history, so a catalog entry that names none reports no member
            // rather than an empty one.
            minimum_engine_version: spec.and_then(|spec| {
                (!spec.minimum_engine_version.is_empty())
                    .then(|| spec.minimum_engine_version.to_string())
            }),
            apply_method: setting.map(|setting| setting.apply_method.clone()),
            supported_engine_modes: None,
        }
    }
}

/// The parameters the DB (instance) parameter group family `family` publishes, or `None`
/// when this emulator publishes none for it.
///
/// This is the catalog `DescribeEngineDefaultParameters` reports for the same family — one
/// family has ONE published parameter list, whichever operation is asked — so the two can
/// never disagree about what a parameter admits. It is a SAMPLE of that list rather than the
/// whole of it (an engine publishes hundreds of parameters per family), which is why
/// [`validate_db_parameter_setting`] checks the value of a parameter the catalog carries and
/// admits any name it does not.
///
/// A family the catalog is empty for, and a string that names no family at all, publish
/// nothing.
///
/// The cluster-level catalog of a family is the separate [`published_parameters`], which the
/// DB CLUSTER parameter group operations read.
pub fn published_db_parameters(family: &str) -> Option<&'static [ClusterParameterSpec]> {
    crate::ops::describe_engine_default_parameters::engine_default_parameters(family)
        .filter(|published| !published.is_empty())
}

/// The spec the DB (instance) parameter group family `family` publishes for the parameter
/// named `name`, if any; see [`published_db_parameters`].
pub fn published_db_parameter(family: &str, name: &str) -> Option<&'static ClusterParameterSpec> {
    spec_in_catalog(published_db_parameters(family), name)
}

/// Refuse a setting a DB (instance) parameter group of `family` cannot carry: a parameter
/// the family publishes that the engine does not let a caller modify, or a value outside the
/// ones that parameter admits.
///
/// A name the published catalog does not carry is ADMITTED, because that catalog is a sample
/// of the family's published list rather than the whole of it: see
/// [`published_db_parameters`] and [`validate_known_setting`], which carries the rule. The
/// cluster-level rule is [`validate_parameter_setting`], over a catalog that IS its family's
/// whole list.
pub fn validate_db_parameter_setting(
    family: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    validate_known_setting(published_db_parameters(family), name, value)
}

/// The cluster parameters the DB CLUSTER parameter group family `family` publishes, or
/// `None` when this emulator publishes none for it.
///
/// This is the catalog `DescribeEngineDefaultClusterParameters` reports for the family and
/// the one a GROUP of that family reports its parameters' metadata from, so what a value is
/// validated against and what a describe publishes as that parameter's `AllowedValues` can
/// never disagree.
///
/// It is the family's CLUSTER-level list, which is half of what a cluster parameter group of
/// that family carries: the group also carries the engine's own parameters, the ones a DB
/// parameter group of the same engine publishes ([`published_db_parameters`]), which is why
/// [`published_cluster_parameter_at_any_level`] resolves a name against both.
pub fn published_cluster_parameters(family: &str) -> Option<&'static [ClusterParameterSpec]> {
    crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_catalog(family)
}

/// The record the DB CLUSTER parameter group family `family` publishes for the parameter
/// named `name`, read from whichever LEVEL of that family publishes it.
///
/// The family selects WHICH catalog a parameter's published metadata comes from, never
/// whether there is any: a group carries the parameters a caller set on it, and each of them
/// is described by the record its family publishes — the family's cluster-level catalog
/// first ([`published_cluster_parameters`]) and then the record that same family publishes
/// at the DB-parameter level ([`published_db_parameter`]), which is the one catalog a family
/// whose engine publishes no cluster-level list has at all. A parameter group's family is
/// not scoped to the engine of the control plane the group was created through, so a
/// cluster group of an instance-only engine's family is an ordinary group whose parameters
/// that family describes — rather than one whose parameters report a bare name and value
/// with no apply type, data type, allowed values, description or modifiability.
///
/// A parameter the family publishes at NEITHER level is described by nothing, which is what
/// a group reports for it: this emulator's catalogs are samples of AWS's published lists
/// wherever the ENGINE's own variables are the vocabulary, so a name outside them is carried
/// (see [`validate_cluster_parameter_name`] for the one case it is not) without invented
/// metadata.
pub fn published_cluster_parameter_at_any_level(
    family: &str,
    name: &str,
) -> Option<&'static ClusterParameterSpec> {
    crate::ops::describe_engine_default_cluster_parameters::cluster_parameter_spec(family, name)
        .or_else(|| published_db_parameter(family, name))
}

/// Whether the vocabulary this emulator publishes for `family` is the WHOLE of what a group of
/// that family carries, rather than a SAMPLE of it. Stated once here, and the only thing that
/// decides whether an unpublished name can be refused ([`validate_cluster_parameter_name`]).
///
/// It is complete where the SERVICE defines the vocabulary and this emulator carries that
/// definition: Amazon DocumentDB publishes a closed, service-managed cluster parameter list
/// per family, which is the shared catalog
/// [`service_common::rds_cluster_parameters::published_parameters`] — whose own documentation
/// states it IS its families' complete list — and Amazon Neptune publishes its own such list,
/// which is this crate's Neptune cluster catalog.
///
/// It is a SAMPLE for every family whose parameters are the open-source ENGINE's own system
/// variables: the MySQL- and PostgreSQL-compatible families carry hundreds of them per
/// release, and both catalogs here say so in their own module headers — the cluster tables are
/// "not AWS's whole list" and the DB-parameter tables are drawn from the head of it. A name
/// outside a sample is therefore no evidence that the engine lacks the parameter, so nothing is
/// refused for it: the engine parameters such a group really carries reach past this sample —
/// `aws_default_s3_role`, which an Aurora MySQL cluster's own S3 import reads off its cluster
/// parameter group, is one of them.
pub fn publishes_a_complete_vocabulary(family: &str) -> bool {
    published_parameters(family).is_some()
        || crate::ops::describe_engine_default_parameters::family_engine(family)
            .is_some_and(|engine| engine == NEPTUNE_ENGINE)
}

/// Refuse the NAME of a parameter a DB CLUSTER parameter group of `family` cannot carry: one
/// the family publishes no record for at either level, where that family's published
/// vocabulary is the whole of it.
///
/// A group carries only parameters its family publishes, so a name the family publishes
/// nothing for ([`published_cluster_parameter_at_any_level`], both levels of it) names no
/// parameter of the group and is refused with
/// [`service_common::rds_cluster_parameters::parameter_not_found_message`] — whether the
/// request would SET it (where the value rule follows, see
/// [`validate_cluster_parameter_setting`]) or RESET it, since there is no such parameter to put
/// back to its default either. The two operations read this one resolution, so they cannot
/// disagree about which parameters a group has.
///
/// The refusal is given only for a family whose vocabulary is COMPLETE
/// ([`publishes_a_complete_vocabulary`]): against a sample of AWS's published list it would
/// reject parameters the engine really has.
pub fn validate_cluster_parameter_name(family: &str, name: &str) -> Result<(), String> {
    if published_cluster_parameter_at_any_level(family, name).is_some()
        || !publishes_a_complete_vocabulary(family)
    {
        return Ok(());
    }
    Err(parameter_not_found_message(name))
}

/// Refuse a setting a DB CLUSTER parameter group of `family` cannot carry: a parameter the
/// family does not publish at all, one the engine does not let a caller modify, or a value
/// outside the ones that parameter admits — the `AllowedValues` its own published catalog
/// entry carries, whichever engine's family it is.
///
/// The name half is [`validate_cluster_parameter_name`], which the reset of a group reads
/// too, so the two operations cannot disagree about which parameters a group has.
///
/// `Err` carries the message to answer with, built from the request and from that parameter's
/// catalog entry; the instance-level rule is [`validate_db_parameter_setting`].
pub fn validate_cluster_parameter_setting(
    family: &str,
    name: &str,
    value: &str,
) -> Result<(), String> {
    validate_parameter_setting(family, name, value)?;
    validate_cluster_parameter_name(family, name)?;
    // ONE resolution answers both halves: the record a group REPORTS for a parameter
    // ([`published_cluster_parameter_at_any_level`]) is the record the value it carries is
    // held to, so a group can never store a value the `AllowedValues` it publishes for that
    // parameter forbids, nor a value for a parameter it reports as not modifiable.
    validate_known_setting(
        published_cluster_parameter_at_any_level(family, name).map(std::slice::from_ref),
        name,
        value,
    )
}

/// The apply method a change to a CLUSTER parameter gets when the caller asks for none: the
/// one the parameter's own apply type implies, as its family publishes it at either level
/// ([`published_cluster_parameter_at_any_level`]); the instance-level sibling is
/// [`db_parameter_default_apply_method`], and
/// [`service_common::rds_cluster_parameters::default_apply_method`] carries the documented
/// rule.
pub fn cluster_parameter_default_apply_method(family: &str, name: &str) -> &'static str {
    let spec = published_cluster_parameter_at_any_level(family, name)
        .map(|spec| published_spec_for_family(family, spec));
    default_apply_method_for(spec.as_ref())
}

/// The apply method a change to a DB (instance) parameter gets when the caller asks for
/// none: the one the parameter's own apply type implies, as its family publishes it
/// ([`published_spec_for_family`]); see
/// [`service_common::rds_cluster_parameters::default_apply_method`] for the documented rule.
pub fn db_parameter_default_apply_method(family: &str, name: &str) -> &'static str {
    let spec = published_db_parameter(family, name)
        .map(|spec| published_spec_for_family(family, spec));
    default_apply_method_for(spec.as_ref())
}

/// A parameter the engine publishes for a family, as the modelled `Parameter` with its
/// default value and `Source` [`PARAMETER_SOURCE_ENGINE_DEFAULT`]: what a describe reports
/// when the caller asks for the engine's own parameters rather than a group's.
///
/// `family` is the DB parameter group family the report is ABOUT, which is what the
/// published record is resolved for ([`published_spec_for_family`]): one catalog serves
/// every family of an engine, and a record the engine moved between versions is the one
/// that family's version carries.
///
/// `MinimumEngineVersion` is reported only where the catalog publishes the version the
/// parameter was introduced in; an entry that names none reports none rather than a guessed
/// version, since "the earliest engine version to which the parameter can apply" is a fact
/// about the engine's release history and not something this front door can derive.
///
/// `ParameterValue`, `AllowedValues` and `Description` are reported only where the catalog
/// publishes one. A parameter the engine AUTOSIZES at startup publishes no default, one the
/// engine publishes neither an enumeration nor a range for has no allowed values, and one the
/// published list carries no explanatory text for has no description; reporting an empty
/// string for any of them would say the engine runs that parameter at the empty value, admits
/// only it, or describes it as the empty sentence, none of which the engine ever published.
pub fn engine_default_parameter_to_wire(family: &str, spec: &ClusterParameterSpec) -> Parameter {
    let spec = published_spec_for_family(family, spec);
    Parameter {
        parameter_name: Some(spec.name.to_string()),
        parameter_value: published_default(spec.default_value),
        description: published_column(spec.description),
        source: Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()),
        apply_type: Some(spec.apply_type.to_string()),
        data_type: Some(spec.data_type.to_string()),
        allowed_values: published_column(spec.allowed_values),
        is_modifiable: Some(spec.is_modifiable),
        minimum_engine_version: (!spec.minimum_engine_version.is_empty())
            .then(|| spec.minimum_engine_version.to_string()),
        apply_method: None,
        supported_engine_modes: None,
    }
}

/// The default a catalog entry publishes, as `ParameterValue` reports it: the value where the
/// catalog publishes one, and nothing where it publishes none.
///
/// The catalog writes "the engine publishes no default for this parameter" as the EMPTY
/// `default_value` — MySQL's `back_log` is autosized from the permitted connection count
/// rather than given a published default — and the member is then left out rather than
/// reported empty, since an empty `ParameterValue` would say the engine runs the parameter at
/// the empty value.
fn published_default(default_value: &str) -> Option<String> {
    published_column(default_value)
}

/// One column of a catalog entry as the matching `Parameter` member reports it: the value
/// where the catalog publishes one, and NOTHING where the column is empty, which is how the
/// catalogs write "the engine publishes none" (see [`published_default`]).
fn published_column(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// A CLUSTER parameter the family `family` publishes, as the modelled `Parameter`: what
/// [`engine_default_parameter_to_wire`] reports plus `SupportedEngineModes`, "the valid DB
/// engine modes", which a cluster parameter has and an instance parameter does not.
///
/// `MinimumEngineVersion` is answered about the FAMILY the report is for, not about the
/// engine as a whole: the earliest version the parameter can apply at inside that family is
/// the later of the version it was introduced in and the family's own first release
/// (`service_common::rds_cluster_parameters::minimum_engine_version_in_family`). A family
/// whose first release this front door does not publish keeps the introduction version
/// unchanged.
pub fn cluster_engine_default_parameter_to_wire(
    family: &str,
    spec: &ClusterParameterSpec,
) -> Parameter {
    Parameter {
        supported_engine_modes: Some(
            crate::ops::describe_engine_default_cluster_parameters::family_engine_modes(family),
        ),
        minimum_engine_version: (!spec.minimum_engine_version.is_empty())
            .then(|| minimum_engine_version_in_family(family, spec)),
        ..engine_default_parameter_to_wire(family, spec)
    }
}

/// A parameter the family publishes, as a GROUP of that family reports it in its own
/// parameter listing: the published record of [`engine_default_parameter_to_wire`] plus the
/// apply method that parameter carries in the group.
///
/// A group's own listing is about the GROUP rather than about the engine, and every
/// parameter of a group has an apply method: `Parameter$ApplyMethod` "indicates when to
/// apply parameter updates", and a parameter whose value nobody has changed is reported
/// under [`APPLY_METHOD_PENDING_REBOOT`] — the method a change to it would take while none
/// is waiting to be applied. The FAMILY's engine-default report
/// (`DescribeEngineDefaultParameters`) is about the engine and about no group at all, so it
/// reports no apply method; the two projections differ in exactly that member.
pub fn group_default_parameter_to_wire(family: &str, spec: &ClusterParameterSpec) -> Parameter {
    Parameter {
        apply_method: Some(APPLY_METHOD_PENDING_REBOOT.to_string()),
        ..engine_default_parameter_to_wire(family, spec)
    }
}

/// The CLUSTER-level counterpart of [`group_default_parameter_to_wire`]: what a DB CLUSTER
/// parameter group reports for a parameter of its family that nobody has changed — the
/// cluster-level published record plus that same apply method.
pub fn group_default_cluster_parameter_to_wire(
    family: &str,
    spec: &ClusterParameterSpec,
) -> Parameter {
    Parameter {
        apply_method: Some(APPLY_METHOD_PENDING_REBOOT.to_string()),
        ..cluster_engine_default_parameter_to_wire(family, spec)
    }
}

/// The whole published record of `spec` FOR the family `family`: the catalog's own columns,
/// except for a parameter whose published record MOVED with the engine version, which reads
/// as that family's own version carries it.
///
/// `rds.force_ssl` is such a parameter, and both halves of its record moved at the SAME
/// release: from PostgreSQL 15 Amazon RDS turns it on by default ("the default is 1 for RDS
/// for PostgreSQL 15 and higher") and the engine applies a change to it without a restart
/// ("beginning with RDS for PostgreSQL 15, rds.force_ssl is a dynamic parameter"). So both
/// read the ONE place that states that version
/// ([`crate::state::data_plane::postgres_forces_ssl_by_default`]), which is also what decides
/// whether a session on an RDS for PostgreSQL instance of that family may be in the clear —
/// the value a describe reports for that engine, the apply type a change to it is held to,
/// and what such an instance holds a session to are one answer.
///
/// The apply type is the PostgreSQL ENGINE's, so every PostgreSQL-compatible family whose
/// version says so reports it dynamic; the DEFAULT that moved is RDS for PostgreSQL's own, so
/// another PostgreSQL-compatible engine keeps the catalog's default (Amazon Aurora PostgreSQL
/// publishes 0, and what its own cluster endpoint holds a session to is that service slice's
/// own rule rather than this one). A family whose version cannot be read, and every other
/// engine that carries a parameter of this name, report the catalog record unchanged.
pub fn published_spec_for_family(family: &str, spec: &ClusterParameterSpec) -> ClusterParameterSpec {
    let mut resolved = *spec;
    if spec.name != POSTGRES_FORCE_SSL_PARAMETER {
        return resolved;
    }
    let Some(engine) = crate::ops::describe_engine_default_parameters::family_engine(family) else {
        return resolved;
    };
    if !crate::ops::describe_engine_default_parameters::is_postgres_engine(engine) {
        return resolved;
    }
    let Some(version) = crate::ops::describe_engine_default_parameters::family_version(family)
    else {
        return resolved;
    };
    let from_fifteen = crate::state::data_plane::postgres_forces_ssl_by_default(version);
    resolved.apply_type = if from_fifteen {
        APPLY_TYPE_DYNAMIC
    } else {
        APPLY_TYPE_STATIC
    };
    if engine == POSTGRES_ENGINE {
        resolved.default_value = if from_fifteen { "1" } else { "0" };
    }
    resolved
}

/// The parameter RDS for PostgreSQL carries the TLS requirement of, which both the
/// published catalog above and the engine's login gate are about.
pub const POSTGRES_FORCE_SSL_PARAMETER: &str = "rds.force_ssl";

/// Whether the engine release the DB parameter group family `family` names CARRIES the
/// parameter `spec` publishes the record of.
///
/// A family is one release line of one engine (`mysql8.0`, `aurora-postgresql16`,
/// `sqlserver-ee-15.0`), and a parameter exists in the engine from the release that
/// introduced it until the release that removed it: the first is the fact a catalog entry's
/// `Parameter$MinimumEngineVersion` states, "the earliest engine version to which the
/// parameter can apply", and the second is the one its catalog publishes beside it
/// (`crate::ops::describe_engine_default_parameters::parameter_removed_in`). So a parameter
/// whose introducing release is NEWER than the line the family names, and one whose removal
/// is at or BEFORE that line, are both outside that family's parameters: the family's engine
/// release does not have them, and reporting one would publish a parameter a caller cannot
/// set on a group of that family. One catalog per engine therefore serves every family of
/// that engine, each family publishing the parameters its own release carries.
///
/// The comparison is over the components the FAMILY's version carries, because a family
/// names a release LINE rather than one release: the `mysql8.0` family is every 8.0.x
/// release, so a parameter introduced at 8.0.2 is one of its parameters and one dropped at
/// 8.0.3 is not, while the `mysql5.7` family — an earlier line — carries the dropped one and
/// none of the 8.0 additions.
///
/// Where a catalog publishes no such release the parameter is carried, since nothing is
/// withheld on a guess: an entry that names no introducing release is one the engine has
/// carried since before any version this service serves, a parameter no entry names a
/// removal for has never been dropped, a family whose version this front door cannot read as
/// a dotted release is compared against neither end, and neither is a family whose engine
/// numbers its own releases independently of the catalog's
/// (`crate::ops::describe_engine_default_parameters::family_version_is_in_catalog_numbering`),
/// where the two numbers name releases of different engines and comparing them would answer
/// by arithmetic rather than by anything published.
pub fn family_carries_parameter(family: &str, spec: &ClusterParameterSpec) -> bool {
    let Some(engine) = crate::ops::describe_engine_default_parameters::family_engine(family)
    else {
        return true;
    };
    if !crate::ops::describe_engine_default_parameters::family_version_is_in_catalog_numbering(
        engine,
    ) {
        return true;
    }
    // A removal is a release of the ENGINE whose catalog the entry belongs to, so it is
    // resolved for that engine and never for another's parameter of the same name.
    let removed =
        crate::ops::describe_engine_default_parameters::parameter_removed_in(engine, spec.name);
    if spec.minimum_engine_version.is_empty() && removed.is_none() {
        return true;
    }
    let Some(version) = crate::ops::describe_engine_default_parameters::family_version(family)
    else {
        return true;
    };
    // The release line the family names, as the number of components its own version
    // carries: an engine release is compared against the family on those components alone.
    let line = |release: &str| -> String {
        release
            .split('.')
            .take(version.split('.').count())
            .collect::<Vec<&str>>()
            .join(".")
    };
    let precedes = ::service_common::rds_cluster_parameters::engine_version_precedes;
    if !spec.minimum_engine_version.is_empty()
        && precedes(version, &line(spec.minimum_engine_version))
    {
        return false;
    }
    !removed.is_some_and(|removed| !precedes(version, &line(removed)))
}

/// The entries of `catalog` the engine release `family` names carries, in the catalog's own
/// order — the whole published list of that family, which is what every report over a
/// family's parameters is a window onto ([`family_carries_parameter`]).
pub fn family_parameters(
    family: &str,
    catalog: &'static [ClusterParameterSpec],
) -> Vec<&'static ClusterParameterSpec> {
    catalog
        .iter()
        .filter(|spec| family_carries_parameter(family, spec))
        .collect()
}

/// The `Engine` value of Amazon RDS for PostgreSQL, as `CreateDBInstance$Engine` publishes
/// it and as a DB parameter group family of that engine is named after
/// (`postgres17`) — the engine whose own published defaults
/// [`published_spec_for_family`] resolves.
pub const POSTGRES_ENGINE: &str = "postgres";

/// What the `Engine` value of every OTHER PostgreSQL-compatible engine ends with
/// (`aurora-postgresql`) — the engines that run the same PostgreSQL releases, and so carry
/// the apply type those releases give a parameter.
pub const POSTGRES_COMPATIBLE_ENGINE_SUFFIX: &str = "postgresql";
