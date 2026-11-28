// SPDX-License-Identifier: CC0-1.0

//! The repositories the gallery AWS serves PUBLISHES ITSELF, and which releases of each this
//! emulator holds.
//!
//! # Why anything is held here at all
//!
//! The gallery AWS serves carries one public registry per publishing account, each under its
//! own alias, and this emulator holds a STORE for exactly one of them — this account's
//! ([`super::gallery_pull`]). A reference into any other alias is therefore a reference into a
//! registry whose repositories no store here lists, which is why nothing is decided against
//! such a fetch: an absent store is not evidence that an image is missing.
//!
//! That is not the whole truth for the registries AWS publishes in the gallery itself. The
//! `docker` alias is the gallery's mirror of the Docker Official Images, and a compute service
//! of this emulator does not merely pass such a reference along: it RUNS the image's own
//! program, which it can only do for an image it holds (the servers and the multi-call binary
//! the Amazon ECS crate performs a container's command with). An image this emulator holds is
//! an image whose RELEASES it holds too — the tags that repository publishes — so a reference
//! into one of them IS decided here: a tag the repository publishes is served, and a tag it
//! publishes no release under resolves to no image, exactly as a repository of this registry's
//! own does. An alias this emulator holds no image of is left undecided as before.
//!
//! What is decided for a repository no image is held of turns on whether that repository
//! EXISTS, and for this one namespace that is a published fact rather than an absent store:
//! the namespace the mirror carries is the Docker Official Images, whose repository list is
//! itself the catalogue of those images ([`OFFICIAL_IMAGE_NAMES`]). So a reference into that
//! namespace naming a repository the catalogue does not list names no repository at all, and
//! the registry answers it as one — while a repository the catalogue DOES list, whose releases
//! this emulator simply does not hold, stays undecided, as every repository of every other
//! namespace, alias and registry does.
//!
//! # What the entries state, and where they come from
//!
//! Each entry is the repository as the gallery PUBLISHES it — the alias it is published under,
//! its repository name and the tags its gallery listing carries (`gallery.ecr.aws/docker/…`,
//! which mirrors the Docker Official Images' own tag lists; the same listing a client reads
//! with `DescribeImageTags` for a repository of this registry). Nothing is computed from a
//! request and no tag is invented: a release this emulator does not hold is one the gallery
//! here holds no image for.
//!
//! A reference that addresses a DIGEST is not answered: a digest names content by its own
//! hash rather than a release the repository published, so the tags a repository publishes
//! state nothing about it.
//!
//! The same entries answer the OTHER spelling of the very same repository. The `docker` alias
//! is a MIRROR, so each of these images has a repository on the registry it is mirrored from,
//! which a reference may name with or without that registry's `library` namespace
//! ([`official_image`]); a mirror carries the releases of the repository it mirrors, so the
//! releases held here decide a reference through either registry rather than only the one
//! whose alias it spells.
//!
//! A READ: deciding a pull consults these entries and writes nothing, so a container start
//! cannot change what a later caller reads back.

use super::gallery_pull::GalleryReference;

/// One repository the gallery AWS serves publishes, whose image this emulator holds.
#[derive(Debug, Clone, Copy)]
pub struct PublishedRepository {
    /// The registry alias the repository is published under, which is the segment a reference
    /// carries after the gallery host.
    pub registry_alias: &'static str,
    /// The repository name, as a reference spells it after the alias.
    pub repository_name: &'static str,
    /// The tags the repository's gallery listing publishes a release under.
    pub image_tags: &'static [&'static str],
    /// The PROGRAM the image this repository publishes declares as its own — the argument
    /// vector its configuration's entry point and command name, which is what a container of
    /// it runs when the client that created the container names no program of its own.
    ///
    /// SOURCE: each image's own published default command, as the image documents it. The
    /// private registry answers the same question off the configuration blob a push stored
    /// (`::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_program`);
    /// this gallery holds the published images themselves, so what each of them declares is
    /// stated here beside the releases it publishes.
    pub image_program: &'static [&'static str],
}

/// The tags the gallery's mirror of the BusyBox official image publishes.
///
/// The image's own release tags (its `1.x` versions, each also reachable under the version
/// family it leads), the moving `latest`/`stable`/`unstable` channels, and the libc variants
/// that image documents (`glibc`, `musl`, `uclibc`), alone and on a channel.
const BUSYBOX_TAGS: &[&str] = &[
    "latest",
    "stable",
    "unstable",
    "1",
    "1.36",
    "1.36.1",
    "1.37",
    "1.37.0",
    "glibc",
    "musl",
    "uclibc",
    "stable-glibc",
    "stable-musl",
    "stable-uclibc",
    "unstable-glibc",
    "unstable-musl",
    "unstable-uclibc",
];

