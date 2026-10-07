//! v0.10.0 item 2: raster layer operations and integration with the scene graph.
use pentool::{composite, diff, mask, raster, scene};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn document() -> Value {
    let mut raw = composite::migrate(scene::new_document(320, 240)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("#ffffff");
    raw
}

fn stroke(raw: &mut Value, page: Option<&str>, id: &str, color: &str, samples: Value) {
    raster::paint(
        raw,
        page,
        id,
        raster::StrokeRequest {
            brush: raster::Brush::parse(
                &json!({"kind":"soft-round","size":40,"hardness":0.5,"flow":0.8}),
            )
            .unwrap(),
            samples: raster::parse_samples(&samples).unwrap(),
            color: raster::parse_color(color).unwrap(),
            blend: raster::Blend::parse("normal").unwrap(),
            seed: 3,
        },
    )
    .unwrap();
}

fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new("target/raster-layers.pen"), None, 1.0).unwrap()
}

fn max_channel_diff(a: &image::RgbaImage, b: &image::RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..4).map(move |c| p[c].abs_diff(q[c])))
        .max()
        .unwrap_or(0)
}

fn nodes(raw: &Value) -> &Vec<Value> {
    raw["pages"][0]["layers"][0]["nodes"].as_array().unwrap()
}

fn node<'a>(raw: &'a Value, id: &str) -> &'a Value {
    nodes(raw).iter().find(|n| n["id"] == id).unwrap()
}

/// Two overlapping painted layers: `low` at (0,0) and `high` offset so the union
/// is larger than either.
fn two_layers() -> Value {
    let mut raw = document();
    raster::add(&mut raw, None, None, "low", 0.0, 0.0, 200, 150).unwrap();
    raster::add(&mut raw, None, None, "high", 60.0, 40.0, 220, 180).unwrap();
    stroke(
        &mut raw,
        None,
        "low",
        "#204060",
        json!([[10, 20, 1], [190, 130, 1]]),
    );
    stroke(
        &mut raw,
        None,
        "high",
        "#D2A184",
        json!([[5, 170, 1], [200, 10, 0.6]]),
    );
    raw
}

#[test]
fn merge_down_keeps_the_rendered_page_and_grows_to_the_union() {
    let mut raw = two_layers();
    let before = render(&raw);
    let result = raster::merge_down(&mut raw, None, "high").unwrap();
    assert_eq!(result["id"], "low");
    assert!(nodes(&raw).iter().all(|n| n["id"] != "high"));
    let low = node(&raw, "low");
    assert_eq!(
        (low["x"].as_f64(), low["y"].as_f64()),
        (Some(0.0), Some(0.0))
    );
    assert!(low["width"].as_u64().unwrap() > 200 && low["height"].as_u64().unwrap() > 150);
    assert_eq!(low["journal"], json!([]));
    assert_eq!(low["checkpoint"]["reason"], "merge-down");
    // Source-over is associative in exact arithmetic, but the merged layer is
    // stored as 8-bit straight alpha before it meets the page background, so
    // soft edges may round differently by one code value.
    assert!(max_channel_diff(&before, &render(&raw)) <= 1);
    raster::verify(&raw, None, None, true)
        .map(|r| assert_eq!(r["ok"], true, "{r}"))
        .unwrap();
}

#[test]
fn merge_down_applies_opacity_and_blend_mode_of_the_upper_layer() {
    let mut raw = two_layers();
    let high = raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == "high")
        .unwrap();
    high["opacity"] = json!(0.6);
    high["blend_mode"] = json!("multiply");
    let before = render(&raw);
    raster::merge_down(&mut raw, None, "high").unwrap();
    // Same 8-bit rounding allowance as the normal-mode merge.
    assert!(max_channel_diff(&before, &render(&raw)) <= 1);
}

