# MIG-024 Long Paths

The committed source describes deterministic deep-path probes. Tests create a path beyond the historical Windows 260-character limit and require successful snapshot/package staging. Paths beyond the 1024-byte portable ceiling or 255-byte component ceiling fail explicitly.
