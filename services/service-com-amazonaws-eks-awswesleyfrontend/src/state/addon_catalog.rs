// SPDX-License-Identifier: CC0-1.0

//! The add-on catalog: the builds, compatibilities and configuration schemas this
//! service publishes for the Amazon EKS add-ons, the records they are read back
//! as, and the seed that puts them in the store.

use super::*;

/// One row of the add-on catalog seed: an Amazon EKS-managed add-on, and the
/// version line its published builds run along.
///
/// The catalog is a PUBLICATION, like the Kubernetes version calendar
/// ([`VERSION_CALENDAR`]): it is what Amazon EKS offers, not per-account data,
/// so it is seeded once and no operation writes it.
#[derive(Clone, Copy)]
pub(super) struct AddonCatalogSeed {
    /// The add-on's name, the value `CreateAddon$addonName` takes.
    pub(super) addon_name: &'static str,
    /// Who owns the add-on ([`ADDON_OWNER_AWS`] or
    /// [`ADDON_OWNER_MARKETPLACE`]), which `DescribeAddonVersions$owners`
    /// filters on.
    pub(super) owner: &'static str,
    /// Who publishes the add-on ([`ADDON_PUBLISHER_EKS`] for an Amazon
    /// EKS-managed add-on, the vendor for a Marketplace one), which
    /// `DescribeAddonVersions$publishers` filters on.
    pub(super) publisher: &'static str,
    /// The category Amazon EKS publishes the add-on under
    /// (`AddonInfo$type`), which `DescribeAddonVersions$types` filters on.
    pub(super) addon_type: &'static str,
    /// The Kubernetes namespace the add-on's workloads are installed into
    /// (`AddonInfo$defaultNamespace`).
    pub(super) default_namespace: &'static str,
    /// How the add-on's pods are placed, which decides what its install needs
    /// from the cluster: a [`AddonWorkload::PerNode`] add-on has nothing to
    /// schedule on a cluster with no nodes, a [`AddonWorkload::Replicated`] one
    /// must get a replica scheduled somewhere.
    ///
    /// Source: the workloads each add-on's chart ships — the node agents and
    /// device plugins are DaemonSets, the controllers, operators and servers are
    /// Deployments.
    pub(super) workload: AddonWorkload,
    /// The add-ons whose Kubernetes resources this add-on's own resources are
    /// applied against: the ones its documented install requires on the cluster
    /// FIRST, because its custom resources, certificates or webhooks come from
    /// them. EMPTY for an add-on that needs nothing but the cluster.
    ///
    /// Source: the prerequisites the Amazon EKS User Guide lists for the add-on.
    pub(super) required_addons: &'static [&'static str],
    /// Whether the add-on serves an admission webhook the API server calls
    /// before it admits the resources of an add-on installed against this one.
    ///
    /// Source: the webhook key the add-on's own configuration schema declares
    /// ([`AddonCatalogSeed::configuration_properties`]).
    pub(super) admission_webhook: bool,
    /// Whether the add-on needs IAM permissions of its own
    /// (`AddonVersionInfo$requiresIamPermissions`).
    pub(super) requires_iam_permissions: bool,
    /// The kinds of compute Amazon EKS publishes THIS add-on's builds for, what
    /// `AddonVersionInfo$computeTypes` reports for every one of them
    /// ([`ADDON_COMPUTE_TYPES_EC2`] and the other published combinations).
    ///
    /// Source: the compute an add-on is offered on, which is a property of the
    /// add-on and of nothing else — the workloads its chart ships and the
    /// compute Amazon EKS supports them on. Every add-on runs on the EC2
    /// instances of a node group; the ones Amazon EKS builds and curates itself
    /// run on the nodes EKS Auto Mode provisions as well; AWS Fargate runs no
    /// DaemonSet, so only an add-on whose every workload is a Deployment is
    /// published for it; and `hybrid` is published for the add-ons a node joined
    /// by EKS Hybrid Nodes needs or reports through.
    pub(super) compute_types: &'static [&'static str],
    /// The configuration properties the add-on cannot be installed without.
    /// EMPTY for an add-on whose configuration values are optional, which is
    /// what makes `AddonVersionInfo$requiresConfiguration` false for it and
    /// what leaves the published configuration schema with no required
    /// property: the flag and the schema have this one source.
    pub(super) required_configuration: &'static [&'static str],
    /// The top-level keys this add-on's OWN configuration schema declares, each
    /// with the JSON type that key takes — published beside the keys every
    /// add-on's schema carries ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), and
    /// beside the ones the install itself takes (see
    /// [`AddonCatalogRecord::configuration_schema`]). A key named here with a
    /// type of its own OVERRIDES the common one.
    ///
    /// Source: the schema each add-on publishes, which is the set of values its
    /// own chart exposes — `aws eks describe-addon-configuration --addon-name
    /// <name> --addon-version <build>` reports it, and the Amazon EKS User
    /// Guide's configuration examples for an add-on set keys from this list
    /// (`aws-efs-csi-driver`'s `useFIPS` is the one the guide's FIPS install
    /// passes). Keys are written in the schema's own alphabetical order.
    pub(super) configuration_properties: &'static [AddonSchemaProperty],
    /// The Kubernetes service accounts the add-on runs as, each with the
    /// managed policies Amazon EKS recommends for it — what
    /// `DescribeAddonConfiguration$podIdentityConfiguration` reports ("The
    /// Kubernetes service account name used by the add-on, and any suggested
    /// IAM policies"). Empty for an add-on that runs as no service account of
    /// its own.
    pub(super) pod_identity: &'static [(&'static str, &'static [&'static str])],
    /// The JSON Schema dialect THIS add-on's published configuration document
    /// is written in ([`ADDON_SCHEMA_DIALECT_DRAFT_06`],
    /// [`ADDON_SCHEMA_DIALECT_DRAFT_07`] or [`ADDON_SCHEMA_DIALECT_2019_09`]).
    ///
    /// Source: the `$schema` keyword of the document `aws eks
    /// describe-addon-configuration --addon-name <name> --addon-version
    /// <build>` reports. The dialect belongs to the DOCUMENT, so it is the
    /// add-on's own: the charts Amazon EKS has published schemas for the longest
    /// declare draft-06, while the newer generation — the upstream projects it
    /// curates, the operators it has added since and the drivers whose documents
    /// it has reissued — declare draft-07 or the 2019-09 dialect, and a
    /// client that loads the document into a validator honours the keywords of
    /// the dialect that add-on published.
    ///
    /// It also decides which SHAPE the published document takes, because the two
    /// go together in the documents Amazon EKS publishes; see
    /// [`AddonCatalogRecord::configuration_schema_value`].
    pub(super) schema_dialect: &'static str,
    /// The OLDEST Kubernetes version in [`VERSION_CALENDAR`] this add-on has a
    /// published build for: the calendar row Amazon EKS first offered it on, or
    /// the oldest cluster version it still supports. `None` for an add-on
    /// published for every cluster version Amazon EKS offers add-ons on, whose
    /// line therefore starts at [`ADDON_CATALOG_FIRST_CLUSTER_VERSION`].
    ///
    /// An add-on offered later than a calendar row has NO build for that row,
    /// which is what makes `AddonVersionInfo$compatibilities` the add-on's own
    /// set rather than the service's version list: a cluster on a version older
    /// than this one cannot install the add-on, and the add-on drops out of a
    /// listing filtered to that version.
    ///
    /// Source: the Kubernetes versions the Amazon EKS User Guide's add-on
    /// compatibility tables list for the add-on.
    pub(super) first_cluster_version: Option<&'static str>,
    /// The add-on's OWN published build line, oldest release first: one release
    /// per Kubernetes minor version it has been published for since
    /// [`Self::first_cluster_version`], as `(major, minor, patch)`.
    ///
    /// An add-on's version tracks the ADD-ON, not the control plane, so each
    /// add-on carries its own line — and a line crosses a MAJOR version
    /// wherever the project released one, so the published history of such an
    /// add-on spans several majors rather than one. A line shorter than the
    /// rows it covers continues from its last release by one minor version per
    /// further row, so a calendar that grows a row still publishes a build for
    /// it.
    ///
    /// Source: the releases the add-on's project published, as the
    /// `DescribeAddonVersions` catalog lists them.
    pub(super) releases: &'static [(u32, u32, u32)],
}

