---
status: accepted
---

# Separate Run lineage from execution dependencies

A Run's optional parent records which attempt created or delegated it. Separate
dependency edges record which successful attempts are prerequisites for starting
another Run. We rejected overloading the parent edge because causality does not
imply scheduling: a parent may delegate parallel children, a child may start
immediately, and a fan-in Run may depend on work from unrelated branches.

The resulting Mission projection contains a lineage forest and a dependency DAG.
This costs an additional edge type and cycle validation, but makes delegation,
fan-out, fan-in, retries, readiness, and failure propagation unambiguous.
