//! UQL serving: runs a parsed statement on the planner (the parser lives in `medulla-uql`).

#[cfg(feature = "query")]
pub mod serve;
#[cfg(test)]
mod tests;
