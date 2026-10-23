//! Deterministic folded meshes from rectangular packaging panel nets.
//!
//! This first solver supports rectangular, tree-shaped panel nets. Panels and
//! creases remain the single source of truth; the mesh is derived at render
//! time and never written back into the `.pen` document.

use super::math::{self, Mat4, Vec3, IDENTITY};
use super::model::{Instance, Material, Model, Primitive};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use kurbo::Shape;
use serde_json::Value;
use std::{collections::VecDeque, path::Path};

const MAX_FLAT_DIELINE_SVG_BYTES: usize = 64 * 1024 * 1024;
const MAX_FLAT_ARTWORK_PIXELS: u64 = 16_777_216;
const MAX_FLAT_BLEED_PIXELS: u64 = 16_777_216;
const TUBE_SEGMENTS: usize = 64;
pub const MAX_IMPORTED_DIELINE_SVG_BYTES: usize = 4 * 1024 * 1024;

/// Build a four-face rectangular sleeve net. This is a visual mockup template,
/// not a production dieline: it intentionally has no glue flap or end closures.
pub fn sleeve_dieline(
    length: f64,
    width: f64,
    height: f64,
    board: f64,
    bleed: f64,
) -> Result<Value> {
    for (name, value) in [("length", length), ("width", width), ("height", height)] {
        ensure!(
            value.is_finite() && (1.0..=100_000.0).contains(&value),
            "[invalid-input] sleeve {name} must be within 1..=100000 mm"
        );
    }
    ensure!(
        board.is_finite() && (0.0..=100.0).contains(&board),
        "[invalid-input] sleeve board must be within 0..=100 mm"
    );
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] sleeve bleed must be within 0..=100 mm"
    );

    let widths = [length, width, length, width];
    let ids = ["front", "right", "back", "left"];
    let mut x = 0.0;
    let mut panels = Vec::with_capacity(4);
    let mut creases = Vec::with_capacity(3);
    for (index, panel_width) in widths.into_iter().enumerate() {
        panels.push(serde_json::json!({
            "id": ids[index], "rect_mm": [x, 0.0, panel_width, height],
            "face_slot": ids[index], "uv_rect": [0.0, 0.0, 1.0, 1.0]
        }));
        x += panel_width;
        if index < 3 {
            creases.push(serde_json::json!({
                "id": format!("{}-fold", ["front-right", "right-back", "back-left"][index]),
                "line_mm": [[x, 0.0], [x, height]], "angle_deg": 90.0,
                "direction": "valley"
            }));
        }
    }
    let crease_path = [length, length + width, 2.0 * length + width]
        .iter()
        .map(|x| format!("M {x} 0 L {x} {height}"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(serde_json::json!({
        "template": "sleeve", "units": "mm",
        "dimensions_mm": {"length": length, "width": width, "height": height, "board": board, "bleed": bleed},
        "panels": panels, "creases": creases,
        "paths": [
            {"id":"sleeve-cut-outline","role":"cut","d":format!("M 0 0 L {x} 0 L {x} {height} L 0 {height} Z"),"spot":"CutContour"},
            {"id":"sleeve-creases","role":"crease","d":crease_path,"spot":"Crease"}
        ], "fold_state": 1.0
    }))
}

/// Build a visual stand-up-pouch net with a front, back, two gussets, and a
/// bottom gusset. This is a simplified box-bottom mockup, not a production bag
/// pattern: curved seals, zipper geometry, film thickness, and forming strain
/// are intentionally not represented.
pub fn pouch_dieline(length: f64, gusset: f64, height: f64, bleed: f64) -> Result<Value> {
    for (name, value) in [
        ("front width", length),
        ("gusset", gusset),
        ("height", height),
    ] {
        ensure!(
            value.is_finite() && (1.0..=100_000.0).contains(&value),
            "[invalid-input] stand-up pouch {name} must be within 1..=100000 mm"
        );
    }
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] stand-up pouch bleed must be within 0..=100 mm"
    );
    ensure!(
        gusset < length && gusset < height,
        "[invalid-input] stand-up pouch gusset must be smaller than its front width and height"
    );

    let x0 = 0.0;
    let x1 = gusset;
    let x2 = gusset + length;
    let x3 = x2 + gusset;
    let y0 = 0.0;
    let y1 = gusset;
    let y2 = y1 + height;
    let y3 = y2 + gusset;
    let y4 = y3 + height;
    let panels = [
        ("left-gusset", [x0, y1, gusset, height]),
        ("front", [x1, y1, length, height]),
        ("right-gusset", [x2, y1, gusset, height]),
        ("bottom-gusset", [x1, y2, length, gusset]),
        ("back", [x1, y3, length, height]),
    ];
    let panels = panels
        .into_iter()
        .map(|(id, rect)| {
            serde_json::json!({
                "id": id, "rect_mm": rect, "face_slot": id,
                "uv_rect": [0.0, 0.0, 1.0, 1.0]
            })
        })
        .collect::<Vec<_>>();
    let creases = [
        ("left-gusset-fold", [[x1, y1], [x1, y2]]),
        ("right-gusset-fold", [[x2, y1], [x2, y2]]),
        ("bottom-fold", [[x1, y2], [x2, y2]]),
        ("back-fold", [[x1, y3], [x2, y3]]),
    ];
    let creases_json = creases
        .iter()
        .map(|(id, line)| {
            serde_json::json!({
                "id": id, "line_mm": line, "angle_deg": 90.0, "direction": "valley"
            })
        })
        .collect::<Vec<_>>();
    let crease_path = creases
        .iter()
        .map(|(_, [[ax, ay], [bx, by]])| format!("M {ax} {ay} L {bx} {by}"))
        .collect::<Vec<_>>()
        .join(" ");
    let outline = format!(
        "M {x0} {y1} L {x1} {y1} L {x1} {y0} L {x2} {y0} L {x2} {y1} L {x3} {y1} L {x3} {y2} L {x2} {y2} L {x2} {y4} L {x1} {y4} L {x1} {y2} L {x0} {y2} Z"
    );
    Ok(serde_json::json!({
        "template": "pouch", "units": "mm",
        "dimensions_mm": {"length": length, "width": gusset, "height": height, "board": 0.0, "bleed": bleed},
        "panels": panels, "creases": creases_json,
        "paths": [
            {"id":"pouch-cut-outline","role":"cut","d":outline,"spot":"CutContour"},
            {"id":"pouch-creases","role":"crease","d":crease_path,"spot":"Crease"}
        ],
        "fold_state": 1.0
    }))
}

/// Build a flat, printable label panel whose width is the measured circumference
/// of a cylindrical tube. The stored wrap record is also used to derive its 3D
/// surface, so the seam and flat-to-surface mapping cannot drift apart.
pub fn tube_label_dieline(radius: f64, height: f64, bleed: f64) -> Result<Value> {
    tube_label_dieline_with_overlap(radius, height, bleed, 0.0)
}

/// Build a tube label with an explicit tail that overlaps the start of the
/// wrap in the folded preview.
pub fn tube_label_dieline_with_overlap(
    radius: f64,
    height: f64,
    bleed: f64,
    overlap_mm: f64,
) -> Result<Value> {
    ensure!(
        radius.is_finite() && (1.0..=100_000.0).contains(&radius),
        "[invalid-input] tube label radius must be within 1..=100000 mm"
    );
    ensure!(
        height.is_finite() && (1.0..=100_000.0).contains(&height),
        "[invalid-input] tube label height must be within 1..=100000 mm"
    );
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] tube label bleed must be within 0..=100 mm"
    );
    let circumference = std::f64::consts::TAU * radius;
    ensure!(
        overlap_mm.is_finite() && overlap_mm >= 0.0 && overlap_mm <= circumference * 0.25,
        "[invalid-input] tube label overlap must be within 0..=25% of the measured circumference"
    );
    let flat_width = circumference + overlap_mm;
    Ok(serde_json::json!({
        "template":"tube", "units":"mm",
        "dimensions_mm":{"length":2.0*radius,"width":2.0*radius,"height":height,"board":0.0,"bleed":bleed},
        "wrap":{"radius_mm":radius,"direction":"clockwise","seam_angle_deg":0.0,"overlap_mm":overlap_mm},
        "panels":[{"id":"tube-label","rect_mm":[0.0,0.0,flat_width,height],"face_slot":"label","uv_rect":[0.0,0.0,1.0,1.0]}],
        "creases":[],
        "paths":[{"id":"tube-label-cut-outline","role":"cut","d":format!("M 0 0 H {flat_width} V {height} H 0 Z"),"spot":"CutContour"}],
        "fold_state":1.0
    }))
}

/// Build a measured cylindrical bottle-label wrap. This maps accurately to the
/// chosen label-zone radius, but does not conform to a tapered or curved bottle
/// profile; use a cylinder-like label zone or a separately authored mesh.
pub fn bottle_label_dieline(
    radius: f64,
    height: f64,
    bleed: f64,
    overlap_mm: f64,
) -> Result<Value> {
    let mut dieline = tube_label_dieline_with_overlap(radius, height, bleed, overlap_mm)?;
    dieline["template"] = serde_json::json!("bottle-label");
    dieline["panels"][0]["id"] = serde_json::json!("bottle-label");
    dieline["paths"][0]["id"] = serde_json::json!("bottle-label-cut-outline");
    Ok(dieline)
}

/// Generate a rectangular tuck-end carton net with four body faces, a glue
/// joint, two tuck flaps, and four dust flaps. The fold solver offsets each
/// hinge axis by half the board thickness to keep the rendered outer surfaces
/// connected. Positive thickness adds inner faces, exposed cut-edge walls, and
/// an untextured interior bridge across each folded hinge wedge. This is visual
/// geometry; score compression and production allowances are not modeled.
pub fn tuck_carton_dieline(
    template: &str,
    length: f64,
    width: f64,
    height: f64,
    board: f64,
    bleed: f64,
    glue_flap: f64,
) -> Result<Value> {
    if template == "mailer" {
        return mailer_box_dieline(length, width, height, board, bleed, glue_flap);
    }
    ensure!(
        matches!(template, "straight-tuck" | "reverse-tuck"),
        "[invalid-input] carton template must be straight-tuck or reverse-tuck"
    );
    for (name, value) in [("length", length), ("width", width), ("height", height)] {
        ensure!(
            value.is_finite() && (1.0..=100_000.0).contains(&value),
            "[invalid-input] carton {name} must be within 1..=100000 mm"
        );
    }
    ensure!(
        board.is_finite() && (0.0..=100.0).contains(&board),
        "[invalid-input] carton board must be within 0..=100 mm"
    );
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] carton bleed must be within 0..=100 mm"
    );
    ensure!(
        glue_flap.is_finite() && glue_flap > 0.0 && glue_flap <= 1000.0,
        "[invalid-input] carton glue flap must be within (0,1000] mm"
    );
    ensure!(
        board < length.min(width).min(height) / 2.0,
        "[invalid-input] carton board must be less than half its smallest dimension"
    );

    let widths = [length, width, length, width];
    let ids = ["front", "right", "back", "left"];
    let x = [0.0, length, length + width, 2.0 * length + width];
    let total = 2.0 * (length + width);
    let dust = length / 2.0;
    let tuck = (width / 3.0).min(35.0);
    // Dust flaps fold slightly past square so their free edge sits more than one
    // board thickness below the closing lid instead of sharing its plane. The
    // lid keeps its square fold, so the board-compensated opening is unchanged.
    // The tilted flap must also stay inside the inner faces of the front and
    // back walls, so it is narrower than the wall by a board thickness per side.
    let dust_inset = (1.5 * board).min(width.min(length) / 4.0);
    let dust_drop = (2.0 * board).max(dust * 0.0175);
    let dust_fold_angle = 90.0 + (dust_drop / dust).min(0.5).asin().to_degrees();
    let body_y = width + dust + tuck;
    let body_end = body_y + height;
    let straight = template == "straight-tuck";
    let top_lid_index = if straight { 2 } else { 0 };
    let bottom_lid_index = if straight { 0 } else { 2 };
    let mut panels = Vec::with_capacity(12);
    let mut rectangles = Vec::with_capacity(12);
    let mut creases = Vec::with_capacity(11);
    let mut add_panel = |id: &str, slot: &str, rect: [f64; 4]| {
        panels.push(serde_json::json!({
            "id":id,"rect_mm":rect,"face_slot":slot,"uv_rect":[0.0,0.0,1.0,1.0]
        }));
        rectangles.push(rect);
    };
    let mut add_crease = |id: &str, line: [[f64; 2]; 2], angle: f64, direction: &str| {
        creases.push(serde_json::json!({
            "id":id,"line_mm":line,"angle_deg":angle,"direction":direction
        }));
    };

    for index in 0..4 {
        add_panel(
            ids[index],
            ids[index],
            [x[index], body_y, widths[index], height],
        );
        if index < 3 {
            let hinge_x = x[index] + widths[index];
            add_crease(
                &format!("{}-body-fold", ids[index]),
                [[hinge_x, body_y], [hinge_x, body_end]],
                90.0,
                "valley",
            );
        }
    }
    add_panel(
        "glue-joint",
        "glue-joint",
        [total, body_y, glue_flap, height],
    );
    add_crease(
        "glue-joint-fold",
        [[total, body_y], [total, body_end]],
        180.0,
        "valley",
    );

    for index in [1usize, 3] {
        let x0 = x[index] + dust_inset;
        let panel_width = widths[index] - 2.0 * dust_inset;
        for (edge, y0, id) in [
            ("top", body_y - dust, format!("{}-dust-top", ids[index])),
            ("bottom", body_end, format!("{}-dust-bottom", ids[index])),
        ] {
            let rect = [x0, y0, panel_width, dust];
            add_panel(&id, &id, rect);
            let hinge_y = if edge == "top" { body_y } else { body_end };
            add_crease(
                &format!("{id}-fold"),
                [[x0, hinge_y], [x0 + panel_width, hinge_y]],
                dust_fold_angle,
                "valley",
            );
        }
    }

    for (edge, index, y0, tab_y, direction) in [
        (
            "top",
            top_lid_index,
            body_y - width,
            body_y - width - tuck,
            if top_lid_index == 2 {
                "valley"
            } else {
                "mountain"
            },
        ),
        (
            "bottom",
            bottom_lid_index,
            body_end,
            body_end + width,
            if bottom_lid_index == 0 {
                "valley"
            } else {
                "mountain"
            },
        ),
    ] {
        let lid_id = format!("{edge}-lid");
        let lid_rect = [x[index], y0, length, width];
        add_panel(&lid_id, &lid_id, lid_rect);
        let hinge_y = if edge == "top" { body_y } else { body_end };
        add_crease(
            &format!("{lid_id}-fold"),
            [[x[index], hinge_y], [x[index] + length, hinge_y]],
            90.0,
            direction,
        );

        let tab_id = format!("{edge}-tuck");
        let tab_rect = [x[index], tab_y, length, tuck];
        add_panel(&tab_id, &tab_id, tab_rect);
        let tab_hinge_y = if edge == "top" { y0 } else { y0 + width };
        add_crease(
            &format!("{tab_id}-fold"),
            [[x[index], tab_hinge_y], [x[index] + length, tab_hinge_y]],
            90.0,
            "mountain",
        );
    }

    let cut_path = rectangle_net_cut_path(&rectangles, &creases);
    let crease_path = creases
        .iter()
        .filter_map(|crease| crease.get("line_mm"))
        .filter_map(Value::as_array)
        .filter_map(|line| Some((line[0].as_array()?, line[1].as_array()?)))
        .filter_map(|(a, b)| {
            Some((
                (a[0].as_f64()?, a[1].as_f64()?),
                (b[0].as_f64()?, b[1].as_f64()?),
            ))
        })
        .map(|(a, b)| format!("M {} {} L {} {}", a.0, a.1, b.0, b.1))
        .collect::<Vec<_>>()
        .join(" ");
    let glue_region = format!(
        "M {total} {body_y} L {} {body_y} L {} {body_end} L {total} {body_end} Z",
        total + glue_flap,
        total + glue_flap
    );
    Ok(serde_json::json!({
        "template":template,"units":"mm",
        "dimensions_mm":{"length":length,"width":width,"height":height,"board":board,"bleed":bleed,"glue_flap":glue_flap},
        "panels":panels,"creases":creases,
        "paths":[
            {"id":"carton-cut-outline","role":"cut","d":cut_path,"spot":"CutContour"},
            {"id":"carton-crease-lines","role":"crease","d":crease_path,"spot":"Crease"},
            {"id":"carton-glue-joint","role":"glue-flap","d":glue_region,"spot":"Glue"}
        ],"fold_state":1.0
    }))
}

/// Generate a rectangular mailer box with a hinged lid and a side glue joint.
/// It is a visual mockup net; positive thickness adds panel backs and exposed
/// cut-edge walls and a visual interior bridge across folded hinge wedges. Lock
/// tabs, score compression, and manufacturing allowances are not modeled.
pub fn mailer_box_dieline(
    length: f64,
    width: f64,
    height: f64,
    board: f64,
    bleed: f64,
    glue_flap: f64,
) -> Result<Value> {
    for (name, value) in [("length", length), ("width", width), ("height", height)] {
        ensure!(
            value.is_finite() && (1.0..=100_000.0).contains(&value),
            "[invalid-input] mailer {name} must be within 1..=100000 mm"
        );
    }
    ensure!(
        board.is_finite() && (0.0..=100.0).contains(&board),
        "[invalid-input] mailer board must be within 0..=100 mm"
    );
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] mailer bleed must be within 0..=100 mm"
    );
    ensure!(
        glue_flap.is_finite() && glue_flap > 0.0 && glue_flap <= 1000.0,
        "[invalid-input] mailer glue flap must be within (0,1000] mm"
    );
    ensure!(
        board < length.min(width).min(height) / 2.0,
        "[invalid-input] mailer board must be less than half its smallest dimension"
    );

    // The center is the base. Four wall panels and a lid connect to it as a
    // tree; the glue joint attaches to the right wall and closes the seam.
    let x = height;
    let y = height;
    let right = x + length;
    let lower = y + width;
    let back_bottom = lower + height;
    let panels = vec![
        serde_json::json!({"id":"base","rect_mm":[x,y,length,width],"face_slot":"base","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"front-wall","rect_mm":[x,y-height,length,height],"face_slot":"front-wall","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"back-wall","rect_mm":[x,lower,length,height],"face_slot":"back-wall","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"left-wall","rect_mm":[x-height,y,height,width],"face_slot":"left-wall","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"right-wall","rect_mm":[right,y,height,width],"face_slot":"right-wall","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"lid","rect_mm":[x,back_bottom,length,width],"face_slot":"lid","uv_rect":[0,0,1,1]}),
        serde_json::json!({"id":"glue-joint","rect_mm":[right+height,y,glue_flap,width],"face_slot":"glue-joint","uv_rect":[0,0,1,1]}),
    ];
    let crease_specs = [
        ("front-fold", [[x, y], [right, y]]),
        ("back-fold", [[x, lower], [right, lower]]),
        ("left-fold", [[x, y], [x, lower]]),
        ("right-fold", [[right, y], [right, lower]]),
        ("lid-fold", [[x, back_bottom], [right, back_bottom]]),
        (
            "glue-joint-fold",
            [[right + height, y], [right + height, lower]],
        ),
    ];
    let creases: Vec<Value> = crease_specs
        .iter()
        .map(|(id, line)| serde_json::json!({"id":id,"line_mm":line,"angle_deg":90,"direction":"valley"}))
        .collect();
    let crease_path = crease_specs
        .iter()
        .map(|(_, [[x0, y0], [x1, y1]])| format!("M {x0} {y0} L {x1} {y1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let panel_rectangles = panels
        .iter()
        .map(|panel| {
            let rect = panel["rect_mm"]
                .as_array()
                .expect("generated panel rectangle");
            [
                rect[0].as_f64().expect("generated panel x"),
                rect[1].as_f64().expect("generated panel y"),
                rect[2].as_f64().expect("generated panel width"),
                rect[3].as_f64().expect("generated panel height"),
            ]
        })
        .collect::<Vec<_>>();
    let cut_path = rectangle_net_cut_path(&panel_rectangles, &creases);
    Ok(serde_json::json!({
        "template":"mailer", "units":"mm",
        "dimensions_mm":{"length":length,"width":width,"height":height,"board":board,"bleed":bleed,"glue_flap":glue_flap},
        "panels":panels, "creases":creases,
        "paths":[
            {"id":"mailer-cut-outline","role":"cut","d":cut_path,"spot":"CutContour"},
            {"id":"mailer-creases","role":"crease","d":crease_path,"spot":"Crease"},
            {"id":"mailer-glue-joint","role":"glue-flap","d":format!("M {} {} h {} v {} h -{} Z",right+height,y,glue_flap,width,glue_flap),"spot":"GlueFlap"}
        ], "fold_state":1.0
    }))
}

/// Export a dieline's flat manufacturing marks as a self-contained SVG.
/// Linked panel artwork is intentionally not embedded; this is a technical
/// net preview and not a print-ready or certified production file.
pub fn flat_dieline_svg(dieline: &Value) -> Result<String> {
    let units = dieline
        .get("units")
        .and_then(Value::as_str)
        .context("[invalid-input] dieline units are missing")?;
    ensure!(
        units == "mm",
        "[unsupported-capability] flat SVG export requires millimeter units"
    );
    let panels = dieline
        .get("panels")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline panels must be an array")?;
    ensure!(!panels.is_empty(), "[invalid-input] dieline has no panels");
    let template = dieline
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or("packaging");
    let bleed = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    ensure!(
        bleed.is_finite() && (0.0..=100.0).contains(&bleed),
        "[invalid-input] dieline bleed must be within 0..=100 mm"
    );
    let mut max_x: f64 = 0.0;
    let mut max_y: f64 = 0.0;
    let mut panel_marks = String::new();
    for panel in panels {
        let id = panel
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline panel ID is missing")?;
        let rect = panel
            .get("rect_mm")
            .and_then(Value::as_array)
            .filter(|rect| rect.len() == 4)
            .context("[invalid-input] dieline panel rect_mm must contain four numbers")?;
        let [x, y, width, height] = [
            rect[0].as_f64(),
            rect[1].as_f64(),
            rect[2].as_f64(),
            rect[3].as_f64(),
        ];
        let (Some(x), Some(y), Some(width), Some(height)) = (x, y, width, height) else {
            bail!("[invalid-input] dieline panel rect_mm must contain finite numbers");
        };
        ensure!(
            [x, y, width, height].iter().all(|value| value.is_finite())
                && x >= 0.0
                && y >= 0.0
                && width > 0.0
                && height > 0.0,
            "[invalid-input] dieline panel rectangle is outside the supported flat layout"
        );
        let draw_x = x + bleed;
        let draw_y = y + bleed;
        max_x = max_x.max(x + width);
        max_y = max_y.max(y + height);
        let safe_id = xml_escape(id);
        let safe_zone = panel
            .get("safe_zone_mm")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        ensure!(
            safe_zone.is_finite() && safe_zone >= 0.0 && safe_zone * 2.0 < width.min(height),
            "[invalid-input] panel `{id}` safe-zone inset does not fit its rectangle"
        );
        let safe_zone_mark = if safe_zone > 0.0 {
            format!(
                "<rect data-id=\"safe-zone-{safe_id}\" data-role=\"safe-zone\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{}\" stroke-dasharray=\"1.2 0.8\" stroke-width=\"0.2\"/>",
                draw_x + safe_zone,
                draw_y + safe_zone,
                width - 2.0 * safe_zone,
                height - 2.0 * safe_zone,
                role_color("safe-zone")
            )
        } else {
            String::new()
        };
        panel_marks.push_str(&format!(
            "<g data-panel=\"{safe_id}\"><rect x=\"{draw_x}\" y=\"{draw_y}\" width=\"{width}\" height=\"{height}\" fill=\"none\" stroke=\"#94a3b8\" stroke-dasharray=\"1 1\" stroke-width=\"0.15\"/>{safe_zone_mark}<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"3\" text-anchor=\"middle\" fill=\"#475569\">{safe_id}</text></g>",
            draw_x + width / 2.0,
            draw_y + height / 2.0
        ));
        if matches!(template, "tube" | "bottle-label") {
            let seam = dieline["wrap"]["seam_angle_deg"].as_f64().unwrap_or(0.0);
            let direction = dieline["wrap"]["direction"].as_str().unwrap_or("clockwise");
            let overlap = dieline["wrap"]["overlap_mm"].as_f64().unwrap_or(0.0);
            let overlap_mark = if overlap > 0.0 {
                format!("<rect id=\"wrap-overlap-{safe_id}\" data-role=\"wrap-overlap-preview\" x=\"{}\" y=\"{draw_y}\" width=\"{overlap}\" height=\"{height}\" fill=\"#f59e0b\" fill-opacity=\"0.16\" stroke=\"#b45309\" stroke-dasharray=\"1 0.6\" stroke-width=\"0.2\"/>", draw_x + width - overlap)
            } else {
                String::new()
            };
            let seam_label = if width >= 28.0 {
                format!("<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"2.5\" text-anchor=\"start\" fill=\"#6d28d9\">WRAP SEAM · {seam}°</text>", draw_x + 0.8, draw_y + 3.5)
            } else {
                String::new()
            };
            panel_marks.push_str(&format!(
                "<g id=\"wrap-seam-{}\" data-role=\"wrap-seam-preview\" data-angle-deg=\"{seam}\" data-direction=\"{}\" data-overlap-mm=\"{overlap}\">{overlap_mark}<path d=\"M {draw_x} {draw_y} V {} M {} {draw_y} V {}\" fill=\"none\" stroke=\"#7c3aed\" stroke-width=\"0.45\" stroke-dasharray=\"1.5 0.8\"/>{seam_label}</g>",
                xml_escape(id), xml_escape(direction), draw_y + height, draw_x + width, draw_y + height
            ));
        }
    }
    ensure!(
        max_x.is_finite() && max_y.is_finite() && max_x > 0.0 && max_y > 0.0,
        "[invalid-input] dieline bounds are invalid"
    );
    let mut marks = String::new();
    for path in dieline
        .get("paths")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline paths must be an array")?
    {
        let id = path
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline path ID is missing")?;
        let role = path
            .get("role")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline path role is missing")?;
        ensure!(
            matches!(
                role,
                "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap"
            ),
            "[invalid-input] dieline path has unknown role `{role}`"
        );
        let data = path
            .get("d")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline path data is missing")?;
        // Structural crease records are the canonical fold definition. Avoid
        // duplicating a production path that consists entirely of those same
        // straight fold lines, but retain independent scoring artwork (which
        // need not be a structural hinge) as its own production path.
        if role == "crease"
            && path["d"].as_str().is_some_and(|data| {
                structural_crease_path_matches_records(data, &dieline["creases"])
            })
        {
            continue;
        }
        let spot = path.get("spot").and_then(Value::as_str).unwrap_or("");
        ensure!(
            !data.is_empty() && data.len() <= 1_000_000,
            "[limit-exceeded] dieline path data is empty or too large"
        );
        let mut parsed = kurbo::BezPath::from_svg(data).with_context(|| {
            format!("[invalid-input] dieline path `{id}` is not valid SVG path data")
        })?;
        let bounds = parsed.bounding_box();
        ensure!(
            [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
                .iter()
                .all(|value| value.is_finite())
                && bounds.x0 >= 0.0
                && bounds.y0 >= 0.0,
            "[invalid-input] production path `{id}` has invalid or negative flat-layout bounds"
        );
        max_x = max_x.max(bounds.x1);
        max_y = max_y.max(bounds.y1);
        parsed.apply_affine(kurbo::Affine::translate((bleed, bleed)));
        ensure!(
            !parsed.elements().is_empty(),
            "[invalid-input] dieline path `{id}` is not valid SVG path data"
        );
        marks.push_str(&format!(
            "<path data-id=\"{}\" data-role=\"{role}\" data-spot=\"{}\" d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"0.2\"/>",
            xml_escape(id), xml_escape(spot), xml_escape(&parsed.to_svg()), role_color(role)
        ));
    }
    for crease in dieline
        .get("creases")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline creases must be an array")?
    {
        let id = crease
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] crease ID is missing")?;
        let points = crease
            .get("line_mm")
            .and_then(Value::as_array)
            .filter(|points| points.len() == 2)
            .context("[invalid-input] crease line_mm must contain two points")?;
        let start = pair(&points[0], "crease start")?;
        let end = pair(&points[1], "crease end")?;
        ensure!(
            start[0] >= 0.0 && start[1] >= 0.0 && end[0] >= 0.0 && end[1] >= 0.0,
            "[invalid-input] crease `{id}` has negative flat-layout bounds"
        );
        max_x = max_x.max(start[0].max(end[0]));
        max_y = max_y.max(start[1].max(end[1]));
        let direction = crease
            .get("direction")
            .and_then(Value::as_str)
            .context("[invalid-input] crease direction is missing")?;
        ensure!(
            matches!(direction, "mountain" | "valley"),
            "[invalid-input] crease direction must be mountain or valley"
        );
        let angle = number(
            crease.get("angle_deg").unwrap_or(&Value::Null),
            "crease angle_deg",
            -180.0,
            180.0,
        )?;
        let (stroke, dash) = if direction == "mountain" {
            ("#7c3aed", " stroke-dasharray=\"1.5 1\"")
        } else {
            (role_color("crease"), "")
        };
        marks.push_str(&format!(
            "<path data-id=\"{}\" data-role=\"crease\" data-spot=\"Crease\" data-direction=\"{direction}\" data-angle-deg=\"{angle}\" d=\"M {} {} L {} {}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"0.2\"{dash}/>",
            xml_escape(id), start[0] + bleed, start[1] + bleed, end[0] + bleed, end[1] + bleed
        ));
    }
    let output_width = max_x + 2.0 * bleed;
    let output_height = max_y + 2.0 * bleed;
    let mut bleed_guides = String::new();
    if bleed > 0.0 {
        for (index, panel) in panels.iter().enumerate() {
            let rect = panel["rect_mm"]
                .as_array()
                .expect("validated panel rectangle");
            let x = rect[0].as_f64().expect("validated panel x");
            let y = rect[1].as_f64().expect("validated panel y");
            let width = rect[2].as_f64().expect("validated panel width");
            let height = rect[3].as_f64().expect("validated panel height");
            let draw_x = x + bleed;
            let draw_y = y + bleed;
            let [left, right, top, bottom] = if panel_cut_contour(dieline, panel).is_some() {
                [false; 4]
            } else {
                panel_cut_edges(dieline, panel)?
            };
            let mut path = String::new();
            if left {
                path.push_str(&format!(
                    "M {} {} V {} ",
                    draw_x - bleed,
                    draw_y,
                    draw_y + height
                ));
            }
            if right {
                path.push_str(&format!(
                    "M {} {} V {} ",
                    draw_x + width + bleed,
                    draw_y,
                    draw_y + height
                ));
            }
            if top {
                path.push_str(&format!(
                    "M {} {} H {} ",
                    draw_x,
                    draw_y - bleed,
                    draw_x + width
                ));
            }
            if bottom {
                path.push_str(&format!(
                    "M {} {} H {} ",
                    draw_x,
                    draw_y + height + bleed,
                    draw_x + width
                ));
            }
            if !path.is_empty() {
                bleed_guides.push_str(&format!("<path id=\"bleed-preview-{index}\" data-role=\"bleed-preview\" data-panel=\"{}\" d=\"{path}\"/>", xml_escape(panel["id"].as_str().unwrap_or("panel"))));
            }
        }
        if !bleed_guides.is_empty() {
            bleed_guides = format!("<g id=\"bleed-guides\" data-bleed-mm=\"{bleed}\" fill=\"none\" stroke=\"{}\" stroke-dasharray=\"1.2 0.8\" stroke-width=\"0.2\">{bleed_guides}</g>", role_color("bleed"));
        }
    }
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{output_width}mm\" height=\"{output_height}mm\" viewBox=\"0 0 {output_width} {output_height}\" data-layout-bleed-mm=\"{bleed}\"><title>{}</title><desc>Flat dieline technical preview in millimeters. Panel artwork is not included; this file is not certified for manufacturing.</desc><g id=\"bleed-preview-guides\">{bleed_guides}</g><g id=\"panel-guides\">{panel_marks}</g><g id=\"production-marks\">{marks}</g></svg>\n",
        xml_escape(template)
    );
    ensure!(
        svg.len() <= MAX_FLAT_DIELINE_SVG_BYTES,
        "[limit-exceeded] flat dieline SVG exceeds the 64 MiB output limit"
    );
    Ok(svg)
}

#[derive(Debug)]
pub struct FlatDielineSvg {
    pub svg: String,
    pub artwork_panels: Vec<String>,
    pub rasterized_panels: Vec<String>,
    pub rasterized_ppi: Vec<Value>,
    pub bleed_synthesized: Vec<Value>,
    pub warnings: Vec<String>,
}

/// Resolve ordinary `.pen` layout objects back into the dieline consumed by
/// the shared flat and folded render paths. The layout page is an editing
/// surface, while stable panel/path IDs in the dieline remain the contract.
pub fn dieline_from_document(raw: &Value, dieline_id: &str) -> Result<Value> {
    let source = raw
        .get("dielines")
        .and_then(|items| items.get(dieline_id))
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?;
    let Some(layout_page_id) = source.get("layout_page").and_then(Value::as_str) else {
        return Ok(source.clone());
    };
    let page = raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(layout_page_id))
        .with_context(|| {
            format!("[missing-resource] editable dieline page `{layout_page_id}` is missing")
        })?;
    ensure!(
        page["dieline_layout"]["dieline"].as_str() == Some(dieline_id),
        "[invalid-input] page `{layout_page_id}` does not declare dieline `{dieline_id}`"
    );
    let bleed = source["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    #[derive(Default)]
    struct Edits {
        panels: std::collections::BTreeMap<String, [f64; 4]>,
        safe: std::collections::BTreeMap<String, [f64; 4]>,
        paths: std::collections::BTreeMap<String, (String, String, Option<String>)>,
        creases: std::collections::BTreeMap<String, [[f64; 2]; 2]>,
    }
    fn transform(node: &Value) -> Result<kurbo::Affine> {
        let Some(values) = node.get("transform").and_then(Value::as_array) else {
            return Ok(kurbo::Affine::IDENTITY);
        };
        ensure!(
            values.len() == 6,
            "[invalid-input] dieline layout object `{}` has an invalid transform",
            node["id"].as_str().unwrap_or("<unknown>")
        );
        let mut matrix = [0.0; 6];
        for (index, value) in values.iter().enumerate() {
            matrix[index] = value
                .as_f64()
                .context("[invalid-input] dieline layout transform contains a non-number")?;
            ensure!(
                matrix[index].is_finite(),
                "[invalid-input] dieline layout transform contains a non-finite value"
            );
        }
        Ok(kurbo::Affine::new(matrix))
    }
    fn axis_aligned_rect(node: &Value, world: kurbo::Affine) -> Result<[f64; 4]> {
        let x = node["x"]
            .as_f64()
            .context("[invalid-input] dieline layout rectangle x is missing")?;
        let y = node["y"]
            .as_f64()
            .context("[invalid-input] dieline layout rectangle y is missing")?;
        let width = node["width"]
            .as_f64()
            .context("[invalid-input] dieline layout rectangle width is missing")?;
        let height = node["height"]
            .as_f64()
            .context("[invalid-input] dieline layout rectangle height is missing")?;
        ensure!(
            [x, y, width, height].iter().all(|v| v.is_finite()) && width > 0.0 && height > 0.0,
            "[invalid-input] dieline layout rectangle is invalid"
        );
        let corners = [
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
        ]
        .map(|(x, y)| world * kurbo::Point::new(x, y));
        let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let tolerance = max_x.abs().max(max_y.abs()).max(1.0) * 1.0e-8;
        ensure!(corners.iter().all(|p| (p.x-min_x).abs()<=tolerance || (p.x-max_x).abs()<=tolerance) && corners.iter().all(|p| (p.y-min_y).abs()<=tolerance || (p.y-max_y).abs()<=tolerance), "[unsupported-capability] dieline panels and safe areas must remain axis-aligned; reset the object rotation or shear");
        Ok([min_x, min_y, max_x - min_x, max_y - min_y])
    }
    fn collect(
        nodes: &[Value],
        parent: kurbo::Affine,
        edits: &mut Edits,
        bleed: f64,
    ) -> Result<()> {
        for node in nodes {
            let world = parent * transform(node)?;
            if let Some(id) = node.get("dieline_panel_id").and_then(Value::as_str) {
                ensure!(
                    node["kind"] == "rect",
                    "[invalid-input] panel layout object `{id}` must remain a rectangle"
                );
                ensure!(
                    edits
                        .panels
                        .insert(id.to_owned(), axis_aligned_rect(node, world)?)
                        .is_none(),
                    "[invalid-input] dieline layout has duplicate panel object `{id}`"
                );
            }
            if let Some(id) = node.get("dieline_safe_zone_panel").and_then(Value::as_str) {
                ensure!(
                    node["kind"] == "rect",
                    "[invalid-input] safe-area layout object for `{id}` must remain a rectangle"
                );
                ensure!(
                    edits
                        .safe
                        .insert(id.to_owned(), axis_aligned_rect(node, world)?)
                        .is_none(),
                    "[invalid-input] dieline layout has duplicate safe-area object for `{id}`"
                );
            }
            if let Some(id) = node.get("dieline_path_id").and_then(Value::as_str) {
                ensure!(
                    node["kind"] == "path",
                    "[invalid-input] production path object `{id}` must remain a path"
                );
                let data = node["d"]
                    .as_str()
                    .context("[invalid-input] editable production path has no SVG data")?;
                let path_data = match kurbo::BezPath::from_svg(data) {
                    Ok(mut path) if !path.elements().is_empty() && path.is_finite() => {
                        path.apply_affine(world);
                        path.apply_affine(kurbo::Affine::translate((-bleed, -bleed)));
                        path.to_svg()
                    }
                    _ => {
                        ensure!(
                            world.as_coeffs() == kurbo::Affine::IDENTITY.as_coeffs(),
                            "[invalid-input] malformed production path `{id}` cannot be transformed"
                        );
                        // Keep malformed source data available to packaging
                        // preflight, which reports it alongside other
                        // production warnings instead of hiding it here.
                        data.to_owned()
                    }
                };
                let role = node["dieline_path_role"]
                    .as_str()
                    .context("[invalid-input] production path role metadata is missing")?
                    .to_owned();
                let spot = node["dieline_path_spot"].as_str().map(str::to_owned);
                ensure!(
                    edits
                        .paths
                        .insert(id.to_owned(), (path_data, role, spot))
                        .is_none(),
                    "[invalid-input] dieline layout has duplicate production path `{id}`"
                );
            }
            if let Some(id) = node.get("dieline_crease_id").and_then(Value::as_str) {
                ensure!(
                    node["kind"] == "path",
                    "[invalid-input] crease object `{id}` must remain a path"
                );
                let mut path = kurbo::BezPath::from_svg(
                    node["d"]
                        .as_str()
                        .context("[invalid-input] editable crease has no SVG data")?,
                )?;
                path.apply_affine(world);
                path.apply_affine(kurbo::Affine::translate((-bleed, -bleed)));
                let [kurbo::PathEl::MoveTo(start), kurbo::PathEl::LineTo(end)] = path.elements()
                else {
                    bail!("[unsupported-capability] crease `{id}` must remain one straight two-point path");
                };
                let line = [[start.x, start.y], [end.x, end.y]];
                ensure!(
                    line.iter().flatten().all(|value| value.is_finite()),
                    "[invalid-input] crease `{id}` contains non-finite coordinates"
                );
                ensure!(
                    edits.creases.insert(id.to_owned(), line).is_none(),
                    "[invalid-input] dieline layout has duplicate crease `{id}`"
                );
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                collect(children, world, edits, bleed)?;
            }
        }
        Ok(())
    }
    let mut edits = Edits::default();
    for layer in page
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let nodes = layer
            .get("nodes")
            .and_then(Value::as_array)
            .context("[invalid-input] dieline layout layer nodes are missing")?;
        collect(nodes, kurbo::Affine::IDENTITY, &mut edits, bleed)?;
    }
    let mut resolved = source.clone();
    for panel in resolved["panels"]
        .as_array_mut()
        .context("[invalid-input] dieline panels are missing")?
    {
        let id = panel["id"]
            .as_str()
            .context("[invalid-input] dieline panel ID is missing")?
            .to_owned();
        let [x, y, width, height] = *edits.panels.get(&id).with_context(|| {
            format!("[missing-resource] editable layout object for panel `{id}` was removed")
        })?;
        ensure!(
            x >= -bleed && y >= -bleed && width > 0.0 && height > 0.0,
            "[invalid-input] edited panel `{id}` lies outside the supported flat layout"
        );
        panel["rect_mm"] = serde_json::json!([x - bleed, y - bleed, width, height]);
        if let Some([safe_x, safe_y, safe_width, safe_height]) = edits.safe.get(&id) {
            let x_inset = ((safe_x - x) + (x + width - (safe_x + safe_width))) / 2.0;
            let y_inset = ((safe_y - y) + (y + height - (safe_y + safe_height))) / 2.0;
            let tolerance = width.max(height).max(1.0) * 1.0e-6;
            ensure!((x_inset-y_inset).abs()<=tolerance && ((safe_x-x)-x_inset).abs()<=tolerance && ((safe_y-y)-y_inset).abs()<=tolerance && *safe_width>0.0 && *safe_height>0.0, "[invalid-input] safe-area guide for panel `{id}` must remain an equal inset on all sides");
            panel["safe_zone_mm"] = serde_json::json!(x_inset.max(0.0));
        } else {
            panel.as_object_mut().unwrap().remove("safe_zone_mm");
        }
    }
    let source_paths = source["paths"]
        .as_array()
        .context("[invalid-input] dieline paths are missing")?;
    let mut paths = Vec::new();
    for original in source_paths {
        let id = original["id"]
            .as_str()
            .context("[invalid-input] production path ID is missing")?;
        if let Some((data, role, spot)) = edits.paths.get(id) {
            let mut path = original.clone();
            path["d"] = serde_json::json!(data);
            path["role"] = serde_json::json!(role);
            if let Some(spot) = spot {
                path["spot"] = serde_json::json!(spot);
            }
            paths.push(path);
        }
    }
    for (id, (data, role, spot)) in &edits.paths {
        let is_new_path = !source_paths
            .iter()
            .any(|original| original["id"].as_str() == Some(id));
        if is_new_path {
            paths.push(serde_json::json!({
                "id":id,
                "role":role,
                "spot":spot,
                "d":data
            }));
        }
    }
    resolved["paths"] = serde_json::json!(paths);
    let mut creases = Vec::new();
    for original in source["creases"]
        .as_array()
        .context("[invalid-input] dieline creases are missing")?
    {
        let id = original["id"]
            .as_str()
            .context("[invalid-input] crease ID is missing")?;
        if let Some(line) = edits.creases.get(id) {
            let mut crease = original.clone();
            crease["line_mm"] = serde_json::json!(line);
            creases.push(crease);
        }
    }
    resolved["creases"] = serde_json::json!(creases);
    Ok(resolved)
}

fn edge_is_creased(dieline: &Value, start: [f64; 2], end: [f64; 2]) -> bool {
    let scale = start
        .into_iter()
        .chain(end)
        .fold(1.0_f64, |max, value| max.max(value.abs()));
    let tolerance = scale * 1.0e-8;
    dieline["creases"].as_array().is_some_and(|creases| {
        creases.iter().any(|crease| {
            let Some(points) = crease["line_mm"]
                .as_array()
                .filter(|points| points.len() == 2)
            else {
                return false;
            };
            let (Ok(a), Ok(b)) = (
                pair(&points[0], "crease start"),
                pair(&points[1], "crease end"),
            ) else {
                return false;
            };
            let close = |left: [f64; 2], right: [f64; 2]| {
                (left[0] - right[0]).abs() <= tolerance && (left[1] - right[1]).abs() <= tolerance
            };
            (close(a, start) && close(b, end)) || (close(a, end) && close(b, start))
        })
    })
}

fn panel_cut_edges(dieline: &Value, panel: &Value) -> Result<[bool; 4]> {
    let rect = panel["rect_mm"]
        .as_array()
        .filter(|rect| rect.len() == 4)
        .context("[invalid-input] panel rect_mm must contain four numbers")?;
    let (x, y, width, height) = (
        rect[0]
            .as_f64()
            .context("[invalid-input] panel x is invalid")?,
        rect[1]
            .as_f64()
            .context("[invalid-input] panel y is invalid")?,
        rect[2]
            .as_f64()
            .context("[invalid-input] panel width is invalid")?,
        rect[3]
            .as_f64()
            .context("[invalid-input] panel height is invalid")?,
    );
    let edges = [
        ([x, y], [x, y + height]),
        ([x + width, y], [x + width, y + height]),
        ([x, y], [x + width, y]),
        ([x, y + height], [x + width, y + height]),
    ];
    let mut cut = edges.map(|(start, end)| !edge_is_creased(dieline, start, end));
    // The two vertical edges of a tube label meet at the wrap seam; they are
    // neither cut edges nor a place to extend artwork across the seam.
    if matches!(dieline["template"].as_str(), Some("tube" | "bottle-label")) {
        cut[0] = false;
        cut[1] = false;
    }
    Ok(cut)
}

fn panel_cut_contour<'a>(dieline: &'a Value, panel: &Value) -> Option<&'a Value> {
    let panel_id = panel.get("id")?.as_str()?;
    dieline.get("paths")?.as_array()?.iter().find(|path| {
        path.get("role").and_then(Value::as_str) == Some("cut")
            && path.get("panel_id").and_then(Value::as_str) == Some(panel_id)
    })
}

