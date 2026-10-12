//! Bounded, read-only soft-proof evaluation for native color samples.
//!
//! A proof sample travels through the selected proof device profile and back to
//! D50 PCS before it is converted to the display target. This is a numeric
//! profile simulation, not a calibrated monitor preview or a complete scene
//! renderer.
use super::{
    catalog,
    icc::Intent,
    roles::{self, Role},
    value::{Destination, Evaluator, NativeColor, ProfileSet},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::Path;

pub const MAX_PROOF_SAMPLES: usize = 4096;
/// Round-trip errors below this value are treated as numeric noise, not a
/// useful gamut warning. The value is Delta E 76 in physical D50 Lab.
pub const GAMUT_DELTA_E_THRESHOLD: f64 = 0.5;

/// Evaluate one sample against the document's configured proof and display
/// roles. The document is never modified.
pub fn evaluate_sample(
    raw: &Value,
    document: &Path,
    sample: &Value,
    preserve_numbers: bool,
) -> Result<Value> {
    ensure!(
        crate::color::has_v8(raw),
        "[unsupported-capability] soft proof requires an explicitly migrated v8 document"
    );
    let source = NativeColor::parse(sample)?;
    let proof = roles::effective(raw, Role::Proof)?;
    let display = roles::effective(raw, Role::Display)?;
    let proof_profile = proof
        .destination
        .profile()
        .expect("proof role validation requires an ICC profile");
    let display_profile = display.destination.profile();

    let proof_set = ProfileSet::load(
        raw,
        document,
        source
            .destination()
            .profile()
            .into_iter()
            .chain([proof_profile]),
    )?;
    let mut proof_engine = Evaluator::new(&proof_set, proof.intent)?;
    let (source_xyz, source_clipped) = proof_engine.to_xyz(&source)?;
    let (proof_color, number_preserved, conversion_clipped) = if preserve_numbers
        && same_device_profile(raw, source.destination(), &proof.destination)?
    {
        (source.clone(), true, false)
    } else {
        let converted = proof_engine.convert(&source, &proof.destination)?;
        (converted.color, false, converted.clipped)
    };
    let (proof_xyz, proof_decode_clipped) = proof_engine.to_xyz(&proof_color)?;
    let source_lab = super::pcs::xyz_to_lab(source_xyz)?;
    let proof_lab = super::pcs::xyz_to_lab(proof_xyz)?;
    let delta_e_76 = source_lab
        .iter()
        .zip(proof_lab)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f64>()
        .sqrt();
    let gamut_warning =
        conversion_clipped || proof_decode_clipped || delta_e_76 > GAMUT_DELTA_E_THRESHOLD;

    let proof_appearance = NativeColor::parse(&json!({
        "space":"lab-d50",
        "channels":proof_lab,
        "alpha":proof_color.alpha()
    }))?;
    let display_set = ProfileSet::load(raw, document, display_profile)?;
    let mut display_engine = Evaluator::new(&display_set, display.intent)?;
    let display_result = display_engine.convert(&proof_appearance, &display.destination)?;

    let proof_record = catalog::resolve(raw, proof_profile)?.1;
    let capabilities = {
        let profile = catalog::load(raw, document, proof_profile)?;
        let parsed = super::icc::Profile::parse(&profile.1)?;
        let white = parsed.media_white_point().ok();
        json!({
            "paper_simulation": proof.intent == Intent::AbsoluteColorimetric && white.is_some(),
            "paper_simulation_method": if proof.intent == Intent::AbsoluteColorimetric { "icc-absolute-media-white-scaling" } else { "unavailable-with-selected-intent" },
            "paper_white_xyz_d50": if proof.intent == Intent::AbsoluteColorimetric { white } else { None::<[f64; 3]> },
            "ink_simulation": true,
            "ink_simulation_method": "proof-profile-device-to-pcs-round-trip",
            "spectral_paper_or_ink_model": false
        })
    };
    let source_identity = source
        .destination()
        .profile()
        .map(|reference| catalog::resolve(raw, reference).map(|(identity, _)| identity))
        .transpose()?;
    let proof_identity = catalog::resolve(raw, proof_profile)?.0;
    let display_identity = display_profile
        .map(|reference| catalog::resolve(raw, reference).map(|(identity, _)| identity))
        .transpose()?;
    let profile_ids = source_identity
        .into_iter()
        .chain([proof_identity])
        .chain(display_identity)
        .collect::<Vec<_>>();

    Ok(json!({
        "version": 1,
        "scope": "soft-proof-sample",
        "source": source.json(),
        "source_profile": source_identity,
        "source_xyz_d50": source_xyz,
        "source_clipped": source_clipped,
        "proof": {
            "profile": proof_identity,
            "intent": proof.intent,
            "channels": proof_color.json(),
            "preserve_numbers_requested": preserve_numbers,
            "numbers_preserved": number_preserved,
            "conversion_clipped": conversion_clipped,
            "round_trip_clipped": proof_decode_clipped,
            "round_trip_xyz_d50": proof_xyz,
            "round_trip_delta_e_76": delta_e_76,
            "gamut_warning": gamut_warning,
            "gamut_warning_method": "proof-device-round-trip-delta-e-76-or-transform-clipping",
            "gamut_warning_threshold_delta_e_76": GAMUT_DELTA_E_THRESHOLD,
            "capabilities": capabilities,
            "profile_class": proof_record["class"]
        },
        "display": {
            "destination": display.destination.json(),
            "intent": display.intent,
            "color": display_result.color.json(),
            "clipped": display_result.clipped,
            "comparison": "source-pcs-versus-proof-round-trip-delta-e-76",
            "limitations": [
                "display transform targets the configured profile but cannot calibrate or characterize the physical monitor",
                "browser color management and operating-system display settings are outside this evaluator",
                "no spectral reflectance, fluorescence, viewing-light, or substrate texture simulation is performed",
                "this sample report does not render scene geometry, transparency interactions, or a proof image"
            ]
        },
        "profiles": profile_ids,
        "mutated": false
    }))
}

/// Evaluate a bounded list with stable order. Each sample uses the same explicit
/// proof/display role policy; no partial result is returned on failure.
pub fn evaluate_samples(
    raw: &Value,
    document: &Path,
    samples: &[Value],
    preserve_numbers: bool,
) -> Result<Value> {
    ensure!(
        samples.len() <= MAX_PROOF_SAMPLES,
        "[limit-exceeded] soft proof accepts at most {MAX_PROOF_SAMPLES} samples per batch"
    );
    let reports = samples
        .iter()
        .map(|sample| evaluate_sample(raw, document, sample, preserve_numbers))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "version": 1,
        "scope": "soft-proof-samples",
        "matches": reports.len(),
        "returned": reports.len(),
        "limit": MAX_PROOF_SAMPLES,
        "has_more": false,
        "reports": reports,
        "mutated": false
    }))
}

fn same_device_profile(raw: &Value, source: &Destination, proof: &Destination) -> Result<bool> {
    if source.space() != proof.space() {
        return Ok(false);
    }
    let (Some(source), Some(proof)) = (source.profile(), proof.profile()) else {
        return Ok(false);
    };
    Ok(catalog::resolve(raw, source)?.0 == catalog::resolve(raw, proof)?.0)
}
