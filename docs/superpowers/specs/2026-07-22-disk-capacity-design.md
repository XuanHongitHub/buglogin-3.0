# Migration Disk Capacity

## Contract

- Production staging reads available bytes from the destination filesystem before payload writes.
- Required bytes include the staging journal, missing package files, checksum manifest, and a fixed free-space reserve.
- Insufficient capacity fails with HTTP-equivalent status `507` before creating a new staging root.
- An interrupted owned staging root remains intact when resume capacity is insufficient.
- No capacity failure may create or promote live destination data; source remains unchanged.
