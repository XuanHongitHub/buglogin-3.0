# Interrupted Migration Copy

## Contract

- Staging writes a package-bound journal before payload files.
- A rerun with the same package and run ID resumes missing files only.
- Existing files must match size and SHA-256; partial or tampered files fail closed.
- Discard requires the matching journal/package and removes only the owned staging root.
- Unexpected files, symlinks, journal mismatch, and source mutation block the operation.