fn bleed_clip_regions(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bleed: f64,
    cut: [bool; 4],
) -> String {
    let [left, right, top, bottom] = cut;
    let mut regions = String::new();
    let mut add = |x: f64, y: f64, width: f64, height: f64| {
        if width > 0.0 && height > 0.0 {
            regions.push_str(&format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\"/>"
            ));
        }
    };
    if left {
        add(x - bleed, y, bleed, height);
    }
    if right {
        add(x + width, y, bleed, height);
    }
    if top {
        add(x, y - bleed, width, bleed);
    }
    if bottom {
        add(x, y + height, width, bleed);
    }
    if left && top {
        add(x - bleed, y - bleed, bleed, bleed);
    }
    if right && top {
        add(x + width, y - bleed, bleed, bleed);
    }
    if left && bottom {
        add(x - bleed, y + height, bleed, bleed);
    }
    if right && bottom {
        add(x + width, y + height, bleed, bleed);
    }
    regions
}

#[derive(Clone, Copy)]
struct PanelRectMm {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn render_artwork_edge(
    fragment: &str,
    rect: PanelRectMm,
    pixel_size: [u32; 2],
    edge: usize,
    ppi: f64,
) -> Result<image::RgbaImage> {
    let sample_mm = 25.4 / ppi;
    let [pixel_width, pixel_height] = pixel_size;
    let (view_x, view_y, view_width, view_height, output_width, output_height) = match edge {
        0 => (rect.x, rect.y, sample_mm, rect.height, 1, pixel_height),
        1 => (
            rect.x + rect.width - sample_mm,
            rect.y,
            sample_mm,
            rect.height,
            1,
            pixel_height,
        ),
        2 => (rect.x, rect.y, rect.width, sample_mm, pixel_width, 1),
        3 => (
            rect.x,
            rect.y + rect.height - sample_mm,
            rect.width,
            sample_mm,
            pixel_width,
            1,
        ),
        _ => bail!("[internal] invalid packaging bleed edge"),
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{output_width}px\" height=\"{output_height}px\" viewBox=\"{view_x} {view_y} {view_width} {view_height}\">{fragment}</svg>"
    );
    let png = crate::render::svg_to_png(&svg, output_width, output_height, 1.0)
        .context("[render-failed] could not sample linked artwork at a cut edge")?;
    Ok(image::load_from_memory(&png)
        .context("[render-failed] could not decode sampled artwork edge")?
        .to_rgba8())
}

fn synthesize_bleed_pixels(
    fragment: &str,
    rect: PanelRectMm,
    bleed: f64,
    ppi: f64,
    cut_edges: [bool; 4],
) -> Result<(image::RgbaImage, u32, u32, u32)> {
    let px_per_mm = ppi / 25.4;
    let inner_width = (rect.width * px_per_mm).ceil();
    let inner_height = (rect.height * px_per_mm).ceil();
    let bleed_px = (bleed * px_per_mm).ceil();
    ensure!(
        [inner_width, inner_height, bleed_px]
            .into_iter()
            .all(|value| value.is_finite() && value >= 1.0 && value <= f64::from(u32::MAX / 2)),
        "[limit-exceeded] panel bleed raster dimensions exceed the supported range"
    );
    let (inner_width, inner_height, bleed_px) =
        (inner_width as u32, inner_height as u32, bleed_px as u32);
    let output_width = inner_width
        .checked_add(bleed_px.saturating_mul(2))
        .context("[limit-exceeded] bleed image width overflow")?;
    let output_height = inner_height
        .checked_add(bleed_px.saturating_mul(2))
        .context("[limit-exceeded] bleed image height overflow")?;
    let pixels = u64::from(output_width)
        .checked_mul(u64::from(output_height))
        .context("[limit-exceeded] bleed image pixel count overflow")?;
    ensure!(
        pixels <= MAX_FLAT_BLEED_PIXELS,
        "[limit-exceeded] synthesized panel bleed exceeds the 16-megapixel limit at the selected ppi"
    );
    let mut extended = image::RgbaImage::new(output_width, output_height);
    let mut samples: [Option<image::RgbaImage>; 4] = [None, None, None, None];
    for edge in 0..4 {
        if cut_edges[edge] {
            samples[edge] = Some(render_artwork_edge(
                fragment,
                rect,
                [inner_width, inner_height],
                edge,
                ppi,
            )?);
        }
    }
    let top = cut_edges[2];
    let bottom = cut_edges[3];
    let left = cut_edges[0];
    let right = cut_edges[1];
    if top {
        let row = samples[2].as_ref().unwrap();
        for dy in 0..bleed_px {
            for sx in 0..inner_width {
                extended.put_pixel(sx + bleed_px, dy, *row.get_pixel(sx, 0));
            }
        }
    }
    if bottom {
        let row = samples[3].as_ref().unwrap();
        for dy in 0..bleed_px {
            for sx in 0..inner_width {
                extended.put_pixel(
                    sx + bleed_px,
                    inner_height + bleed_px + dy,
                    *row.get_pixel(sx, 0),
                );
            }
        }
    }
    if left {
        let column = samples[0].as_ref().unwrap();
        for sy in 0..inner_height {
            for dx in 0..bleed_px {
                extended.put_pixel(dx, sy + bleed_px, *column.get_pixel(0, sy));
            }
        }
    }
    if right {
        let column = samples[1].as_ref().unwrap();
        for sy in 0..inner_height {
            for dx in 0..bleed_px {
                extended.put_pixel(
                    inner_width + bleed_px + dx,
                    sy + bleed_px,
                    *column.get_pixel(0, sy),
                );
            }
        }
    }
    let corners = [
        (left && top, 0, 0, 0, 0),
        (right && top, output_width - bleed_px, 0, inner_width - 1, 0),
        (
            left && bottom,
            0,
            output_height - bleed_px,
            0,
            inner_height - 1,
        ),
        (
            right && bottom,
            output_width - bleed_px,
            output_height - bleed_px,
            inner_width - 1,
            inner_height - 1,
        ),
    ];
    for (enabled, x0, y0, sx, sy) in corners {
        if !enabled {
            continue;
        }
        let sample = if sy == 0 {
            samples[2].as_ref().unwrap()
        } else {
            samples[3].as_ref().unwrap()
        };
        let pixel = *sample.get_pixel(sx, 0);
        for dy in 0..bleed_px {
            for dx in 0..bleed_px {
                extended.put_pixel(x0 + dx, y0 + dy, pixel);
            }
        }
    }
    Ok((extended, inner_width, inner_height, bleed_px))
}

fn render_fragment_in_bounds(
    fragment: &str,
    view_box: [f64; 4],
    size: [u32; 2],
) -> Result<image::RgbaImage> {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}px\" height=\"{}px\" viewBox=\"{} {} {} {}\">{fragment}</svg>",
        size[0], size[1], view_box[0], view_box[1], view_box[2], view_box[3]
    );
    let png = crate::render::svg_to_png(&svg, size[0], size[1], 1.0)
        .context("[render-failed] could not rasterize panel contour bleed")?;
    Ok(image::load_from_memory(&png)
        .context("[render-failed] could not decode panel contour bleed raster")?
        .to_rgba8())
}

/// Extend linked art outwards from an explicitly panel-associated closed cut
/// contour. A bounded 5/7 chamfer pass supplies nearest contour pixels; final
/// bleed membership is checked against the exact Euclidean pixel radius.
fn synthesize_contour_bleed(
    fragment: &str,
    rect: PanelRectMm,
    bleed: f64,
    ppi: f64,
    contour: &str,
) -> Result<(image::RgbaImage, u32, u32, u32)> {
    let px_per_mm = ppi / 25.4;
    let inner_width = (rect.width * px_per_mm).ceil();
    let inner_height = (rect.height * px_per_mm).ceil();
    let bleed_px = (bleed * px_per_mm).ceil();
    ensure!(
        [inner_width, inner_height, bleed_px]
            .into_iter()
            .all(|value| value.is_finite() && value >= 1.0 && value <= f64::from(u32::MAX / 2)),
        "[limit-exceeded] panel contour bleed raster dimensions exceed the supported range"
    );
    let (inner_width, inner_height, bleed_px) =
        (inner_width as u32, inner_height as u32, bleed_px as u32);
    let width = inner_width
        .checked_add(bleed_px.saturating_mul(2))
        .context("[limit-exceeded] contour bleed image width overflow")?;
    let height = inner_height
        .checked_add(bleed_px.saturating_mul(2))
        .context("[limit-exceeded] contour bleed image height overflow")?;
    let pixel_count = u64::from(width)
        .checked_mul(u64::from(height))
        .context("[limit-exceeded] contour bleed pixel count overflow")?;
    ensure!(
        pixel_count <= MAX_FLAT_BLEED_PIXELS,
        "[limit-exceeded] synthesized panel contour bleed exceeds the 16-megapixel limit at the selected ppi"
    );
    let view_box = [
        rect.x - bleed,
        rect.y - bleed,
        rect.width + 2.0 * bleed,
        rect.height + 2.0 * bleed,
    ];
    let size = [width, height];
    let source = render_fragment_in_bounds(fragment, view_box, size)?;
    let mask_fragment = format!(
        "<path d=\"{}\" fill=\"#fff\" fill-rule=\"evenodd\"/>",
        xml_escape(contour)
    );
    let mask = render_fragment_in_bounds(&mask_fragment, view_box, size)?;
    let len = usize::try_from(pixel_count)
        .context("[limit-exceeded] contour bleed pixel count exceeds addressable memory")?;
    let mut nearest = vec![u32::MAX; len];
    let mut distance = vec![u16::MAX; len];
    for (index, pixel) in mask.pixels().enumerate() {
        if pixel[3] != 0 {
            nearest[index] = index as u32;
            distance[index] = 0;
        }
    }
    drop(mask);
    ensure!(
        nearest.iter().any(|index| *index != u32::MAX),
        "[invalid-input] associated cut contour rasterizes to an empty panel mask"
    );
    let update =
        |index: usize, neighbor: usize, weight: u16, nearest: &mut [u32], distance: &mut [u16]| {
            if nearest[neighbor] == u32::MAX {
                return;
            }
            let candidate = distance[neighbor].saturating_add(weight);
            if candidate < distance[index] {
                distance[index] = candidate;
                nearest[index] = nearest[neighbor];
            }
        };
    let row_width = width as usize;
    let row_count = height as usize;
    for y in 0..row_count {
        for x in 0..row_width {
            let index = y * row_width + x;
            if x > 0 {
                update(index, index - 1, 5, &mut nearest, &mut distance);
            }
            if y > 0 {
                update(index, index - row_width, 5, &mut nearest, &mut distance);
                if x > 0 {
                    update(index, index - row_width - 1, 7, &mut nearest, &mut distance);
                }
                if x + 1 < row_width {
                    update(index, index - row_width + 1, 7, &mut nearest, &mut distance);
                }
            }
        }
    }
    for y in (0..row_count).rev() {
        for x in (0..row_width).rev() {
            let index = y * row_width + x;
            if x + 1 < row_width {
                update(index, index + 1, 5, &mut nearest, &mut distance);
            }
            if y + 1 < row_count {
                update(index, index + row_width, 5, &mut nearest, &mut distance);
                if x > 0 {
                    update(index, index + row_width - 1, 7, &mut nearest, &mut distance);
                }
                if x + 1 < row_width {
                    update(index, index + row_width + 1, 7, &mut nearest, &mut distance);
                }
            }
        }
    }
    let radius_squared = u64::from(bleed_px) * u64::from(bleed_px);
    let mut extended = image::RgbaImage::new(width, height);
    for y in 0..row_count {
        for x in 0..row_width {
            let index = y * row_width + x;
            if distance[index] == 0 || nearest[index] == u32::MAX {
                continue;
            }
            let nearest_index = nearest[index] as usize;
            let seed_x = nearest_index % row_width;
            let seed_y = nearest_index / row_width;
            let dx = x.abs_diff(seed_x) as u64;
            let dy = y.abs_diff(seed_y) as u64;
            if dx * dx + dy * dy <= radius_squared {
                extended.put_pixel(
                    x as u32,
                    y as u32,
                    *source.get_pixel(seed_x as u32, seed_y as u32),
                );
            }
        }
    }
    Ok((extended, inner_width, inner_height, bleed_px))
}

fn flat_svg_size_mm(svg: &str) -> Result<(f64, f64)> {
    let root = svg
        .split_once("<svg ")
        .and_then(|(_, root)| root.split_once('>').map(|(attrs, _)| attrs))
        .context("[internal] flat dieline SVG has no root element")?;
    let dimension = |name: &str| -> Result<f64> {
        let text = root
            .split_once(&format!("{name}=\""))
            .and_then(|(_, tail)| tail.split_once('"').map(|(value, _)| value))
            .with_context(|| format!("[internal] flat dieline SVG is missing {name}"))?;
        let value = text
            .strip_suffix("mm")
            .context("[internal] flat dieline SVG dimensions must use millimeters")?
            .parse::<f64>()?;
        ensure!(
            value.is_finite() && value > 0.0,
            "[invalid-input] flat dieline SVG has invalid {name}"
        );
        Ok(value)
    };
    Ok((dimension("width")?, dimension("height")?))
}

/// Export the technical net and linked page/node artwork as a hybrid SVG.
/// Vector-capable sources remain vector; source content the v3 vector exporter
/// cannot represent is embedded as a deterministic CPU-rendered PNG.
pub fn flat_dieline_svg_with_artwork(
    raw: &Value,
    document: &Path,
    dieline_id: &str,
    min_ppi: f64,
) -> Result<FlatDielineSvg> {
    crate::scene::validate(raw)?;
    ensure!(
        min_ppi.is_finite() && (1.0..=2400.0).contains(&min_ppi),
        "[invalid-input] minimum artwork ppi must be within 1..=2400"
    );
    let resolved_dieline = dieline_from_document(raw, dieline_id)?;
    let dieline = &resolved_dieline;
    let mut svg = flat_dieline_svg(dieline)?;
    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels must be an array")?;
    let owner_id = dieline
        .get("scene3d_node")
        .and_then(Value::as_str)
        .context("[invalid-input] dieline scene3d owner is missing")?;
    fn contains(nodes: &[Value], id: &str) -> bool {
        nodes.iter().any(|node| {
            node.get("id").and_then(Value::as_str) == Some(id)
                || node
                    .get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|children| contains(children, id))
        })
    }
    let owner_page = raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|page| {
            page.get("layers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|layer| layer.get("nodes").and_then(Value::as_array))
                .any(|nodes| contains(nodes, owner_id))
        })
        .and_then(|page| page.get("id"))
        .and_then(Value::as_str)
        .context("[missing-resource] dieline owner page is missing")?;
    let bleed_mm = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    ensure!(
        bleed_mm.is_finite() && (0.0..=100.0).contains(&bleed_mm),
        "[invalid-input] dieline bleed must be within 0..=100 mm"
    );

    let mut artwork_group = String::new();
    let mut artwork_panels = Vec::new();
    let mut rasterized_panels = Vec::new();
    let mut rasterized_ppi = Vec::new();
    let mut bleed_synthesized = Vec::new();
    let mut warnings = Vec::new();
    let mut bleed_pixels_used = 0_u64;
    let bleed_ppi = min_ppi.max(300.0);
    let mut source_cache = std::collections::HashMap::<String, (String, bool, u32, u32)>::new();
    for (panel_index, panel) in panels.iter().enumerate() {
        let Some(source) = panel.get("artwork") else {
            continue;
        };
        let panel_id = panel
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] artwork panel ID is missing")?;
        let source_page = source
            .get("page")
            .and_then(Value::as_str)
            .context("[invalid-input] panel artwork page is missing")?;
        let cache_key = serde_json::to_string(source)?;
        let (content, rasterized, artwork_width, artwork_height) = if let Some(cached) =
            source_cache.get(&cache_key)
        {
            cached.clone()
        } else {
            let source_doc = super::bindings::filtered_source_document(
                raw,
                source_page,
                Some(owner_page),
                owner_id,
                source,
            )?;
            let (source_width, source_height) = source_page_data(&source_doc, source_page)?;
            let mut transparent = source_doc.clone();
            let transparent_page = transparent["pages"]
                .as_array_mut()
                .and_then(|pages| {
                    pages
                        .iter_mut()
                        .find(|page| page.get("id").and_then(Value::as_str) == Some(source_page))
                })
                .context("[missing-resource] filtered source page disappeared")?;
            transparent_page["canvas"]["background"] = Value::String("none".into());

            let vector_svg = source_vector_svg(&transparent, source_page);
            let resolved = match vector_svg {
                Ok(svg) => (
                    svg_inner(&svg)?.to_owned(),
                    false,
                    source_width,
                    source_height,
                ),
                Err(vector_error) => {
                    let source_pixels = u64::from(source_width)
                        .checked_mul(u64::from(source_height))
                        .context("[limit-exceeded] panel artwork pixel count overflow")?;
                    ensure!(
                        source_pixels <= MAX_FLAT_ARTWORK_PIXELS,
                        "[limit-exceeded] rasterized panel artwork exceeds the 16-megapixel limit"
                    );
                    let pixels = crate::composite::render_content(
                            &transparent,
                            Path::new(crate::resource::document_root(document)),
                            Some(source_page),
                            1.0,
                        )
                        .with_context(|| {
                            format!(
                                "[unsupported-capability] panel `{panel_id}` vector export failed ({vector_error}); CPU raster fallback also failed"
                            )
                        })?;
                    let (width, height) = (pixels.width(), pixels.height());
                    let mut png = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgba8(pixels)
                        .write_to(&mut png, image::ImageFormat::Png)
                        .context("[render-failed] could not encode linked panel artwork PNG")?;
                    let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
                    (
                            format!(
                                "<image x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"none\" href=\"data:image/png;base64,{data}\"/>"
                            ),
                            true,
                            width,
                            height,
                        )
                }
            };
            ensure!(
                resolved.0.len() <= MAX_FLAT_DIELINE_SVG_BYTES,
                "[limit-exceeded] linked panel artwork exceeds the 64 MiB SVG fragment limit"
            );
            source_cache.insert(cache_key.clone(), resolved.clone());
            resolved
        };
        if rasterized {
            rasterized_panels.push(panel_id.to_owned());
        }
        artwork_panels.push(panel_id.to_owned());
        let rect = panel["rect_mm"].as_array().unwrap();
        let x = rect[0].as_f64().unwrap();
        let y = rect[1].as_f64().unwrap();
        let width = rect[2].as_f64().unwrap();
        let height = rect[3].as_f64().unwrap();
        let (scale, offset, rotate) = super::node::panel_artwork_transform(panel);
        let placed_width = width * scale[0];
        let placed_height = height * scale[1];
        let placed_x = width * (offset[0] - scale[0] / 2.0);
        let placed_y = height * (offset[1] - scale[1] / 2.0);
        if rasterized {
            let effective_ppi = (f64::from(artwork_width) * 25.4 / placed_width)
                .min(f64::from(artwork_height) * 25.4 / placed_height);
            rasterized_ppi
                .push(serde_json::json!({"panel":panel_id,"effective_ppi":effective_ppi}));
            if effective_ppi < min_ppi {
                warnings.push(format!(
                    "panel `{panel_id}` raster artwork resolves to {effective_ppi:.1} ppi, below --min-ppi {min_ppi:.1}"
                ));
            }
        }
        let placed_artwork = format!(
            "<svg x=\"{placed_x}\" y=\"{placed_y}\" width=\"{placed_width}\" height=\"{placed_height}\" viewBox=\"0 0 {artwork_width} {artwork_height}\" preserveAspectRatio=\"none\" overflow=\"hidden\"><g transform=\"rotate({rotate} {} {})\">{content}</g></svg>",
            f64::from(artwork_width) / 2.0,
            f64::from(artwork_height) / 2.0
        );
        let fragment = if let Some(contour_path) = panel_cut_contour(dieline, panel) {
            let mut contour = kurbo::BezPath::from_svg(contour_path["d"].as_str().unwrap())?;
            contour.apply_affine(kurbo::Affine::translate((-x, -y)));
            let clip_id = format!("panel-art-cut-clip-{panel_index}");
            format!(
                "<svg data-artwork-panel=\"{}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" overflow=\"hidden\"><defs><clipPath id=\"{clip_id}\" clipPathUnits=\"userSpaceOnUse\"><path d=\"{}\" fill-rule=\"evenodd\"/></clipPath></defs><g clip-path=\"url(#{clip_id})\">{placed_artwork}</g></svg>",
                xml_escape(panel_id),
                xml_escape(&contour.to_svg())
            )
        } else {
            format!(
            "<svg data-artwork-panel=\"{}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" overflow=\"hidden\"><svg x=\"{placed_x}\" y=\"{placed_y}\" width=\"{placed_width}\" height=\"{placed_height}\" viewBox=\"0 0 {artwork_width} {artwork_height}\" preserveAspectRatio=\"none\" overflow=\"hidden\"><g transform=\"rotate({rotate} {} {})\">{content}</g></svg></svg>",
            xml_escape(panel_id),
            f64::from(artwork_width) / 2.0,
            f64::from(artwork_height) / 2.0
            )
        };
        if bleed_mm > 0.0 {
            let contour_path = panel_cut_contour(dieline, panel);
            let cut_edges = if contour_path.is_some() {
                [false; 4]
            } else {
                panel_cut_edges(dieline, panel)?
            };
            let regions = bleed_clip_regions(x, y, width, height, bleed_mm, cut_edges);
            if contour_path.is_some() || !regions.is_empty() {
                let px_per_mm = bleed_ppi / 25.4;
                let panel_px_width = (width * px_per_mm).ceil();
                let panel_px_height = (height * px_per_mm).ceil();
                let bleed_px = (bleed_mm * px_per_mm).ceil();
                ensure!(
                    panel_px_width.is_finite()
                        && panel_px_height.is_finite()
                        && bleed_px.is_finite()
                        && panel_px_width >= 1.0
                        && panel_px_height >= 1.0
                        && bleed_px >= 1.0
                        && panel_px_width <= f64::from(u32::MAX / 2)
                        && panel_px_height <= f64::from(u32::MAX / 2)
                        && bleed_px <= f64::from(u32::MAX / 2),
                    "[limit-exceeded] panel `{panel_id}` bleed raster dimensions exceed the supported range"
                );
                let (panel_px_width, panel_px_height) =
                    (panel_px_width as u32, panel_px_height as u32);
                let bleed_px = bleed_px as u32;
                let extended_width = panel_px_width
                    .checked_add(bleed_px.saturating_mul(2))
                    .context("[limit-exceeded] bleed image width overflow")?;
                let extended_height = panel_px_height
                    .checked_add(bleed_px.saturating_mul(2))
                    .context("[limit-exceeded] bleed image height overflow")?;
                let extended_pixels = u64::from(extended_width)
                    .checked_mul(u64::from(extended_height))
                    .context("[limit-exceeded] bleed image pixel count overflow")?;
                let next_bleed_pixels = bleed_pixels_used
                    .checked_add(extended_pixels)
                    .context("[limit-exceeded] aggregate bleed pixel count overflow")?;
                ensure!(
                    next_bleed_pixels <= MAX_FLAT_BLEED_PIXELS,
                    "[limit-exceeded] synthesized package bleed exceeds the 16-megapixel aggregate limit"
                );
                bleed_pixels_used = next_bleed_pixels;
                let rect = PanelRectMm {
                    x,
                    y,
                    width,
                    height,
                };
                let (extended, _, _, _) = if let Some(cut_path) = contour_path {
                    synthesize_contour_bleed(
                        &fragment,
                        rect,
                        bleed_mm,
                        bleed_ppi,
                        cut_path["d"].as_str().unwrap(),
                    )
                } else {
                    synthesize_bleed_pixels(&fragment, rect, bleed_mm, bleed_ppi, cut_edges)
                }
                .with_context(|| {
                    format!("[render-failed] could not synthesize panel `{panel_id}` bleed")
                })?;
                let (extended_width, extended_height) = (extended.width(), extended.height());
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(extended)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .context("[render-failed] could not encode synthesized panel bleed")?;
                let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
                let clip_id = format!("panel-bleed-clip-{panel_index}");
                let image_attrs = format!(
                    "data-panel-bleed=\"{}\" data-bleed-mm=\"{bleed_mm}\" data-bleed-ppi=\"{bleed_ppi}\" href=\"data:image/png;base64,{data}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"",
                    xml_escape(panel_id),
                    x - bleed_mm,
                    y - bleed_mm,
                    width + 2.0 * bleed_mm,
                    height + 2.0 * bleed_mm
                );
                let bleed_fragment = if contour_path.is_some() {
                    format!("<image {image_attrs}/>")
                } else {
                    format!(
                        "<defs><clipPath id=\"{clip_id}\" clipPathUnits=\"userSpaceOnUse\">{regions}</clipPath></defs><image {image_attrs} clip-path=\"url(#{clip_id})\"/>"
                    )
                };
                artwork_group.push_str(&bleed_fragment);
                let edges = ["left", "right", "top", "bottom"]
                    .into_iter()
                    .zip(cut_edges)
                    .filter_map(|(name, is_cut)| is_cut.then_some(name))
                    .collect::<Vec<_>>();
                let mut bleed_report = serde_json::json!({
                    "panel":panel_id,
                    "bleed_mm":bleed_mm,
                    "raster_ppi":bleed_ppi,
                    "pixel_width":extended_width,
                    "pixel_height":extended_height,
                    "cut_edges":edges,
                    "method":if contour_path.is_some() {"nearest-contour pixel extension; closed panel cut contour"} else {"nearest-edge pixel extension; crease and tube-seam edges excluded"}
                });
                if let Some(cut_path) = contour_path {
                    bleed_report["cut_path"] = cut_path["id"].clone();
                    warnings.push(format!(
                        "panel `{panel_id}` has {bleed_mm} mm nearest-contour bleed along cut path `{}` at {bleed_ppi:.1} ppi",
                        cut_path["id"].as_str().unwrap_or("?")
                    ));
                } else {
                    warnings.push(format!(
                        "panel `{panel_id}` has {bleed_mm} mm edge-extended bleed on cut edges at {bleed_ppi:.1} ppi; fold and tube-seam edges are excluded"
                    ));
                }
                bleed_synthesized.push(bleed_report);
            }
        }
        ensure!(
            artwork_group
                .len()
                .saturating_add(fragment.len())
                .saturating_add(svg.len())
                <= MAX_FLAT_DIELINE_SVG_BYTES,
            "[limit-exceeded] flat dieline SVG with artwork exceeds the 64 MiB output limit"
        );
        artwork_group.push_str(&fragment);
    }
    if !artwork_group.is_empty() {
        ensure!(
            svg.len().saturating_add(artwork_group.len()) <= MAX_FLAT_DIELINE_SVG_BYTES,
            "[limit-exceeded] flat dieline SVG with artwork exceeds the 64 MiB output limit"
        );
        let insertion = svg
            .find("<g id=\"panel-guides\">")
            .context("[invalid-operation] flat dieline SVG has no panel layer")?;
        svg.insert_str(
            insertion,
            &format!(
                "<g id=\"panel-artwork\" transform=\"translate({bleed_mm} {bleed_mm})\">{artwork_group}</g>"
            ),
        );
        svg = svg.replace(
            "Panel artwork is not included",
            "Linked artwork is included for bound panels",
        );
    }
    Ok(FlatDielineSvg {
        svg,
        artwork_panels,
        rasterized_panels,
        rasterized_ppi,
        bleed_synthesized,
        warnings,
    })
}

/// Build a flattened PDF/X-4 packaging draft from the exact flat SVG plan.
/// Technical cut/fold guides are rasterized into process CMYK by the existing
/// PDF/X subset; this is a proof/draft, not a cut-path separation or certified
/// manufacturing deliverable.
pub fn flat_dieline_pdfx_with_artwork(
    raw: &Value,
    document: &Path,
    dieline_id: &str,
    min_ppi: f64,
    ppi: f64,
) -> Result<(crate::color::pdfx::Exported, FlatDielineSvg)> {
    ensure!(
        ppi.is_finite() && (36.0..=1200.0).contains(&ppi),
        "[invalid-input] PDF/X packaging resolution must be within 36..=1200 ppi"
    );
    let flat = flat_dieline_svg_with_artwork(raw, document, dieline_id, min_ppi)?;
    let (width_mm, height_mm) = flat_svg_size_mm(&flat.svg)?;
    let pixels = |mm: f64| -> Result<u32> {
        let value = (mm / 25.4 * ppi).ceil();
        ensure!(
            value.is_finite() && (1.0..=32_768.0).contains(&value),
            "[limit-exceeded] packaging PDF/X side exceeds the 32768-pixel image limit"
        );
        Ok(value as u32)
    };
    let (width_px, height_px) = (pixels(width_mm)?, pixels(height_mm)?);
    ensure!(
        u64::from(width_px) * u64::from(height_px) <= crate::color::pdfx::MAX_PIXELS,
        "[limit-exceeded] packaging PDF/X exceeds the {}-pixel PDF/X limit; lower --ppi",
        crate::color::pdfx::MAX_PIXELS
    );
    let png = crate::render::svg_to_png(&flat.svg, width_px, height_px, 1.0)?;
    // Store the proof page in PDF points rather than millimeters. The shared
    // PDF/X renderer derives its raster scale as ppi / units_per_inch and the
    // composite engine caps that scale at 8. A millimeter page at 300 ppi
    // would require scale 11.81 and fail even though the requested output is
    // within both the PDF/X pixel budget and the packaging CLI's PPI range.
    // Point units preserve the exact physical page size while keeping the
    // renderer scale at the conventional 300 / 72.
    let points_per_mm = 72.0 / 25.4;
    let (width_pt, height_pt) = (width_mm * points_per_mm, height_mm * points_per_mm);

    // Render a temporary one-page document containing only this exact flat
    // export. Keep the original color catalog/output role so the shared PDF/X
    // path uses the user's declared CMYK destination and intent.
    let mut planned = raw.clone();
    let page = planned["pages"]
        .as_array_mut()
        .and_then(|pages| pages.first_mut())
        .context("[invalid-input] packaging PDF/X needs a document page")?;
    let page_id = page["id"]
        .as_str()
        .context("[invalid-input] packaging PDF/X page has no ID")?
        .to_owned();
    let layers = page["layers"]
        .as_array_mut()
        .context("[invalid-input] packaging PDF/X page has no layers")?;
    let first_layer = layers
        .first_mut()
        .context("[invalid-input] packaging PDF/X page has no target layer")?;
    let layer_id = first_layer["id"]
        .as_str()
        .context("[invalid-input] packaging PDF/X layer has no ID")?
        .to_owned();
    first_layer["nodes"] = serde_json::json!([]);
    first_layer["visible"] = serde_json::json!(true);
    first_layer["locked"] = serde_json::json!(false);
    first_layer["opacity"] = serde_json::json!(1.0);
    first_layer["blend_mode"] = serde_json::json!("normal");
    page["layers"] = serde_json::json!([first_layer.clone()]);
    page["canvas"] = serde_json::json!({
        "width": width_pt,
        "height": height_pt,
        "background": "#ffffffff"
    });
    let bleed_mm = raw["dielines"][dieline_id]["dimensions_mm"]["bleed"]
        .as_f64()
        .unwrap_or(0.0);
    page["production"] = serde_json::json!({
        "units_per_inch": 72.0,
        "trim": {
            "x":bleed_mm*points_per_mm,
            "y":bleed_mm*points_per_mm,
            "width":(width_mm-2.0*bleed_mm)*points_per_mm,
            "height":(height_mm-2.0*bleed_mm)*points_per_mm
        },
        "bleed": bleed_mm*points_per_mm
    });
    // This is now a one-page flattened proof, not a live packaging document.
    // Remove other pages and dieline ownership before image::add validates the
    // temporary document; the preview owner node was intentionally discarded.
    planned["pages"] = serde_json::json!([planned["pages"][0].clone()]);
    // v9+ layout metadata stores page IDs in spreads. Keep the proof's page in
    // a single non-facing spread as well; otherwise a multi-page v11 document
    // retains dangling references after the other pages above are removed.
    if let Some(layout) = planned.get_mut("layout").and_then(Value::as_object_mut) {
        let existing_spread = layout
            .get("spreads")
            .and_then(Value::as_array)
            .and_then(|spreads| {
                spreads.iter().find(|spread| {
                    spread
                        .get("pages")
                        .and_then(Value::as_array)
                        .is_some_and(|pages| {
                            pages.iter().any(|page| page.as_str() == Some(&page_id))
                        })
                })
            })
            .cloned();
        let mut proof_spread = existing_spread.unwrap_or_else(
            || serde_json::json!({"id":"packaging-proof","facing":false,"pages":[page_id]}),
        );
        proof_spread["facing"] = serde_json::json!(false);
        proof_spread["pages"] = serde_json::json!([page_id]);
        layout.insert("spreads".to_owned(), serde_json::json!([proof_spread]));
    }
    planned["dielines"] = serde_json::json!({});
    crate::image::add(
        &mut planned,
        Some(&page_id),
        &layer_id,
        "pack-flat-proof",
        &png,
        crate::image::embedded_storage(&png),
        0.0,
        0.0,
        width_pt,
        height_pt,
        crate::image::Fit::Fill,
    )?;
    crate::scene::validate(&planned)
        .context("[invalid-input] temporary flattened packaging proof is invalid")?;
    let pdf = crate::color::pdfx::build(
        &planned,
        document,
        Some(&page_id),
        &crate::color::pdfx::Options {
            ppi,
            title: Some(format!("Packaging dieline {dieline_id} (flattened draft)")),
            ..Default::default()
        },
    )?;
    Ok((pdf, flat))
}

