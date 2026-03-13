# Artifact Pipeline Plan

## Summary

This repository now grows a static `artifacts` subsystem. `taskd` stays the scheduler, `taskctl` exposes taskd-native reports, and `artifactctl` orchestrates generic `collect -> render -> sink` workflows from a separate YAML config.

## Interfaces

- `artifactctl validate|list|collect|render|sink|run`
- `/etc/taskd/artifacts.yaml` with artifact definitions, collector commands, one renderer command, and sink commands
- `taskctl report daily --date <YYYY-MM-DD> --timezone <TZ> [--output <PATH>]`

## Runtime Model

- Stable per-date run directory under each artifact workdir
- Fixed files:
  - `records.jsonl`
  - `render-input.json`
  - `rendered.json`
  - `run.json`
- Collectors emit one JSON record file each
- `artifactctl collect` validates and aggregates them into `records.jsonl`
- `artifactctl render` reads `records.jsonl`, writes `render-input.json`, runs the renderer, and validates `rendered.json`
- `artifactctl sink` reads `rendered.json` and dispatches it to all enabled sinks

## Tests

- Config validation for unknown template variables
- End-to-end `artifactctl run` integration with mixed collector success/failure
- `taskctl report daily` output shape and daily aggregation semantics

## Defaults

- Command contracts are the primary extension mechanism
- Partial collector failure produces synthetic error records instead of aborting the artifact
- File-based handoff is the v1 transport between collect, render, and sink
