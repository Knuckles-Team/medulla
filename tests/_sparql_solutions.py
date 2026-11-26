"""Read a typed ``SparqlResult`` as one dict per solution.

``RdfClient.sparql`` and ``RdfClient.sparql_virtual`` return the engine's typed
``SparqlResult``: every cell is a ``SparqlTerm`` (``kind``, ``value``, ``datatype``,
``language``) or ``None`` for an unbound variable. A test that compares lexical forms
or IRIs reads ``term.value`` through this one function.
"""

from __future__ import annotations

from medulla.generated.models import SparqlResult

__all__ = ["solution_values"]


def solution_values(result: SparqlResult) -> list[dict[str, str | None]]:
    """One ``{var: term.value}`` dict per row (``None`` for an unbound variable)."""
    return [
        {
            name: None if cell is None else cell.value
            for name, cell in zip(result.vars, row, strict=True)
        }
        for row in result.rows
    ]
