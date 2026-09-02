---
status: accepted
---

# Separate Run phase, outcome, and disposition

Run execution phase, terminal Outcome, human Disposition, and attention are
orthogonal projections. We rejected a single status enum because values such as
`AwaitingAttention`, `Succeeded`, and `Rejected` answer different questions and
create impossible transitions when combined.

A Run moves through Pending, Running, Paused, and Finished phases. Finishing
records Succeeded, Failed, or Cancelled Outcome. A successful result separately
becomes AwaitingReview, Accepted, or Rejected. Attention is derived from unresolved
Signals and can coexist with Running or Paused. This costs several fields but
keeps scheduling, process state, human judgment, and interface badges precise.
