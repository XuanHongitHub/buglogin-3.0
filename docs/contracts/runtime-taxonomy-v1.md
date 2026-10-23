# Runtime Taxonomy v1

## Canonical IDs

| ID | Role | Compatibility status |
|---|---|---|
| `wayfern` | BugLogin runtime | The only runtime selectable for new profiles, installable and launchable |
| `bugmini` | Legacy runtime (retired 2026-10-23) | Preserve existing profiles; not selectable or launchable |
| `bugox` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bugium` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bugthor` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bughelium` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |

## Upstream aliases

`cloak`, `thorium`, and `helium` are accepted only at adapter/import boundaries. They normalize to the canonical IDs above only when the adapter declares a verified mapping. Unknown or unsupported settings remain preserved as unsupported data; never silently converted.

## Contract rule

Persist canonical IDs in new domain records, manifests, audit events, and API/MCP payloads. Preserve the original runtime string in migration provenance.
