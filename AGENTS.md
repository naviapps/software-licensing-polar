# Polar licensing SDK

Rust owns fixed HTTPS transport, strict Polar codecs, API version, deadlines, cancellation and mutation evidence. Swift and .NET are thin SoftwareLicensing provider bindings; the generic core owns storage, lifecycle and offline policy. Scope every accepted key to the configured organization and nonempty benefit allowlist. Environment is explicitly sandbox or production. Never put merchant tokens in this SDK.

Use main; parent owns commits, publication and VM lifecycle. Never operate real licenses or customers. Tests use owned loopback fixtures and invalid/pre-cancelled inputs. Keep the `polar` source distinct from other issuers; never probe an existing license at a different provider. Preserve uncertain mutation evidence; never interpret 404 as a successful mutation. All shell commands use login:false.

Use `make check` for contributor changes. The public SDK Tooling revision in
`mise.toml` supplies common lint and consumer checks. Use that same revision for
distribution checks and changes to source-bound native artifacts. Preserve original third-party notices, and rebuild native artifacts
after Rust changes. Report Rust, C, Swift, .NET, remote CI and provider evidence
separately. Use GitHub-hosted CI. Before publication, run the full gate locally on
the exact pinned sources.
