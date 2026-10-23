# Long Path Policy

## Contract

- Relative migration paths up to 1024 UTF-8 bytes are portable candidates.
- Each path component is limited to 255 UTF-8 bytes.
- Deep paths above the historical Windows 260-character boundary remain supported when inside the portable ceiling.
- Paths over either ceiling fail with explicit source/package error codes before destination writes.
- Path spelling, checksums, and source bytes remain unchanged.
