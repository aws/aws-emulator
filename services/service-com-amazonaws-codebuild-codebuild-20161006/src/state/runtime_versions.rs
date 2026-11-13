// SPDX-License-Identifier: CC0-1.0

//! The RUNTIMES a build environment has, and the ones a build installs.
//!
//! # The rule this module implements
//!
//! A buildspec's `phases.install.runtime-versions` selects which version of a runtime the
//! build INSTALLS, and what it may select is what the build environment HAS: a runtime the
//! environment does not provide, or a version of one it does not have, is a specification
//! the build cannot be prepared from, so the build FAILS in the phase that prepares it
//! rather than running anything — each with the `PhaseContext$statusCode` CodeBuild gives
//! that kind of failure ([`RuntimeInstallFailure`]). A selection the environment can
//! satisfy installs that runtime, and the programs that version of it carries
//! ([`ProvidedVersion::programs`]) then answer the build's commands with the version that
//! was installed — so what a build observes follows from its own selection and from what
//! the environment has, and from nothing else. `latest` selects the newest version of a
//! runtime the environment has, which is the value AWS's reference documents for "the most
//! recent version".
//!
//! # What an installed runtime performs here
//!
//! The build environment runs IN THIS PROCESS (see [`super::build_shell`]), so the only
//! question an installed runtime's program answers is the one whose answer IS the
//! installation: its version query, in the spellings that program takes. Any OTHER
//! invocation would have to run a program with that runtime, which this environment does
//! not do, so it is answered as a command that did not run — the same closed failure as a
//! command the environment has no program for at all. Nothing here reports output, a status
//! or a duration for a program nothing performed.
//!
//! A runtime the environment has but carries no program OF is installable and runs nothing:
//! its version query is answered the same closed way. That is deliberate rather than
//! unfinished — `go version`, `ruby -v`, `php -v` and `java --version` write a line carrying
//! a build revision, a release date or the host's platform triple, none of which this
//! environment has anything to take from, and writing one anyway would be output nothing
//! produced. A runtime whose version line is the version and nothing else carries its
//! programs below.
//!
//! # Where the catalog comes from
//!
//! [`PROVIDED_RUNTIMES`] is the runtimes CodeBuild's build images provide and the values a
//! buildspec selects a version of each with, from AWS's buildspec reference for
//! `runtime-versions` (the `android`, `dotnet`, `golang`, `java`, `nodejs`, `php`, `python`
//! and `ruby` runtimes, each listed with the versions a current standard image carries).
//! The version a runtime's program REPORTS is that release's own version string, as the
//! runtime itself writes it, and the arguments it answers are the ones that program takes
//! for a version query.

use std::collections::BTreeMap;

use ::service_common::posix_shell_session::{EnvironmentPrograms, ProgramOutcome};

/// One program a version of a runtime carries: the name a command line invokes it by, the
/// arguments that ask it for its version, and the line it writes for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvidedProgram {
    /// The name a command line invokes this program by.
    pub name: &'static str,
    /// The arguments this program answers its version to — the spellings that program takes,
    /// which differ between them (`-v`, `--version`, `-V`).
    pub version_queries: &'static [&'static str],
    /// The line it writes for a version query.
    pub reported: &'static str,
}

/// One version of one runtime the build environment has.
#[derive(Debug, Clone, Copy)]
pub struct ProvidedVersion {
    /// The value a buildspec's `runtime-versions` selects this version with.
    pub selector: &'static str,
    /// The programs this version of the runtime carries.
    pub programs: &'static [ProvidedProgram],
}

/// One runtime the build environment has.
#[derive(Debug, Clone, Copy)]
pub struct ProvidedRuntime {
    /// The name a buildspec's `runtime-versions` names this runtime by.
    pub name: &'static str,
    /// The versions of it the environment has, in the order AWS lists them.
    pub versions: &'static [ProvidedVersion],
}

/// One program of a runtime, as the environment carries it: `node` and `npm` take `-v` and
/// `--version`, `python` and `python3` take `-V` and `--version` (NOT `-v`, which asks that
/// program for a verbose trace), and the `dotnet` driver takes `--version`.
const fn program(
    name: &'static str,
    version_queries: &'static [&'static str],
    reported: &'static str,
) -> ProvidedProgram {
    ProvidedProgram {
        name,
        version_queries,
        reported,
    }
}

/// The arguments `node` and `npm` answer their version to.
const NODE_QUERIES: &[&str] = &["-v", "--version"];

/// The arguments `python` and `python3` answer their version to.
const PYTHON_QUERIES: &[&str] = &["-V", "--version"];

/// The argument the `dotnet` driver answers its SDK version to.
const DOTNET_QUERIES: &[&str] = &["--version"];

