use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use bytes::Bytes;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::internal::compression::{compress_and_send, pipe, Body, ChunkSource, Readable};
use crate::internal::errors::{framework_error, is_client_disconnect, CpeakError, ErrorCode};
use crate::internal::files::{self, File, Version};
use crate::internal::mime_types::get_mime;
use crate::internal::uri::{normalize_path, resolve_path};
use crate::types::{LocalBoxFuture, Middleware, Next};
use crate::{CpeakRequest, CpeakResponse};

pub const MAX_PATTERN: usize = 128;

// How deep includes can go, so a template that includes itself fails instead of going on forever
// (in the Node.js version, it would end with a stack overflow)
const MAX_INCLUDE_DEPTH: usize = 64;

// The same result as value.replace(/&/g, "&amp;").replace(/</g, "&lt;")... but in one go
fn escape_html(value: &str, output: &mut String) {
    if !value.contains(['&', '<', '>', '"', '\'']) {
        output.push_str(value);
        return;
    }
    for c in value.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            c => output.push(c),
        }
    }
}

// A template, split up into its pieces: text to send as it is, and the places where the data
// goes. Where the tags are only depends on the template, not on the data, so a template can be
// split up once and then filled in with new data on every request.
enum Part {
    Text(String),
    // {{ key }}: the value goes in HTML-escaped
    Escaped(String),
    // <cpeak html={ key } />: the value goes in as it is
    Raw(String),
}

fn push_text(parts: &mut Vec<Part>, text: &str) {
    match parts.last_mut() {
        Some(Part::Text(last)) => last.push_str(text),
        _ => parts.push(Part::Text(text.to_string())),
    }
}

// Fill the pieces of a template in with the data
fn render_parts(parts: &[Part], data: &Value, output: &mut String) {
    for part in parts {
        match part {
            Part::Text(text) => output.push_str(text),
            Part::Raw(raw_key) => {
                if let Some(val) = data.get(raw_key.as_str()) {
                    output.push_str(&to_js_string(val));
                }
            }
            Part::Escaped(escaped_key) => {
                if let Some(val) = data.get(escaped_key.as_str()) {
                    escape_html(&to_js_string(val), output);
                }
            }
        }
    }
}

struct TemplateTransform {
    tail: String,
    // The bytes of a character that got cut in half at the end of the last chunk
    partial_char: Vec<u8>,
    base_dir: PathBuf,
    // The files that were included, to know when the template changes (None: too big to track)
    dependencies: Vec<(PathBuf, Option<Version>)>,
    // How deep in includes we are
    depth: usize,
}

impl TemplateTransform {
    fn new(base_dir: PathBuf, depth: usize) -> Self {
        Self {
            tail: String::new(),
            partial_char: Vec::new(),
            base_dir,
            dependencies: Vec::new(),
            depth,
        }
    }

    async fn transform(&mut self, chunk: &[u8], parts: &mut Vec<Part>) -> io::Result<()> {
        let str = std::mem::take(&mut self.tail) + &self.decode_utf8(chunk);
        if str.len() <= MAX_PATTERN {
            self.tail = str;
            return Ok(());
        }

        let mut boundary = floor_char_boundary(&str, str.len() - MAX_PATTERN);

        // Prevent cutting a tag in two
        for (opener, closer) in [("{{", "}}"), ("<cpeak", ">")] {
            let Some(last) = last_index_of(&str, opener, boundary.saturating_sub(1)) else {
                continue;
            };
            let close_idx = index_of(&str, closer, last + opener.len());
            if close_idx.is_none_or(|close_idx| close_idx >= boundary) {
                boundary = boundary.min(last);
            }
        }

        self.tail = str[boundary..].to_string();
        let safe = &str[..boundary];
        if safe.is_empty() {
            return Ok(());
        }
        self.process(safe, parts).await
    }

    async fn flush(&mut self, parts: &mut Vec<Part>) -> io::Result<()> {
        let mut tail = std::mem::take(&mut self.tail);
        // A character that never got its remaining bytes is invalid, so it becomes a "�"
        if !self.partial_char.is_empty() {
            tail.push_str(&String::from_utf8_lossy(&std::mem::take(
                &mut self.partial_char,
            )));
        }
        if tail.is_empty() {
            return Ok(());
        }
        self.process(&tail, parts).await
    }

