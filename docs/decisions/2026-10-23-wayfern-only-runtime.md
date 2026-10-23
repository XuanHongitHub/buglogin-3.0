# Wayfern is the only provisionable runtime (2026-10-23)

- New profiles can only be created with `wayfern` (`ProfileCreateRuntime`), and only Wayfern releases are accepted from the signed runtime catalog, installed, approved or launched by the desktop and API.
- The Bugmini adapter, its runtime targets, environment overrides (`BUGLOGIN_BUGMINI_*`), install records and Windows smoke script are removed.
- `bugmini` stays in the `Runtime` taxonomy as a legacy value, like `bugox`/`bugium`: existing records still deserialize and display, but are not launchable. Legacy imports map `bugmini` to `preserved`, and migration inspection classifies it `unsupported` / `runtime_adapter_not_approved`.
