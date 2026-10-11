# MIG-023 Case and Unicode

A Wayfern profile uses a Vietnamese display name. Collision probes cover ASCII case variants and NFC/NFD-equivalent filenames. Each pair must either alias at the filesystem boundary or be rejected by migration before packaging.
