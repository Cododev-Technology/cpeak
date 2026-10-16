// Rust versions of the URL helpers that JavaScript has built in and that the Node.js version of Cpeak relies on.

use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::types::StringMap;

// JavaScript's encodeURIComponent()
pub(crate) fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

// JavaScript's decodeURIComponent(). Returns None where JavaScript would throw a URIError
// (a "%" that isn't followed by two hex digits, or bytes that aren't valid UTF-8).
pub(crate) fn decode_uri_component(value: &str) -> Option<String> {
    if !value.contains('%') {
        return Some(value.to_string());
    }

    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let high = hex_value(*bytes.get(i + 1)?)?;
            let low = hex_value(*bytes.get(i + 2)?)?;
            decoded.push(high << 4 | low);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

// Parse a query string (like "key1=value1&key2=value2") the same way JavaScript's
// `Object.fromEntries(new URLSearchParams(query))` does. When a key shows up more than once,
// the last value wins.
pub(crate) fn parse_search_params(query: &str) -> StringMap {
    let query = query.strip_prefix('?').unwrap_or(query);
    if query.is_empty() {
        return StringMap::new();
    }
    // Room for every pair up front, so the map never has to grow
    let pairs = query.bytes().filter(|&byte| byte == b'&').count() + 1;
    let mut params = StringMap::with_capacity(pairs);

    let mut rest = query;
    loop {
        let (pair, next) = match rest.bytes().position(|byte| byte == b'&') {
            Some(at) => (&rest[..at], Some(&rest[at + 1..])),
            None => (rest, None),
        };
        if !pair.is_empty() {
            let (name, value) = match pair.bytes().position(|byte| byte == b'=') {
                Some(at) => (&pair[..at], &pair[at + 1..]),
                None => (pair, ""),
            };
            params.insert(form_decode(name), form_decode(value));
        }
        match next {
            Some(next) => rest = next,
            None => break,
        }
    }

    params
}

// Decoding for application/x-www-form-urlencoded: "+" is a space, and malformed
// percent-encoding is left as it is instead of failing.
fn form_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    // Most names and values have nothing to decode
    if !bytes.iter().any(|&byte| byte == b'+' || byte == b'%') {
        return value.to_owned();
    }
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => decoded.push(b' '),
            b'%' => match (
                bytes.get(i + 1).copied().and_then(hex_value),
                bytes.get(i + 2).copied().and_then(hex_value),
            ) {
                (Some(high), Some(low)) => {
                    decoded.push(high << 4 | low);
                    i += 2;
                }
                _ => decoded.push(b'%'),
            },
            byte => decoded.push(byte),
        }
        i += 1;
    }
    match String::from_utf8(decoded) {
        Ok(decoded) => decoded,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// Node.js's path.resolve(): make a path absolute (relative to the current working directory)
// and normalize away the "." and ".." segments.
pub(crate) fn resolve_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        normalize_path(path)
    } else {
        with_current_dir(|cwd| normalize_path(&cwd.join(path)))
    }
}

// Node.js's process.cwd(). Like Node.js, we remember it instead of asking the operating system
// every time (on macOS, that's four system calls). Node.js forgets it on process.chdir(), and
// since Rust can't tell us about std::env::set_current_dir(), we look it up again every second.
fn with_current_dir<T>(f: impl FnOnce(&Path) -> T) -> T {
    thread_local! {
        static CWD: RefCell<Option<(PathBuf, Instant)>> = const { RefCell::new(None) };
    }
    CWD.with(|cwd| {
        let mut cwd = cwd.borrow_mut();
        let fresh = cwd
            .as_ref()
            .is_some_and(|(_, looked_up)| looked_up.elapsed() < Duration::from_secs(1));
        if !fresh {
            *cwd = Some((std::env::current_dir().unwrap_or_default(), Instant::now()));
        }
        let (path, _) = cwd
            .as_ref()
            .expect("the current directory was just looked up");
        f(path)
    })
}

// Node.js's path.normalize(): resolve the "." and ".." segments without touching the disk.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match components.last() {
                Some(Component::Normal(_)) => {
                    components.pop();
                }
                // There is nothing above the root, so "/.." is just "/"
                Some(Component::RootDir | Component::Prefix(_)) => {}
                // A relative path that climbs up, like "../../file"
                _ => components.push(component),
            },
            other => components.push(other),
        }
    }
    components.iter().collect()
}