#[test]
fn merge_down_refuses_what_it_cannot_merge_exactly() {
    let mut raw = two_layers();
    let original = raw.clone();
    assert!(raster::merge_down(&mut raw, None, "low")
        .unwrap_err()
        .to_string()
        .contains("bottom node"));
    raw["pages"][0]["layers"][0]["nodes"][1]["transform"] = json!([2, 0, 0, 1, 0, 0]);
    let error = raster::merge_down(&mut raw, None, "high")
        .unwrap_err()
        .to_string();
    assert!(error.contains("rasterize it first"), "{error}");
    raw = original.clone();
    raw["pages"][0]["layers"][0]["nodes"][1]["x"] = json!(60.5);
    assert!(raster::merge_down(&mut raw, None, "high")
        .unwrap_err()
        .to_string()
        .contains("fractional"));
    raw = original.clone();
    raw["pages"][0]["layers"][0]["nodes"][1]["effects"] = json!([{"id":"s","kind":"drop-shadow","enabled":true,"opacity":1,"blend_mode":"normal","params":{"x":2,"y":0,"blur":0,"color":"#ff0000"}}]);
    let snapshot = raw.clone();
    assert!(raster::merge_down(&mut raw, None, "high")
        .unwrap_err()
        .to_string()
        .contains("effects"));
    assert_eq!(raw, snapshot, "failed merge must not mutate");
    raw = original;
    raw["pages"][0]["layers"][0]["nodes"][0]["opacity"] = json!(0.5);
    assert!(raster::merge_down(&mut raw, None, "high")
        .unwrap_err()
        .to_string()
        .contains("opacity"));
}

#[test]
fn crop_trim_resize_and_duplicate_keep_pixels_where_they_belong() {
    let mut raw = two_layers();
    let before = render(&raw);

    // Growing the canvas by 30px on the left keeps every pixel in place.
    raster::crop(&mut raw, None, "low", [-30, 0, 260, 150]).unwrap();
    assert_eq!(node(&raw, "low")["x"].as_f64(), Some(-30.0));
    assert_eq!(max_channel_diff(&before, &render(&raw)), 0);

    // Trim shrinks back to painted pixels without changing the render.
    let trimmed = raster::trim(&mut raw, None, "low").unwrap();
    assert_eq!(trimmed["pixels_removed"], 0);
    assert!(node(&raw, "low")["width"].as_u64().unwrap() < 260);
    assert_eq!(max_channel_diff(&before, &render(&raw)), 0);

    // A crop that cuts pixels reports them.
    let mut cut = raw.clone();
    assert!(
        raster::crop(&mut cut, None, "low", [0, 0, 20, 20]).unwrap()["pixels_removed"]
            .as_u64()
            .unwrap()
            > 0
    );

    // Duplicate shares tiles and sits directly above the source.
    let tiles = node(&raw, "low")["tiles"].clone();
    let store = raw["raster_tiles"].as_object().unwrap().len();
    raster::duplicate(&mut raw, None, "low", "low-copy").unwrap();
    let index = nodes(&raw).iter().position(|n| n["id"] == "low").unwrap();
    assert_eq!(nodes(&raw)[index + 1]["id"], "low-copy");
    assert_eq!(nodes(&raw)[index + 1]["tiles"], tiles);
    assert_eq!(raw["raster_tiles"].as_object().unwrap().len(), store);
    assert!(raster::duplicate(&mut raw, None, "low", "high")
        .unwrap_err()
        .to_string()
        .contains("already used"));

    // Nearest 2x then 0.5x is lossless; bilinear is deterministic.
    let mut scaled = raw.clone();
    let hash = raster::info(&scaled, None, "high").unwrap()["tile_map_sha256"].clone();
    raster::resize(
        &mut scaled,
        None,
        "high",
        440,
        360,
        raster::Resample::Nearest,
    )
    .unwrap();
    raster::resize(
        &mut scaled,
        None,
        "high",
        220,
        180,
        raster::Resample::Nearest,
    )
    .unwrap();
    assert_eq!(
        raster::info(&scaled, None, "high").unwrap()["tile_map_sha256"],
        hash
    );
    let (mut a, mut b) = (raw.clone(), raw.clone());
    let ra = raster::resize(&mut a, None, "high", 101, 77, raster::Resample::Bilinear).unwrap();
    let rb = raster::resize(&mut b, None, "high", 101, 77, raster::Resample::Bilinear).unwrap();
    assert_eq!(ra["tile_map_sha256"], rb["tile_map_sha256"]);
    assert!(raster::resize(&mut a, None, "high", 0, 10, raster::Resample::Bilinear).is_err());
    raster::verify(&a, None, None, true)
        .map(|r| assert_eq!(r["ok"], true, "{r}"))
        .unwrap();
}

