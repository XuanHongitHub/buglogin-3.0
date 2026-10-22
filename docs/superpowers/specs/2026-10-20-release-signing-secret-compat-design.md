# Release signing credential compatibility

## Goal

Allow the protected macOS signing workflow to consume the Apple credentials already configured on the designated BugLogin GitHub repository without copying or renaming secret values.

## Contract

- Prefer the explicit workflow names `APPLE_CERTIFICATE_P12_BASE64` and `APPLE_APP_PASSWORD` when configured.
- Accept the existing repository names `APPLE_CERTIFICATE` and `APPLE_PASSWORD` as compatibility fallbacks. The former is the base64 PKCS#12 bundle; the latter is the Apple app-specific notarization password.
- Continue requiring `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, and `APPLE_TEAM_ID`.
- If `APPLE_SIGNING_IDENTITY` is configured, require it to match an imported valid identity for the configured team. Otherwise derive the identity from the imported keychain and require exactly a `Developer ID Application` identity whose Team ID matches `APPLE_TEAM_ID`.
- Export the selected identity only to subsequent steps in that workflow job. Do not print credentials or weaken certificate, hardened-runtime, notarization, stapling, signature, or release-evidence checks.
- Do not change Windows signing, runtime catalog trust, updater metadata, release approval, or publishing gates.

## Verification

- Validate the workflow YAML and expression syntax without printing secret values.
- Review the diff to ensure all existing fail-closed gates remain and only Apple secret-name resolution changes.
- Actual Apple signing/notarization remains proven only by the protected native GitHub workflow; local static validation is not release evidence.