/// The add-ons this service publishes.
///
/// The Amazon EKS-managed rows are the add-ons the "Amazon EKS add-ons" section
/// of the Amazon EKS User Guide publishes — the ones Amazon EKS builds itself
/// and the community projects it curates alongside them — with the name a caller
/// installs each by, the category it is published under, the namespace its
/// workloads run in, whether it needs IAM permissions of its own, EVERY service
/// account it runs as — a driver whose controller and node components both call
/// AWS carries one per component — with the managed policies the guide
/// recommends for each, and the keys its own configuration schema declares.
///
/// Each row carries the add-on's OWN owner: the ones Amazon Web Services builds
/// are owned by [`ADDON_OWNER_AWS`], the upstream projects Amazon EKS only
/// curates by [`ADDON_OWNER_COMMUNITY`], and the ones a caller subscribes to by
/// [`ADDON_OWNER_MARKETPLACE`] — which is what makes
/// `DescribeAddonVersions$owners` narrow the catalog. Amazon EKS builds and
/// publishes the EKS-managed and the community rows alike, so both are published
/// by [`ADDON_PUBLISHER_EKS`].
///
/// This table, beside the vendor rebuilds of its rows ([`ADDON_FIPS_REBUILDS`]),
/// is what the catalog PUBLISHES, so an add-on missing from both cannot be
/// discovered or installed at all: a row is the only way a caller reads an
/// add-on's builds, its compatibilities and its configuration surface.
///
/// The rest are AWS Marketplace add-ons ([`ADDON_OWNER_MARKETPLACE`]), published
/// by their VENDOR and carrying the `marketplaceInformation` of the listing they
/// are subscribed through. They are in the catalog because the owner, publisher
/// and `requiresConfiguration` an add-on reports are properties of the add-on,
/// not of Amazon EKS: a Marketplace add-on that cannot run without its vendor's
/// credentials is the kind that reports `requiresConfiguration` true, and the
/// configuration properties named here are the ones that vendor's own
/// documentation requires.
///
/// Rows are written in add-on NAME order, which is the order the catalog is
/// published in ([`seed_addon_catalog`]). The table is fixed, so it also bounds
/// this part of the store.
pub(super) const ADDON_CATALOG: &[AddonCatalogSeed] = &[
    AddonCatalogSeed {
        // AccuKnox's KubeArmor: the runtime security enforcement agent, a node
        // agent a caller subscribes to through AWS Marketplace.
        addon_name: "accuknox_kubearmor",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "accuknox",
        addon_type: "security",
        default_namespace: "kubearmor",
        requires_iam_permissions: false,
        // KubeArmor enforces policy on the cluster it runs on, so it needs no
        // vendor credentials to start: a Marketplace add-on requires
        // configuration only when it cannot run without it.
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("kubearmorConfig", "object"),
            ("kubearmorOperator", "object"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 4, 3)],
    },
    AddonCatalogSeed {
        addon_name: "adot",
        compute_types: ADDON_COMPUTE_TYPES_EVERY_COMPUTE,
        // The OpenTelemetry Operator, whose admission webhooks are served with
        // certificates cert-manager issues.
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "opentelemetry-operator-system",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("affinity", "object"),
            ("collector", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        // The operator's line stays inside its 0.x series.
        releases: &[(0, 102, 1)],
    },
    AddonCatalogSeed {
        // Akuity's agent, which connects the cluster to the Akuity Platform's
        // managed Argo CD control plane.
        addon_name: "akuity_akuity-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "akuity",
        addon_type: "kubernetes-management",
        default_namespace: "akuity",
        requires_iam_permissions: false,
        // The agent registers with ONE Akuity instance, which it cannot do
        // without that instance's id and its credentials.
        required_configuration: &["agentId", "agentToken"],
        pod_identity: &[],
        configuration_properties: &[("replicaCount", "integer"), ("server", "string")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(0, 24, 0)],
    },
    AddonCatalogSeed {
        addon_name: "amazon-cloudwatch-observability",
        compute_types: ADDON_COMPUTE_TYPES_EC2_HYBRID_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "amazon-cloudwatch",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "cloudwatch-agent",
            &[
                "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
                "arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess",
            ],
        )],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("agent", "object"),
            ("containerLogs", "object"),
            ("manager", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        // The agent and its operator crossed three major versions over these
        // Kubernetes releases, so the published history spans all of them.
        releases: &[
            (1, 5, 0),
            (2, 1, 0),
            (2, 5, 0),
            (3, 2, 0),
            (3, 6, 0),
            (4, 0, 0),
            (4, 2, 0),
            (4, 4, 0),
        ],
    },
    AddonCatalogSeed {
        // Amazon SageMaker HyperPod's task governance, which schedules the
        // training and inference workloads of a HyperPod cluster whose
        // orchestration runs on this cluster.
        addon_name: "amazon-sagemaker-hyperpod-taskgovernance",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // Its chart exposes the queue the governance controller schedules
        // through and how many replicas of that controller run, beside the
        // placement values every add-on's schema carries.
        configuration_properties: &[
            ("kueue", "object"),
            ("replicaCount", "integer"),
        ],
        // Its schema is one of the newer documents, written in the newer
        // dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        // Amazon EKS began offering task governance after the oldest Kubernetes
        // versions the calendar carries, so it has no build for them.
        first_cluster_version: Some("1.31"),
        releases: &[(1, 0, 0)],
    },
    AddonCatalogSeed {
        addon_name: "amazon-sagemaker-hyperpod-training-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &["cert-manager"],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "machine-learning",
        default_namespace: "aws-hyperpod",
        // The operator calls Amazon SageMaker for the training jobs it runs, so
        // it needs IAM permissions of its own.
        requires_iam_permissions: true,
        required_configuration: &[],
        // It runs as the controller manager its own schema names
        // (`hpTrainingControllerManager`), with the managed policy Amazon Web
        // Services publishes for the operator.
        pod_identity: &[(
            "hp-training-controller-manager",
            &["arn:aws:iam::aws:policy/AmazonSageMakerHyperPodTrainingOperatorAccess"],
        )],
        configuration_properties: &[
            ("hpTrainingControllerManager", "object"),
            ("nodeSelector", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        // Its published schema is written in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        // The newest add-on in the catalog: Amazon EKS offers it only from this
        // Kubernetes version on.
        first_cluster_version: Some("1.34"),
        releases: &[(1, 0, 0)],
    },
    AddonCatalogSeed {
        addon_name: "aws-ebs-csi-driver",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        // The CSI drivers run a controller Deployment beside their node
        // DaemonSet, so an install of one has a replica to place.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "ebs-csi-controller-sa",
            &["arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"],
        )],
        configuration_properties: &[
            ("additionalDaemonSets", "object"),
            ("controller", "object"),
            ("defaultStorageClass", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 31, 0)],
    },
    AddonCatalogSeed {
        // The Elastic Fabric Adapter device plugin, which advertises the EFA
        // network interfaces of a node to the scheduler.
        addon_name: "aws-efa-k8s-device-plugin",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        // The plugin is a DaemonSet that advertises a node's devices and takes
        // no settings of its own beyond the placement values every add-on's
        // schema carries.
        configuration_properties: &[],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(0, 5, 3)],
    },
    AddonCatalogSeed {
        addon_name: "aws-efs-csi-driver",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        // BOTH of the driver's components call Amazon EFS: the controller
        // Deployment provisions access points and the node DaemonSet resolves the
        // mount targets it mounts through, so the catalog carries an entry per
        // service account the driver runs as — not just the controller's — each
        // recommending the policy Amazon EKS publishes for the driver.
        pod_identity: &[
            (
                "efs-csi-controller-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
            (
                "efs-csi-node-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
        ],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("replicaCount", "integer"),
            ("sidecars", "object"),
            ("storageClasses", "array"),
            // The key the guide's FIPS install sets: an add-on's schema
            // declares the options that add-on itself takes.
            ("useFIPS", "boolean"),
        ],
        // The driver's document is one Amazon EKS has reissued in the 2019-09
        // dialect, which is the `$schema` it declares.
        schema_dialect: ADDON_SCHEMA_DIALECT_2019_09,
        first_cluster_version: None,
        // The driver released two new major versions over these Kubernetes
        // releases, so its published history spans majors 1, 2 and 3: a caller
        // pinning, upgrading or rolling back to a build of any of them finds it
        // in the catalog.
        releases: &[
            (1, 7, 6),
            (2, 0, 7),
            (2, 1, 4),
            (2, 1, 9),
            (3, 0, 3),
            (3, 1, 0),
            (3, 1, 2),
            (3, 2, 0),
        ],
    },
    AddonCatalogSeed {
        addon_name: "aws-guardduty-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "amazon-guardduty",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("priorityClassName", "string"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 6, 1)],
    },
    AddonCatalogSeed {
        addon_name: "aws-mountpoint-s3-csi-driver",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("controller", "object"),
            ("node", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        // The driver's second major version replaced its mount architecture, so
        // the published history carries both majors.
        releases: &[
            (1, 7, 0),
            (1, 9, 0),
            (1, 11, 0),
            (1, 14, 1),
            (2, 0, 0),
            (2, 1, 0),
            (2, 2, 0),
            (2, 3, 0),
        ],
    },
    AddonCatalogSeed {
        // The agent that reports the cluster's network flows to Amazon
        // CloudWatch Network Flow Monitor, which it publishes to with
        // permissions of its own.
        addon_name: "aws-network-flow-monitoring-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "aws-network-flow-monitor",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "aws-network-flow-monitor-agent",
            &["arn:aws:iam::aws:policy/CloudWatchNetworkFlowMonitorAgentPublishPolicy"],
        )],
        // Its chart exposes the agent itself; the rest of what a cluster
        // operator sets on it are the placement values every add-on's schema
        // carries.
        configuration_properties: &[("agent", "object")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        // The agent was published alongside Amazon CloudWatch Network Flow
        // Monitor, after the oldest Kubernetes version the calendar carries.
        first_cluster_version: Some("1.31"),
        releases: &[(1, 0, 1)],
    },
    AddonCatalogSeed {
        addon_name: "cert-manager",
        compute_types: ADDON_COMPUTE_TYPES_EVERY_COMPUTE,
        // Its controller, cainjector and webhook are Deployments, and the
        // webhook is the one the API server calls for the resources of the
        // add-ons installed against it.
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        // An upstream project Amazon EKS curates rather than builds, so the
        // add-on is the community's, not Amazon Web Services'.
        owner: ADDON_OWNER_COMMUNITY,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: "cert-manager",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("cainjector", "object"),
            ("installCRDs", "boolean"),
            ("prometheus", "object"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("webhook", "object"),
        ],
        // The community charts' schemas are published in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        first_cluster_version: None,
        // The project's own 1.x line, one release per Kubernetes version it has
        // been published for.
        releases: &[
            (1, 14, 2),
            (1, 15, 3),
            (1, 16, 2),
            (1, 17, 1),
            (1, 18, 2),
            (1, 19, 0),
            (1, 20, 1),
            (1, 21, 2),
        ],
    },
    AddonCatalogSeed {
        addon_name: "coredns",
        compute_types: ADDON_COMPUTE_TYPES_EVERY_COMPUTE,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("autoScaling", "object"),
            ("computeType", "string"),
            ("corefile", "string"),
            ("replicaCount", "integer"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 11, 1)],
    },
    AddonCatalogSeed {
        addon_name: "datadog_operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "datadog",
        addon_type: "monitoring",
        default_namespace: "datadog-agent",
        requires_iam_permissions: false,
        // The Datadog Operator sends telemetry to a Datadog organisation, which
        // it cannot do without that organisation's API key.
        required_configuration: &["apiKey"],
        pod_identity: &[],
        configuration_properties: &[
            ("clusterName", "string"),
            ("resources", "object"),
            ("site", "string"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 8, 0)],
    },
    AddonCatalogSeed {
        addon_name: "dynatrace_dynatrace-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "dynatrace",
        addon_type: "monitoring",
        default_namespace: "dynatrace",
        requires_iam_permissions: false,
        // The Dynatrace Operator connects to one Dynatrace environment, which it
        // cannot do without that environment's API URL and token.
        required_configuration: &["apiUrl", "apiToken"],
        pod_identity: &[],
        configuration_properties: &[("resources", "object"), ("tolerations", "array")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 3, 0)],
    },
    AddonCatalogSeed {
        // eG Innovations' universal agent operator, which installs the eG agents
        // that report the cluster's workloads to an eG Manager.
        addon_name: "eg-innovations_eg-universal-agent-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "eg-innovations",
        addon_type: "monitoring",
        default_namespace: "eg-agents",
        requires_iam_permissions: false,
        // The agents report to one eG Manager, which they cannot reach without
        // that manager's address and the licence it authorises them with.
        required_configuration: &["managerUrl", "licenseKey"],
        pod_identity: &[],
        configuration_properties: &[("agent", "object"), ("resources", "object")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 0, 2)],
    },
    AddonCatalogSeed {
        addon_name: "eks-node-monitoring-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2_HYBRID_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        // The agent was published alongside node auto repair, after the oldest
        // Kubernetes version the calendar carries.
        first_cluster_version: Some("1.31"),
        releases: &[(1, 0, 0)],
    },
    AddonCatalogSeed {
        addon_name: "eks-pod-identity-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2_HYBRID_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "security",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("agent", "object"),
            ("nodeSelector", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 2, 0)],
    },
    AddonCatalogSeed {
        addon_name: "external-dns",
        compute_types: ADDON_COMPUTE_TYPES_EC2_FARGATE_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        // An upstream project Amazon EKS curates rather than builds.
        owner: ADDON_OWNER_COMMUNITY,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: "external-dns",
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[(
            "external-dns",
            &["arn:aws:iam::aws:policy/AmazonRoute53FullAccess"],
        )],
        configuration_properties: &[
            ("domainFilters", "array"),
            ("policy", "string"),
            ("provider", "string"),
            ("resources", "object"),
            ("sources", "array"),
            ("txtOwnerId", "string"),
        ],
        // The community charts' schemas are published in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        first_cluster_version: None,
        releases: &[(0, 15, 0)],
    },
    AddonCatalogSeed {
        // HAProxy Technologies' enterprise Kubernetes ingress controller.
        addon_name: "haproxy-technologies_kubernetes-ingress-ee",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "haproxy-technologies",
        addon_type: "networking",
        default_namespace: "haproxy-controller",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("controller", "object"),
            ("defaultBackend", "object"),
            ("replicaCount", "integer"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 11, 0)],
    },
    AddonCatalogSeed {
        // Kong's Konnect reference implementation: the Kong gateway, managed from
        // a Konnect control plane.
        addon_name: "kong_konnect-ri",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "kong",
        addon_type: "networking",
        default_namespace: "kong",
        requires_iam_permissions: false,
        // The gateway is managed from one Konnect control plane, which it cannot
        // join without that control plane's endpoint and certificate.
        required_configuration: &["clusterControlPlane", "clusterCertificate"],
        pod_identity: &[],
        configuration_properties: &[("gateway", "object"), ("replicaCount", "integer")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(2, 46, 0)],
    },
    AddonCatalogSeed {
        addon_name: "kube-proxy",
        compute_types: ADDON_COMPUTE_TYPES_EC2_HYBRID_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("affinity", "object"),
            ("nodeSelector", "object"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        // kube-proxy is the one add-on whose version IS the Kubernetes version
        // it runs with, so its line starts at the oldest calendar row's version
        // and steps with the calendar.
        releases: &[(1, 30, 0)],
    },
    AddonCatalogSeed {
        addon_name: "kube-state-metrics",
        compute_types: ADDON_COMPUTE_TYPES_EVERY_COMPUTE,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        // An upstream project Amazon EKS curates rather than builds.
        owner: ADDON_OWNER_COMMUNITY,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("collectors", "array"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        // The community charts' schemas are published in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        first_cluster_version: None,
        releases: &[(2, 13, 0)],
    },
    AddonCatalogSeed {
        addon_name: "kubecost_kubecost",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "kubecost",
        addon_type: "cost-management",
        default_namespace: "kubecost",
        requires_iam_permissions: false,
        // Kubecost reports on the cluster it runs in, so it needs no vendor
        // credentials: a Marketplace add-on requires configuration only when it
        // cannot run without it.
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("global", "object"),
            ("kubecostToken", "string"),
            ("persistentVolume", "object"),
            ("prometheus", "object"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(2, 3, 0)],
    },
    AddonCatalogSeed {
        addon_name: "metrics-server",
        compute_types: ADDON_COMPUTE_TYPES_EVERY_COMPUTE,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        // An upstream project Amazon EKS curates rather than builds.
        owner: ADDON_OWNER_COMMUNITY,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("args", "array"),
            ("nodeSelector", "object"),
            ("replicas", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
        ],
        // The community charts' schemas are published in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        first_cluster_version: None,
        releases: &[(0, 7, 0)],
    },
    AddonCatalogSeed {
        // NetApp's Trident operator, which installs the Trident CSI driver for
        // NetApp storage.
        addon_name: "netapp_trident-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "netapp",
        addon_type: "storage",
        default_namespace: "trident",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("cloudProvider", "string"),
            ("tridentDebug", "boolean"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(100, 2, 0)],
    },
    AddonCatalogSeed {
        addon_name: "prometheus-node-exporter",
        compute_types: ADDON_COMPUTE_TYPES_EC2_HYBRID_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        // An upstream project Amazon EKS curates rather than builds.
        owner: ADDON_OWNER_COMMUNITY,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "observability",
        default_namespace: "prometheus-node-exporter",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("hostNetwork", "boolean"),
            ("resources", "object"),
            ("service", "object"),
            ("tolerations", "array"),
        ],
        // The community charts' schemas are published in the newer dialect.
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_07,
        first_cluster_version: None,
        releases: &[(1, 8, 2)],
    },
    AddonCatalogSeed {
        // Rafay Systems' operator, which registers the cluster with the Rafay
        // controller that manages it.
        addon_name: "rafay-systems_rafay-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "rafay-systems",
        addon_type: "kubernetes-management",
        default_namespace: "rafay-system",
        requires_iam_permissions: false,
        // The operator reports to one Rafay controller, which it cannot register
        // with without that controller's address and the project's credentials.
        required_configuration: &["controllerAddress", "apiKey"],
        pod_identity: &[],
        configuration_properties: &[("clusterName", "string"), ("resources", "object")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 1, 0)],
    },
    AddonCatalogSeed {
        addon_name: "snapshot-controller",
        compute_types: ADDON_COMPUTE_TYPES_EC2_FARGATE_AUTO,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "storage",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("tolerations", "array"),
            ("volumeSnapshotClasses", "array"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(8, 1, 0)],
    },
    AddonCatalogSeed {
        // Solo.io's Istio distribution: the service mesh's control plane, built
        // and supported by Solo.io.
        addon_name: "solo-io_istio-distro",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "solo-io",
        addon_type: "networking",
        default_namespace: "istio-system",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("global", "object"),
            ("pilot", "object"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 23, 0)],
    },
    AddonCatalogSeed {
        // Splunk's OpenTelemetry collector chart, which ships the cluster's
        // metrics, traces and logs to a Splunk Observability realm.
        addon_name: "splunk_splunk-otel-collector-chart",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "splunk",
        addon_type: "observability",
        default_namespace: "splunk-otel-collector",
        requires_iam_permissions: false,
        // The collector sends to one Splunk realm, which it cannot reach without
        // that realm and its access token.
        required_configuration: &["splunkRealm", "splunkAccessToken"],
        pod_identity: &[],
        configuration_properties: &[
            ("agent", "object"),
            ("clusterName", "string"),
            ("gateway", "object"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(0, 111, 0)],
    },
    AddonCatalogSeed {
        // StormForge Optimize Live, which right-sizes the cluster's workloads
        // from the recommendations its own platform computes.
        addon_name: "stormforge_optimize-live",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "stormforge",
        addon_type: "cost-management",
        default_namespace: "stormforge-system",
        requires_iam_permissions: false,
        // The agent reports to one StormForge organisation, which it cannot do
        // without that organisation's client credentials.
        required_configuration: &["clusterName", "stormforgeClientId"],
        pod_identity: &[],
        configuration_properties: &[("agent", "object"), ("resources", "object")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(2, 11, 0)],
    },
    AddonCatalogSeed {
        // Teleport's Kubernetes agent, which joins the cluster to a Teleport
        // cluster so access to it goes through Teleport.
        addon_name: "teleport_teleport-kube-agent",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "teleport",
        addon_type: "security",
        default_namespace: "teleport-agent",
        requires_iam_permissions: false,
        // The agent joins ONE Teleport cluster, which it cannot do without that
        // cluster's proxy address and a join token.
        required_configuration: &["proxyAddr", "authToken"],
        pod_identity: &[],
        configuration_properties: &[("roles", "string"), ("kubeClusterName", "string")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(16, 4, 0)],
    },
    AddonCatalogSeed {
        // Tetrate's Istio distribution, the hardened build of the mesh Tetrate
        // supports.
        addon_name: "tetrate-io_istio-distro",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: true,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "tetrate-io",
        addon_type: "networking",
        default_namespace: "istio-system",
        requires_iam_permissions: false,
        required_configuration: &[],
        pod_identity: &[],
        configuration_properties: &[
            ("admissionWebhooks", "object"),
            ("global", "object"),
            ("profile", "string"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 23, 0)],
    },
    AddonCatalogSeed {
        // Upbound's Universal Crossplane, which reconciles the cloud
        // infrastructure the cluster's own resources declare.
        addon_name: "upbound_universal-crossplane",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "upbound",
        addon_type: "infra-management",
        default_namespace: "upbound-system",
        requires_iam_permissions: true,
        required_configuration: &[],
        // Crossplane's AWS providers call AWS for the infrastructure they
        // reconcile, so the add-on runs as a service account of its own.
        pod_identity: &[(
            "crossplane",
            &["arn:aws:iam::aws:policy/AdministratorAccess"],
        )],
        configuration_properties: &[("provider", "object"), ("replicas", "integer")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 17, 0)],
    },
    AddonCatalogSeed {
        // Upwind Security's operator, which runs its runtime sensors on the
        // cluster's nodes.
        addon_name: "upwind-security_upwind-operator",
        compute_types: ADDON_COMPUTE_TYPES_EC2,
        workload: AddonWorkload::Replicated,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_MARKETPLACE,
        publisher: "upwind-security",
        addon_type: "security",
        default_namespace: "upwind",
        requires_iam_permissions: false,
        // The sensors report to one Upwind organisation, which they cannot do
        // without that organisation's client credentials.
        required_configuration: &["upwindClientId", "upwindClientSecret"],
        pod_identity: &[],
        configuration_properties: &[("sensor", "object"), ("resources", "object")],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 3, 0)],
    },
    AddonCatalogSeed {
        addon_name: "vpc-cni",
        compute_types: ADDON_COMPUTE_TYPES_EC2_AUTO,
        workload: AddonWorkload::PerNode,
        required_addons: &[],
        admission_webhook: false,
        owner: ADDON_OWNER_AWS,
        publisher: ADDON_PUBLISHER_EKS,
        addon_type: "networking",
        default_namespace: ADDON_DEFAULT_NAMESPACE,
        requires_iam_permissions: true,
        required_configuration: &[],
        pod_identity: &[("aws-node", &["arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"])],
        configuration_properties: &[
            ("branchENICooldown", "integer"),
            // `enableNetworkPolicy` and `enableWindowsIpam` are STRINGS in this
            // add-on's schema, not booleans: the key a client sets is the one
            // the schema declares, with the type the schema gives it.
            ("enableNetworkPolicy", "string"),
            ("enableWindowsIpam", "string"),
            ("eniConfig", "object"),
            ("env", "object"),
            ("init", "object"),
            ("nodeAgent", "object"),
        ],
        schema_dialect: ADDON_SCHEMA_DIALECT_DRAFT_06,
        first_cluster_version: None,
        releases: &[(1, 18, 0)],
    },
];

