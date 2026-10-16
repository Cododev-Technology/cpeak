// Compressing files ahead of time with precompress(), and sending those copies
mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{request, start};
use cpeak::{cpeak, precompress, serve_static, ServeStaticOptions};

// A folder of its own for every test, with a script, a page and an image in it
fn folder(name: &str) -> PathBuf {
    let folder =
        std::env::temp_dir().join(format!("cpeak-precompress-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(folder.join("pages")).unwrap();

    std::fs::write(folder.join("app.js"), script()).unwrap();
    std::fs::write(
        folder.join("pages/about.html"),
        "<p>About us</p>".repeat(200),
    )
    .unwrap();
    std::fs::write(
        folder.join("logo.png"),
        [137, 80, 78, 71, 13, 10, 26, 10].repeat(500),
    )
    .unwrap();
    // Too small for a copy of it to be any smaller
    std::fs::write(folder.join("tiny.txt"), "hi").unwrap();
    folder
}

fn script() -> String {
    (0..2000)
        .map(|i| {
            format!("export function handler{i}(req, res) {{ return res.json({{ id: {i} }}); }}\n")
        })
        .collect()
}

fn path(folder: &Path) -> &str {
    folder.to_str().unwrap()
}

fn modified(file: PathBuf) -> std::time::SystemTime {
    std::fs::metadata(file).unwrap().modified().unwrap()
}

fn unbrotli(data: &[u8]) -> String {
    let mut text = Vec::new();
    brotli::BrotliDecompress(&mut &data[..], &mut text).unwrap();
    String::from_utf8(text).unwrap()
}

fn gunzip(data: &[u8]) -> String {
    let mut text = String::new();
    flate2::read::GzDecoder::new(data)
        .read_to_string(&mut text)
        .unwrap();
    text
}

// A server that serves the folder. Compression isn't turned on: the copies are all there is.
fn serve(folder: &Path, options: impl Into<Option<ServeStaticOptions>>) -> u16 {
    let folder = path(folder).to_string();
    let options = options.into();
    start(move || {
        let mut server = cpeak();
        server.before_each(serve_static(&folder, options));
        server
    })
}

#[test]
fn should_write_a_br_and_a_gz_copy_next_to_every_file_that_gets_compressed() {
    let folder = folder("copies");

    // app.js and pages/about.html, two copies each
    assert_eq!(precompress(path(&folder)).unwrap(), 4);

    let br = std::fs::read(folder.join("app.js.br")).unwrap();
    let gz = std::fs::read(folder.join("app.js.gz")).unwrap();
    assert_eq!(unbrotli(&br), script());
    assert_eq!(gunzip(&gz), script());
    assert!(br.len() < gz.len() && gz.len() < script().len() / 5);
    assert!(folder.join("pages/about.html.br").exists());
    assert!(folder.join("pages/about.html.gz").exists());

    // Images are compressed already, and a copy has to be smaller than its file
    assert!(!folder.join("logo.png.br").exists());
    assert!(!folder.join("tiny.txt.br").exists());
    assert!(!folder.join("tiny.txt.gz").exists());
}

#[test]
fn should_give_the_copies_the_modification_time_of_their_file() {
    let folder = folder("times");
    precompress(path(&folder)).unwrap();

    let file = modified(folder.join("app.js"));
    assert_eq!(modified(folder.join("app.js.br")), file);
    assert_eq!(modified(folder.join("app.js.gz")), file);
}

#[test]
fn should_only_compress_the_files_that_changed_when_it_runs_again() {
    let folder = folder("again");
    assert_eq!(precompress(path(&folder)).unwrap(), 4);
    assert_eq!(precompress(path(&folder)).unwrap(), 0);

    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        folder.join("app.js"),
        "console.log('changed');\n".repeat(100),
    )
    .unwrap();
    assert_eq!(precompress(path(&folder)).unwrap(), 2);
    let br = std::fs::read(folder.join("app.js.br")).unwrap();
    assert!(unbrotli(&br).starts_with("console.log('changed');"));
}

#[test]
fn should_send_the_br_copy_as_it_is_when_the_client_accepts_brotli() {
    let folder = folder("br");
    precompress(path(&folder)).unwrap();
    let port = serve(&folder, None);

    let res = request(port, "GET", "/app.js")
        .header("Accept-Encoding", "gzip, deflate, br, zstd")
        .send();

    assert_eq!(res.status, 200);
    assert_eq!(res.header("content-type"), Some("application/javascript"));
    assert_eq!(res.header("content-encoding"), Some("br"));
    assert_eq!(res.header("vary"), Some("Accept-Encoding"));
    // The very bytes of the copy on the disk
    let copy = std::fs::read(folder.join("app.js.br")).unwrap();
    assert_eq!(
        res.header("content-length"),
        Some(copy.len().to_string().as_str())
    );
    assert!(res.body == copy);
    assert_eq!(unbrotli(&res.body), script());
}

#[test]
fn should_send_the_gz_copy_when_the_client_only_accepts_gzip() {
    let folder = folder("gz");
    precompress(path(&folder)).unwrap();
    let port = serve(&folder, None);

    let res = request(port, "GET", "/pages/about.html")
        .header("Accept-Encoding", "gzip")
        .send();

    assert_eq!(res.header("content-type"), Some("text/html"));
    assert_eq!(res.header("content-encoding"), Some("gzip"));
    assert!(res.body == std::fs::read(folder.join("pages/about.html.gz")).unwrap());
    assert_eq!(gunzip(&res.body), "<p>About us</p>".repeat(200));
}

#[test]
fn should_send_the_file_itself_when_the_client_accepts_neither_copy() {
    let folder = folder("neither");
    precompress(path(&folder)).unwrap();
    let port = serve(&folder, None);

    let plain = request(port, "GET", "/app.js").send();
    assert_eq!(plain.header("content-encoding"), None);
    assert_eq!(plain.text(), script());
    // Another client would have gotten another response
    assert_eq!(plain.header("vary"), Some("Accept-Encoding"));

    let refused = request(port, "GET", "/app.js")
        .header("Accept-Encoding", "br;q=0, gzip;q=0")
        .send();
    assert_eq!(refused.header("content-encoding"), None);
    assert_eq!(refused.text(), script());

    // A file without copies is sent the way it always was
    let image = request(port, "GET", "/logo.png")
        .header("Accept-Encoding", "br")
        .send();
    assert_eq!(image.header("content-encoding"), None);
    assert_eq!(image.header("vary"), None);
    assert_eq!(image.body.len(), 4000);
}

#[test]
fn should_leave_a_copy_alone_that_has_another_modification_time_than_its_file() {
    let folder = folder("stale");
    precompress(path(&folder)).unwrap();

    // The file was put back the way it was at some earlier date: its copy is "newer", and wrong
    let file = std::fs::File::options()
        .write(true)
        .open(folder.join("app.js"))
        .unwrap();
    file.set_modified(modified(folder.join("app.js")) - Duration::from_secs(3600))
        .unwrap();
    let port = serve(&folder, None);

    let res = request(port, "GET", "/app.js")
        .header("Accept-Encoding", "br, gzip")
        .send();
    assert_eq!(res.header("content-encoding"), None);
    assert_eq!(res.text(), script());

    // The copies of the other file still count
    let page = request(port, "GET", "/pages/about.html")
        .header("Accept-Encoding", "br, gzip")
        .send();
    assert_eq!(page.header("content-encoding"), Some("br"));
}

#[test]
fn should_send_the_copies_from_res_send_file_too() {
    let folder = folder("send-file");
    precompress(path(&folder)).unwrap();
    let file = folder.join("app.js").to_str().unwrap().to_string();
    let port = start(move || {
        let mut server = cpeak();
        server.route("get", "/script", async move |_req, res| {
            res.send_file(&file, "application/javascript").await
        });
        server
    });

    let res = request(port, "GET", "/script")
        .header("Accept-Encoding", "br")
        .send();
    assert_eq!(res.header("content-encoding"), Some("br"));
    assert_eq!(unbrotli(&res.body), script());

    // The response to a HEAD request is never compressed
    let head = request(port, "HEAD", "/script")
        .header("Accept-Encoding", "br")
        .send();
    assert_eq!(head.header("content-encoding"), None);
}

#[test]
fn should_not_look_for_copies_when_precompressed_is_turned_off() {
    let folder = folder("off");
    precompress(path(&folder)).unwrap();
    let port = serve(
        &folder,
        ServeStaticOptions {
            precompressed: false,
            ..Default::default()
        },
    );

    let res = request(port, "GET", "/app.js")
        .header("Accept-Encoding", "br, gzip")
        .send();
    assert_eq!(res.header("content-encoding"), None);
    assert_eq!(res.text(), script());
}
