# adapter-slack

Class C Slack messaging adapter for the reel protocol.

This crate provides a [`SlackAdapter`] that buffers Slack Web API
operations (`post_message`, `edit_message`, `delete_message`) as
[`EffectDescriptor::C`](reel_spec::EffectDescriptor) entries and
fires them at commit time. Class C means: irreversible at the
external system, so the adapter is the canonical demonstration of
I-002 (abort silences Class C).

## Status

- v0.1 — MVP adapter; transport is trait-abstracted; the in-tree
  implementation is a mock-only transport used by tests and the
  forthcoming `examples/dryrun/send_slack.py` demo.
- A real HTTP transport will land behind a `transport-http` cargo
  feature once an HTTP client is added to the workspace lockfile.

See
[`spec/adapter-interface.md`](../../../spec/adapter-interface.md) and
the design notes.