/// The vendor that publishes the FIPS rebuilds [`ADDON_FIPS_REBUILDS`] names,
/// the value `DescribeAddonVersions$publishers` filters them by.
pub(super) const ADDON_PUBLISHER_FIPS_REBUILDS: &str = "chainguard";

/// The AWS Marketplace add-ons that are a VENDOR'S REBUILD of an add-on
/// [`ADDON_CATALOG`] already publishes, as `(the name the rebuild is sold under,
/// the add-on it rebuilds)`.
///
/// Source: the AWS Marketplace add-on listings Amazon EKS publishes for the
/// FIPS-validated builds of its add-ons, whose `<vendor>_<add-on>-fips` names
/// are what `CreateAddon$addonName` takes. A caller installing a cluster that
/// must use validated cryptography installs these INSTEAD of the Amazon
/// EKS-managed add-ons of the same software, which is why both are published.
///
/// A rebuild is the SAME software built by a vendor rather than by Amazon EKS,
/// so what the catalog publishes about it is the rebuilt add-on's own — its
/// category, the namespace its workloads run in, how its pods are placed, the
/// add-ons its resources are applied against, the IAM permissions and service
/// accounts it needs, and its whole configuration surface — under the vendor's
/// name, owned by [`ADDON_OWNER_MARKETPLACE`] and carrying the
/// `marketplaceInformation` of its own listing (see [`published_addon_seeds`]).
/// Writing the rebuilt add-on's name here rather than repeating its row is what
/// keeps the two from publishing different surfaces for one piece of software.
pub(super) const ADDON_FIPS_REBUILDS: &[(&str, &str)] = &[
    ("chainguard_aws-ebs-csi-driver-fips", "aws-ebs-csi-driver"),
    ("chainguard_aws-efs-csi-driver-fips", "aws-efs-csi-driver"),
    ("chainguard_cert-manager-fips", "cert-manager"),
    ("chainguard_coredns-fips", "coredns"),
    ("chainguard_external-dns-fips", "external-dns"),
    ("chainguard_kube-proxy-fips", "kube-proxy"),
    ("chainguard_kube-state-metrics-fips", "kube-state-metrics"),
    ("chainguard_metrics-server-fips", "metrics-server"),
    (
        "chainguard_prometheus-node-exporter-fips",
        "prometheus-node-exporter",
    ),
    ("chainguard_vpc-cni-fips", "vpc-cni"),
];

