# MIG-022 Disk Full

A valid Wayfern migration package is staged against a deterministic zero-byte capacity. New staging fails before creation. Interrupted staging resume also fails without deleting its journal or promoting destination data.
