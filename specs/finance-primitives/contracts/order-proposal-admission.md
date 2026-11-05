# Seam: order proposal staging and promotion

Provider: medulla MEDULLA-FINANCE-PRIMITIVES-R015.1
Consumer: arbora ARBORA-DATA-MARKET-R007
Consumer: scion-sdk SCION-GOVERNED-WRITEBACK-R001

## Wire shape

`Method::FinanceOrderStage` creates a sealed, content-addressed `OrderProposalSnapshot` revision bound to a decision identity and digest. `Method::FinanceOrderPromote` rechecks the staged digest, the decision verification and approval, and the risk policy, then commits one promotion with a receipt. Output means host-review admission only; it is not an order or a provider effect.

## Refusal codes

`ORDER_PROMOTION_VETOED` and the stage and promote refusals named in the rows of `MEDULLA-FINANCE-PRIMITIVES-R015.1.1` to `MEDULLA-FINANCE-PRIMITIVES-R015.1.3`. A refusal writes no pointer, history or receipt.

## Authoritative types

`SEALED_RECORD_CLASSES` in `crates/medulla-mutation-types/src/sealed_record.rs` and the types named in the `Shape:` of `MEDULLA-FINANCE-PRIMITIVES-R015.1.1`.

## Test vectors

`tests/finance_order_proposal_served.rs` holds the stage, revision-chain and promote vectors. Arbora proves confirmation and the SDK proves the writeback schema against the same receipt fields.