#[test]
fn rasterize_bakes_a_vector_group_and_hides_the_source() {
    let mut raw = document();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"group","id":"badge","children":[
            {"kind":"rect","id":"plate","x":20,"y":30,"width":120,"height":60,"radius_x":12,"radius_y":12,"style":{"fill":{"fallback":"#1f6feb"}}},
            {"kind":"ellipse","id":"dot","cx":135,"cy":85,"radius_x":35,"radius_y":35,"style":{"fill":{"fallback":"#f2cc60"}},"opacity":0.7}
        ]}
    ]);
    let before = render(&raw);
    let result = raster::rasterize(
        &mut raw,
        Path::new("target/r.pen"),
        None,
        "badge",
        "badge-px",
        false,
    )
    .unwrap();
    assert_eq!(result["source_kept"], true);
    assert_eq!(nodes(&raw)[0]["visible"], false);
    assert_eq!(nodes(&raw)[1]["id"], "badge-px");
    assert_eq!(nodes(&raw)[1]["checkpoint"]["source"], "badge");
    assert!(max_channel_diff(&before, &render(&raw)) <= 1);

    // A child of a group can be baked too while the group is untransformed.
    let mut again = raw.clone();
    raster::rasterize(
        &mut again,
        Path::new("target/r.pen"),
        None,
        "plate",
        "plate-px",
        true,
    )
    .unwrap();
    let group = &nodes(&again)[0]["children"];
    assert_eq!(group[0]["id"], "plate-px");
    assert_eq!(group[0]["kind"], "raster");

    // Transformed parents and empty renders are refused without mutation.
    raw["pages"][0]["layers"][0]["nodes"][0]["transform"] = json!([1, 0, 0, 1, 5, 0]);
    let snapshot = raw.clone();
    let error = raster::rasterize(
        &mut raw,
        Path::new("target/r.pen"),
        None,
        "dot",
        "dot-px",
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("transformed group"), "{error}");
    assert_eq!(raw, snapshot);
}

#[test]
fn rasters_compose_inside_groups_with_effects_and_on_other_pages() {
    let mut raw = two_layers();
    // Move `high` into a translated group with a drop shadow on it.
    let high = raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"group","id":"g","transform":[1,0,0,1,10,0],"opacity":0.8,"children":[high]}));
    let plain = render(&raw);
    raw["pages"][0]["layers"][0]["nodes"][1]["children"][0]["effects"] = json!([{"id":"s","kind":"drop-shadow","enabled":true,"opacity":1,"blend_mode":"normal","params":{"x":4,"y":4,"blur":0,"color":"#ff0000"}}]);
    let shadowed = render(&raw);
    assert!(
        max_channel_diff(&plain, &shadowed) > 0,
        "effect must apply to raster"
    );
    // Painting inside a group addresses the node by id.
    stroke(
        &mut raw,
        None,
        "high",
        "#00ff00",
        json!([[20, 20, 1], [30, 30, 1]]),
    );

    // Second page: operations affect only the selected page.
    let mut page2 = raw["pages"][0].clone();
    page2["id"] = json!("page-2");
    page2["layers"][0]["id"] = json!("layer-2");
    page2["layers"][0]["nodes"] = json!([]);
    raw["pages"].as_array_mut().unwrap().push(page2);
    raster::add(&mut raw, Some("page-2"), None, "p2", 0.0, 0.0, 64, 64).unwrap();
    stroke(
        &mut raw,
        Some("page-2"),
        "p2",
        "#123456",
        json!([[10, 10, 1], [50, 50, 1]]),
    );
    let page_one = raw["pages"][0].clone();
    raster::crop(&mut raw, Some("page-2"), "p2", [0, 0, 32, 32]).unwrap();
    assert_eq!(raw["pages"][0], page_one);
    assert!(raster::crop(&mut raw, Some("page-2"), "low", [0, 0, 8, 8]).is_err());
    assert_eq!(raw["pages"][1]["layers"][0]["nodes"][0]["width"], 32);
    scene::validate(&raw).unwrap();
}

