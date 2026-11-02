//! The size a consent prompt shows for a file on this machine is the size on disk.
//!
//! A `file:` handle carries the size the app claims, and `Disclosure::for_request` (pure, no I/O)
//! can only repeat it. Here the file is looked at instead: a regular file on this machine shows
//! its length, and one that cannot be looked at (gone, a folder, another machine's share, not a
//! valid address) shows no size rather than the claimed one. Other handles and inline data keep
//! the size they carry.

use craft_ai_core::{Disclosure, Payload, Request};

/// Replaces the claimed size of every `file:` input in `disclosure` with the measured one.
pub(crate) fn measure_local_files(disclosure: &mut Disclosure, request: &Request) {
    for (item, input) in disclosure.items.iter_mut().zip(&request.inputs) {
        let Payload::Handle(handle) = &input.payload else {
            continue;
        };
        let url = url::Url::parse(&handle.uri).ok();
        // Even a malformed file address must discard the app's unverified claim.
        let is_file = url.as_ref().map_or_else(
            || {
                handle
                    .uri
                    .split_once(':')
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
            },
            |url| url.scheme() == "file",
        );
        if is_file {
            item.bytes = url.as_ref().and_then(local_file_len);
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
