//! An answer's text, read up to a limit on what it decodes to. ureq's own `limit` counts the bytes
//! on the wire, before a `Content-Encoding: gzip` the server chooses expands them, so a few
//! megabytes could fill the memory.

use std::io::{self, Read};

use ureq::Body;

// the text of `body`, or `BodyExceedsLimit` once it decodes to more than `limit` bytes, or
// `InvalidData` for what is not UTF-8
pub fn text(body: &mut Body, limit: u64) -> Result<String, ureq::Error> {
    let mut bytes = Vec::new();

    body.with_config()
        .limit(limit)
        .reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;

    if bytes.len() as u64 > limit {
        return Err(ureq::Error::BodyExceedsLimit(limit));
    }

    String::from_utf8(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidData).into())
}
