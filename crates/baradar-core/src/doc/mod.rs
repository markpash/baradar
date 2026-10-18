//! Label document model (port of `web/src/doc`).

pub mod files;
pub mod history;
pub mod migrate;
pub mod ops;
pub mod schema;
pub mod secrets;
pub mod share;
pub mod validate;

pub use schema::*;
