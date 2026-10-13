# Rust API Container CI Plan

- [x] Confirm `Dockerfile.api` is the documented supported production API image and inspect current CI aggregation.
- [x] Add the CI image build and local-mode smoke check for health and exact OpenAPI bytes.
- [x] Add a production-mode missing-PostgreSQL fail-closed assertion.
- [x] Parse workflow contracts with the repository's contract gate.
- [x] Add contract regression coverage that makes aggregate CI require the image job.
- [x] Run contract tests, API generation check, and diff validation.
- [ ] Run the image build and container smoke tests on an Ubuntu CI runner.

## Status

Workflow implementation, contract regression, and API generation checks pass. Local Docker daemon is unavailable, so the image build and smoke tests remain pending an Ubuntu CI run. No deployment or live staging verification has occurred.
