//! Native color values and shared physical-PCS evaluation for scene and print tools.
//! Stored channels are independent of preview pixels; alpha never enters an ICC CMM.
use super::{
    catalog,
    icc::{Intent, Link, LinkDescription, Profile},
    pcs,
};
use crate::photo::color::{self, ColorSpace, Matrix};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub const MAX_COMPILED_LINKS: usize = 64;
/// Matrix inversion at a gamut face can differ by a few floating-point ulps.
pub const CHANNEL_ROUNDOFF: f64 = crate::blend::RGB_ROUNDOFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    space: String,
    profile: Option<String>,
    count: usize,
}

impl Destination {
    pub fn parse(value: &Value) -> Result<Self> {
        let fields = value
            .as_object()
            .context("[invalid-input] color destination must be an object")?;
        ensure!(
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "space" | "profile")),
            "[invalid-input] color destination only accepts space and profile"
        );
        Self::fields(value)
    }

    fn fields(value: &Value) -> Result<Self> {
        let space = value["space"]
            .as_str()
            .context("[invalid-input] color space is required")?;
        let count = match space {
            "icc-gray" => 1,
            "icc-cmyk" => 4,
            "icc-rgb" | "lab-d50" => 3,
            named => {
                let base = named.strip_suffix(":linear").unwrap_or(named);
                ensure!(
                    color::NAMED_SPACES.contains(&base),
                    "[unsupported-capability] unsupported native color space {named}"
                );
                ColorSpace::named(named)?;
                3
            }
        };
        let profile = if space.starts_with("icc-") {
            let profile = value["profile"]
                .as_str()
                .context("[missing-resource] ICC color requires a profile reference")?;
            ensure!(
                !profile.is_empty() && profile.len() <= 80,
                "[invalid-input] ICC profile reference must contain 1..=80 UTF-8 bytes"
            );
            catalog::validate_reference(profile)?;
            Some(profile.to_owned())
        } else {
            ensure!(
                value.get("profile").is_none(),
                "[invalid-input] named RGB and D50 Lab cannot carry an ICC profile reference"
            );
            None
        };
        Ok(Self {
            space: space.into(),
            profile,
            count,
        })
    }

    pub fn space(&self) -> &str {
        &self.space
    }
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }
    pub fn channel_count(&self) -> usize {
        self.count
    }
    /// Production RGB blending requires a declared linear transfer and primaries.
    /// ICC device channels, CMYK and Lab are interpretations, not this blend space.
    pub fn is_linear_rgb(&self) -> bool {
        self.profile.is_none() && self.space.ends_with(":linear")
    }
    pub fn json(&self) -> Value {
        let mut value = json!({"space":self.space});
        if let Some(profile) = &self.profile {
            value["profile"] = json!(profile);
        }
        value
    }
    pub(crate) fn signature(&self) -> Option<&'static str> {
        match self.space.as_str() {
            "icc-rgb" => Some("RGB "),
            "icc-gray" => Some("GRAY"),
            "icc-cmyk" => Some("CMYK"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeColor {
    destination: Destination,
    channels: [f64; 4],
    alpha: f64,
}

impl NativeColor {
    /// Interpret released hex/RGBA paint syntax as sRGB without changing its
    /// meaning. `none` and absent optional paints have no color to convert.
    pub fn from_legacy(value: &Value) -> Result<Option<Self>> {
        if value.is_null() || value.as_str() == Some("none") {
            return Ok(None);
        }
        let rgba = crate::fill::color(value, &Value::Null)?;
        Ok(Some(Self::parse(
            &json!({"space":"srgb","channels":&rgba[..3],"alpha":rgba[3]}),
        )?))
    }
    pub fn parse(value: &Value) -> Result<Self> {
        let fields = value
            .as_object()
            .context("[invalid-input] native color must be an object")?;
        ensure!(
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "space" | "profile" | "channels" | "alpha")),
            "[invalid-input] native color only accepts space, profile, channels and alpha"
        );
        let destination = Destination::fields(value)?;
        let channels = value["channels"]
            .as_array()
            .context("[invalid-input] native color channels must be an array")?;
        ensure!(
            channels.len() == destination.count,
            "[invalid-input] {} requires {} channels",
            destination.space,
            destination.count
        );
        let mut sample = [0.0; 4];
        for (i, value) in channels.iter().enumerate() {
            sample[i] = value
                .as_f64()
                .filter(|v| v.is_finite())
                .context("[invalid-input] native color channels must be finite numbers")?;
            if destination.space != "lab-d50" {
                ensure!(
                    (0.0..=1.0).contains(&sample[i]),
                    "[invalid-input] {} channel {i} must be in [0,1]",
                    destination.space
                );
            }
        }
        if destination.space == "lab-d50" {
            pcs::lab_to_xyz(sample[..3].try_into().unwrap())?;
        }
        let alpha = match value.get("alpha") {
            None => 1.0,
            Some(alpha) => alpha
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                .context("[invalid-input] native color alpha must be in [0,1]")?,
        };
        Ok(Self {
            destination,
            channels: sample,
            alpha,
        })
    }
    pub fn destination(&self) -> &Destination {
        &self.destination
    }
    pub fn channels(&self) -> &[f64] {
        &self.channels[..self.destination.count]
    }
    pub fn alpha(&self) -> f64 {
        self.alpha
    }
    /// Reuse a prepared sample without allocating per-pixel JSON or channel
    /// buffers. Endpoints must already use exactly the same destination.
    pub(crate) fn blend(&mut self, from: &Self, to: &Self, position: f64) -> Result<()> {
        ensure!(
            position.is_finite() && (0.0..=1.0).contains(&position),
            "[invalid-input] color interpolation position must be finite and in [0,1]"
        );
        ensure!(self.destination == from.destination && self.destination == to.destination, "[invalid-input] interpolation endpoints must be converted into the same destination first");
        for i in 0..self.destination.count {
            self.channels[i] = from.channels[i] * (1.0 - position) + to.channels[i] * position;
        }
        self.alpha = from.alpha * (1.0 - position) + to.alpha * position;
        if self.destination.space == "lab-d50" {
            pcs::lab_to_xyz(self.channels[..3].try_into().unwrap())?;
        }
        Ok(())
    }
    pub fn json(&self) -> Value {
        let mut value = self.destination.json();
        value["channels"] = json!(self.channels());
        value["alpha"] = json!(self.alpha);
        value
    }
}

