# MIG-018 ACL Equivalence

1. [x] Audit legacy mapper and hosted import authority
2. [x] Fix profile binding key to canonical profile ID
3. [x] Authorize profile routes with workspace/resource scope
4. [x] Honor resource-specific share grants by action
5. [x] Add workspace/member/share fixture goldens
6. [x] Prove owner/shared/unshared effective matrix
7. [x] Prove destination lease reissue; never copy source fence
8. [x] Run migration, API, role, and CI gates

Lease policy: ACL, resource, and profile binding migrate. Source lease tokens and fencing counters never migrate. A destination lease is acquired from destination coordination authority after imported ACL and binding validation.
