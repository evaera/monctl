//! Vendor-specific monitor control, for OSD features DDC/CI cannot reach.
//! Each vendor contributes a module here, a [`crate::cli::VendorCommand`]
//! variant, and (when it has mode actions) an [`crate::config::Action`]
//! variant named after the vendor.

pub mod msi;