/// The add-on `addon_name` is a vendor's rebuild of, when the catalog publishes
/// it as one ([`ADDON_FIPS_REBUILDS`]); `None` for every add-on that is published
/// in its own right.
pub(super) fn rebuilt_addon(addon_name: &str) -> Option<&'static str> {
    ADDON_FIPS_REBUILDS
        .iter()
        .find(|(rebuild, _)| *rebuild == addon_name)
        .map(|(_, rebuilt)| *rebuilt)
}

/// Every add-on this service publishes, in add-on NAME order — the order the
/// catalog is published in: the rows [`ADDON_CATALOG`] writes out, and the
/// Marketplace rebuilds [`ADDON_FIPS_REBUILDS`] names, each taking the published
/// surface of the add-on it rebuilds under its vendor's name.
///
/// A rebuild of a name the catalog does not publish is a typo in the table,
/// which is why it panics here — at seed time, with both names — rather than
/// quietly publishing nothing for it.
fn published_addon_seeds() -> Vec<AddonCatalogSeed> {
    let mut seeds: Vec<AddonCatalogSeed> = ADDON_CATALOG.to_vec();
    for &(addon_name, rebuilt) in ADDON_FIPS_REBUILDS {
        let published = ADDON_CATALOG
            .iter()
            .find(|seed| seed.addon_name == rebuilt)
            .unwrap_or_else(|| {
                panic!(
                    "the add-on catalog publishes {addon_name} as a rebuild of {rebuilt}, \
                     which it does not publish"
                )
            });
        seeds.push(AddonCatalogSeed {
            addon_name,
            owner: ADDON_OWNER_MARKETPLACE,
            publisher: ADDON_PUBLISHER_FIPS_REBUILDS,
            ..*published
        });
    }
    seeds.sort_by(|left, right| left.addon_name.cmp(right.addon_name));
    seeds
}

/// One Kubernetes version a published add-on build supports, as the catalog
/// holds it: the modelled `Compatibility`.
#[derive(Debug, Clone)]
pub struct AddonCompatibilityRecord {
    /// The Kubernetes minor version this build supports, a
    /// [`VERSION_CALENDAR`] row.
    pub cluster_version: String,
    /// The platform versions of that cluster version the build runs on
    /// ([`ADDON_ANY_PLATFORM_VERSION`]).
    pub platform_versions: Vec<String>,
}

impl AddonCompatibilityRecord {
    /// This row as the modelled `Compatibility` shape.
    ///
    /// `default_version` is whether the build this row belongs to is the one
    /// Amazon EKS installs on a cluster of that Kubernetes version when the
    /// caller names no version. It is NOT stored on the row: which build that is
    /// depends on which builds the clock has reached, so it is decided per read
    /// (see [`AddonCatalogRecord::to_addon_version_infos`]) and cannot go stale.
    pub fn to_compatibility(&self, default_version: bool) -> Compatibility {
        Compatibility {
            cluster_version: Some(self.cluster_version.clone()),
            platform_versions: Some(self.platform_versions.clone()),
            default_version,
        }
    }
}

/// One published build of one add-on, as the catalog holds it: the modelled
/// `AddonVersionInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogVersionRecord {
    /// The build's version, in the `vX.Y.Z-eksbuild.N` form every Amazon EKS
    /// add-on version takes.
    pub addon_version: String,
    /// When Amazon EKS published this build — the release date of the
    /// Kubernetes version it was published for. A build the clock has not
    /// reached is not one the catalog offers yet, exactly as an unreleased
    /// Kubernetes version is not ([`ClusterVersionRecord::is_released`]).
    pub published_at: SystemTime,
    /// The architectures the build is published for ([`ADDON_ARCHITECTURES`]).
    pub architecture: Vec<String>,
    /// The kinds of compute the build is published for; see
    /// [`AddonCatalogSeed::compute_types`].
    pub compute_types: Vec<String>,
    /// The Kubernetes versions the build supports, NEWEST FIRST — one row per
    /// Kubernetes minor version, which is the order and the granularity the
    /// catalog publishes an add-on build's compatibilities in.
    pub compatibilities: Vec<AddonCompatibilityRecord>,
    /// The configuration properties the build cannot be installed without; see
    /// [`AddonCatalogSeed::required_configuration`].
    pub required_configuration: Vec<String>,
    /// Whether the build needs IAM permissions of its own.
    pub requires_iam_permissions: bool,
}

impl AddonCatalogVersionRecord {
    /// Whether Amazon EKS has published this build at `now`.
    pub fn is_published(&self, now: SystemTime) -> bool {
        now >= self.published_at
    }

    /// Whether this build supports a Kubernetes version.
    pub fn supports(&self, cluster_version: &str) -> bool {
        self.compatibilities
            .iter()
            .any(|entry| entry.cluster_version == cluster_version)
    }

    /// Whether this build cannot be installed without configuration values —
    /// `AddonVersionInfo$requiresConfiguration`, which is exactly "this build
    /// has a required configuration property".
    pub fn requires_configuration(&self) -> bool {
        !self.required_configuration.is_empty()
    }

    /// This build as the modelled `AddonVersionInfo` shape.
    ///
    /// `cluster_version`, when given, is the Kubernetes version the listing was
    /// filtered to: the reported compatibilities are then the ones for that
    /// version alone, because a filtered listing answers about that version.
    ///
    /// `is_default_for` answers, for one Kubernetes version this build supports,
    /// whether this build is the DEFAULT build for it — the question the add-on
    /// as a whole decides, since it is about the other builds of the same add-on
    /// (see [`AddonCatalogRecord::to_addon_version_infos`]).
    pub fn to_addon_version_info(
        &self,
        cluster_version: Option<&str>,
        is_default_for: impl Fn(&str) -> bool,
    ) -> AddonVersionInfo {
        AddonVersionInfo {
            addon_version: Some(self.addon_version.clone()),
            architecture: Some(self.architecture.clone()),
            // "Indicates the compute type of the add-on version": the kinds of
            // compute Amazon EKS publishes this build for, which the catalog
            // carries per add-on ([`AddonCatalogSeed::compute_types`]).
            compute_types: Some(self.compute_types.clone()),
            compatibilities: Some(
                self.compatibilities
                    .iter()
                    .filter(|entry| {
                        cluster_version.is_none_or(|wanted| wanted == entry.cluster_version)
                    })
                    .map(|entry| entry.to_compatibility(is_default_for(&entry.cluster_version)))
                    .collect(),
            ),
            requires_configuration: self.requires_configuration(),
            requires_iam_permissions: self.requires_iam_permissions,
        }
    }
}

/// One add-on the catalog publishes, with every build of it: the modelled
/// `AddonInfo`.
#[derive(Debug, Clone)]
pub struct AddonCatalogRecord {
    pub addon_name: String,
    pub addon_type: String,
    pub owner: String,
    pub publisher: String,
    pub default_namespace: String,
    /// How this add-on's pods are placed; see [`AddonCatalogSeed::workload`].
    pub workload: AddonWorkload,
    /// The add-ons this one's resources are applied against; see
    /// [`AddonCatalogSeed::required_addons`].
    pub required_addons: Vec<String>,
    /// Whether this add-on serves an admission webhook; see
    /// [`AddonCatalogSeed::admission_webhook`].
    pub admission_webhook: bool,
    /// The AWS Marketplace listing this add-on is subscribed through, for an
    /// add-on owned by [`ADDON_OWNER_MARKETPLACE`]; `None` for an Amazon
    /// EKS-managed one, which is not listed on Marketplace at all.
    pub marketplace_information: Option<MarketplaceInformation>,
    /// The service accounts the add-on runs as, with the managed policies
    /// recommended for each; see [`AddonCatalogSeed::pod_identity`].
    pub pod_identity: Vec<(String, Vec<String>)>,
    /// The keys this add-on's configuration schema declares, each with the JSON
    /// type it takes: its own ([`AddonCatalogSeed::configuration_properties`])
    /// merged with the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), in the schema's own
    /// alphabetical order.
    pub configuration_properties: Vec<(String, String)>,
    /// The JSON Schema dialect this add-on's own published configuration
    /// document is written in; see [`AddonCatalogSeed::schema_dialect`].
    pub schema_dialect: String,
    /// Every build of this add-on, NEWEST FIRST — the order the catalog
    /// publishes versions in, so a caller reading it to pick a version sees the
    /// newest build at the head of the list.
    pub versions: Vec<AddonCatalogVersionRecord>,
}