/// The tags the gallery's mirror of the nginx official image publishes: its `1.x` release
/// families, the two branches that image names (`mainline`, `stable`), the Alpine-based
/// variants of each and the `perl` build, plus `latest`.
const NGINX_TAGS: &[&str] = &[
    "latest",
    "mainline",
    "stable",
    "alpine",
    "alpine-slim",
    "mainline-alpine",
    "stable-alpine",
    "perl",
    "1",
    "1.26",
    "1.26-alpine",
    "1.27",
    "1.27-alpine",
    "1.28",
    "1.28-alpine",
    "1.29",
    "1.29-alpine",
];

/// The tags the gallery's mirror of the Apache HTTP Server official image publishes: its
/// `2.4` release family, the Alpine-based variant of it, and `latest`.
const HTTPD_TAGS: &[&str] = &["latest", "alpine", "2", "2.4", "2.4-alpine"];

/// The repositories of the gallery AWS serves whose image this emulator holds.
///
/// All three are Docker Official Images, published in the gallery under the `docker` alias
/// with the `library/` path those images carry, and all three are images whose own program a
/// container of this emulator performs: the multi-call binary a small image's command invokes
/// its applets through, and the two web servers an image ships
/// (`::service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::container_servers`
/// holds that reading, which is also where the repositories each server's image is published
/// under are listed).
pub const PUBLISHED_REPOSITORIES: &[PublishedRepository] = &[
    PublishedRepository {
        registry_alias: DOCKER_OFFICIAL_IMAGES_ALIAS,
        repository_name: "library/busybox",
        image_tags: BUSYBOX_TAGS,
        // The BusyBox official image's own default command is the SHELL that multi-call
        // binary provides, which is what a container of it runs when its client names no
        // program: the image's documented command, and the one a `docker run` of it drops
        // into.
        image_program: &["sh"],
    },
    PublishedRepository {
        registry_alias: DOCKER_OFFICIAL_IMAGES_ALIAS,
        repository_name: "library/nginx",
        image_tags: NGINX_TAGS,
        // The command the nginx official image documents for its own container: the server in
        // the foreground, which is how that image runs it.
        image_program: &["nginx", "-g", "daemon off;"],
    },
    PublishedRepository {
        registry_alias: DOCKER_OFFICIAL_IMAGES_ALIAS,
        repository_name: "library/httpd",
        image_tags: HTTPD_TAGS,
        // The Apache HTTP Server official image's own command, which its documentation names
        // as the foreground wrapper the image ships.
        image_program: &["httpd-foreground"],
    },
];

/// The alias the gallery publishes its mirror of the Docker Official Images under, which is
/// the registry every entry above belongs to.
const DOCKER_OFFICIAL_IMAGES_ALIAS: &str = "docker";

/// The namespace a Docker Official Image's own repository carries on the registry the gallery
/// mirrors it from: "Docker Official Images" live under `library`, which is why the gallery
/// spells each of them `library/<name>` under its [`DOCKER_OFFICIAL_IMAGES_ALIAS`] and why a
/// reference that names the bare `<name>` addresses the same repository.
const OFFICIAL_IMAGES_NAMESPACE: &str = "library";

