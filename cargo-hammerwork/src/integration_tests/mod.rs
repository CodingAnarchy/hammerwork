//! Tests that span several command and utility modules. `cargo-hammerwork` is a binary
//! crate, so these live here instead of in `tests/`, where its internals are not visible.

mod graph_output_format;
mod job_state;
mod sql_query;
mod workflow_visualization;
