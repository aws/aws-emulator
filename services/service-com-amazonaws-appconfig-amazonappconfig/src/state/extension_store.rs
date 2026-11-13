// SPDX-License-Identifier: CC0-1.0

//! The EXTENSION half of this service's store: the extensions an account holds, their
//! versions, and the associations that bind a version to an application, an environment or
//! a configuration profile.
//!
//! One concern, moved out of `state.rs` so each file holds one: the records here are owned
//! by the ACCOUNT rather than by any application, and every operation that resolves an
//! `ExtensionIdentifier`, an extension version or an association reaches them through these
//! methods. Behaviour is unchanged — this is the same code the state module held, and
//! `use super::*` keeps every path in it resolving as it did.

use super::*;

impl ComAmazonawsAppconfigAmazonappconfigState {
    /// The extension `identifier` names, and the version number the identifier itself
    /// pins, or `None` when no extension this account can see answers to it.
    ///
    /// `identifier` is "the name, the ID, or the Amazon Resource Name (ARN) of the
    /// extension"
    /// (`smithy_query(query="doc",shape="GetExtensionRequest",member="ExtensionIdentifier")`),
    /// so all three forms resolve here and every operation that takes an
    /// `ExtensionIdentifier` resolves it the same way. An ARN addresses ONE version, so
    /// it pins that version number; an id or a name addresses the extension as a whole
    /// and pins none.
    ///
    /// Both halves of the account's catalogue answer: an extension this account created,
    /// and failing that one of AppConfig's AWS-authored extensions
    /// ([`AWS_AUTHORED_EXTENSIONS`]), which every account can read without having created
    /// it. A created extension cannot carry an authored name — `CreateExtension` refuses
    /// one — so the two halves never answer for the same identifier.
    pub fn resolve_extension(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<(Extension, Option<i32>)> {
        if let Some(by_id) = self.extensions.get(identifier) {
            return Some((by_id.clone(), None));
        }
        if let Some(TaggedResource::Extension {
            extension_id,
            version_number,
        }) = parse_resource_arn(identifier)
        {
            if let Some(by_arn) = self.extensions.get(extension_id) {
                return Some((by_arn.clone(), Some(version_number)));
            }
        }
        if let Some(by_name) = self
            .extensions
            .values()
            .find(|extension| extension.name == identifier)
        {
            return Some((by_name.clone(), None));
        }
        aws_authored_extension(identifier, region)
    }

    /// The extension stored under `extension_id`, for a caller that already holds the
    /// minted id rather than a caller-supplied identifier.
    pub fn extension(&self, extension_id: &str) -> Option<&Extension> {
        self.extensions.get(extension_id)
    }

    /// The extension carrying `name`, or `None` when no extension does. Extension names
    /// are unique per account ("Each extension name in your account must be unique", as
    /// the model documents on `CreateExtension`'s `Name` member), so at most one answers.
    pub fn extension_named(&self, name: &str) -> Option<&Extension> {
        self.extensions
            .values()
            .find(|extension| extension.name == name)
    }

    /// Every extension this account created, for a reader that must report them without
    /// reaching into the map.
    pub fn created_extensions(&self) -> impl Iterator<Item = &Extension> {
        self.extensions.values()
    }

    /// The whole extension catalogue this account can see, in `region`: AppConfig's
    /// AWS-authored extensions ([`AWS_AUTHORED_EXTENSIONS`]) and every extension this
    /// account created.
    ///
    /// This is [`resolve_extension`] read over the whole catalogue rather than for one
    /// identifier, so a listing reports nothing a reader of it would then miss on, and
    /// each extension exactly once: an authored name is reserved, so no created extension
    /// can carry one. `ListExtensions`, which reports "all custom and Amazon Web Services
    /// authored AppConfig extensions in the account", is the caller this exists for; the
    /// order a listing reports them in is the listing's own.
    ///
    /// [`resolve_extension`]: ComAmazonawsAppconfigAmazonappconfigState::resolve_extension
    pub fn all_extensions(&self, region: &str) -> Vec<Extension> {
        AWS_AUTHORED_EXTENSIONS
            .iter()
            .map(|authored| Extension::aws_authored(authored, region))
            .chain(self.created_extensions().cloned())
            .collect()
    }

    /// Store a brand-new extension, holding its first version.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_EXTENSIONS`] of them — the bound that keeps a loop of create calls from
    /// growing the emulator's heap without limit. `CreateExtension` is the caller this
    /// exists for, and answers that `false` with the `ServiceQuotaExceededException` it
    /// models.
    pub fn try_insert_extension(&mut self, extension: Extension) -> bool {
        if self.extensions.len() >= MAX_EXTENSIONS {
            return false;
        }
        self.extensions.insert(extension.id.clone(), extension);
        true
    }

    /// Add one more version to the extension `extension_id` names.
    ///
    /// Answers `false`, having stored nothing, when that extension already holds
    /// [`MAX_EXTENSION_VERSIONS`] versions, or when no extension answers to
    /// `extension_id`.
    pub fn try_insert_extension_version(
        &mut self,
        extension_id: &str,
        version: ExtensionVersion,
    ) -> bool {
        let Some(extension) = self.extensions.get_mut(extension_id) else {
            return false;
        };
        if extension.versions.len() >= MAX_EXTENSION_VERSIONS {
            return false;
        }
        extension.versions.insert(version.version_number, version);
        true
    }

    /// The stored version `version_number` of the extension `extension_id` names, for a
    /// caller that EDITS that version in place rather than appending another one.
    ///
    /// `UpdateExtension` is the caller this exists for: it updates the extension version
    /// a request names, so the version number, the ARN that names it and the tags it
    /// carries all stay as they are and only the definition members the request carried
    /// change. Answers `None` when no extension answers to `extension_id` or it holds no
    /// such version.
    pub fn extension_version_mut(
        &mut self,
        extension_id: &str,
        version_number: i32,
    ) -> Option<&mut ExtensionVersion> {
        self.extensions
            .get_mut(extension_id)?
            .versions
            .get_mut(&version_number)
    }

    /// Forget an extension this account created, with every version it holds.
    /// `DeleteExtension` is the caller this exists for, and removing the record is what
    /// frees its slot.
    pub fn remove_extension(&mut self, extension_id: &str) -> Option<Extension> {
        self.extensions.remove(extension_id)
    }

    /// Forget ONE version of an extension this account created, answering the version
    /// that was removed, or `None` when no extension answers to `extension_id` or it
    /// holds no such version.
    ///
    /// An extension exists only as its versions — the id and the name are properties of
    /// the set, and the description, actions, parameters and tags belong to a version —
    /// so an extension whose last version is removed is itself gone, and
    /// [`Self::resolve_extension`] stops answering for it. That is the same bookkeeping
    /// [`Self::remove_extension`] does, reached one version at a time, so a deleted
    /// version never leaves an extension that answers with nothing.
    pub fn remove_extension_version(
        &mut self,
        extension_id: &str,
        version_number: i32,
    ) -> Option<ExtensionVersion> {
        let extension = self.extensions.get_mut(extension_id)?;
        let removed = extension.versions.remove(&version_number)?;
        if extension.versions.is_empty() {
            self.extensions.remove(extension_id);
        }
        Some(removed)
    }

    /// The ARN of the AppConfig resource `identifier` names, as the record an extension
    /// association reports it by, or `None` when it addresses nothing an extension may be
    /// associated to.
    ///
    /// `ResourceIdentifier` is "the ARN of an application, configuration profile, or
    /// environment"
    /// (`smithy_query(query="doc",shape="CreateExtensionAssociationRequest",member="ResourceIdentifier")`),
    /// so an ARN is resolved against the tree and answered only when the resource it
    /// addresses exists. The ARN is rebuilt from the resolved record in `region` rather
    /// than echoed back, so what the association stores is this service's own ARN for the
    /// resource it found — not whatever partition or account the caller typed.
    ///
    /// The member's type is `Identifier`, the same shape every "ID or name" member of this
    /// service targets, so an identifier that is not an ARN at all is resolved the way
    /// every other `ApplicationIdentifier` is ([`Self::resolve_application_id`]): an
    /// application is the one resource of the three that an id or a name addresses
    /// unambiguously on its own.
    pub fn resolve_association_resource(&self, identifier: &str, region: &str) -> Option<String> {
        let resource = match parse_resource_arn(identifier) {
            Some(resource) => resource,
            // Not an ARN: an application id or name, which is the one form of the three
            // that addresses a resource on its own.
            None => {
                let application_id = self.resolve_application_id(identifier)?;
                return Some(::engine::emulator_arn(
                    SERVICE_NAMESPACE,
                    region,
                    &format!("application/{application_id}"),
                ));
            }
        };
        let portion = match resource {
            TaggedResource::Application { application_id } => {
                let application = self.applications.get(application_id)?;
                format!("application/{}", application.id)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => {
                let environment = self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?;
                format!("application/{application_id}/environment/{}", environment.id)
            }
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => {
                let profile = self
                    .applications
                    .get(application_id)?
                    .profiles
                    .get(configuration_profile_id)?;
                configuration_profile_arn_portion(application_id, &profile.id)
            }
            // Every other AppConfig resource — a deployment, a deployment strategy, an
            // extension, another association — is not one of the three an extension
            // associates to.
            _ => return None,
        };
        Some(::engine::emulator_arn(SERVICE_NAMESPACE, region, &portion))
    }

    /// The extension association `association_id` names, or `None` when this account
    /// created none under that id. `GetExtensionAssociation` is the caller this exists for.
    pub fn extension_association(&self, association_id: &str) -> Option<&ExtensionAssociation> {
        self.extension_associations.get(association_id)
    }

    /// The extension association `association_id` names, for a caller that EDITS it in
    /// place rather than creating another one.
    ///
    /// `UpdateExtensionAssociation` is the caller this exists for: the association keeps
    /// the id and ARN its create minted, the extension version it binds and the resource
    /// it is bound to, so only the members an update carries change.
    pub fn extension_association_mut(
        &mut self,
        association_id: &str,
    ) -> Option<&mut ExtensionAssociation> {
        self.extension_associations.get_mut(association_id)
    }

    /// Every extension association this account created, for a reader that must report
    /// them without reaching into the map. The order is the map's, which is no order at
    /// all: a listing orders what it reports itself.
    ///
    /// `ListExtensionAssociations`, which reports "all AppConfig extension associations in
    /// the account", is the caller this exists for.
    pub fn extension_associations(&self) -> impl Iterator<Item = &ExtensionAssociation> {
        self.extension_associations.values()
    }

    /// The extension associations in force for a deployment of
    /// `configuration_profile_id` into `environment_id` of `application_id`, as the
    /// `AppliedExtensions` that deployment records: "the extensions that were previously
    /// associated to the configuration profile, environment, or the application when
    /// `StartDeployment` was called"
    /// (`smithy_query(query="doc",shape="Deployment",member="AppliedExtensions")`).
    ///
    /// An association binds ONE of the three, and a deployment runs all three levels, so
    /// an association bound to any of them is applied to the deployment. The binding is
    /// matched on the resource the association's ARN ADDRESSES rather than on the ARN text,
    /// so an association created against one Region's ARN still applies to a deployment
    /// started in another: this emulator holds one tree, not one per Region. The order is
    /// by association id, so two reads of one deployment report the same list.
    pub fn applied_extensions_for(
        &self,
        application_id: &str,
        environment_id: &str,
        configuration_profile_id: &str,
    ) -> Vec<AppliedExtension> {
        let scope = extension_actions::Scope {
            application_id,
            environment_id: Some(environment_id),
            configuration_profile_id: Some(configuration_profile_id),
        };
        let mut applied: BTreeMap<&str, AppliedExtension> = BTreeMap::new();
        for association in self.extension_associations.values() {
            // The SAME binding the action-point dispatch resolves its targets through,
            // so what a deployment reports as applied and what it actually invokes
            // cannot disagree.
            if !extension_actions::association_binds(&association.resource_arn, &scope) {
                continue;
            }
            applied.insert(
                association.id.as_str(),
                AppliedExtension {
                    extension_id: Some(association.extension_id.clone()),
                    extension_association_id: Some(association.id.clone()),
                    version_number: association.extension_version_number,
                    parameters: Some(association.parameters.clone()),
                    provided_identifier: Some(association.provided_identifier.clone()),
                },
            );
        }
        applied.into_values().collect()
    }

    /// How many associations bind the `version_number` version of the extension
    /// `extension_id` names.
    ///
    /// "You must delete all associations to an extension before you delete the extension"
    /// (`smithy_query(query="operation",operation="DeleteExtension")`), which this is what
    /// `DeleteExtension` checks before removing a version: an association names ONE
    /// version, so the associations that block a delete are the ones bound to the version
    /// the delete would take away. The COUNT is what the refusal reports, so it is counted
    /// here rather than being answered as a yes-or-no and counted again by the caller.
    pub fn extension_version_association_count(
        &self,
        extension_id: &str,
        version_number: i32,
    ) -> usize {
        self.extension_associations
            .values()
            .filter(|association| {
                association.extension_id == extension_id
                    && association.extension_version_number == version_number
            })
            .count()
    }

    /// Materialise the extension association AppConfig itself makes for a configuration
    /// profile of type [`FEATURE_FLAGS_PROFILE_TYPE`], and answer the association's id.
    ///
    /// The feature-flag helper ([`FEATURE_FLAGS_EXTENSION_ID`]) is what checks a
    /// feature-flag document as it is written and transforms it as it is served, so
    /// AppConfig associates it to such a profile as the profile is created rather than
    /// leaving it to a `CreateExtensionAssociation` call of the account's. What it
    /// materialises is an association record like any other — the extension's only
    /// published version, bound to the profile's own ARN, with no parameter values because
    /// the helper declares no parameter — so every reader of an association reports it, and
    /// a deployment of that profile reports it among its `AppliedExtensions`.
    ///
    /// IDEMPOTENT: an association already binding that extension to that profile IS this
    /// association, so its id is answered and nothing is written. `None` means nothing is
    /// associated: this account already holds [`MAX_EXTENSION_ASSOCIATIONS`] associations.
    pub fn associate_feature_flags_extension(
        &mut self,
        application_id: &str,
        configuration_profile_id: &str,
        region: &str,
    ) -> Option<String> {
        let authored = aws_authored_extension_named(FEATURE_FLAGS_EXTENSION_ID)?;
        let extension = Extension::aws_authored(authored, region);
        let version = extension.latest_version()?;
        let resource_arn =
            configuration_profile_arn(region, application_id, configuration_profile_id);
        if let Some(existing) = self.extension_associations.values().find(|association| {
            association.extension_id == extension.id && association.resource_arn == resource_arn
        }) {
            return Some(existing.id.clone());
        }
        let id = self.mint_id();
        let association = ExtensionAssociation {
            arn: extension_association_arn(region, &id),
            id: id.clone(),
            extension_id: extension.id.clone(),
            // AppConfig makes this association itself and names the extension by its
            // published id, so that is the identifier it provided.
            provided_identifier: extension.id.clone(),
            extension_arn: version.arn.clone(),
            extension_version_number: version.version_number,
            resource_arn,
            parameters: HashMap::new(),
            tags: HashMap::new(),
        };
        self.try_insert_extension_association(association)
            .then_some(id)
    }

    /// Store an extension association this account created.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_EXTENSION_ASSOCIATIONS`] of them — the bound that keeps a loop of create
    /// calls from growing the emulator's heap without limit.
    /// `CreateExtensionAssociation` is the caller this exists for, and answers that
    /// `false` with the `ServiceQuotaExceededException` it models.
    pub fn try_insert_extension_association(
        &mut self,
        association: ExtensionAssociation,
    ) -> bool {
        if self.extension_associations.len() >= MAX_EXTENSION_ASSOCIATIONS {
            return false;
        }
        self.extension_associations
            .insert(association.id.clone(), association);
        true
    }

    /// Forget the extension association `association_id` names, answering the record that
    /// was removed, or `None` when this account created none under that id.
    ///
    /// `DeleteExtensionAssociation` is the caller this exists for. The association record
    /// holds the binding and the tags that were assigned to it and nothing else, so taking
    /// it out of the map takes both — and because the extension it bound is held in its own
    /// map, removing the association leaves that extension and every version of it exactly
    /// where it was. Removing the record also frees its slot and releases its id.
    pub fn remove_extension_association(
        &mut self,
        association_id: &str,
    ) -> Option<ExtensionAssociation> {
        self.extension_associations.remove(association_id)
    }
}
