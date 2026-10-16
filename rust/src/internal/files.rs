// Reading the files that res.send_file(), serve_static() and res.render() send out.
//
// The Node.js version reads a file from the disk on every request, through Node's thread pool.
// Doing that here (with tokio's thread pool) means several trips to another thread and back
// for every request. So instead, small files are kept in memory, with one copy per worker
// thread so the threads never wait on each other.
//
// A file we kept is checked against the disk again (a stat) when it's been more than
// CHECK_INTERVAL since we last looked, so a changed file is sent within a tenth of a second.
// (Checking on every request costs more than sending the file: the stats of all the threads
// wait on each other in the kernel. Drogon, for comparison, doesn't look for 5 seconds.)

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;

// Bigger files are streamed from the disk every time
pub(crate) const MAX_CACHED_FILE_SIZE: u64 = 1024 * 1024;

// How much memory a worker thread can use for files. When it's full, we start over.
const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;

// How long we trust what we know about a file, before checking it on the disk again
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_millis(100);

// A file that changed less than this long ago is read again on every request. Some file
// systems only keep the modification time to the second (or two), so a file changed twice
// within the same second, to the same size, could otherwise look unchanged.
const SETTLE_TIME: Duration = Duration::from_secs(2);

pub(crate) enum File {
    // The whole content of a small file, and which version of the file it is
    Content(Bytes, Version),
    // A file too big to keep in memory, of this size
    TooBig(u64),
    NotAFile,
}

struct Cached {
    content: Bytes,
    version: Version,
    // When we last checked that the file on the disk is still this version
    checked_at: Instant,
}

// What tells us that a file is still the same one we read before
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) struct Version {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Version {
    fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }

    // The version of the file on the disk right now (None if it's gone)
    pub(crate) fn current(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| Self::of(&metadata))
    }

    // Whether the file has been left alone for long enough that its modification time can be trusted
    pub(crate) fn settled(&self) -> bool {
        self.modified
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= SETTLE_TIME)
    }
}

thread_local! {
    static CACHE: RefCell<FileCache> = RefCell::new(FileCache {
        files: HashMap::new(),
        bytes: 0,
    });
}

struct FileCache {
    files: HashMap<String, Cached>,
    bytes: usize,
}

// Read a file (following symlinks, like fs.stat), from memory when it hasn't changed since last time
pub(crate) fn read(path: impl AsRef<Path>) -> io::Result<File> {
    let path = path.as_ref();
    let key = path.to_string_lossy();

    // We checked this one on the disk a moment ago
    let recently_checked = CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .files
            .get(key.as_ref())
            .filter(|cached| cached.checked_at.elapsed() < CHECK_INTERVAL)
            .map(|cached| File::Content(cached.content.clone(), cached.version))
    });
    if let Some(file) = recently_checked {
        return Ok(file);
    }

    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Ok(File::NotAFile);
    }
    if metadata.len() > MAX_CACHED_FILE_SIZE {
        return Ok(File::TooBig(metadata.len()));
    }

    let version = Version::of(&metadata);
    let cached = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let cached = cache
            .files
            .get_mut(key.as_ref())
            .filter(|cached| cached.version == version && version.settled())?;
        cached.checked_at = Instant::now();
        Some(cached.content.clone())
    });
    if let Some(content) = cached {
        return Ok(File::Content(content, version));
    }

    let content = Bytes::from(std::fs::read(path)?);
    // Only keep it if it didn't change while we were reading it
    let unchanged = std::fs::metadata(path).is_ok_and(|after| Version::of(&after) == version);
    if unchanged && version.settled() {
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.bytes + content.len() > MAX_CACHED_BYTES {
                cache.files.clear();
                cache.bytes = 0;
            }
            cache.bytes += content.len();
            if let Some(previous) = cache.files.insert(
                key.into_owned(),
                Cached {
                    content: content.clone(),
                    version,
                    checked_at: Instant::now(),
                },
            ) {
                cache.bytes -= previous.content.len();
            }
        });
    }
    Ok(File::Content(content, version))
}

// The copies of a file that were compressed ahead of time, by cpeak::precompress(): app.js.br
// and app.js.gz, right next to app.js. Sending one of those takes no compressing at all.
//
// A copy only counts while it has the very same modification time as its file, which is what
// precompress() gives it. With any other time it was made from another version of the file
// (maybe an older one that was put back since), so we leave it alone.
#[derive(Clone, Copy, Default)]
pub(crate) struct Copies {
    pub(crate) br: bool,
    pub(crate) gzip: bool,
}

// How many files a worker thread remembers that for. When it's full, we start over.
const MAX_REMEMBERED_COPIES: usize = 10_000;

thread_local! {
    // Which copies each file has, and when we last looked on the disk
    static COPIES: RefCell<HashMap<String, (Copies, Instant)>> = RefCell::new(HashMap::new());
}

// Which copies of the file count right now. Like the files themselves, they're looked for on
// the disk again when it's been more than CHECK_INTERVAL.
pub(crate) fn precompressed(path: &str) -> Copies {
    let recently_checked = COPIES.with(|copies| {
        copies
            .borrow()
            .get(path)
            .filter(|(_, checked_at)| checked_at.elapsed() < CHECK_INTERVAL)
            .map(|(copies, _)| *copies)
    });
    if let Some(copies) = recently_checked {
        return copies;
    }

    let modified = |path: &str| {
        let metadata = std::fs::metadata(path).ok()?;
        metadata.modified().ok().filter(|_| metadata.is_file())
    };
    let file = modified(path);
    let counts =
        |extension: &str| file.is_some() && modified(&format!("{path}.{extension}")) == file;
    let copies = Copies {
        br: counts("br"),
        gzip: counts("gz"),
    };

    COPIES.with(|remembered| {
        let mut remembered = remembered.borrow_mut();
        if remembered.len() >= MAX_REMEMBERED_COPIES {
            remembered.clear();
        }
        remembered.insert(path.to_string(), (copies, Instant::now()));
    });
    copies
}
