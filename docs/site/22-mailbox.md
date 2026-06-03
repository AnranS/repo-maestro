# Mailbox handoffs

The mailbox is a tiny local queue for cross-role and cross-project handoffs. Use it when one agent needs to leave a durable note for another role or project without turning that note into long-term memory yet.

Messages live under `.maestro/mailbox/messages/*.json`.

## CLI

```bash
maestro mailbox send \
  --from architect \
  --to frontend \
  --project web \
  --task T_web_impl \
  --subject "Contract is ready" \
  --body "Use schemas/openapi.yaml from T_api_schema."

maestro mailbox ls
maestro mailbox show msg-20260522-101530
maestro mailbox resolve msg-20260522 --note "Consumed by T_web_impl"
```

`ls` shows open messages by default. Add `--all`, `--to <name>`, or `--project <name>` to narrow it.

## MCP tools

Agents connected through Maestro's MCP server can use:

- `maestro_mailbox_list`
- `maestro_mailbox_send`
- `maestro_mailbox_resolve`

This gives reviewer, architect, builder, and QA personas a lightweight way to coordinate without inventing hidden state in chat.

## When To Use It

Use mailbox for short-lived handoffs:

- "Backend schema is ready; frontend can start."
- "QA found a failing edge case; builder should patch this next."
- "Architect deferred a decision; ask before implementing."

Use `.maestro/memory/` for stable facts and decisions that future runs should always inherit.