/// Original verified bytes are owned once; compiled engines borrow them. Only
/// explicitly used references are loaded, so unrelated external profiles stay offline.
#[derive(Debug, Default)]
pub struct ProfileSet {
    bytes: BTreeMap<String, Vec<u8>>,
    references: BTreeMap<String, String>,
}
impl ProfileSet {
    pub fn load<'a>(
        raw: &Value,
        document: &Path,
        references: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self> {
        let mut set = Self::default();
        let catalog = catalog::Catalog::new(raw)?;
        for reference in references {
            if set.references.contains_key(reference) {
                continue;
            }
            let (identity, _) = catalog.resolve(reference)?;
            let identity = identity.to_owned();
            if !set.bytes.contains_key(&identity) {
                let (_, bytes) = catalog.load(document, reference)?;
                set.bytes.insert(identity.clone(), bytes);
            }
            set.references.insert(reference.into(), identity);
        }
        Ok(set)
    }
    fn identity(&self, reference: &str) -> Result<&str> {
        self.references
            .get(reference)
            .map(String::as_str)
            .with_context(|| {
                format!(
                    "[missing-resource] ICC {reference} was not loaded for this color evaluation"
                )
            })
    }
}

struct Named {
    space: ColorSpace,
    to_pcs: Matrix,
    from_pcs: Matrix,
}

#[derive(Debug)]
pub struct Evaluation {
    pub color: NativeColor,
    pub clipped: bool,
    pub path: String,
    pub link: Option<LinkDescription>,
}

/// A transient linear working sample, not a stored `.pen` device color. RGB may
/// be negative or above one; only coverage is normalized. Gamut excursions are
/// reported separately from clipping inside the selected source ICC path.
#[derive(Debug)]
pub struct WorkingEvaluation {
    pub working_space: Destination,
    pub rgba: [f64; 4],
    pub source_clipped: bool,
    pub outside_working_gamut: bool,
    pub source_link: Option<LinkDescription>,
}
impl WorkingEvaluation {
    pub fn channels(&self) -> &[f64] {
        &self.rgba[..3]
    }
    pub fn alpha(&self) -> f64 {
        self.rgba[3]
    }
    pub fn destination(&self) -> &Destination {
        &self.working_space
    }
}

