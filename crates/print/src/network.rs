//! Network printing on craft-print's `wsd` and `ipp` backends. Cargo feature `network-print`; not
//! built for wasm32.
//!
//! The protocols, the print ticket and the job classification are craft-print's; this module is
//! what the app adds on top, shared by the Print dialog and tests:
//!
//! * [`station::Station`]: one printer the person chose, behind a private [`Registry`] that holds
//!   both network backends; the printer is added to the one its address names. It asks what the
//!   printer is (read-only) and makes the tracked print (`craft_print::PrintJob`).
//! * **Formats and conversion are the registry's.** A document in a format the printer lists is
//!   sent as it is. One it does not list goes through the app's [`DocumentConverter`]; with no way
//!   there the print is [`ErrorKind::UnsupportedFormat`] before a job exists.
//! * **No retry of an unknown outcome.** `PrintJob` refuses to resend what may have printed.
//! * **The deny list** (`CRAFT_PRINT_DENY`, legacy `PDFCRAFT_PRINT_DENY`) is checked by address
//!   when a printer is opened and by the name it reports after a query, and enforced by the
//!   registry for every request a job makes.

use std::sync::{Arc, PoisonError, RwLock};

pub use craft_print::ipp;
pub use craft_print::wsd;
pub use craft_print::{
    AttemptError, CancelToken, ColourMode, Conversion, ConversionPolicy, ConvertContext, Document, DocumentConverter, DocumentFormat, Duplex, ErrorKind, Job,
    JobHint, JobId, JobKind, JobOptions, JobState, JobView, LifecycleState, Orientation, Outcome as PrintOutcome, OwnedDocument, Phase, PrintError, PrintJob,
    PrintJobConfig, PrinterDetails, Progress, Quality, Reconciled as TrackedReconciled, Registry, Resolution, RetryAdvice, RetrySafety, SimpleState, Snapshot,
    StaleReason, UserConfirmed, pwg,
};

mod converter;
pub mod station;

pub use converter::{RenderFn, RenderSpec, RenderedPages};

/// A transport shared with worker threads: the plain-HTTP one of craft-wsd, or an in-memory fake.
pub type Transport = Arc<dyn wsd::adapter::Transport + Send + Sync>;

/// The converter every [`station::Station`] opened afterwards registers on its registry.
static CONVERTER: RwLock<Option<Arc<dyn DocumentConverter>>> = RwLock::new(None);

/// Register the converter the app brings: stations opened from now on use it for a document the
/// printer does not list. Replaces an earlier one.
pub fn register_converter(converter: Arc<dyn DocumentConverter>) {
    *CONVERTER.write().unwrap_or_else(PoisonError::into_inner) = Some(converter);
}

pub(crate) fn registered_converter() -> Option<Arc<dyn DocumentConverter>> {
    CONVERTER.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The refusal for a printer on the deny list: [`ErrorKind::Denied`], naming the variable and never
/// the printer.
pub(crate) fn denied_error() -> PrintError {
    PrintError::new(ErrorKind::Denied, format!("the printer is on the deny list ({}); it was not opened and nothing was sent to it", craft_print::DENY_ENV))
}

/// Refuse when any of `texts` (an address, a name the printer reported) names a printer on `denied`.
pub(crate) fn check_denied(denied: &[String], texts: &[&str]) -> Result<(), PrintError> {
    if craft_print::any_denied(texts, denied) { Err(denied_error()) } else { Ok(()) }
}

/// What the person is told when a send failed and the printer may nevertheless have the document.
pub const CHECK_THE_PRINTER: &str = "the request may have reached the printer: check the printer before sending again";
