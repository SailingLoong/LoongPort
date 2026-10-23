//! TRAE's isolated account owner; no native credential scanning or extra pool.
pub(crate) mod auth;
pub(crate) mod checkin;
pub(crate) mod discovery;
pub(crate) mod entry;
pub(crate) mod import;
pub(crate) mod inference;
pub(crate) mod models;
pub(crate) mod protocol;
pub(crate) mod store;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod lifecycle_tests;