    fn process<'a>(
        &'a mut self,
        str: &'a str,
        parts: &'a mut Vec<Part>,
    ) -> LocalBoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let mut last = 0;

            while let Some((idx, end, tag)) = find_tag(str, last) {
                if idx > last {
                    push_text(parts, &str[last..idx]);
                }

                match tag {
                    Tag::Include(include_src) => {
                        if self.depth >= MAX_INCLUDE_DEPTH {
                            return Err(io::Error::other("too many nested includes"));
                        }
                        let include_path = normalize_path(&self.base_dir.join(include_src));
                        // Small files come from memory (see internal/files.rs)
                        let (content, version) = match files::read(&include_path) {
                            Ok(File::Content(content, version)) => (content, Some(version)),
                            _ => (Bytes::from(tokio::fs::read(&include_path).await?), None),
                        };
                        let mut nested = TemplateTransform::new(
                            include_path
                                .parent()
                                .unwrap_or(Path::new("/"))
                                .to_path_buf(),
                            self.depth + 1,
                        );
                        self.dependencies.push((include_path, version));
                        nested.transform(&content, parts).await?;
                        nested.flush(parts).await?;
                        self.dependencies.append(&mut nested.dependencies);
                    }
                    Tag::Raw(raw_key) => parts.push(Part::Raw(raw_key.trim().to_string())),
                    Tag::Escaped(escaped_key) => {
                        parts.push(Part::Escaped(escaped_key.trim().to_string()))
                    }
                }

                last = end;
            }

            if last < str.len() {
                push_text(parts, &str[last..]);
            }
            Ok(())
        })
    }

    // Turn the chunk into text. A multi-byte character can be cut in half at the end of a
    // chunk, so we hold on to its first bytes until the next chunk brings the rest.
    fn decode_utf8(&mut self, chunk: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.partial_char);
        bytes.extend_from_slice(chunk);

        let mut decoded = String::with_capacity(bytes.len());
        let mut rest = &bytes[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    break;
                }
                Err(error) => {
                    let (valid, after_valid) = rest.split_at(error.valid_up_to());
                    decoded.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    match error.error_len() {
                        // The chunk ends in the middle of a character
                        None => {
                            self.partial_char = after_valid.to_vec();
                            break;
                        }
                        Some(invalid_len) => {
                            decoded.push(char::REPLACEMENT_CHARACTER);
                            rest = &after_valid[invalid_len..];
                        }
                    }
                }
            }
        }
        decoded
    }
}

