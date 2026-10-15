//! One network printer reached over WS-Print or IPP, and the tracked print job on it.
//!
//! A [`Station`] is a private [`craft_print::Registry`] with BOTH network backends registered
//! (`wsd` and `ipp`); the printer the person typed is added to the one its address names
//! ([`protocol_of`]). A print is a [`PrintJob`] made by [`Station::job`] and advanced by the
//! caller (`advance(station.registry())`), so one button press is one tracked print however the
//! sending, following, retrying and reconciling go. The converter (PDF to PWG raster) and the
//! per-job options go through the registry.
//!
//! WS-Eventing push for WS-Print is opt-in: [`events_from_env`] reads `PDFCRAFT_TEST_EVENTS_BIND`.
//!
//! The deny list (`CRAFT_PRINT_DENY`) is applied here too, because the two `*_info` reads go to
//! the printer with a client of their own, outside the registry: the address is checked when the
//! station is opened and the name the printer reports before an answer is kept.

use std::net::SocketAddr;
use std::sync::Arc;

use craft_print::ipp::{self, IppBackend, PrinterAttrs, PrinterUri};
use craft_print::wsd::error::Op;
use craft_print::wsd::events::NotifyConfig;
use craft_print::wsd::{Client, PrinterElements, PrinterUrl, WsdBackend};
use craft_print::{
    BytesSource, DocumentConverter, DocumentFormat, ErrorKind, Job, JobOptions, PrintError, PrintJob, PrintJobConfig, Registry, UserConfirmed, namespaced_id,
};

use super::{Transport, check_denied, registered_converter, wsd};

/// Which protocol a typed address names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// WS-Print (the default).
    Wsd,
    /// IPP (`ipp://`, `ipps://`, or an `http://` address whose path has an `/ipp/` segment).
    Ipp,
}

impl Protocol {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Wsd => "WS-Print",
            Self::Ipp => "IPP",
        }
    }
}

/// Choose the protocol by scheme and path: `ipp:`/`ipps:` is IPP, an `http:` path containing an
/// `/ipp/` segment (or ending in `/ipp`) is IPP, everything else is WS-Print.
pub fn protocol_of(address: &str) -> Protocol {
    let text = address.trim().to_ascii_lowercase();
    if text.starts_with("ipp://") || text.starts_with("ipps://") {
        return Protocol::Ipp;
    }
    let rest = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    if path.contains("/ipp/") || path.ends_with("/ipp") { Protocol::Ipp } else { Protocol::Wsd }
}

/// Check an address against the protocol's own URL policy (numeric local address, no TLS for IPP).
pub fn validate(address: &str) -> Result<Protocol, PrintError> {
    let protocol = protocol_of(address);
    match protocol {
        Protocol::Wsd => PrinterUrl::parse(address).map(|_| ()).map_err(PrintError::from)?,
        Protocol::Ipp => PrinterUri::parse(address).map(|_| ()).map_err(PrintError::from)?,
    }
    Ok(protocol)
}

/// The WS-Eventing listener configuration the test env var asks for: `PDFCRAFT_TEST_EVENTS_BIND`
/// (for instance `127.0.0.1:0`). `None` when unset or empty. A bind address that cannot be
/// advertised (unspecified) is an error.
pub fn events_from_env() -> Option<Result<NotifyConfig, PrintError>> {
    let text = std::env::var("PDFCRAFT_TEST_EVENTS_BIND").ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(events_config(text))
}

/// [`events_from_env`] for a given text (`ip:port`).
pub fn events_config(bind: &str) -> Result<NotifyConfig, PrintError> {
    let addr: SocketAddr =
        bind.parse().map_err(|e| PrintError::new(ErrorKind::InvalidRequest, format!("PDFCRAFT_TEST_EVENTS_BIND {bind:?} is not ip:port: {e}")))?;
    if addr.ip().is_unspecified() {
        return Err(PrintError::new(ErrorKind::InvalidRequest, "PDFCRAFT_TEST_EVENTS_BIND must name the address the printer can call, not 0.0.0.0"));
    }
    let advertise = format!("http://{}:{}/", addr.ip(), if addr.port() == 0 { 1 } else { addr.port() });
    NotifyConfig::new(addr, &advertise)
}

/// One network printer behind a registry that holds both network backends.
pub struct Station {
    protocol: Protocol,
    address: String,
    id: String,
    registry: Registry,
    transport: Transport,
    wsd: WsdBackend,
    events: bool,
    /// The deny list as it was when the station was opened.
    denied: Vec<String>,
    /// What the printer calls itself, once a query has read it; for the words shown to the person.
    display_name: std::sync::Mutex<String>,
}

