//! The size a consent prompt shows for a file on this machine is the size on disk.
//!
//! A `file:` handle carries the size the app claims, and `Disclosure::for_request` (pure, no I/O)
//! can only repeat it. Here the file is looked at instead: a regular file on this machine shows
//! its length, and one that cannot be looked at (gone, a folder, another machine's share) shows
//! no size rather than the claimed one. An address that does not parse cannot be checked at all,
//! whatever its scheme looks like, so it shows no size either. Other handles and inline data keep
//! the size they carry.
//!
//! The address is read with the same parser and host rule the uploaders use, so the size shown
//! is the size of the file that would be read.

use craft_ai_core::{Disclosure, Payload, Request};

/// Replaces the claimed size of every `file:` input in `disclosure` with the measured one.
pub(crate) fn measure_local_files(disclosure: &mut Disclosure, request: &Request) {
    // `Disclosure::for_request` makes one item per input, in order.
    for (item, input) in disclosure.items.iter_mut().zip(&request.inputs) {
        let Payload::Handle(handle) = &input.payload else {
            continue;
        };
        match url::Url::parse(&handle.uri) {
            Ok(url) if url.scheme() == "file" => item.bytes = local_file_len(&url),
            Ok(_) => {}
            Err(_) => item.bytes = None,
        }
    }
}

/// The length of the regular file `url` names on this machine.
fn local_file_len(url: &url::Url) -> Option<u64> {
    // On Windows a host turns into a network share. Only this machine's files are measured.
    if !matches!(url.host_str(), None | Some("" | "localhost")) {
        return None;
    }
    let meta = std::fs::metadata(url.to_file_path().ok()?).ok()?;
    meta.is_file().then_some(meta.len())
}
