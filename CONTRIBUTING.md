# Contributing

This is a personal project. Bug reports and feature requests via Issues are welcome.

Pull requests are not accepted at this time.

## Working on it locally

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Both must pass. Tests cover the pure logic — window slicing, the storage
transitions, output validation. The process-level behavior (double fork, hook
wiring, injection) is not unit-testable in any meaningful way; `README.md`
carries the manual procedure for it, and changes to that layer should be
checked against a real session rather than a fixture.

Commits go through the repository's audit gate, so a commit needs a
report-bound receipt before it will land.
