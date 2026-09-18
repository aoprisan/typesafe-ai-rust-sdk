default: check

# fmt + clippy + tests (what CI runs)
check: fmt-check clippy test repl-check

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

# the learning REPL; TYPESAFE_API_KEY optional (without it, answers are simulated)
repl:
    cargo run -p jev-repl

# fmt + clippy + tests for the REPL
repl-check:
    cargo fmt -p jev-repl --check
    cargo clippy -p jev-repl --all-targets -- -D warnings
    cargo test -p jev-repl

doc:
    cargo doc --all-features --no-deps --open

publish-dry:
    cargo publish -p typesafe-ai-sdk --dry-run --locked

publish:
    cargo publish -p typesafe-ai-sdk --locked

# the REPL is its own crate (`cargo install jev-repl`); publish the library version it needs first
publish-repl-dry:
    cargo publish -p jev-repl --dry-run --locked

publish-repl:
    cargo publish -p jev-repl --locked