fn spot_for_dieline_role(role: &str) -> Result<(&'static str, [f64; 3])> {
    match role {
        "cut" => Ok(("CutContour", [1.0, 0.0, 1.0])),
        "crease" => Ok(("Crease", [0.0, 0.8, 1.0])),
        "perforation" => Ok(("Perforation", [1.0, 0.8, 0.0])),
        "bleed" => Ok(("Bleed", [1.0, 0.35, 0.0])),
        "safe-zone" => Ok(("SafeZone", [0.0, 0.65, 0.1])),
        "glue-flap" => Ok(("GlueFlap", [0.0, 0.65, 0.75])),
        _ => bail!(
            "[unsupported-capability] dieline role `{role}` has no packaging spot-ink mapping"
        ),
    }
}

/// A single packaging spot plate represents one production operation. Mixing
/// roles (for example cut and crease) under one ink name makes the finishing
/// handoff ambiguous, even if the underlying paths remain individually labeled.
fn validate_spot_role_assignments<'a>(
    assignments: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<()> {
    let mut role_by_ink = std::collections::BTreeMap::<&str, &str>::new();
    for (ink, role) in assignments {
        if let Some(previous_role) = role_by_ink.insert(ink, role) {
            ensure!(
                previous_role == role,
                "[invalid-input] packaging spot ink `{ink}` is assigned to both `{previous_role}` and `{role}` roles; use a distinct spot ink for each operation"
            );
        }
    }
    Ok(())
}

/// Emit editable, process-independent SVG plates from the canonical dieline
/// paths. The same stored path data feeds the PNG plates and flat artwork.
fn crease_has_production_path(paths: &[Value], line: [[f64; 2]; 2]) -> bool {
    let scale = line
        .iter()
        .flatten()
        .fold(1.0_f64, |s, value| s.max(value.abs()));
    let tol = scale * 1.0e-8;
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol;
    paths
        .iter()
        .filter(|path| path["role"] == "crease")
        .any(|path| {
            let Some(data) = path["d"].as_str() else {
                return false;
            };
            let Ok(parsed) = kurbo::BezPath::from_svg(data) else {
                return false;
            };
            let mut start = None;
            parsed.elements().iter().any(|element| match element {
                kurbo::PathEl::MoveTo(point) => {
                    start = Some([point.x, point.y]);
                    false
                }
                kurbo::PathEl::LineTo(point) => {
                    let end = [point.x, point.y];
                    let found = start.is_some_and(|begin| {
                        (close(begin, line[0]) && close(end, line[1]))
                            || (close(begin, line[1]) && close(end, line[0]))
                    });
                    start = Some(end);
                    found
                }
                _ => {
                    start = None;
                    false
                }
            })
        })
}

fn structural_crease_path_matches_records(data: &str, creases: &Value) -> bool {
    if data.is_empty() || data.len() > 1_000_000 {
        return false;
    }
    let Ok(path) = kurbo::BezPath::from_svg(data) else {
        return false;
    };
    let Some(creases) = creases.as_array() else {
        return false;
    };
    let close = |a: [f64; 2], b: [f64; 2]| {
        let tolerance = a
            .into_iter()
            .chain(b)
            .fold(1.0_f64, |scale, value| scale.max(value.abs()))
            * 1e-8;
        (a[0] - b[0]).abs() <= tolerance && (a[1] - b[1]).abs() <= tolerance
    };
    let matches_crease = |start: [f64; 2], end: [f64; 2]| {
        creases.iter().any(|crease| {
            let Some(points) = crease["line_mm"]
                .as_array()
                .filter(|points| points.len() == 2)
            else {
                return false;
            };
            let Some(start_record) = points[0]
                .as_array()
                .filter(|point| point.len() == 2)
                .and_then(|point| Some([point[0].as_f64()?, point[1].as_f64()?]))
            else {
                return false;
            };
            let Some(end_record) = points[1]
                .as_array()
                .filter(|point| point.len() == 2)
                .and_then(|point| Some([point[0].as_f64()?, point[1].as_f64()?]))
            else {
                return false;
            };
            (close(start, start_record) && close(end, end_record))
                || (close(start, end_record) && close(end, start_record))
        })
    };

    let mut current = None;
    let mut segments = 0usize;
    for element in path.elements() {
        match element {
            kurbo::PathEl::MoveTo(point) => current = Some([point.x, point.y]),
            kurbo::PathEl::LineTo(point) => {
                let Some(start) = current else {
                    return false;
                };
                let end = [point.x, point.y];
                if !matches_crease(start, end) {
                    return false;
                }
                segments += 1;
                current = Some(end);
            }
            _ => return false,
        }
    }
    segments > 0
}

pub fn write_dieline_vector_spot_plates(
    raw: &Value,
    dieline_id: &str,
    out: &Path,
) -> Result<Value> {
    ensure!(
        !out.exists(),
        "[output-exists] vector plate directory {} already exists",
        out.display()
    );
    crate::scene::validate(raw)?;
    let resolved_dieline = dieline_from_document(raw, dieline_id)?;
    let dieline = &resolved_dieline;
    let panels = dieline["panels"]
        .as_array()
        .context("dieline panels missing")?;
    let mut width: f64 = 0.0;
    let mut height: f64 = 0.0;
    for panel in panels {
        let r = panel["rect_mm"]
            .as_array()
            .context("panel rect_mm missing")?;
        width = width.max(
            r[0].as_f64().context("panel x invalid")?
                + r[2].as_f64().context("panel width invalid")?,
        );
        height = height.max(
            r[1].as_f64().context("panel y invalid")?
                + r[3].as_f64().context("panel height invalid")?,
        );
    }
    ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "[invalid-input] dieline has invalid vector plate bounds"
    );
    let mut by_ink = std::collections::BTreeMap::<String, Vec<(String, String, String)>>::new();
    for path in dieline["paths"]
        .as_array()
        .context("dieline paths missing")?
    {
        let id = path["id"].as_str().context("dieline path ID missing")?;
        let role = path["role"].as_str().context("dieline path role missing")?;
        let (default_ink, _) = spot_for_dieline_role(role)?;
        let ink = path["spot"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(default_ink);
        let d = path["d"].as_str().context("dieline path data missing")?;
        let bez = kurbo::BezPath::from_svg(d)
            .with_context(|| format!("invalid production path `{id}`"))?;
        ensure!(
            !bez.elements().is_empty() && bez.is_finite(),
            "[invalid-input] production path `{id}` is empty or non-finite"
        );
        let bounds = bez.bounding_box();
        ensure!(
            bounds.x0 >= 0.0 && bounds.y0 >= 0.0,
            "[invalid-input] production path `{id}` lies outside the non-negative flat layout"
        );
        width = width.max(bounds.x1);
        height = height.max(bounds.y1);
        by_ink.entry(ink.to_owned()).or_default().push((
            id.to_owned(),
            role.to_owned(),
            d.to_owned(),
        ));
    }
    let production_paths = dieline["paths"]
        .as_array()
        .context("dieline paths missing")?;
    for crease in dieline["creases"]
        .as_array()
        .context("dieline creases missing")?
    {
        let id = crease["id"].as_str().context("crease ID missing")?;
        let points = crease["line_mm"]
            .as_array()
            .filter(|points| points.len() == 2)
            .context("crease line_mm must contain two points")?;
        let line = [
            pair(&points[0], "crease start")?,
            pair(&points[1], "crease end")?,
        ];
        if !crease_has_production_path(production_paths, line) {
            by_ink.entry("Crease".into()).or_default().push((
                format!("structural-crease-{id}"),
                "crease".into(),
                format!(
                    "M {} {} L {} {}",
                    line[0][0], line[0][1], line[1][0], line[1][1]
                ),
            ));
        }
    }
    for panel in panels {
        let inset = panel["safe_zone_mm"].as_f64().unwrap_or(0.0);
        if inset <= 0.0 {
            continue;
        }
        let r = panel["rect_mm"]
            .as_array()
            .context("panel rect_mm missing")?;
        let x = r[0].as_f64().context("panel x invalid")? + inset;
        let y = r[1].as_f64().context("panel y invalid")? + inset;
        let w = r[2].as_f64().context("panel width invalid")? - 2.0 * inset;
        let h = r[3].as_f64().context("panel height invalid")? - 2.0 * inset;
        ensure!(
            w > 0.0 && h > 0.0,
            "[invalid-input] safe-zone guide does not fit panel"
        );
        let id = panel["id"].as_str().context("panel ID missing")?;
        by_ink.entry("SafeZone".into()).or_default().push((
            format!("safe-zone-{id}"),
            "safe-zone".into(),
            format!("M {x} {y} h {w} v {h} h -{w} Z"),
        ));
    }
    validate_spot_role_assignments(by_ink.iter().flat_map(|(ink, paths)| {
        paths
            .iter()
            .map(move |(_, role, _)| (ink.as_str(), role.as_str()))
    }))?;
    ensure!(
        !by_ink.is_empty(),
        "[invalid-input] dieline has no production paths for vector plates"
    );
    let bleed_mm = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    ensure!(
        bleed_mm.is_finite() && (0.0..=100.0).contains(&bleed_mm),
        "[invalid-input] dieline bleed must be within 0..=100 mm"
    );
    let output_width = width + 2.0 * bleed_mm;
    let output_height = height + 2.0 * bleed_mm;
    std::fs::create_dir(out)?;
    let result = (|| -> Result<Value> {
        let mut entries = Vec::new();
        for (ink, paths) in by_ink {
            let filename = format!(
                "spot-{}.svg",
                ink.as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let mut svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{output_width}mm\" height=\"{output_height}mm\" viewBox=\"0 0 {output_width} {output_height}\" data-spot-ink=\"{}\"><g transform=\"translate({bleed_mm} {bleed_mm})\">", xml_escape(&ink));
            for (id, role, d) in paths {
                svg.push_str(&format!("<path data-id=\"{}\" data-role=\"{}\" d=\"{}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"0.2\" stroke-linecap=\"butt\" stroke-linejoin=\"round\"/>", xml_escape(&id), xml_escape(&role), xml_escape(&d)));
            }
            svg.push_str("</g></svg>\n");
            crate::editing::atomic_write(&out.join(&filename), svg.as_bytes())?;
            entries.push(serde_json::json!({"ink":ink,"path":filename,"sha256":crate::resource::sha256(svg.as_bytes()),"bytes":svg.len()}));
        }
        Ok(
            serde_json::json!({"format":"svg-vector-spot-plates","units":"mm","width_mm":output_width,"height_mm":output_height,"bleed_mm":bleed_mm,"plates":entries,"manufacturing_certified":false}),
        )
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(out);
    }
    result
}

/// Create process-independent spot plates from stored dieline paths and safe
/// area guides using the shared deterministic plate separation renderer.
pub fn dieline_spot_separations(
    raw: &Value,
    document: &Path,
    dieline_id: &str,
    ppi: f64,
) -> Result<(crate::color::separation::Separations, Value)> {
    ensure!(
        ppi.is_finite() && (36.0..=1200.0).contains(&ppi),
        "[invalid-input] packaging plate resolution must be within 36..=1200 ppi"
    );
    crate::scene::validate(raw)?;
    let resolved_dieline = dieline_from_document(raw, dieline_id)?;
    let dieline = &resolved_dieline;
    // Stored production paths can extend beyond panel edges (for example, a
    // cut contour may include bleed). Size the plate canvas to include them;
    // the panel-only SVG helper intentionally sizes its guides from panels.
    let panels = dieline["panels"]
        .as_array()
        .context("dieline panels missing")?;
    let mut width_mm: f64 = 0.0;
    let mut height_mm: f64 = 0.0;
    for panel in panels {
        let rect = panel["rect_mm"]
            .as_array()
            .context("panel rect_mm missing")?;
        width_mm = width_mm.max(
            rect[0].as_f64().context("panel x invalid")?
                + rect[2].as_f64().context("panel width invalid")?,
        );
        height_mm = height_mm.max(
            rect[1].as_f64().context("panel y invalid")?
                + rect[3].as_f64().context("panel height invalid")?,
        );
    }
    let px = |mm: f64, dimension: &str| -> Result<u32> {
        let value = (mm * ppi / 25.4).round();
        ensure!(
            value.is_finite() && (1.0..=u32::MAX as f64).contains(&value),
            "[limit-exceeded] {dimension} packaging plate dimension is outside the supported pixel range"
        );
        Ok(value as u32)
    };
    let (width_px, height_px) = (px(width_mm, "width")?, px(height_mm, "height")?);
    let scale_x = f64::from(width_px) / width_mm;
    let scale_y = f64::from(height_px) / height_mm;
    let stroke_scale = (scale_x + scale_y) / 2.0;

    // Spot IDs are canonical document ink IDs. Reject malformed/custom names
    // rather than silently slugifying them to a different output plate.
    let mut marks: Vec<(String, String, String, String, [f64; 3])> = Vec::new();
    for path in dieline["paths"]
        .as_array()
        .context("dieline paths missing")?
    {
        let id = path["id"]
            .as_str()
            .context("[invalid-input] dieline path ID missing")?;
        let role = path["role"]
            .as_str()
            .context("[invalid-input] dieline path role missing")?;
        let (default_spot, alternate) = spot_for_dieline_role(role)?;
        let spot = path["spot"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(default_spot);
        let d = path["d"]
            .as_str()
            .context("[invalid-input] dieline path d missing")?;
        marks.push((id.into(), role.into(), d.into(), spot.into(), alternate));
    }
    let production_paths = dieline["paths"]
        .as_array()
        .context("dieline paths missing")?;
    for crease in dieline["creases"]
        .as_array()
        .context("dieline creases missing")?
    {
        let id = crease["id"].as_str().context("crease ID missing")?;
        let points = crease["line_mm"]
            .as_array()
            .filter(|points| points.len() == 2)
            .context("crease line_mm must contain two points")?;
        let line = [
            pair(&points[0], "crease start")?,
            pair(&points[1], "crease end")?,
        ];
        if !crease_has_production_path(production_paths, line) {
            let (spot, alternate) = spot_for_dieline_role("crease")?;
            marks.push((
                format!("structural-crease-{id}"),
                "crease".into(),
                format!(
                    "M {} {} L {} {}",
                    line[0][0], line[0][1], line[1][0], line[1][1]
                ),
                spot.into(),
                alternate,
            ));
        }
    }
    for panel in dieline["panels"]
        .as_array()
        .context("dieline panels missing")?
    {
        let inset = panel["safe_zone_mm"].as_f64().unwrap_or(0.0);
        if inset <= 0.0 {
            continue;
        }
        let id = panel["id"].as_str().context("panel ID missing")?;
        let rect = panel["rect_mm"]
            .as_array()
            .context("panel rect_mm missing")?;
        let x = rect[0].as_f64().context("panel x invalid")? + inset;
        let y = rect[1].as_f64().context("panel y invalid")? + inset;
        let width = rect[2].as_f64().context("panel width invalid")? - 2.0 * inset;
        let height = rect[3].as_f64().context("panel height invalid")? - 2.0 * inset;
        ensure!(
            width > 0.0 && height > 0.0,
            "[invalid-input] safe-zone guide does not fit panel `{id}`"
        );
        marks.push((
            format!("safe-zone-{id}"),
            "safe-zone".into(),
            format!("M {x} {y} h {width} v {height} h -{width} Z"),
            "SafeZone".into(),
            [0.0, 0.65, 0.1],
        ));
    }
    validate_spot_role_assignments(
        marks
            .iter()
            .map(|(_, role, _, spot, _)| (spot.as_str(), role.as_str())),
    )?;
    ensure!(
        !marks.is_empty(),
        "[invalid-input] dieline has no spot production paths or safe-zone guides to separate"
    );
    let mut ink_alternates = std::collections::BTreeMap::<String, [f64; 3]>::new();
    let mut ink_roles =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for (_, role, _, spot, alternate) in &marks {
        ink_alternates.entry(spot.clone()).or_insert(*alternate);
        ink_roles
            .entry(spot.clone())
            .or_default()
            .insert(role.clone());
    }
    for (_, _, d, _, _) in &marks {
        let path =
            kurbo::BezPath::from_svg(d).context("[invalid-input] malformed production path")?;
        let bounds = path.bounding_box();
        ensure!(
            bounds.x0 >= 0.0 && bounds.y0 >= 0.0,
            "[invalid-input] production path lies outside the non-negative flat layout"
        );
        width_mm = width_mm.max(bounds.x1);
        height_mm = height_mm.max(bounds.y1);
    }
    let flat_width_mm = width_mm;
    let flat_height_mm = height_mm;
    let bleed_mm = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    ensure!(
        bleed_mm.is_finite() && (0.0..=100.0).contains(&bleed_mm),
        "[invalid-input] dieline bleed must be within 0..=100 mm"
    );
    width_mm += 2.0 * bleed_mm;
    height_mm += 2.0 * bleed_mm;
    ensure!(
        ink_alternates.len() <= 32,
        "[limit-exceeded] packaging spot export supports at most 32 spot inks"
    );

    let mut planned = raw.clone();
    let first_page = planned["pages"]
        .as_array()
        .and_then(|pages| pages.first())
        .context("[invalid-input] packaging separation needs a document page")?;
    let page_id = first_page["id"]
        .as_str()
        .context("[invalid-input] packaging separation page has no ID")?
        .to_owned();
    first_page["layers"]
        .as_array()
        .and_then(|layers| layers.first())
        .and_then(|layer| layer["id"].as_str())
        .context("[invalid-input] packaging separation page has no layer")?;
    let mut nodes = Vec::with_capacity(marks.len());
    for (index, (id, role, d, spot, alternate)) in marks.iter().enumerate() {
        let mut path = kurbo::BezPath::from_svg(d).with_context(|| {
            format!("[invalid-input] production path `{id}` has invalid SVG data")
        })?;
        ensure!(
            !path.elements().is_empty() && path.is_finite(),
            "[invalid-input] production path `{id}` is empty or non-finite"
        );
        let bounds = path.bounding_box();
        ensure!(
            bounds.x0 >= 0.0
                && bounds.y0 >= 0.0
                && bounds.x1 <= flat_width_mm
                && bounds.y1 <= flat_height_mm,
            "[invalid-input] production path `{id}` lies outside the flat dieline bounds"
        );
        path.apply_affine(kurbo::Affine::new([
            scale_x,
            0.0,
            0.0,
            scale_y,
            bleed_mm * scale_x,
            bleed_mm * scale_y,
        ]));
        let fallback = format!(
            "#{:02x}{:02x}{:02x}",
            (alternate[0] * 255.0).round() as u8,
            (alternate[1] * 255.0).round() as u8,
            (alternate[2] * 255.0).round() as u8
        );
        nodes.push(serde_json::json!({
            "kind":"path","id":format!("pack-spot-{index:04}-{role}"),
            "d":path.to_svg(),"closed":false,
            "style":{
                "fill":{"fallback":"none"},
                "stroke":{"spot":{"ink":spot,"tint":1.0},"fallback":fallback},
                "stroke_width":{"fallback":0.2*stroke_scale},
                "stroke_linecap":{"fallback":"butt"},
                "stroke_linejoin":{"fallback":"round"},
                "stroke_miterlimit":{"fallback":4.0},
                "overprint":{"fill":false,"stroke":true,"mode":0}
            }
        }));
    }
    let inks: serde_json::Map<String, Value> = ink_alternates
        .iter()
        .map(|(id, alternate)| (
            id.clone(),
            serde_json::json!({
                "id":id,"name":format!("Packaging {id}"),"aliases":[],
                "alternate":{"space":"srgb","channels":alternate},
                "provenance":{"source":"Pentool packaging role plate alternate","license":"generated technical role mapping"},
                "redistribution":"allowed"
            }),
        ))
        .collect();
    planned["color_management"]["inks"] = serde_json::json!({"schema":1,"inks":inks});
    let page = planned["pages"]
        .as_array_mut()
        .and_then(|pages| pages.first_mut())
        .context("[invalid-input] packaging separation needs a document page")?;
    page["canvas"] =
        serde_json::json!({"width":width_px,"height":height_px,"background":"#ffffffff"});
    page.as_object_mut().unwrap().remove("production");
    let mut layer = page["layers"]
        .as_array()
        .and_then(|layers| layers.first())
        .cloned()
        .context("[invalid-input] packaging separation page has no layer")?;
    layer["nodes"] = serde_json::json!(nodes);
    layer["visible"] = serde_json::json!(true);
    layer["locked"] = serde_json::json!(false);
    layer["opacity"] = serde_json::json!(1.0);
    layer["blend_mode"] = serde_json::json!("normal");
    page["layers"] = serde_json::json!([layer]);
    // The separation canvas contains only technical marks; its former 3D
    // preview owner is gone. Keep just this raster-plate page and discard the
    // dieline catalog before validation so no owner links can dangle.
    planned["pages"] = serde_json::json!([planned["pages"][0].clone()]);
    // v9+ layout metadata stores page IDs in spreads. This temporary document
    // is now a one-page technical plate, so remove spread references to the
    // discarded artwork pages before validating it.
    if let Some(layout) = planned.get_mut("layout").and_then(Value::as_object_mut) {
        let existing_spread = layout
            .get("spreads")
            .and_then(Value::as_array)
            .and_then(|spreads| {
                spreads.iter().find(|spread| {
                    spread
                        .get("pages")
                        .and_then(Value::as_array)
                        .is_some_and(|pages| {
                            pages.iter().any(|page| page.as_str() == Some(&page_id))
                        })
                })
            })
            .cloned();
        let mut plate_spread = existing_spread.unwrap_or_else(
            || serde_json::json!({"id":"packaging-plate","facing":false,"pages":[page_id]}),
        );
        plate_spread["facing"] = serde_json::json!(false);
        plate_spread["pages"] = serde_json::json!([page_id]);
        layout.insert("spreads".to_owned(), serde_json::json!([plate_spread]));
    }
    planned["dielines"] = serde_json::json!({});
    crate::scene::validate(&planned)?;
    let separated = crate::color::separation::separate(
        &planned,
        document,
        Some(&page_id),
        &crate::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )?;
    let roles = ink_roles
        .into_iter()
        .map(|(ink, roles)| serde_json::json!({"ink":ink,"roles":roles}))
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "dieline":dieline_id,"format":"png-gray-spot-separations",
        "requested_ppi":ppi,
        "effective_ppi":{"x":f64::from(separated.width)/width_mm*25.4,"y":f64::from(separated.height)/height_mm*25.4},
        "pixels":{"width":separated.width,"height":separated.height},
        "spot_roles":roles,"process_plates":"blank",
        "manufacturing_certified":false,
        "limitations":["spot paths are vector-rasterized at the requested resolution","board thickness adds panel interiors, exposed cut-edge walls, and visual interior bridges at folded hinges; score compression and manufacturing allowances are not modeled","structural production checks are not performed"],
        "separations":separated.json()
    });
    Ok((separated, report))
}