/// The names the Docker Official Images catalogue publishes a repository under, which is the
/// repository list of the namespace the gallery's [`DOCKER_OFFICIAL_IMAGES_ALIAS`] mirrors
/// ([`OFFICIAL_IMAGES_NAMESPACE`]).
///
/// SOURCE: that namespace's own published catalogue — the `library/` image definitions of the
/// Docker Official Images, which is the list the gallery's mirror of them carries and the list
/// a container runtime resolves a bare reference against. The entries this emulator holds an
/// IMAGE of are a few of these ([`PUBLISHED_REPOSITORIES`]); the rest are repositories that
/// exist and whose releases are not held here, which is why a reference into one of them is
/// left undecided rather than answered.
///
/// ABSENCE from this list is the only thing decided from it, so a name too many only leaves a
/// reference undecided, exactly as before.
const OFFICIAL_IMAGE_NAMES: &[&str] = &[
    "adminer",
    "aerospike",
    "almalinux",
    "alpine",
    "alt",
    "amazoncorretto",
    "amazonlinux",
    "api-firewall",
    "arangodb",
    "archlinux",
    "backdrop",
    "bash",
    "bonita",
    "buildpack-deps",
    "busybox",
    "caddy",
    "cassandra",
    "chronograf",
    "cirros",
    "clearlinux",
    "clefos",
    "clickhouse",
    "clojure",
    "composer",
    "convertigo",
    "couchbase",
    "couchdb",
    "crate",
    "dart",
    "debian",
    "docker",
    "drupal",
    "eclipse-mosquitto",
    "eclipse-temurin",
    "eggdrop",
    "elasticsearch",
    "elixir",
    "emqx",
    "erlang",
    "fedora",
    "flink",
    "fluentd",
    "friendica",
    "gazebo",
    "gcc",
    "geonetwork",
    "ghost",
    "golang",
    "gradle",
    "groovy",
    "haproxy",
    "haskell",
    "haxe",
    "hello-world",
    "hitch",
    "httpd",
    "hylang",
    "ibm-semeru-runtimes",
    "ibmjava",
    "influxdb",
    "irssi",
    "jetty",
    "joomla",
    "jruby",
    "julia",
    "kapacitor",
    "kibana",
    "kong",
    "krakend",
    "lightstreamer",
    "logstash",
    "mageia",
    "mariadb",
    "matomo",
    "maven",
    "mediawiki",
    "memcached",
    "mongo",
    "mongo-express",
    "monica",
    "mono",
    "mysql",
    "nats",
    "neo4j",
    "neurodebian",
    "nextcloud",
    "nginx",
    "node",
    "notary",
    "odoo",
    "open-liberty",
    "openjdk",
    "oraclelinux",
    "orientdb",
    "percona",
    "perl",
    "photon",
    "php",
    "phpmyadmin",
    "plone",
    "postfixadmin",
    "postgres",
    "pypy",
    "python",
    "r-base",
    "rabbitmq",
    "rakudo-star",
    "redis",
    "redmine",
    "registry",
    "rethinkdb",
    "rocket.chat",
    "rockylinux",
    "ros",
    "ruby",
    "rust",
    "sapmachine",
    "satosa",
    "scratch",
    "silverpeas",
    "solr",
    "sonarqube",
    "spark",
    "spiped",
    "storm",
    "swift",
    "swipl",
    "teamspeak",
    "telegraf",
    "tomcat",
    "tomee",
    "traefik",
    "ubuntu",
    "unit",
    "varnish",
    "websphere-liberty",
    "wordpress",
    "xwiki",
    "yourls",
    "znc",
    "zookeeper",
];

/// What this catalogue says about one reference into a repository it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedImage {
    /// The repository publishes a release under the tag the reference asks for.
    Published,
    /// The repository publishes no release under it, so the reference resolves to no image.
    NotPublished,
    /// The registry publishes no such repository AT ALL, so the reference names nothing for a
    /// tag to be asked of: the mirrored namespace's own catalogue does not list it
    /// ([`OFFICIAL_IMAGE_NAMES`]).
    NoSuchRepository,
}

/// Whether the MIRRORED namespace publishes a repository named `repository_name` — the name as
/// that namespace spells it, `library/<name>` — which is what tells a repository whose
/// releases are merely not held here from one that does not exist.
///
/// `None` for a name outside that namespace, about which this catalogue says nothing.
fn official_images_namespace_publishes(repository_name: &str) -> Option<bool> {
    let name = repository_name
        .strip_prefix(OFFICIAL_IMAGES_NAMESPACE)?
        .strip_prefix('/')?;
    Some(OFFICIAL_IMAGE_NAMES.contains(&name))
}

/// What the catalogue says about a reference into `repository_name` once no entry of this
/// emulator's own holds that repository: within the mirrored namespace its published
/// catalogue decides it, and a repository outside that namespace is not answered at all.
fn unheld_repository(repository_name: &str) -> Option<PublishedImage> {
    match official_images_namespace_publishes(repository_name)? {
        // A repository that namespace publishes whose releases this emulator does not hold:
        // the repository is there, and nothing here knows which releases it carries.
        true => None,
        false => Some(PublishedImage::NoSuchRepository),
    }
}

