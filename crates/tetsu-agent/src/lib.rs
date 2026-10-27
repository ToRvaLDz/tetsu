//! Tetsu agent core: discovers the bus calls, maps each to one tool for a
//! model, runs them through the capability gate and orchestrates a turn.
//!
//! Four layers, each a module:
//!
//! - [`contract`] and [`tools`]: the shipped IDL becomes one tool per
//!   method (id, wire name, schema, description);
//! - [`discovery`]: which interfaces are served, and where;
//! - [`runner`]: one tool call through the facade, a refusal as an outcome;
//! - [`turn`] and [`agent`]: the loop over a [`backend`] and the runner.
//!
//! The core holds no credential and calls no model provider: the model sits
//! behind the [`backend::Backend`] boundary, implemented elsewhere. [`cli`]
//! is the pure part of the binary.

pub mod agent;
pub mod backend;
pub mod cli;
pub mod contract;
pub mod discovery;
pub mod runner;
pub mod tools;
pub mod turn;
