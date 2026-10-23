# MIG-019 ACL Unknown Role

The canonical control-plane export contains an unsupported membership role. A sibling variant contains an unsupported `resourceAccessMode`. Both must fail closed without role or access downgrading, package creation, or destination writes.
