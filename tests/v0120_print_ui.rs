//! Print preflight UI: the panel ships in the binary and its read-only endpoint
//! reports the shared document without writing it.
use serde_json::{json, Value};

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn print_panel_and_preflight_endpoint_are_served_read_only() {
    let dir = std::env::temp_dir().join(format!("pentool-printui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":9});
    let bytes = serde_json::to_vec(&raw).unwrap();
    std::fs::write(dir.join("doc.pen"), &bytes).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", "doc.pen", "--port", &port.to_string()])
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    assert!(
        (0..100).any(|_| {
            let up = client.get(format!("{base}/api/health")).send().is_ok();
            if !up {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            up
        }),
        "server did not start"
    );
    let page = client
        .get(format!("{base}/classic"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    let root = client.get(&base).send().unwrap().text().unwrap();
    assert!(root.contains("Pentool Studio") && root.contains("id=\"root\""));
    assert!(page.contains("id=\"printPanel\"") && page.contains("/print-panel.js"));
    let studio = client
        .get(format!("{base}/studio"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(studio.contains("Pentool Studio") && studio.contains("id=\"root\""));
    assert!(page.contains("id=\"printPlates\"") && page.contains("id=\"printProof\""));
    assert!(page.contains("id=\"printGuides\"") && page.contains("id=\"printGuidesToggle\""));
    assert!(page.contains("id=\"printOverprintToggle\"") && page.contains("id=\"printGamut\""));
    assert!(client
        .get(format!("{base}/print-panel.js"))
        .send()
        .unwrap()
        .status()
        .is_success());
    let response = client
        .get(format!("{base}/api/color/preflight?min_ppi=200"))
        .send()
        .unwrap();
    assert!(response.status().is_success());
    let report: Value = serde_json::from_str(&response.text().unwrap()).unwrap();
    assert_eq!(report["scope"], "print-preflight");
    assert_eq!(report["min_ppi"], 200.0);
    assert_eq!(report["production"]["trim"]["width"], 160.0);
    // No output role is configured, so the report must block production.
    assert_eq!(report["ok"], false);
    assert!(report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "output-role-missing"));
    assert_eq!(std::fs::read(dir.join("doc.pen")).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plate_and_proof_views_are_served_read_only() {
    use pentool::color::catalog;
    let dir = std::env::temp_dir().join(format!("pentool-printview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let document = dir.join("doc.pen");
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    catalog::import(
        &mut raw,
        &document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"paint","x":0,"y":0,
        "width":200,"height":100,
        "style":{"fill":{"space":"icc-cmyk","profile":"press","channels":[0.0,0.8,0.9,0.0]}}}]);
    let bytes = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&document, &bytes).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", "doc.pen", "--port", &port.to_string()])
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    assert!((0..100).any(|_| {
        let up = client.get(format!("{base}/api/health")).send().is_ok();
        if !up {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        up
    }));
    let listing: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/color/plates"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = listing["plates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["plate"].as_str().unwrap())
        .collect();
    assert_eq!(&ids[..4], ["C", "M", "Y", "K"]);
    let magenta = listing["plates"][1]["average_coverage_percent"]
        .as_f64()
        .unwrap();
    assert!(magenta > 1.0, "{listing}");
    let plate = client
        .get(format!("{base}/api/color/plates?plate=M"))
        .send()
        .unwrap();
    assert_eq!(plate.headers()["content-type"], "image/png");
    assert_eq!(&plate.bytes().unwrap()[1..4], b"PNG");
    let missing = client
        .get(format!("{base}/api/color/plates?plate=Z"))
        .send()
        .unwrap();
    assert_eq!(missing.status(), 400);
    let proof = client
        .get(format!("{base}/api/color/proof"))
        .send()
        .unwrap();
    assert_eq!(proof.headers()["content-type"], "image/png");
    assert_eq!(&proof.bytes().unwrap()[1..4], b"PNG");
    let gamut = client
        .get(format!("{base}/api/color/gamut"))
        .send()
        .unwrap();
    assert_eq!(gamut.headers()["content-type"], "image/png");
    let by_intent = client
        .get(format!("{base}/api/color/proof?intent=perceptual"))
        .send()
        .unwrap();
    assert_eq!(by_intent.status(), 200);
    let bad_intent = client
        .get(format!("{base}/api/color/proof?intent=sparkly"))
        .send()
        .unwrap();
    assert_eq!(bad_intent.status(), 400);
    assert_eq!(&gamut.bytes().unwrap()[1..4], b"PNG");
    assert_eq!(std::fs::read(&document).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(&dir);
}
