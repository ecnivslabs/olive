# Security

## Reporting a vulnerability

Report privately through GitHub Security Advisories: go to the
[Security tab](https://github.com/ecnivslabs/olive/security) and click
"Report a vulnerability". Do not open a public issue for suspected
vulnerabilities.

You should get an initial response within 7 days. If the issue is
confirmed, we'll work with you on a fix and coordinate a disclosure
timeline before any public advisory is published.

## Scope

In scope:
- The `pit` compiler and its MIR passes (borrow checker, ownership
  inference, gencheck solver)
- The Olive runtime and standard library (`std_lib/`), including the
  slab allocator and generation-check machinery
- The Python and C FFI boundary
- `install.sh` and the release/build pipeline (`.github/workflows/`)

Out of scope (see [SAFETY.md](SAFETY.md) for what's guaranteed vs. not):
- Bugs in `unsafe` blocks that don't lead to exploitable memory
  corruption or an unsound safety-check bypass are tracked as regular
  bugs, not security issues
- Vulnerabilities in Python packages imported via Olive's Python
  interop — report those upstream
- Vulnerabilities in third-party Rust crates — report those upstream
  or via a `cargo audit` / RUSTSEC advisory; open an issue here only
  if Olive needs to bump a pinned version in response

## Supported versions

Only the latest released version of `pit` receives security fixes.
There is no LTS branch.
