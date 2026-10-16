use std::fs;
use std::io::Write;

use crate::internal::compression::{compress_as_small_as_possible, is_compressible_type};
use crate::internal::errors::CpeakError;
use crate::internal::mime_types::get_mime;
use crate::internal::uri::resolve_path;
use crate::utils::serve_static::{file_extension, process_folder};

// Compress the files of a folder ahead of time, as part of your build: next to app.js it
// writes app.js.br and app.js.gz, as small as brotli and gzip can make them. serve_static and
// res.send_file send those copies as they are, so your server doesn't compress the same file
// again for every request.
//
// A copy gets the modification time of its file, and that is how it's told apart from a copy
// of another version: it's only sent while the two times are the same. Files that already
// have their copies are skipped, so running this again costs next to nothing. It returns how
// many copies it wrote.
pub fn precompress(folder_path: &str) -> Result<usize, CpeakError> {
    let folder = resolve_path(folder_path);
    let mut written = 0;

    for file in process_folder(&folder, &folder, &[])? {
        // The same files that the server compresses when it sends them: text, not images
        let compressible =
            get_mime(file_extension(&file)).is_some_and(|mime| is_compressible_type(&mime));
        if !compressible {
            continue;
        }

        let path = format!("{}{file}", folder.to_string_lossy());
        let modified = fs::metadata(&path)?.modified()?;
        let mut content = None;

        for extension in ["br", "gz"] {
            let copy_path = format!("{path}.{extension}");
            let up_to_date = fs::metadata(&copy_path)
                .is_ok_and(|copy| copy.modified().is_ok_and(|time| time == modified));
            if up_to_date {
                continue;
            }

            let content = match &content {
                Some(content) => content,
                None => content.insert(fs::read(&path)?),
            };
            let compressed = compress_as_small_as_possible(content, extension)?;
            // A copy that isn't smaller than its file is of no use to anyone
            if compressed.len() >= content.len() {
                let _ = fs::remove_file(&copy_path);
                continue;
            }

            let mut copy = fs::File::create(&copy_path)?;
            copy.write_all(&compressed)?;
            copy.set_modified(modified)?;
            written += 1;
        }
    }

    Ok(written)
}