/// Compile each named space and selected ICC pair at most once per evaluation.
pub struct Evaluator<'a> {
    set: &'a ProfileSet,
    profiles: BTreeMap<&'a str, (Profile<'a>, String, String)>,
    links: BTreeMap<String, BTreeMap<String, Link<'a>>>,
    link_count: usize,
    named: BTreeMap<String, Named>,
    intent: Intent,
}
impl<'a> Evaluator<'a> {
    pub fn new(set: &'a ProfileSet, intent: Intent) -> Result<Self> {
        let mut profiles = BTreeMap::new();
        for (identity, bytes) in &set.bytes {
            let profile = Profile::parse(bytes)?;
            let summary = profile.summary();
            profiles.insert(
                identity.as_str(),
                (profile, summary.color_space, summary.class),
            );
        }
        Ok(Self {
            set,
            profiles,
            links: BTreeMap::new(),
            link_count: 0,
            named: BTreeMap::new(),
            intent,
        })
    }

    fn validate_destination(&self, destination: &Destination) -> Result<()> {
        if let Some(reference) = destination.profile() {
            let identity = self.set.identity(reference)?;
            let (_, signature, class) = &self.profiles[identity];
            ensure!(
                Some(signature.as_str()) == destination.signature(),
                "[invalid-input] ICC {reference} has {signature:?} channels, incompatible with {}",
                destination.space
            );
            ensure!(class != "link", "[unsupported-capability] device-link ICC {reference} requires an explicit device-link workflow");
        }
        Ok(())
    }

    fn named(&mut self, id: &str) -> Result<&Named> {
        if !self.named.contains_key(id) {
            let space = ColorSpace::named(id)?;
            let to_pcs = color::multiply(
                &color::bradford_xyz(color::xyz(space.white), pcs::D50),
                &space.to_xyz(),
            );
            let from_pcs = color::invert(&to_pcs);
            self.named.insert(
                id.into(),
                Named {
                    space,
                    to_pcs,
                    from_pcs,
                },
            );
        }
        Ok(&self.named[id])
    }