#[test]
fn rasters_respect_masks_and_clipping() {
    let mut raw = two_layers();
    let plain = render(&raw);
    // A clipped rect above `low` only shows where `low` has alpha.
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .insert(1, json!({"kind":"rect","id":"tint","x":0,"y":0,"width":320,"height":240,"style":{"fill":{"fallback":"#ff0000"}},"clipping":{"base":"low"}}));
    let clipped = render(&raw);
    assert_eq!(clipped.get_pixel(300, 230).0, plain.get_pixel(300, 230).0);
    assert_ne!(clipped, plain);
    // A shared vector mask hides `high` outside a small window.
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"rect","id":"window","x":200,"y":40,"width":60,"height":60,"style":{"fill":{"fallback":"#000000"}}}));
    let file = Path::new("target/raster-layers.pen");
    mask::create(&mut raw, file, None, "win", "vector:window").unwrap();
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .pop();
    mask::attach(&mut raw, None, "high", "win", &json!({})).unwrap();
    let masked = render(&raw);
    assert_ne!(masked, clipped);
    let mut no_high = raw.clone();
    no_high["pages"][0]["layers"][0]["nodes"][2]["visible"] = json!(false);
    assert_eq!(no_high["pages"][0]["layers"][0]["nodes"][2]["id"], "high");
    let hidden = render(&no_high);
    // Outside the window the masked raster contributes nothing.
    for (x, y) in [(150, 128), (120, 150), (280, 40)] {
        assert_eq!(
            masked.get_pixel(x, y).0,
            hidden.get_pixel(x, y).0,
            "({x},{y})"
        );
    }
    assert_ne!(masked, hidden);
}