impl AddonCatalogRecord {
    /// The builds of this add-on Amazon EKS has published at `now`, newest
    /// first.
    pub fn published_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &AddonCatalogVersionRecord> + '_ {
        self.versions
            .iter()
            .filter(move |version| version.is_published(now))
    }

    /// This add-on as the modelled `AddonInfo` shape, reporting `versions`.
    pub fn to_addon_info(&self, versions: Vec<AddonVersionInfo>) -> AddonInfo {
        AddonInfo {
            addon_name: Some(self.addon_name.clone()),
            r#type: Some(self.addon_type.clone()),
            addon_versions: Some(versions),
            publisher: Some(self.publisher.clone()),
            owner: Some(self.owner.clone()),
            marketplace_information: self.marketplace_information.clone(),
            default_namespace: Some(self.default_namespace.clone()),
        }
    }

    /// The builds of this add-on a listing reports at `now`, newest first — the
    /// modelled `AddonVersionInfo` list of its `AddonInfo`.
    ///
    /// `cluster_version`, when given, is the Kubernetes version the listing was
    /// filtered to: only the builds that support it are reported, each with its
    /// compatibility for that version alone.
    ///
    /// Each compatibility's `defaultVersion` marks the build Amazon EKS installs
    /// on a cluster of that Kubernetes version when the caller names no
    /// `addonVersion`, which is the NEWEST published build that supports it: a
    /// superseded build is the default for no Kubernetes version, and whichever
    /// version the listing is filtered to, the flag is the same one the
    /// unfiltered listing reports, because it is decided against every published
    /// build of the add-on rather than against the ones this answer carries.
    pub fn to_addon_version_infos(
        &self,
        now: SystemTime,
        cluster_version: Option<&str>,
    ) -> Vec<AddonVersionInfo> {
        let published: Vec<&AddonCatalogVersionRecord> = self.published_versions(now).collect();
        published
            .iter()
            .enumerate()
            .filter(|(_, build)| cluster_version.is_none_or(|wanted| build.supports(wanted)))
            .map(|(position, build)| {
                // The builds published AFTER this one, which are the ones that
                // take the default away from it: `published` is newest first, so
                // they are the ones before it.
                let newer = &published[..position];
                build.to_addon_version_info(cluster_version, |minor| {
                    !newer.iter().any(|newer| newer.supports(minor))
                })
            })
            .collect()
    }

    /// The service accounts this add-on runs as, as the modelled
    /// `AddonPodIdentityConfiguration` list.
    pub fn to_pod_identity_configuration(&self) -> Vec<AddonPodIdentityConfiguration> {
        self.pod_identity
            .iter()
            .map(|(service_account, policies)| AddonPodIdentityConfiguration {
                service_account: Some(service_account.clone()),
                recommended_managed_policies: Some(policies.clone()),
            })
            .collect()
    }

    /// The JSON schema `DescribeAddonConfiguration` publishes for one build of
    /// this add-on: "A JSON schema that's used to validate the configuration
    /// values you provide when an add-on is created or updated".
    ///
    /// The properties it declares are THIS add-on's own configurable keys
    /// ([`AddonCatalogSeed::configuration_properties`], each with the JSON type
    /// that key takes) and the ones every add-on's schema carries
    /// ([`ADDON_COMMON_CONFIGURATION_PROPERTIES`]), beside the two the install
    /// itself takes — the namespace the add-on is installed into and the service
    /// account it runs as — and the properties the build cannot be installed
    /// without. A document may set no other key, which is what
    /// `additionalProperties: false` states, so a client reading the schema
    /// learns exactly the configuration surface of this add-on rather than a
    /// document shared by every one of them. Its `required` list is the build's
    /// own
    /// [`AddonCatalogVersionRecord::required_configuration`], which is also what
    /// makes [`AddonCatalogVersionRecord::requires_configuration`] true: a build
    /// that requires configuration publishes a schema that requires it, and one
    /// that does not publishes a schema with nothing required.
    ///
    /// The document is the ADD-ON's own, so the dialect it declares is the
    /// add-on's own too ([`AddonCatalogSeed::schema_dialect`]): a client that
    /// loads the schema into a validator reads the dialect that add-on published
    /// rather than one fixed for the whole service, which is what decides which
    /// keywords that validator honours.
    pub fn configuration_schema(&self, version: &AddonCatalogVersionRecord) -> String {
        self.configuration_schema_value(version).to_string()
    }

    /// That same published schema as the JSON document it is, which is what
    /// [`Self::configuration_violation`] holds a configuration document to:
    /// ONE source, with no serialise-and-parse round trip between the schema a
    /// client reads and the schema a create is judged by.
    ///
    /// The document has the shape Amazon EKS publishes THIS add-on's schema in,
    /// and which of the two shapes that is goes with the dialect the add-on's
    /// document declares ([`AddonCatalogSeed::schema_dialect`]), because the two
    /// are properties of the same published document:
    ///
    /// * the documents written in the draft dialects are a ROOT `$ref` into the
    ///   document's own `definitions`, whose named definition is the add-on's
    ///   chart values type, with one further definition per object-valued key
    ///   that type carries ([`schema_definition_title`] names them). A key whose
    ///   value is an object therefore reads as
    ///   `{"$ref": "#/definitions/<Type>"}` and the type it names declares that
    ///   object's own keys ([`nested_configuration_properties`]);
    /// * the ones written in the 2019-09 dialect ([`ADDON_SCHEMA_DIALECT_2019_09`])
    ///   declare the add-on's keys at the document ROOT, under `properties`, with
    ///   each object-valued key's own keys declared inline under it and no
    ///   `definitions` section at all, beside the `description` of what the
    ///   document configures.
    ///
    /// Either way a client that loads the document into a validator walks the
    /// same keys Amazon EKS publishes for the add-on, and finds them where that
    /// add-on's own document puts them.
    pub(super) fn configuration_schema_value(
        &self,
        version: &AddonCatalogVersionRecord,
    ) -> ::serde_json::Value {
        // Whether this add-on's document carries its types in a `definitions`
        // section a root `$ref` points into, or declares them at the root.
        let references_definitions = self.schema_dialect != ADDON_SCHEMA_DIALECT_2019_09;
        let mut definitions = ::serde_json::Map::new();
        let mut properties = ::serde_json::Map::new();
        properties.insert(
            "namespace".to_string(),
            ::serde_json::json!({
                "type": "string",
                "default": self.default_namespace,
                "description": "The Kubernetes namespace the add-on is installed into.",
            }),
        );
        if let Some((service_account, _)) = self.pod_identity.first() {
            let title = schema_definition_title("serviceAccount");
            let declared = ::serde_json::json!({
                "type": "object",
                "title": title,
                "properties": {
                    "name": {
                        "type": "string",
                        "default": service_account,
                        "description":
                            "The Kubernetes service account the add-on runs as.",
                    },
                },
            });
            if references_definitions {
                definitions.insert(title.clone(), declared);
                properties.insert("serviceAccount".to_string(), definition_ref(&title));
            } else {
                properties.insert("serviceAccount".to_string(), declared);
            }
        }
        for (property, json_type) in &self.configuration_properties {
            if json_type == "object" {
                let title = schema_definition_title(property);
                let nested = self.nested_properties(property);
                if references_definitions {
                    definitions
                        .entry(title.clone())
                        .or_insert_with(|| object_definition(&title, nested));
                    properties.insert(property.clone(), definition_ref(&title));
                } else {
                    properties.insert(property.clone(), object_definition(&title, nested));
                }
            } else {
                properties.insert(
                    property.clone(),
                    ::serde_json::json!({ "type": json_type }),
                );
            }
        }
        for property in &version.required_configuration {
            // A required key keeps what the add-on's own schema declares it as —
            // only the "you cannot install without this" description is added, and
            // only to a key declared as a scalar — so a required property of a type
            // other than string cannot be mis-declared here and then refused when
            // a caller sets it. A key that reads as a `$ref`, or that declares an
            // object's own keys, is left as it is: a `$ref`'s neighbouring keywords
            // are not read in these dialects, and overwriting the other would drop
            // the keys that object declares.
            let declared = properties.get(property);
            if declared.is_some_and(|declared| {
                declared.get("$ref").is_some() || declared.get("properties").is_some()
            }) {
                continue;
            }
            let json_type = declared
                .and_then(|declared| declared.get("type"))
                .and_then(::serde_json::Value::as_str)
                .unwrap_or("string")
                .to_string();
            properties.insert(
                property.clone(),
                ::serde_json::json!({
                    "type": json_type,
                    "description": format!(
                        "Required to install {}: the add-on cannot run without it.",
                        self.addon_name
                    ),
                }),
            );
        }
        if !references_definitions {
            return ::serde_json::json!({
                "$schema": self.schema_dialect,
                "description": format!("Configurable parameters for the {}", self.addon_name),
                "type": "object",
                "properties": ::serde_json::Value::Object(properties),
                "required": version.required_configuration,
                // A key the schema does not declare is not a key this add-on
                // takes, so a document that sets one does not satisfy the schema.
                "additionalProperties": false,
            });
        }
        let root = schema_definition_title(&self.addon_name);
        definitions.insert(
            root.clone(),
            ::serde_json::json!({
                "type": "object",
                "title": root,
                "properties": ::serde_json::Value::Object(properties),
                "required": version.required_configuration,
                // A key the schema does not declare is not a key this add-on
                // takes, so a document that sets one does not satisfy the schema.
                "additionalProperties": false,
            }),
        );
        ::serde_json::json!({
            "$schema": self.schema_dialect,
            "$ref": format!("{JSON_SCHEMA_DEFINITION_PREFIX}{root}"),
            "definitions": ::serde_json::Value::Object(definitions),
        })
    }

    /// The keys the object-valued configuration key `property` of THIS add-on
    /// declares; EMPTY for one whose own keys the catalog does not publish, which
    /// is published as an object with no declared key rather than as one with
    /// none allowed. See [`nested_configuration_properties`].
    fn nested_properties(&self, property: &str) -> &'static [AddonSchemaProperty] {
        nested_configuration_properties(&self.addon_name, property)
    }

    /// How `document` fails the configuration schema this entry publishes for
    /// `version` — or `None` when it satisfies that schema.
    ///
    /// "The values that you provide are validated against the schema returned by
    /// DescribeAddonConfiguration" (`CreateAddon$configurationValues`), so this
    /// validates against THAT document — the one
    /// [`Self::configuration_schema_value`] builds and
    /// [`Self::configuration_schema`] publishes — which is what keeps what the
    /// schema declares and what a document is held to from ever disagreeing.
    pub fn configuration_violation(
        &self,
        version: &AddonCatalogVersionRecord,
        document: &::serde_json::Value,
    ) -> Option<String> {
        json_schema_violation(
            JSON_SCHEMA_ROOT_PATH,
            &self.configuration_schema_value(version),
            document,
        )
    }
}

/// The path a JSON Schema validator names the whole document by, which every
/// violation below is reported relative to.
pub(super) const JSON_SCHEMA_ROOT_PATH: &str = "$";

/// The pointer prefix a published add-on schema names its own definitions by:
/// every `$ref` in one of these documents points into its `definitions` section.
pub(super) const JSON_SCHEMA_DEFINITION_PREFIX: &str = "#/definitions/";

/// The name a published add-on schema gives the definition of `name`: its
/// segments capitalised and run together, which is the form the published
/// documents title their chart types by (`vpc-cni` is published as `VpcCni`,
/// its `eniConfig` key as `EniConfig`).
pub(super) fn schema_definition_title(name: &str) -> String {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                None => String::new(),
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + characters.as_str()
                }
            }
        })
        .collect()
}

/// A reference to the definition titled `title`, as a schema node.
fn definition_ref(title: &str) -> ::serde_json::Value {
    ::serde_json::json!({ "$ref": format!("{JSON_SCHEMA_DEFINITION_PREFIX}{title}") })
}

/// The definition of one object-valued configuration key: the keys it declares,
/// each with the JSON type that key takes.
///
/// A key whose own keys the catalog publishes declares them; one whose keys it
/// does not declares none, and such an object carries whatever the chart's own
/// values take — which is why `additionalProperties` is not closed here the way
/// it is on the root type, and why no key inside one is published as required:
/// only the ROOT bounds the keys a document may set, and only the build's own
/// required configuration is a key a document cannot leave out.
fn object_definition(
    title: &str,
    properties: &'static [AddonSchemaProperty],
) -> ::serde_json::Value {
    let declared: ::serde_json::Map<String, ::serde_json::Value> = properties
        .iter()
        .map(|(property, json_type)| {
            ((*property).to_string(), ::serde_json::json!({ "type": json_type }))
        })
        .collect();
    ::serde_json::json!({
        "type": "object",
        "title": title,
        "properties": ::serde_json::Value::Object(declared),
    })
}

/// The schema node `schema` stands for: the definition it references, or
/// `schema` itself when it references nothing.
///
/// A `$ref` in one of these documents points into the document's own
/// `definitions` ([`JSON_SCHEMA_DEFINITION_PREFIX`]), so resolving one is a
/// lookup in `root`. The hops are BOUNDED, so a document whose definitions
/// referenced each other could not make this loop.
fn resolved_schema<'document>(
    root: &'document ::serde_json::Value,
    schema: &'document ::serde_json::Value,
) -> &'document ::serde_json::Value {
    let mut node = schema;
    for _ in 0..MAX_CONFIGURATION_DEPTH {
        let Some(title) = node
            .get("$ref")
            .and_then(::serde_json::Value::as_str)
            .and_then(|pointer| pointer.strip_prefix(JSON_SCHEMA_DEFINITION_PREFIX))
        else {
            return node;
        };
        match root.get("definitions").and_then(|section| section.get(title)) {
            // A reference the document does not define constrains nothing, which
            // is what an unresolvable keyword means: the node is taken as it is.
            None => return node,
            Some(definition) => node = definition,
        }
    }
    node
}