/// Atomically write a self-contained directory of grayscale spot plates and
/// their manifest. Existing destinations are never replaced.
pub fn write_dieline_spot_separations(
    raw: &Value,
    document: &Path,
    dieline_id: &str,
    out: &Path,
    ppi: f64,
    bits: u8,
    source_revision: &str,
) -> Result<Value> {
    ensure!(
        !out.exists(),
        "[output-exists] spot separation directory {} already exists",
        out.display()
    );
    let separated_result = dieline_spot_separations(raw, document, dieline_id, ppi);
    if separated_result.as_ref().is_err_and(|error| {
        error
            .to_string()
            .contains("plate memory exceeds the working budget")
    }) {
        return write_dieline_spot_separations_streamed(
            raw,
            dieline_id,
            out,
            ppi,
            bits,
            source_revision,
        );
    }
    let (separated, mut report) = separated_result?;
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .context("[missing-resource] spot separation output parent must exist")?;
    let name = out
        .file_name()
        .context("[invalid-input] spot separation output needs a directory name")?;
    let out_path = parent.join(name);
    ensure!(
        !out_path.exists(),
        "[output-exists] spot separation directory already exists"
    );
    static STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut stage = None;
    for _ in 0..128 {
        let candidate = parent.join(format!(
            ".{}.pentool-pack-separation-{}-{}",
            name.to_string_lossy(),
            std::process::id(),
            STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                stage = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let stage =
        stage.context("[conflict] could not reserve a temporary spot-separation directory")?;
    let write = (|| -> Result<Value> {
        let mut files = separated.write_with_depth(&stage, bits)?;
        report["document_revision"] = serde_json::json!(source_revision);
        report["files"] = files["files"].clone();
        files = report.clone();
        let mut bytes = serde_json::to_vec_pretty(&files)?;
        bytes.push(b'\n');
        crate::editing::atomic_write(&stage.join("separations.json"), &bytes)?;
        ensure!(
            !out_path.exists(),
            "[output-exists] spot separation directory already exists"
        );
        std::fs::rename(&stage, &out_path)
            .with_context(|| format!("move spot plates into place at {}", out_path.display()))?;
        report["out"] = serde_json::json!(out_path);
        Ok(report)
    })();
    if write.is_err() && stage.exists() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    write
}

/// Rasterize packaging finishing artwork one spot ink at a time when the
/// general-purpose color separator's all-plates-in-memory representation
/// would exceed its bounded work surface. The vector SVG remains normative;
/// these grayscale PNGs are convenience previews/plates at the requested PPI.
fn write_dieline_spot_separations_streamed(
    raw: &Value,
    dieline_id: &str,
    out: &Path,
    ppi: f64,
    bits: u8,
    source_revision: &str,
) -> Result<Value> {
    ensure!(
        bits == 8 || bits == 16,
        "[invalid-input] plate bit depth must be 8 or 16, not {bits}"
    );
    ensure!(
        ppi.is_finite() && (36.0..=1200.0).contains(&ppi),
        "[invalid-input] packaging plate resolution must be within 36..=1200 ppi"
    );
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .context("[missing-resource] spot separation output parent must exist")?;
    let name = out
        .file_name()
        .context("[invalid-input] spot separation output needs a directory name")?;
    let out_path = parent.join(name);
    ensure!(
        !out_path.exists(),
        "[output-exists] spot separation directory already exists"
    );
    static STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut stage = None;
    for _ in 0..128 {
        let candidate = parent.join(format!(
            ".{}.pentool-pack-separation-{}-{}",
            name.to_string_lossy(),
            std::process::id(),
            STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                stage = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let stage =
        stage.context("[conflict] could not reserve a temporary spot-separation directory")?;
    let result = (|| -> Result<Value> {
        crate::scene::validate(raw)?;
        let dieline = dieline_from_document(raw, dieline_id)?;
        let vector_dir = stage.join(".vector-source");
        let vector_report = write_dieline_vector_spot_plates(raw, dieline_id, &vector_dir)?;
        let width_mm = vector_report["width_mm"]
            .as_f64()
            .context("[internal] vector spot report has no width")?;
        let height_mm = vector_report["height_mm"]
            .as_f64()
            .context("[internal] vector spot report has no height")?;
        let dimension = |mm: f64| -> Result<u32> {
            let px = (mm * ppi / 25.4).round();
            ensure!(
                px.is_finite() && (1.0..=u32::MAX as f64).contains(&px),
                "[limit-exceeded] packaging plate dimensions exceed the supported pixel range"
            );
            Ok(px as u32)
        };
        let (width, height) = (dimension(width_mm)?, dimension(height_mm)?);
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .context("[limit-exceeded] packaging plate pixel count overflow")?;
        let plates = vector_report["plates"]
            .as_array()
            .context("[internal] vector plate report has no plates")?;
        let aggregate = pixels
            .checked_mul(plates.len() as u64)
            .context("[limit-exceeded] aggregate packaging plate work overflow")?;
        const MAX_PACKAGING_PLATE_WORK_PIXELS: u64 = 268_435_456;
        ensure!(
            aggregate <= MAX_PACKAGING_PLATE_WORK_PIXELS,
            "[limit-exceeded] aggregate packaging raster-plate work exceeds {MAX_PACKAGING_PLATE_WORK_PIXELS} pixels; use vector spot plates or a lower raster PPI"
        );
        let prefix = std::fs::read_dir(&vector_dir)?;
        let mut vector_files = prefix
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        vector_files.sort();
        ensure!(
            vector_files.len() == plates.len(),
            "[internal] vector plate file count does not match its manifest"
        );
        // The usual four process plates are intentionally blank for packaging
        // operations. Reuse one tiny encoded white PNG across their filenames.
        let white = image::GrayImage::from_pixel(width, height, image::Luma([255]));
        let mut white_bytes = Vec::new();
        if bits == 16 {
            let white16 = image::ImageBuffer::from_pixel(width, height, image::Luma([u16::MAX]));
            white16.write_to(
                &mut std::io::Cursor::new(&mut white_bytes),
                image::ImageFormat::Png,
            )?;
        } else {
            white.write_to(
                &mut std::io::Cursor::new(&mut white_bytes),
                image::ImageFormat::Png,
            )?;
        }
        drop(white);
        let mut files = Vec::with_capacity(4 + plates.len());
        let mut plate_reports = Vec::with_capacity(4 + plates.len());
        for (index, id) in ["C", "M", "Y", "K"].into_iter().enumerate() {
            let file = format!("plate-{index:02}-{id}.png");
            crate::editing::atomic_write(&stage.join(&file), &white_bytes)?;
            files.push(serde_json::json!({"plate":id,"file":file,"bits":bits,"sha256":crate::resource::sha256(&white_bytes)}));
            plate_reports.push(serde_json::json!({"index":index,"id":id,"kind":"process","inked_pixels":0,"max":0.0}));
        }
        let mut spot_roles = Vec::with_capacity(plates.len());
        let mut inks = Vec::with_capacity(plates.len());
        let mut overprints = 0usize;
        let mut objects = 0usize;
        let mut alternate_by_ink = std::collections::BTreeMap::<String, [f64; 3]>::new();
        let mut roles_by_ink =
            std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        let paths = dieline["paths"]
            .as_array()
            .context("dieline paths missing")?;
        for path in paths {
            let role = path["role"].as_str().context("production role missing")?;
            let (default_ink, alternate) = spot_for_dieline_role(role)?;
            let ink = path["spot"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default_ink);
            alternate_by_ink.entry(ink.to_owned()).or_insert(alternate);
            roles_by_ink
                .entry(ink.to_owned())
                .or_default()
                .insert(role.to_owned());
            objects += 1;
            overprints += 1;
        }
        for crease in dieline["creases"]
            .as_array()
            .context("dieline creases missing")?
        {
            let id = crease["id"].as_str().context("crease ID missing")?;
            let points = crease["line_mm"]
                .as_array()
                .filter(|points| points.len() == 2)
                .context("crease line must have two points")?;
            let line = [
                pair(&points[0], "crease start")?,
                pair(&points[1], "crease end")?,
            ];
            if !crease_has_production_path(paths, line) {
                let (ink, alternate) = spot_for_dieline_role("crease")?;
                alternate_by_ink.entry(ink.to_owned()).or_insert(alternate);
                roles_by_ink
                    .entry(ink.to_owned())
                    .or_default()
                    .insert("crease".into());
                objects += 1;
                overprints += 1;
            }
            let _ = id;
        }
        for panel in dieline["panels"]
            .as_array()
            .context("dieline panels missing")?
        {
            if panel["safe_zone_mm"].as_f64().unwrap_or(0.0) > 0.0 {
                alternate_by_ink
                    .entry("SafeZone".into())
                    .or_insert([0.0, 0.65, 0.1]);
                roles_by_ink
                    .entry("SafeZone".into())
                    .or_default()
                    .insert("safe-zone".into());
                objects += 1;
                overprints += 1;
            }
        }
        for (offset, (entry, svg_path)) in plates.iter().zip(vector_files).enumerate() {
            let ink = entry["ink"].as_str().context("vector plate ink missing")?;
            let svg_bytes = std::fs::read(&svg_path)?;
            ensure!(
                svg_bytes.len() <= MAX_FLAT_DIELINE_SVG_BYTES,
                "[limit-exceeded] vector finishing plate exceeds the 64 MiB raster input limit"
            );
            let svg =
                std::str::from_utf8(&svg_bytes).context("vector finishing plate is not UTF-8")?;
            let open = svg
                .find('>')
                .context("vector finishing plate has no root")?;
            let close = svg
                .rfind("</svg>")
                .context("vector finishing plate has no closing root")?;
            let rendered = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width_mm} {height_mm}\"><rect width=\"{width_mm}\" height=\"{height_mm}\" fill=\"#ffffff\"/>{}</svg>",
                &svg[open + 1..close]
            );
            let encoded =
                crate::render::svg_to_png(&rendered, width, height, 1.0).with_context(|| {
                    format!("[render-failed] could not rasterize `{ink}` finishing plate")
                })?;
            let rgba = image::load_from_memory(&encoded)
                .context("[render-failed] could not decode rasterized finishing plate")?;
            drop(encoded);
            let gray = rgba.to_luma8();
            drop(rgba);
            let inked_pixels = gray.pixels().filter(|pixel| pixel[0] < 255).count();
            let max = gray.pixels().map(|pixel| 255 - pixel[0]).max().unwrap_or(0) as f64 / 255.0;
            let mut png = Vec::new();
            if bits == 16 {
                let gray16 = image::ImageBuffer::from_fn(width, height, |x, y| {
                    image::Luma([u16::from(gray.get_pixel(x, y)[0]) * 257])
                });
                gray16.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            } else {
                gray.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            }
            drop(gray);
            let file_index = 4 + offset;
            let file = format!("plate-{file_index:02}-spot.png");
            crate::editing::atomic_write(&stage.join(&file), &png)?;
            files.push(serde_json::json!({"plate":ink,"file":file,"bits":bits,"sha256":crate::resource::sha256(&png)}));
            plate_reports.push(serde_json::json!({"index":file_index,"id":ink,"kind":"spot","inked_pixels":inked_pixels,"max":max}));
            let roles = roles_by_ink.get(ink).cloned().unwrap_or_default();
            spot_roles.push(serde_json::json!({"ink":ink,"roles":roles}));
            let alternate = alternate_by_ink
                .get(ink)
                .copied()
                .unwrap_or([0.0, 0.0, 0.0]);
            inks.push(serde_json::json!({"id":ink,"name":format!("Packaging {ink}"),"alternate":{"space":"srgb","channels":alternate},"redistribution":"allowed"}));
        }
        std::fs::remove_dir_all(&vector_dir)?;
        let separations = serde_json::json!({
            "version":1,"scope":"plate-separations","width":width,"height":height,"scale":1.0,
            "destination":null,"intent":null,"objects":objects,"flattened_scene3d_objects":0,"warnings":[],
            "knockouts":0,"overprints":overprints,"clipped_paints":0,"traps":0,"auto_trapped_pixels":0,
            "marks":0,"color_bar_patches":0,"slug_text":0,"plates":plate_reports,"inks":inks
        });
        let mut report = serde_json::json!({
            "dieline":dieline_id,"format":"png-gray-spot-separations","requested_ppi":ppi,
            "effective_ppi":{"x":f64::from(width)/width_mm*25.4,"y":f64::from(height)/height_mm*25.4},
            "pixels":{"width":width,"height":height},"spot_roles":spot_roles,"process_plates":"blank",
            "manufacturing_certified":false,
            "limitations":["spot paths are vector-rasterized at the requested resolution","board thickness adds panel interiors, exposed cut-edge walls, and visual interior bridges at folded hinges; score compression and manufacturing allowances are not modeled","structural production checks are not performed"],
            "separations":separations,"document_revision":source_revision,"bits":bits,"files":files
        });
        let mut manifest = serde_json::to_vec_pretty(&report)?;
        manifest.push(b'\n');
        crate::editing::atomic_write(&stage.join("separations.json"), &manifest)?;
        ensure!(
            !out_path.exists(),
            "[output-exists] spot separation directory already exists"
        );
        std::fs::rename(&stage, &out_path)
            .with_context(|| format!("move spot plates into place at {}", out_path.display()))?;
        report["out"] = serde_json::json!(out_path);
        Ok(report)
    })();
    if result.is_err() && stage.exists() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}

fn folded_hinge_checks(dieline: &Value, model: &Model) -> Result<Vec<Value>> {
    let template = dieline["template"].as_str().unwrap_or_default();
    if matches!(template, "tube" | "bottle-label") {
        ensure!(
            model.primitives.len() >= TUBE_SEGMENTS,
            "[invalid-input] cylindrical wrap generated too few surface segments"
        );
        let first = &model.primitives[0].positions;
        let last = &model.primitives[TUBE_SEGMENTS - 1].positions;
        let gap = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let max_gap = f64::from(gap(first[0], last[1]).max(gap(first[3], last[2])));
        let radius = dieline["wrap"]["radius_mm"].as_f64().unwrap_or(1.0);
        let tolerance = (radius * 1e-6).max(1e-4);
        return Ok(vec![serde_json::json!({
            "kind":"wrap-seam", "panels":[dieline["panels"][0]["id"]],
            "gap_mm":max_gap, "tolerance_mm":tolerance, "joined":max_gap <= tolerance
        })]);
    }

    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels must be an array")?;
    let rects = panels
        .iter()
        .map(|panel| rect(&panel["rect_mm"], "panel rect_mm", false))
        .collect::<Result<Vec<_>>>()?;
    let scale = rects
        .iter()
        .map(|rect| rect[2].max(rect[3]))
        .fold(1.0_f64, f64::max);
    let tolerance = (scale * 1e-6).max(1e-4);
    let distance = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let crease_lines = dieline["creases"]
        .as_array()
        .map(|creases| {
            creases
                .iter()
                .filter_map(|crease| {
                    let line = crease["line_mm"].as_array()?;
                    Some([
                        pair(&line[0], "crease start").ok()?,
                        pair(&line[1], "crease end").ok()?,
                    ])
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut checks = Vec::new();
    for first in 0..rects.len() {
        for second in first + 1..rects.len() {
            let Some(edge) = panel_hinged(rects[first], rects[second], &crease_lines) else {
                continue;
            };
            // A hinge may cover only part of a larger panel's edge (inset dust
            // flaps), so a non-corner endpoint is interpolated along its edge.
            let hinge_point = |index: usize, target: [f64; 2]| -> Result<[f32; 3]> {
                let points = if let Some(path) = panel_cut_contour(dieline, &panels[index]) {
                    flatten_panel_contour(
                        path["d"].as_str().unwrap_or_default(),
                        panels[index]["id"].as_str().unwrap_or("?"),
                    )?
                } else {
                    let [x, y, width, height] = rects[index];
                    vec![
                        [x, y],
                        [x + width, y],
                        [x + width, y + height],
                        [x, y + height],
                    ]
                };
                let positions = &model.primitives[index].positions;
                if let Some(vertex) = points.iter().position(|point| {
                    (point[0] - target[0]).abs() <= 1e-8 && (point[1] - target[1]).abs() <= 1e-8
                }) {
                    return Ok(positions[vertex]);
                }
                for start in 0..points.len() {
                    let end = (start + 1) % points.len();
                    if !point_on_segment(target, [points[start], points[end]]) {
                        continue;
                    }
                    let span = (points[end][0] - points[start][0])
                        .hypot(points[end][1] - points[start][1]);
                    let along = (target[0] - points[start][0]).hypot(target[1] - points[start][1]);
                    let t = (along / span) as f32;
                    let (p, q) = (positions[start], positions[end]);
                    return Ok([
                        p[0] + (q[0] - p[0]) * t,
                        p[1] + (q[1] - p[1]) * t,
                        p[2] + (q[2] - p[2]) * t,
                    ]);
                }
                bail!("[invalid-input] shared panel edge endpoint is not on the panel boundary")
            };
            let first_points = [hinge_point(first, edge[0])?, hinge_point(first, edge[1])?];
            let second_points = [hinge_point(second, edge[0])?, hinge_point(second, edge[1])?];
            let direct = distance(first_points[0], second_points[0])
                .max(distance(first_points[1], second_points[1]));
            let reverse = distance(first_points[0], second_points[1])
                .max(distance(first_points[1], second_points[0]));
            let gap = f64::from(direct.min(reverse));
            checks.push(serde_json::json!({
                "kind":"hinge", "panels":[panels[first]["id"],panels[second]["id"]],
                "gap_mm":gap, "tolerance_mm":tolerance, "joined":gap <= tolerance
            }));
        }
    }
    Ok(checks)
}

const MAX_FOLD_COLLISION_PAIRS: usize = 32_768;
const MAX_REPORTED_FOLD_COLLISIONS: usize = 256;
const MAX_FOLD_COLLISION_TRIANGLE_PAIRS: u64 = 1_000_000;
const MAX_DIELINE_MESH_VERTICES: usize = 2_000_000;

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

fn project_triangle(point: [f64; 3], drop_axis: usize) -> [f64; 2] {
    match drop_axis {
        0 => [point[1], point[2]],
        1 => [point[0], point[2]],
        _ => [point[0], point[1]],
    }
}

fn orient2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn inside_triangle_2d(point: [f64; 2], triangle: [[f64; 2]; 3], epsilon: f64) -> bool {
    let area = orient2(triangle[0], triangle[1], triangle[2]);
    if area.abs() <= epsilon {
        return false;
    }
    let a = orient2(triangle[0], triangle[1], point) / area;
    let b = orient2(triangle[1], triangle[2], point) / area;
    let c = orient2(triangle[2], triangle[0], point) / area;
    a > epsilon && b > epsilon && c > epsilon
}

fn proper_segments_intersect_2d(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
    epsilon: f64,
) -> bool {
    let ab_c = orient2(a, b, c);
    let ab_d = orient2(a, b, d);
    let cd_a = orient2(c, d, a);
    let cd_b = orient2(c, d, b);
    (ab_c > epsilon && ab_d < -epsilon || ab_c < -epsilon && ab_d > epsilon)
        && (cd_a > epsilon && cd_b < -epsilon || cd_a < -epsilon && cd_b > epsilon)
}

fn coplanar_triangles_overlap(a: [[f64; 3]; 3], b: [[f64; 3]; 3], normal: [f64; 3]) -> bool {
    let drop_axis = (0..3)
        .max_by(|&left, &right| normal[left].abs().total_cmp(&normal[right].abs()))
        .unwrap_or(2);
    let a = a.map(|point| project_triangle(point, drop_axis));
    let b = b.map(|point| project_triangle(point, drop_axis));
    let epsilon = 1e-10;
    if a.iter().any(|point| inside_triangle_2d(*point, b, epsilon))
        || b.iter().any(|point| inside_triangle_2d(*point, a, epsilon))
    {
        return true;
    }
    for ai in 0..3 {
        for bi in 0..3 {
            if proper_segments_intersect_2d(a[ai], a[(ai + 1) % 3], b[bi], b[(bi + 1) % 3], epsilon)
            {
                return true;
            }
        }
    }
    let centroid = |triangle: [[f64; 2]; 3]| {
        [
            (triangle[0][0] + triangle[1][0] + triangle[2][0]) / 3.0,
            (triangle[0][1] + triangle[1][1] + triangle[2][1]) / 3.0,
        ]
    };
    inside_triangle_2d(centroid(a), b, epsilon) || inside_triangle_2d(centroid(b), a, epsilon)
}

fn point_strictly_inside_triangle(point: [f64; 3], triangle: [[f64; 3]; 3]) -> bool {
    let v0 = sub3(triangle[1], triangle[0]);
    let v1 = sub3(triangle[2], triangle[0]);
    let v2 = sub3(point, triangle[0]);
    let d00 = dot3(v0, v0);
    let d01 = dot3(v0, v1);
    let d11 = dot3(v1, v1);
    let d20 = dot3(v2, v0);
    let d21 = dot3(v2, v1);
    let denominator = d00 * d11 - d01 * d01;
    if denominator <= 1e-20 {
        return false;
    }
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    let u = 1.0 - v - w;
    u > 1e-9 && v > 1e-9 && w > 1e-9
}

fn segment_crosses_triangle(start: [f64; 3], end: [f64; 3], triangle: [[f64; 3]; 3]) -> bool {
    let normal = cross3(
        sub3(triangle[1], triangle[0]),
        sub3(triangle[2], triangle[0]),
    );
    let normal_length = len3(normal);
    if normal_length <= 1e-12 {
        return false;
    }
    let from = dot3(normal, sub3(start, triangle[0]));
    let to = dot3(normal, sub3(end, triangle[0]));
    if (from > 0.0 && to > 0.0) || (from < 0.0 && to < 0.0) || (from - to).abs() <= 1e-14 {
        return false;
    }
    let t = from / (from - to);
    if !(-1e-10..=1.0 + 1e-10).contains(&t) {
        return false;
    }
    let point = [
        start[0] + (end[0] - start[0]) * t,
        start[1] + (end[1] - start[1]) * t,
        start[2] + (end[2] - start[2]) * t,
    ];
    point_strictly_inside_triangle(point, triangle)
}

fn triangles_have_interior_intersection(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> bool {
    let normal_a = cross3(sub3(a[1], a[0]), sub3(a[2], a[0]));
    let normal_b = cross3(sub3(b[1], b[0]), sub3(b[2], b[0]));
    let length_a = len3(normal_a);
    let length_b = len3(normal_b);
    if length_a <= 1e-12 || length_b <= 1e-12 {
        return false;
    }
    let normals_cross = len3(cross3(normal_a, normal_b));
    let parallel = normals_cross <= length_a * length_b * 1e-9;
    let plane_gap = dot3(normal_a, sub3(b[0], a[0])).abs() / length_a;
    if parallel {
        return plane_gap <= 1e-8
            && dot3(normal_a, normal_b) >= 0.0
            && coplanar_triangles_overlap(a, b, normal_a);
    }
    for edge in 0..3 {
        if segment_crosses_triangle(a[edge], a[(edge + 1) % 3], b)
            || segment_crosses_triangle(b[edge], b[(edge + 1) % 3], a)
        {
            return true;
        }
    }
    false
}

fn triangle_aabbs_overlap(a: [[f64; 3]; 3], b: [[f64; 3]; 3], tolerance: f64) -> bool {
    (0..3).all(|axis| {
        let a_min = a
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min);
        let a_max = a
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        let b_min = b
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min);
        let b_max = b
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        a_max >= b_min - tolerance && b_max >= a_min - tolerance
    })
}

fn triangles_overlap_with_thickness(
    first: [[f64; 3]; 3],
    second: [[f64; 3]; 3],
    thickness: f64,
) -> bool {
    if !thickness.is_finite() || thickness <= 0.0 {
        return false;
    }
    let first_normal = cross3(sub3(first[1], first[0]), sub3(first[2], first[0]));
    let first_length = len3(first_normal);
    if first_length <= 1e-12 {
        return false;
    }
    let normal = first_normal.map(|component| component / first_length);
    let magnitude = first
        .iter()
        .chain(&second)
        .flatten()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    let tolerance = magnitude * 1e-8;
    let distances = second.map(|point| dot3(normal, sub3(point, first[0])));
    if distances
        .iter()
        .any(|distance| distance.abs() > thickness + tolerance)
    {
        return false;
    }
    // A separation exactly one sheet thickness is face contact between two
    // layers, not positive-volume overlap. Retain a scale-aware tolerance so
    // numeric noise at the designed contact plane does not turn it into a
    // collision.
    if distances
        .iter()
        .all(|distance| distance.abs() >= thickness - tolerance)
    {
        return false;
    }
    let aligned_second = second.map(|point| {
        let distance = dot3(normal, sub3(point, first[0]));
        [
            point[0] - distance * normal[0],
            point[1] - distance * normal[1],
            point[2] - distance * normal[2],
        ]
    });
    let drop_axis = (0..3)
        .max_by(|&left, &right| normal[left].abs().total_cmp(&normal[right].abs()))
        .unwrap_or(2);
    let first_2d = first.map(|point| project_triangle(point, drop_axis));
    let second_2d = aligned_second.map(|point| project_triangle(point, drop_axis));
    let as_3d = |point: [f64; 2]| [point[0], point[1], 0.0];
    let overlap =
        coplanar_triangles_overlap(first_2d.map(as_3d), second_2d.map(as_3d), [0.0, 0.0, 1.0]);
    let second_normal = cross3(sub3(second[1], second[0]), sub3(second[2], second[0]));
    let face_contact = distances.iter().all(|distance| distance.abs() <= tolerance)
        && dot3(first_normal, second_normal) < 0.0;
    overlap && !face_contact
}

#[cfg(test)]
fn panel_quads_intersect(first: [[f64; 3]; 4], second: [[f64; 3]; 4]) -> bool {
    let magnitude = first
        .iter()
        .chain(&second)
        .flatten()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    let tolerance = magnitude * 1e-8;
    for axis in 0..3 {
        let first_min = first
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min);
        let first_max = first
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        let second_min = second
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min);
        let second_max = second
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        // Equality is allowed here because coplanar faces have zero extent on
        // their normal axis; the narrow phase rejects edge-only contact.
        if first_max < second_min - tolerance || second_max < first_min - tolerance {
            return false;
        }
    }
    let triangles = [[0, 1, 2], [0, 2, 3]];
    triangles.into_iter().any(|first_triangle| {
        triangles.into_iter().any(|second_triangle| {
            triangles_have_interior_intersection(
                first_triangle.map(|index| first[index]),
                second_triangle.map(|index| second[index]),
            )
        })
    })
}

/// Conservatively flag panel faces whose interiors overlap after projection
/// onto one face and whose plane separation stays within one board thickness.
/// This is a clearance screen, not a solid-board or crease-compression
/// simulation.
#[cfg(test)]
fn panels_overlap_with_thickness(
    first: [[f64; 3]; 4],
    second: [[f64; 3]; 4],
    thickness: f64,
) -> bool {
    if !thickness.is_finite() || thickness <= 0.0 {
        return false;
    }
    let first_normal = cross3(sub3(first[1], first[0]), sub3(first[2], first[0]));
    let first_length = len3(first_normal);
    if first_length <= 1e-12 {
        return false;
    }
    let normal = first_normal.map(|component| component / first_length);
    let magnitude = first
        .iter()
        .chain(&second)
        .flatten()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    let tolerance = magnitude * 1e-8;
    let distances = second.map(|point| dot3(normal, sub3(point, first[0])));
    if distances
        .iter()
        .any(|distance| distance.abs() > thickness + tolerance)
    {
        return false;
    }
    if distances
        .iter()
        .all(|distance| distance.abs() >= thickness - tolerance)
    {
        return false;
    }

    // Orthogonally project onto the first face's plane. Since signed plane
    // distance varies linearly over a planar face, the vertex bound above
    // constrains its full projected footprint.
    let aligned_second = second.map(|point| {
        let distance = dot3(normal, sub3(point, first[0]));
        [
            point[0] - distance * normal[0],
            point[1] - distance * normal[1],
            point[2] - distance * normal[2],
        ]
    });
    let drop_axis = (0..3)
        .max_by(|&left, &right| normal[left].abs().total_cmp(&normal[right].abs()))
        .unwrap_or(2);
    let first_2d = first.map(|point| project_triangle(point, drop_axis));
    let second_2d = aligned_second.map(|point| project_triangle(point, drop_axis));
    let as_3d = |point: [f64; 2]| [point[0], point[1], 0.0];
    let triangles = [[0, 1, 2], [0, 2, 3]];
    triangles.into_iter().any(|first_triangle| {
        triangles.into_iter().any(|second_triangle| {
            coplanar_triangles_overlap(
                first_triangle.map(|index| as_3d(first_2d[index])),
                second_triangle.map(|index| as_3d(second_2d[index])),
                [0.0, 0.0, 1.0],
            )
        })
    })
}

fn folded_collision_report(dieline: &Value, model: &Model) -> Result<Value> {
    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels must be an array")?;
    let fold_state = dieline["fold_state"].as_f64().unwrap_or(1.0);
    let board_thickness = dieline["dimensions_mm"]["board"].as_f64().unwrap_or(0.0);
    if matches!(dieline["template"].as_str(), Some("tube" | "bottle-label")) {
        // A wrapped label is tessellated into many mesh segments but remains
        // one dieline face. Its seam continuity is reported separately.
        return Ok(serde_json::json!({
            "status":"checked-clear", "fold_state":fold_state,
            "method":"bounded zero-thickness triangle-interior intersection",
            "pairs_tested":0, "collision_count":0, "collisions":[],
            "collisions_omitted":0, "physical_thickness_tested":false,
            "board_thickness_mm":board_thickness
        }));
    }
    ensure!(
        panels.len() <= model.primitives.len(),
        "[invalid-composite] folded panel mesh is missing one or more dieline faces"
    );
    let total_pairs = panels.len().saturating_mul(panels.len().saturating_sub(1)) / 2;
    ensure!(
        total_pairs <= MAX_FOLD_COLLISION_PAIRS,
        "[limit-exceeded] folded collision scan needs {total_pairs} pairs; limit is {MAX_FOLD_COLLISION_PAIRS}"
    );
    let panel_triangles = model
        .primitives
        .iter()
        .take(panels.len())
        .zip(panels)
        .map(|(primitive, panel)| -> Result<Vec<[[f64; 3]; 3]>> {
            ensure!(
                primitive.indices.len() % 3 == 0,
                "[invalid-composite] folded panel `{}` has an incomplete triangle index list",
                panel["id"]
            );
            primitive
                .indices
                .chunks_exact(3)
                .map(|triangle| {
                    let mut points = [[0.0; 3]; 3];
                    for corner in 0..3 {
                        let position = primitive
                            .positions
                            .get(triangle[corner] as usize)
                            .with_context(|| {
                                format!(
                                    "[invalid-composite] folded panel `{}` has an out-of-range triangle index",
                                    panel["id"]
                                )
                            })?;
                        points[corner] = position.map(f64::from);
                    }
                    Ok(points)
                })
                .collect()
        })
        .collect::<Result<Vec<_>>>()?;
    let triangle_work = (0..panel_triangles.len())
        .flat_map(|first| {
            let panel_triangles = &panel_triangles;
            (first + 1..panel_triangles.len()).map(move |second| {
                panel_triangles[first].len() as u64 * panel_triangles[second].len() as u64
            })
        })
        .try_fold(0_u64, |sum, pairs| sum.checked_add(pairs))
        .context("[limit-exceeded] folded triangle-pair work count overflow")?;
    ensure!(
        triangle_work <= MAX_FOLD_COLLISION_TRIANGLE_PAIRS,
        "[limit-exceeded] folded collision scan needs {triangle_work} triangle pairs; limit is {MAX_FOLD_COLLISION_TRIANGLE_PAIRS}"
    );
    let mut pairs_tested = 0usize;
    let mut triangle_pairs_tested = 0_u64;
    let mut collision_count = 0usize;
    let mut collisions = Vec::new();
    for first in 0..panels.len() {
        for second in first + 1..panels.len() {
            pairs_tested += 1;
            let mut collision_kind = None;
            'triangles: for first_triangle in &panel_triangles[first] {
                for second_triangle in &panel_triangles[second] {
                    triangle_pairs_tested += 1;
                    let magnitude = first_triangle
                        .iter()
                        .chain(second_triangle)
                        .flatten()
                        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
                    if !triangle_aabbs_overlap(
                        *first_triangle,
                        *second_triangle,
                        magnitude * 1.0e-8 + board_thickness,
                    ) {
                        continue;
                    }
                    if triangles_have_interior_intersection(*first_triangle, *second_triangle) {
                        collision_kind = Some("zero-thickness-intersection");
                        break 'triangles;
                    }
                    if triangles_overlap_with_thickness(
                        *first_triangle,
                        *second_triangle,
                        board_thickness,
                    ) {
                        collision_kind = Some("board-thickness-intersection");
                        break 'triangles;
                    }
                }
            }
            if let Some(kind) = collision_kind {
                collision_count += 1;
                if collisions.len() < MAX_REPORTED_FOLD_COLLISIONS {
                    collisions.push(serde_json::json!({
                        "panels":[panels[first]["id"],panels[second]["id"]],
                        "kind":kind
                    }));
                }
            }
        }
    }
    Ok(serde_json::json!({
        "status":if collision_count == 0 {"checked-clear"} else {"potential-overlaps"},
        "fold_state":fold_state,
        "method":"bounded triangle intersection plus board-thickness plane-distance clearance screen",
        "pairs_tested":pairs_tested,
        "triangle_pairs_tested":triangle_pairs_tested,
        "collision_count":collision_count,
        "collisions":collisions,
        "collisions_omitted":collision_count.saturating_sub(collisions.len()),
        "physical_thickness_tested":board_thickness > 0.0,
        "board_thickness_mm":board_thickness
    }))
}

/// Report basic mockup preflight findings without mutating the document or
/// claiming that the visual net is safe for manufacturing.
pub fn preflight(raw: &Value, document: &Path, dieline_id: &str, min_ppi: f64) -> Result<Value> {
    crate::scene::validate(raw)?;
    ensure!(
        min_ppi.is_finite() && (1.0..=2400.0).contains(&min_ppi),
        "[invalid-input] minimum artwork ppi must be within 1..=2400"
    );
    let pdfx_output_role = match crate::color::roles::effective(
        raw,
        crate::color::roles::Role::Output,
    ) {
        Ok(setting) => {
            let destination = setting.destination;
            let profile = destination.profile().map(str::to_owned);
            let configured = destination.space() == "icc-cmyk" && profile.is_some();
            serde_json::json!({
                "configured": configured,
                "space": destination.space(),
                "profile": profile,
                "message": if configured { Value::Null } else { serde_json::json!("PDF/X packaging export requires an ICC CMYK output role") }
            })
        }
        Err(error) => serde_json::json!({
            "configured":false,
            "space":Value::Null,
            "profile":Value::Null,
            "message":error.to_string()
        }),
    };
    let resolved_dieline = dieline_from_document(raw, dieline_id)?;
    let dieline = &resolved_dieline;
    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels must be an array")?;
    let paths = dieline["paths"]
        .as_array()
        .context("[invalid-input] dieline paths must be an array")?;
    let panel_art_bound = panels
        .iter()
        .filter(|panel| panel.get("artwork").is_some())
        .count();
    let panels_without_artwork: Vec<&str> = panels
        .iter()
        .filter(|panel| panel.get("artwork").is_none())
        .filter_map(|panel| panel.get("id").and_then(Value::as_str))
        .collect();
    let roles: std::collections::BTreeSet<&str> = paths
        .iter()
        .filter_map(|path| path.get("role").and_then(Value::as_str))
        .collect();
    let mut warnings = Vec::new();
    let mut panel_bounds = [f64::INFINITY, f64::INFINITY, 0.0, 0.0];
    let mut panel_rects = Vec::with_capacity(panels.len());
    for panel in panels {
        let rect = panel["rect_mm"]
            .as_array()
            .context("[invalid-input] panel rect_mm is missing")?;
        let [x, y, width, height] = [
            rect[0].as_f64().unwrap_or(0.0),
            rect[1].as_f64().unwrap_or(0.0),
            rect[2].as_f64().unwrap_or(0.0),
            rect[3].as_f64().unwrap_or(0.0),
        ];
        panel_bounds[0] = panel_bounds[0].min(x);
        panel_bounds[1] = panel_bounds[1].min(y);
        panel_bounds[2] = panel_bounds[2].max(x + width);
        panel_bounds[3] = panel_bounds[3].max(y + height);
        panel_rects.push([x, y, width, height]);
    }
    let shared_edge = |a: [f64; 4], b: [f64; 4]| {
        let tolerance = a
            .into_iter()
            .chain(b)
            .fold(1.0_f64, |scale, value| scale.max(value.abs()))
            * 1e-8;
        let vertical =
            ((a[0] + a[2]) - b[0]).abs() <= tolerance || ((b[0] + b[2]) - a[0]).abs() <= tolerance;
        let horizontal =
            ((a[1] + a[3]) - b[1]).abs() <= tolerance || ((b[1] + b[3]) - a[1]).abs() <= tolerance;
        (vertical && a[1].max(b[1]) < (a[1] + a[3]).min(b[1] + b[3]) - tolerance)
            || (horizontal && a[0].max(b[0]) < (a[0] + a[2]).min(b[0] + b[2]) - tolerance)
    };
    let mut visited = vec![false; panels.len()];
    let mut connected_components = 0usize;
    for root in 0..panels.len() {
        if visited[root] {
            continue;
        }
        connected_components += 1;
        visited[root] = true;
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            for other in 0..panels.len() {
                if !visited[other] && shared_edge(panel_rects[current], panel_rects[other]) {
                    visited[other] = true;
                    stack.push(other);
                }
            }
        }
    }
    let built_in_net = matches!(
        dieline["template"].as_str(),
        Some("straight-tuck" | "reverse-tuck" | "sleeve" | "mailer" | "pouch")
    );
    if built_in_net && connected_components != 1 {
        warnings.push(format!("built-in package panels form {connected_components} disconnected flat-layout components; each panel must share an edge-connected net"));
    }
    let connectivity_ok = !built_in_net || connected_components == 1;
    let template = dieline["template"].as_str().unwrap_or_default();
    let foldable_template =
        built_in_net || matches!(template, "tube" | "bottle-label" | "svg-import");
    let folded_model = if foldable_template && connected_components == 1 {
        Some(generate(dieline)?)
    } else {
        None
    };
    let fold_hinge_checks = if let Some(model) = folded_model.as_ref() {
        folded_hinge_checks(dieline, model)?
    } else {
        Vec::new()
    };
    let fold_collision_report = if let Some(model) = folded_model.as_ref() {
        Some(folded_collision_report(dieline, model)?)
    } else {
        None
    };
    if let Some(report) = fold_collision_report
        .as_ref()
        .filter(|report| report["collision_count"].as_u64().unwrap_or(0) > 0)
    {
        warnings.push(format!("folded preview has {} potential panel intersections or board-thickness clearance conflicts; inspect the current fold state", report["collision_count"]));
    }
    for check in fold_hinge_checks
        .iter()
        .filter(|check| check["joined"] == false)
    {
        warnings.push(format!(
            "folded {:?} seam between {} exceeds its {:.6} mm tolerance",
            check["kind"].as_str().unwrap_or("panel"),
            check["panels"],
            check["tolerance_mm"].as_f64().unwrap_or(0.0)
        ));
    }
    let bleed_for_bounds = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    let bounds_tolerance = panel_bounds[2].max(panel_bounds[3]).max(1.0) * 1e-8;
    let mut production_path_checks = Vec::with_capacity(paths.len());
    let mut path_intersection_work = 0_u64;
    for path in paths {
        let id = path["id"].as_str().unwrap_or("path");
        let role = path["role"].as_str().unwrap_or("unknown");
        let parsed = path["d"]
            .as_str()
            .and_then(|data| kurbo::BezPath::from_svg(data).ok());
        let (valid, within_layout, bounds_value, segment_count, closed_subpaths, self_intersects) =
            if let Some(parsed) =
                parsed.filter(|parsed| !parsed.elements().is_empty() && parsed.is_finite())
            {
                let bounds = parsed.bounding_box();
                let within = bounds.x0 >= panel_bounds[0] - bleed_for_bounds - bounds_tolerance
                    && bounds.y0 >= panel_bounds[1] - bleed_for_bounds - bounds_tolerance
                    && bounds.x1 <= panel_bounds[2] + bleed_for_bounds + bounds_tolerance
                    && bounds.y1 <= panel_bounds[3] + bleed_for_bounds + bounds_tolerance;
                let closed_subpaths = parsed
                    .elements()
                    .iter()
                    .filter(|element| matches!(element, kurbo::PathEl::ClosePath))
                    .count();
                let self_intersects =
                    svg_path_self_intersects(&parsed, &mut path_intersection_work)?;
                (
                    true,
                    within,
                    serde_json::json!([bounds.x0, bounds.y0, bounds.x1, bounds.y1]),
                    parsed.elements().len(),
                    closed_subpaths,
                    self_intersects,
                )
            } else {
                (false, false, Value::Null, 0, 0, false)
            };
        if !valid {
            warnings.push(format!("production path `{id}` ({role}) is malformed, non-finite, or has no drawable geometry"));
        } else if !within_layout {
            warnings.push(format!("production path `{id}` ({role}) extends beyond the panel envelope plus the stored bleed allowance"));
        }
        if role == "cut" && closed_subpaths == 0 {
            warnings.push(format!("cut path `{id}` has no closed contour; verify the through-cut outline before sending it to a cutter"));
        }
        if self_intersects {
            warnings.push(format!("production path `{id}` ({role}) self-intersects; correct the crossing before exporting print plates"));
        }
        production_path_checks.push(serde_json::json!({"id":id,"role":role,"valid":valid,"within_panel_envelope":within_layout,"bounds_mm":bounds_value,"path_elements":segment_count,"closed_subpaths":closed_subpaths,"self_intersects":self_intersects}));
    }
    if !panels_without_artwork.is_empty() {
        warnings.push(format!(
            "{} of {} panels have no linked artwork",
            panels_without_artwork.len(),
            panels.len()
        ));
    }
    if !roles.contains("cut") {
        warnings.push("the dieline has no stored cut paths".to_owned());
    }
    let has_structural_creases = dieline["creases"]
        .as_array()
        .is_some_and(|creases| !creases.is_empty());
    if !roles.contains("crease") && !has_structural_creases {
        warnings
            .push("no crease production paths or structural crease records are stored".to_owned());
    }
    let board_mm = dieline["dimensions_mm"]["board"].as_f64().unwrap_or(0.0);
    if board_mm > 0.0 {
        warnings.push(
            "board thickness adds interior faces and exposed cut-edge walls, bridges folded interior hinge wedges, and offsets fold axes for connected outer surfaces; score compression and allowances are not modeled, and angled clearance is not validated".to_owned(),
        );
    }
    let bleed_mm = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    if bleed_mm > 0.0 && panel_art_bound == 0 {
        warnings.push(format!(
            "bleed is set to {bleed_mm} mm, but no linked panel artwork is available to synthesize edge bleed"
        ));
    }
    let mut safe_zone_checks = Vec::new();
    let mut wrap_seam_checks = Vec::new();
    let is_tube = matches!(dieline["template"].as_str(), Some("tube" | "bottle-label"));
    let seam_angle = dieline["wrap"]["seam_angle_deg"].as_f64().unwrap_or(0.0);
    let wrap_direction = dieline["wrap"]["direction"].as_str().unwrap_or("clockwise");
    for panel in panels.iter().filter(|panel| panel.get("artwork").is_some()) {
        let id = panel.get("id").and_then(Value::as_str).unwrap_or("panel");
        let rect = panel["rect_mm"]
            .as_array()
            .context("[invalid-input] panel rect_mm is missing")?;
        let (x, y, width, height) = (
            rect[0].as_f64().unwrap_or(0.0),
            rect[1].as_f64().unwrap_or(0.0),
            rect[2].as_f64().unwrap_or(0.0),
            rect[3].as_f64().unwrap_or(0.0),
        );
        let inset = panel
            .get("safe_zone_mm")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let has_safe_zone = inset > 0.0;
        let transform = panel.get("artwork_transform");
        let scale = transform
            .and_then(|value| value.get("scale"))
            .and_then(Value::as_array)
            .filter(|values| values.len() == 2);
        let offset = transform
            .and_then(|value| value.get("offset"))
            .and_then(Value::as_array)
            .filter(|values| values.len() == 2);
        let sx = scale.and_then(|values| values[0].as_f64()).unwrap_or(1.0);
        let sy = scale.and_then(|values| values[1].as_f64()).unwrap_or(1.0);
        let cx = offset.and_then(|values| values[0].as_f64()).unwrap_or(0.5);
        let cy = offset.and_then(|values| values[1].as_f64()).unwrap_or(0.5);
        let angle = transform
            .and_then(|value| value.get("rotate"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .to_radians();
        let (sin, cos) = angle.sin_cos();
        let half_width = width * sx / 2.0;
        let half_height = height * sy / 2.0;
        let rotated_half_width = cos.abs() * half_width + sin.abs() * half_height;
        let rotated_half_height = sin.abs() * half_width + cos.abs() * half_height;
        let center_x = x + width * cx;
        let center_y = y + height * cy;
        let tolerance = 1e-8 * width.max(height).max(1.0);
        if is_tube {
            let touches_start = center_x - rotated_half_width <= x + tolerance;
            let touches_end = center_x + rotated_half_width >= x + width - tolerance;
            let touches_seam = touches_start || touches_end;
            if touches_seam {
                warnings.push(format!(
                    "panel `{id}` artwork bounds touch the cylindrical wrap seam at {seam_angle}° ({wrap_direction}); inspect seam alignment"
                ));
            }
            wrap_seam_checks.push(serde_json::json!({
                "panel":id,
                "seam_angle_deg":seam_angle,
                "direction":wrap_direction,
                "overlap_mm":dieline["wrap"]["overlap_mm"].as_f64().unwrap_or(0.0),
                "touches_start_edge":touches_start,
                "touches_end_edge":touches_end,
                "touches_seam":touches_seam,
                "method":"conservative transformed bounds"
            }));
        }
        let within_safe_zone = has_safe_zone
            && center_x - rotated_half_width >= x + inset - tolerance
            && center_x + rotated_half_width <= x + width - inset + tolerance
            && center_y - rotated_half_height >= y + inset - tolerance
            && center_y + rotated_half_height <= y + height - inset + tolerance;
        if !has_safe_zone {
            warnings.push(format!(
                "panel `{id}` has linked artwork but no safe-area inset is configured"
            ));
        } else if !within_safe_zone {
            warnings.push(format!(
                "panel `{id}` linked artwork bounds extend outside its {inset} mm safe area"
            ));
        }
        safe_zone_checks.push(serde_json::json!({
            "panel":id,
            "inset_mm":if has_safe_zone { Some(inset) } else { None },
            "within_safe_zone":within_safe_zone
        }));
    }
    let (rasterized_panels, rasterized_ppi, bleed_synthesized, export_warnings) =
        if panel_art_bound > 0 {
            let export = flat_dieline_svg_with_artwork(raw, document, dieline_id, min_ppi)?;
            (
                export.rasterized_panels,
                export.rasterized_ppi,
                export.bleed_synthesized,
                export.warnings,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
    warnings.extend(export_warnings);
    Ok(serde_json::json!({
        "id":dieline_id,
        "template":dieline["template"],
        "preflight_status":if warnings.is_empty() {"mockup-ready"} else {"needs-review"},
        "manufacturing_certified":false,
        "panel_count":panels.len(),
        "panel_art_bound":panel_art_bound,
        "panels_without_artwork":panels_without_artwork,
        "path_roles":roles,
        "design_geometry_status":if connectivity_ok && (!foldable_template || fold_collision_report.is_some()) && fold_hinge_checks.iter().all(|check| check["joined"] == true) && fold_collision_report.as_ref().is_none_or(|report| report["collision_count"].as_u64().unwrap_or(0) == 0) && production_path_checks.iter().all(|check| check["valid"] == true && check["within_panel_envelope"] == true && check["self_intersects"] == false && (check["role"] != "cut" || check["closed_subpaths"].as_u64().unwrap_or(0) > 0)) {"checks-passed"} else {"needs-review"},
        "panel_connectivity_required":built_in_net,
        "panel_connected_components":connected_components,
        "panel_bounds_mm":panel_bounds,
        "fold_hinge_checks":fold_hinge_checks,
        "fold_collision_report":fold_collision_report,
        "production_path_checks":production_path_checks,
        "rasterized_panels":rasterized_panels,
        "rasterized_ppi":rasterized_ppi,
        "bleed_synthesized":bleed_synthesized,
        "safe_zone_checks":safe_zone_checks,
        "wrap_seam_checks":wrap_seam_checks,
        "minimum_ppi":min_ppi,
        "pdfx_output_role":pdfx_output_role,
        "warnings":warnings,
        "production_blockers":[
            "board thickness creates panel interiors, exposed cut-edge walls, and visual bridges across folded interior hinge wedges and offsets folded outer-surface hinge axes; cut/fold allowances and score compression are not modeled, and manufacturing compensation is not certified",
            "edge-extended bleed is a design aid; inspect per-panel method and printer-specific requirements",
            "current-fold-state face intersections and near-parallel overlap within stored board thickness are screened; angled panel clearance, score compression, and structural behavior are not validated",
            "PDF/X is a flattened process-CMYK draft; technical spot plates are separate SVG/PNG outputs and are not embedded in that PDF"
        ]
    }))
}

fn source_page_data(raw: &Value, page_id: &str) -> Result<(u32, u32)> {
    let page = raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
        .with_context(|| format!("[missing-resource] source page `{page_id}` is missing"))?;
    let width = page["canvas"]["width"]
        .as_u64()
        .and_then(|width| u32::try_from(width).ok())
        .filter(|width| *width > 0)
        .context("[invalid-input] panel artwork canvas width is invalid")?;
    let height = page["canvas"]["height"]
        .as_u64()
        .and_then(|height| u32::try_from(height).ok())
        .filter(|height| *height > 0)
        .context("[invalid-input] panel artwork canvas height is invalid")?;
    Ok((width, height))
}

fn source_vector_svg(raw: &Value, page_id: &str) -> Result<String> {
    let page = raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
        .cloned()
        .with_context(|| format!("[missing-resource] source page `{page_id}` is missing"))?;
    let mut mini = serde_json::json!({
        "format":"pentool", "version":4, "name":"Panel artwork", "pages":[page]
    });
    for key in ["styles", "components", "fonts"] {
        if let Some(value) = raw.get(key) {
            mini[key] = value.clone();
        }
    }
    let flattened = crate::scene::flatten_to_v3(&mini)?;
    let mut document: crate::document::Document = serde_json::from_value(flattened)
        .context("[unsupported-capability] vector artwork cannot be represented as an SVG page")?;
    document.canvas.background = "none".into();
    crate::render::to_svg(&document)
}

fn svg_inner(svg: &str) -> Result<&str> {
    let start = svg
        .find('>')
        .context("[malformed-resource] source SVG has no root element")?
        + 1;
    let end = svg
        .rfind("</svg>")
        .context("[malformed-resource] source SVG has no closing root element")?;
    ensure!(
        start <= end,
        "[malformed-resource] source SVG root is malformed"
    );
    Ok(&svg[start..end])
}

fn role_color(role: &str) -> &'static str {
    match role {
        "cut" => "#dc2626",
        "crease" => "#2563eb",
        "perforation" => "#7c3aed",
        "bleed" => "#f97316",
        "safe-zone" => "#16a34a",
        "glue-flap" => "#0891b2",
        _ => "#334155",
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn svg_mm_attribute(node: roxmltree::Node<'_, '_>, name: &str) -> Result<f64> {
    let text = node
        .attribute(name)
        .with_context(|| format!("[invalid-input] SVG root needs an explicit {name} in mm"))?;
    let value = text
        .trim()
        .strip_suffix("mm")
        .with_context(|| {
            format!(
                "[unsupported-capability] SVG {name} must use absolute millimeters, got `{text}`"
            )
        })?
        .trim()
        .parse::<f64>()
        .with_context(|| format!("[invalid-input] SVG {name} is not numeric"))?;
    ensure!(
        value.is_finite() && (1.0..=100_000.0).contains(&value),
        "[invalid-input] SVG {name} must be within 1..=100000 mm"
    );
    Ok(value)
}

fn svg_length(node: roxmltree::Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    let value = node
        .attribute(name)
        .map(str::parse::<f64>)
        .transpose()
        .with_context(|| format!("[invalid-input] SVG {name} must be a number"))?
        .unwrap_or(default);
    ensure!(
        value.is_finite(),
        "[invalid-input] SVG {name} must be finite"
    );
    Ok(value)
}

fn svg_path_role(node: roxmltree::Node<'_, '_>) -> Result<&'static str> {
    match node.attribute("data-role") {
        Some("cut") => Ok("cut"),
        Some("crease") => Ok("crease"),
        Some("perforation") => Ok("perforation"),
        Some("bleed") => Ok("bleed"),
        Some("safe-zone") => Ok("safe-zone"),
        Some("glue-flap") => Ok("glue-flap"),
        Some(role) => bail!("[unsupported-capability] SVG production role `{role}` is unsupported; use cut, crease, perforation, bleed, safe-zone or glue-flap"),
        None => bail!("[invalid-input] SVG path is unlabeled; add an explicit data-role"),
    }
}

fn svg_path_self_intersects(path: &kurbo::BezPath, work: &mut u64) -> Result<bool> {
    const CURVE_STEPS: usize = 16;
    const MAX_SEGMENTS_PER_SUBPATH: usize = 2048;
    const MAX_SEGMENTS_PER_PATH: usize = 2048;
    const MAX_INTERSECTION_WORK: u64 = 2_000_000;

    #[derive(Clone, Copy)]
    struct Segment {
        start: [f64; 2],
        end: [f64; 2],
        index: usize,
        count: usize,
        closed: bool,
        subpath: usize,
    }

    let mut subpaths: Vec<(Vec<[f64; 2]>, bool)> = Vec::new();
    let mut points = Vec::new();
    let mut current = None;
    let mut start = None;
    let mut closed = false;
    let flush = |subpaths: &mut Vec<(Vec<[f64; 2]>, bool)>,
                 points: &mut Vec<[f64; 2]>,
                 closed: &mut bool| {
        if points.len() > 1 {
            subpaths.push((std::mem::take(points), *closed));
        } else {
            points.clear();
        }
        *closed = false;
    };
    for element in path.elements() {
        match *element {
            kurbo::PathEl::MoveTo(point) => {
                flush(&mut subpaths, &mut points, &mut closed);
                let point = [point.x, point.y];
                points.push(point);
                current = Some(point);
                start = Some(point);
            }
            kurbo::PathEl::LineTo(point) => {
                let point = [point.x, point.y];
                ensure!(
                    current.replace(point).is_some(),
                    "[invalid-input] SVG production path must move before drawing"
                );
                points.push(point);
            }
            kurbo::PathEl::QuadTo(control, end) => {
                let from = current
                    .context("[invalid-input] SVG production path must move before drawing")?;
                let control = [control.x, control.y];
                let end = [end.x, end.y];
                for step in 1..=CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    points.push([
                        u * u * from[0] + 2.0 * u * t * control[0] + t * t * end[0],
                        u * u * from[1] + 2.0 * u * t * control[1] + t * t * end[1],
                    ]);
                }
                current = Some(end);
            }
            kurbo::PathEl::CurveTo(control1, control2, end) => {
                let from = current
                    .context("[invalid-input] SVG production path must move before drawing")?;
                let control1 = [control1.x, control1.y];
                let control2 = [control2.x, control2.y];
                let end = [end.x, end.y];
                for step in 1..=CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    points.push([
                        u.powi(3) * from[0]
                            + 3.0 * u * u * t * control1[0]
                            + 3.0 * u * t * t * control2[0]
                            + t.powi(3) * end[0],
                        u.powi(3) * from[1]
                            + 3.0 * u * u * t * control1[1]
                            + 3.0 * u * t * t * control2[1]
                            + t.powi(3) * end[1],
                    ]);
                }
                current = Some(end);
            }
            kurbo::PathEl::ClosePath => {
                if let Some(first) = start {
                    if current != Some(first) {
                        points.push(first);
                    }
                    current = Some(first);
                    closed = true;
                }
            }
        }
    }
    flush(&mut subpaths, &mut points, &mut closed);

    fn orientation(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }
    fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2], epsilon: f64) -> bool {
        p[0] >= a[0].min(b[0]) - epsilon
            && p[0] <= a[0].max(b[0]) + epsilon
            && p[1] >= a[1].min(b[1]) - epsilon
            && p[1] <= a[1].max(b[1]) + epsilon
    }
    fn intersects(a: &Segment, b: &Segment, epsilon: f64) -> bool {
        let (p, p2, q, q2) = (a.start, a.end, b.start, b.end);
        let (o1, o2, o3, o4) = (
            orientation(p, p2, q),
            orientation(p, p2, q2),
            orientation(q, q2, p),
            orientation(q, q2, p2),
        );
        ((o1 > epsilon && o2 < -epsilon || o1 < -epsilon && o2 > epsilon)
            && (o3 > epsilon && o4 < -epsilon || o3 < -epsilon && o4 > epsilon))
            || (o1.abs() <= epsilon && on_segment(p, p2, q, epsilon))
            || (o2.abs() <= epsilon && on_segment(p, p2, q2, epsilon))
            || (o3.abs() <= epsilon && on_segment(q, q2, p, epsilon))
            || (o4.abs() <= epsilon && on_segment(q, q2, p2, epsilon))
    }
    fn intersects_between_subpaths(a: &Segment, b: &Segment, epsilon: f64) -> bool {
        let (p, p2, q, q2) = (a.start, a.end, b.start, b.end);
        let (o1, o2, o3, o4) = (
            orientation(p, p2, q),
            orientation(p, p2, q2),
            orientation(q, q2, p),
            orientation(q, q2, p2),
        );
        if (o1 > epsilon && o2 < -epsilon || o1 < -epsilon && o2 > epsilon)
            && (o3 > epsilon && o4 < -epsilon || o3 < -epsilon && o4 > epsilon)
        {
            return true;
        }
        let collinear = [o1, o2, o3, o4]
            .into_iter()
            .all(|orientation| orientation.abs() <= epsilon);
        if !collinear {
            return false;
        }
        let axis = usize::from((p[0] - p2[0]).abs() < (p[1] - p2[1]).abs());
        let overlap = p[axis].max(p2[axis]).min(q[axis].max(q2[axis]))
            - p[axis].min(p2[axis]).max(q[axis].min(q2[axis]));
        overlap > epsilon
    }

    let mut segments = Vec::new();
    for (subpath, (points, closed)) in subpaths.into_iter().enumerate() {
        let count = points.len().saturating_sub(1);
        ensure!(
            count <= MAX_SEGMENTS_PER_SUBPATH,
            "[limit-exceeded] SVG production subpath has more than {MAX_SEGMENTS_PER_SUBPATH} segments; simplify it before import"
        );
        ensure!(
            segments.len().saturating_add(count) <= MAX_SEGMENTS_PER_PATH,
            "[limit-exceeded] SVG production path has more than {MAX_SEGMENTS_PER_PATH} segments; simplify it before import"
        );
        segments.extend(points.windows(2).enumerate().map(|(index, pair)| Segment {
            start: pair[0],
            end: pair[1],
            index,
            count,
            closed,
            subpath,
        }));
    }
    let comparisons =
        (segments.len() as u64).saturating_mul(segments.len().saturating_sub(1) as u64) / 2;
    *work = (*work)
        .checked_add(comparisons)
        .context("[limit-exceeded] SVG path intersection work overflow")?;
    ensure!(
        *work <= MAX_INTERSECTION_WORK,
        "[limit-exceeded] SVG dieline exceeds the bounded self-intersection check budget; simplify its production paths"
    );
    let magnitude = segments
        .iter()
        .flat_map(|segment| segment.start.into_iter().chain(segment.end))
        .map(f64::abs)
        .fold(1.0, f64::max);
    let epsilon = magnitude * magnitude * 1.0e-12;
    for (index, left) in segments.iter().enumerate() {
        for right in segments.iter().skip(index + 1) {
            if left.subpath == right.subpath {
                let adjacent = right.index == left.index + 1
                    || (left.closed && left.index == 0 && right.index + 1 == left.count);
                if !adjacent && intersects(left, right, epsilon) {
                    return Ok(true);
                }
            } else if intersects_between_subpaths(left, right, epsilon) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn crease_segments(d: &str, id: &str, direction: &str, angle_deg: f64) -> Result<Vec<Value>> {
    ensure!(
        matches!(direction, "mountain" | "valley"),
        "[invalid-input] crease `{id}` needs data-direction=mountain or valley"
    );
    ensure!(
        angle_deg.is_finite() && (-180.0..=180.0).contains(&angle_deg),
        "[invalid-input] crease `{id}` data-angle-deg must be within -180..=180"
    );
    let path = kurbo::BezPath::from_svg(d)
        .with_context(|| format!("[invalid-input] crease path `{id}` is invalid SVG path data"))?;
    let (mut current, mut segments) = (None, Vec::new());
    for element in path.elements() {
        match *element {
            kurbo::PathEl::MoveTo(point) => current = Some(point),
            kurbo::PathEl::LineTo(point) => {
                let start = current.replace(point).with_context(|| {
                    format!("[invalid-input] crease path `{id}` must start each segment with M")
                })?;
                ensure!(
                    start != point,
                    "[invalid-input] crease path `{id}` contains a zero-length segment"
                );
                ensure!(
                    segments.len() < 512,
                    "[limit-exceeded] crease path `{id}` expands to more than 512 fold segments"
                );
                let suffix = if segments.is_empty() {
                    String::new()
                } else {
                    format!("-{}", segments.len() + 1)
                };
                segments.push(serde_json::json!({
                    "id":format!("{id}{suffix}"),
                    "line_mm":[[start.x,start.y],[point.x,point.y]],
                    "angle_deg":angle_deg,
                    "direction":direction
                }));
            }
            _ => bail!("[unsupported-capability] crease path `{id}` must contain only straight M/L segments"),
        }
    }
    ensure!(
        !segments.is_empty(),
        "[invalid-input] crease path `{id}` has no line segments"
    );
    Ok(segments)
}

/// Import a deliberately small, safe SVG dieline subset and add its linked 3D
/// preview node in the same candidate document. Coordinates must already be
/// millimeters; panels are labeled rectangles and fold lines are explicit,
/// straight, direction-labeled paths. Unsupported SVG drawing is refused.
#[allow(clippy::too_many_arguments)]
pub fn add_svg_dieline(
    raw: &mut Value,
    svg: &str,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    placement: [f64; 4],
) -> Result<Value> {
    ensure!(
        raw.get("version")
            .and_then(Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-capability] SVG dielines require document format v11 or newer"
    );
    ensure!(
        !svg.is_empty() && svg.len() <= MAX_IMPORTED_DIELINE_SVG_BYTES,
        "[limit-exceeded] SVG dieline input must be 1..={} bytes",
        MAX_IMPORTED_DIELINE_SVG_BYTES
    );
    ensure!(
        !svg.contains("<!DOCTYPE"),
        "[unsupported-capability] SVG dieline DOCTYPE declarations are not accepted"
    );
    let document = roxmltree::Document::parse(svg)
        .context("[malformed-resource] dieline input is not well-formed XML")?;
    let root = document.root_element();
    ensure!(
        root.tag_name().name() == "svg"
            && root
                .tag_name()
                .namespace()
                .is_none_or(|ns| ns == "http://www.w3.org/2000/svg"),
        "[invalid-input] dieline XML root must be an SVG element"
    );
    ensure!(
        root.attribute("transform").is_none(),
        "[unsupported-capability] transformed SVG roots are not supported; bake transforms into millimeter coordinates"
    );
    let (width, height) = (
        svg_mm_attribute(root, "width")?,
        svg_mm_attribute(root, "height")?,
    );
    let layout_bleed = root
        .attribute("data-layout-bleed-mm")
        .unwrap_or("0")
        .parse::<f64>()
        .context("[invalid-input] SVG data-layout-bleed-mm must be numeric")?;
    ensure!(
        layout_bleed.is_finite()
            && (0.0..=100.0).contains(&layout_bleed)
            && width > 2.0 * layout_bleed
            && height > 2.0 * layout_bleed,
        "[invalid-input] SVG layout bleed must be within 0..=100 mm and fit the page"
    );
    let view_box = root.attribute("viewBox").context(
        "[invalid-input] SVG dieline needs viewBox='0 0 width height' in millimeter coordinates",
    )?;
    let view_box = view_box
        .split_ascii_whitespace()
        .map(str::parse::<f64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("[invalid-input] SVG viewBox must contain four numbers")?;
    ensure!(
        view_box.len() == 4
            && view_box.iter().all(|value| value.is_finite())
            && view_box[0].abs() < 1e-9
            && view_box[1].abs() < 1e-9
            && (view_box[2] - width).abs() < 1e-6
            && (view_box[3] - height).abs() < 1e-6,
        "[unsupported-capability] SVG viewBox must be 0 0 width height with coordinates equal to its declared millimeter dimensions"
    );
    ensure!(
        root.descendants()
            .filter(roxmltree::Node::is_element)
            .count()
            <= 8192,
        "[limit-exceeded] SVG dieline may contain at most 8192 elements"
    );

    for node in root.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        ensure!(
            node.tag_name().namespace() == root.tag_name().namespace(),
            "[unsupported-capability] SVG elements from foreign namespaces are not accepted"
        );
        ensure!(
            matches!(
                name,
                "svg" | "g" | "rect" | "path" | "title" | "desc" | "text"
            ),
            "[unsupported-capability] SVG element <{name}> is not supported in dieline import"
        );
        ensure!(
            node.attribute("transform").is_none() && node.attribute("style").is_none(),
            "[unsupported-capability] SVG transforms and CSS styles must be converted to explicit dieline geometry before import"
        );
        for attribute in node.attributes() {
            let name = attribute.name().to_ascii_lowercase();
            ensure!(
                name != "href" && !name.starts_with("on"),
                "[unsupported-capability] SVG external references and event attributes are not accepted"
            );
        }
        ensure!(
            node.attribute("display") != Some("none")
                && node.attribute("visibility") != Some("hidden"),
            "[unsupported-capability] hidden SVG geometry cannot be imported as an active dieline"
        );
        if name == "text" {
            ensure!(
                node.ancestors()
                    .any(|ancestor| ancestor.attribute("data-panel").is_some()),
                "[unsupported-capability] SVG text outside a labeled panel is not dieline geometry"
            );
        }
    }

    let mut panels = Vec::new();
    let mut paths = Vec::new();
    let mut creases = Vec::new();
    let mut self_intersection_work = 0_u64;
    for node in root.descendants().filter(roxmltree::Node::is_element) {
        if node.attribute("data-role") == Some("bleed-preview") {
            continue;
        }
        match node.tag_name().name() {
            "rect" => {
                let role = node.attribute("data-role");
                if let Some(role) = role {
                    ensure!(
                        role != "crease",
                        "[unsupported-capability] crease rectangles are ambiguous; use a straight path with data-direction and data-angle-deg"
                    );
                    let role = svg_path_role(node)?;
                    let id = node
                        .attribute("data-id")
                        .filter(|id| !id.trim().is_empty())
                        .context("[invalid-input] labeled production rectangle needs data-id")?;
                    let x = svg_length(node, "x", 0.0)? - layout_bleed;
                    let y = svg_length(node, "y", 0.0)? - layout_bleed;
                    let w = svg_length(node, "width", 0.0)?;
                    let h = svg_length(node, "height", 0.0)?;
                    ensure!(w > 0.0 && h > 0.0, "[invalid-input] SVG production rectangle `{id}` must have positive dimensions");
                    ensure!(
                        x >= 0.0 && y >= 0.0 && x + w <= width && y + h <= height,
                        "[invalid-input] SVG production region `{id}` lies outside the millimeter viewBox"
                    );
                    ensure!(
                        paths.len() < 4096,
                        "[limit-exceeded] SVG dieline contains more than 4096 production paths"
                    );
                    let mut path = serde_json::json!({"id":id,"role":role,"d":format!("M {x} {y} h {w} v {h} h -{w} Z")});
                    if let Some(spot) = node.attribute("data-spot") {
                        path["spot"] = serde_json::json!(spot);
                    }
                    paths.push(path);
                } else {
                    ensure!(
                        node.attribute("rx").is_none_or(|value| value == "0")
                            && node.attribute("ry").is_none_or(|value| value == "0"),
                        "[unsupported-capability] rounded SVG rectangles are not supported as dieline panels"
                    );
                    let id = node
                        .attribute("data-panel")
                        .or_else(|| node.parent().and_then(|parent| parent.attribute("data-panel")))
                        .filter(|id| !id.trim().is_empty())
                        .context("[invalid-input] SVG rectangle is unlabeled; panel rectangles need data-panel and production regions need data-role")?;
                    let x = svg_length(node, "x", 0.0)? - layout_bleed;
                    let y = svg_length(node, "y", 0.0)? - layout_bleed;
                    let w = svg_length(node, "width", 0.0)?;
                    let h = svg_length(node, "height", 0.0)?;
                    ensure!(
                        w > 0.0 && h > 0.0,
                        "[invalid-input] SVG panel `{id}` must be a positive-size rectangle"
                    );
                    ensure!(
                        x >= 0.0 && y >= 0.0 && x + w <= width && y + h <= height,
                        "[invalid-input] SVG panel `{id}` lies outside the millimeter viewBox"
                    );
                    ensure!(
                        panels.len() < 256,
                        "[limit-exceeded] SVG dieline contains more than 256 panels"
                    );
                    panels.push(serde_json::json!({"id":id,"rect_mm":[x,y,w,h],"face_slot":id,"uv_rect":[0,0,1,1]}));
                }
            }
            "path" => {
                let role = svg_path_role(node)?;
                let id = node
                    .attribute("data-id")
                    .filter(|id| !id.trim().is_empty())
                    .context("[invalid-input] labeled production path needs data-id")?;
                let d = node
                    .attribute("d")
                    .filter(|d| !d.is_empty() && d.len() <= 1_000_000)
                    .context("[limit-exceeded] SVG production path needs d data of at most 1,000,000 bytes")?;
                let parsed_path = kurbo::BezPath::from_svg(d).with_context(|| {
                    format!("[invalid-input] SVG path `{id}` has invalid path data")
                })?;
                ensure!(
                    parsed_path.elements().len() >= 2,
                    "[invalid-input] SVG production path `{id}` has no drawable segments"
                );
                ensure!(
                    !svg_path_self_intersects(&parsed_path, &mut self_intersection_work)?,
                    "[invalid-input] SVG production path `{id}` self-intersects; repair or split the contour before import"
                );
                let bounds = parsed_path.bounding_box();
                ensure!(
                    bounds.x0 >= 0.0
                        && bounds.y0 >= 0.0
                        && bounds.x1 <= width
                        && bounds.y1 <= height,
                    "[invalid-input] SVG production path `{id}` lies outside the millimeter viewBox"
                );
                if role == "crease" {
                    let direction = node.attribute("data-direction").unwrap_or_default();
                    let angle = node
                        .attribute("data-angle-deg")
                        .context("[invalid-input] crease path `{id}` needs data-angle-deg")?
                        .parse::<f64>()
                        .context("[invalid-input] crease data-angle-deg must be numeric")?;
                    let mut canonical_path = parsed_path.clone();
                    canonical_path
                        .apply_affine(kurbo::Affine::translate((-layout_bleed, -layout_bleed)));
                    let segments = crease_segments(&canonical_path.to_svg(), id, direction, angle)?;
                    ensure!(
                        creases.len() + segments.len() <= 512,
                        "[limit-exceeded] SVG dieline contains more than 512 fold segments"
                    );
                    creases.extend(segments);
                }
                ensure!(
                    paths.len() < 4096,
                    "[limit-exceeded] SVG dieline contains more than 4096 production paths"
                );
                let mut canonical_path = parsed_path;
                canonical_path
                    .apply_affine(kurbo::Affine::translate((-layout_bleed, -layout_bleed)));
                let mut path = serde_json::json!({"id":id,"role":role,"d":canonical_path.to_svg()});
                if let Some(spot) = node.attribute("data-spot") {
                    path["spot"] = serde_json::json!(spot);
                }
                paths.push(path);
            }
            _ => {}
        }
    }
    ensure!(
        !panels.is_empty() && panels.len() <= 256,
        "[invalid-input] SVG dieline needs 1..=256 labeled panel rectangles"
    );
    let dieline = serde_json::json!({
        "template":"svg-import", "units":"mm",
        "dimensions_mm":{"length":width - 2.0 * layout_bleed,"width":height - 2.0 * layout_bleed,"height":1.0,"board":0.0,"bleed":layout_bleed},
        "panels":panels,"creases":creases,"paths":paths,"fold_state":1.0
    });
    let mut candidate = raw.clone();
    let report = add_generated_dieline(
        &mut candidate,
        dieline_id,
        node_id,
        layer_id,
        page_id,
        placement,
        dieline,
    )?;
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(report)
}

/// Add a generated sleeve and its linked scene node to a v11 document draft.
/// The caller validates and commits the complete candidate transactionally.
#[allow(clippy::too_many_arguments)]
pub fn add_sleeve(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    length: f64,
    width: f64,
    height: f64,
    board: f64,
    bleed: f64,
    placement: [f64; 4],
) -> Result<Value> {
    let dieline = sleeve_dieline(length, width, height, board, bleed)?;
    add_generated_dieline(
        raw, dieline_id, node_id, layer_id, page_id, placement, dieline,
    )
}

/// Add a generated stand-up-pouch mockup and its linked folded preview node.
#[allow(clippy::too_many_arguments)]
pub fn add_pouch(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    front_width: f64,
    gusset: f64,
    height: f64,
    bleed: f64,
    placement: [f64; 4],
) -> Result<Value> {
    let dieline = pouch_dieline(front_width, gusset, height, bleed)?;
    add_generated_dieline(
        raw, dieline_id, node_id, layer_id, page_id, placement, dieline,
    )
}

/// Add a cylindrical tube-label dieline and its linked 3D preview node.
#[allow(clippy::too_many_arguments)]
pub fn add_tube_label(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    radius: f64,
    height: f64,
    bleed: f64,
    placement: [f64; 4],
) -> Result<Value> {
    add_tube_label_with_overlap(
        raw, dieline_id, node_id, layer_id, page_id, radius, height, bleed, 0.0, placement,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn add_tube_label_with_overlap(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    radius: f64,
    height: f64,
    bleed: f64,
    overlap_mm: f64,
    placement: [f64; 4],
) -> Result<Value> {
    let dieline = tube_label_dieline_with_overlap(radius, height, bleed, overlap_mm)?;
    add_generated_dieline(
        raw, dieline_id, node_id, layer_id, page_id, placement, dieline,
    )
}

/// Add a measured cylindrical bottle-label dieline and its folded wrap node.
#[allow(clippy::too_many_arguments)]
pub fn add_bottle_label(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    radius: f64,
    height: f64,
    bleed: f64,
    overlap_mm: f64,
    placement: [f64; 4],
) -> Result<Value> {
    let dieline = bottle_label_dieline(radius, height, bleed, overlap_mm)?;
    add_generated_dieline(
        raw, dieline_id, node_id, layer_id, page_id, placement, dieline,
    )
}

/// Add a generated tuck-end carton as a linked dieline-backed 3D node.
#[allow(clippy::too_many_arguments)]
pub fn add_tuck_carton(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    template: &str,
    length: f64,
    width: f64,
    height: f64,
    board: f64,
    bleed: f64,
    glue_flap: f64,
    placement: [f64; 4],
) -> Result<Value> {
    let dieline = tuck_carton_dieline(template, length, width, height, board, bleed, glue_flap)?;
    add_generated_dieline(
        raw, dieline_id, node_id, layer_id, page_id, placement, dieline,
    )
}

/// Change the target fold angle for one stored crease without changing the
/// dieline's shared flat-to-folded preview amount.
pub fn set_crease_angle(
    raw: &mut Value,
    dieline_id: &str,
    crease_id: &str,
    angle_deg: f64,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    ensure!(
        raw.get("version").and_then(Value::as_u64).is_some_and(|v| v >= 11),
        "[unsupported-version] per-crease editing needs document format v11 or newer; upgrade the document first"
    );
    ensure!(
        angle_deg.is_finite() && (-180.0..=180.0).contains(&angle_deg),
        "[invalid-input] crease target angle must be within -180..=180 degrees"
    );
    let crease = raw
        .get_mut("dielines")
        .and_then(|catalog| catalog.get_mut(dieline_id))
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?
        .get_mut("creases")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] dieline creases must be an array")?
        .iter_mut()
        .find(|crease| crease.get("id").and_then(Value::as_str) == Some(crease_id))
        .with_context(|| {
            format!(
                "[missing-resource] crease `{crease_id}` does not exist on dieline `{dieline_id}`"
            )
        })?;
    let previous_angle_deg = crease["angle_deg"].as_f64().unwrap_or_default();
    crease["angle_deg"] = serde_json::json!(angle_deg);
    Ok(serde_json::json!({
        "dieline":dieline_id,
        "crease":crease_id,
        "previous_angle_deg":previous_angle_deg,
        "angle_deg":angle_deg
    }))
}

/// Change one crease's mountain/valley orientation without changing its angle
/// or the dieline's current fold preview amount.
pub fn set_crease_direction(
    raw: &mut Value,
    dieline_id: &str,
    crease_id: &str,
    direction: &str,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    ensure!(
        raw.get("version").and_then(Value::as_u64).is_some_and(|v| v >= 11),
        "[unsupported-version] crease direction editing needs document format v11 or newer; upgrade the document first"
    );
    ensure!(
        matches!(direction, "mountain" | "valley"),
        "[invalid-input] crease direction must be `mountain` or `valley`"
    );
    let crease = raw
        .get_mut("dielines")
        .and_then(|catalog| catalog.get_mut(dieline_id))
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?
        .get_mut("creases")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] dieline creases must be an array")?
        .iter_mut()
        .find(|crease| crease.get("id").and_then(Value::as_str) == Some(crease_id))
        .with_context(|| {
            format!(
                "[missing-resource] crease `{crease_id}` does not exist on dieline `{dieline_id}`"
            )
        })?;
    let previous_direction = crease["direction"].as_str().unwrap_or_default().to_owned();
    crease["direction"] = serde_json::json!(direction);
    Ok(serde_json::json!({
        "dieline":dieline_id,
        "crease":crease_id,
        "previous_direction":previous_direction,
        "direction":direction
    }))
}

/// Update stored board thickness and/or print bleed without rebuilding or
/// disturbing authored panel artwork and production paths.
fn translate_layout_page_for_bleed(
    raw: &mut Value,
    page_id: &str,
    delta_mm: f64,
    bleed_mm: f64,
) -> Result<()> {
    let page = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .and_then(|pages| {
            pages
                .iter_mut()
                .find(|page| page["id"].as_str() == Some(page_id))
        })
        .with_context(|| {
            format!("[missing-resource] editable dieline page `{page_id}` is missing")
        })?;
    let canvas = page
        .get_mut("canvas")
        .and_then(Value::as_object_mut)
        .context("[invalid-input] editable dieline page canvas must be an object")?;
    for axis in ["width", "height"] {
        let previous = canvas.get(axis).and_then(Value::as_f64).with_context(|| {
            format!("[invalid-input] editable dieline canvas {axis} is invalid")
        })?;
        let next = previous + delta_mm * 2.0;
        ensure!(
            next.is_finite() && next > 0.0 && next <= 16_384.0,
            "[limit-exceeded] updated editable dieline canvas {axis} must fit 1..=16384 mm"
        );
        canvas.insert(axis.into(), serde_json::json!(next.ceil() as u32));
    }
    if let Some(trim) = page
        .get_mut("production")
        .and_then(|production| production.get_mut("trim"))
        .and_then(Value::as_object_mut)
    {
        trim.insert("x".into(), serde_json::json!(bleed_mm));
        trim.insert("y".into(), serde_json::json!(bleed_mm));
    }
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] editable dieline page layers must be an array")?
    {
        let nodes = layer
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .context("[invalid-input] editable dieline page nodes must be an array")?;
        for node in nodes {
            let transform = node
                .as_object_mut()
                .context("[invalid-input] editable dieline node must be an object")?
                .entry("transform")
                .or_insert_with(|| serde_json::json!([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]))
                .as_array_mut()
                .context(
                    "[invalid-input] editable dieline node transform must be a six-number array",
                )?;
            ensure!(
                transform.len() == 6,
                "[invalid-input] editable dieline node transform must contain six numbers"
            );
            for index in [4, 5] {
                let previous = transform[index].as_f64().context(
                    "[invalid-input] editable dieline node transform must contain numbers",
                )?;
                let next = previous + delta_mm;
                ensure!(
                    next.is_finite(),
                    "[invalid-input] editable dieline node transform translation is non-finite"
                );
                transform[index] = serde_json::json!(next);
            }
        }
    }
    Ok(())
}

pub fn set_print_settings(
    raw: &mut Value,
    dieline_id: &str,
    board_mm: Option<f64>,
    bleed_mm: Option<f64>,
    seam_angle_deg: Option<f64>,
    wrap_direction: Option<&str>,
) -> Result<Value> {
    let mut candidate = raw.clone();
    crate::scene::validate(&candidate)
        .context("[invalid-input] document must validate before packaging settings")?;
    ensure!(
        candidate.get("version").and_then(Value::as_u64).is_some_and(|v| v >= 11),
        "[unsupported-version] packaging print settings need document format v11 or newer; upgrade the document first"
    );
    ensure!(
        board_mm.is_some()
            || bleed_mm.is_some()
            || seam_angle_deg.is_some()
            || wrap_direction.is_some(),
        "[invalid-input] provide board thickness, bleed, wrap seam angle, or wrap direction"
    );
    for (name, value) in [("board thickness", board_mm), ("bleed", bleed_mm)] {
        if let Some(value) = value {
            ensure!(
                value.is_finite() && (0.0..=100.0).contains(&value),
                "[invalid-input] {name} must be within 0..=100 mm"
            );
        }
    }
    if let Some(value) = seam_angle_deg {
        ensure!(
            value.is_finite() && (-360.0..=360.0).contains(&value),
            "[invalid-input] wrap seam angle must be within -360..=360 degrees"
        );
    }
    if let Some(value) = wrap_direction {
        ensure!(
            matches!(value, "clockwise" | "counterclockwise"),
            "[invalid-input] wrap direction must be clockwise or counterclockwise"
        );
    }
    let (layout_page, previous_board_mm, previous_bleed_mm, next_board_mm, next_bleed_mm) = {
        let dieline = candidate
            .get_mut("dielines")
            .and_then(|catalog| catalog.get_mut(dieline_id))
            .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?;
        let layout_page = dieline
            .get("layout_page")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let dimensions = dieline
            .get_mut("dimensions_mm")
            .and_then(Value::as_object_mut)
            .context("[invalid-input] dieline dimensions_mm must be an object")?;
        let previous_board_mm = dimensions
            .get("board")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let previous_bleed_mm = dimensions
            .get("bleed")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if let Some(value) = board_mm {
            dimensions.insert("board".into(), serde_json::json!(value));
        }
        if let Some(value) = bleed_mm {
            dimensions.insert("bleed".into(), serde_json::json!(value));
        }
        let next_board_mm = dimensions
            .get("board")
            .and_then(Value::as_f64)
            .unwrap_or(previous_board_mm);
        let next_bleed_mm = dimensions
            .get("bleed")
            .and_then(Value::as_f64)
            .unwrap_or(previous_bleed_mm);
        (
            layout_page,
            previous_board_mm,
            previous_bleed_mm,
            next_board_mm,
            next_bleed_mm,
        )
    };
    let (previous_seam_angle_deg, seam_angle_deg, previous_wrap_direction, wrap_direction) =
        if seam_angle_deg.is_some() || wrap_direction.is_some() {
            let dieline = candidate["dielines"]
                .get_mut(dieline_id)
                .context("[missing-resource] packaging dieline disappeared")?;
            ensure!(
                matches!(dieline["template"].as_str(), Some("tube" | "bottle-label")),
                "[invalid-input] wrap seam settings apply only to tube and bottle-label dielines"
            );
            let wrap = dieline
                .get_mut("wrap")
                .and_then(Value::as_object_mut)
                .context("[invalid-input] wrapped dieline needs wrap settings")?;
            let previous_seam = wrap
                .get("seam_angle_deg")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let previous_direction = wrap
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("clockwise")
                .to_owned();
            if let Some(value) = seam_angle_deg {
                wrap.insert("seam_angle_deg".into(), serde_json::json!(value));
            }
            if let Some(value) = wrap_direction {
                wrap.insert("direction".into(), serde_json::json!(value));
            }
            (
                Some(previous_seam),
                wrap.get("seam_angle_deg")
                    .and_then(Value::as_f64)
                    .or(Some(previous_seam)),
                Some(previous_direction),
                wrap.get("direction")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(Some("clockwise".to_owned())),
            )
        } else {
            (None, None, None, None)
        };
    if let Some(layout_page) = layout_page {
        let delta = next_bleed_mm - previous_bleed_mm;
        if delta.abs() > f64::EPSILON {
            translate_layout_page_for_bleed(&mut candidate, &layout_page, delta, next_bleed_mm)?;
        }
    }
    crate::scene::validate(&candidate)
        .context("[invalid-input] packaging settings produce an invalid dieline document")?;
    let result = serde_json::json!({
        "dieline":dieline_id,
        "previous_board_mm":previous_board_mm,
        "board_mm":next_board_mm,
        "previous_bleed_mm":previous_bleed_mm,
        "bleed_mm":next_bleed_mm,
        "previous_seam_angle_deg":previous_seam_angle_deg,
        "seam_angle_deg":seam_angle_deg,
        "previous_wrap_direction":previous_wrap_direction,
        "wrap_direction":wrap_direction
    });
    *raw = candidate;
    Ok(result)
}

fn add_generated_dieline(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    layer_id: &str,
    page_id: Option<&str>,
    placement: [f64; 4],
    mut dieline: Value,
) -> Result<Value> {
    ensure!(
        raw.get("version")
            .and_then(Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-capability] package mockups require document format v11 or newer"
    );
    ensure!(
        !dieline_id.trim().is_empty() && !node_id.trim().is_empty(),
        "[invalid-input] sleeve dieline and node IDs cannot be empty"
    );
    ensure!(
        raw.get("dielines")
            .and_then(|dielines| dielines.get(dieline_id))
            .is_none(),
        "[invalid-input] dieline `{dieline_id}` already exists"
    );
    ensure!(
        raw.get("dielines")
            .and_then(Value::as_object)
            .is_some_and(|dielines| dielines.len() < 256),
        "[limit-exceeded] a document holds at most 256 packaging dielines"
    );
    ensure!(
        placement
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            && placement[2] > 0.0
            && placement[3] > 0.0,
        "[invalid-input] sleeve node placement must have finite nonnegative origin and positive extent"
    );
    let page_id = page_id
        .map(str::to_owned)
        .or_else(|| {
            raw.get("pages")
                .and_then(Value::as_array)
                .and_then(|pages| pages.first())
                .and_then(|page| page.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .context("[invalid-input] sleeve target page is required")?;
    let mut candidate = raw.clone();
    let (layout_page_id, layout_page) =
        make_editable_layout_page(&candidate, dieline_id, &dieline)?;
    let pages = candidate
        .get("pages")
        .and_then(Value::as_array)
        .context("[invalid-input] document pages are missing")?
        .iter()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id.as_str()))
        .with_context(|| format!("[missing-resource] page `{page_id}` does not exist"))?;
    let layer = pages
        .get("layers")
        .and_then(Value::as_array)
        .context("[invalid-input] target page has no layers")?
        .iter()
        .find(|layer| layer.get("id").and_then(Value::as_str) == Some(layer_id))
        .with_context(|| {
            format!("[missing-resource] layer `{layer_id}` does not exist on page `{page_id}`")
        })?;
    ensure!(
        layer.get("locked").and_then(Value::as_bool) != Some(true),
        "[locked] target layer `{layer_id}` is locked"
    );
    layer
        .get("nodes")
        .and_then(Value::as_array)
        .context("[invalid-input] target layer has no node array")?;
    let spread_action = crate::page::PageAction::Add {
        id: layout_page_id.clone(),
        name: layout_page["name"].as_str().map(str::to_owned),
        width: layout_page["canvas"]["width"]
            .as_u64()
            .map(|value| value as u32),
        height: layout_page["canvas"]["height"]
            .as_u64()
            .map(|value| value as u32),
        background: layout_page["canvas"]["background"]
            .as_str()
            .map(str::to_owned),
        after: Some(page_id.clone()),
        before: None,
        parent: None,
        count: Some(1),
    };
    crate::spreads::apply_page(&mut candidate, spread_action)?;
    let created_page = candidate["pages"]
        .as_array_mut()
        .context("[invalid-input] document pages are missing")?
        .iter_mut()
        .find(|page| page["id"].as_str() == Some(layout_page_id.as_str()))
        .context("[invalid-input] spread planner did not create the dieline page")?;
    *created_page = layout_page;

    dieline["scene3d_node"] = serde_json::json!(node_id);
    dieline["layout_page"] = serde_json::json!(layout_page_id);
    candidate["dielines"][dieline_id] = dieline.clone();
    let page = candidate["pages"]
        .as_array_mut()
        .context("[invalid-input] document pages are missing")?
        .iter_mut()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id.as_str()))
        .unwrap();
    let layer = page["layers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|layer| layer.get("id").and_then(Value::as_str) == Some(layer_id))
        .unwrap();
    let nodes = layer["nodes"].as_array_mut().unwrap();
    nodes.push(serde_json::json!({
        "kind":"scene3d", "id":node_id, "x":placement[0], "y":placement[1],
        "width":placement[2], "height":placement[3], "dieline":dieline_id
    }));
    *raw = candidate;
    Ok(serde_json::json!({
        "node":node_id,"dieline":dieline_id,
        "template":dieline["template"],"panels":dieline["panels"].as_array().map_or(0, Vec::len),
        "fold_state":dieline["fold_state"]
    }))
}

fn make_editable_layout_page(
    raw: &Value,
    dieline_id: &str,
    dieline: &Value,
) -> Result<(String, Value)> {
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .context("[invalid-input] document pages are missing")?;
    ensure!(
        pages.len() < 5000,
        "[limit-exceeded] adding the editable dieline page would exceed 5000 document pages"
    );
    let mut layout_page_id = format!("{dieline_id}-flat");
    let mut suffix = 2_u32;
    while pages
        .iter()
        .any(|page| page["id"].as_str() == Some(layout_page_id.as_str()))
    {
        layout_page_id = format!("{dieline_id}-flat-{suffix}");
        suffix = suffix.saturating_add(1);
        ensure!(
            suffix < u32::MAX,
            "[limit-exceeded] could not allocate a unique dieline page ID"
        );
    }
    let bleed = dieline["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels must be an array")?;
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;
    let mut guides = Vec::new();
    let mut vectors = Vec::new();
    for panel in panels {
        let id = panel["id"]
            .as_str()
            .context("[invalid-input] dieline panel ID is missing")?;
        let rect = panel["rect_mm"]
            .as_array()
            .filter(|rect| rect.len() == 4)
            .context("[invalid-input] panel rect_mm must contain four values")?;
        let (x, y, width, height) = (
            rect[0].as_f64().unwrap_or(f64::NAN),
            rect[1].as_f64().unwrap_or(f64::NAN),
            rect[2].as_f64().unwrap_or(f64::NAN),
            rect[3].as_f64().unwrap_or(f64::NAN),
        );
        ensure!(
            [x, y, width, height].iter().all(|value| value.is_finite())
                && x >= 0.0
                && y >= 0.0
                && width > 0.0
                && height > 0.0,
            "[invalid-input] panel `{id}` has invalid flat bounds"
        );
        max_x = max_x.max(x + width);
        max_y = max_y.max(y + height);
        let safe = panel["safe_zone_mm"].as_f64().unwrap_or(0.0);
        guides.push(serde_json::json!({
            "kind":"rect", "id":format!("{layout_page_id}-panel-{id}"), "name":format!("Panel · {id}"),
            "dieline_panel_id":id, "x":x+bleed, "y":y+bleed, "width":width, "height":height,
            "style":{"fill":{"fallback":"none"},"stroke":{"fallback":"#94a3b8"},"stroke_width":{"fallback":0.15}}
        }));
        if safe > 0.0 {
            guides.push(serde_json::json!({
                "kind":"rect", "id":format!("{layout_page_id}-safe-{id}"), "name":format!("Safe area · {id}"),
                "dieline_safe_zone_panel":id, "x":x+bleed+safe, "y":y+bleed+safe, "width":width-2.0*safe, "height":height-2.0*safe,
                "style":{"fill":{"fallback":"none"},"stroke":{"fallback":role_color("safe-zone")},"stroke_width":{"fallback":0.2},"stroke_dasharray":{"fallback":"1.2 0.8"}}
            }));
        }
        guides.push(serde_json::json!({
            "kind":"text", "id":format!("{layout_page_id}-label-{id}"), "name":format!("Panel label · {id}"),
            "dieline_panel_label":id,
            "content":id, "x":x+bleed+width/2.0, "y":y+bleed+height/2.0+1.5, "font_size":3.0, "align":"center",
            "style":{"fill":{"fallback":"#475569"}}
        }));
        let [left, right, top, bottom] = panel_cut_edges(dieline, panel)?;
        if bleed > 0.0 {
            let mut data = String::new();
            if left {
                data.push_str(&format!("M {} {} V {} ", x, y + bleed, y + bleed + height));
            }
            if right {
                data.push_str(&format!(
                    "M {} {} V {} ",
                    x + width + 2.0 * bleed,
                    y + bleed,
                    y + bleed + height
                ));
            }
            if top {
                data.push_str(&format!("M {} {} H {} ", x + bleed, y, x + bleed + width));
            }
            if bottom {
                data.push_str(&format!(
                    "M {} {} H {} ",
                    x + bleed,
                    y + height + 2.0 * bleed,
                    x + bleed + width
                ));
            }
            if !data.is_empty() {
                guides.push(serde_json::json!({"kind":"path","id":format!("{layout_page_id}-bleed-{id}"),"name":format!("Bleed guide · {id}"),"d":data,"style":{"fill":{"fallback":"none"},"stroke":{"fallback":role_color("bleed")},"stroke_width":{"fallback":0.2},"stroke_dasharray":{"fallback":"1.2 0.8"}}}));
            }
        }
    }
    for path in dieline["paths"]
        .as_array()
        .context("[invalid-input] dieline paths must be an array")?
    {
        let id = path["id"]
            .as_str()
            .context("[invalid-input] production path ID is missing")?;
        let role = path["role"]
            .as_str()
            .context("[invalid-input] production path role is missing")?;
        if role == "crease" {
            continue;
        }
        let data = path["d"]
            .as_str()
            .context("[invalid-input] production path data is missing")?;
        let mut bez = kurbo::BezPath::from_svg(data).with_context(|| {
            format!("[invalid-input] production path `{id}` has invalid SVG data")
        })?;
        bez.apply_affine(kurbo::Affine::translate((bleed, bleed)));
        vectors.push(serde_json::json!({
            "kind":"path", "id":format!("{layout_page_id}-path-{id}"), "name":format!("{role} · {id}"),
            "dieline_path_id":id, "dieline_path_role":role, "dieline_path_spot":path["spot"], "d":bez.to_svg(),
            "style":{"fill":{"fallback":"none"},"stroke":{"fallback":role_color(role)},"stroke_width":{"fallback":0.2},"stroke_linecap":{"fallback":"butt"},"stroke_linejoin":{"fallback":"round"}}
        }));
    }
    for crease in dieline["creases"]
        .as_array()
        .context("[invalid-input] dieline creases must be an array")?
    {
        let id = crease["id"]
            .as_str()
            .context("[invalid-input] crease ID is missing")?;
        let line = crease["line_mm"]
            .as_array()
            .filter(|points| points.len() == 2)
            .context("[invalid-input] crease line must have two points")?;
        let start = pair(&line[0], "crease start")?;
        let end = pair(&line[1], "crease end")?;
        ensure!(
            start[0] >= 0.0 && start[1] >= 0.0 && end[0] >= 0.0 && end[1] >= 0.0,
            "[invalid-input] crease `{id}` has negative editable-layout bounds"
        );
        max_x = max_x.max(start[0].max(end[0]));
        max_y = max_y.max(start[1].max(end[1]));
        vectors.push(serde_json::json!({
            "kind":"path", "id":format!("{layout_page_id}-crease-{id}"), "name":format!("Crease · {id}"),
            "dieline_crease_id":id, "d":format!("M {} {} L {} {}",start[0]+bleed,start[1]+bleed,end[0]+bleed,end[1]+bleed),
            "style":{"fill":{"fallback":"none"},"stroke":{"fallback":role_color("crease")},"stroke_width":{"fallback":0.2},"stroke_linecap":{"fallback":"butt"}}
        }));
    }
    for path in dieline["paths"]
        .as_array()
        .context("[invalid-input] dieline paths must be an array")?
    {
        let id = path["id"]
            .as_str()
            .context("[invalid-input] production path ID is missing")?;
        let data = path["d"]
            .as_str()
            .context("[invalid-input] production path data is missing")?;
        let bounds = kurbo::BezPath::from_svg(data)
            .with_context(|| {
                format!("[invalid-input] production path `{id}` has invalid SVG data")
            })?
            .bounding_box();
        ensure!(
            [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
                .iter()
                .all(|value| value.is_finite())
                && bounds.x0 >= 0.0
                && bounds.y0 >= 0.0,
            "[invalid-input] production path `{id}` has invalid or negative editable-layout bounds"
        );
        max_x = max_x.max(bounds.x1);
        max_y = max_y.max(bounds.y1);
    }
    let output_width = max_x + 2.0 * bleed;
    let output_height = max_y + 2.0 * bleed;
    ensure!(
        output_width.is_finite()
            && output_height.is_finite()
            && output_width > 0.0
            && output_height > 0.0
            && output_width.ceil() <= 16384.0
            && output_height.ceil() <= 16384.0,
        "[limit-exceeded] editable dieline page dimensions must fit the 16384 mm canvas limit"
    );
    let page = serde_json::json!({
        "id":layout_page_id,
        "name":format!("{dieline_id} · Flat dieline"),
        "canvas":{"width":output_width.ceil() as u32,"height":output_height.ceil() as u32,"background":"#ffffff"},
        "production":{"units_per_inch":25.4,"trim":{"x":bleed,"y":bleed,"width":max_x,"height":max_y},"bleed":bleed},
        "dieline_layout":{"dieline":dieline_id,"coordinates":"mm"},
        "layers":[
            {"id":format!("{layout_page_id}-guides"),"name":"Panels and print guides","visible":true,"locked":false,"nodes":guides},
            {"id":format!("{layout_page_id}-production"),"name":"Cut, crease and production paths","visible":true,"locked":false,"nodes":vectors}
        ]
    });
    Ok((layout_page_id, page))
}

fn flatten_panel_contour(data: &str, id: &str) -> Result<Vec<[f64; 2]>> {
    let path = kurbo::BezPath::from_svg(data)
        .with_context(|| format!("[invalid-input] cut contour for panel `{id}` is malformed"))?;
    ensure!(
        path.is_finite(),
        "[invalid-input] cut contour for panel `{id}` contains non-finite geometry"
    );
    let mut intersection_work = 0;
    ensure!(
        !svg_path_self_intersects(&path, &mut intersection_work)?,
        "[invalid-input] cut contour for panel `{id}` self-intersects"
    );
    let mut points = Vec::new();
    let mut moves = 0;
    let mut closes = 0;
    kurbo::flatten(&path, 0.1, |element| match element {
        kurbo::PathEl::MoveTo(point) => {
            moves += 1;
            points.push([point.x, point.y]);
        }
        kurbo::PathEl::LineTo(point) => points.push([point.x, point.y]),
        kurbo::PathEl::ClosePath => closes += 1,
        kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..) => {
            // `BezPath::flatten` emits only move, line, and close elements.
        }
    });
    ensure!(
        moves == 1 && closes == 1,
        "[invalid-input] cut contour for panel `{id}` must be one closed, non-holed outline"
    );
    if points.len() > 1 && points[0] == *points.last().unwrap() {
        points.pop();
    }
    ensure!(
        (3..=2048).contains(&points.len()),
        "[limit-exceeded] cut contour for panel `{id}` must flatten to 3..=2048 vertices at 0.1 mm tolerance"
    );
    Ok(points)
}

fn contour_covers_edge(points: &[[f64; 2]], edge: [[f64; 2]; 2]) -> bool {
    let scale = points
        .iter()
        .flatten()
        .copied()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    let tolerance = scale * 1.0e-8;
    let horizontal = (edge[0][1] - edge[1][1]).abs() <= tolerance;
    let fixed = if horizontal { edge[0][1] } else { edge[0][0] };
    let range = if horizontal {
        [edge[0][0].min(edge[1][0]), edge[0][0].max(edge[1][0])]
    } else {
        [edge[0][1].min(edge[1][1]), edge[0][1].max(edge[1][1])]
    };
    let mut intervals = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .filter_map(|(a, b)| {
            let (a_fixed, b_fixed, a_axis, b_axis) = if horizontal {
                (a[1], b[1], a[0], b[0])
            } else {
                (a[0], b[0], a[1], b[1])
            };
            ((a_fixed - fixed).abs() <= tolerance && (b_fixed - fixed).abs() <= tolerance)
                .then_some([a_axis.min(b_axis), a_axis.max(b_axis)])
        })
        .collect::<Vec<_>>();
    intervals.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut covered = range[0];
    for interval in intervals {
        if interval[0] > covered + tolerance {
            return false;
        }
        covered = covered.max(interval[1]);
        if covered >= range[1] - tolerance {
            return true;
        }
    }
    false
}

fn triangulate_simple_polygon(points: &[[f64; 2]], id: &str) -> Result<Vec<u32>> {
    let area2: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum();
    ensure!(
        area2.is_finite() && area2.abs() > 1e-10,
        "[invalid-input] cut contour for panel `{id}` has zero area"
    );
    let orientation = area2.signum();
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let inside_triangle = |point: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        let epsilon = 1e-10;
        cross(a, b, point) * orientation >= -epsilon
            && cross(b, c, point) * orientation >= -epsilon
            && cross(c, a, point) * orientation >= -epsilon
    };
    let mut remaining = (0..points.len()).collect::<Vec<_>>();
    let mut triangles = Vec::with_capacity((points.len() - 2) * 3);
    while remaining.len() > 3 {
        let mut ear = None;
        for index in 0..remaining.len() {
            let prev = remaining[(index + remaining.len() - 1) % remaining.len()];
            let current = remaining[index];
            let next = remaining[(index + 1) % remaining.len()];
            if cross(points[prev], points[current], points[next]) * orientation <= 1e-10 {
                continue;
            }
            if remaining.iter().copied().any(|candidate| {
                candidate != prev
                    && candidate != current
                    && candidate != next
                    && inside_triangle(
                        points[candidate],
                        points[prev],
                        points[current],
                        points[next],
                    )
            }) {
                continue;
            }
            ear = Some((index, [prev, current, next]));
            break;
        }
        let (index, mut triangle) = ear.with_context(|| {
            format!("[invalid-input] cut contour for panel `{id}` cannot be triangulated")
        })?;
        if orientation > 0.0 {
            triangle.swap(1, 2);
        }
        triangles.extend(triangle.map(|vertex| vertex as u32));
        remaining.remove(index);
    }
    let mut final_triangle = [remaining[0], remaining[1], remaining[2]];
    if orientation > 0.0 {
        final_triangle.swap(1, 2);
    }
    triangles.extend(final_triangle.map(|vertex| vertex as u32));
    Ok(triangles)
}

#[derive(Clone)]
struct Panel {
    id: String,
    slot: String,
    rect: [f64; 4],
    uv: [f64; 4],
    contour: Option<Vec<[f64; 2]>>,
}

#[derive(Clone)]
struct Hinge {
    to: usize,
    line: [[f64; 2]; 2],
    angle: f64,
    direction: f64,
}

fn number(value: &Value, label: &str, min: f64, max: f64) -> Result<f64> {
    let value = value
        .as_f64()
        .with_context(|| format!("[invalid-input] {label} must be a number"))?;
    ensure!(
        value.is_finite() && (min..=max).contains(&value),
        "[invalid-input] {label} must be within {min}..={max}"
    );
    Ok(value)
}

fn pair(value: &Value, label: &str) -> Result<[f64; 2]> {
    let values = value
        .as_array()
        .filter(|values| values.len() == 2)
        .with_context(|| format!("[invalid-input] {label} must be [x,y]"))?;
    Ok([
        number(&values[0], &format!("{label} x"), 0.0, 1_000_000.0)?,
        number(&values[1], &format!("{label} y"), 0.0, 1_000_000.0)?,
    ])
}

fn rect(value: &Value, label: &str, uv: bool) -> Result<[f64; 4]> {
    let values = value
        .as_array()
        .filter(|values| values.len() == 4)
        .with_context(|| format!("[invalid-input] {label} must be [x,y,width,height]"))?;
    let max_position = if uv { 1.0 } else { 1_000_000.0 };
    let max_size = if uv { 1.0 } else { 1_000_000.0 };
    let out = [
        number(&values[0], &format!("{label} x"), 0.0, max_position)?,
        number(&values[1], &format!("{label} y"), 0.0, max_position)?,
        number(
            &values[2],
            &format!("{label} width"),
            f64::MIN_POSITIVE,
            max_size,
        )?,
        number(
            &values[3],
            &format!("{label} height"),
            f64::MIN_POSITIVE,
            max_size,
        )?,
    ];
    if uv {
        ensure!(
            out[0] + out[2] <= 1.0 && out[1] + out[3] <= 1.0,
            "[invalid-input] {label} extends outside normalized UV space"
        );
    }
    Ok(out)
}

fn panel_adjacent(a: [f64; 4], b: [f64; 4]) -> Option<[[f64; 2]; 2]> {
    const EPS: f64 = 1e-6;
    let ar = a[0] + a[2];
    let ab = a[1] + a[3];
    let br = b[0] + b[2];
    let bb = b[1] + b[3];
    if (ar - b[0]).abs() <= EPS && (a[1] - b[1]).abs() <= EPS && (ab - bb).abs() <= EPS {
        Some([[ar, a[1]], [ar, ab]])
    } else if (br - a[0]).abs() <= EPS && (a[1] - b[1]).abs() <= EPS && (ab - bb).abs() <= EPS {
        Some([[a[0], a[1]], [a[0], ab]])
    } else if (ab - b[1]).abs() <= EPS && (a[0] - b[0]).abs() <= EPS && (ar - br).abs() <= EPS {
        Some([[a[0], ab], [ar, ab]])
    } else if (bb - a[1]).abs() <= EPS && (a[0] - b[0]).abs() <= EPS && (ar - br).abs() <= EPS {
        Some([[a[0], a[1]], [ar, a[1]]])
    } else {
        None
    }
}

/// Like [`panel_adjacent`], but also accepts a hinge that covers only part of
/// the longer panel's edge (an inset flap), and only when a stored crease runs
/// exactly along that shorter shared segment. Partial contact without a matching
/// crease stays non-adjacent, as before.
fn panel_hinged(a: [f64; 4], b: [f64; 4], crease_lines: &[[[f64; 2]; 2]]) -> Option<[[f64; 2]; 2]> {
    const EPS: f64 = 1e-6;
    if let Some(edge) = panel_adjacent(a, b) {
        return Some(edge);
    }
    let nested = |a0: f64, a1: f64, b0: f64, b1: f64| {
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        let matches_a = (lo - a0).abs() <= EPS && (hi - a1).abs() <= EPS;
        let matches_b = (lo - b0).abs() <= EPS && (hi - b1).abs() <= EPS;
        (hi - lo > EPS && (matches_a || matches_b)).then_some((lo, hi))
    };
    let (ar, ab) = (a[0] + a[2], a[1] + a[3]);
    let (br, bb) = (b[0] + b[2], b[1] + b[3]);
    let edge = if (ar - b[0]).abs() <= EPS {
        nested(a[1], ab, b[1], bb).map(|(lo, hi)| [[ar, lo], [ar, hi]])
    } else if (br - a[0]).abs() <= EPS {
        nested(a[1], ab, b[1], bb).map(|(lo, hi)| [[a[0], lo], [a[0], hi]])
    } else if (ab - b[1]).abs() <= EPS {
        nested(a[0], ar, b[0], br).map(|(lo, hi)| [[lo, ab], [hi, ab]])
    } else if (bb - a[1]).abs() <= EPS {
        nested(a[0], ar, b[0], br).map(|(lo, hi)| [[lo, a[1]], [hi, a[1]]])
    } else {
        None
    }?;
    crease_lines
        .iter()
        .any(|line| same_edge(*line, edge))
        .then_some(edge)
}

fn same_edge(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    const EPS: f64 = 1e-6;
    ((a[0][0] - b[0][0]).abs() <= EPS
        && (a[0][1] - b[0][1]).abs() <= EPS
        && (a[1][0] - b[1][0]).abs() <= EPS
        && (a[1][1] - b[1][1]).abs() <= EPS)
        || ((a[0][0] - b[1][0]).abs() <= EPS
            && (a[0][1] - b[1][1]).abs() <= EPS
            && (a[1][0] - b[0][0]).abs() <= EPS
            && (a[1][1] - b[0][1]).abs() <= EPS)
}

fn panel_boundary(panel: &Panel) -> Vec<[f64; 2]> {
    panel.contour.clone().unwrap_or_else(|| {
        let [x, y, width, height] = panel.rect;
        vec![
            [x, y],
            [x + width, y],
            [x + width, y + height],
            [x, y + height],
        ]
    })
}

fn point_on_segment(point: [f64; 2], segment: [[f64; 2]; 2]) -> bool {
    const EPS: f64 = 1e-7;
    let [a, b] = segment;
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [point[0] - a[0], point[1] - a[1]];
    let length_squared = ab[0] * ab[0] + ab[1] * ab[1];
    let cross = ab[0] * ap[1] - ab[1] * ap[0];
    let projection = ap[0] * ab[0] + ap[1] * ab[1];
    cross.abs() <= EPS * length_squared.sqrt().max(1.0)
        && projection >= -EPS
        && projection <= length_squared + EPS
}

fn panel_edge_is_hinge(edge: [[f64; 2]; 2], hinges: &[Hinge]) -> bool {
    hinges
        .iter()
        .any(|hinge| point_on_segment(edge[0], hinge.line) && point_on_segment(edge[1], hinge.line))
}

/// Return the exposed outline of an axis-aligned panel net as closed SVG
/// contours. Shared edges (including partial shared edges) are fold joints,
/// not through-cuts, and must not be duplicated on the cut plate.
fn rectangle_net_cut_path(rectangles: &[[f64; 4]], creases: &[Value]) -> String {
    const EPS: f64 = 1e-7;
    let mut edges = Vec::<[[f64; 2]; 2]>::new();
    let mut cut_slits = Vec::<[[f64; 2]; 2]>::new();
    let crease_lines = creases
        .iter()
        .filter_map(|crease| crease.get("line_mm")?.as_array())
        .filter_map(|line| Some([line[0].as_array()?, line[1].as_array()?]))
        .filter_map(|line| {
            Some([
                [line[0][0].as_f64()?, line[0][1].as_f64()?],
                [line[1][0].as_f64()?, line[1][1].as_f64()?],
            ])
        })
        .collect::<Vec<_>>();
    let is_crease = |edge: [[f64; 2]; 2]| {
        crease_lines.iter().any(|crease| {
            if (edge[0][1] - edge[1][1]).abs() <= EPS && (crease[0][1] - crease[1][1]).abs() <= EPS
            {
                (edge[0][1] - crease[0][1]).abs() <= EPS
                    && edge[0][0].min(edge[1][0]) >= crease[0][0].min(crease[1][0]) - EPS
                    && edge[0][0].max(edge[1][0]) <= crease[0][0].max(crease[1][0]) + EPS
            } else if (edge[0][0] - edge[1][0]).abs() <= EPS
                && (crease[0][0] - crease[1][0]).abs() <= EPS
            {
                (edge[0][0] - crease[0][0]).abs() <= EPS
                    && edge[0][1].min(edge[1][1]) >= crease[0][1].min(crease[1][1]) - EPS
                    && edge[0][1].max(edge[1][1]) <= crease[0][1].max(crease[1][1]) + EPS
            } else {
                false
            }
        })
    };
    for (index, &[x, y, width, height]) in rectangles.iter().enumerate() {
        let sides = [
            [[x, y], [x + width, y]],
            [[x + width, y], [x + width, y + height]],
            [[x + width, y + height], [x, y + height]],
            [[x, y + height], [x, y]],
        ];
        for side in sides {
            let horizontal = (side[0][1] - side[1][1]).abs() <= EPS;
            let axis = usize::from(!horizontal);
            let fixed = side[0][1 - axis];
            let low = side[0][axis].min(side[1][axis]);
            let high = side[0][axis].max(side[1][axis]);
            let mut breaks = vec![low, high];
            for (other_index, &[ox, oy, ow, oh]) in rectangles.iter().enumerate() {
                if index == other_index {
                    continue;
                }
                let other = if horizontal {
                    [ox, ox + ow, oy, oy + oh]
                } else {
                    [oy, oy + oh, ox, ox + ow]
                };
                if (other[2] - fixed).abs() <= EPS || (other[3] - fixed).abs() <= EPS {
                    for coordinate in [other[0], other[1]] {
                        if coordinate > low + EPS && coordinate < high - EPS {
                            breaks.push(coordinate);
                        }
                    }
                }
            }
            breaks.sort_by(f64::total_cmp);
            breaks.dedup_by(|a, b| (*a - *b).abs() <= EPS);
            for interval in breaks.windows(2) {
                let (start, end) = (interval[0], interval[1]);
                if end - start <= EPS {
                    continue;
                }
                let middle = (start + end) / 2.0;
                let shared = rectangles.iter().enumerate().any(|(other_index, other)| {
                    if index == other_index {
                        return false;
                    }
                    let [ox, oy, ow, oh] = *other;
                    let (other_low, other_high, other_fixed_a, other_fixed_b) = if horizontal {
                        (ox, ox + ow, oy, oy + oh)
                    } else {
                        (oy, oy + oh, ox, ox + ow)
                    };
                    ((other_fixed_a - fixed).abs() <= EPS || (other_fixed_b - fixed).abs() <= EPS)
                        && middle >= other_low - EPS
                        && middle <= other_high + EPS
                });
                let point = |coordinate: f64| {
                    if horizontal {
                        [coordinate, fixed]
                    } else {
                        [fixed, coordinate]
                    }
                };
                let a = point(start);
                let b = point(end);
                let edge = if side[0][axis] <= side[1][axis] {
                    [a, b]
                } else {
                    [b, a]
                };
                if shared {
                    if is_crease(edge) {
                        continue;
                    }
                    if !cut_slits.iter().any(|existing| same_edge(*existing, edge)) {
                        cut_slits.push(edge);
                    }
                } else if !edges.iter().any(|existing| same_edge(*existing, edge)) {
                    edges.push(edge);
                }
            }
        }
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() <= 1e-6 && (a[1] - b[1]).abs() <= 1e-6
    }
    let mut paths = Vec::new();
    while let Some(first) = edges.pop() {
        let start = first[0];
        let mut points = vec![start, first[1]];
        while !close(*points.last().unwrap(), start) {
            let end = *points.last().unwrap();
            let Some(index) = edges
                .iter()
                .position(|edge| close(edge[0], end) || close(edge[1], end))
            else {
                break;
            };
            let edge = edges.swap_remove(index);
            points.push(if close(edge[0], end) {
                edge[1]
            } else {
                edge[0]
            });
        }
        let closed = points.len() > 2 && close(*points.last().unwrap(), start);
        if closed {
            points.pop();
        }
        let mut path = format!("M {} {}", points[0][0], points[0][1]);
        for point in points.iter().skip(1) {
            path.push_str(&format!(" L {} {}", point[0], point[1]));
        }
        if closed {
            path.push_str(" Z");
        }
        paths.push(path);
    }
    paths.extend(cut_slits.into_iter().map(|edge| {
        format!(
            "M {} {} L {} {}",
            edge[0][0], edge[0][1], edge[1][0], edge[1][1]
        )
    }));
    paths.join(" ")
}

fn panel_has_edge(rect: [f64; 4], edge: [[f64; 2]; 2]) -> bool {
    let [x, y, width, height] = rect;
    [
        [[x, y], [x + width, y]],
        [[x + width, y], [x + width, y + height]],
        [[x + width, y + height], [x, y + height]],
        [[x, y + height], [x, y]],
    ]
    .iter()
    .any(|panel_edge| same_edge(*panel_edge, edge))
}

fn transform_flat(m: &Mat4, point: [f64; 2], z: f64) -> Vec3 {
    math::transform_point(m, [point[0], -point[1], z])
}

fn rotate_about_line(a: Vec3, b: Vec3, angle: f64) -> Mat4 {
    let axis = math::normalize(math::sub(b, a));
    let (x, y, z) = (axis[0], axis[1], axis[2]);
    let (s, c) = (math::sin(angle), math::cos(angle));
    let t = 1.0 - c;
    let (r00, r01, r02) = (t * x * x + c, t * x * y - s * z, t * x * z + s * y);
    let (r10, r11, r12) = (t * x * y + s * z, t * y * y + c, t * y * z - s * x);
    let (r20, r21, r22) = (t * x * z - s * y, t * y * z + s * x, t * z * z + c);
    let rotated_a = [
        r00 * a[0] + r01 * a[1] + r02 * a[2],
        r10 * a[0] + r11 * a[1] + r12 * a[2],
        r20 * a[0] + r21 * a[1] + r22 * a[2],
    ];
    [
        r00,
        r10,
        r20,
        0.0,
        r01,
        r11,
        r21,
        0.0,
        r02,
        r12,
        r22,
        0.0,
        a[0] - rotated_a[0],
        a[1] - rotated_a[1],
        a[2] - rotated_a[2],
        1.0,
    ]
}

fn child_side(line: [[f64; 2]; 2], panel: [f64; 4]) -> f64 {
    let center = [panel[0] + panel[2] / 2.0, panel[1] + panel[3] / 2.0];
    let dx = line[1][0] - line[0][0];
    let dy = -(line[1][1] - line[0][1]);
    let cx = center[0] - line[0][0];
    let cy = -(center[1] - line[0][1]);
    if dx * cy - dy * cx >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

fn build_transforms(
    panels: &[Panel],
    adjacency: &[Vec<Hinge>],
    fold_state: f64,
    board_thickness: f64,
) -> Result<Vec<Mat4>> {
    ensure!(
        panels.len() <= super::model::LIMITS.max_primitives,
        "[limit-exceeded] folded dieline exceeds the primitive limit"
    );
    let mut transforms = vec![None; panels.len()];
    transforms[0] = Some(IDENTITY);
    let mut queue = VecDeque::from([0usize]);
    let mut visited_edges = 0usize;
    while let Some(parent) = queue.pop_front() {
        let parent_transform = transforms[parent].unwrap();
        for hinge in &adjacency[parent] {
            if transforms[hinge.to].is_some() {
                continue;
            }
            let hinge_surface_offset = board_thickness / 2.0;
            let a = transform_flat(&parent_transform, hinge.line[0], hinge_surface_offset);
            let b = transform_flat(&parent_transform, hinge.line[1], hinge_surface_offset);
            let angle = -hinge.angle
                * fold_state
                * hinge.direction
                * child_side(hinge.line, panels[hinge.to].rect)
                * math::PI
                / 180.0;
            let rotation = rotate_about_line(a, b, angle);
            transforms[hinge.to] = Some(math::mul(&rotation, &parent_transform));
            visited_edges += 1;
            queue.push_back(hinge.to);
        }
    }
    ensure!(
        transforms.iter().all(Option::is_some),
        "[invalid-input] dieline panels must form one connected foldable net"
    );
    let graph_edges = adjacency.iter().map(Vec::len).sum::<usize>() / 2;
    ensure!(
        visited_edges == panels.len() - 1 && graph_edges == panels.len() - 1,
        "[unsupported-capability] dieline panel graph contains a cycle; only tree-shaped fold nets are currently supported"
    );
    Ok(transforms.into_iter().map(Option::unwrap).collect())
}

/// Build a deterministic curved mesh for one flat tube label.
fn generate_tube(dieline: &Value) -> Result<Model> {
    const SEGMENTS: usize = TUBE_SEGMENTS;
    let wrap = dieline
        .get("wrap")
        .context("[invalid-input] tube dieline needs a wrap mapping")?;
    let radius = number(
        wrap.get("radius_mm").unwrap_or(&Value::Null),
        "tube wrap radius_mm",
        1.0,
        100_000.0,
    )?;
    let circumference = std::f64::consts::TAU * radius;
    let overlap = wrap
        .get("overlap_mm")
        .map(|value| {
            value
                .as_f64()
                .context("[invalid-input] tube overlap_mm must be a number")
        })
        .transpose()?
        .unwrap_or(0.0);
    ensure!(
        overlap.is_finite() && (0.0..=circumference * 0.25).contains(&overlap),
        "[invalid-input] tube overlap_mm must be within 0..=25% of its measured circumference"
    );
    let seam = number(
        wrap.get("seam_angle_deg").unwrap_or(&Value::Null),
        "tube seam_angle_deg",
        -360.0,
        360.0,
    )?;
    let direction = match wrap.get("direction").and_then(Value::as_str) {
        Some("clockwise") => 1.0,
        Some("counterclockwise") => -1.0,
        _ => bail!("[invalid-input] tube wrap direction must be clockwise or counterclockwise"),
    };
    let panels = dieline
        .get("panels")
        .and_then(Value::as_array)
        .context("[invalid-input] tube panels must be an array")?;
    ensure!(
        panels.len() == 1,
        "[invalid-input] tube labels require exactly one flat panel"
    );
    let panel = &panels[0];
    let id = panel
        .get("id")
        .and_then(Value::as_str)
        .context("[invalid-input] tube label panel needs an ID")?;
    let slot = panel
        .get("face_slot")
        .and_then(Value::as_str)
        .context("[invalid-input] tube label panel needs a material slot")?;
    let [_x, y, width, height] = rect(
        panel.get("rect_mm").unwrap_or(&Value::Null),
        "tube panel rect_mm",
        false,
    )?;
    let uv = rect(
        panel.get("uv_rect").unwrap_or(&Value::Null),
        "tube panel uv_rect",
        true,
    )?;
    ensure!(
        (width - circumference - overlap).abs() <= circumference * 1e-6,
        "[invalid-input] tube label panel width must equal 2π × wrap radius_mm plus overlap_mm (expected {:.6} mm)",
        circumference + overlap
    );
    let phase = seam.to_radians();
    let mut model = Model {
        materials: vec![Material {
            name: slot.to_owned(),
            ..Material::default()
        }],
        ..Model::default()
    };
    let main_u_fraction = circumference / width;
    for index in 0..SEGMENTS {
        let t0 = index as f64 / SEGMENTS as f64;
        let t1 = (index + 1) as f64 / SEGMENTS as f64;
        let angle0 = phase + direction * std::f64::consts::TAU * t0;
        let angle1 = phase + direction * std::f64::consts::TAU * t1;
        let point = |angle: f64, y: f64, r: f64| {
            [(r * angle.sin()) as f32, y as f32, (r * angle.cos()) as f32]
        };
        let positions = vec![
            point(angle0, y, radius),
            point(angle1, y, radius),
            point(angle1, y + height, radius),
            point(angle0, y + height, radius),
        ];
        let middle = (angle0 + angle1) * 0.5;
        let normal = [middle.sin() as f32, 0.0, middle.cos() as f32];
        let u0 = uv[0] + uv[2] * main_u_fraction * t0;
        let u1 = uv[0] + uv[2] * main_u_fraction * t1;
        let v0 = uv[1];
        let v1 = uv[1] + uv[3];
        model.primitives.push(Primitive {
            name: format!("{id}-wrap-{index:02}"),
            positions,
            normals: vec![normal; 4],
            uvs: vec![vec![
                [u0 as f32, v0 as f32],
                [u1 as f32, v0 as f32],
                [u1 as f32, v1 as f32],
                [u0 as f32, v1 as f32],
            ]],
            indices: if direction > 0.0 {
                vec![0, 1, 2, 0, 2, 3]
            } else {
                vec![0, 2, 1, 0, 3, 2]
            },
            material: Some(0),
        });
        model.instances.push(Instance {
            id: format!("{id}-wrap-{index:02}"),
            primitive: index,
            transform: IDENTITY,
        });
    }
    let overlap_segments = if overlap > 0.0 {
        ((overlap / circumference * SEGMENTS as f64).ceil() as usize).max(1)
    } else {
        0
    };
    let overlap_fraction = overlap / circumference;
    for index in 0..overlap_segments {
        let local0 = index as f64 / overlap_segments as f64;
        let local1 = (index + 1) as f64 / overlap_segments as f64;
        let t0 = overlap_fraction * local0;
        let t1 = overlap_fraction * local1;
        let angle0 = phase + direction * std::f64::consts::TAU * t0;
        let angle1 = phase + direction * std::f64::consts::TAU * t1;
        let raised_radius = radius + 0.05;
        let point = |angle: f64, yy: f64| {
            [
                (raised_radius * angle.sin()) as f32,
                yy as f32,
                (raised_radius * angle.cos()) as f32,
            ]
        };
        let primitive_index = model.primitives.len();
        let name = format!("{id}-overlap-{index:02}");
        let u0 = uv[0] + uv[2] * (main_u_fraction + (1.0 - main_u_fraction) * local0);
        let u1 = uv[0] + uv[2] * (main_u_fraction + (1.0 - main_u_fraction) * local1);
        let v0 = uv[1];
        let v1 = uv[1] + uv[3];
        let middle = (angle0 + angle1) * 0.5;
        model.primitives.push(Primitive {
            name: name.clone(),
            positions: vec![
                point(angle0, y),
                point(angle1, y),
                point(angle1, y + height),
                point(angle0, y + height),
            ],
            normals: vec![[(middle.sin()) as f32, 0.0, (middle.cos()) as f32]; 4],
            uvs: vec![vec![
                [u0 as f32, v0 as f32],
                [u1 as f32, v0 as f32],
                [u1 as f32, v1 as f32],
                [u0 as f32, v1 as f32],
            ]],
            indices: if direction > 0.0 {
                vec![0, 1, 2, 0, 2, 3]
            } else {
                vec![0, 2, 1, 0, 3, 2]
            },
            material: Some(0),
        });
        model.instances.push(Instance {
            id: name,
            primitive: primitive_index,
            transform: IDENTITY,
        });
    }
    model.report.format = if dieline["template"] == "bottle-label" {
        "pentool-bottle-label".into()
    } else {
        "pentool-tube-label".into()
    };
    model.report.vertices = model
        .primitives
        .iter()
        .map(|primitive| primitive.positions.len())
        .sum();
    model.report.triangles = model.primitives.iter().map(Primitive::triangle_count).sum();
    Ok(model)
}

/// Build a CPU-renderable folded panel mesh from a rectangular dieline.
pub fn generate(dieline: &Value) -> Result<Model> {
    let template = dieline
        .get("template")
        .and_then(Value::as_str)
        .context("[invalid-input] dieline template is missing")?;
    ensure!(
        matches!(template, "straight-tuck" | "reverse-tuck" | "sleeve" | "mailer" | "pouch" | "tube" | "bottle-label" | "svg-import"),
        "[unsupported-capability] `{template}` is not a rectangular carton template supported by the fold solver yet"
    );
    if matches!(template, "tube" | "bottle-label") {
        return generate_tube(dieline);
    }
    let fold_state = dieline
        .get("fold_state")
        .map_or(Ok(1.0), |v| number(v, "dieline fold_state", 0.0, 1.0))?;
    let board_thickness = dieline["dimensions_mm"]["board"].as_f64().unwrap_or(0.0);
    ensure!(
        board_thickness.is_finite() && (0.0..=100.0).contains(&board_thickness),
        "[invalid-input] dieline board thickness must be within 0..=100 mm"
    );
    let panel_values = dieline
        .get("panels")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline panels must be an array")?;
    ensure!(
        !panel_values.is_empty(),
        "[invalid-input] a foldable dieline needs at least one panel"
    );
    let mut panels = Vec::with_capacity(panel_values.len());
    let mut slots = Vec::new();
    for value in panel_values {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline panel has no ID")?;
        let slot = value
            .get("face_slot")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline panel face_slot is missing")?;
        ensure!(
            !slot.trim().is_empty(),
            "[invalid-input] panel `{id}` face_slot cannot be empty"
        );
        if !slots.iter().any(|old: &String| old == slot) {
            slots.push(slot.to_owned());
        }
        let panel_rect = rect(
            value.get("rect_mm").unwrap_or(&Value::Null),
            "panel rect_mm",
            false,
        )?;
        let contour = panel_cut_contour(dieline, value)
            .map(|cut_path| flatten_panel_contour(cut_path["d"].as_str().unwrap_or_default(), id))
            .transpose()?;
        if let Some(points) = contour.as_ref() {
            let [x, y, width, height] = panel_rect;
            ensure!(
                points.iter().all(|point| {
                    point[0] >= x - 1e-8
                        && point[0] <= x + width + 1e-8
                        && point[1] >= y - 1e-8
                        && point[1] <= y + height + 1e-8
                }),
                "[invalid-input] cut contour for panel `{id}` has vertices outside its panel rectangle"
            );
        }
        let id = id.to_owned();
        panels.push(Panel {
            id,
            slot: slot.to_owned(),
            rect: panel_rect,
            uv: rect(
                value.get("uv_rect").unwrap_or(&Value::Null),
                "panel uv_rect",
                true,
            )?,
            contour,
        });
    }
    let crease_values = dieline
        .get("creases")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline creases must be an array")?;
    let mut creases = Vec::with_capacity(crease_values.len());
    for value in crease_values {
        let line = value
            .get("line_mm")
            .and_then(Value::as_array)
            .filter(|line| line.len() == 2)
            .context("[invalid-input] crease line_mm must be two points")?;
        let line = [
            pair(&line[0], "crease start")?,
            pair(&line[1], "crease end")?,
        ];
        ensure!(
            line[0] != line[1],
            "[invalid-input] crease line cannot have zero length"
        );
        let direction = match value.get("direction").and_then(Value::as_str) {
            Some("valley") => 1.0,
            Some("mountain") => -1.0,
            _ => bail!("[invalid-input] crease direction must be valley or mountain"),
        };
        creases.push((
            value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("crease")
                .to_owned(),
            line,
            number(
                value.get("angle_deg").unwrap_or(&Value::Null),
                "crease angle_deg",
                -180.0,
                180.0,
            )?,
            direction,
        ));
    }
    let crease_lines = creases
        .iter()
        .map(|(_, line, _, _)| *line)
        .collect::<Vec<_>>();
    let mut adjacency = vec![Vec::new(); panels.len()];
    let mut crease_used = vec![false; creases.len()];
    for a in 0..panels.len() {
        for b in a + 1..panels.len() {
            let Some(edge) = panel_hinged(panels[a].rect, panels[b].rect, &crease_lines) else {
                continue;
            };
            let mut matching_creases = creases
                .iter()
                .enumerate()
                .filter(|(_, (_, line, _, _))| same_edge(*line, edge));
            let (crease_index, (crease_id, line, angle, direction)) = matching_creases
                .next()
                .with_context(|| format!("[invalid-input] adjacent panels `{}` and `{}` need a crease along their shared edge", panels[a].id, panels[b].id))?;
            ensure!(
                matching_creases.next().is_none(),
                "[invalid-input] panel hinge between `{}` and `{}` matches multiple crease records; remove the duplicate crease or define a distinct panel edge",
                panels[a].id,
                panels[b].id
            );
            ensure!(
                !crease_used[crease_index],
                "[invalid-input] crease `{crease_id}` duplicates a hinge edge already assigned to another panel pair"
            );
            crease_used[crease_index] = true;
            adjacency[a].push(Hinge {
                to: b,
                line: *line,
                angle: *angle,
                direction: *direction,
            });
            adjacency[b].push(Hinge {
                to: a,
                line: *line,
                angle: *angle,
                direction: *direction,
            });
        }
    }
    for (index, panel) in panels.iter().enumerate() {
        if let Some(contour) = panel.contour.as_ref() {
            for hinge in &adjacency[index] {
                ensure!(
                    contour_covers_edge(contour, hinge.line),
                    "[invalid-input] cut contour for panel `{}` must preserve the complete shared fold edge with `{}`",
                    panel.id,
                    panels[hinge.to].id
                );
            }
        }
    }
    let transforms = build_transforms(&panels, &adjacency, fold_state, board_thickness)?;
    // Some older v11 dielines include creases on exposed panel boundaries that
    // do not join a second stored panel. Preserve those, but reject marks that
    // are geometrically unrelated to every panel rather than silently ignoring
    // typos or stale coordinates.
    for (index, used) in crease_used.iter().enumerate() {
        ensure!(
            *used
                || panels
                    .iter()
                    .any(|panel| panel_has_edge(panel.rect, creases[index].1)),
            "[invalid-input] crease `{}` does not match a panel edge",
            creases[index].0
        );
    }

    let mut generated_vertices = 0usize;
    let mut generated_triangles = 0usize;
    let mut generated_primitives = panels.len();
    let mut folded_hinges = 0usize;
    if board_thickness > 0.0 && fold_state > 0.0 {
        for (panel_index, hinges) in adjacency.iter().enumerate() {
            folded_hinges = folded_hinges
                .checked_add(
                    hinges
                        .iter()
                        .filter(|hinge| {
                            panel_index < hinge.to && hinge.angle.abs() * fold_state > 1e-8
                        })
                        .count(),
                )
                .context("[limit-exceeded] dieline hinge count overflow")?;
        }
    }
    for (index, panel) in panels.iter().enumerate() {
        let boundary = panel_boundary(panel);
        generated_vertices = generated_vertices
            .checked_add(boundary.len())
            .context("[limit-exceeded] dieline mesh vertex count overflow")?;
        generated_triangles = generated_triangles
            .checked_add(boundary.len().saturating_sub(2))
            .context("[limit-exceeded] dieline mesh triangle count overflow")?;
        if board_thickness > 0.0 {
            generated_vertices = generated_vertices
                .checked_add(boundary.len())
                .context("[limit-exceeded] dieline board vertex count overflow")?;
            generated_triangles = generated_triangles
                .checked_add(boundary.len().saturating_sub(2))
                .context("[limit-exceeded] dieline board triangle count overflow")?;
            let exposed_edges = (0..boundary.len())
                .filter(|edge_index| {
                    !panel_edge_is_hinge(
                        [
                            boundary[*edge_index],
                            boundary[(*edge_index + 1) % boundary.len()],
                        ],
                        &adjacency[index],
                    )
                })
                .count();
            generated_triangles = generated_triangles
                .checked_add(
                    exposed_edges
                        .checked_mul(2)
                        .context("[limit-exceeded] dieline board edge triangle count overflow")?,
                )
                .context("[limit-exceeded] dieline board triangle count overflow")?;
            generated_vertices = generated_vertices
                .checked_add(
                    exposed_edges
                        .checked_mul(4)
                        .context("[limit-exceeded] dieline board edge vertex count overflow")?,
                )
                .context("[limit-exceeded] dieline board vertex count overflow")?;
            generated_primitives = generated_primitives
                .checked_add(1 + usize::from(exposed_edges > 0))
                .context("[limit-exceeded] dieline board primitive count overflow")?;
        }
        ensure!(
            generated_vertices <= MAX_DIELINE_MESH_VERTICES,
            "[limit-exceeded] dieline fold mesh needs {generated_vertices} vertices; limit is {MAX_DIELINE_MESH_VERTICES}"
        );
    }
    generated_vertices = generated_vertices
        .checked_add(
            folded_hinges
                .checked_mul(4)
                .context("[limit-exceeded] dieline hinge bridge vertex count overflow")?,
        )
        .context("[limit-exceeded] dieline hinge bridge vertex count overflow")?;
    generated_triangles = generated_triangles
        .checked_add(
            folded_hinges
                .checked_mul(2)
                .context("[limit-exceeded] dieline hinge bridge triangle count overflow")?,
        )
        .context("[limit-exceeded] dieline hinge bridge triangle count overflow")?;
    generated_primitives = generated_primitives
        .checked_add(usize::from(folded_hinges > 0))
        .context("[limit-exceeded] dieline hinge bridge primitive count overflow")?;
    ensure!(
        generated_vertices <= MAX_DIELINE_MESH_VERTICES,
        "[limit-exceeded] dieline fold mesh needs {generated_vertices} vertices; limit is {MAX_DIELINE_MESH_VERTICES}"
    );
    ensure!(
        generated_triangles <= super::model::LIMITS.max_triangles,
        "[limit-exceeded] dieline fold mesh needs {generated_triangles} triangles; limit is {}",
        super::model::LIMITS.max_triangles
    );
    ensure!(
        generated_primitives <= super::model::LIMITS.max_primitives,
        "[limit-exceeded] dieline fold mesh needs {generated_primitives} primitives; limit is {}",
        super::model::LIMITS.max_primitives
    );

    let materials = slots
        .iter()
        .map(|name| Material {
            name: name.clone(),
            ..Material::default()
        })
        .collect::<Vec<_>>();
    let mut model = Model {
        materials,
        ..Model::default()
    };
    let board_inner_material = if board_thickness > 0.0 {
        let material = model.materials.len();
        model.materials.push(Material {
            name: "Pentool board interior".into(),
            double_sided: true,
            ..Material::default()
        });
        Some(material)
    } else {
        None
    };
    let board_edge_material = if board_thickness > 0.0 {
        let material = model.materials.len();
        model.materials.push(Material {
            name: "Pentool board cut edges".into(),
            ..Material::default()
        });
        Some(material)
    } else {
        None
    };
    for (index, panel) in panels.iter().enumerate() {
        let [x, y, w, h] = panel.rect;
        let [[u, v], [uw, vh]] = [[panel.uv[0], panel.uv[1]], [panel.uv[2], panel.uv[3]]];
        let (positions, uvs, indices, normal) = if let Some(contour) = panel.contour.as_ref() {
            let indices = triangulate_simple_polygon(contour, &panel.id)?;
            let positions = contour
                .iter()
                .map(|point| {
                    transform_flat(&transforms[index], *point, board_thickness / 2.0)
                        .map(|v| v as f32)
                })
                .collect::<Vec<_>>();
            let uvs = contour
                .iter()
                .map(|point| {
                    [
                        (u + (point[0] - x) / w * uw) as f32,
                        (v + (point[1] - y) / h * vh) as f32,
                    ]
                })
                .collect::<Vec<_>>();
            let triangle = [
                indices[0] as usize,
                indices[1] as usize,
                indices[2] as usize,
            ];
            let normal = math::normalize(math::cross(
                math::sub(
                    positions[triangle[1]].map(f64::from),
                    positions[triangle[0]].map(f64::from),
                ),
                math::sub(
                    positions[triangle[2]].map(f64::from),
                    positions[triangle[0]].map(f64::from),
                ),
            ));
            (positions, vec![uvs], indices, normal)
        } else {
            let flat = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
            let positions = flat
                .iter()
                .map(|point| {
                    transform_flat(&transforms[index], *point, board_thickness / 2.0)
                        .map(|v| v as f32)
                })
                .collect::<Vec<_>>();
            let normal = math::normalize(math::cross(
                math::sub(positions[3].map(f64::from), positions[0].map(f64::from)),
                math::sub(positions[1].map(f64::from), positions[0].map(f64::from)),
            ));
            let uvs = vec![[u, v], [u + uw, v], [u + uw, v + vh], [u, v + vh]]
                .into_iter()
                .map(|point| point.map(|value| value as f32))
                .collect();
            (positions, vec![uvs], vec![0, 2, 1, 0, 3, 2], normal)
        };
        let material = slots.iter().position(|name| name == &panel.slot).unwrap();
        model.primitives.push(Primitive {
            name: panel.id.clone(),
            positions,
            normals: vec![normal.map(|v| v as f32); panel.contour.as_ref().map_or(4, Vec::len)],
            uvs,
            indices,
            material: Some(material),
        });
        model.instances.push(Instance {
            id: panel.id.clone(),
            primitive: index,
            transform: IDENTITY,
        });
    }
    if let (Some(inner_material), Some(edge_material)) = (board_inner_material, board_edge_material)
    {
        let mut hinge_positions = Vec::new();
        let mut hinge_normals = Vec::new();
        let mut hinge_indices = Vec::new();
        for (panel_index, panel) in panels.iter().enumerate() {
            let outer = &model.primitives[panel_index];
            let outer_indices = outer.indices.clone();
            let outer_uvs = outer.uvs.clone();
            let panel_normal = outer.normals[0].map(f64::from);
            let boundary = panel_boundary(panel);
            let inner_positions = boundary
                .iter()
                .map(|point| {
                    transform_flat(&transforms[panel_index], *point, -board_thickness / 2.0)
                        .map(|value| value as f32)
                })
                .collect::<Vec<_>>();
            let mut inner_indices = Vec::with_capacity(outer_indices.len());
            for triangle in outer_indices.chunks_exact(3) {
                inner_indices.extend([triangle[0], triangle[2], triangle[1]]);
            }
            let inner_normal = math::normalize(math::cross(
                math::sub(
                    inner_positions[inner_indices[1] as usize].map(f64::from),
                    inner_positions[inner_indices[0] as usize].map(f64::from),
                ),
                math::sub(
                    inner_positions[inner_indices[2] as usize].map(f64::from),
                    inner_positions[inner_indices[0] as usize].map(f64::from),
                ),
            ));
            let inner_primitive = model.primitives.len();
            model.primitives.push(Primitive {
                name: format!("{}-interior", panel.id),
                positions: inner_positions.clone(),
                normals: vec![inner_normal.map(|value| value as f32); inner_positions.len()],
                uvs: outer_uvs,
                indices: inner_indices,
                material: Some(inner_material),
            });
            model.instances.push(Instance {
                id: format!("{}-interior", panel.id),
                primitive: inner_primitive,
                transform: IDENTITY,
            });

            let signed_area_twice = boundary
                .iter()
                .enumerate()
                .map(|(index, point)| {
                    let next = boundary[(index + 1) % boundary.len()];
                    point[0] * next[1] - next[0] * point[1]
                })
                .sum::<f64>();
            let orientation = signed_area_twice.signum();
            ensure!(
                orientation != 0.0,
                "[invalid-input] panel `{}` has a degenerate board boundary",
                panel.id
            );
            let mut edge_positions = Vec::new();
            let mut edge_normals = Vec::new();
            let mut edge_indices = Vec::new();
            for edge_index in 0..boundary.len() {
                let next = (edge_index + 1) % boundary.len();
                let edge = [boundary[edge_index], boundary[next]];
                if panel_edge_is_hinge(edge, &adjacency[panel_index]) {
                    continue;
                }
                let outer_a =
                    transform_flat(&transforms[panel_index], edge[0], board_thickness / 2.0);
                let outer_b =
                    transform_flat(&transforms[panel_index], edge[1], board_thickness / 2.0);
                let inner_b =
                    transform_flat(&transforms[panel_index], edge[1], -board_thickness / 2.0);
                let inner_a =
                    transform_flat(&transforms[panel_index], edge[0], -board_thickness / 2.0);
                let positions = [outer_a, outer_b, inner_b, inner_a]
                    .map(|point| point.map(|value| value as f32))
                    .to_vec();
                let mut indices = vec![0, 1, 2, 0, 2, 3];
                let normal = math::normalize(math::cross(
                    math::sub(positions[1].map(f64::from), positions[0].map(f64::from)),
                    math::sub(positions[2].map(f64::from), positions[0].map(f64::from)),
                ));
                let edge_direction = math::sub(outer_b, outer_a);
                let desired_normal =
                    math::cross(panel_normal, edge_direction).map(|value| value * orientation);
                if math::dot(normal, desired_normal) < 0.0 {
                    indices = vec![0, 2, 1, 0, 3, 2];
                }
                let offset = edge_positions.len() as u32;
                edge_positions.extend(positions);
                edge_normals.extend(vec![normal.map(|value| value as f32); 4]);
                edge_indices.extend(indices.into_iter().map(|index| index + offset));
            }
            if !edge_positions.is_empty() {
                let primitive = model.primitives.len();
                let name = format!("{}-cut-edges", panel.id);
                model.primitives.push(Primitive {
                    name: name.clone(),
                    positions: edge_positions,
                    normals: edge_normals,
                    uvs: Vec::new(),
                    indices: edge_indices,
                    material: Some(edge_material),
                });
                model.instances.push(Instance {
                    id: name,
                    primitive,
                    transform: IDENTITY,
                });
            }

            for hinge in adjacency[panel_index]
                .iter()
                .filter(|hinge| panel_index < hinge.to && hinge.angle.abs() * fold_state > 1e-8)
            {
                let parent_inner_start = transform_flat(
                    &transforms[panel_index],
                    hinge.line[0],
                    -board_thickness / 2.0,
                );
                let parent_inner_end = transform_flat(
                    &transforms[panel_index],
                    hinge.line[1],
                    -board_thickness / 2.0,
                );
                let child_inner_end =
                    transform_flat(&transforms[hinge.to], hinge.line[1], -board_thickness / 2.0);
                let child_inner_start =
                    transform_flat(&transforms[hinge.to], hinge.line[0], -board_thickness / 2.0);
                let positions = [
                    parent_inner_start,
                    parent_inner_end,
                    child_inner_end,
                    child_inner_start,
                ]
                .map(|point| point.map(|value| value as f32));
                let raw_normal = math::cross(
                    math::sub(positions[1].map(f64::from), positions[0].map(f64::from)),
                    math::sub(positions[2].map(f64::from), positions[0].map(f64::from)),
                );
                if math::length(raw_normal) < 1e-8 {
                    continue;
                }
                let normal = math::normalize(raw_normal);
                let offset = hinge_positions.len() as u32;
                hinge_positions.extend(positions);
                hinge_normals.extend(vec![normal.map(|value| value as f32); 4]);
                hinge_indices.extend([0, 1, 2, 0, 2, 3].map(|index| index + offset));
            }
        }
        if !hinge_positions.is_empty() {
            let primitive = model.primitives.len();
            model.primitives.push(Primitive {
                name: "scored-hinge-interiors".into(),
                positions: hinge_positions,
                normals: hinge_normals,
                uvs: Vec::new(),
                indices: hinge_indices,
                material: Some(inner_material),
            });
            model.instances.push(Instance {
                id: "scored-hinge-interiors".into(),
                primitive,
                transform: IDENTITY,
            });
        }
    }
    model.report.format = "pentool-dieline".into();
    model.report.vertices = model
        .primitives
        .iter()
        .map(|primitive| primitive.positions.len())
        .sum();
    model.report.triangles = model.primitives.iter().map(Primitive::triangle_count).sum();
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn editable_layout_page_changes_the_shared_dieline_geometry() {
        let mut dieline =
            tuck_carton_dieline("straight-tuck", 80.0, 50.0, 120.0, 0.5, 3.0, 15.0).unwrap();
        let (page_id, page) =
            make_editable_layout_page(&json!({"pages": []}), "carton", &dieline).unwrap();
        assert_eq!(page["dieline_layout"]["coordinates"], "mm");
        let generated_kinds: std::collections::BTreeSet<&str> = page["layers"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|layer| layer["nodes"].as_array().unwrap())
            .filter_map(|node| node["kind"].as_str())
            .collect();
        assert!(generated_kinds.contains("rect"));
        assert!(generated_kinds.contains("text"));
        assert!(generated_kinds.contains("path"));
        dieline["layout_page"] = json!(page_id);
        let panel_id = dieline["panels"][0]["id"].as_str().unwrap().to_owned();
        let before = dieline["panels"][0]["rect_mm"][2].as_f64().unwrap();
        let mut document = json!({"pages":[page],"dielines":{"carton":dieline}});
        let panel_node = document["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["dieline_panel_id"] == panel_id)
            .unwrap();
        panel_node["width"] = json!(panel_node["width"].as_f64().unwrap() + 4.0);

        let path_id = document["pages"][0]["layers"][1]["nodes"][0]["dieline_path_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let source_path = document["dielines"]["carton"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .find(|path| path["id"] == path_id)
            .unwrap()["d"]
            .clone();
        document["pages"][0]["layers"][1]["nodes"][0]["d"] = json!("M 10 10 L 40 10");

        let crease_id = document["pages"][0]["layers"][1]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["dieline_crease_id"].is_string())
            .unwrap()["dieline_crease_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let crease_node = document["pages"][0]["layers"][1]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["dieline_crease_id"] == crease_id)
            .unwrap();
        crease_node["d"] = json!("M 14 14 L 54 14");

        let resolved = dieline_from_document(&document, "carton").unwrap();
        assert_eq!(
            resolved["panels"][0]["rect_mm"][2].as_f64().unwrap(),
            before + 4.0
        );
        assert_ne!(
            resolved["paths"]
                .as_array()
                .unwrap()
                .iter()
                .find(|path| path["id"] == path_id)
                .unwrap()["d"],
            source_path
        );
        assert_eq!(
            resolved["creases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|crease| crease["id"] == crease_id)
                .unwrap()["line_mm"],
            json!([[11.0, 11.0], [51.0, 11.0]])
        );
    }

    #[test]
    fn editable_layout_canvas_contains_production_paths_and_structural_creases() {
        let mut dieline =
            tuck_carton_dieline("straight-tuck", 80.0, 50.0, 120.0, 0.5, 3.0, 15.0).unwrap();
        dieline["paths"].as_array_mut().unwrap().push(json!({
            "id":"extended-cut", "role":"cut", "d":"M 0 0 L 900 0"
        }));
        let furthest_fold_y = 960.0;
        dieline["creases"].as_array_mut().unwrap().push(json!({
            "id":"extended-fold", "line_mm":[[10.0, 10.0], [10.0, furthest_fold_y]],
            "direction":"valley", "angle_deg":90.0
        }));

        let (_, page) =
            make_editable_layout_page(&json!({"pages": []}), "carton", &dieline).unwrap();

        assert_eq!(page["canvas"]["width"], 906);
        assert_eq!(page["canvas"]["height"], 966);
        assert_eq!(page["production"]["trim"]["width"], 900.0);
        assert_eq!(page["production"]["trim"]["height"], furthest_fold_y);
    }

    #[test]
    fn every_builtin_packaging_template_generates_at_minimum_dimensions_and_fold_states() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            tuck_carton_dieline("reverse-tuck", 1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            sleeve_dieline(1.0, 1.0, 1.0, 0.0, 0.0).unwrap(),
            mailer_box_dieline(1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            pouch_dieline(1.01, 1.0, 1.01, 0.0).unwrap(),
            tube_label_dieline(1.0, 1.0, 0.0).unwrap(),
            bottle_label_dieline(1.0, 1.0, 0.0, 0.0).unwrap(),
        ];

        for source in templates {
            for fold in [0.0, 0.5, 1.0] {
                let mut dieline = source.clone();
                dieline["fold_state"] = json!(fold);
                let model = generate(&dieline).unwrap_or_else(|error| {
                    panic!("minimum {} at fold {fold}: {error}", source["template"])
                });
                assert!(
                    !model.primitives.is_empty(),
                    "minimum {} at fold {fold} produced no package faces",
                    source["template"]
                );
                assert!(
                    model
                        .primitives
                        .iter()
                        .flat_map(|primitive| primitive.positions.iter().flatten())
                        .all(|coordinate| coordinate.is_finite()),
                    "minimum {} at fold {fold} produced non-finite geometry",
                    source["template"]
                );
                assert!(model.report.triangles > 0);
            }
        }
    }

    #[test]
    fn minimum_rectangular_nets_preserve_panel_dimensions_and_hinges() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            tuck_carton_dieline("reverse-tuck", 1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            sleeve_dieline(1.0, 1.0, 1.0, 0.0, 0.0).unwrap(),
            mailer_box_dieline(1.0, 1.0, 1.0, 0.0, 0.0, 0.01).unwrap(),
            pouch_dieline(1.01, 1.0, 1.01, 0.0).unwrap(),
        ];
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };

        for source in templates {
            let panel_values = source["panels"].as_array().unwrap();
            let rects = panel_values
                .iter()
                .map(|panel| rect(&panel["rect_mm"], "minimum test panel", false).unwrap())
                .collect::<Vec<_>>();
            for fold in [0.0, 0.5, 1.0] {
                let mut dieline = source.clone();
                dieline["fold_state"] = json!(fold);
                let model = generate(&dieline).unwrap_or_else(|error| {
                    panic!("minimum {} at fold {fold}: {error}", source["template"])
                });
                for (index, (panel, primitive)) in
                    panel_values.iter().zip(&model.primitives).enumerate()
                {
                    let [_, _, width, height] = rects[index];
                    let positions = &primitive.positions;
                    assert!(
                        (distance(positions[0], positions[1]) - width as f32).abs() < 1e-3,
                        "minimum {} panel `{}` width changed at fold {fold}",
                        source["template"],
                        panel["id"]
                    );
                    assert!(
                        (distance(positions[0], positions[3]) - height as f32).abs() < 1e-3,
                        "minimum {} panel `{}` height changed at fold {fold}",
                        source["template"],
                        panel["id"]
                    );
                }

                for first in 0..rects.len() {
                    for second in first + 1..rects.len() {
                        let Some(edge) = panel_adjacent(rects[first], rects[second]) else {
                            continue;
                        };
                        let vertex = |index: usize, target: [f64; 2]| {
                            let [x, y, width, height] = rects[index];
                            [
                                [x, y],
                                [x + width, y],
                                [x + width, y + height],
                                [x, y + height],
                            ]
                            .iter()
                            .position(|point| {
                                (point[0] - target[0]).abs() < 1e-8
                                    && (point[1] - target[1]).abs() < 1e-8
                            })
                            .unwrap_or_else(|| panic!("minimum panel shared corner is missing"))
                        };
                        let a = &model.primitives[first].positions;
                        let b = &model.primitives[second].positions;
                        let a0 = vertex(first, edge[0]);
                        let a1 = vertex(first, edge[1]);
                        let b0 = vertex(second, edge[0]);
                        let b1 = vertex(second, edge[1]);
                        let direct = distance(a[a0], b[b0]) + distance(a[a1], b[b1]);
                        let reverse = distance(a[a0], b[b1]) + distance(a[a1], b[b0]);
                        assert!(
                            direct.min(reverse) < 1e-3,
                            "minimum {} panels `{}` and `{}` separate at fold {fold}: {direct}/{reverse}",
                            source["template"],
                            panel_values[first]["id"],
                            panel_values[second]["id"]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn flat_builtin_panel_nets_report_no_face_collisions() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            tuck_carton_dieline("reverse-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap(),
            mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap(),
            pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap(),
        ];

        for mut dieline in templates {
            dieline["fold_state"] = json!(0.0);
            let model = generate(&dieline).unwrap();
            let report = folded_collision_report(&dieline, &model).unwrap();
            let panel_count = dieline["panels"].as_array().unwrap().len();
            assert_eq!(
                report["pairs_tested"],
                panel_count * panel_count.saturating_sub(1) / 2,
                "{}",
                dieline["template"]
            );
            assert_eq!(report["status"], "checked-clear", "{}", dieline["template"]);
            assert_eq!(report["fold_state"], 0.0);
            assert_eq!(report["collision_count"], 0);
            assert_eq!(
                report["physical_thickness_tested"],
                dieline["dimensions_mm"]["board"].as_f64().unwrap_or(0.0) > 0.0
            );
        }
    }

    #[test]
    fn folded_collision_reports_are_stable_across_builtin_preview_states() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            tuck_carton_dieline("reverse-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap(),
            mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap(),
            pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap(),
        ];

        for source in templates {
            let panel_count = source["panels"].as_array().unwrap().len();
            let expected_pairs = panel_count * panel_count.saturating_sub(1) / 2;
            for fold in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut dieline = source.clone();
                dieline["fold_state"] = json!(fold);
                let model = generate(&dieline).unwrap();
                let report = folded_collision_report(&dieline, &model).unwrap();
                assert_eq!(report["fold_state"], fold);
                assert_eq!(report["pairs_tested"], expected_pairs);
                let collision_count = report["collision_count"].as_u64().unwrap() as usize;
                assert_eq!(
                    report["status"],
                    if collision_count == 0 {
                        "checked-clear"
                    } else {
                        "potential-overlaps"
                    },
                    "{} at fold {fold}",
                    source["template"]
                );
                let collisions = report["collisions"].as_array().unwrap();
                assert!(collisions.len() <= MAX_REPORTED_FOLD_COLLISIONS);
                assert_eq!(
                    report["collisions_omitted"],
                    collision_count.saturating_sub(collisions.len())
                );
                assert_eq!(
                    report,
                    folded_collision_report(&dieline, &model).unwrap(),
                    "collision report changed across identical runs for {} at fold {fold}",
                    source["template"]
                );
            }
        }
    }

    #[test]
    fn folded_tuck_cartons_classify_contacts_and_keep_layer_overlap_warnings() {
        for template in ["straight-tuck", "reverse-tuck"] {
            let dieline =
                tuck_carton_dieline(template, 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap();
            let model = generate(&dieline).unwrap();
            let report = folded_collision_report(&dieline, &model).unwrap();
            assert_eq!(report["collision_count"], 0, "{template}: {report}");
            for collision in report["collisions"].as_array().unwrap() {
                let panels = collision["panels"].as_array().unwrap();
                assert!(panels
                    .iter()
                    .any(|panel| panel.as_str().unwrap().contains("dust")));
                assert!(panels
                    .iter()
                    .any(|panel| panel.as_str().unwrap().contains("lid")));
                assert_eq!(collision["kind"], "zero-thickness-intersection");
            }
        }

        let first = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let opposite = [first[0], first[2], first[1]];
        let same_facing = first;
        assert!(!triangles_have_interior_intersection(first, opposite));
        assert!(triangles_have_interior_intersection(first, same_facing));
    }

    #[test]
    fn packaging_preflight_reports_hinge_and_wrap_seam_measurements() {
        let mut raw: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/v11-product-packaging.pen"
        ))
        .unwrap();
        add_pouch(
            &mut raw,
            "preflight-pouch",
            "preflight-pouch-preview",
            "mockups",
            Some("products"),
            120.0,
            35.0,
            180.0,
            3.0,
            [0.0, 0.0, 200.0, 240.0],
        )
        .unwrap();
        let pouch = preflight(&raw, Path::new("."), "preflight-pouch", 300.0).unwrap();
        assert_eq!(pouch["fold_hinge_checks"].as_array().unwrap().len(), 4);
        assert!(pouch["fold_hinge_checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["joined"] == true
                && check["gap_mm"].as_f64().unwrap() <= check["tolerance_mm"].as_f64().unwrap()));
        assert_eq!(pouch["fold_collision_report"]["status"], "checked-clear");
        assert_eq!(
            pouch["fold_collision_report"]["physical_thickness_tested"],
            false
        );
        assert!(pouch["production_blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker.as_str().unwrap().contains("score compression")));

        add_bottle_label(
            &mut raw,
            "preflight-bottle-label",
            "preflight-bottle-label-preview",
            "mockups",
            Some("products"),
            30.0,
            90.0,
            3.0,
            2.0,
            [0.0, 0.0, 200.0, 240.0],
        )
        .unwrap();
        let bottle = preflight(&raw, Path::new("."), "preflight-bottle-label", 300.0).unwrap();
        assert_eq!(bottle["fold_hinge_checks"].as_array().unwrap().len(), 1);
        assert_eq!(bottle["fold_hinge_checks"][0]["kind"], "wrap-seam");
        assert_eq!(bottle["fold_hinge_checks"][0]["joined"], true);
        assert_eq!(bottle["fold_collision_report"]["status"], "checked-clear");
    }

    #[test]
    fn folded_collision_scan_finds_interior_crossings_but_ignores_edge_contact() {
        let horizontal = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let crossing = [
            [0.5, -0.5, -1.0],
            [0.5, 1.5, -1.0],
            [0.5, 1.5, 1.0],
            [0.5, -0.5, 1.0],
        ];
        let edge_touch = [
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        assert!(panel_quads_intersect(horizontal, crossing));
        assert!(!panel_quads_intersect(horizontal, edge_touch));
    }

    #[test]
    fn folded_collision_scan_checks_board_thickness_clearance() {
        let first = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ];
        let near_parallel = first.map(|[x, y, z]| [x, y, z + 0.75]);
        let one_board_thickness = first.map(|[x, y, z]| [x, y, z + 1.0]);
        let slightly_tilted = first.map(|[x, y, z]| [x, y, z + 0.25 + x * 0.02]);
        let far_parallel = first.map(|[x, y, z]| [x, y, z + 1.25]);
        let edge_touch = first.map(|[x, y, z]| [x + 10.0, y, z + 0.25]);
        assert!(panels_overlap_with_thickness(first, near_parallel, 1.0));
        assert!(!panels_overlap_with_thickness(
            first,
            one_board_thickness,
            1.0
        ));
        assert!(panels_overlap_with_thickness(first, slightly_tilted, 1.0));
        assert!(!panels_overlap_with_thickness(first, far_parallel, 1.0));
        assert!(!panels_overlap_with_thickness(first, edge_touch, 1.0));
        assert!(!panels_overlap_with_thickness(first, near_parallel, 0.0));
    }

    #[test]
    fn folded_collision_report_identifies_board_thickness_conflicts() {
        let panel = |name: &str, positions: [[f32; 3]; 4]| Primitive {
            name: name.to_owned(),
            positions: positions.into(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2, 0, 2, 3],
            material: None,
        };
        let base = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ];
        let parallel = base.map(|[x, y, z]| [x, y, z + 0.75]);
        let dieline = json!({
            "template":"synthetic-clearance", "fold_state":1.0,
            "dimensions_mm":{"board":1.0},
            "panels":[
                {"id":"near","rect_mm":[0.0,0.0,10.0,10.0]},
                {"id":"far","rect_mm":[20.0,0.0,10.0,10.0]}
            ]
        });
        let model = Model {
            primitives: vec![panel("near", base), panel("far", parallel)],
            ..Model::default()
        };
        let report = folded_collision_report(&dieline, &model).unwrap();
        assert_eq!(report["physical_thickness_tested"], true);
        assert_eq!(report["board_thickness_mm"], 1.0);
        assert_eq!(report["collision_count"], 1);
        assert_eq!(
            report["collisions"][0]["kind"],
            "board-thickness-intersection"
        );

        let contact = base.map(|[x, y, z]| [x, y, z + 1.0]);
        let contact_model = Model {
            primitives: vec![panel("near", base), panel("contact", contact)],
            ..Model::default()
        };
        let contact_report = folded_collision_report(&dieline, &contact_model).unwrap();
        assert_eq!(contact_report["collision_count"], 0);
    }

    #[test]
    fn folded_collision_scan_does_not_skip_hinged_panel_pairs() {
        let panel = |name: &str, positions: [[f32; 3]; 4]| Primitive {
            name: name.to_owned(),
            positions: positions.into(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2, 0, 2, 3],
            material: None,
        };
        let dieline = json!({
            "template":"synthetic-hinged-net", "fold_state":0.5,
            "panels":[
                {"id":"left","rect_mm":[0.0,0.0,1.0,1.0]},
                {"id":"right","rect_mm":[1.0,0.0,1.0,1.0]}
            ]
        });
        let model = Model {
            primitives: vec![
                panel(
                    "left",
                    [
                        [0.0, 0.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [0.0, 1.0, 0.0],
                    ],
                ),
                panel(
                    "right",
                    [
                        [0.5, -0.5, -1.0],
                        [0.5, 1.5, -1.0],
                        [0.5, 1.5, 1.0],
                        [0.5, -0.5, 1.0],
                    ],
                ),
            ],
            ..Model::default()
        };

        let report = folded_collision_report(&dieline, &model).unwrap();
        assert_eq!(report["pairs_tested"], 1);
        assert_eq!(report["collision_count"], 1);
        assert_eq!(report["collisions"][0]["panels"], json!(["left", "right"]));
    }

    #[test]
    fn flat_svg_keeps_panel_ids_and_production_mark_roles() {
        let dieline =
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap();
        let svg = flat_dieline_svg(&dieline).unwrap();
        assert!(svg.contains("width=\"401mm\""));
        assert!(svg.contains("viewBox=\"0 0 401 "));
        assert!(svg.contains("data-role=\"bleed-preview\""));
        assert!(svg.contains("data-panel=\"front\""));
        assert!(svg.contains("data-role=\"cut\""));
        assert!(svg.contains("data-spot=\"CutContour\""));
        assert!(svg.contains("data-role=\"crease\""));
        assert!(svg.contains("data-id=\"top-lid-fold\""));
        assert!(!svg.contains("data-id=\"carton-crease-lines\""));
        assert!(svg.contains("data-role=\"glue-flap\""));
        assert!(svg.contains("Panel artwork is not included"));

        let mut hostile = dieline;
        hostile["panels"][0]["id"] = json!("front\"><script>");
        let svg = flat_dieline_svg(&hostile).unwrap();
        assert!(svg.contains("data-panel=\"front&quot;&gt;&lt;script&gt;\""));
        assert!(!svg.contains("data-panel=\"front\"><script>"));

        let mut malformed =
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap();
        malformed["paths"][0]["d"] = json!("NOT A PATH");
        assert!(flat_dieline_svg(&malformed).is_err());
    }

    #[test]
    fn synthesized_bleed_extends_only_cut_edges_and_preserves_panel_art() {
        let fragment = "<svg x=\"2\" y=\"2\" width=\"4\" height=\"4\" viewBox=\"0 0 4 4\"><rect width=\"4\" height=\"4\" fill=\"#dc1e32\"/></svg>";
        let cut_edges = [true, false, true, true];
        let (extended, _, _, _) = synthesize_bleed_pixels(
            fragment,
            PanelRectMm {
                x: 2.0,
                y: 2.0,
                width: 4.0,
                height: 4.0,
            },
            1.0,
            25.4,
            cut_edges,
        )
        .unwrap();
        assert_eq!(*extended.get_pixel(0, 2), image::Rgba([220, 30, 50, 255]));
        assert_eq!(*extended.get_pixel(5, 2), image::Rgba([0, 0, 0, 0]));
        assert_eq!(*extended.get_pixel(2, 0), image::Rgba([220, 30, 50, 255]));
        assert_eq!(*extended.get_pixel(2, 5), image::Rgba([220, 30, 50, 255]));
        assert_eq!(*extended.get_pixel(2, 2), image::Rgba([0, 0, 0, 0]));

        let regions = bleed_clip_regions(2.0, 2.0, 4.0, 4.0, 1.0, cut_edges);
        let encoded = base64::engine::general_purpose::STANDARD.encode({
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(extended)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            png.into_inner()
        });
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\" viewBox=\"0 0 8 8\"><defs><clipPath id=\"bleed\" clipPathUnits=\"userSpaceOnUse\">{regions}</clipPath></defs><image x=\"1\" y=\"1\" width=\"6\" height=\"6\" preserveAspectRatio=\"none\" href=\"data:image/png;base64,{encoded}\" clip-path=\"url(#bleed)\"/><rect x=\"2\" y=\"2\" width=\"4\" height=\"4\" fill=\"#2040d0\"/></svg>"
        );
        let png = crate::render::svg_to_png(&svg, 8, 8, 1.0).unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(*pixels.get_pixel(1, 3), image::Rgba([220, 30, 50, 255]));
        assert_eq!(*pixels.get_pixel(6, 3), image::Rgba([0, 0, 0, 0]));
        assert_eq!(*pixels.get_pixel(3, 1), image::Rgba([220, 30, 50, 255]));
        assert_eq!(*pixels.get_pixel(3, 3), image::Rgba([32, 64, 208, 255]));
    }

    #[test]
    fn contour_bleed_follows_a_non_rectangular_cut_path() {
        let fragment = "<svg x=\"0\" y=\"0\" width=\"4\" height=\"4\" viewBox=\"0 0 4 4\"><rect width=\"4\" height=\"4\" fill=\"#dc1e32\"/></svg>";
        let contour = "M 2 0 L 4 2 L 4 4 L 0 4 L 0 2 Z";
        let (extended, _, _, _) = synthesize_contour_bleed(
            fragment,
            PanelRectMm {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
            },
            1.0,
            25.4,
            contour,
        )
        .unwrap();
        assert_eq!(extended.dimensions(), (6, 6));
        assert_eq!(extended.get_pixel(0, 3)[3], 255, "cut edge receives bleed");
        assert_eq!(extended.get_pixel(0, 0)[3], 0, "distant corner stays clear");
        assert_eq!(
            extended.get_pixel(2, 3)[3],
            0,
            "cut interior is not duplicated as bleed"
        );
    }

    #[test]
    fn panel_artwork_clip_uses_the_associated_non_rectangular_contour() {
        let contour = "M 2 0 L 4 2 L 4 4 L 0 4 L 0 2 Z";
        let fragment = format!(
            "<svg x=\"0\" y=\"0\" width=\"4\" height=\"4\" viewBox=\"0 0 4 4\"><defs><clipPath id=\"cut\"><path d=\"{contour}\" fill-rule=\"evenodd\"/></clipPath></defs><g clip-path=\"url(#cut)\"><rect width=\"4\" height=\"4\" fill=\"#dc1e32\"/></g></svg>"
        );
        let image = render_fragment_in_bounds(&fragment, [0.0, 0.0, 4.0, 4.0], [40, 40]).unwrap();
        assert_eq!(image.get_pixel(20, 20)[3], 255, "panel interior keeps art");
        assert_eq!(
            image.get_pixel(1, 1)[3],
            0,
            "outside the cut contour is clipped"
        );
    }

    #[test]
    fn shared_fold_edges_and_tube_seams_do_not_receive_bleed() {
        let net = two_panel_net(1.0);
        assert_eq!(
            panel_cut_edges(&net, &net["panels"][0]).unwrap(),
            [true, false, true, true]
        );
        assert_eq!(
            panel_cut_edges(&net, &net["panels"][1]).unwrap(),
            [false, true, true, true]
        );
        let mut tube = net;
        tube["template"] = json!("tube");
        assert_eq!(
            panel_cut_edges(&tube, &tube["panels"][0]).unwrap(),
            [false, false, true, true]
        );
    }

    fn two_panel_net(fold: f64) -> Value {
        json!({
            "template":"mailer", "units":"mm", "fold_state":fold,
            "dimensions_mm":{"length":10,"width":10,"height":10,"board":0.4,"bleed":2},
            "panels":[
                {"id":"base","face_slot":"base","rect_mm":[0,0,10,10],"uv_rect":[0,0,1,1]},
                {"id":"side","face_slot":"side","rect_mm":[10,0,10,10],"uv_rect":[0,0,1,1]}
            ],
            "creases":[{"id":"right-fold","line_mm":[[10,0],[10,10]],"angle_deg":90,"direction":"valley"}],
            "paths":[]
        })
    }

    #[test]
    fn rectangular_panel_net_folds_from_flat_to_its_crease_angle() {
        let flat = generate(&two_panel_net(0.0)).unwrap();
        let folded = generate(&two_panel_net(1.0)).unwrap();
        assert_eq!(flat.report.triangles, 20);
        assert!(flat.primitives[..2]
            .iter()
            .flat_map(|p| &p.positions)
            .all(|p| (f64::from(p[2]) - 0.2).abs() < 1e-5));
        assert!(folded.primitives[..2]
            .iter()
            .flat_map(|p| &p.positions)
            .any(|p| p[2].abs() > 9.0));
        assert_eq!(folded.primitives[1].uvs[0], flat.primitives[1].uvs[0]);
        assert_eq!(folded.primitives.len(), 7);
        assert_eq!(folded.report.vertices, 44);
        assert_eq!(folded.report.triangles, 22);
        assert_eq!(folded.primitives[2].name, "base-interior");
        assert!(folded.primitives[2]
            .positions
            .iter()
            .all(|point| (f64::from(point[2]) + 0.2).abs() < 1e-5));
        assert_eq!(folded.materials[2].name, "Pentool board interior");
        assert_eq!(folded.materials[3].name, "Pentool board cut edges");
        let hinge_fill = folded
            .primitives
            .iter()
            .find(|primitive| primitive.name == "scored-hinge-interiors")
            .unwrap();
        assert_eq!(hinge_fill.positions.len(), 4);
        assert_eq!(hinge_fill.indices.len(), 6);
        assert!(hinge_fill
            .normals
            .iter()
            .flatten()
            .all(|value| value.is_finite()));
        assert!(folded.primitives[4..]
            .iter()
            .filter(|primitive| primitive.name.contains("cut-edge"))
            .all(|primitive| primitive.uvs.is_empty() && primitive.indices.len() % 6 == 0));
    }

    #[test]
    fn board_thickness_bridges_scored_hinges_after_they_fold() {
        for fold_state in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let model = generate(&two_panel_net(fold_state)).unwrap();
            let bridge = model
                .primitives
                .iter()
                .find(|primitive| primitive.name == "scored-hinge-interiors");
            if fold_state == 0.0 {
                assert!(bridge.is_none(), "a flat hinge needs no wedge fill");
                continue;
            }
            let bridge = bridge.expect("folded board interiors should meet at the hinge");
            assert_eq!(bridge.positions.len(), 4);
            let area = bridge
                .indices
                .chunks_exact(3)
                .map(|triangle| {
                    let a = bridge.positions[triangle[0] as usize].map(f64::from);
                    let b = bridge.positions[triangle[1] as usize].map(f64::from);
                    let c = bridge.positions[triangle[2] as usize].map(f64::from);
                    math::length(math::cross(math::sub(b, a), math::sub(c, a))) / 2.0
                })
                .sum::<f64>();
            assert!(
                area > 1e-6,
                "fold={fold_state} produced a degenerate hinge fill"
            );
        }
    }

    #[test]
    fn board_thickness_offsets_fold_axes_and_preserves_surface_hinge_continuity() {
        for fold_state in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let compensated = two_panel_net(fold_state);
            let model = generate(&compensated).unwrap();
            let checks = folded_hinge_checks(&compensated, &model).unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0]["joined"], true, "fold={fold_state}");
            assert!(checks[0]["gap_mm"].as_f64().unwrap() <= 1e-4);
        }

        let compensated = generate(&two_panel_net(1.0)).unwrap();
        let mut zero_thickness = two_panel_net(1.0);
        zero_thickness["dimensions_mm"]["board"] = json!(0.0);
        let unoffset = generate(&zero_thickness).unwrap();
        assert!(compensated
            .primitives
            .iter()
            .flat_map(|primitive| &primitive.positions)
            .zip(
                unoffset
                    .primitives
                    .iter()
                    .flat_map(|primitive| &primitive.positions)
            )
            .any(|(with_board, without_board)| {
                (f64::from(with_board[0]) - f64::from(without_board[0])).abs() > 1e-4
                    || (f64::from(with_board[1]) - f64::from(without_board[1])).abs() > 1e-4
                    || (f64::from(with_board[2]) - f64::from(without_board[2])).abs() > 1e-4
            }));
    }

    #[test]
    fn stand_up_pouch_net_generates_connected_folded_panel_mesh() {
        let dieline = pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap();
        assert_eq!(dieline["panels"].as_array().unwrap().len(), 5);
        assert_eq!(dieline["creases"].as_array().unwrap().len(), 4);
        assert_eq!(dieline["template"], "pouch");
        let model = generate(&dieline).unwrap();
        assert_eq!(model.primitives.len(), 5);
        assert!(model
            .primitives
            .iter()
            .all(|primitive| primitive.indices.len() >= 6));
        assert!(pouch_dieline(20.0, 20.0, 80.0, 3.0).is_err());
    }

    #[test]
    fn straight_tuck_carton_generates_a_foldable_body_and_closure_flaps() {
        let dieline =
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap();
        let cut_path =
            kurbo::BezPath::from_svg(dieline["paths"][0]["d"].as_str().unwrap()).unwrap();
        assert!(cut_path
            .elements()
            .iter()
            .any(|element| matches!(element, kurbo::PathEl::ClosePath)));
        let model = generate(&dieline).unwrap();
        assert_eq!(model.primitives.len(), 40);
        assert_eq!(model.report.triangles, 140);
        let body_panels: Vec<_> = model
            .primitives
            .iter()
            .filter(|primitive| {
                matches!(primitive.name.as_str(), "front" | "right" | "back" | "left")
            })
            .collect();
        let mut body_min = [f64::INFINITY; 3];
        let mut body_max = [f64::NEG_INFINITY; 3];
        for primitive in &body_panels {
            for point in &primitive.positions {
                for axis in 0..3 {
                    body_min[axis] = body_min[axis].min(f64::from(point[axis]));
                    body_max[axis] = body_max[axis].max(f64::from(point[axis]));
                }
            }
        }
        let body_center =
            std::array::from_fn::<_, 3, _>(|axis| (body_min[axis] + body_max[axis]) * 0.5);
        for primitive in &body_panels {
            let panel_center = std::array::from_fn::<_, 3, _>(|axis| {
                primitive
                    .positions
                    .iter()
                    .map(|point| f64::from(point[axis]))
                    .sum::<f64>()
                    / primitive.positions.len() as f64
            });
            let normal = primitive.normals[0].map(f64::from);
            let outward = math::dot(math::sub(panel_center, body_center), normal);
            assert!(
                outward > 0.0,
                "folded body panel {} points inward (outward score {outward})",
                primitive.name
            );
        }
        for primitive in model.primitives.iter().filter(|primitive| {
            dieline["panels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|panel| panel["id"] == primitive.name)
        }) {
            for triangle in primitive.indices.chunks_exact(3) {
                let point = |index: u32| primitive.positions[index as usize].map(f64::from);
                let cross = math::cross(
                    math::sub(point(triangle[1]), point(triangle[0])),
                    math::sub(point(triangle[2]), point(triangle[0])),
                );
                let normal = primitive.normals[triangle[0] as usize].map(f64::from);
                assert!(
                    math::dot(cross, normal) > 0.0,
                    "panel {} triangle winding opposes its stored normal",
                    primitive.name
                );
            }
        }
        assert!(model
            .primitives
            .iter()
            .flat_map(|p| &p.positions)
            .all(|position| position.iter().all(|value| value.is_finite())));
        let flat = {
            let mut flat = dieline.clone();
            flat["fold_state"] = serde_json::json!(0.0);
            generate(&flat).unwrap()
        };
        assert!(
            model
                .primitives
                .iter()
                .zip(&flat.primitives)
                .any(|(folded, flat)| folded.positions != flat.positions),
            "carton fold must move its connected panels"
        );
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let front = &model.primitives[0].positions;
        let left = &model.primitives[3].positions;
        let seam_direct = distance(front[0], left[1]) + distance(front[3], left[2]);
        let seam_reversed = distance(front[0], left[2]) + distance(front[3], left[1]);
        assert!(
            seam_direct.min(seam_reversed) < 1e-3,
            "body closure seam gap: {seam_direct}/{seam_reversed}"
        );
        let top = model
            .primitives
            .iter()
            .find(|p| p.name == "top-lid")
            .unwrap();
        let top_y = front[0][1];
        assert!(
            top.positions.iter().all(|p| (p[1] - top_y).abs() < 1e-3),
            "top lid does not fold into a horizontal cap: {:?}",
            top.positions
        );
        let (top_min_x, top_max_x) = top
            .positions
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), p| {
                (min.min(p[0]), max.max(p[0]))
            });
        let (top_min_z, top_max_z) = top
            .positions
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), p| {
                (min.min(p[2]), max.max(p[2]))
            });
        assert!(
            (top_min_x).abs() < 1e-3 && (top_max_x - 120.0).abs() < 1e-3,
            "top lid x span is wrong: {top_min_x}..{top_max_x}"
        );
        let (body_min_z, body_max_z) = model
            .primitives
            .iter()
            .filter(|primitive| {
                matches!(primitive.name.as_str(), "front" | "right" | "back" | "left")
            })
            .flat_map(|primitive| &primitive.positions)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), point| {
                (min.min(point[2]), max.max(point[2]))
            });
        assert!(
            (top_min_z - body_min_z).abs() < 1e-3
                && (top_max_z - body_max_z).abs() < 1e-3
                && ((top_max_z - top_min_z) - 70.0).abs() < 1e-3,
            "top lid must meet and cover the board-compensated box depth: {top_min_z}..{top_max_z} vs {body_min_z}..{body_max_z}"
        );
        let bottom = model
            .primitives
            .iter()
            .find(|p| p.name == "bottom-lid")
            .unwrap();
        let bottom_y = front[2][1];
        assert!(
            bottom
                .positions
                .iter()
                .all(|p| (p[1] - bottom_y).abs() < 1e-3),
            "bottom lid does not fold into a horizontal cap: {:?}",
            bottom.positions
        );
        // Dust flaps fold slightly past square so they layer under the lid
        // instead of sharing its plane: the hinge stays on the closure plane
        // and the free edge sits more than one board thickness below it.
        for (id, closure_y, inward) in [
            ("right-dust-top", top_y, -1.0_f32),
            ("left-dust-top", top_y, -1.0),
            ("right-dust-bottom", bottom_y, 1.0),
            ("left-dust-bottom", bottom_y, 1.0),
        ] {
            let dust = model.primitives.iter().find(|p| p.name == id).unwrap();
            let drops: Vec<f32> = dust
                .positions
                .iter()
                .map(|p| (p[1] - closure_y) * inward)
                .collect();
            assert!(
                drops.iter().all(|d| *d > -1e-3),
                "{id} rises above its closure plane: {:?}",
                dust.positions
            );
            let deepest = drops.iter().cloned().fold(0.0_f32, f32::max);
            assert!(
                deepest > 1.0 && deepest < 3.0,
                "{id} free edge is not layered below the lid: {deepest}"
            );
        }
        for id in ["top-tuck", "bottom-tuck"] {
            let flap = model.primitives.iter().find(|p| p.name == id).unwrap();
            let min_z = flap
                .positions
                .iter()
                .map(|p| p[2])
                .fold(f32::INFINITY, f32::min);
            let max_z = flap
                .positions
                .iter()
                .map(|p| p[2])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (max_z - min_z).abs() < 1e-3,
                "{id} should fold against a carton wall: {min_z}..{max_z}"
            );
        }
        let glue = model
            .primitives
            .iter()
            .find(|p| p.name == "glue-joint")
            .unwrap();
        let spans = (0..3)
            .map(|axis| {
                let min = glue
                    .positions
                    .iter()
                    .map(|p| p[axis])
                    .fold(f32::INFINITY, f32::min);
                let max = glue
                    .positions
                    .iter()
                    .map(|p| p[axis])
                    .fold(f32::NEG_INFINITY, f32::max);
                max - min
            })
            .collect::<Vec<_>>();
        assert!(
            spans.iter().copied().min_by(f32::total_cmp).unwrap() < 1e-3,
            "glue joint must remain planar: {spans:?}"
        );
    }

    #[test]
    fn carton_exterior_panel_overlay_is_visible_in_cpu_render() {
        let dieline =
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap();
        let mut model = generate(&dieline).unwrap();
        let base_model = model.clone();
        model.textures.push(super::super::model::Texture {
            name: "test-red-panel-art".into(),
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        });
        let overlay_material = model.materials.len();
        model.materials.push(Material {
            name: "front-test-overlay".into(),
            base_color: [1.0; 4],
            base_color_texture: Some(super::super::model::TexRef {
                texture: 0,
                uv_set: 0,
                wrap: super::super::model::WrapMode::Clamp,
                projection: super::super::model::TextureProjection::Uv,
            }),
            alpha_mode: super::super::model::AlphaMode::Blend,
            ..Material::default()
        });
        let mut overlay = model.primitives[0].clone();
        overlay.name = "front-test-overlay".into();
        overlay.material = Some(overlay_material);
        for (position, normal) in overlay.positions.iter_mut().zip(&overlay.normals) {
            for axis in 0..3 {
                position[axis] += normal[axis] * 0.25;
            }
        }
        let overlay_index = model.primitives.len();
        model.primitives.push(overlay);
        model.instances.push(Instance {
            id: "front-test-overlay".into(),
            primitive: overlay_index,
            transform: IDENTITY,
        });

        let mut changed_pixels = 0usize;
        for azimuth in [0.0, 90.0, 180.0, 270.0] {
            let node = serde_json::json!({
                "kind":"scene3d", "id":"carton-overlay-test", "width":128, "height":128,
                "camera":{"azimuth":azimuth,"elevation":15.0}
            });
            let (scene, options) =
                super::super::node::scene_for(node.as_object().unwrap(), Some(&model)).unwrap();
            let rendered = super::super::render::render(&model, &scene, &options).unwrap();
            let base = super::super::render::render(&base_model, &scene, &options).unwrap();
            let changed = rendered
                .rgba
                .chunks_exact(4)
                .zip(base.rgba.chunks_exact(4))
                .filter(|(after, before)| after != before)
                .count();
            changed_pixels = changed_pixels.max(changed);
        }
        assert!(
            changed_pixels > 50,
            "panel overlay changed no visible pixels: {changed_pixels}"
        );
    }

    #[test]
    fn mailer_box_has_a_connected_foldable_net_and_exportable_flat_layout() {
        let dieline = mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap();
        assert_eq!(dieline["template"], "mailer");
        assert_eq!(dieline["panels"].as_array().unwrap().len(), 7);
        assert_eq!(dieline["creases"].as_array().unwrap().len(), 6);
        assert_eq!(dieline["paths"][0]["role"], "cut");
        assert_eq!(dieline["paths"][1]["role"], "crease");
        assert_eq!(dieline["paths"][2]["role"], "glue-flap");
        let cut_path =
            kurbo::BezPath::from_svg(dieline["paths"][0]["d"].as_str().unwrap()).unwrap();
        assert!(cut_path
            .elements()
            .iter()
            .any(|element| matches!(element, kurbo::PathEl::ClosePath)));

        let flat = generate(&dieline).unwrap();
        let mut folded_dieline = dieline.clone();
        folded_dieline["fold_state"] = serde_json::json!(0.75);
        let folded = generate(&folded_dieline).unwrap();
        assert_eq!(flat.primitives.len(), 21);
        assert_eq!(flat.report.triangles, 72);
        assert!(flat
            .primitives
            .iter()
            .zip(&folded.primitives)
            .any(|(flat, folded)| flat.positions != folded.positions));
        assert!(flat_dieline_svg(&dieline)
            .unwrap()
            .contains("data-role=\"glue-flap\""));

        assert!(mailer_box_dieline(180.0, 120.0, 35.0, 100.0, 3.0, 18.0).is_err());
    }

    #[test]
    fn every_builtin_packaging_template_has_a_closed_cut_contour() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            tuck_carton_dieline("reverse-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap(),
            mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap(),
            pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap(),
            tube_label_dieline(30.0, 90.0, 3.0).unwrap(),
            bottle_label_dieline(30.0, 90.0, 3.0, 0.0).unwrap(),
        ];

        for dieline in templates {
            let template = dieline["template"].as_str().unwrap();
            let cut_paths = dieline["paths"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|path| path["role"] == "cut")
                .collect::<Vec<_>>();
            assert_eq!(
                cut_paths.len(),
                1,
                "{template} must define exactly one primary cut path"
            );
            let data = cut_paths[0]["d"].as_str().unwrap();
            let path = kurbo::BezPath::from_svg(data)
                .unwrap_or_else(|error| panic!("{template} cut path must parse: {error}"));
            assert!(path.is_finite(), "{template} cut path must be finite");
            assert!(
                path.elements()
                    .iter()
                    .any(|element| matches!(element, kurbo::PathEl::ClosePath)),
                "{template} cut path must include a closed trim contour"
            );
        }
    }

    #[test]
    fn supported_package_panels_preserve_dimensions_at_intermediate_fold_states() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            tuck_carton_dieline("reverse-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap(),
            mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap(),
            pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap(),
        ];
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };

        for source in templates {
            let cut_path =
                kurbo::BezPath::from_svg(source["paths"][0]["d"].as_str().unwrap()).unwrap();
            let move_count = cut_path
                .elements()
                .iter()
                .filter(|element| matches!(element, kurbo::PathEl::MoveTo(_)))
                .count();
            let close_count = cut_path
                .elements()
                .iter()
                .filter(|element| matches!(element, kurbo::PathEl::ClosePath))
                .count();
            assert!(
                close_count > 0,
                "{} cut outline must include at least one closed contour",
                source["template"]
            );
            if matches!(
                source["template"].as_str(),
                Some("straight-tuck" | "reverse-tuck")
            ) {
                // Inset dust flaps leave open notches instead of slits, so the
                // whole net is one closed contour.
                assert_eq!(
                    move_count, close_count,
                    "{} cut outline",
                    source["template"]
                );
            }
            let mut path_segments = Vec::new();
            let mut current = None;
            let mut start = None;
            for element in cut_path.elements() {
                match element {
                    kurbo::PathEl::MoveTo(point) => {
                        current = Some([point.x, point.y]);
                        start = current;
                    }
                    kurbo::PathEl::LineTo(point) => {
                        let end = [point.x, point.y];
                        if let Some(from) = current {
                            path_segments.push([from, end]);
                        }
                        current = Some(end);
                    }
                    kurbo::PathEl::ClosePath => {
                        if let (Some(from), Some(to)) = (current, start) {
                            path_segments.push([from, to]);
                            current = Some(to);
                        }
                    }
                    kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..) => {
                        panic!("generated packaging cut contours must stay linear")
                    }
                }
            }
            for crease in source["creases"].as_array().unwrap() {
                let line = &crease["line_mm"];
                let crease_edge = [
                    [line[0][0].as_f64().unwrap(), line[0][1].as_f64().unwrap()],
                    [line[1][0].as_f64().unwrap(), line[1][1].as_f64().unwrap()],
                ];
                for segment in &path_segments {
                    let horizontal = (segment[0][1] - segment[1][1]).abs() < 1e-6
                        && (crease_edge[0][1] - crease_edge[1][1]).abs() < 1e-6
                        && (segment[0][1] - crease_edge[0][1]).abs() < 1e-6;
                    let vertical = (segment[0][0] - segment[1][0]).abs() < 1e-6
                        && (crease_edge[0][0] - crease_edge[1][0]).abs() < 1e-6
                        && (segment[0][0] - crease_edge[0][0]).abs() < 1e-6;
                    let overlap = if horizontal {
                        segment[0][0]
                            .max(segment[1][0])
                            .min(crease_edge[0][0].max(crease_edge[1][0]))
                            - segment[0][0]
                                .min(segment[1][0])
                                .max(crease_edge[0][0].min(crease_edge[1][0]))
                    } else if vertical {
                        segment[0][1]
                            .max(segment[1][1])
                            .min(crease_edge[0][1].max(crease_edge[1][1]))
                            - segment[0][1]
                                .min(segment[1][1])
                                .max(crease_edge[0][1].min(crease_edge[1][1]))
                    } else {
                        0.0
                    };
                    assert!(
                        overlap <= 1e-6,
                        "{} cut plate overlaps crease `{}` by {overlap} mm",
                        source["template"],
                        crease["id"]
                    );
                }
            }
            for fold in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut dieline = source.clone();
                dieline["fold_state"] = json!(fold);
                let model = generate(&dieline).unwrap_or_else(|error| {
                    panic!("{} at fold {fold}: {error}", source["template"])
                });
                for (index, (panel, primitive)) in dieline["panels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(&model.primitives)
                    .enumerate()
                {
                    let rect = rect(&panel["rect_mm"], "test panel", false).unwrap();
                    let [_, _, width, height] = rect;
                    let p = &primitive.positions;
                    assert!(
                        p.iter().flatten().all(|coordinate| coordinate.is_finite()),
                        "{} panel {index} has non-finite coordinates at fold {fold}",
                        source["template"]
                    );
                    assert!(
                        (distance(p[0], p[1]) - width as f32).abs() < 1e-3,
                        "{} panel {index} width changed at fold {fold}",
                        source["template"]
                    );
                    assert!(
                        (distance(p[0], p[3]) - height as f32).abs() < 1e-3,
                        "{} panel {index} height changed at fold {fold}",
                        source["template"]
                    );
                }
            }
        }
    }

    #[test]
    fn all_builtin_rectangular_panel_hinges_remain_joined_through_fold_states() {
        let templates = [
            tuck_carton_dieline("straight-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            tuck_carton_dieline("reverse-tuck", 120.0, 70.0, 180.0, 0.5, 3.0, 15.0).unwrap(),
            sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap(),
            mailer_box_dieline(180.0, 120.0, 35.0, 0.6, 3.0, 18.0).unwrap(),
            pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap(),
        ];
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        for source in templates {
            let panel_values = source["panels"].as_array().unwrap();
            let rects = panel_values
                .iter()
                .map(|panel| rect(&panel["rect_mm"], "test panel", false).unwrap())
                .collect::<Vec<_>>();
            for fold in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let mut dieline = source.clone();
                dieline["fold_state"] = json!(fold);
                let model = generate(&dieline).unwrap();
                for first in 0..rects.len() {
                    for second in first + 1..rects.len() {
                        let Some(edge) = panel_adjacent(rects[first], rects[second]) else {
                            continue;
                        };
                        let edge_vertices = |index: usize| {
                            let [x, y, width, height] = rects[index];
                            [
                                [x, y],
                                [x + width, y],
                                [x + width, y + height],
                                [x, y + height],
                            ]
                        };
                        let find_vertex = |index: usize, target: [f64; 2]| {
                            edge_vertices(index)
                                .iter()
                                .position(|point| {
                                    (point[0] - target[0]).abs() < 1e-8
                                        && (point[1] - target[1]).abs() < 1e-8
                                })
                                .unwrap_or_else(|| {
                                    panic!("shared edge endpoint missing from panel")
                                })
                        };
                        let first_indices =
                            [find_vertex(first, edge[0]), find_vertex(first, edge[1])];
                        let second_indices =
                            [find_vertex(second, edge[0]), find_vertex(second, edge[1])];
                        let a = &model.primitives[first].positions;
                        let b = &model.primitives[second].positions;
                        let direct = distance(a[first_indices[0]], b[second_indices[0]])
                            + distance(a[first_indices[1]], b[second_indices[1]]);
                        let reverse = distance(a[first_indices[0]], b[second_indices[1]])
                            + distance(a[first_indices[1]], b[second_indices[0]]);
                        assert!(
                            direct.min(reverse) < 1e-3,
                            "{} panels `{}` and `{}` separate at fold {fold}: {direct}/{reverse}",
                            source["template"],
                            panel_values[first]["id"],
                            panel_values[second]["id"]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn stand_up_pouch_hinges_remain_gap_free_through_fold_states() {
        let source = pouch_dieline(120.0, 35.0, 180.0, 3.0).unwrap();
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let assert_joined = |first: &[f32; 3],
                             second: &[f32; 3],
                             third: &[f32; 3],
                             fourth: &[f32; 3],
                             seam: &str,
                             fold: f64| {
            let direct = distance(*first, *third) + distance(*second, *fourth);
            let reversed = distance(*first, *fourth) + distance(*second, *third);
            assert!(
                direct.min(reversed) < 1e-3,
                "{seam} opens at fold {fold}: {direct}/{reversed}"
            );
        };
        for fold in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mut dieline = source.clone();
            dieline["fold_state"] = json!(fold);
            let model = generate(&dieline).unwrap();
            let panels = &model.primitives;
            assert_joined(
                &panels[0].positions[1],
                &panels[0].positions[2],
                &panels[1].positions[0],
                &panels[1].positions[3],
                "left gusset",
                fold,
            );
            assert_joined(
                &panels[1].positions[1],
                &panels[1].positions[2],
                &panels[2].positions[0],
                &panels[2].positions[3],
                "right gusset",
                fold,
            );
            assert_joined(
                &panels[1].positions[3],
                &panels[1].positions[2],
                &panels[3].positions[0],
                &panels[3].positions[1],
                "bottom gusset",
                fold,
            );
            assert_joined(
                &panels[3].positions[3],
                &panels[3].positions[2],
                &panels[4].positions[0],
                &panels[4].positions[1],
                "back panel",
                fold,
            );
        }
    }

    #[test]
    fn per_crease_angle_changes_only_its_fold_target_and_is_failure_atomic() {
        let mut document: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/v11-product-packaging.pen"
        ))
        .unwrap();
        let original = document.clone();
        let flat_source = &document["dielines"]["shirt-carton"];
        let before = generate(flat_source).unwrap();
        let changed =
            set_crease_angle(&mut document, "shirt-carton", "front-right-fold", 45.0).unwrap();
        assert_eq!(changed["previous_angle_deg"], 90.0);
        assert_eq!(changed["angle_deg"], 45.0);
        assert_eq!(document["dielines"]["shirt-carton"]["fold_state"], 1.0);
        assert_eq!(
            document["dielines"]["shirt-carton"]["creases"][0]["angle_deg"],
            90.0
        );
        let after = generate(&document["dielines"]["shirt-carton"]).unwrap();
        assert_ne!(
            before.primitives[1].positions,
            after.primitives[1].positions
        );

        let mut invalid = original.clone();
        assert!(set_crease_angle(&mut invalid, "shirt-carton", "front-right-fold", 181.0).is_err());
        assert_eq!(invalid, original);
        assert!(set_crease_angle(&mut invalid, "shirt-carton", "missing", 45.0).is_err());
        assert_eq!(invalid, original);
    }

    #[test]
    fn generated_sleeve_has_four_faces_and_closes_its_cross_section() {
        let dieline = sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap();
        let model = generate(&dieline).unwrap();
        assert_eq!(model.report.triangles, 42);
        let first = &model.primitives[0].positions;
        let last = &model.primitives[3].positions;
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let direct = distance(first[0], last[1]) + distance(first[3], last[2]);
        let reversed = distance(first[0], last[2]) + distance(first[3], last[1]);
        assert!(
            direct.min(reversed) < 1e-3,
            "sleeve seam is not closed: {direct} / {reversed}"
        );
        assert_eq!(
            model.primitives[0].uvs[0],
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        );
    }

    #[test]
    fn tube_label_maps_its_print_width_to_a_closed_cylindrical_surface() {
        let mut dieline = tube_label_dieline(32.0, 95.0, 3.0).unwrap();
        dieline["wrap"]["seam_angle_deg"] = json!(42.0);
        dieline["wrap"]["direction"] = json!("counterclockwise");
        let flat_svg = flat_dieline_svg(&dieline).unwrap();
        assert!(flat_svg.contains("data-role=\"wrap-seam-preview\""));
        assert!(flat_svg.contains("data-angle-deg=\"42\""));
        assert!(flat_svg.contains("WRAP SEAM · 42°"));
        let model = generate(&dieline).unwrap();
        assert_eq!(model.primitives.len(), 64);
        assert_eq!(model.report.format, "pentool-tube-label");
        assert_eq!(model.report.triangles, 128);
        assert_eq!(dieline["panels"][0]["face_slot"], "label");
        let first = &model.primitives[0].positions;
        let last = &model.primitives[63].positions;
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        assert!(distance(first[0], last[1]) < 1e-4);
        assert!(distance(first[3], last[2]) < 1e-4);
        assert!((distance(first[0], first[3]) - 95.0).abs() < 1e-4);
        assert!(model
            .primitives
            .iter()
            .flat_map(|primitive| primitive.uvs[0].iter())
            .all(|uv| uv.iter().all(|coordinate| coordinate.is_finite())));

        let mut malformed = dieline.clone();
        malformed["panels"][0]["rect_mm"][2] = json!(1.0);
        assert!(generate(&malformed)
            .unwrap_err()
            .to_string()
            .contains("2π × wrap radius"));
        assert!(tube_label_dieline(0.0, 95.0, 3.0).is_err());

        let overlap_dieline = tube_label_dieline_with_overlap(32.0, 95.0, 3.0, 5.0).unwrap();
        let flat_width = overlap_dieline["panels"][0]["rect_mm"][2].as_f64().unwrap();
        let expected = std::f64::consts::TAU * 32.0 + 5.0;
        assert!((flat_width - expected).abs() < 1e-9);
        assert!((overlap_dieline["wrap"]["overlap_mm"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        let overlap_model = generate(&overlap_dieline).unwrap();
        assert_eq!(overlap_model.primitives.len(), 66);
        assert!(overlap_model.primitives[64..].iter().all(|primitive| {
            primitive.name.contains("overlap")
                && primitive.positions.iter().all(|point| {
                    (f64::from(point[0]).hypot(f64::from(point[2])) - 32.05).abs() < 1e-4
                })
        }));
        let overlap_svg = flat_dieline_svg(&overlap_dieline).unwrap();
        assert!(overlap_svg.contains("data-role=\"wrap-overlap-preview\""));
        let too_much_overlap = std::f64::consts::TAU * 32.0 * 0.2501;
        assert!(tube_label_dieline_with_overlap(32.0, 95.0, 3.0, too_much_overlap).is_err());

        let bottle = bottle_label_dieline(30.0, 90.0, 3.0, 2.0).unwrap();
        assert_eq!(bottle["template"], "bottle-label");
        assert_eq!(bottle["panels"][0]["id"], "bottle-label");
        assert_eq!(bottle["panels"][0]["face_slot"], "label");
        let bottle_model = generate(&bottle).unwrap();
        assert_eq!(bottle_model.primitives.len(), 65);
        assert_eq!(bottle_model.report.format, "pentool-bottle-label");
        assert!(flat_dieline_svg(&bottle)
            .unwrap()
            .contains("data-role=\"wrap-seam-preview\""));
    }

    #[test]
    fn panel_cycles_and_uncreased_adjacencies_fail_closed() {
        let mut net = two_panel_net(1.0);
        net["panels"].as_array_mut().unwrap().push(
            json!({"id":"upper","face_slot":"upper","rect_mm":[0,10,10,10],"uv_rect":[0,0,1,1]}),
        );
        net["panels"].as_array_mut().unwrap().push(json!({"id":"upper-right","face_slot":"upper-right","rect_mm":[10,10,10,10],"uv_rect":[0,0,1,1]}));
        net["creases"].as_array_mut().unwrap().extend([
            json!({"id":"upper-fold","line_mm":[[10,10],[10,20]],"angle_deg":90,"direction":"valley"}),
            json!({"id":"top-fold","line_mm":[[0,10],[10,10]],"angle_deg":90,"direction":"valley"}),
            json!({"id":"cross-fold","line_mm":[[10,10],[20,10]],"angle_deg":90,"direction":"valley"}),
            json!({"id":"back-fold","line_mm":[[10,20],[20,20]],"angle_deg":90,"direction":"valley"})
        ]);
        let error = generate(&net).unwrap_err().to_string();
        assert!(error.contains("cycle"), "{error}");

        let mut uncreased = two_panel_net(1.0);
        uncreased["creases"] = json!([]);
        assert!(generate(&uncreased)
            .unwrap_err()
            .to_string()
            .contains("need a crease"));
    }

    #[test]
    fn duplicate_hinge_creases_are_rejected_as_ambiguous_geometry() {
        let mut duplicate = two_panel_net(1.0);
        duplicate["creases"].as_array_mut().unwrap().push(
            json!({"id":"duplicate-right-fold","line_mm":[[10,0],[10,10]],"angle_deg":45,"direction":"mountain"}),
        );
        let error = generate(&duplicate).unwrap_err().to_string();
        assert!(error.contains("matches multiple crease records"), "{error}");
    }

    #[test]
    fn crease_not_attached_to_a_panel_hinge_is_rejected() {
        let mut net = two_panel_net(1.0);
        net["creases"].as_array_mut().unwrap().push(
            json!({"id":"orphan-crease","line_mm":[[0,20],[10,20]],"angle_deg":45,"direction":"mountain"}),
        );
        let error = generate(&net).unwrap_err().to_string();
        assert!(error.contains("orphan-crease"), "{error}");
        assert!(error.contains("does not match a panel edge"), "{error}");
    }
}
