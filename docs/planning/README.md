# Planning notes

Working notes that back the [roadmap](../../ROADMAP.md). The roadmap says
*what* and *why*; these notes hold the code-level reasoning behind cost
estimates and sequencing, so the roadmap can stay short and the reasoning can
be checked against the source. When a note turns into a decision, write a
record under [`../decisions/`](../decisions/README.md) instead.

| Note | Backs |
| --- | --- |
| [`grid-render-cost-model.md`](grid-render-cost-model.md) | Why some grid templates are cheap and others need a spatial index; what the napi layer adds to every new grid. |
| [`byte-access-and-remote.md`](byte-access-and-remote.md) | Why remote access is deferred, the prefetch-then-decode-sync answer, and the constraint to protect now. Input to ADR-0005. |
| [`hdf5-filters.md`](hdf5-filters.md) | Filter coverage and per-filter cost, including why szip is a project rather than a quick win. |
| [`parameter-table-sources.md`](parameter-table-sources.md) | Upstream sources, sizes, and licenses for the parameter and code tables, plus the resolution policy. |
| [`standards-watch-list.md`](standards-watch-list.md) | Where new templates, filters, and conventions are announced, and the checkpoint rhythm. |
