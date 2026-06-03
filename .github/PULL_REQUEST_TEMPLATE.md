<!--
Thanks for opening a PR! A few notes:
- Small focused PRs are easier to review. < ~500 lines is a soft target.
- Tests required for scheduler / channel adapter / plan synth changes.
- Squash-merge is the default; the squash subject should match Conventional
  Commits (e.g. `feat(channel): drain outbound replies (#42)`).
-->

## Goal

<!-- One sentence: what change does this PR introduce and why? -->

## Approach

<!-- Bullet list of what changed, ordered by impact. Link to design docs
     under docs/design/ if any. -->

-
-
-

## Verification

<!-- Paste the raw output blocks for each. Replace failing or skipped
     lines with reasons. -->

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

<!-- Add boundary greps you ran to confirm scope (e.g. "no edits outside
     src/channel/"), and any manual smoke tests. -->

## Risks and follow-ups

<!-- Anything you punted to a later PR? Anything risky reviewers should
     stress-test? -->

-
-

## Linked issues

<!-- Closes #123, Refs #456 -->

Closes #
