//! Reproducible native font provisioning and bounded fallback diagnostics.
//!
//! The complete database is assembled and validated before `FontSystem` is
//! constructed. This matters because cosmic-text snapshots its monospace and
//! per-script fallback indexes during `FontSystem` construction.

use std::{
    collections::BTreeSet,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use glyphon::{
    FontSystem,
    cosmic_text::{Fallback, PlatformFallback},
    fontdb::{self, Database, FaceInfo, Source, Stretch, Style, Weight},
};
use serde::Serialize;
use unicode_script::Script;

pub const BUNDLED_FAMILY: &str = "JetBrains Mono";
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
pub const MAX_FALLBACK_RECORDS: usize = 64;
pub const MAX_FALLBACK_BYTES: usize = 64 * 1024;
pub const MAX_FALLBACK_FIELD_BYTES: usize = 256;

const REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/jetbrains-mono-v2.304/JetBrainsMono-Regular.ttf");
const BOLD_BYTES: &[u8] =
    include_bytes!("../assets/fonts/jetbrains-mono-v2.304/JetBrainsMono-Bold.ttf");
const ITALIC_BYTES: &[u8] =
    include_bytes!("../assets/fonts/jetbrains-mono-v2.304/JetBrainsMono-Italic.ttf");
const BOLD_ITALIC_BYTES: &[u8] =
    include_bytes!("../assets/fonts/jetbrains-mono-v2.304/JetBrainsMono-BoldItalic.ttf");

/// Generated Braille block (U+2800-U+28FF) at JetBrains Mono metrics
/// (600/1000 em advance). Neither the bundled family nor any stock macOS
/// monospace font covers Braille, so without this face CLI spinner glyphs
/// fell through to Apple Braille, whose 0.692em advance fails cell
/// admission and forces anchored decomposition. Regenerate with
/// `packaging/make-braille-font.py`.
pub const BRAILLE_FALLBACK_FAMILY: &str = "Mandatum Braille";
const BRAILLE_BYTES: &[u8] =
    include_bytes!("../assets/fonts/mandatum-braille/MandatumBraille-Regular.ttf");

static NEXT_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Platform fallback with one override: Braille resolves to the bundled
/// metric-matched face instead of whatever the database scan finds first
/// (Apple Braille on macOS). Script fallback runs before the common list
/// and the whole-database scan, so the override is deterministic.
struct MandatumFallback;

impl Fallback for MandatumFallback {
    fn common_fallback(&self) -> &[&'static str] {
        PlatformFallback.common_fallback()
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
        if script == Script::Braille {
            return &[BRAILLE_FALLBACK_FAMILY];
        }
        PlatformFallback.script_fallback(script, locale)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FontRequest {
    BundledDefault { size: f32 },
    InstalledFamily { family: String, size: f32 },
}

impl Default for FontRequest {
    fn default() -> Self {
        Self::BundledDefault {
            size: DEFAULT_FONT_SIZE,
        }
    }
}

impl FontRequest {
    pub fn installed(family: impl Into<String>, size: f32) -> Self {
        Self::InstalledFamily {
            family: family.into(),
            size,
        }
    }

    pub fn size(&self) -> f32 {
        match self {
            Self::BundledDefault { size } | Self::InstalledFamily { size, .. } => *size,
        }
    }

    pub fn requested_family(&self) -> &str {
        match self {
            Self::BundledDefault { .. } => BUNDLED_FAMILY,
            Self::InstalledFamily { family, .. } => family,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FontProfileSource {
    Bundled,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FontFacesInfo {
    pub regular: String,
    pub bold: String,
    pub italic: String,
    pub bold_italic: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FontInfo {
    pub source: FontProfileSource,
    pub requested_family: String,
    pub size: f32,
    pub faces: FontFacesInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedFontFaces {
    pub regular: fontdb::ID,
    pub bold: fontdb::ID,
    pub italic: fontdb::ID,
    pub bold_italic: fontdb::ID,
}

impl SelectedFontFaces {
    fn contains(self, id: fontdb::ID) -> bool {
        [self.regular, self.bold, self.italic, self.bold_italic].contains(&id)
    }

    /// Select one of the only four primary faces provisioned by the profile.
    ///
    /// Native interface text uses the same closed face set as terminal row
    /// runs; synthetic styles and unprovisioned primary variants are never
    /// introduced by the presentation adapter.
    pub fn for_style(self, bold: bool, italic: bool) -> fontdb::ID {
        match (bold, italic) {
            (false, false) => self.regular,
            (true, false) => self.bold,
            (false, true) => self.italic,
            (true, true) => self.bold_italic,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedFontProfile {
    database: Database,
    locale: String,
    generation: u64,
    selected: SelectedFontFaces,
    info: FontInfo,
}

impl ResolvedFontProfile {
    pub fn resolve(request: FontRequest) -> Result<Self, FontProvisionError> {
        validate_request(&request)?;
        let mut database = Database::new();
        database.load_system_fonts();
        resolve_in_database(database, request)
    }

    pub fn family(&self) -> &str {
        &self.info.requested_family
    }

    pub fn size(&self) -> f32 {
        self.info.size
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn selected_faces(&self) -> SelectedFontFaces {
        self.selected
    }

    pub fn info(&self) -> &FontInfo {
        &self.info
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    /// Construct cosmic-text from the already resolved catalog.
    ///
    /// Cloning this profile and calling this method during device recreation
    /// preserves the selected IDs and cannot rescan into a different primary.
    pub fn create_font_system(&self) -> FontSystem {
        FontSystem::new_with_locale_and_db_and_fallback(
            self.locale.clone(),
            self.database.clone(),
            MandatumFallback,
        )
    }

    /// Clone the already resolved catalog for GPU-device recreation.
    ///
    /// This named seam is used by the renderer so tests can prove that
    /// recreation preserves the catalog generation and selected source IDs.
    pub fn clone_for_device_recreation(&self) -> Self {
        self.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontProvisionError {
    message: String,
}

impl FontProvisionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for FontProvisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FontProvisionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FallbackRecord {
    Face {
        font_id: fontdb::ID,
        family: String,
        postscript_name: String,
        sample: String,
    },
    MissingGlyph {
        sample: String,
    },
}

impl FallbackRecord {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Face {
                    family,
                    postscript_name,
                    sample,
                    ..
                } => family.capacity() + postscript_name.capacity() + sample.capacity(),
                Self::MissingGlyph { sample } => sample.capacity(),
            }
    }
}

#[derive(Clone, Debug)]
pub struct FallbackReport {
    generation: u64,
    records: Box<[FallbackRecord]>,
    retained_bytes: usize,
    dropped: usize,
    limits: FallbackLimits,
}

#[derive(Clone, Copy, Debug)]
struct FallbackLimits {
    records: usize,
    bytes: usize,
    field_bytes: usize,
}

impl FallbackReport {
    pub fn new(generation: u64) -> Self {
        Self::with_limits(
            generation,
            FallbackLimits {
                records: MAX_FALLBACK_RECORDS,
                bytes: MAX_FALLBACK_BYTES,
                field_bytes: MAX_FALLBACK_FIELD_BYTES,
            },
        )
    }

    fn with_limits(generation: u64, limits: FallbackLimits) -> Self {
        Self {
            generation,
            records: Box::default(),
            retained_bytes: 0,
            dropped: 0,
            limits,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn records(&self) -> &[FallbackRecord] {
        &self.records
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.records = Box::default();
        self.retained_bytes = 0;
        self.dropped = 0;
    }

    /// Record a shaped font that is not one of the four selected primary
    /// faces. The row-run adapter owns deciding which shaped glyph/sample to
    /// pass here.
    pub fn observe_face(
        &mut self,
        profile: &ResolvedFontProfile,
        font_id: fontdb::ID,
        sample: &str,
    ) -> bool {
        if self.generation != profile.generation {
            self.reset(profile.generation);
        }
        if profile.selected.contains(font_id) {
            return false;
        }
        if self.records.iter().any(
            |record| matches!(record, FallbackRecord::Face { font_id: seen, .. } if *seen == font_id),
        ) {
            return false;
        }
        let Some(face) = profile.database.face(font_id) else {
            return self.observe_missing_glyph(sample);
        };
        let family = face
            .families
            .first()
            .map(|family| family.0.as_str())
            .unwrap_or("<unnamed>");
        self.insert(FallbackRecord::Face {
            font_id,
            family: truncate_utf8(family, self.limits.field_bytes),
            postscript_name: truncate_utf8(&face.post_script_name, self.limits.field_bytes),
            sample: truncate_utf8(sample, self.limits.field_bytes),
        })
    }

    pub fn observe_missing_glyph(&mut self, sample: &str) -> bool {
        let sample = truncate_utf8(sample, self.limits.field_bytes);
        if self.records.iter().any(
            |record| matches!(record, FallbackRecord::MissingGlyph { sample: seen } if seen == &sample),
        ) {
            return false;
        }
        self.insert(FallbackRecord::MissingGlyph { sample })
    }

    fn insert(&mut self, record: FallbackRecord) -> bool {
        let bytes = record.retained_bytes();
        if self.records.len() >= self.limits.records
            || self.retained_bytes.saturating_add(bytes) > self.limits.bytes
        {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let current = std::mem::take(&mut self.records);
        let mut records = current.into_vec();
        records.push(record);
        self.records = records.into_boxed_slice();
        self.retained_bytes += bytes;
        true
    }
}

/// Families the appearance overlay's font row can offer: the bundled
/// default first, then every installed family whose catalog metadata shows
/// the four exact monospaced static-stretch style faces. This is a cheap
/// prefilter — it reads face records, never font files — so a candidate can
/// still fail strict resolution (a variable font, say); the apply path
/// degrades that case to a status warning.
pub fn cycle_candidate_families(database: &Database) -> Vec<String> {
    let mut candidates = std::collections::BTreeMap::<String, [bool; 4]>::new();
    for face in database.faces() {
        let Some((family, _)) = face.families.first() else {
            continue;
        };
        if family == BUNDLED_FAMILY || family == BRAILLE_FALLBACK_FAMILY {
            continue;
        }
        if !face.monospaced || face.stretch != Stretch::Normal {
            continue;
        }
        let slot = match (face.weight, face.style) {
            (Weight::NORMAL, Style::Normal) => 0,
            (Weight::BOLD, Style::Normal) => 1,
            (Weight::NORMAL, Style::Italic) => 2,
            (Weight::BOLD, Style::Italic) => 3,
            _ => continue,
        };
        candidates.entry(family.clone()).or_default()[slot] = true;
    }
    let mut families = vec![BUNDLED_FAMILY.to_owned()];
    families.extend(
        candidates
            .into_iter()
            .filter(|(_, slots)| slots.iter().all(|present| *present))
            .map(|(family, _)| family),
    );
    families
}

fn validate_request(request: &FontRequest) -> Result<(), FontProvisionError> {
    let family = request.requested_family().trim();
    if family.is_empty() || family.len() > 128 || family.chars().any(char::is_control) {
        return Err(FontProvisionError::new(
            "font family must be 1..=128 visible characters",
        ));
    }
    if matches!(request, FontRequest::InstalledFamily { .. }) && is_generic_family(family) {
        return Err(FontProvisionError::new(format!(
            "generic font family is not an installed-family override: {family}"
        )));
    }
    let size = request.size();
    if !size.is_finite() || !(6.0..=72.0).contains(&size) {
        return Err(FontProvisionError::new(
            "font size must be finite and between 6 and 72 points",
        ));
    }
    Ok(())
}

fn is_generic_family(family: &str) -> bool {
    ["serif", "sans-serif", "cursive", "fantasy", "monospace"]
        .iter()
        .any(|generic| family.eq_ignore_ascii_case(generic))
}

fn resolve_in_database(
    mut database: Database,
    request: FontRequest,
) -> Result<ResolvedFontProfile, FontProvisionError> {
    let size = request.size();
    let (source, family, selected) = match request {
        FontRequest::BundledDefault { .. } => {
            let duplicate_ids = database
                .faces()
                .filter(|face| face_has_family(face, BUNDLED_FAMILY, false))
                .map(|face| face.id)
                .collect::<Vec<_>>();
            for id in duplicate_ids {
                database.remove_face(id);
            }
            let bundled_ids = [
                load_one_bundled_face(&mut database, REGULAR_BYTES, "Regular")?,
                load_one_bundled_face(&mut database, BOLD_BYTES, "Bold")?,
                load_one_bundled_face(&mut database, ITALIC_BYTES, "Italic")?,
                load_one_bundled_face(&mut database, BOLD_ITALIC_BYTES, "BoldItalic")?,
            ];
            (
                FontProfileSource::Bundled,
                BUNDLED_FAMILY.to_owned(),
                select_exact_faces(&database, BUNDLED_FAMILY, Some(&bundled_ids))?,
            )
        }
        FontRequest::InstalledFamily { family, .. } => {
            let family = family.trim().to_owned();
            let selected = select_exact_faces(&database, &family, None)?;
            (FontProfileSource::System, family, selected)
        }
    };
    load_braille_fallback_face(&mut database)?;
    database.set_monospace_family(family.clone());
    let info = FontInfo {
        source,
        requested_family: family,
        size,
        faces: FontFacesInfo {
            regular: postscript_name(&database, selected.regular)?,
            bold: postscript_name(&database, selected.bold)?,
            italic: postscript_name(&database, selected.italic)?,
            bold_italic: postscript_name(&database, selected.bold_italic)?,
        },
    };
    Ok(ResolvedFontProfile {
        database,
        locale: sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned()),
        generation: NEXT_CATALOG_GENERATION.fetch_add(1, Ordering::Relaxed),
        selected,
        info,
    })
}

fn load_one_bundled_face(
    database: &mut Database,
    bytes: &[u8],
    label: &str,
) -> Result<fontdb::ID, FontProvisionError> {
    let ids = database.load_font_source(Source::Binary(Arc::new(bytes.to_vec())));
    if ids.len() != 1 {
        return Err(FontProvisionError::new(format!(
            "bundled JetBrains Mono {label} contained {} faces, expected one",
            ids.len()
        )));
    }
    Ok(ids[0])
}

/// Load the bundled Braille face for every profile, whichever primary the
/// request selected. Any same-named system face is removed first so the
/// resolved catalog is deterministic.
fn load_braille_fallback_face(database: &mut Database) -> Result<(), FontProvisionError> {
    let duplicate_ids = database
        .faces()
        .filter(|face| face_has_family(face, BRAILLE_FALLBACK_FAMILY, false))
        .map(|face| face.id)
        .collect::<Vec<_>>();
    for id in duplicate_ids {
        database.remove_face(id);
    }
    let ids = database.load_font_source(Source::Binary(Arc::new(BRAILLE_BYTES.to_vec())));
    if ids.len() != 1 {
        return Err(FontProvisionError::new(format!(
            "bundled Mandatum Braille contained {} faces, expected one",
            ids.len()
        )));
    }
    Ok(())
}

fn select_exact_faces(
    database: &Database,
    family: &str,
    allowed_ids: Option<&[fontdb::ID]>,
) -> Result<SelectedFontFaces, FontProvisionError> {
    let regular = select_exact_face(
        database,
        family,
        allowed_ids,
        Weight::NORMAL,
        Style::Normal,
        "Regular",
    )?;
    let bold = select_exact_face(
        database,
        family,
        allowed_ids,
        Weight::BOLD,
        Style::Normal,
        "Bold",
    )?;
    let italic = select_exact_face(
        database,
        family,
        allowed_ids,
        Weight::NORMAL,
        Style::Italic,
        "Italic",
    )?;
    let bold_italic = select_exact_face(
        database,
        family,
        allowed_ids,
        Weight::BOLD,
        Style::Italic,
        "BoldItalic",
    )?;
    let selected = SelectedFontFaces {
        regular,
        bold,
        italic,
        bold_italic,
    };
    let unique = [regular, bold, italic, bold_italic]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if unique.len() != 4 {
        return Err(FontProvisionError::new(format!(
            "font family {family:?} does not provide four distinct static style faces"
        )));
    }
    Ok(selected)
}

fn select_exact_face(
    database: &Database,
    family: &str,
    allowed_ids: Option<&[fontdb::ID]>,
    weight: Weight,
    style: Style,
    label: &str,
) -> Result<fontdb::ID, FontProvisionError> {
    let candidates = database
        .faces()
        .filter(|face| allowed_ids.is_none_or(|ids| ids.contains(&face.id)))
        .filter(|face| face_has_family(face, family, true))
        .filter(|face| {
            face.monospaced
                && face.weight == weight
                && face.style == style
                && face.stretch == Stretch::Normal
                && !face_is_variable(database, face.id)
        })
        .map(|face| face.id)
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [id] => Ok(*id),
        [] => Err(FontProvisionError::new(format!(
            "font family {family:?} is missing an exact monospaced {label} face"
        ))),
        _ => Err(FontProvisionError::new(format!(
            "font family {family:?} has ambiguous exact {label} faces"
        ))),
    }
}

fn face_is_variable(database: &Database, id: fontdb::ID) -> bool {
    database
        .with_face_data(id, |data, index| {
            ttf_parser::Face::parse(data, index)
                .map(|face| face.is_variable())
                .unwrap_or(true)
        })
        .unwrap_or(true)
}

fn face_has_family(face: &FaceInfo, family: &str, exact_case: bool) -> bool {
    face.families.iter().any(|candidate| {
        if exact_case {
            candidate.0 == family
        } else {
            candidate.0.eq_ignore_ascii_case(family)
        }
    })
}

fn postscript_name(database: &Database, id: fontdb::ID) -> Result<String, FontProvisionError> {
    database
        .face(id)
        .map(|face| face.post_script_name.clone())
        .ok_or_else(|| FontProvisionError::new("selected font face disappeared from the catalog"))
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut truncated = if value.len() <= max_bytes {
        value.to_owned()
    } else {
        let mut end = max_bytes;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value[..end].to_owned()
    };
    if truncated.capacity() > truncated.len() {
        truncated.shrink_to_fit();
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARIABLE_BYTES: &[u8] =
        include_bytes!("../tests/fixtures/JetBrainsMono-variable-wght.ttf");

    fn bundled_database() -> Database {
        let mut database = Database::new();
        for bytes in [REGULAR_BYTES, BOLD_BYTES, ITALIC_BYTES, BOLD_ITALIC_BYTES] {
            database.load_font_source(Source::Binary(Arc::new(bytes.to_vec())));
        }
        database
    }

    fn renamed_database(family: &str) -> Database {
        let source = bundled_database();
        let mut database = Database::new();
        for face in source.faces() {
            let mut face = face.clone();
            face.id = fontdb::ID::dummy();
            for candidate in &mut face.families {
                candidate.0 = family.to_owned();
            }
            face.post_script_name = format!("{family}-{}", face.post_script_name);
            database.push_face_info(face);
        }
        database
    }

    #[test]
    fn bundled_profile_removes_same_family_duplicates_and_selects_owned_static_faces() {
        let database = renamed_database(BUNDLED_FAMILY);
        let duplicate_ids = database.faces().map(|face| face.id).collect::<Vec<_>>();
        let profile = resolve_in_database(database.clone(), FontRequest::default())
            .expect("bundled profile resolves");

        assert_eq!(profile.info.source, FontProfileSource::Bundled);
        assert_eq!(profile.info.requested_family, BUNDLED_FAMILY);
        assert_eq!(profile.info.size, DEFAULT_FONT_SIZE);
        assert_eq!(
            profile.info.faces,
            FontFacesInfo {
                regular: "JetBrainsMono-Regular".to_owned(),
                bold: "JetBrainsMono-Bold".to_owned(),
                italic: "JetBrainsMono-Italic".to_owned(),
                bold_italic: "JetBrainsMono-BoldItalic".to_owned(),
            }
        );
        for id in duplicate_ids {
            assert!(profile.database.face(id).is_none());
        }
        for id in [
            profile.selected.regular,
            profile.selected.bold,
            profile.selected.italic,
            profile.selected.bold_italic,
        ] {
            assert!(matches!(
                profile.database.face(id).map(|face| &face.source),
                Some(Source::Binary(_))
            ));
        }
        let cloned = profile.clone();
        assert_eq!(cloned.generation, profile.generation);
        assert_eq!(cloned.selected, profile.selected);
        let font_system = cloned.create_font_system();
        assert_eq!(
            font_system
                .db()
                .face(profile.selected.regular)
                .unwrap()
                .post_script_name,
            "JetBrainsMono-Regular"
        );
    }

    #[test]
    fn installed_family_requires_exact_complete_static_monospace_faces() {
        let database = renamed_database("Fixture Mono");
        let profile = resolve_in_database(
            database.clone(),
            FontRequest::installed("Fixture Mono", 14.0),
        )
        .expect("complete exact family resolves");
        assert_eq!(profile.info.source, FontProfileSource::System);

        let mut missing_bold = Database::new();
        for face in database.faces().filter(|face| face.weight != Weight::BOLD) {
            missing_bold.push_face_info(face.clone());
        }
        let error = resolve_in_database(missing_bold, FontRequest::installed("Fixture Mono", 14.0))
            .unwrap_err();
        assert!(error.to_string().contains("Bold"));

        let mut proportional = Database::new();
        for face in database.faces() {
            let mut face = face.clone();
            face.id = fontdb::ID::dummy();
            face.monospaced = false;
            proportional.push_face_info(face);
        }
        let error = resolve_in_database(proportional, FontRequest::installed("Fixture Mono", 14.0))
            .unwrap_err();
        assert!(error.to_string().contains("monospaced"));
    }

    #[test]
    fn installed_family_rejects_generic_missing_and_inexact_names() {
        for generic in ["monospace", "SERIF", "sans-serif", "cursive", "fantasy"] {
            assert!(validate_request(&FontRequest::installed(generic, 13.0)).is_err());
        }
        let database = renamed_database("Fixture Mono");
        assert!(
            resolve_in_database(
                database.clone(),
                FontRequest::installed("Missing Mono", 13.0)
            )
            .unwrap_err()
            .to_string()
            .contains("missing")
        );
        assert!(
            resolve_in_database(database, FontRequest::installed("fixture mono", 13.0))
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
    }

    #[test]
    fn installed_family_rejects_real_variable_faces_even_with_exact_style_metadata() {
        let mut parsed = Database::new();
        parsed.load_font_source(Source::Binary(Arc::new(VARIABLE_BYTES.to_vec())));
        let source_face = parsed
            .faces()
            .next()
            .expect("variable fixture face")
            .clone();
        assert!(face_is_variable(&parsed, source_face.id));

        let mut database = Database::new();
        for (weight, style, label) in [
            (Weight::NORMAL, Style::Normal, "Regular"),
            (Weight::BOLD, Style::Normal, "Bold"),
            (Weight::NORMAL, Style::Italic, "Italic"),
            (Weight::BOLD, Style::Italic, "BoldItalic"),
        ] {
            let mut face = source_face.clone();
            face.id = fontdb::ID::dummy();
            for family in &mut face.families {
                family.0 = "Variable Fixture".to_owned();
            }
            face.post_script_name = format!("VariableFixture-{label}");
            face.weight = weight;
            face.style = style;
            face.monospaced = true;
            database.push_face_info(face);
        }

        let error = resolve_in_database(database, FontRequest::installed("Variable Fixture", 13.0))
            .unwrap_err();
        assert!(
            error.to_string().contains("missing an exact monospaced"),
            "{error}"
        );
    }

    #[test]
    fn fallback_report_deduplicates_truncates_bounds_and_resets() {
        let profile =
            resolve_in_database(Database::new(), FontRequest::default()).expect("bundle resolves");
        let one_record_budget = std::mem::size_of::<FallbackRecord>() + 8;
        let mut report = FallbackReport::with_limits(
            profile.generation,
            FallbackLimits {
                records: 2,
                bytes: one_record_budget,
                field_bytes: 8,
            },
        );
        assert!(report.observe_missing_glyph("ééééé"));
        assert!(!report.observe_missing_glyph("ééééé"));
        assert_eq!(
            report.records(),
            &[FallbackRecord::MissingGlyph {
                sample: "éééé".to_owned()
            }]
        );
        assert!(!report.observe_missing_glyph("abcdef"));
        assert_eq!(report.records().len(), 1);
        assert_eq!(report.dropped(), 1);
        assert_eq!(
            report.retained_bytes(),
            report.records()[0].retained_bytes()
        );
        assert!(report.retained_bytes() <= one_record_budget);

        report.reset(profile.generation + 1);
        assert!(report.records().is_empty());
        assert_eq!(report.retained_bytes(), 0);
        assert_eq!(report.dropped(), 0);
    }

    #[test]
    fn fallback_faces_deduplicate_by_face_identity_not_sample() {
        let mut profile =
            resolve_in_database(Database::new(), FontRequest::default()).expect("bundle resolves");
        let mut fallback = profile
            .database
            .face(profile.selected.regular)
            .expect("regular face")
            .clone();
        fallback.id = fontdb::ID::dummy();
        fallback.families[0].0 = "Fallback Fixture".to_owned();
        fallback.post_script_name = "FallbackFixture-Regular".to_owned();
        let fallback_id = profile.database.push_face_info(fallback);
        let mut report = FallbackReport::new(profile.generation);

        assert!(report.observe_face(&profile, fallback_id, "界"));
        assert!(!report.observe_face(&profile, fallback_id, "語"));
        assert_eq!(report.records().len(), 1);
        assert!(matches!(
            &report.records()[0],
            FallbackRecord::Face {
                font_id,
                sample,
                ..
            } if *font_id == fallback_id && sample == "界"
        ));
    }

    #[test]
    fn device_recreation_clone_preserves_catalog_generation_and_all_selected_sources() {
        let profile =
            resolve_in_database(Database::new(), FontRequest::default()).expect("bundle resolves");
        let replacement = profile.clone_for_device_recreation();

        assert_eq!(replacement.generation(), profile.generation());
        assert_eq!(replacement.selected_faces(), profile.selected_faces());
        assert_eq!(replacement.info(), profile.info());
        for id in [
            profile.selected.regular,
            profile.selected.bold,
            profile.selected.italic,
            profile.selected.bold_italic,
        ] {
            assert_eq!(
                replacement
                    .database()
                    .face(id)
                    .expect("replacement face")
                    .post_script_name,
                profile
                    .database()
                    .face(id)
                    .expect("original face")
                    .post_script_name
            );
        }
    }

    #[test]
    fn ui_style_selection_is_closed_over_the_four_provisioned_faces() {
        let profile =
            resolve_in_database(Database::new(), FontRequest::default()).expect("bundle resolves");
        let selected = profile.selected_faces();

        assert_eq!(selected.for_style(false, false), selected.regular);
        assert_eq!(selected.for_style(true, false), selected.bold);
        assert_eq!(selected.for_style(false, true), selected.italic);
        assert_eq!(selected.for_style(true, true), selected.bold_italic);
    }

    #[test]
    fn cycle_candidates_lead_with_the_bundled_family_and_require_four_styles() {
        // An empty catalog offers only the bundled default.
        assert_eq!(
            cycle_candidate_families(&Database::new()),
            vec![BUNDLED_FAMILY.to_owned()]
        );

        // A complete renamed monospace family qualifies; the bundled and
        // Braille families are never listed as installed candidates, and a
        // family missing a style face is excluded.
        let mut database = renamed_database("Candidate Mono");
        for face in renamed_database(BUNDLED_FAMILY).faces() {
            let mut face = face.clone();
            face.id = fontdb::ID::dummy();
            database.push_face_info(face);
        }
        for face in renamed_database("Incomplete Mono").faces().take(3) {
            let mut face = face.clone();
            face.id = fontdb::ID::dummy();
            database.push_face_info(face);
        }
        assert_eq!(
            cycle_candidate_families(&database),
            vec![BUNDLED_FAMILY.to_owned(), "Candidate Mono".to_owned()]
        );
    }
}
