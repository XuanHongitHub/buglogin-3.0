# MCP Profile Authorization Scope Design

Status: implemented and locally verified.

## Problem

REST and MCP had separate profile authorization paths. In hosted mode, REST supported resource-level profile share grants while MCP did not, so a recipient could see/use a shared profile through one adapter but be denied through another. MCP profile listing also filtered only by capability grants, omitting profiles visible through resource shares.

The REST fallback also needed a stricter boundary: a resource share must not override a token scope restriction or an explicit capability deny, and it must not grant destructive or management actions.

## Contract

- REST and hosted MCP profile authorization share one backend decision function and canonical binding lookup.
- Workspace/resource/profile capability grants remain first-class. An active workspace member's resource share is an additional grant path for read and execute only.
- Profile runtime/manifest/blob read and execute lease gates resolve the resource back to its canonical profile binding and use the same decision function; upload/update authority remains governed by update capability grants.
- Token scope restrictions and matching explicit capability denies always win over resource shares.
- Resource-share evaluation verifies the resource's workspace and active membership; deny resource grants override allows.
- Profile list uses the same decision function for each record, so direct requests and list visibility agree.
- Local MCP behavior and create scope resolution keep their existing contracts.
- Authorization remains server-side and deny-by-default.

## Verification

- Regression tests cover workspace-scoped capability access, resource-share access, token-scope restrictions, explicit capability denies, and deny precedence.
- Run targeted hosted profile authorization tests, MCP execution/vertical tests, REST profile tests, and formatting.