/// The entry for the repository `repository_name` of the registry `registry_alias`, or `None`
/// for every repository and alias this emulator holds no image of.
#[must_use]
pub fn published_repository(
    registry_alias: &str,
    repository_name: &str,
) -> Option<&'static PublishedRepository> {
    PUBLISHED_REPOSITORIES.iter().find(|published| {
        published.registry_alias == registry_alias && published.repository_name == repository_name
    })
}

/// The PROGRAM the image `reference` names declares as its own, for a reference into a
/// repository of this gallery whose image this emulator holds and under a tag that
/// repository publishes ([`PublishedRepository::image_program`]).
///
/// `None` for every reference this gallery states no image of — another alias, a repository
/// whose releases are not held here, and a release this gallery does not publish — because
/// nothing here says what such an image declares, and for a reference addressing a DIGEST,
/// which names a release rather than one of the published tags (see the module docs).
///
/// A READ of the published catalogue alone: it takes no store and changes nothing.
#[must_use]
pub fn published_image_program(reference: &GalleryReference) -> Option<Vec<String>> {
    let tag = reference.image_tag.as_deref()?;
    let published = published_repository(&reference.registry_alias, &reference.repository_name)?;
    if !published.image_tags.contains(&tag) {
        return None;
    }
    Some(
        published
            .image_program
            .iter()
            .map(|&word| word.to_string())
            .collect(),
    )
}

/// Whether the gallery publishes the image `reference` names, and `None` where nothing here
/// decides it: a repository of the MIRRORED namespace whose releases this emulator does not
/// hold, and every repository of every other alias, as for any registry it holds no store for.
///
/// A repository the mirrored namespace does not publish at all is answered
/// ([`unheld_repository`]): the namespace's catalogue is published, so absence from it is a
/// fact about the registry rather than about this emulator's store.
///
/// A reference addressing a DIGEST is not answered either (see the module docs).
#[must_use]
pub fn published_image(reference: &GalleryReference) -> Option<PublishedImage> {
    let tag = reference.image_tag.as_deref()?;
    let Some(published) =
        published_repository(&reference.registry_alias, &reference.repository_name)
    else {
        // The mirrored namespace is the one this gallery's own alias for it carries, so only a
        // reference under THAT alias is answered from its catalogue.
        if reference.registry_alias != DOCKER_OFFICIAL_IMAGES_ALIAS {
            return None;
        }
        return unheld_repository(&reference.repository_name);
    };
    Some(if published.image_tags.contains(&tag) {
        PublishedImage::Published
    } else {
        PublishedImage::NotPublished
    })
}

