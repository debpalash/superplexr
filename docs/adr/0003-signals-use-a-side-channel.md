# Agent signals use a local side-channel

Agents report questions, approvals, blockers, progress, and artifacts through the
versioned local control protocol rather than by decorating or scraping terminal
output. This preserves unmodified byte streams, makes attention deterministic,
and allows non-terminal agents to participate. Shell integration may infer weak
fallback signals for unaware programs, but inferred data never replaces explicit
agent signals.