/// How `value` fails `schema`, as a validator states it — the path at fault and
/// what is wrong with it — or `None` when it satisfies the schema.
///
/// It reads the keywords the add-on configuration schemas above are written with:
/// `$ref` into the document's own `definitions`, `type`, `properties`, `required`
/// and `additionalProperties`. The first violation found is the one reported, and
/// the walk is depth-first through the properties the SCHEMA declares, so its
/// depth is bounded by the nesting of these schemas rather than by the document a
/// caller sent; the document itself is parsed by `serde_json`, not here.
pub(super) fn json_schema_violation(
    path: &str,
    schema: &::serde_json::Value,
    value: &::serde_json::Value,
) -> Option<String> {
    json_schema_violation_in(schema, path, schema, value)
}

/// [`json_schema_violation`] with the whole document in hand, so a `$ref` can be
/// resolved against the `definitions` it points into at every depth.
fn json_schema_violation_in(
    root: &::serde_json::Value,
    path: &str,
    schema: &::serde_json::Value,
    value: &::serde_json::Value,
) -> Option<String> {
    let schema = resolved_schema(root, schema);
    if let Some(expected) = schema.get("type").and_then(::serde_json::Value::as_str) {
        if !json_value_has_type(expected, value) {
            return Some(format!(
                "{path}: {} found, {expected} expected",
                json_type_name(value)
            ));
        }
    }
    let members = value.as_object()?;
    let properties = schema
        .get("properties")
        .and_then(::serde_json::Value::as_object);
    // `additionalProperties: false` is what makes a key the schema does not
    // declare a violation rather than a value carried along unread.
    if schema.get("additionalProperties") == Some(&::serde_json::Value::Bool(false)) {
        if let Some(undeclared) = members.keys().find(|key| {
            properties.is_none_or(|declared| !declared.contains_key(key.as_str()))
        }) {
            return Some(format!(
                "{path}.{undeclared}: is not defined in the schema and the schema does not \
                 allow additional properties"
            ));
        }
    }
    for required in schema
        .get("required")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let Some(property) = required
            .as_str()
            .filter(|property| !members.contains_key(*property))
        {
            return Some(format!("{path}.{property}: is missing but it is required"));
        }
    }
    let properties = properties?;
    members.iter().find_map(|(key, member)| {
        let declared = properties.get(key)?;
        json_schema_violation_in(root, &format!("{path}.{key}"), declared, member)
    })
}

/// Whether `value` is of the JSON Schema `type` named by `expected`.
///
/// A type the add-on configuration schemas do not use constrains nothing, which
/// is what an unknown keyword value means in JSON Schema.
pub(super) fn json_value_has_type(expected: &str, value: &::serde_json::Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        // In the dialect these schemas name, an integer is any number with a
        // zero fractional part, so `2.0` is an integer and `1.5` is not.
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

/// The JSON Schema type name of a value, as a violation names what it found.
pub(super) fn json_type_name(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "boolean",
        ::serde_json::Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        ::serde_json::Value::String(_) => "string",
        ::serde_json::Value::Array(_) => "array",
        ::serde_json::Value::Object(_) => "object",
    }
}

/// The keys an object-valued configuration key declares, per add-on: the rows of
/// the published schemas' own definitions.
///
/// Source: the chart values each add-on's published configuration schema is
/// generated from — what `aws eks describe-addon-configuration --addon-name
/// <name> --addon-version <build>` reports under the definition of that key. The
/// VPC CNI's `env` definition is the add-on's documented environment-variable
/// surface (the `ENABLE_PREFIX_DELEGATION`, `WARM_*`/`MINIMUM_IP_TARGET`,
/// SNAT, log-level and network-policy variables the Amazon EKS User Guide sets
/// through `configurationValues`), its `eniConfig` definition the custom
/// networking keys, and its `init` and
/// `nodeAgent` definitions the sidecars the chart runs beside the node agent. The
/// EFS CSI driver's `controller` and `node` definitions are the two components
/// its chart configures separately, each with the log level, placement and
/// annotations of that component, and the controller with the access-point switch
/// its own document declares there.
///
/// A key missing from this table is published as an object with no declared key,
/// which is what an object whose own surface the catalog does not publish is: a
/// document may still set it, and the keys inside it are the chart's own.
const ADDON_NESTED_CONFIGURATION_PROPERTIES: &[(&str, &str, &[AddonSchemaProperty])] = &[
    (
        "vpc-cni",
        "env",
        &[
            ("ADDITIONAL_ENI_TAGS", "string"),
            ("ANNOTATE_POD_IP", "string"),
            ("AWS_VPC_CNI_NODE_PORT_SUPPORT", "string"),
            ("AWS_VPC_ENI_MTU", "string"),
            ("AWS_VPC_K8S_CNI_CONFIGURE_RPFILTER", "string"),
            ("AWS_VPC_K8S_CNI_CUSTOM_NETWORK_CFG", "string"),
            ("AWS_VPC_K8S_CNI_EXCLUDE_SNAT_CIDRS", "string"),
            ("AWS_VPC_K8S_CNI_EXTERNALSNAT", "string"),
            ("AWS_VPC_K8S_CNI_LOGLEVEL", "string"),
            ("AWS_VPC_K8S_CNI_LOG_FILE", "string"),
            ("AWS_VPC_K8S_CNI_RANDOMIZESNAT", "string"),
            ("AWS_VPC_K8S_CNI_VETHPREFIX", "string"),
            ("AWS_VPC_K8S_PLUGIN_LOG_FILE", "string"),
            ("AWS_VPC_K8S_PLUGIN_LOG_LEVEL", "string"),
            ("DISABLE_INTROSPECTION", "string"),
            ("DISABLE_METRICS", "string"),
            ("DISABLE_NETWORK_RESOURCE_PROVISIONING", "string"),
            ("ENABLE_POD_ENI", "string"),
            ("ENABLE_PREFIX_DELEGATION", "string"),
            ("MINIMUM_IP_TARGET", "string"),
            ("NETWORK_POLICY_ENFORCING_MODE", "string"),
            ("POD_SECURITY_GROUP_ENFORCING_MODE", "string"),
            ("WARM_ENI_TARGET", "string"),
            ("WARM_IP_TARGET", "string"),
            ("WARM_PREFIX_TARGET", "string"),
        ],
    ),
    (
        "vpc-cni",
        "eniConfig",
        &[
            ("create", "boolean"),
            ("region", "string"),
            ("subnets", "object"),
        ],
    ),
    (
        "vpc-cni",
        "init",
        &[("env", "object"), ("image", "object"), ("resources", "object")],
    ),
    (
        "vpc-cni",
        "nodeAgent",
        &[
            ("enabled", "boolean"),
            ("healthzBindAddr", "string"),
            ("image", "object"),
            ("metricsBindAddr", "string"),
            ("resources", "object"),
        ],
    ),
    (
        "aws-ebs-csi-driver",
        "controller",
        &[
            ("env", "object"),
            ("logLevel", "integer"),
            ("replicaCount", "integer"),
            ("resources", "object"),
            ("volumeModificationFeature", "object"),
        ],
    ),
    (
        "aws-ebs-csi-driver",
        "defaultStorageClass",
        &[("enabled", "boolean")],
    ),
    (
        "aws-ebs-csi-driver",
        "node",
        &[
            ("enableMetrics", "boolean"),
            ("logLevel", "integer"),
            ("resources", "object"),
            ("volumeAttachLimit", "integer"),
        ],
    ),
    (
        "aws-efs-csi-driver",
        "controller",
        &[
            ("deleteAccessPointRootDir", "boolean"),
            ("logLevel", "integer"),
            ("nodeSelector", "object"),
            ("podAnnotations", "object"),
            ("tolerations", "array"),
        ],
    ),
    (
        "aws-efs-csi-driver",
        "node",
        &[
            ("logLevel", "integer"),
            ("nodeSelector", "object"),
            ("podAnnotations", "object"),
            ("tolerations", "array"),
        ],
    ),
    (
        "coredns",
        "autoScaling",
        &[
            ("enabled", "boolean"),
            ("maxReplicas", "integer"),
            ("minReplicas", "integer"),
        ],
    ),
];

/// The keys the object-valued configuration key `property` of the add-on named
/// `addon_name` declares, or EMPTY for one the catalog publishes no keys for.
/// See [`ADDON_NESTED_CONFIGURATION_PROPERTIES`].
pub(super) fn nested_configuration_properties(
    addon_name: &str,
    property: &str,
) -> &'static [AddonSchemaProperty] {
    // A vendor's rebuild of an add-on is the same software, so the keys the
    // objects of its configuration carry are the rebuilt add-on's own.
    let published_for = rebuilt_addon(addon_name).unwrap_or(addon_name);
    ADDON_NESTED_CONFIGURATION_PROPERTIES
        .iter()
        .find(|(addon, key, _)| *addon == published_for && *key == property)
        .map(|(_, _, properties)| *properties)
        .unwrap_or_default()
}

/// The AWS Marketplace listing one Marketplace add-on is subscribed through.
///
/// `MarketplaceInformation` is the listing's `productId` and `productUrl`. This
/// emulator holds no AWS Marketplace subscription, so there is no real listing
/// to report: both are DERIVED from the add-on's name through the one
/// derivation this service's UUID-shaped identifiers use
/// ([`derive_uuid_from_seed`]), which makes them distinct per add-on and stable
/// for as long as the catalog publishes it — the properties a caller can rely
/// on — and the URL is the `prodview-<id>` form a Marketplace listing is
/// published at.
pub(super) fn marketplace_listing(addon_name: &str) -> MarketplaceInformation {
    let product_id = derive_uuid_from_seed(addon_name);
    let view_id: String = product_id.chars().filter(char::is_ascii_alphanumeric).take(13).collect();
    MarketplaceInformation {
        product_id: Some(product_id),
        product_url: Some(format!(
            "https://aws.amazon.com/marketplace/pp/prodview-{view_id}"
        )),
    }
}

/// The index of the [`VERSION_CALENDAR`] row an add-on's published history
/// starts at: the row [`AddonCatalogSeed::first_cluster_version`] names, or the
/// oldest row the published add-on build lines target
/// ([`ADDON_CATALOG_FIRST_CLUSTER_VERSION`]) for an add-on published for every
/// cluster version Amazon EKS offers add-ons on.
///
/// A name no calendar row carries is a typo in the seed, which is why it panics
/// here — at seed time, with the add-on named — rather than quietly publishing
/// the add-on for every row.
fn first_published_row(seed: &AddonCatalogSeed) -> usize {
    let first = seed
        .first_cluster_version
        .unwrap_or(ADDON_CATALOG_FIRST_CLUSTER_VERSION);
    VERSION_CALENDAR
        .iter()
        .position(|(cluster_version, _, _, _)| *cluster_version == first)
        .unwrap_or_else(|| {
            panic!(
                "the add-on catalog publishes {} from Kubernetes {first}, which the version \
                 calendar does not carry",
                seed.addon_name
            )
        })
}

/// The version of the build an add-on published for the `step`-th Kubernetes
/// version it has been offered for, in the `vX.Y.Z-eksbuild.N` form every Amazon
/// EKS add-on version takes.
///
/// It is the add-on's OWN release at that step
/// ([`AddonCatalogSeed::releases`]) — which is how a published history crosses
/// the major versions the project released — and past the end of that line it
/// continues from the last release by one minor version per further step, so a
/// calendar row the recorded history does not reach still has a build.
fn addon_build_version(seed: &AddonCatalogSeed, step: usize) -> String {
    let (major, minor, patch) = seed
        .releases
        .get(step)
        .copied()
        .or_else(|| {
            let (major, minor, patch) = *seed.releases.last()?;
            Some((major, minor + (step + 1 - seed.releases.len()) as u32, patch))
        })
        .unwrap_or_else(|| {
            panic!(
                "the add-on catalog publishes {} with no release line at all",
                seed.addon_name
            )
        });
    format!("v{major}.{minor}.{patch}-eksbuild.1")
}