/// Whether the same catalogue publishes the image a reference into the registry the gallery
/// MIRRORS those images from names: `repository_path` as that registry spells it (the bare
/// `<name>` of a Docker Official Image, or its full [`OFFICIAL_IMAGES_NAMESPACE`] path) under
/// the tag `image_tag`.
///
/// One repository, two spellings. The entries above are the gallery's mirror of the Docker
/// Official Images, and a mirror carries the releases of the repository it mirrors, so the
/// releases this emulator holds of such an image are the same whichever of the two registries
/// a reference addresses it through — which is what keeps one tag from naming an image under
/// one spelling and no image at all under the other.
///
/// `None` for every repository path nothing here decides: one the mirrored namespace publishes
/// whose releases this emulator does not hold, and a path under another namespace of that
/// registry, which is left undecided exactly as a reference into an alias no image is held of
/// is. A path the namespace publishes NO repository under is answered
/// ([`unheld_repository`]).
#[must_use]
pub fn official_image(repository_path: &str, image_tag: &str) -> Option<PublishedImage> {
    let repository_name = match repository_path.split_once('/') {
        // The official images live in ONE namespace of that registry; a path under any other
        // namespace names somebody else's repository there.
        Some((namespace, name)) if namespace == OFFICIAL_IMAGES_NAMESPACE => {
            format!("{OFFICIAL_IMAGES_NAMESPACE}/{name}")
        }
        Some(_) => return None,
        // A reference may name an official image without its namespace, which is the form a
        // container runtime completes with that namespace.
        None => format!("{OFFICIAL_IMAGES_NAMESPACE}/{repository_path}"),
    };
    let Some(published) = published_repository(DOCKER_OFFICIAL_IMAGES_ALIAS, &repository_name)
    else {
        return unheld_repository(&repository_name);
    };
    Some(if published.image_tags.contains(&image_tag) {
        PublishedImage::Published
    } else {
        PublishedImage::NotPublished
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference a case asks about, built as the gallery's own reading answers one.
    fn reference(alias: &str, repository: &str, tag: Option<&str>, digest: Option<&str>) -> GalleryReference {
        GalleryReference {
            registry_alias: alias.to_string(),
            repository_name: repository.to_string(),
            image_tag: tag.map(str::to_string),
            image_digest: digest.map(str::to_string),
        }
    }

    /// RULE: the program an image of this gallery declares is the one its published entry
    /// states ([`PublishedRepository::image_program`]), for a reference into a repository
    /// whose image this emulator holds and under a tag that repository publishes — and
    /// nothing is stated for a reference this gallery holds no image of, so a release it does
    /// not publish, a repository it holds no image of, another alias and a reference
    /// addressing a digest each declare nothing.
    #[test]
    fn a_published_image_declares_the_program_its_entry_states() {
        let shell = Some(vec!["sh".to_string()]);
        // (case, the reference, the program the image declares)
        let rows: &[(&str, GalleryReference, Option<Vec<String>>)] = &[
            (
                "a release of a held repository",
                reference("docker", "library/busybox", Some("1.36"), None),
                shell.clone(),
            ),
            (
                "another release of the same repository",
                reference("docker", "library/busybox", Some("musl"), None),
                shell.clone(),
            ),
            (
                "a held repository whose image runs a server",
                reference("docker", "library/nginx", Some("1.29"), None),
                Some(vec![
                    "nginx".to_string(),
                    "-g".to_string(),
                    "daemon off;".to_string(),
                ]),
            ),
            (
                "the other held server image",
                reference("docker", "library/httpd", Some("2.4-alpine"), None),
                Some(vec!["httpd-foreground".to_string()]),
            ),
            (
                "a tag the held repository publishes no release under",
                reference("docker", "library/busybox", Some("no-such-release"), None),
                None,
            ),
            (
                "a repository of that registry no image is held of",
                reference("docker", "library/python", Some("3.12"), None),
                None,
            ),
            (
                "a repository of another alias",
                reference("someone", "library/busybox", Some("1.36"), None),
                None,
            ),
            (
                "a reference addressing a digest",
                reference("docker", "library/busybox", None, Some("sha256:abc")),
                None,
            ),
        ];
        for (case, reference, expected) in rows {
            let declared = published_image_program(reference);
            assert_eq!(
                declared, *expected,
                "case {case:?}: expected {expected:?}, got {declared:?}"
            );
        }

        // EVERY held repository states a program: an entry that stated none would leave a
        // container of its image with nothing to run while the gallery holds the image.
        for published in PUBLISHED_REPOSITORIES {
            assert!(
                !published.image_program.is_empty(),
                "the repository {:?} must state the program its image declares",
                published.repository_name
            );
        }
    }

    /// RULE: a reference into a repository of the gallery whose image this emulator holds is
    /// decided by the tags that repository publishes — a published tag names an image and any
    /// other tag names none; a repository the MIRRORED namespace's published catalogue does
    /// not list names no repository at all; and a repository that catalogue does list whose
    /// releases are not held, an alias no image is held of, and a reference that addresses a
    /// digest are not answered at all.
    #[test]
    fn the_published_tags_of_a_held_repository_decide_a_reference() {
        // (case, the reference, what the catalogue says)
        let rows: &[(&str, GalleryReference, Option<PublishedImage>)] = &[
            (
                "a release of a held repository",
                reference("docker", "library/busybox", Some("1.36"), None),
                Some(PublishedImage::Published),
            ),
            (
                "the moving channel of a held repository",
                reference("docker", "library/nginx", Some("latest"), None),
                Some(PublishedImage::Published),
            ),
            (
                "a variant release of a held repository",
                reference("docker", "library/httpd", Some("2.4-alpine"), None),
                Some(PublishedImage::Published),
            ),
            (
                "a tag the held repository publishes no release under",
                reference("docker", "library/busybox", Some("no-such-release"), None),
                Some(PublishedImage::NotPublished),
            ),
            (
                "a version the held repository publishes no release under",
                reference("docker", "library/nginx", Some("0.1"), None),
                Some(PublishedImage::NotPublished),
            ),
            (
                "a repository of that registry no image is held of",
                reference("docker", "library/python", Some("3.12"), None),
                None,
            ),
            (
                "a repository that namespace publishes none of",
                reference("docker", "library/no-such-official-image", Some("1"), None),
                Some(PublishedImage::NoSuchRepository),
            ),
            (
                "a path under another namespace of the mirrored registry's alias",
                reference("docker", "someone/no-such-official-image", Some("1"), None),
                None,
            ),
            (
                "an alias no image is held of",
                reference("amazonlinux", "amazonlinux", Some("2023"), None),
                None,
            ),
            (
                "a digest, which names content rather than a release",
                reference("docker", "library/busybox", None, Some("sha256:abc")),
                None,
            ),
        ];
        for (case, asked, expected) in rows {
            let answered = published_image(asked);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }

    /// Every entry has to be reachable by the alias and repository a reference spells, and
    /// carry at least one tag — an entry with none would hold no release at all and so refuse
    /// every reference into a repository this emulator does hold the image of.
    #[test]
    fn every_published_entry_is_reachable_and_holds_releases() {
        for published in PUBLISHED_REPOSITORIES {
            let found = published_repository(published.registry_alias, published.repository_name);
            assert!(
                found.is_some(),
                "case {}/{}: the entry must be reachable by the reference's own spelling",
                published.registry_alias,
                published.repository_name
            );
            assert!(
                !published.image_tags.is_empty(),
                "case {}/{}: expected at least one published tag, got none",
                published.registry_alias,
                published.repository_name
            );
        }
    }

    /// RULE: the releases of a held image decide a reference into the repository the mirror
    /// carries it FROM as well — with or without that registry's namespace; a name that
    /// namespace's published catalogue does not list names no repository there; and a path
    /// under another namespace, and a repository the catalogue lists no image of which is
    /// held, are left undecided.
    ///
    /// The rows differ along the spelling of the path, the repository named, and whether the
    /// tag asked for is one the entry publishes; `library/httpd` is a repository the table
    /// above does not reach through this spelling.
    #[test]
    fn a_held_images_releases_decide_its_mirrored_repositorys_spelling_too() {
        // (case, repository path, tag, what the catalogue says)
        let rows: &[(&str, &str, &str, Option<PublishedImage>)] = &[
            (
                "the bare name of a held repository, on a published release",
                "busybox",
                "1.36",
                Some(PublishedImage::Published),
            ),
            (
                "the namespaced path of a held repository, on a published release",
                "library/busybox",
                "1.36",
                Some(PublishedImage::Published),
            ),
            (
                "the moving channel of a held repository",
                "library/httpd",
                "latest",
                Some(PublishedImage::Published),
            ),
            (
                "a variant release of a held repository",
                "nginx",
                "1.27-alpine",
                Some(PublishedImage::Published),
            ),
            (
                "a tag the held repository publishes no release under",
                "busybox",
                "no-such-release",
                Some(PublishedImage::NotPublished),
            ),
            (
                "the same under the namespaced spelling",
                "library/nginx",
                "no-such-release",
                Some(PublishedImage::NotPublished),
            ),
            (
                "a repository of that registry no image is held of",
                "library/python",
                "3.12",
                None,
            ),
            (
                "the bare name of such a repository",
                "maven",
                "3",
                None,
            ),
            (
                "a bare name that namespace publishes no repository under",
                "no-such-official-image",
                "1.0",
                Some(PublishedImage::NoSuchRepository),
            ),
            (
                "the same under the namespaced spelling",
                "library/no-such-official-image",
                "1.0",
                Some(PublishedImage::NoSuchRepository),
            ),
            (
                "a path under somebody else's namespace of that registry",
                "someone/busybox",
                "1.36",
                None,
            ),
            (
                "a path under somebody else's namespace naming no official image either",
                "someone/no-such-official-image",
                "1.0",
                None,
            ),
        ];
        for (case, repository_path, image_tag, expected) in rows {
            let answered = official_image(repository_path, image_tag);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }

    /// Every repository this emulator holds an image of under the mirrored alias is a
    /// repository that namespace PUBLISHES: the entries are its mirror of those images, so a
    /// held entry the catalogue did not list would answer "no such repository" for the very
    /// reference it holds the image for.
    #[test]
    fn every_held_mirrored_entry_is_a_repository_that_namespace_publishes() {
        for published in PUBLISHED_REPOSITORIES
            .iter()
            .filter(|published| published.registry_alias == DOCKER_OFFICIAL_IMAGES_ALIAS)
        {
            assert_eq!(
                official_images_namespace_publishes(published.repository_name),
                Some(true),
                "case {}: expected the mirrored namespace to publish the held repository, \
                 its catalogue does not list it",
                published.repository_name
            );
        }
    }
}
