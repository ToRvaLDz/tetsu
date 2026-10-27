//! Tetsu agent core: discovers the bus calls, maps each to one tool for a
//! model, runs them through the capability gate and orchestrates a turn.
//!
//! The core holds no credential and calls no model provider: the model sits
//! behind the `Backend` boundary, implemented elsewhere.

pub mod contract;
pub mod discovery;
pub mod runner;
pub mod tools;
