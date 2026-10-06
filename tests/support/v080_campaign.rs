use anyhow::Result;
use pentool::{composite, image, mask, scene, transform};
use serde_json::{json, Value};
use std::path::Path;

pub fn build(document: &Path) -> Result<Value> {
    let mut raw = composite::migrate(scene::new_document(480, 640))?;
    raw["name"] = json!("Orbital perspectives · compositing acceptance demo");
    raw["pages"][0]["id"] = json!("poster");
    raw["pages"][0]["canvas"]["background"] = json!("#080e19");
    let root = document.parent().unwrap();
    for (id, file, x, y, w, h) in [
        ("earth", "earth.jpg", 64.0, 100.0, 352.0, 352.0),
        ("sunrise", "sunrise.jpg", 0.0, 60.0, 480.0, 440.0),
        ("headline-texture", "horizon.jpg", 30.0, 460.0, 420.0, 85.0),
    ] {
        let bytes = std::fs::read(root.join("sources").join(file))?;
        image::add(
            &mut raw,
            Some("poster"),
            "layer-1",
            id,
            &bytes,
            image::external_storage(Path::new(&format!("sources/{file}")))?,
            x,
            y,
            w,
            h,
            image::Fit::Cover,
        )?;
    }
    let images = raw["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .clone();
    let gradient = json!({"kind":"linear","x1":0,"y1":0,"x2":480,"y2":640,"space":"linear","stops":[{"position":0,"color":"#101d37"},{"position":1,"color":"#080e19"}]});
    raw["styles"]["campaign-gradient"] = json!({"type":"fill","value":gradient});
    raw["styles"]["campaign-shadow"] = json!({"type":"effects","value":[{"id":"shadow","kind":"drop-shadow","enabled":true,"opacity":0.55,"blend_mode":"normal","params":{"x":0,"y":10,"blur":12,"color":"#000000"}}]});
    let mut sunrise = images[1].clone();
    sunrise["blend_mode"] = json!("screen");
    sunrise["opacity"] = json!(0.42);
    let mut texture = images[2].clone();
    texture["clipping"] = json!({"base":"headline"});
    texture["blend_mode"] = json!("screen");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"fill","id":"backdrop","x":0,"y":0,"width":480,"height":640,"fill":{"ref":"campaign-gradient","fallback":gradient}},
        {"kind":"group","id":"hero","isolation":"isolated","children":[images[0],sunrise]},
        {"kind":"adjustment","id":"shared-curves","adjustment":"curves","params":{"points":[[0,0],[64,48],[192,208],[255,255]]},"scope":{"kind":"group","id":"hero"},"enabled":true,"opacity":0.7,"blend_mode":"normal"},
        {"kind":"adjustment","id":"global-grade","adjustment":"vibrance","params":{"amount":15},"scope":{"kind":"below"},"enabled":true,"opacity":0.8,"blend_mode":"normal"},
        {"kind":"text","id":"eyebrow","content":"ORBITAL PERSPECTIVES / 01","x":32,"y":54,"font_family":"Atkinson Hyperlegible","font_size":14,"font_weight":700,"style":{"fill":{"fallback":"#87baff"}}},
        {"kind":"text","id":"headline","content":"EARTH","x":30,"y":535,"font_family":"Atkinson Hyperlegible","font_size":108,"font_weight":700,"style":{"fill":{"fallback":"#5790e3"}},"effects":{"ref":"campaign-shadow","fallback":[]}},
        texture,
        {"kind":"text","id":"caption","content":"A DIFFERENT POINT OF VIEW\nSOURCE PIXELS INTACT / TWO-PAGE STUDY","x":32,"y":581,"font_family":"Atkinson Hyperlegible","font_size":13,"font_weight":400,"style":{"fill":{"fallback":"#c2cde0"}}}
    ]);
    // Snapshot the vector frame, then reuse its immutable resource through feathered attachments.
    let frame = json!({"kind":"ellipse","id":"mask-frame","cx":240,"cy":270,"radius_x":190,"radius_y":190,"style":{"fill":{"fallback":"#ffffff"}}});
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(frame);
    mask::create(
        &mut raw,
        document,
        Some("poster"),
        "hero-mask",
        "vector:mask-frame",
    )?;
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .pop();
    mask::attach(
        &mut raw,
        Some("poster"),
        "shared-curves",
        "hero-mask",
        &json!({"feather":8,"linked":false}),
    )?;
    mask::attach(
        &mut raw,
        Some("poster"),
        "hero",
        "hero-mask",
        &json!({"feather":4}),
    )?;
    let mut screen_content = images[0].clone();
    screen_content["id"] = json!("screen-photo");
    screen_content["x"] = json!(120);
    screen_content["y"] = json!(65);
    screen_content["width"] = json!(400);
    screen_content["height"] = json!(280);
    raw["pages"].as_array_mut().unwrap().push(json!({"id":"screen","name":"Perspective screen study","canvas":{"width":640,"height":420,"background":"#080e19"},"layers":[{"id":"layer-1","name":"Content","visible":true,"locked":false,"nodes":[
        {"kind":"fill","id":"screen-backdrop","x":0,"y":0,"width":640,"height":420,"fill":{"ref":"campaign-gradient","fallback":gradient}},
        {"kind":"group","id":"screen-design","isolation":"isolated","effects":{"ref":"campaign-shadow","fallback":[]},"children":[{"kind":"rect","id":"screen-bezel","x":110,"y":55,"width":420,"height":300,"style":{"fill":{"fallback":"#303a4f"}}},screen_content,{"kind":"text","id":"screen-title","content":"EARTH / 01","x":144,"y":322,"font_family":"Atkinson Hyperlegible","font_size":32,"font_weight":700,"style":{"fill":{"fallback":"#ffffff"}}}]},
        {"kind":"text","id":"screen-caption","content":"ONE SOURCE / REVERSIBLE PLACEMENT","x":32,"y":390,"font_family":"Atkinson Hyperlegible","font_size":14,"font_weight":400,"style":{"fill":{"fallback":"#c2cde0"}}}
    ]}]}));
    transform::edit(
        &mut raw,
        Some("screen"),
        "screen-design",
        "add",
        "screen-placement",
        Some("perspective"),
        &json!({"quad":[[105,80],[530,45],[505,340],[140,360]]}),
        None,
    )?;
    scene::validate(&raw)?;
    Ok(raw)
}
