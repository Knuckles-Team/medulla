//! medulla-finance-types — the finance slice of the contract layer
//! (MEDULLA-SDLC-R021.6): the finance wire DTOs the protocol enum embeds and
//! the finance records around them. It depends on `medulla-types` only; the
//! finance algorithms live upstream and name these types directly.

// MEDULLA-FINANCE-PRIMITIVES-R011.1 — the typed finance alert envelope staged for
// the finance outbox topic family.
#[cfg(feature = "finance")]
pub mod finance_event_envelope;
// MEDULLA-FINANCE-PRIMITIVES-R751.750 — the strategy research record types.
#[cfg(feature = "finance")]
pub mod finance_order;
#[cfg(feature = "finance")]
pub mod finance_research;
// MEDULLA-FINANCE-PRIMITIVES-R904 — the wallet key trust record read by
// `Method::WalletKeyTrustGet`.
pub mod finance_wallet_trust;
// Finance wire DTOs; the query and data-science wire family lives in `medulla-query-types`.
#[cfg(feature = "finance")]
pub mod wire_finance;
