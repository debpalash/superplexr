# Missions are event-sourced graphs

termi9ne models work as an append-only stream of mission events whose projection
is a graph of parent and child runs. We rejected a persistent pane tree because
layout is a client concern, cannot express causal delegation, and would make the
terminal UI—not agent work—the source of truth. This choice enables audit,
reconstruction, and multiple future projections at the cost of explicit command
validation and event migration.