/// The versions of `nodejs` the environment has: the version `node` writes for a version
/// query (`v<version>`, as it writes it) and the version of the npm that release of
/// Node.js bundles.
const NODEJS_VERSIONS: &[ProvidedVersion] = &[
    ProvidedVersion {
        selector: "18",
        programs: &[
            program("node", NODE_QUERIES, "v18.20.8"),
            program("npm", NODE_QUERIES, "10.8.2"),
        ],
    },
    ProvidedVersion {
        selector: "20",
        programs: &[
            program("node", NODE_QUERIES, "v20.19.0"),
            program("npm", NODE_QUERIES, "10.8.2"),
        ],
    },
    ProvidedVersion {
        selector: "22",
        programs: &[
            program("node", NODE_QUERIES, "v22.14.0"),
            program("npm", NODE_QUERIES, "10.9.2"),
        ],
    },
    ProvidedVersion {
        selector: "24",
        programs: &[
            program("node", NODE_QUERIES, "v24.0.0"),
            program("npm", NODE_QUERIES, "11.3.0"),
        ],
    },
];

/// The versions of `python` the environment has, with the line `python` and `python3` write
/// for a version query (`Python <version>`, as they write it).
const PYTHON_VERSIONS: &[ProvidedVersion] = &[
    ProvidedVersion {
        selector: "3.9",
        programs: &[
            program("python", PYTHON_QUERIES, "Python 3.9.21"),
            program("python3", PYTHON_QUERIES, "Python 3.9.21"),
        ],
    },
    ProvidedVersion {
        selector: "3.10",
        programs: &[
            program("python", PYTHON_QUERIES, "Python 3.10.16"),
            program("python3", PYTHON_QUERIES, "Python 3.10.16"),
        ],
    },
    ProvidedVersion {
        selector: "3.11",
        programs: &[
            program("python", PYTHON_QUERIES, "Python 3.11.11"),
            program("python3", PYTHON_QUERIES, "Python 3.11.11"),
        ],
    },
    ProvidedVersion {
        selector: "3.12",
        programs: &[
            program("python", PYTHON_QUERIES, "Python 3.12.9"),
            program("python3", PYTHON_QUERIES, "Python 3.12.9"),
        ],
    },
    ProvidedVersion {
        selector: "3.13",
        programs: &[
            program("python", PYTHON_QUERIES, "Python 3.13.2"),
            program("python3", PYTHON_QUERIES, "Python 3.13.2"),
        ],
    },
];

/// The versions of `dotnet` the environment has, with the SDK version the driver writes for
/// `--version`, which is that version and nothing else.
const DOTNET_VERSIONS: &[ProvidedVersion] = &[
    ProvidedVersion {
        selector: "6.0",
        programs: &[program("dotnet", DOTNET_QUERIES, "6.0.428")],
    },
    ProvidedVersion {
        selector: "8.0",
        programs: &[program("dotnet", DOTNET_QUERIES, "8.0.404")],
    },
    ProvidedVersion {
        selector: "9.0",
        programs: &[program("dotnet", DOTNET_QUERIES, "9.0.101")],
    },
];

/// A version of a runtime the environment has and carries no program of its own for: it is
/// installable, and its own programs are ones this environment performs none of — see this
/// module's header for why writing their version line would be output nothing produced.
const fn installable(selector: &'static str) -> ProvidedVersion {
    ProvidedVersion {
        selector,
        programs: &[],
    }
}

/// The versions of `android` the environment has, as the API levels a buildspec selects.
const ANDROID_VERSIONS: &[ProvidedVersion] = &[
    installable("29"),
    installable("30"),
    installable("31"),
    installable("32"),
    installable("33"),
];

/// The versions of `golang` the environment has.
const GOLANG_VERSIONS: &[ProvidedVersion] = &[
    installable("1.20"),
    installable("1.21"),
    installable("1.22"),
    installable("1.23"),
    installable("1.24"),
];

/// The versions of `java` the environment has, as the Corretto distributions a buildspec
/// selects.
const JAVA_VERSIONS: &[ProvidedVersion] = &[
    installable("corretto8"),
    installable("corretto11"),
    installable("corretto17"),
    installable("corretto21"),
];

/// The versions of `php` the environment has.
const PHP_VERSIONS: &[ProvidedVersion] =
    &[installable("8.1"), installable("8.2"), installable("8.3")];

/// The versions of `ruby` the environment has.
const RUBY_VERSIONS: &[ProvidedVersion] =
    &[installable("3.2"), installable("3.3"), installable("3.4")];

/// Every runtime the build environment has, with the versions of it a buildspec may
/// select. See this module's header for the source.
pub const PROVIDED_RUNTIMES: &[ProvidedRuntime] = &[
    ProvidedRuntime {
        name: "android",
        versions: ANDROID_VERSIONS,
    },
    ProvidedRuntime {
        name: "dotnet",
        versions: DOTNET_VERSIONS,
    },
    ProvidedRuntime {
        name: "golang",
        versions: GOLANG_VERSIONS,
    },
    ProvidedRuntime {
        name: "java",
        versions: JAVA_VERSIONS,
    },
    ProvidedRuntime {
        name: "nodejs",
        versions: NODEJS_VERSIONS,
    },
    ProvidedRuntime {
        name: "php",
        versions: PHP_VERSIONS,
    },
    ProvidedRuntime {
        name: "python",
        versions: PYTHON_VERSIONS,
    },
    ProvidedRuntime {
        name: "ruby",
        versions: RUBY_VERSIONS,
    },
];