/// Build the add-on catalog this service publishes from
/// [`published_addon_seeds`].
///
/// An add-on publishes one build per Kubernetes minor version it has been
/// OFFERED for, along its own version line ([`addon_build_version`]): the build
/// published alongside a Kubernetes version supports that version and EVERY
/// older version the add-on has been offered for — an add-on build keeps
/// working on the cluster versions released before it — so a build's
/// `compatibilities` carry one entry per Kubernetes minor version it supports,
/// newest first, and the newest build of an add-on is the one compatible with
/// the whole window.
///
/// Which build is the DEFAULT for a Kubernetes version is not seeded: it is the
/// newest PUBLISHED build that supports that version, decided per read against
/// the clock ([`AddonCatalogRecord::to_addon_version_infos`]), so for any
/// Kubernetes version the catalog names exactly ONE default build per add-on
/// while every superseded build is offered as a non-default choice. That is what
/// `CreateAddon` resolves an absent `addonVersion` through
/// ([`ComAmazonawsEksAwswesleyfrontendState::default_addon_version`]).
///
/// The versions an add-on is offered for start at its OWN first calendar row
/// ([`first_published_row`]), so the compatible cluster-version set an add-on
/// reports is the add-on's own: an add-on Amazon EKS began offering later has no
/// build for the older rows, and the oldest build it does publish names no
/// cluster version older than the one it was introduced on.
///
/// Each add-on's builds come out NEWEST FIRST, the order the catalog publishes
/// them in; the add-ons themselves keep the publication's name order.
pub(super) fn seed_addon_catalog() -> Vec<AddonCatalogRecord> {
    published_addon_seeds()
        .iter()
        .map(|seed| {
            let first = first_published_row(seed);
            let mut versions: Vec<AddonCatalogVersionRecord> = VERSION_CALENDAR
                .iter()
                .enumerate()
                .skip(first)
                .map(|(index, (_, release, _, _))| {
                    // Every Kubernetes version the add-on has been offered for
                    // up to and including the one this build was published
                    // alongside, newest first.
                    let compatibilities: Vec<AddonCompatibilityRecord> = VERSION_CALENDAR
                        [first..=index]
                        .iter()
                        .rev()
                        .map(|(cluster_version, _, _, _)| AddonCompatibilityRecord {
                            cluster_version: (*cluster_version).to_string(),
                            platform_versions: vec![ADDON_ANY_PLATFORM_VERSION.to_string()],
                        })
                        .collect();
                    AddonCatalogVersionRecord {
                        addon_version: addon_build_version(seed, index - first),
                        published_at: calendar_instant(release),
                        architecture: ADDON_ARCHITECTURES
                            .iter()
                            .map(|architecture| (*architecture).to_string())
                            .collect(),
                        compute_types: seed
                            .compute_types
                            .iter()
                            .map(|compute_type| (*compute_type).to_string())
                            .collect(),
                        compatibilities,
                        required_configuration: seed
                            .required_configuration
                            .iter()
                            .map(|property| (*property).to_string())
                            .collect(),
                        requires_iam_permissions: seed.requires_iam_permissions,
                    }
                })
                .collect();
            versions.reverse();
            AddonCatalogRecord {
                addon_name: seed.addon_name.to_string(),
                addon_type: seed.addon_type.to_string(),
                owner: seed.owner.to_string(),
                publisher: seed.publisher.to_string(),
                default_namespace: seed.default_namespace.to_string(),
                workload: seed.workload,
                required_addons: seed
                    .required_addons
                    .iter()
                    .map(|required| (*required).to_string())
                    .collect(),
                admission_webhook: seed.admission_webhook,
                marketplace_information: (seed.owner == ADDON_OWNER_MARKETPLACE)
                    .then(|| marketplace_listing(seed.addon_name)),
                pod_identity: seed
                    .pod_identity
                    .iter()
                    .map(|(service_account, policies)| {
                        (
                            (*service_account).to_string(),
                            policies.iter().map(|policy| (*policy).to_string()).collect(),
                        )
                    })
                    .collect(),
                configuration_properties: ADDON_COMMON_CONFIGURATION_PROPERTIES
                    .iter()
                    .chain(seed.configuration_properties)
                    .map(|(property, json_type)| {
                        ((*property).to_string(), (*json_type).to_string())
                    })
                    // A `BTreeMap` so a key the row declares a type of its own
                    // for overrides the common one, and the properties come out
                    // in the schema's alphabetical order with no duplicates.
                    .collect::<BTreeMap<String, String>>()
                    .into_iter()
                    .collect(),
                schema_dialect: seed.schema_dialect.to_string(),
                versions,
            }
        })
        .collect()
}

/// The sentence Amazon EKS refuses a `configurationValues` document it cannot
/// read at all with.
///
/// Fixed text in its entirety, with no variable part: the document is the
/// caller's own, and the refusal says only that it is neither of the two formats
/// an add-on configuration is written in.
pub const ADDON_CONFIGURATION_UNREADABLE_MESSAGE: &str =
    "The ConfigurationValues provided is not in valid YAML or JSON format.";

/// The sentence Amazon EKS refuses a `configurationValues` document that does not
/// satisfy the configuration schema published for the build with.
///
/// `format` is the format the document was READ as — the word the refusal names,
/// which is [`AddonConfiguration::format`] — and `violation` is the path at fault
/// and what is wrong with it, as the validator states it
/// ([`AddonCatalogRecord::configuration_violation`]).
#[must_use]
pub fn addon_configuration_violation_message(format: &str, violation: &str) -> String {
    format!(
        "ConfigurationValue provided in request is not supported: {format} schema \
         validation failed with error: [{violation}]"
    )
}

/// What a `configurationValues` document a request carried states.
///
/// "The set of configuration values for the add-on" is a document a caller writes
/// in JSON or in YAML, and either form is held to the configuration schema
/// published for the build ([`AddonCatalogRecord::configuration_violation`]) —
/// which is why the FORM it was read as travels with it: the refusal a document
/// that fails that schema earns names the format it was read as.
#[derive(Debug)]
pub enum AddonConfiguration {
    /// The request carried no document, or carried one with nothing in it: there
    /// is nothing to read and nothing to validate.
    Absent,
    /// A document that was read, as the format [`Self::format`] names.
    Read {
        /// Whether it was read as JSON or as YAML.
        json: bool,
        /// The document, as the value the schema is checked against.
        document: ::serde_json::Value,
    },
    /// A document that is neither JSON nor YAML, which is refused with
    /// [`ADDON_CONFIGURATION_UNREADABLE_MESSAGE`].
    Unreadable,
}

impl AddonConfiguration {
    /// The format word the refusal of this document names: `Json` for a document
    /// read as JSON, `Yaml` for one read as YAML — and, for a document there was
    /// nothing to read, `Yaml` is never asked for: only a document that WAS read
    /// earns a refusal that names a format.
    #[must_use]
    pub fn format(&self) -> &'static str {
        match self {
            Self::Read { json: true, .. } => "Json",
            Self::Read { json: false, .. } => "Yaml",
            Self::Absent | Self::Unreadable => "",
        }
    }
}

/// How deeply a configuration document is read.
///
/// It is the bound at BOTH ends of the reading, so one document is not read
/// deeper by one half of it than by the other:
///
/// * a document whose nesting goes past it is read as no document at all, decided
///   by the one pre-parse bound this tree states
///   ([`::service_common::yaml_nesting::nesting_depth_bound`]) BEFORE any parser
///   sees the text, because a parser costs a level of recursion per nested
///   collection and exhausting the stack aborts the process rather than
///   unwinding;
/// * a node past it in a document that bound admitted is read as the EMPTY value
///   of its own kind: an object or an array with nothing in it, which states the
///   same TYPE at that level and so is judged exactly as the document states it.
///   The published configuration schemas declare properties at the top level
///   only, so nothing below that level is a key a schema could declare and
///   nothing is skipped by cutting there — the schema check runs on every
///   document that bound admits.
///
/// The value is the recursion limit `serde_json` applies to a JSON document of
/// its own accord, applied to the YAML reading too.
pub const MAX_CONFIGURATION_DEPTH: usize = 128;

/// The most bytes of a `configurationValues` document this service reads.
///
/// `CreateAddon$configurationValues` carries no modelled `length`, and the
/// document is read into a tree sized by what the caller sent, so this is the
/// bound that keeps one request from growing the emulator's memory without a
/// limit. It is generous next to the documents an add-on's schema can be
/// satisfied by — the published schemas declare a couple of dozen top-level
/// keys — and a document past it is read as no document at all
/// ([`ADDON_CONFIGURATION_UNREADABLE_MESSAGE`]) rather than parsed.
pub const MAX_CONFIGURATION_BYTES: usize = 64 * 1024;

/// Read the `configurationValues` document of an add-on request the way Amazon
/// EKS reads one: as JSON, or else as the YAML an add-on configuration may
/// equally be written in.
///
/// This is the one reading both add-on write operations share, so an install and
/// an update cannot disagree about which documents are readable or about which
/// form one was read as.
///
/// BOUNDED on the way in, because the text is a caller's, and every bound is
/// settled BEFORE a parser is handed the text: a document past
/// [`MAX_CONFIGURATION_BYTES`], one whose nesting is past what this service reads
/// (the one pre-parse nesting bound this tree states,
/// [`::service_common::yaml_nesting::nesting_depth_bound`]), and one that
/// references an anchor — which that bound deliberately leaves to its caller,
/// because a YAML loader resolves an alias by COPYING the anchored node into each
/// alias position, so a shallow document that anchors a collection and references
/// it a few times per level expands multiplicatively while it is being loaded.
/// None of the three is a document this service reads, so each is answered as one
/// it could not read.
pub fn read_addon_configuration(values: Option<&str>) -> AddonConfiguration {
    let Some(text) = values.map(str::trim).filter(|text| !text.is_empty()) else {
        return AddonConfiguration::Absent;
    };
    if text.len() > MAX_CONFIGURATION_BYTES {
        return AddonConfiguration::Unreadable;
    }
    if let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) {
        return AddonConfiguration::Read {
            json: true,
            document,
        };
    }
    if ::service_common::yaml_nesting::nesting_depth_bound(text, MAX_CONFIGURATION_DEPTH)
        > MAX_CONFIGURATION_DEPTH
        || references_an_anchor(text)
    {
        return AddonConfiguration::Unreadable;
    }
    let Ok(documents) = ::yaml_rust2::YamlLoader::load_from_str(text) else {
        return AddonConfiguration::Unreadable;
    };
    // A YAML stream with nothing in it (a document of comments alone) states no
    // values, which is the same as carrying none.
    let Some(first) = documents.first() else {
        return AddonConfiguration::Absent;
    };
    AddonConfiguration::Read {
        json: false,
        document: yaml_as_json(first, MAX_CONFIGURATION_DEPTH),
    }
}

/// Whether `text` references a YAML anchor anywhere.
///
/// Answered by the EVENT parser, which reports an alias as one event and copies
/// nothing: the question has to be settled before a loader builds a tree, which
/// is the point at which an alias would be expanded (see
/// [`read_addon_configuration`]). The events are discarded, so this costs the
/// bytes the document already occupies and nothing more.
fn references_an_anchor(text: &str) -> bool {
    struct AliasDetector {
        found: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.found = true;
            }
        }
    }
    let mut detector = AliasDetector { found: false };
    let _ = ::yaml_rust2::parser::Parser::new_from_str(text).load(&mut detector, true);
    detector.found
}


