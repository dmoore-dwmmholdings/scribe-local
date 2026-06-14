//! HTTP request handlers, one module per resource group. The router in `lib.rs`
//! wires these to their paths.

pub mod audio;
pub mod health;
pub mod recordings;
pub mod search;
pub mod segments;
pub mod speakers;
