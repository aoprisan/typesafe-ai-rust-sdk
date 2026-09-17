default: check

# fmt + clippy + tests (what CI runs)
check: fmt-check clippy test

test:
    cargo test --all-features
    cargo test --no-default-features

clippy:
    cargo clippy --all-features --all-targets -- -D warnings

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

# live smoke test; needs TYPESAFE_API_KEY
live:
    cargo run --example triage

doc:
    cargo doc --all-features --no-deps --open

publish-dry:
    cargo publish --dry-run
