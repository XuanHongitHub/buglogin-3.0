# MIG-015 Path Traversal

An otherwise valid empty source is paired with adversarial package paths plus a dynamically created source directory link. Package traversal must fail before staging exists. Source junctions/symlinks must fail before target bytes are inventoried or copied.
