# Keep verified delivery in the Mission graph

Change admission, Harness Snapshots, Escalations, Handoffs, Evaluation Receipts,
and owner Settlement are authoritative facts about why a Run could execute and
whether its exact Candidate was accepted. ultraplexr records them as immutable
Mission events and projections rather than creating a second workflow store or
encoding them in terminal text. Frequently replaced provider and CI observations
remain outside Mission history as Run evidence; they may support a receipt but
cannot settle work. This keeps one auditable governance graph while allowing
external adapters to refresh without growing or rewriting domain history.
