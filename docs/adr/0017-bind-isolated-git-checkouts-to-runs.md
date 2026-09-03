# Bind isolated Git checkouts to Runs

An isolated Git worktree is execution provenance for one Run, not a property of
a terminal Session or browser-style workspace. superplexr records its exact
repository, base revision, branch, and lifecycle against the Run; Sessions may
come and go while the checkout remains. Retirement is explicit and refuses
dirty or unmerged state, because automatic or forceful cleanup would make a
process-lifecycle convenience capable of destroying the Run's result.
