## Summary of Changes
<!-- Provide a concise description of the changes introduced by this pull request. -->

## Motivation & Context
<!-- Why is this change required? What problem does it solve or feature does it add? -->

## Quality & Compliance Checklist
- [ ] Conforms to FrameMC architectural directives (R-01 through R-12 in `docs/ARCHITECTURE.md`)
- [ ] Zero-copy Play forwarding invariant maintained (`tokio::io::copy_bidirectional`)
- [ ] No `std::sync::Mutex` held across `.await` points
- [ ] Cryptographic keys implement `Zeroize` on drop
- [ ] All unit and integration tests pass: `cargo test --all-targets -- --nocapture`
- [ ] Clippy passes with zero warnings: `cargo clippy --all-targets -- -D warnings`
- [ ] Formatted with rustfmt: `cargo fmt --check`
- [ ] Relevant test vectors added for any new packet or protocol changes