/// One YAML node as the JSON value a JSON Schema is checked against.
///
/// YAML's scalars are JSON's: a mapping is an object keyed by the text of its
/// keys, a sequence is an array, and a node YAML carries that JSON has no value
/// for (an alias, a value the parser could not resolve) reads as null, the same
/// as an explicitly null one. A node past `depth` reads as the empty value of its
/// own kind, which states the same type at that level
/// ([`MAX_CONFIGURATION_DEPTH`]).
fn yaml_as_json(node: &::yaml_rust2::Yaml, depth: usize) -> ::serde_json::Value {
    use ::yaml_rust2::Yaml;
    let Some(depth) = depth.checked_sub(1) else {
        return match node {
            Yaml::Array(_) => ::serde_json::Value::Array(Vec::new()),
            Yaml::Hash(_) => ::serde_json::Value::Object(::serde_json::Map::new()),
            scalar => yaml_as_json(scalar, 1),
        };
    };
    match node {
        Yaml::String(text) => ::serde_json::Value::String(text.clone()),
        Yaml::Integer(number) => ::serde_json::Value::from(*number),
        Yaml::Real(text) => text
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or(::serde_json::Value::String(text.clone()), |number| {
                ::serde_json::Value::Number(number)
            }),
        Yaml::Boolean(held) => ::serde_json::Value::Bool(*held),
        Yaml::Array(items) => ::serde_json::Value::Array(
            items.iter().map(|item| yaml_as_json(item, depth)).collect(),
        ),
        Yaml::Hash(entries) => {
            let mut members = ::serde_json::Map::new();
            for (key, value) in entries {
                members.insert(yaml_key(key, depth), yaml_as_json(value, depth));
            }
            ::serde_json::Value::Object(members)
        }
        Yaml::Null | Yaml::BadValue | Yaml::Alias(_) => ::serde_json::Value::Null,
    }
}

/// The member name a YAML mapping key states, as a JSON object names one.
///
/// A scalar key names the member its own text does. A key that is itself a
/// collection names no member a schema can declare — JSON has no such key — so it
/// is named by the JSON the key is written as, which keeps the member in the
/// document: a key no schema declares is the schema's to refuse, not this
/// reading's to drop.
fn yaml_key(key: &::yaml_rust2::Yaml, depth: usize) -> String {
    use ::yaml_rust2::Yaml;
    match key {
        Yaml::String(text) | Yaml::Real(text) => text.clone(),
        Yaml::Integer(number) => number.to_string(),
        Yaml::Boolean(held) => held.to_string(),
        other => ::serde_json::to_string(&yaml_as_json(other, depth)).unwrap_or_default(),
    }
}

#[cfg(test)]
mod configuration_document_tests {
    use super::{
        read_addon_configuration, AddonConfiguration, MAX_CONFIGURATION_BYTES,
        MAX_CONFIGURATION_DEPTH,
    };

    /// RULE: a configuration document is read as JSON, or else as YAML; one that
    /// is neither is unreadable. The rows differ along the FORM the document
    /// takes rather than along one recorded document, and each row states the
    /// format word the refusal of that document would name.
    #[test]
    fn a_configuration_document_is_read_as_json_or_as_yaml() {
        // (case, document, what it states)
        let rows: &[(&str, Option<&str>, &str)] = &[
            ("no document at all", None, "absent"),
            ("an empty document", Some(""), "absent"),
            ("a document of whitespace", Some("  \n\t"), "absent"),
            ("a document of comments alone", Some("# nothing\n"), "absent"),
            ("a JSON object", Some("{\"useFips\":true}"), "Json"),
            ("a JSON array", Some("[1, 2]"), "Json"),
            ("a JSON scalar", Some("3"), "Json"),
            ("a YAML mapping", Some("env:\n  LOG_LEVEL: debug\n"), "Yaml"),
            ("a YAML list", Some("- one\n- two\n"), "Yaml"),
            (
                "a YAML flow mapping JSON would reject",
                Some("{useFips: true}"),
                "Yaml",
            ),
            ("a YAML scalar", Some("replicaCount"), "Yaml"),
            ("a JSON object that is not closed", Some("{\"a\":"), "neither"),
            ("a JSON array that is not closed", Some("[1,"), "neither"),
            ("a document that opens a brace", Some("{nope"), "neither"),
            ("a mapping with a stray quote", Some("a: \"b\nc: d\n"), "neither"),
        ];
        for (case, document, expected) in rows {
            let read = read_addon_configuration(*document);
            let actual = match &read {
                AddonConfiguration::Absent => "absent",
                AddonConfiguration::Read { .. } => read.format(),
                AddonConfiguration::Unreadable => "neither",
            };
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: either form is read into the SAME value, so the schema check cannot
    /// depend on which of them the caller wrote. The rows differ along the JSON
    /// type the document states; each YAML form is one JSON would not accept, so
    /// the two readings are genuinely different paths.
    #[test]
    fn the_two_forms_of_one_document_are_read_into_one_value() {
        // (case, the JSON form, the YAML form)
        let rows: &[(&str, &str, &str)] = &[
            ("an object", "{\"useFIPS\":true}", "useFIPS: true\n"),
            (
                "a nested object",
                "{\"node\":{\"replicaCount\":2}}",
                "node:\n  replicaCount: 2\n",
            ),
            ("an array", "[\"one\",\"two\"]", "- one\n- two\n"),
            ("a string", "\"replicaCount\"", "replicaCount\n"),
            ("a number", "{\"replicaCount\":2}", "replicaCount: 2\n"),
            ("a fraction", "{\"ratio\":1.5}", "ratio: 1.5\n"),
            ("null", "null", "~\n"),
            (
                "an object of several types",
                "{\"useFIPS\":true,\"replicaCount\":2,\"name\":\"efs\"}",
                "useFIPS: true\nreplicaCount: 2\nname: efs\n",
            ),
        ];
        for (case, as_json, as_yaml) in rows {
            let json = read_addon_configuration(Some(as_json));
            let yaml = read_addon_configuration(Some(as_yaml));
            let (
                AddonConfiguration::Read {
                    document: from_json,
                    ..
                },
                AddonConfiguration::Read {
                    document: from_yaml,
                    ..
                },
            ) = (&json, &yaml)
            else {
                panic!("case {case:?}: both forms must be read, got {json:?} and {yaml:?}");
            };
            assert_eq!(
                from_json, from_yaml,
                "case {case:?}: expected {from_json:?}, got {from_yaml:?}"
            );
            assert_eq!(json.format(), "Json", "case {case:?}: the JSON form");
        }
    }

    /// RULE: how deeply a document nests is settled BEFORE any parser is handed
    /// the text, by the one pre-parse nesting bound this tree states — a document
    /// past it is read as no document at all — and a document that bound admits
    /// is read to [`MAX_CONFIGURATION_DEPTH`], the node at the bound keeping the
    /// TYPE the document states there, so the schema check runs on every document
    /// that is read and no caller can opt out of it by nesting.
    #[test]
    fn how_deeply_a_document_nests_is_settled_before_it_is_parsed() {
        // A YAML flow sequence nested `depth` deep: `[[[…]]]`, so every level
        // states an array and the document carries one nesting indicator per
        // level. The rows straddle the bound, and the last is sized from the BYTE
        // bound rather than the depth one: a document that is well inside the
        // bytes a request may carry still cannot drive a parse that deep.
        for (case, depth, read) in [
            ("at the bound", MAX_CONFIGURATION_DEPTH, true),
            ("one level past the bound", MAX_CONFIGURATION_DEPTH + 1, false),
            ("two levels past the bound", MAX_CONFIGURATION_DEPTH + 2, false),
            ("as deep as the byte bound allows", MAX_CONFIGURATION_BYTES / 2, false),
        ] {
            let document = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let answer = read_addon_configuration(Some(&document));
            assert_eq!(
                !matches!(answer, AddonConfiguration::Unreadable),
                read,
                "case {case:?}: expected read={read}, got {answer:?}"
            );
            if read {
                match answer {
                    AddonConfiguration::Read { document, .. } => assert!(
                        document.is_array(),
                        "case {case:?}: the document states an array, got {document:?}"
                    ),
                    other => panic!("case {case:?}: the document must be read, got {other:?}"),
                }
            }
        }

        // A closer hidden in a quoted scalar cannot talk the bound down: every
        // opener counts, however many closers the text carries.
        let hidden = format!(
            "{}\"]\"{}",
            "[".repeat(MAX_CONFIGURATION_DEPTH + 1),
            "]".repeat(MAX_CONFIGURATION_DEPTH + 1)
        );
        assert!(
            matches!(
                read_addon_configuration(Some(&hidden)),
                AddonConfiguration::Unreadable
            ),
            "a document with more openers than the bound allows is read as none"
        );

        // Indentation is the other form nesting takes, and it needs no indicator
        // byte at all: a line indented past the bound is read as no document.
        let indented = format!("a:\n{}b: one\n", " ".repeat(MAX_CONFIGURATION_DEPTH + 1));
        assert!(
            matches!(
                read_addon_configuration(Some(&indented)),
                AddonConfiguration::Unreadable
            ),
            "a line indented past the bound is read as no document"
        );

        // A block mapping the bound admits is read, so the key it sets is the key
        // the schema judges — and the cut the reading makes at the bound (above)
        // never touches a top-level key.
        let deep = format!(
            "notAProperty:\n{}",
            (1..MAX_CONFIGURATION_DEPTH / 4)
                .map(|level| format!("{}a{level}:\n", " ".repeat(level)))
                .collect::<String>()
        );
        match read_addon_configuration(Some(&deep)) {
            AddonConfiguration::Read { document, .. } => assert!(
                document.get("notAProperty").is_some(),
                "a nested mapping keeps the key it sets, got {document:?}"
            ),
            other => panic!("the document must be read, got {other:?}"),
        }
    }

    /// RULE: a document this service cannot read WITHOUT growing without a bound
    /// is read as no document at all. The rows differ along what would grow: the
    /// text itself, and the anchor references a loader expands by copying.
    #[test]
    fn a_document_that_could_only_be_read_by_growing_without_a_bound_is_unreadable() {
        let key = "priorityClassName: system-cluster-critical\n";
        // (case, the document, whether this service reads it)
        let rows: Vec<(&str, String, bool)> = vec![
            ("a document at the byte bound", "a".repeat(MAX_CONFIGURATION_BYTES), true),
            (
                "a document one byte past it",
                "a".repeat(MAX_CONFIGURATION_BYTES + 1),
                false,
            ),
            (
                "a JSON document past it",
                format!("{{\"a\":\"{}\"}}", "b".repeat(MAX_CONFIGURATION_BYTES)),
                false,
            ),
            ("a document well inside it", key.to_string(), true),
            (
                "one alias of an anchored scalar",
                "anchored: &a one\nalias: *a\n".to_string(),
                false,
            ),
            (
                "a mapping anchored and referenced per level",
                "a: &x\n  k: v\nb: [*x, *x, *x]\nc: [*x, *x, *x]\n".to_string(),
                false,
            ),
            (
                "an anchor nothing references",
                "anchored: &a one\nother: two\n".to_string(),
                true,
            ),
        ];
        for (case, document, read) in rows {
            let answer = read_addon_configuration(Some(&document));
            assert_eq!(
                !matches!(answer, AddonConfiguration::Unreadable),
                read,
                "case {case:?}: expected read={read}, got {answer:?}"
            );
        }
    }
}
