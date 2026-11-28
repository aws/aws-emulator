Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-configuration-template-lifecycle-and-validation
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-configuration-template-lifecycle-and-validation elasticbeanstalk-environment-create-option-setting-reflected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*]
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*]
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].Value
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].Namespace
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].OptionName
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].OptionSource
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].OptionSource.Identifier
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].OptionSource.Origin
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettingDetails.member[*].ResourceName
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].Namespace
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].OptionName
Issue-Key: elasticbeanstalk|DescribeConfigurationSettings|2xx->2xx|$.DescribeConfigurationSettingsResponse.DescribeConfigurationSettingsResult.ConfigurationSettings.member.OptionSettings.member[*].ResourceName
Issue-Commit: 0c46b3cc7da45ad21000fa4a6694a9d98cd58e91

# Elastic Beanstalk DescribeConfigurationSettings returns an option list whose membership, order and OptionSource diverge from AWS (2 causes)

> _Note: this issue was identified against the code base at commit `0c46b3cc7da45ad21000fa4a6694a9d98cd58e91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationSettings

## Description
DescribeConfigurationSettings is served with a configuration-settings body that does not match the one AWS returns for the same application/template/environment. Two distinct causes, both inside the DescribeConfigurationSettings response composition, are bundled here because they share one root fault: the emulator builds the per-settings option list from a fixed internal table instead of composing the effective option set AWS reports, and it never emits the per-option source.

Cause 1 of 2: the OptionSettings / OptionSettingDetails list of a ConfigurationSettingsDescription has the wrong membership and the wrong order.
AWS: Smithy (elasticbeanstalk, DescribeConfigurationSettings -> ConfigurationSettingsDescriptions.ConfigurationSettings: ConfigurationSettingsDescriptionList of ConfigurationSettingsDescription, member OptionSettings: ConfigurationOptionSettingsList of ConfigurationOptionSetting{ResourceName, Namespace, OptionName, Value}). The model constrains the shape but not which options appear; the recorded AWS response (recorded behavior only for membership/order) returns the full effective option set for the settings, including the aws:cloudformation:template:parameter options (StaticFiles, EnvironmentVariables, AppSource, HooksPkgUrl) and the platform's aws:elasticbeanstalk:application:environment / aws:elasticbeanstalk:container:<platform> options, in AWS's own stable order. AWS also returns a parallel OptionSettingDetails list (not in the Smithy model - recorded behavior only) carrying the same entries plus an OptionSource member.
Emulator: for the same request the list is a different set in a different order: at the position where AWS reports {Namespace aws:cloudformation:template:parameter, OptionName StaticFiles} the emulator reports {Namespace aws:cloudformation:template:parameter, OptionName EnvironmentVariables}; where AWS reports {Namespace aws:elasticbeanstalk:application:environment, OptionName PYTHONPATH, OptionSource {Identifier Service, Origin Service}} the emulator reports {Namespace aws:autoscaling:asg, OptionName Cooldown}; where AWS reports the aws:cloudformation:template:parameter options EnvironmentVariables / AppSource / HooksPkgUrl the emulator reports aws:autoscaling:asg MinSize and aws:autoscaling:launchconfiguration IamInstanceProfile. The divergence runs across the whole list (both OptionSettings and OptionSettingDetails, element indices 0..117 in the recorded step), and several OptionSettings entries also differ in Value. Status is 200 on both sides, so there is no error code or message involved.
Why: a client that reads its deployed configuration back - the AWS CLI 'aws elasticbeanstalk describe-configuration-settings', the EB CLI, or any tool that looks an option up by (Namespace, OptionName) before issuing UpdateEnvironment - finds options AWS does not report, misses the aws:cloudformation:template:parameter and platform-container options AWS does report, and reads a different value at a given list position. Positional readers pick up the wrong option entirely; set-based readers conclude an option is unset and either skip a required update or re-send a default, silently changing configuration.
Fix: the DescribeConfigurationSettings handler and the state it reads - the effective-option-set composition for a configuration template / environment (merge of platform defaults, template-created options and API-applied options), plus the serializer that orders the ConfigurationOptionSettingsList. The list must be the effective option set AWS reports, in AWS's order, not a fixed internal table.
Traces: elasticbeanstalk-configuration-template-lifecycle-and-validation#task#6 (cluster representative; the same cluster also fails at elasticbeanstalk-configuration-template-lifecycle-and-validation#task#9).

Cause 2 of 2: OptionSettingDetails entries omit OptionSource entirely.
AWS: OptionSource is not in the Smithy model (recorded behavior only). The recording shows AWS emitting, on every OptionSettingDetails entry, an OptionSource that is either the structure {Identifier, Origin} - {Identifier API, Origin API} for an option the caller supplied through the API and {Identifier Service, Origin Service} for one the service/platform contributed - or the empty element for an option with no attributable source. The Identifier/Origin values are not per-run data: they are determined by where the option came from (the request, or the service).
Emulator: for the same step every OptionSettingDetails entry is serialized with OptionSource absent (expected {Identifier API, Origin API} at member[1], {Identifier Service, Origin Service} at member[2], the empty element at member[3]; got None in each case). On top of that the entries at those positions are the wrong options, which is Cause 1 showing through on this fixture too: at member[1] AWS reports {aws:elasticbeanstalk:application:environment, SPRING_PROFILES_ACTIVE} and the emulator reports {aws:cloudformation:template:parameter, StaticFiles}; at member[2] AWS reports {aws:elasticbeanstalk:container:python, NumThreads} and the emulator reports {aws:cloudformation:template:parameter, EnvironmentVariables}. Status is 200 on both sides.
Why: OptionSource is how a client distinguishes an option it set through the API from one the platform supplied. With it missing, a caller that round-trips settings cannot tell which options it owns, so it either re-asserts service-owned defaults as API-owned options on the next UpdateEnvironment, or drops its own API-set options as if they were service defaults. An option the test asserted was reflected back from CreateEnvironment is reported with no source at all.
Fix: the DescribeConfigurationSettings serializer (OptionSettingDetails member emission) plus the state that records provenance per option setting - each stored option needs an origin (API vs Service) so OptionSource can be emitted, with the empty element for options that have none.
Traces: elasticbeanstalk-environment-create-option-setting-reflected#task#5.

Neither cause is covered by an existing open issue: the replay marks both clusters unowned. The nearest open issues are 20261115T085940Z-elasticbeanstalk-elastic-beanstalk-configuration-option-validation-unknown-namespaces-options.md (which is about validation of unknown namespaces/options on write, not the composition of the read-back list) and 20261109T203350Z-elasticbeanstalk-environment-reads-drop-modeled-members-and-mint.md (which owns the DescribeEnvironments member/CNAME divergence, a different operation).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 157 steps, 11 mismatched).

Cause 1 - fixture elasticbeanstalk-configuration-template-lifecycle-and-validation, step task#6 (also task#9), DescribeConfigurationSettings, expected 200 / actual 200:
  $...ConfigurationSettings.member.OptionSettingDetails.member[2]: missing (expected {'OptionSource': '', 'Namespace': 'aws:cloudformation:template:parameter', 'OptionName': 'StaticFiles'}, got {'Namespace': 'aws:cloudformation:template:parameter', 'OptionName': 'EnvironmentVariables'})
  $...OptionSettingDetails.member[5]: missing (expected {'OptionSource': {'Identifier': 'Service', 'Origin': 'Service'}, 'Namespace': 'aws:elasticbeanstalk:application:environment', 'OptionName': 'PYTHONPATH'}, got {'Namespace': 'aws:autoscaling:asg', 'OptionName': 'Cooldown', 'OptionSource': {'Identifier': 'Service', 'Origin': 'Service'}})
  $...OptionSettingDetails.member[7]: missing (expected {'OptionSource': {'Identifier': 'Service', 'Origin': 'Service'}, 'Namespace': 'aws:cloudformation:template:parameter', 'OptionName': 'EnvironmentVariables'}, got {'Namespace': 'aws:autoscaling:asg', 'OptionName': 'MinSize', 'OptionSource': {'Identifier': 'API', 'Origin': 'API'}})
  $...OptionSettingDetails.member[9]: missing (expected {... 'OptionName': 'AppSource'}, got {'Namespace': 'aws:autoscaling:launchconfiguration', 'OptionName': 'IamInstanceProfile', ...})
  $...OptionSettingDetails.member[11]: missing (expected {... 'OptionName': 'HooksPkgUrl'}, ...)
  divergence_key lists mismatching keys across OptionSettingDetails.member[2..116] and OptionSettings.member[0..117], including OptionSettings.member[9].Value, member[31].Value, member[34].Value, member[48].Value.

Cause 2 - fixture elasticbeanstalk-environment-create-option-setting-reflected, step task#5, DescribeConfigurationSettings, expected 200 / actual 200:
  $...OptionSettingDetails.member[1].OptionSource: missing (expected {'Identifier': 'API', 'Origin': 'API'}, got None)
  $...OptionSettingDetails.member[1].Namespace: value_mismatch (expected 'aws:elasticbeanstalk:application:environment', got 'aws:cloudformation:template:parameter')
  $...OptionSettingDetails.member[1].OptionName: value_mismatch (expected 'SPRING_PROFILES_ACTIVE', got 'StaticFiles')
  $...OptionSettingDetails.member[2].OptionSource: missing (expected {'Identifier': 'Service', 'Origin': 'Service'}, got None)
  $...OptionSettingDetails.member[2].Namespace: value_mismatch (expected 'aws:elasticbeanstalk:container:python', got 'aws:cloudformation:template:parameter')
  $...OptionSettingDetails.member[2].OptionName: value_mismatch (expected 'NumThreads', got 'EnvironmentVariables')
  $...OptionSettingDetails.member[3].OptionSource: missing (expected '', got None)
  $...OptionSettingDetails.member[3].Namespace: value_mismatch (expected 'aws:cloudformation:template:parameter', got 'aws:elasticbeanstalk:application:environment')

Model grounding: Smithy elasticbeanstalk DescribeConfigurationSettings -> ConfigurationSettingsDescriptions{ConfigurationSettings: ConfigurationSettingsDescriptionList}; ConfigurationSettingsDescription.OptionSettings: ConfigurationOptionSettingsList; ConfigurationOptionSetting{ResourceName, Namespace, OptionName, Value}. OptionSettingDetails and OptionSource are not modeled - recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture elasticbeanstalk-configuration-template-lifecycle-and-validation and compare step task#6 (and task#9) - CreateApplication, CreateConfigurationTemplate / CreateEnvironment, then DescribeConfigurationSettings for that application + template/environment. Diff the returned ConfigurationSettings[0].OptionSettings and OptionSettingDetails lists against the recorded response: membership and order differ from the first aws:cloudformation:template:parameter entry onward, and several Values differ.
4. Cause 2: replay fixture elasticbeanstalk-environment-create-option-setting-reflected and compare step task#5 - create an environment with an explicit OptionSettings entry (aws:elasticbeanstalk:application:environment / SPRING_PROFILES_ACTIVE) via the API, then call DescribeConfigurationSettings for that environment. Every OptionSettingDetails entry comes back without OptionSource (AWS returns {Identifier: API, Origin: API} for the API-supplied option, {Identifier: Service, Origin: Service} for service-contributed options and the empty element where there is no source), and the entries at member[1..3] are different options than AWS reports.
5. Both steps answer HTTP 200 on both sides; the divergence is entirely in the response body, so the check is a body diff, not a status/error-code comparison.

## Resolution

Cause 2 is fixed as a rule: an `OptionSettingDetails` entry now ALWAYS carries an
`OptionSource`, naming the layer the value in force came from — `API` where the
configuration set itself holds the setting, `Service` where the platform version
put the value there — and naming nothing, as the EMPTY element, for an option in
force at no value. The emulator previously dropped the member entirely for such
an option, which is what the recording reported as `expected '', got None`.

Cause 1 is fixed in its composition rule: a read of a configuration set reports
the EFFECTIVE option set of the platform version the set is for — every option
that version SUPPLIES to the environment it provisions plus every option its
published catalog DEFINES, each at the value in force (the configuration's own
setting, else the published `DefaultValue`, else no value). The set was
previously filtered down to the options the catalog publishes a non-empty
default for, so an option the version defines without a default was missing from
the read altogether; for a Python platform version the reported set grows from 58
options to 113 against the 118 the recorded step carries. Beside that rule, the
catalog itself gained what it had been missing: the two `aws:cloudformation:
template:parameter` entries the recorded set carries for every environment
(`AppSource`, `HooksPkgUrl`), the whole `aws:autoscaling:trigger` family (ten
options the published option reference documents for every environment, which
this catalog described none of), the documented defaults of the classic load
balancer's health-check target and listener ports and of the rolling update's
timeout, and a value in force for the environment property a Python runtime
supplies — the directory the platform runs the deployed application from — which
is why the recording attributes it to the service.

What the replay still reports, left here so the gap is not lost: the recorded
answer's exact membership and order (118 entries in the service's own sequence)
is not reached. What remains is the CONTENT of the per-platform-version option
catalog AWS publishes — which options that version defines, in which order it
lists them — which is data this emulator does not hold and which the recorded
values are withheld for; the catalog here is written from Elastic Beanstalk's
published configuration-option reference. That gap is the subject of the open
`DescribeConfigurationOptions` catalog issue
(20261029T180136Z-elasticbeanstalk-elastic-beanstalk-accepts-inputs-aws-refuses-and-publishes.md).
No rule of the composition or of the attribution is missing here.