impl Station {
    /// Open the printer at `address` (no connection is made). `events`: the opt-in WS-Eventing
    /// listener for a WS-Print printer (ignored for IPP, which polls). A printer on the deny list
    /// is [`ErrorKind::Denied`] and nothing is sent to it.
    pub fn open(address: &str, transport: Option<Transport>, events: Option<NotifyConfig>) -> Result<Self, PrintError> {
        Self::open_denying(address, transport, events, craft_print::deny_list())
    }

    /// [`Self::open`] with the deny list given instead of read from the environment (tests).
    pub fn open_denying(address: &str, transport: Option<Transport>, events: Option<NotifyConfig>, denied: Vec<String>) -> Result<Self, PrintError> {
        let protocol = validate(address)?;
        let transport = transport.unwrap_or_else(wsd::adapter::default_transport);
        let mut backend = WsdBackend::new().with_transport(Arc::clone(&transport));
        let with_events = events.is_some() && protocol == Protocol::Wsd;
        // The check comes before anything that could open a socket (the events listener binds one).
        check_denied(&denied, &[address])?;
        if let Some(config) = events.filter(|_| protocol == Protocol::Wsd) {
            backend = backend.with_events(config)?;
        }
        let ipp_backend = IppBackend::new().with_transport(Arc::clone(&transport));
        let (native, name) = match protocol {
            Protocol::Wsd => (backend.add(address)?, "wsd"),
            Protocol::Ipp => (ipp_backend.add(address)?, "ipp"),
        };
        check_denied(&denied, &[&native])?;
        let registry = Registry::new();
        registry.set_deny_list(Some(denied.clone()));
        registry.register(Box::new(backend.clone())).map_err(|e| PrintError::new(ErrorKind::Failed, e.to_string()))?;
        registry.register(Box::new(ipp_backend)).map_err(|e| PrintError::new(ErrorKind::Failed, e.to_string()))?;
        if let Some(converter) = registered_converter() {
            registry.set_converter(converter);
        }
        Ok(Self {
            protocol,
            id: namespaced_id(name, &native),
            address: native,
            registry,
            transport,
            wsd: backend,
            events: with_events,
            denied,
            display_name: std::sync::Mutex::default(),
        })
    }

    /// Use `converter` for this printer instead of the registered one: the print it was made for.
    pub fn set_converter(&self, converter: Arc<dyn DocumentConverter>) {
        self.registry.set_converter(converter);
    }

    /// Remember the name the printer reported (shown in status lines instead of its address).
    pub fn set_display_name(&self, name: &str) {
        *self.display_name.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = name.trim().to_owned();
    }