#[test]
fn rasters_inside_components_keep_their_tiles() {
    let mut raw = document();
    raster::add(&mut raw, None, None, "paint", 10.0, 10.0, 100, 100).unwrap();
    stroke(
        &mut raw,
        None,
        "paint",
        "#336699",
        json!([[10, 10, 1], [90, 90, 1]]),
    );
    let painted = raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    raw["pages"][0]["layers"][0]["nodes"] =
        json!([{"kind":"group","id":"art","children":[painted]}]);
    let layer = raw["pages"][0]["layers"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = render(&raw);
    scene::promote_component(&mut raw, None, "art", "art-c").unwrap();
    scene::instantiate_component(&mut raw, None, &layer, "art-c", "inst", 0.0, 0.0).unwrap();
    // Delete the source group, then run an unrelated raster edit (which collects
    // garbage): the component snapshot and instance fallback still need the tiles.
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    raster::add(&mut raw, None, None, "other", 0.0, 0.0, 8, 8).unwrap();
    raster::clear(&mut raw, None, "other").unwrap();
    scene::validate(&raw).unwrap();
    assert_eq!(max_channel_diff(&original, &render(&raw)), 0);
}

#[test]
fn diff_summarizes_tile_store_changes_without_pixel_data() {
    let before = two_layers();
    let mut after = before.clone();
    stroke(
        &mut after,
        None,
        "low",
        "#ff0000",
        json!([[50, 50, 1], [60, 60, 1]]),
    );
    let changes = diff::structural(&before, &after);
    let tiles: Vec<_> = changes
        .iter()
        .filter(|c| c.path.starts_with("/raster_tiles/"))
        .collect();
    assert!(!tiles.is_empty());
    for change in &tiles {
        let text = serde_json::to_string(change).unwrap();
        assert!(text.contains("\"raster_tile\":true"), "{text}");
        assert!(!text.contains("\"data\""), "tile data leaked: {text}");
        assert!(text.len() < 300);
    }
}

// ---- CLI: dry-run, undo, atomic failure, packages ----

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-raster-layers-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

#[test]
fn every_layer_operation_supports_dry_run_undo_and_atomic_failure() {
    let ws = Workspace::new("cli");
    let doc = ws.path("a.pen");
    fs::write(&doc, serde_json::to_vec_pretty(&two_layers()).unwrap()).unwrap();
    let operations: [&[&str]; 6] = [
        &["resize", "high", "--width", "110", "--height", "90"],
        &[
            "crop", "low", "--x", "-10", "--y", "5", "--width", "150", "--height", "100",
        ],
        &["trim", "low"],
        &["duplicate", "low", "--new-id", "low-2"],
        &["merge-down", "high"],
        &["rasterize", "low", "--new-id", "low-px"],
    ];
    for operation in operations {
        let original = fs::read(&doc).unwrap();
        let mut args = vec!["raster", doc.as_str(), "--dry-run"];
        args.extend_from_slice(operation);
        let dry = ok(&args);
        assert_eq!(dry["dry_run"], true);
        assert_eq!(
            fs::read(&doc).unwrap(),
            original,
            "{operation:?} dry run wrote"
        );
        args.remove(2);
        let committed = ok(&args);
        assert_eq!(committed["result"], dry["result"], "{operation:?}");
        assert_ne!(fs::read(&doc).unwrap(), original);
        ok(&["undo", &doc]);
        assert_eq!(fs::read(&doc).unwrap(), original, "{operation:?} undo");
    }
    let original = fs::read(&doc).unwrap();
    for bad in [
        &["resize", "high", "--width", "0", "--height", "9"][..],
        &[
            "crop", "nope", "--x", "0", "--y", "0", "--width", "5", "--height", "5",
        ],
        &["merge-down", "low"],
        &["duplicate", "low", "--new-id", "high"],
        &["rasterize", "low", "--new-id", "low"],
        &[
            "resize",
            "high",
            "--width",
            "9",
            "--height",
            "9",
            "--resample",
            "cubic",
        ],
    ] {
        let mut args = vec!["raster", doc.as_str()];
        args.extend_from_slice(bad);
        assert!(!run(&args).status.success(), "{bad:?} should fail");
        assert_eq!(fs::read(&doc).unwrap(), original, "{bad:?} mutated");
    }
}

#[test]
fn raster_documents_pack_into_packages_with_their_tiles() {
    let ws = Workspace::new("pkg");
    let kit = ws.path("kit");
    ok(&["package", "init", &kit, "--name", "paint-kit"]);
    let mut raw = two_layers();
    raw["asset"] = json!({"schema":1,"id":"painted","name":"Painted","asset_version":"0.1.0","kind":"component"});
    let asset = Path::new(&kit).join("assets/painted.pen");
    fs::write(&asset, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let (a, b) = (ws.path("a.penpkg"), ws.path("b.penpkg"));
    ok(&["package", "pack", &kit, "--output", &a]);
    ok(&["package", "pack", &kit, "--output", &b]);
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    ok(&["package", "verify", &a]);
    let mut zip = zip::ZipArchive::new(fs::File::open(&a).unwrap()).unwrap();
    let name = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_owned())
        .find(|n| n.ends_with("painted.pen"))
        .unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut zip.by_name(&name).unwrap(), &mut text).unwrap();
    let packed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(packed["raster_tiles"], raw["raster_tiles"]);
    raster::verify(&packed, None, None, true)
        .map(|r| assert_eq!(r["ok"], true))
        .unwrap();
}
