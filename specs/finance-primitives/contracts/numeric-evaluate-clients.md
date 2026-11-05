# Seam: numeric kernel evaluation for connector analyses

Provider: medulla MEDULLA-FINANCE-PRIMITIVES-R002.2.5
Consumer: scion-sdk SCION-FINANCE-SOURCES-R902
Consumer: emerald-exchange EMERALD-OPS-R007

## Wire shape

A client sends `Method::NumericEvaluate { op }`, where `op` is a tagged enum whose variants carry the field names of the retired `Finance<Kernel>` methods. Medulla returns `NumericKernelOutput`. The SDK adapter maps each connector analysis to one `op`.

## Refusal codes

`KernelRefusal` carries the stable codes named in the `Refusal:` of `MEDULLA-FINANCE-PRIMITIVES-R002.2.5`. A retired method name fails to decode; there is no alias or redirect.

## Authoritative types

`NumericEvaluateOp`, `NumericKernelOutput` and `KernelRefusal` in `crates/medulla-types/src/numeric_kernel.rs`.

## Test vectors

The independent-oracle vectors in `tests/finance_numeric_evaluate_served.rs` are the shared vectors; the SDK and emerald prove their mapping against the same op and output types.
