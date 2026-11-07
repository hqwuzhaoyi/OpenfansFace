# Issue tracker

Use GitHub Issues in `hqwuzhaoyi/OpenfansFace`. Implementation-ready issues use `ready-for-agent`; readiness does not override native blocking dependencies.

For the Rust file backend migration, the accepted spec is `docs/specs/rust-file-backend.md`, with tickets #1–#14. Follow the native blocked-by graph. Keep issues open until their PR merges; use closing references in the PR.

Fetch complete issue bodies and comments through the GitHub connector, or `gh issue view NUMBER --repo hqwuzhaoyi/OpenfansFace --comments` when the connector lacks a required capability. Preserve existing parent issues and unrelated work.