    fn link(&mut self, source: Option<&str>, destination: Option<&str>) -> Result<&Link<'a>> {
        let source = source
            .map(|id| self.set.identity(id))
            .transpose()?
            .unwrap_or_default();
        let destination = destination
            .map(|id| self.set.identity(id))
            .transpose()?
            .unwrap_or_default();
        if !self
            .links
            .get(source)
            .is_some_and(|links| links.contains_key(destination))
        {
            ensure!(self.link_count < MAX_COMPILED_LINKS, "[limit-exceeded] native color evaluation exceeds {MAX_COMPILED_LINKS} compiled ICC links; evaluate a smaller batch");
            let compiled = match (source.is_empty(), destination.is_empty()) {
                (false, false) => Link::compile(
                    &self.profiles[source].0,
                    &self.profiles[destination].0,
                    self.intent,
                ),
                (false, true) => Link::to_pcs(&self.profiles[source].0, self.intent),
                (true, false) => Link::from_pcs(&self.profiles[destination].0, self.intent),
                _ => bail!("[invalid-input] PCS-to-PCS does not require an ICC link"),
            }
            .with_context(|| format!("native color ICC link {source:?} -> {destination:?}"))?;
            self.links
                .entry(source.into())
                .or_default()
                .insert(destination.into(), compiled);
            self.link_count += 1;
        }
        Ok(&self.links[source][destination])
    }

    fn named_from_xyz(
        &mut self,
        xyz: [f64; 3],
        space: &str,
        source_clipped: bool,
    ) -> Result<([f64; 3], bool)> {
        let named = self.named(space)?;
        let linear = color::apply(&named.from_pcs, xyz);
        let mut channels = [0.0; 3];
        let mut clipped = source_clipped;
        for (channel, linear) in channels.iter_mut().zip(linear) {
            let encoded = named.space.transfer.encode(linear);
            ensure!(
                encoded.is_finite(),
                "[invalid-input] native RGB conversion overflow"
            );
            clipped |= !(-CHANNEL_ROUNDOFF..=1.0 + CHANNEL_ROUNDOFF).contains(&encoded);
            *channel = encoded.clamp(0.0, 1.0);
        }
        Ok((channels, clipped))
    }

    /// Ordinary sRGB preview using the same conversion math, without allocating
    /// link reports or constructing a new native value for each pixel.
    pub(crate) fn preview_rgba(&mut self, source: &NativeColor) -> Result<[f64; 4]> {
        let channels = if source.destination.space == "srgb" {
            source.channels[..3].try_into().unwrap()
        } else {
            let (xyz, clipped) = self.to_xyz(source)?;
            self.named_from_xyz(xyz, "srgb", clipped)?.0
        };
        Ok([channels[0], channels[1], channels[2], source.alpha])
    }

    pub fn to_xyz(&mut self, color: &NativeColor) -> Result<([f64; 3], bool)> {
        self.validate_destination(color.destination())?;
        let (xyz, clipped) = if let Some(profile) = color.destination.profile() {
            let sample = self.link(Some(profile), None)?.evaluate(color.channels())?;
            (sample.channels().try_into().unwrap(), sample.clipped)
        } else if color.destination.space == "lab-d50" {
            (
                pcs::lab_to_xyz(color.channels().try_into().unwrap())?,
                false,
            )
        } else {
            let named = self.named(&color.destination.space)?;
            let linear = std::array::from_fn(|i| named.space.transfer.decode(color.channels[i]));
            (color::apply(&named.to_pcs, linear), false)
        };
        ensure!(
            xyz.iter().all(|v| v.is_finite()),
            "[invalid-input] native color XYZ conversion overflow"
        );
        Ok((xyz, clipped))
    }

    pub fn convert(
        &mut self,
        source: &NativeColor,
        destination: &Destination,
    ) -> Result<Evaluation> {
        self.validate_destination(source.destination())?;
        self.validate_destination(destination)?;
        let same_profile = match (source.destination.profile(), destination.profile()) {
            (Some(a), Some(b)) => self.set.identity(a)? == self.set.identity(b)?,
            (None, None) => true,
            _ => false,
        };
        if source.destination.space == destination.space && same_profile {
            return Ok(Evaluation {
                color: NativeColor {
                    destination: destination.clone(),
                    channels: source.channels,
                    alpha: source.alpha,
                },
                clipped: false,
                path: "identity".into(),
                link: None,
            });
        }
        let mut channels = [0.0; 4];
        let (clipped, path, link) = if let Some(to) = destination.profile() {
            let input;
            let values = if source.destination.profile().is_some() {
                source.channels()
            } else {
                input = self.to_xyz(source)?.0;
                &input
            };
            let link = self.link(source.destination.profile(), Some(to))?;
            let sample = link.evaluate(values)?;
            channels[..destination.count].copy_from_slice(sample.channels());
            (sample.clipped, "icc-link", Some(link.description().clone()))
        } else {
            let (xyz, source_clipped) = self.to_xyz(source)?;
            if destination.space == "lab-d50" {
                channels[..3].copy_from_slice(&pcs::xyz_to_lab(xyz)?);
                (source_clipped, "d50-pcs-to-lab", None)
            } else {
                let (rgb, clipped) =
                    self.named_from_xyz(xyz, &destination.space, source_clipped)?;
                channels[..3].copy_from_slice(&rgb);
                (clipped, "d50-pcs-to-named-rgb", None)
            }
        };
        Ok(Evaluation {
            color: NativeColor {
                destination: destination.clone(),
                channels,
                alpha: source.alpha,
            },
            clipped,
            path: path.into(),
            link,
        })
    }

    /// Convert one unquantized linear working RGB sample into a delivery
    /// destination through D50 PCS and the evaluator's selected intent. Returns
    /// the destination channels (zero padded) and whether the transform clipped.
    /// It never quantizes: that is the caller's explicit delivery boundary.
    pub(crate) fn working_to_destination(
        &mut self,
        rgb: [f64; 3],
        working: &Destination,
        destination: &Destination,
    ) -> Result<([f64; 4], bool)> {
        ensure!(
            working.is_linear_rgb(),
            "[unsupported-capability] output transform requires a named linear RGB working space"
        );
        self.validate_destination(destination)?;
        let xyz = color::apply(&self.named(working.space())?.to_pcs, rgb);
        ensure!(
            xyz.iter().all(|v| v.is_finite()),
            "[invalid-input] output transform XYZ overflow"
        );
        let mut channels = [0.0; 4];
        let clipped = if let Some(profile) = destination.profile() {
            let sample = self.link(None, Some(profile))?.evaluate(&xyz)?;
            channels[..destination.count].copy_from_slice(sample.channels());
            sample.clipped
        } else if destination.space == "lab-d50" {
            channels[..3].copy_from_slice(&pcs::xyz_to_lab(xyz)?);
            false
        } else {
            let (rgb, clipped) = self.named_from_xyz(xyz, &destination.space, false)?;
            channels[..3].copy_from_slice(&rgb);
            clipped
        };
        ensure!(
            channels.iter().all(|v| v.is_finite()),
            "[invalid-input] output transform conversion overflow"
        );
        Ok((channels, clipped))
    }

    /// Preserve extended linear RGB through the shared PCS conversion without
    /// calling the normalized stored-device conversion or preview quantizer.
    pub(crate) fn working_rgba(
        &mut self,
        source: &NativeColor,
        working: &Destination,
    ) -> Result<([f64; 4], bool)> {
        ensure!(working.is_linear_rgb(), "[unsupported-capability] production RGB compositing requires an explicit named linear RGB working space");
        self.validate_destination(source.destination())?;
        let (channels, source_clipped) = if source.destination() == working {
            (source.channels[..3].try_into().unwrap(), false)
        } else {
            let (xyz, clipped) = self.to_xyz(source)?;
            (
                color::apply(&self.named(working.space())?.from_pcs, xyz),
                clipped,
            )
        };
        ensure!(
            channels.iter().all(|v| v.is_finite()),
            "[invalid-input] linear working RGB conversion overflow"
        );
        Ok((
            [channels[0], channels[1], channels[2], source.alpha],
            source_clipped,
        ))
    }

    /// Legacy interpolation produces encoded sRGB, but working conversion must
    /// not allocate native JSON or link reports at every coverage subsample.
    pub(crate) fn srgb_working_rgba(
        &mut self,
        rgba: [f64; 4],
        working: &Destination,
    ) -> Result<[f64; 4]> {
        ensure!(
            working.is_linear_rgb(),
            "[unsupported-capability] working fill requires named linear RGB"
        );
        let srgb = self.named("srgb")?;
        let linear = std::array::from_fn(|i| srgb.space.transfer.decode(rgba[i]));
        let xyz = color::apply(&srgb.to_pcs, linear);
        let channels = color::apply(&self.named(working.space())?.from_pcs, xyz);
        ensure!(
            channels.iter().all(|v| v.is_finite()) && rgba[3].is_finite(),
            "[invalid-input] working fill conversion overflow"
        );
        Ok([channels[0], channels[1], channels[2], rgba[3]])
    }

    pub(crate) fn intent(&self) -> Intent {
        self.intent
    }

    /// Convert to transient signed working RGB with source clipping, gamut and
    /// ICC path reports. Raster sampling uses the allocation-free helper above.
    pub fn working(
        &mut self,
        source: &NativeColor,
        working: &Destination,
    ) -> Result<WorkingEvaluation> {
        let (rgba, source_clipped) = self.working_rgba(source, working)?;
        let source_link = source
            .destination()
            .profile()
            .map(|profile| {
                self.link(Some(profile), None)
                    .map(|link| link.description().clone())
            })
            .transpose()?;
        Ok(WorkingEvaluation {
            working_space: working.clone(),
            rgba,
            source_clipped,
            outside_working_gamut: rgba[..3]
                .iter()
                .any(|v| !(-CHANNEL_ROUNDOFF..=1.0 + CHANNEL_ROUNDOFF).contains(v)),
            source_link,
        })
    }

    /// Convert native paints into an explicit linear RGB working space, then
    /// apply the shared unquantized source-over/blend kernel. The returned
    /// endpoint evaluations disclose source-path clipping, gamut excursions and paths.
    /// Native source channels and alpha remain unchanged.
    pub fn composite(
        &mut self,
        backdrop: &NativeColor,
        source: &NativeColor,
        working: &Destination,
        mode: &str,
    ) -> Result<(WorkingEvaluation, [WorkingEvaluation; 2])> {
        ensure!(
            working.is_linear_rgb(),
            "[unsupported-capability] production RGB compositing requires an explicit named linear RGB working space"
        );
        crate::blend::validate(mode, "linear")?;
        let endpoints = [
            self.working(backdrop, working)?,
            self.working(source, working)?,
        ];
        let composed =
            crate::blend::working_source_over(mode, endpoints[0].rgba, endpoints[1].rgba)?;
        let color = WorkingEvaluation {
            working_space: working.clone(),
            rgba: composed,
            source_clipped: endpoints.iter().any(|v| v.source_clipped),
            outside_working_gamut: composed[..3]
                .iter()
                .any(|v| !(-CHANNEL_ROUNDOFF..=1.0 + CHANNEL_ROUNDOFF).contains(v)),
            source_link: None,
        };
        Ok((color, endpoints))
    }

    /// Convert the endpoints once, then interpolate straight native channels.
    /// Alpha is independent; even zero-alpha endpoints retain their channel
    /// meaning. This is a numeric sampler, not alpha compositing or RGB preview.
    pub fn interpolate(
        &mut self,
        from: &NativeColor,
        to: &NativeColor,
        destination: &Destination,
        position: f64,
    ) -> Result<(NativeColor, [Evaluation; 2])> {
        ensure!(
            position.is_finite() && (0.0..=1.0).contains(&position),
            "[invalid-input] color interpolation position must be finite and in [0,1]"
        );
        let endpoints = [
            self.convert(from, destination)?,
            self.convert(to, destination)?,
        ];
        let mut sample = endpoints[0].color.clone();
        sample.blend(&endpoints[0].color, &endpoints[1].color, position)?;
        Ok((sample, endpoints))
    }
}

