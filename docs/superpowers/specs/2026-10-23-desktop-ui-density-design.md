# Desktop UI density cleanup

## Scope

Make the About and Create Profile surfaces more compact without changing update,
runtime catalog, or download behavior.

## Behavior

- About keeps the existing app update check action and removes secondary website,
  GitHub, and license/open-source presentation from the dialog.
- Runtime versions remain full values internally and in command payloads, but the
  Create Profile selector and status surfaces display the major version only
  (`154`, `155`).
- Installed status stays concise.
- Browser download percentage is shown in the modal progress bar and the global
  download toast only. The inline status pill communicates activity without a
  second percentage.
- Fingerprint summary labels use concise localized wording for the device-default
  screen setting.

## Non-goals

- No runtime catalog, download, update, or backend changes.
- No removal of open-source/license copy from unrelated onboarding or legal views.
- No change to the selected version value sent when creating a profile.