/// The value AWS's `runtime-versions` reference documents for the most recent version of a
/// runtime the image has.
const LATEST_VERSION: &str = "latest";

/// What CodeBuild states when a buildspec's `runtime-versions` names a runtime the build
/// image has none of: the name it named, and the runtimes the image does have.
fn unknown_runtime_message(name: &str) -> String {
    let provided: Vec<&str> = PROVIDED_RUNTIMES
        .iter()
        .map(|runtime| runtime.name)
        .collect();
    format!(
        "Unknown runtime named '{name}'. Should be one of: {}",
        provided.join(", ")
    )
}

/// What CodeBuild states when a buildspec selects a version of a runtime the build image
/// does not have.
fn unknown_runtime_version_message(runtime: &str, version: &str) -> String {
    format!(
        "Unknown runtime version named '{version}' of {runtime}. This build image requires \
         selecting at least one runtime version."
    )
}

/// The runtimes one build installed, as the programs they carry.
///
/// This is what the build's shell asks for a command its own catalogs do not name, so an
/// installed runtime's program is reachable exactly in the build that installed it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InstalledRuntimes {
    /// Each program an installed runtime carries, keyed by the name a command line invokes
    /// it by — the catalog's own entry, so what it answers and what it answers TO are the
    /// ones that version of the runtime carries.
    programs: BTreeMap<String, ProvidedProgram>,
}

impl EnvironmentPrograms for InstalledRuntimes {
    /// The version an installed runtime's program answers a version query with.
    ///
    /// Every other invocation answers `None` — a program this environment did not perform —
    /// because running one with the runtime is what this environment does not do, and a
    /// status for a program nothing ran would be a status for work nothing performed.
    fn run(&self, name: &str, arguments: &[String]) -> Option<ProgramOutcome> {
        let program = self.programs.get(name)?;
        let queries_version = arguments.len() == 1
            && program
                .version_queries
                .contains(&arguments.first().map(String::as_str).unwrap_or_default());
        if !queries_version {
            return None;
        }
        Some(ProgramOutcome {
            status: 0,
            output: vec![program.reported.to_string()],
        })
    }
}

/// Why the build environment could not install what a buildspec's `runtime-versions`
/// selected.
///
/// The two conditions are different failures of the build, not one: a runtime NO build
/// image has is the buildspec declaring something CodeBuild cannot read as a runtime, while
/// a runtime the image has at other versions only is an installation that could not be
/// performed — so each is reported with the `PhaseContext$statusCode` CodeBuild gives that
/// kind of failure (see [`super::buildspec`], which pairs them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeInstallFailure {
    /// The buildspec named a runtime the build environment has none of.
    UnknownRuntime(String),
    /// The buildspec selected a version of a provided runtime the environment does not
    /// have.
    UnknownVersion(String),
}

impl RuntimeInstallFailure {
    /// What CodeBuild states about this failure, as the phase context's message.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::UnknownRuntime(message) | Self::UnknownVersion(message) => message,
        }
    }
}

/// Install the runtimes `selected` names — the `runtime-versions` of a buildspec's install
/// phase, as the runtime and the version each entry selects — or answer what CodeBuild
/// states about the selection the environment cannot satisfy.
///
/// The first unsatisfiable entry is what is reported, in the order the buildspec declares
/// them, because that is the entry the build could not be prepared from.
pub fn install_runtimes(
    selected: &[(String, String)],
) -> Result<InstalledRuntimes, RuntimeInstallFailure> {
    let mut installed = InstalledRuntimes::default();
    for (runtime, version) in selected {
        let Some(provided) = PROVIDED_RUNTIMES
            .iter()
            .find(|candidate| candidate.name == runtime)
        else {
            return Err(RuntimeInstallFailure::UnknownRuntime(
                unknown_runtime_message(runtime),
            ));
        };
        // `latest` is "the most recent version" the image has of that runtime, which is the
        // last one the catalog lists, since each runtime's versions are listed in the order
        // AWS lists them.
        let installable = if version == LATEST_VERSION {
            provided.versions.last()
        } else {
            provided
                .versions
                .iter()
                .find(|candidate| candidate.selector == version)
        };
        let Some(installable) = installable else {
            return Err(RuntimeInstallFailure::UnknownVersion(
                unknown_runtime_version_message(runtime, version),
            ));
        };
        for program in installable.programs {
            installed.programs.insert(program.name.to_string(), *program);
        }
    }
    Ok(installed)
}
