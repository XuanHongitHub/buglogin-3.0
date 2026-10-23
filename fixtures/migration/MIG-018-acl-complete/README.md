# MIG-018 ACL Equivalence

One managed profile bound to one workspace resource. The control export contains an owner, a shared restricted member, an unshared restricted member, workspace membership grants, and a run-sync-limited profile share. Source leases are intentionally absent: destination coordination must issue a new lease and fencing token after import.
