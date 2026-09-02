# Stamp scheduler time in the event store

Planning and completion events receive Unix-microsecond lifecycle timestamps at
the durable Mission store, before decision replay and journal append. Control
commands contain no timestamp field, so clients cannot forge scheduling age.
The timestamp fields are optional for backward replay compatibility; legacy Runs
without trusted time remain deterministic but receive no age promotion.

A dependency-free pending Run is ready from its planning timestamp. A dependent
Run is ready from the maximum of its own planning time and every successful
prerequisite's completion time. Effective priority promotes one level for each
complete five-minute ready interval, capped at urgent. Ordering is effective
priority, ready-since time, then Run ID. The same pure projection feeds preview,
explicit batch launch, and continuous policy reconciliation.
