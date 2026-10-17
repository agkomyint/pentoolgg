//! Studio design routes (`/api/design/*`) call the same functions as the CLI,
//! guard mutations with the document revision and never touch the template.
use serde_json::{json, Value};
use std::process::Command;

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn color(value: &str) -> Value {
    json!({"type":"color","value":value})
}

fn template_document(dir: &std::path::Path) -> Vec<u8> {
    let path = dir.join("post.pen");
    let status = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["new", &path.to_string_lossy()])
        .status()
        .unwrap();
    assert!(status.success());
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["styles"] = json!({"color.accent": color("#f59e0b")});
    doc["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"text","id":"head","x":10,"y":30,"content":"Title","font_size":24,
         "style":{"fill":{"fallback":"#111827"}}},
        {"kind":"rect","id":"bar","x":0,"y":0,"width":10,"height":10,"radius_x":0,"radius_y":0,
         "style":{"fill":{"ref":"color.accent","fallback":"#000000"}}},
        {"kind":"rect","id":"raw","x":20,"y":0,"width":10,"height":10,"radius_x":0,"radius_y":0,
         "style":{"fill":{"fallback":"#f59e0b"}}},
    ]);
    doc["design"] = json!({"template":{"schema":1,"name":"social/post","version":"1.0.0",
        "parameters":{"headline":{"type":"text","default":"Title",
            "targets":[{"object":"head","property":"content"}],
            "constraints":{"max_length":12,"required":true}}}}});
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    bytes
}

#[test]
fn design_routes_share_cli_logic_and_guard_fill() {
    let dir = std::env::temp_dir().join(format!("pentool-design-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = template_document(&dir);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", "post.pen", "--port", &port.to_string()])
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
    let get = |path: &str| -> (u16, Value) {
        let r = client.get(format!("{base}{path}")).send().unwrap();
        (
            r.status().as_u16(),
            serde_json::from_str(&r.text().unwrap()).unwrap(),
        )
    };

    let (status, tokens) = get("/api/design/tokens?resolve=true");
    assert_eq!(status, 200);
    assert_eq!(tokens["tokens"][0]["token"], "color.accent");
    assert_eq!(tokens["tokens"][0]["resolved"]["status"], "resolved");
    assert_eq!(tokens["returned"], 1);

    let (status, libs) = get("/api/design/library/status");
    assert_eq!(status, 200);
    assert_eq!(libs["libraries"], json!([]));

    let (status, audit) = get("/api/design/audit?rules=color&fix_plan=true");
    assert_eq!(status, 200);
    assert!(audit["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == "raw"));

    let (status, list) = get("/api/design/templates");
    assert_eq!(status, 200);
    assert_eq!(list["templates"][0]["name"], "social/post");
    let (status, check) = get("/api/design/template/check");
    assert_eq!(status, 200, "{check}");
    assert_eq!(check["parameter_count"], 1);

    // Paths cannot leave the project; errors keep the structured shape.
    let (status, bad) = get("/api/design/templates?path=..");
    assert_eq!(status, 400);
    assert_eq!(bad["error"]["code"], "invalid-input");

    let document: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/document"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    let revision = document["revision"].as_str().unwrap().to_string();
    let post = |body: Value| -> (u16, Value) {
        let r = client
            .post(format!("{base}/api/design/template/fill"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .unwrap();
        (
            r.status().as_u16(),
            serde_json::from_str(&r.text().unwrap()).unwrap(),
        )
    };

    // A missing or stale revision is refused before anything is written.
    let (status, stale) = post(json!({"values":{"headline":"Hi"},"output":"out.pen"}));
    assert_eq!(status, 400);
    assert_eq!(stale["error"]["code"], "conflict");
    let (status, _) = post(json!({"values":{"headline":"Hi"},"output":"out.pen","revision":"x"}));
    assert_eq!(status, 400);
    let (status, esc) = post(json!({"values":{},"output":"../out.pen","revision":revision}));
    assert_eq!(status, 400);
    assert_eq!(esc["error"]["code"], "invalid-input");
    assert!(!dir.join("out.pen").exists());

    let (status, dry) = post(json!({"values":{"headline":"Hi"},"output":"out.pen",
        "revision":revision,"dry_run":true}));
    assert_eq!(status, 200, "{dry}");
    assert_eq!(dry["dry_run"], true);
    assert!(!dir.join("out.pen").exists());

    let (status, done) = post(json!({"values":{"headline":"Hi"},"output":"out.pen",
        "revision":revision}));
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["output_revision"], dry["output_revision"]);
    let filled: Value =
        serde_json::from_slice(&std::fs::read(dir.join("out.pen")).unwrap()).unwrap();
    assert_eq!(filled["design"]["template"]["values"]["headline"], "Hi");
    assert_eq!(std::fs::read(dir.join("post.pen")).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(&dir);
}
