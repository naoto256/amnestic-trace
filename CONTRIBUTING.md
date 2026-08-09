# Contributing

This is a personal project. Bug reports and feature requests via Issues are welcome.

Pull requests are not accepted at this time.

## Working on it locally

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
tests/hook-regressions.sh
```

All four must pass. CI runs the same checks, including the hook regressions
under sh, dash, bash, and ksh, plus consistency checks over the plugin
manifests. The Rust tests cover window slicing, storage transitions, concurrent
claims, shared deadlines, cleanup races, and output validation. The shell
regressions exercise only the thin adapter against a stub binary: canonical
arguments, stdin/stdout forwarding, PATH repair, and fail-open behavior under
all four shells. Host invocation, hook wiring, and acceptance of injected
context still cannot be represented fully by that fixture;
`README.md` carries the manual procedure for checking those in a real session.

Commits go through the repository's audit gate, so a commit needs a
report-bound receipt before it will land.
