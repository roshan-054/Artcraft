//! Printing for PhotoCraft beyond the system spooler. Layer L0 (standalone: no workspace crate).
//!
//! With the `network-print` feature, [`network`] sends a document straight to one network printer
//! over WS-Print or IPP (craft-print's `wsd` and `ipp` backends) and follows the job. The spooler
//! and the Windows GDI path stay in `photocraft-engine`.
//!
//! This crate is the part PhotoCraft shares in shape with PdfCraft's `pdfcraft-print::network`
//! (the `Station`, the converter registration, the deny-list checks); the one app-specific piece
//! is [`network::RenderedPages`], the converter that makes PWG raster from the print page the
//! engine laid out.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(feature = "network-print", not(target_arch = "wasm32")))]
pub mod network;
