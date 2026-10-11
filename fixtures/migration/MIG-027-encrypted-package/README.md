# MIG-027 Encrypted Package

A supported Wayfern profile contains a plaintext canary in its legacy profile payload. Export must produce an authenticated encrypted package. A wrong OS-vault key must fail before destination mutation; the correct key must restore exact bytes without leaving plaintext in package evidence.
