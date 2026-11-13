//! Acceptance tests of the assembly decision function (DECIDE-LAYER-DESIGN
//! §11.1, WP A6): the routing fixture, planted bad inputs, the brute-force
//! oracle, golden vectors and the latency report.

mod evidence;
mod fixture;
mod golden;
mod oracle;
mod planted;
mod primitive;
mod prompt;
mod refusal;
mod routing;
mod template;
mod topology;
mod uniqueness;
