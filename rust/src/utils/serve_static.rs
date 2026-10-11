use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::internal::mime_types::get_mime;
use crate::internal::uri::{decode_uri_component, normalize_path, resolve_path};
use crate::types::{Middleware, Next};
use crate::{CpeakRequest, CpeakResponse};

#[derive(Debug, Clone, Default)]
pub struct ServeStaticOptions {
    // Serve the files under a URL prefix, like "/static"
    pub prefix: String,
    // Look for the files on every request instead of once at startup
    pub live: bool,
    // Files and folders (relative to the served folder) that should not be served
    pub exclude: Vec<String>,
}

struct FileRoute {
    path: String,
    mime: String,
}

enum StaticFiles {
    Live {
        resolved_folder: PathBuf,
        excludes: Vec<PathBuf>,
    },
    Cached {
        files_map: HashMap<String, FileRoute>,
    },
}

pub fn serve_static(
    folder_path: &str,
    options: impl Into<Option<ServeStaticOptions>>,
) -> impl Middleware {
    let options = options.into().unwrap_or_default();
    let prefix = options.prefix;
    let live = options.live;

    let resolved_folder = resolve_path(folder_path);
    let excludes: Vec<PathBuf> = options
        .exclude
        .iter()
        .map(|e| normalize_path(&resolved_folder.join(e.trim_start_matches('/'))))
        .collect();

    // This processes the folder on every request, which is useful during development when files are changing often.
    // In production, it's better to process the folder once and store the file paths in memory for faster access if file names are not changing often.
    // If file names dynamically change often in production, then live option can be set to true to process the folder on every request, but it may have performance implications.
    let static_files = if live {
        StaticFiles::Live {
            resolved_folder,
            excludes,
        }
    } else {
        let process_folder = |folder_path: &Path| {
            process_folder(folder_path, folder_path, &excludes).unwrap_or_else(|error| {
                panic!(
                    "serve_static: failed to read \"{}\": {error}",
                    folder_path.display()
                )
            })
        };

        let files_array_to_files_map = |files_array: Vec<String>| {
            let mut files_map = HashMap::new();
            for file in files_array {
                let file_extension = file_extension(&file);
                if let Some(mime) = get_mime(file_extension) {
                    files_map.insert(
                        format!("{prefix}{file}"),
                        FileRoute {
                            path: format!("{folder_path}{file}"),
                            mime,
                        },
                    );
                }
            }
            files_map
        };

        // Start processing the folder
        StaticFiles::Cached {
            files_map: files_array_to_files_map(process_folder(&resolved_folder)),
        }
    };

    let static_files = Arc::new(static_files);

    async move |req: &mut CpeakRequest, res: &mut CpeakResponse, next: Next| {
        let pathname = req.url.split('?').next().unwrap_or("");
        // Just like a browser would, turn "%20" back into a space and so on
        let pathname: Cow<str> = if pathname.contains('%') {
            Cow::Owned(decode_uri_component(pathname).unwrap_or_else(|| pathname.to_string()))
        } else {
            Cow::Borrowed(pathname)
        };

        match &*static_files {
            StaticFiles::Live {
                resolved_folder,
                excludes,
            } => {
                let Some(unprefixed) = pathname.strip_prefix(prefix.as_str()) else {
                    return next();
                };
                let file_path =
                    normalize_path(&resolved_folder.join(unprefixed.trim_start_matches('/')));
                let Some(mime) = get_mime(file_extension(&file_path.to_string_lossy())) else {
                    return next();
                };

                if !file_path.starts_with(resolved_folder) {
                    return next();
                }
                if excludes.iter().any(|e| file_path.starts_with(e)) {
                    return next();
                }

                let is_file = tokio::fs::metadata(&file_path)
                    .await
                    .is_ok_and(|stat| stat.is_file());
                if is_file {
                    return res
                        .send_file(&file_path.to_string_lossy(), mime.as_str())
                        .await;
                }

                next()
            }
            StaticFiles::Cached { files_map } => {
                if let Some(file_route) = files_map.get(pathname.as_ref()) {
                    return res
                        .send_file(&file_route.path, file_route.mime.as_str())
                        .await;
                }

                next()
            }
        }
    }
}

fn process_folder(
    folder_path: &Path,
    parent_folder: &Path,
    excludes: &[PathBuf],
) -> std::io::Result<Vec<String>> {
    let mut static_files: Vec<String> = Vec::new();

    // Read the contents of the folder
    let mut files: Vec<_> = std::fs::read_dir(folder_path)?.collect::<Result<_, _>>()?;
    files.sort_by_key(|entry| entry.file_name());

    // Loop through the files and subfolders
    for file in files {
        let full_path = file.path();
        if excludes.iter().any(|e| full_path.starts_with(e)) {
            continue;
        }

        // Check if it's a directory
        if std::fs::metadata(&full_path)?.is_dir() {
            // If it's a directory, recursively process it
            let subfolder_files = process_folder(&full_path, parent_folder, excludes)?;
            static_files.extend(subfolder_files);
        } else {
            // If it's a file, add it to the array
            let Ok(relative_path) = full_path.strip_prefix(parent_folder) else {
                continue;
            };
            let relative_path: Vec<String> = relative_path
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect();
            let relative_path = relative_path.join("/");
            if get_mime(file_extension(&relative_path)).is_some() {
                static_files.push(format!("/{relative_path}"));
            }
        }
    }

    Ok(static_files)
}

// Like Node.js's path.extname(file).slice(1): "css" for "/styles/main.css", "" for "/.htaccess"
fn file_extension(file: &str) -> &str {
    let file_name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    match file_name.rfind('.') {
        Some(0) | None => "",
        Some(dot_index) => &file_name[dot_index + 1..],
    }
}