enum Tag<'a> {
    Include(&'a str),
    Raw(&'a str),
    Escaped(&'a str),
}

// Finds the next match of the same regex that the Node.js version uses:
// /<cpeak\s+include="([^"]+)"\s*\/?>|<cpeak\s+html=\{([^}]+)\}\s*\/?>|\{\{([^}]+)\}\}/g
fn find_tag(str: &str, from: usize) -> Option<(usize, usize, Tag<'_>)> {
    let bytes = str.as_bytes();
    for idx in from..bytes.len() {
        match bytes[idx] {
            b'<' if str[idx..].starts_with("<cpeak") => {
                if let Some((end, tag)) = match_cpeak_tag(str, idx) {
                    return Some((idx, end, tag));
                }
            }
            b'{' if str[idx..].starts_with("{{") => {
                if let Some((end, key)) = match_escaped(str, idx) {
                    return Some((idx, end, Tag::Escaped(key)));
                }
            }
            _ => {}
        }
    }
    None
}

// <cpeak\s+include="([^"]+)"\s*\/?>  or  <cpeak\s+html=\{([^}]+)\}\s*\/?>
fn match_cpeak_tag(str: &str, start: usize) -> Option<(usize, Tag<'_>)> {
    let rest = &str[start + "<cpeak".len()..];
    let attribute = rest.trim_start_matches(is_js_whitespace);
    if attribute.len() == rest.len() {
        return None;
    }

    if let Some(after) = attribute.strip_prefix("include=\"") {
        let quote = after.find('"').filter(|quote| *quote > 0)?;
        let end = match_tag_end(str, str.len() - after.len() + quote + 1)?;
        return Some((end, Tag::Include(&after[..quote])));
    }

    if let Some(after) = attribute.strip_prefix("html={") {
        let brace = after.find('}').filter(|brace| *brace > 0)?;
        let end = match_tag_end(str, str.len() - after.len() + brace + 1)?;
        return Some((end, Tag::Raw(&after[..brace])));
    }

    None
}

// \s*\/?>
fn match_tag_end(str: &str, from: usize) -> Option<usize> {
    let rest = str[from..].trim_start_matches(is_js_whitespace);
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let rest = rest.strip_prefix('>')?;
    Some(str.len() - rest.len())
}

// \{\{([^}]+)\}\}
fn match_escaped(str: &str, start: usize) -> Option<(usize, &str)> {
    let key_start = start + 2;
    let key_len = str[key_start..].find('}').filter(|key_len| *key_len > 0)?;
    let key_end = key_start + key_len;
    if !str[key_end..].starts_with("}}") {
        return None;
    }
    Some((key_end + 2, &str[key_start..key_end]))
}

// The same characters as \s in a JavaScript regex
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

// Turns a value into text the same way JavaScript's String(value) does
fn to_js_string(value: &Value) -> String {
    match value {
        Value::String(string) => string.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    to_js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

// Like JavaScript's str.lastIndexOf(search, fromIndex)
fn last_index_of(str: &str, search: &str, from_index: usize) -> Option<usize> {
    let haystack = str.as_bytes();
    let last_start = from_index.min(haystack.len().checked_sub(search.len())?);
    (0..=last_start)
        .rev()
        .find(|&i| haystack[i..].starts_with(search.as_bytes()))
}

// Like JavaScript's str.indexOf(search, fromIndex)
fn index_of(str: &str, search: &str, from_index: usize) -> Option<usize> {
    str.get(from_index..)?
        .find(search)
        .map(|index| index + from_index)
}

fn floor_char_boundary(str: &str, mut index: usize) -> usize {
    while !str.is_char_boundary(index) {
        index -= 1;
    }
    index
}

// Reads a template file chunk by chunk and renders it on the way out (for templates too
// big to keep in memory, see compiled_template for the others)
struct TemplateSource {
    file: tokio::fs::File,
    buffer: Vec<u8>,
    transform: TemplateTransform,
    data: Value,
    ended: bool,
}

// The same chunk size that Node.js's fs.createReadStream uses by default
const CHUNK_SIZE: usize = 64 * 1024;

impl TemplateSource {
    async fn open(resolved: &Path, data: Value) -> io::Result<Self> {
        let file = tokio::fs::File::open(resolved).await?;
        let base_dir = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
        Ok(Self {
            file,
            buffer: vec![0u8; CHUNK_SIZE],
            transform: TemplateTransform::new(base_dir, 0),
            data,
            ended: false,
        })
    }

    async fn next_str(&mut self) -> io::Result<Option<String>> {
        while !self.ended {
            let bytes_read = self.file.read(&mut self.buffer).await?;
            let mut parts = Vec::new();
            if bytes_read == 0 {
                self.ended = true;
                self.transform.flush(&mut parts).await?;
            } else {
                self.transform
                    .transform(&self.buffer[..bytes_read], &mut parts)
                    .await?;
            }
            let mut rendered = String::new();
            render_parts(&parts, &self.data, &mut rendered);
            if !rendered.is_empty() {
                return Ok(Some(rendered));
            }
        }
        Ok(None)
    }
}

impl ChunkSource for TemplateSource {
    fn next_chunk(&mut self) -> LocalBoxFuture<'_, io::Result<Option<Bytes>>> {
        Box::pin(async move { Ok(self.next_str().await?.map(Bytes::from)) })
    }
}

// A small template, split up into its pieces once (see Part)
struct CompiledTemplate {
    parts: Vec<Part>,
    // The template file and the files it includes, as they were when we split it up
    dependencies: Vec<(PathBuf, Version)>,
    // When we last checked those files on the disk (see files::CHECK_INTERVAL)
    checked_at: Cell<Instant>,
    // How big the page was last time, so we can make room for the next one in one go
    size: Cell<usize>,
}

// We don't keep more templates than this in memory (per worker thread)
const MAX_COMPILED_TEMPLATES: usize = 1024;

thread_local! {
    static COMPILED_TEMPLATES: RefCell<HashMap<PathBuf, Rc<CompiledTemplate>>> = RefCell::new(HashMap::new());
}

// The pieces of a small template. It's split up once, and then reused for as long as neither
// the template nor its includes change (they're checked on the disk again when it's been more
// than files::CHECK_INTERVAL). None for a template too big to keep in memory, which is streamed instead.
async fn compiled_template(resolved: &Path) -> io::Result<Option<Rc<CompiledTemplate>>> {
    let cached = COMPILED_TEMPLATES.with(|templates| templates.borrow().get(resolved).cloned());
    if let Some(template) = cached {
        if template.checked_at.get().elapsed() < files::CHECK_INTERVAL {
            return Ok(Some(template));
        }
        let unchanged = template.dependencies.iter().all(|(path, version)| {
            version.settled() && Version::current(path).as_ref() == Some(version)
        });
        if unchanged {
            template.checked_at.set(Instant::now());
            return Ok(Some(template));
        }
    }

    let (content, version) = match files::read(resolved)? {
        File::Content(content, version) => (content, version),
        File::TooBig(_) | File::NotAFile => return Ok(None),
    };

    let base_dir = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
    let mut transform = TemplateTransform::new(base_dir, 0);
    let mut parts = Vec::new();
    // The same chunks that reading the file from the disk would give
    for chunk in content.chunks(CHUNK_SIZE) {
        transform.transform(chunk, &mut parts).await?;
    }
    transform.flush(&mut parts).await?;

    let mut dependencies = vec![(resolved.to_path_buf(), version)];
    let mut cacheable = version.settled();
    for (path, version) in transform.dependencies {
        match version {
            Some(version) => {
                cacheable &= version.settled();
                dependencies.push((path, version));
            }
            None => cacheable = false,
        }
    }

    let template = Rc::new(CompiledTemplate {
        parts,
        dependencies,
        checked_at: Cell::new(Instant::now()),
        size: Cell::new(content.len()),
    });
    if cacheable {
        COMPILED_TEMPLATES.with(|templates| {
            let mut templates = templates.borrow_mut();
            if templates.len() >= MAX_COMPILED_TEMPLATES {
                templates.clear();
            }
            templates.insert(resolved.to_path_buf(), template.clone());
        });
    }
    Ok(Some(template))
}

// A page that's already rendered, sent as a single chunk
struct RenderedPage(Option<Bytes>);

impl ChunkSource for RenderedPage {
    fn next_chunk(&mut self) -> LocalBoxFuture<'_, io::Result<Option<Bytes>>> {
        let page = self.0.take().filter(|page| !page.is_empty());
        Box::pin(async move { Ok(page) })
    }
}

// Render a template, from its pieces when it's small enough to keep them in memory
async fn render_template(resolved: &Path, data: Value) -> io::Result<Readable> {
    match compiled_template(resolved).await? {
        Some(template) => {
            let mut rendered = String::with_capacity(template.size.get());
            render_parts(&template.parts, &data, &mut rendered);
            template.size.set(rendered.len());
            Ok(Readable::from_source(RenderedPage(Some(Bytes::from(
                rendered,
            )))))
        }
        None => Ok(Readable::from_source(
            TemplateSource::open(resolved, data).await?,
        )),
    }
}

pub fn render() -> impl Middleware {
    async |_req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        res.render_enabled = true;
        next()
    }
}

// res.render()
impl CpeakResponse {
    pub async fn render<'a>(
        &mut self,
        file_path: &str,
        data: impl Serialize,
        mime: impl Into<Option<&'a str>>,
    ) -> Result<(), CpeakError> {
        if !self.render_enabled {
            return Err(framework_error(
                "render middleware not registered. Add render() via app.before_each(render()) to use res.render.",
                ErrorCode::RenderNotEnabled,
                None,
                false,
            ));
        }
        if self.headers_sent() {
            return Ok(());
        }
        let mime = match mime.into() {
            Some(mime) => mime.to_string(),
            None => {
                let file_extension = file_path
                    .rfind('.')
                    .map_or("", |dot_index| &file_path[dot_index + 1..]);
                match get_mime(file_extension) {
                    Some(mime) => mime,
                    None => {
                        return Err(framework_error(
                            format!(
                                "MIME type is missing for \"{file_path}\". Pass it as the third argument or register the extension via Cpeak::new(CpeakOptions {{ mime_types: ... }}), like (\"{}\", \"...\").",
                                if file_extension.is_empty() { "ext" } else { file_extension }
                            ),
                            ErrorCode::MissingMime,
                            None,
                            false,
                        ))
                    }
                }
            }
        };

        let resolved = resolve_path(file_path);
        let data = serde_json::to_value(data)?;

        let result: Result<(), CpeakError> = async {
            let source = render_template(&resolved, data).await?;

            if let Some(compression) = self.compression.clone() {
                return compress_and_send(self, &mime, Body::Readable(source), &compression, None)
                    .await;
            }

            self.set_header("Content-Type", &mime);
            pipe(source, self).await
        }
        .await;

        result.map_err(|err| {
            framework_error(
                format!("Failed to render \"{file_path}.\" Error: {err}"),
                ErrorCode::RenderFail,
                None,
                is_client_disconnect(&err),
            )
        })
    }
}

// Render a template into a String instead of sending it (render.string in the Node.js version)
pub async fn string(file_path: &str, data: impl Serialize) -> Result<String, CpeakError> {
    let resolved = resolve_path(file_path);
    let mut source = render_template(&resolved, serde_json::to_value(data)?).await?;
    let mut rendered = Vec::new();
    while let Some(chunk) = source.next_chunk().await? {
        rendered.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(rendered)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()))
}
