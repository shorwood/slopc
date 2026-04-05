# Security Policy

## Vulnerability Disclosure

The entire concept is the vulnerability. There is no safe version. This crate sends your function signatures to an LLM and blindly injects whatever comes back into your build. The attack surface is the software itself.

### Known Issues

| Severity | Description |
|----------|-------------|
| Critical | Generated code is written by a hallucinating machine with no accountability |
| Critical | API keys are sent over the network at compile time |
| Critical | Build output is non-deterministic: the same source can compile to different binaries |
| High | The LLM **will** introduce logic bugs that pass type-checking but fail at runtime |
| Medium | Compile times now depend on network latency and API rate limits |
| Low | Your coworkers will lose all respect for you |

### Recommended Mitigations

1. Do not use this crate.
2. If you have already used this crate, stop.
3. If you cannot stop, do not deploy to production.
4. If you have deployed to production, consider a career change.

### Reporting

If you discover a new vulnerability, **congratulations!** you've used the crate as intended. There is nothing to report. The entire thing is a vulnerability.

## Supported Versions

| Version | Supported |
|---------|-----------|
| any     | no        |
