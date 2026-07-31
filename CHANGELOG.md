# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0 releases may introduce breaking changes freely as the storage layout and hook contract converge. After 1.0, changes will follow semver strictly.

## [Unreleased]

## [0.1.0] - 2026-07-31

> Replacement memory across a context boundary.

### Added — replacement memory across a context boundary

`amtr synthesize` runs from a `PreCompact` hook: it records that a snapshot is
owed, detaches a worker by double fork, and returns, so extraction runs
alongside compaction instead of delaying it. The worker reads the journal since
the previous compaction, runs the extraction agent over that window plus the
prior handoff, and replaces the session's single stored row.

`amtr recall` reads that row back. Delivery happens at the next turn start,
because neither host can inject context from a compaction hook.

Cross-session handoff is `amtr recall --amtr-key <key>`, which moves the row to
the calling session so the giving session forgets, or copies it with `--clone`.
A clone carries no key, since a key names one snapshot rather than a lineage.

Both Claude Code transcripts and Codex rollouts are read, and the extraction
agent launched is whichever host produced the journal.

### Added — plugin for Claude Code and Codex

Installable on both hosts from one marketplace catalog at the repo root. Each
host has its own manifest and its own hook declaration, and both run the same
`tools/amtr-hook.sh`. The `/amtr` skill wraps the cross-session handoff.

The Claude Code declaration sets explicit hook timeouts; the Codex one does not,
because the unit of that field is unverified there and a wrong guess would kill
the hook outright.

### Added — extraction prompt as the customization surface

A default prompt is materialized at `~/.local/share/amtr/prompt.md` on first
run and is never overwritten afterward, including by upgrades. There is no
config file, no environment variable, and no flag for it: the caller is a hook,
so editing that file in place is the whole interface.

### Notes

The executable is `amtr` rather than `amt` because macOS ships an unrelated
root-owned `/usr/sbin/amt` that wins on a default `PATH`.

Stored memory is deliberately ephemeral. Every failure path writes nothing and
exits zero, on the principle that the next compaction redoes the work.

[Unreleased]: https://github.com/naoto256/amnestic-trace/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/naoto256/amnestic-trace/releases/tag/v0.1.0
