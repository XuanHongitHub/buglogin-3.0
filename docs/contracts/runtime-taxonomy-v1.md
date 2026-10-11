# Runtime Taxonomy v1

## Canonical IDs

| ID | Role | Compatibility status |
|---|---|---|
| `wayfern` | Primary BugLogin runtime | Required launch runtime |
| `bugox` | Legacy BugLogin adapter | Explicit capability matrix |
| `bugium` | Legacy BugLogin adapter | Explicit capability matrix |
| `bugmini` | Legacy BugLogin adapter | Explicit capability matrix |
| `bugthor` | Legacy BugLogin adapter | Explicit capability matrix |
| `bughelium` | Legacy BugLogin adapter | Explicit capability matrix |

## Upstream aliases

`cloak`, `thorium`, and `helium` are accepted only at adapter/import boundaries. They normalize to the canonical IDs above only when the adapter declares a verified mapping. Unknown or unsupported settings remain preserved as unsupported data; never silently converted.

## Contract rule

Persist canonical IDs in new domain records, manifests, audit events, and API/MCP payloads. Preserve the original runtime string in migration provenance.