/// Read-only native interpolation, using one verified profile set and the same
/// conversions as paint edits. No sample is quantized to preview RGB.
pub fn interpolate(
    raw: &Value,
    document: &Path,
    from: &Value,
    to: &Value,
    destination: &Value,
    position: f64,
    intent: Intent,
) -> Result<Value> {
    ensure!(raw["version"].as_u64() == Some(super::VERSION), "[unsupported-capability] native color interpolation requires an explicitly migrated v8 document");
    ensure!(
        position.is_finite() && (0.0..=1.0).contains(&position),
        "[invalid-input] color interpolation position must be finite and in [0,1]"
    );
    let from = NativeColor::parse(from)?;
    let to = NativeColor::parse(to)?;
    let destination = Destination::parse(destination)?;
    let set = ProfileSet::load(
        raw,
        document,
        from.destination()
            .profile()
            .into_iter()
            .chain(to.destination().profile())
            .chain(destination.profile()),
    )?;
    let mut engine = Evaluator::new(&set, intent)?;
    let (color, endpoints) = engine.interpolate(&from, &to, &destination, position)?;
    let paths: Vec<_> = endpoints
        .iter()
        .map(|endpoint| {
            json!({
                "color":endpoint.color.json(), "clipped":endpoint.clipped,
                "path":endpoint.path, "link":endpoint.link
            })
        })
        .collect();
    Ok(
        json!({"version":1,"scope":"native-channel-interpolation","intent":intent,
        "position":position,"interpolation_space":destination.json(),
        "alpha_interpolation":"straight-separate","source":[from.json(),to.json()],
        "endpoints":paths,"color":color.json(),
        "endpoint_clipped":endpoints.iter().any(|endpoint|endpoint.clipped),
        "profiles":set.bytes.keys().collect::<Vec<_>>(),"mutated":false}),
    )
}

