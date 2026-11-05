# Seam: banking risk calculations

Provider: medulla MEDULLA-FINANCE-PRIMITIVES-R901
Consumer: arbora ARBORA-DATA-MARKET-R003.14
Consumer: scion-sdk SCION-FINANCE-SOURCES-R904
Consumer: emerald-exchange EMERALD-OPS-R013

## Wire shape

Callers send `Method::FinanceAmlScreen`, `FinanceCapitalRatios`, `FinanceExpectedLoss` or `FinanceSettlementChain` with typed banking inputs. Banking records arrive from the connector; no connector computes the figures. Medulla returns typed results.

## Refusal codes

Each method returns the stable refusal codes of its `Refusal` row in `requirements.md`; an input outside the declared ranges returns a typed refusal and no figure.

## Authoritative types

The request and result types of `crates/medulla-result-types/src/compute_result/finance.rs` and the kernels of `crates/medulla-finance-quant`.

## Test vectors

The reference vectors of the banking rows in `test-spec.md` are the shared vectors; each consumer proves its mapping against the same request and result types.
