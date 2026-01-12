//! AWS module for credentials and Cost Explorer API

pub mod credentials;
pub mod cost_explorer;

pub use credentials::Credentials;
pub use cost_explorer::{CostExplorerClient, CostData};