/// Compact read-only entry point, shared by agent sample inspection and future
/// document conversion planners. This function never changes the document.
pub fn evaluate(
    raw: &Value,
    document: &Path,
    value: &Value,
    destination: Option<&Value>,
    intent: Intent,
) -> Result<Value> {
    ensure!(raw["version"].as_u64() == Some(super::VERSION), "[unsupported-capability] native color evaluation requires an explicitly migrated v8 document");
    let source = NativeColor::parse(value)?;
    let destination = destination.map(Destination::parse).transpose()?;
    let set = ProfileSet::load(
        raw,
        document,
        source
            .destination
            .profile()
            .into_iter()
            .chain(destination.as_ref().and_then(Destination::profile)),
    )?;
    let mut engine = Evaluator::new(&set, intent)?;
    let (xyz, clipped) = engine.to_xyz(&source)?;
    let source_link = source
        .destination
        .profile()
        .map(|profile| {
            engine
                .link(Some(profile), None)
                .map(|link| link.description().clone())
        })
        .transpose()?;
    let dependencies: Vec<_> = set.bytes.keys().map(String::as_str).collect();
    let converted = destination
        .as_ref()
        .map(|target| engine.convert(&source, target))
        .transpose()?;
    Ok(
        json!({"version":1,"intent":intent,"source":source.json(),"xyz_d50":xyz,"source_clipped":clipped,"source_link":source_link,"profiles":dependencies,"converted":converted.map(|result|json!({"color":result.color.json(),"clipped":result.clipped,"path":result.path,"link":result.link})),"mutated":false}),
    )
}