    /// The name the printer reported, or empty when no query has read one.
    pub fn display_name(&self) -> String {
        self.display_name.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// The canonical address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The registry's id of the printer (`wsd:<url>` or `ipp:<uri>`).
    pub fn printer_id(&self) -> &str {
        &self.id
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Is the WS-Eventing listener on for this printer?
    pub fn events_enabled(&self) -> bool {
        self.events
    }

    /// The listener and its subscription as plain text for a status line, when it is on.
    pub fn events_summary(&self) -> Option<String> {
        self.wsd.events_status().map(|s| format!("{s:?}"))
    }

    /// WS-Print: what the printer is and lists (`GetPrinterElements`). Sends no job. The name the
    /// printer reports is checked against the deny list; the answer is dropped when it matches.
    pub fn wsd_info(&self) -> Result<PrinterElements, PrintError> {
        check_denied(&self.denied, &[&self.address])?;
        let url = PrinterUrl::parse(&self.address).map_err(PrintError::from)?;
        let elements = wsd::printer_info(&Client::new(Arc::clone(&self.transport), url)).map_err(|e| e.print_error(Op::Query))?;
        check_denied(&self.denied, &[elements.description.as_ref().map_or("", |d| d.name.as_str())])?;
        Ok(elements)
    }

    /// IPP: the printer's attributes (`Get-Printer-Attributes`). Sends no job. The names the
    /// printer reports are checked against the deny list; the answer is dropped when one matches.
    pub fn ipp_info(&self) -> Result<PrinterAttrs, PrintError> {
        check_denied(&self.denied, &[&self.address])?;
        let uri = PrinterUri::parse(&self.address).map_err(PrintError::from)?;
        let attrs = ipp::Client::new(Arc::clone(&self.transport), uri).get_printer_attributes(&[]).map_err(PrintError::from)?;
        let reported: Vec<&str> = [&attrs.name, &attrs.info, &attrs.make_and_model].into_iter().flatten().map(String::as_str).collect();
        check_denied(&self.denied, &reported)?;
        Ok(attrs)
    }

    /// A tracked print of the print-ready `pdf`: one object for the whole print, advanced by the
    /// caller. `job.printer` is set to this printer; `consent` is the person's ticked box.
    pub fn job(&self, pdf: Vec<u8>, job: Job, options: JobOptions, consent: UserConfirmed) -> PrintJob {
        let job = Job { printer: Some(self.id.clone()), ..job };
        PrintJob::new(job, BytesSource::new(DocumentFormat::Pdf, pdf)).with_options(options).with_consent(consent).with_config(PrintJobConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_is_chosen_by_scheme_and_path() {
        for (address, expected) in [
            ("ipp://127.0.0.1:3921/ipp/print", Protocol::Ipp),
            ("ipps://127.0.0.1/ipp/print", Protocol::Ipp),
            ("http://127.0.0.1:3921/d/inkjet1/ipp/print", Protocol::Ipp),
            ("http://127.0.0.1:3921/ipp", Protocol::Ipp),
            ("HTTP://127.0.0.1:3921/IPP/print", Protocol::Ipp),
            ("http://127.0.0.1:3921/d/inkjet1/WebServices/PrinterService", Protocol::Wsd),
            ("http://192.168.1.50:5357/service", Protocol::Wsd),
            ("127.0.0.1:3921/x", Protocol::Wsd),
        ] {
            assert_eq!(protocol_of(address), expected, "{address}");
        }
    }

    #[test]
    fn both_urls_validate_and_open_without_connecting() {
        assert_eq!(validate("http://127.0.0.1:3921/d/inkjet1/WebServices/PrinterService").ok(), Some(Protocol::Wsd));
        assert_eq!(validate("ipp://127.0.0.1:3921/ipp/print").ok(), Some(Protocol::Ipp));
        assert!(validate("ipp://printer.example/ipp/print").is_err());
        let ipp = Station::open_denying("ipp://127.0.0.1:3921/ipp/print", None, None, Vec::new()).unwrap();
        assert!(ipp.printer_id().starts_with("ipp:"));
        let wsd = Station::open_denying("http://127.0.0.1:3921/d/inkjet1/WebServices/PrinterService", None, None, Vec::new()).unwrap();
        assert!(wsd.printer_id().starts_with("wsd:"));
        assert!(!wsd.events_enabled());
    }

    #[test]
    fn events_config_needs_a_callable_address() {
        assert!(events_config("127.0.0.1:0").is_ok());
        assert!(events_config("0.0.0.0:9000").is_err());
        assert!(events_config("nope").is_err());
    }

    fn deny(entries: &str) -> Vec<String> {
        craft_print::parse_deny_list(entries)
    }

    #[test]
    fn a_denied_address_is_refused_at_open_and_nothing_is_sent() {
        // The default transport would connect: a refusal that came after it would hang or fail differently.
        let error = Station::open_denying("http://127.0.0.1:9/0f1e2d3c-1111-4222-8333-444455556666", None, None, deny("0F1E2D3C11114222")).err().unwrap();
        assert_eq!(error.kind(), ErrorKind::Denied);
        assert!(error.message().contains("CRAFT_PRINT_DENY") && !error.message().contains("0f1e"), "{error}");
        let by_name = Station::open_denying("http://127.0.0.1:9/Brother-MFC", None, None, deny("brother")).err().unwrap();
        assert_eq!(by_name.kind(), ErrorKind::Denied);
        let ipp = Station::open_denying("ipp://127.0.0.1:631/ipp/print/brother", None, None, deny("brother")).err().unwrap();
        assert_eq!(ipp.kind(), ErrorKind::Denied);
    }

    #[test]
    fn an_allowed_address_opens_and_the_reported_name_is_checked() {
        let station = Station::open_denying("http://127.0.0.1:9/other", None, None, deny("brother;0f1e")).unwrap();
        assert_eq!(station.address(), "http://127.0.0.1:9/other");
        assert!(check_denied(&station.denied, &["Brother MFC-9330CDW"]).is_err(), "a reported name on the list is dropped");
        assert!(check_denied(&station.denied, &["Office Laser", ""]).is_ok());
    }
}
