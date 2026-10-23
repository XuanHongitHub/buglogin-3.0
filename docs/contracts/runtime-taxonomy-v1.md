# Runtime Taxonomy v1

## Canonical IDs

| ID | Role | Compatibility status |
|---|---|---|
| `wayfern` | Primary BugLogin runtime | Selectable for new profiles; required baseline |
| `bugmini` | Secondary BugLogin runtime | Selectable where a verified local binary is configured |
| `bugox` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bugium` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bugthor` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |
| `bughelium` | Legacy runtime | Preserve existing profiles; not selectable until an adapter is approved |

## Upstream aliases

`cloak`, `thorium`, and `helium` are accepted only at adapter/import boundaries. They normalize to the canonical IDs above only when the adapter declares a verified mapping. Unknown or unsupported settings remain preserved as unsupported data; never silently converted.

## Contract rule

Persist canonical IDs in new domain records, manifests, audit events, and API/MCP payloads. Preserve the original runtime string in migration provenance.
