//! Three-way merge model. The model lives in the portable `gall-compare` crate (it also runs in a
//! browser worker); this module keeps the desktop API as it was and converts the errors.
//!
//! Nothing here writes anything: see `gall_compare::merge` for the contract of every item.

use crate::Result;
use std::collections::BTreeMap;

pub use gall_compare::merge::*;

/// Merges the three inputs; `cancel` is asked now and then and ends the work with `CoreError::Cancelled`.
pub fn merge(inputs: &Inputs, cancel: &dyn Fn() -> bool) -> Result<Outcome> {
    Ok(gall_compare::merge::merge(inputs, cancel)?)
}

/// The result of the merge with `choices` (by conflict ID), only when every conflict has a choice.
pub fn resolve(
    inputs: &Inputs,
    choices: &BTreeMap<String, Choice>,
    cancel: &dyn Fn() -> bool,
) -> Result<Resolution> {
    Ok(gall_compare::merge::resolve(inputs, choices, cancel)?)
}

/// An unfinished merge for editing by hand; it is not a result and cannot be saved through [`resolve`].
pub fn preview(
    inputs: &Inputs,
    choices: &BTreeMap<String, Choice>,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<Preview>> {
    Ok(gall_compare::merge::preview(inputs, choices, cancel)?)
}
