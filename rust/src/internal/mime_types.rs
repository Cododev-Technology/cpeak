use std::sync::{LazyLock, PoisonError, RwLock};

use crate::types::StringMap;

// Developers can expand this if needed in the Cpeak::new() constructor
pub(crate) static MIME_TYPES: LazyLock<RwLock<StringMap>> = LazyLock::new(|| {
    let mime_types = [
        ("html", "text/html"),
        ("css", "text/css"),
        ("js", "application/javascript"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("png", "image/png"),
        ("svg", "image/svg+xml"),
        ("txt", "text/plain"),
        ("eot", "application/vnd.ms-fontobject"),
        ("otf", "font/otf"),
        ("ttf", "font/ttf"),
        ("woff", "font/woff"),
        ("woff2", "font/woff2"),
        ("gif", "image/gif"),
        ("ico", "image/x-icon"),
        ("json", "application/json"),
        ("map", "application/json"),
        ("webmanifest", "application/manifest+json"),
        ("xml", "application/xml"),
        ("pdf", "application/pdf"),
        ("mp4", "video/mp4"),
        ("webm", "video/webm"),
        ("mp3", "audio/mpeg"),
        ("wav", "audio/wav"),
        ("webp", "image/webp"),
        ("avif", "image/avif"),
    ];

    RwLock::new(
        mime_types
            .into_iter()
            .map(|(ext, mime)| (ext.to_string(), mime.to_string()))
            .collect(),
    )
});

// Look up the MIME type of a file extension (like "html" or "css")
pub(crate) fn get_mime(file_extension: &str) -> Option<String> {
    MIME_TYPES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(file_extension)
        .cloned()
}

// Merge developer-supplied mime types with the defaults
pub(crate) fn register_mime_types(mime_types: StringMap) {
    MIME_TYPES
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(mime_types);
}
