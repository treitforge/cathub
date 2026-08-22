//! Private transport types for CatHub-managed virtual serial endpoints.
//!
//! This crate contains the shared wire contract for the UMDF driver and the CatHub daemon.
//! It also contains the data model for serial conformance reports.

#![allow(clippy::doc_markdown)]

#[cfg(feature = "conformance")]
pub mod conformance;
pub mod protocol;
