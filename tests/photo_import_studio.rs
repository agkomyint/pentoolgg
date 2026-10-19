//! Studio's Photo workspace on a project that has no photos yet: the catalog
//! lists nothing (it is not an error) and a plain JPEG or PNG imports.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn start(doc: PathBuf) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let _ = pentool::server::serve_listener(listener, Some(doc), false, None).await;
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn jpeg() -> Vec<u8> {
    let pixels =
        image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8 * 30, y as u8 * 40, 90]));
    let mut out = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

#[test]
fn a_project_without_photos_lists_none_and_imports_a_jpeg() {
    let dir = std::env::temp_dir().join(format!("pentool-photo-first-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let ws = Workspace(dir);
    let doc = ws.0.join("project.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let url = start(doc.clone());
    let client = reqwest::blocking::Client::new();

    let response = client
        .get(format!("{url}/api/photo/catalog?query=&limit=200"))
        .send()
        .unwrap();
    let status = response.status().as_u16();
    let catalog: Value = serde_json::from_str(&response.text().unwrap()).unwrap();
    assert_eq!(status, 200, "{catalog}");
    assert_eq!(catalog["matches"], 0, "{catalog}");
    assert_eq!(catalog["photos"], serde_json::json!([]));
    let revision = catalog["revision"].as_str().unwrap().to_owned();

    let response = client
        .post(format!("{url}/api/photo/import?id=beach&name=beach.jpg"))
        .header("content-type", "application/octet-stream")
        .header("x-pentool-revision", revision)
        .body(jpeg())
        .send()
        .unwrap();
    let status = response.status().as_u16();
    let imported: Value = serde_json::from_str(&response.text().unwrap()).unwrap();
    assert_eq!(status, 200, "{imported}");
    assert_eq!(imported["result"]["photo"], "beach");

    let catalog: Value = serde_json::from_str(
        &client
            .get(format!("{url}/api/photo/catalog?query=&limit=200"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(catalog["matches"], 1, "{catalog}");
}
