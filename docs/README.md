# Documentation Map

This directory separates durable technical documentation by purpose:

- **[`invariants.md`](invariants.md)** — High-entropy system invariants (`@MODULE -> !INVARIANT`) and load-bearing rationale (rejected simplifications).
- **[`architecture.md`](architecture.md)** — The system as currently implemented: module map, seams, data flow, and authority boundaries.
- **[`contracts/`](contracts/README.md)** — Behavioral and boundary contracts across subsystem seams.
- **[`deferred.md`](deferred.md)** — Capabilities explicitly outside the current checkpoint.
- **[`plan/`](plan/README.md)** — Internal implementation plans and the living [`deviations.md`](plan/deviations.md) log.
- **Optional audience folders** — `guide/` (human onboarding, FAQ, testing walkthroughs), `operations/` (executable runbooks and drills), `security/` (threat model and evidence). Create a folder when its contents justify the maintenance. [`architecture.md`](architecture.md) stays the agent-facing map; human-facing exposition belongs in `guide/`, never in a parallel wiki that duplicates it.

## Fact Ownership

Each fact has one maintained definition. Navigation maps, indexes, and decision records link to that definition when they need to refer to it.

| Fact | Owner | When to update |
| :--- | :--- | :--- |
| Implementation and enforcement | Code, configuration, and tests | The implementation changes. Documentation links to evidence rather than cataloging implementation details. |
| Implemented modules, entrypoints, flows, and state ownership | [`architecture.md`](architecture.md) | A recorded responsibility, seam, flow, owner, or verification command changes. Link to contracts and invariants for their guarantees. |
| Guarantees and failure/recovery behavior at a subsystem seam | [`contracts/`](contracts/README.md) | A recorded guarantee or its enforcement changes. |
| Cross-cutting constraints and load-bearing rationale | [`invariants.md`](invariants.md) | A constraint or rationale changes. If a contract already owns the guarantee, index its scope and enforcement with a link to the owning section. |
| Decision context, alternatives, and trade-offs | [`adr/`](adr/README.md) | A decision meets the ADR creation criteria. Preserve past rationale and mark superseded decisions. Link to current guarantees in their owners. |
| Domain terms whose ambiguity changes behavior | Root `GLOSSARY.md`, when needed | A consequential ambiguity is resolved or an existing definition changes. |
| Proposed work and intentionally deferred scope | [`plan/`](plan/README.md), [`deferred.md`](deferred.md) | A plan or recorded scope boundary changes. These documents do not define implemented behavior. |

Verify affected claims against code and tests. Surface disagreements between implementation and recorded guarantees. A documentation edit alone does not establish that a guarantee holds.

The scaffold shows where information belongs. A document's presence does not require filling it out. Create additional documents only when their contents justify the maintenance. Keep placeholders while this repository is a template. During adoption, replace or omit them according to the implemented system.

Record observed failures and adapt the smallest relevant instruction or adapter. Add routing, hooks, or shared runtime memory when a demonstrated need justifies them.
