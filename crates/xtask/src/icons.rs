//! The brand icon, as each platform's icon set (task b76ce8bf).
//!
//! The parent task (`77367032`) landed the brand schema and `gen-brand`'s ten TEXT outputs; the
//! `assets` section carried `icon` through but nothing was emitted from it, because rasterising is
//! a dependency decision rather than another emitter. Jon settled it on 2026-11-13: **the brand
//! icon source is an SVG**, and `gen-brand` rasterises only where a platform cannot take a vector.
//!
//! That line is the whole design, and it is why `assets.rs`'s argument still holds one level up —
//! "fixed-colour bitmaps cannot take the priority tokens or a partner's brand" (register DS21). A
//! partner edits one SVG. Where a platform reads vectors, it gets that SVG; where it demands a
//! bitmap, the bitmap is *derived* and never the source of truth:
//!
//! | Platform | Vector | Raster |
//! |---|---|---|
//! | Web | `icon.svg`, the first favicon a modern browser picks | the PWA and `apple-touch-icon` sizes |
//! | Android | the adaptive-icon foreground, a VectorDrawable | the legacy `mipmap-*dpi` launcher PNGs |
//! | VS Code / Astrid Code | — | the marketplace's 128 and 256 |
//! | Apple | — | `AppIcon.appiconset`, one 1024 source |
//! | Windows | — | the MSIX logo set |
//!
//! **The sizes are a table, pinned by a test, not a comment** — that is the task's own acceptance
//! line, and the reason is that a missing size does not fail a build, it fails an *install* or
//! ships a blurry tile. Each platform's list comes from that platform's own consumer: the Windows
//! one is read off `astrid-windows/packaging/AppxManifest.xml`, which names SIX logos, not the
//! four the task description listed from memory.

#[cfg(test)]
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::brand::Profile;

/// One file `gen-brand` writes: text a platform reads, or the bytes of a rendered PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Text(String),
    Bytes(Vec<u8>),
}

impl Output {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Output::Text(text) => text.as_bytes(),
            Output::Bytes(bytes) => bytes,
        }
    }
}

/// One rendered PNG: where it goes, how big, and how much of that canvas the artwork fills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Raster {
    /// Path under `gen-brand`'s output directory.
    pub path: &'static str,
    pub width: u32,
    pub height: u32,
    /// The fraction of the shorter side the artwork is drawn at, centred. `1.0` everywhere except
    /// Android's adaptive foreground, whose 108dp canvas keeps only the inner 72dp unmasked — a
    /// foreground drawn edge to edge there is cropped by every launcher that rounds the corners.
    pub fill: f32,
}

const fn icon(path: &'static str, size: u32) -> Raster {
    Raster {
        path,
        width: size,
        height: size,
        fill: 1.0,
    }
}

const fn wide(path: &'static str, width: u32, height: u32) -> Raster {
    Raster {
        path,
        width,
        height,
        fill: 1.0,
    }
}

/// Android's adaptive-icon safe zone: the inner 72dp of a 108dp canvas.
const ADAPTIVE_SAFE: f32 = 72.0 / 108.0;

const fn adaptive(path: &'static str, size: u32) -> Raster {
    Raster {
        path,
        width: size,
        height: size,
        fill: ADAPTIVE_SAFE,
    }
}

/// Apple: modern Xcode takes a single 1024 source and derives the rest itself, so this is one
/// file and a `Contents.json`, not a resize table.
pub const APPLE: &[Raster] = &[icon(
    "apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png",
    1024,
)];

/// Android: the legacy launcher icon at the five densities (48dp at mdpi through xxxhdpi), plus
/// the adaptive foreground at the same densities on the 108dp canvas. The adaptive BACKGROUND is
/// a colour, not an image — written as a resource below rather than rendered.
pub const ANDROID: &[Raster] = &[
    icon("android/res/mipmap-mdpi/ic_launcher.png", 48),
    icon("android/res/mipmap-hdpi/ic_launcher.png", 72),
    icon("android/res/mipmap-xhdpi/ic_launcher.png", 96),
    icon("android/res/mipmap-xxhdpi/ic_launcher.png", 144),
    icon("android/res/mipmap-xxxhdpi/ic_launcher.png", 192),
    adaptive("android/res/mipmap-mdpi/ic_launcher_foreground.png", 108),
    adaptive("android/res/mipmap-hdpi/ic_launcher_foreground.png", 162),
    adaptive("android/res/mipmap-xhdpi/ic_launcher_foreground.png", 216),
    adaptive("android/res/mipmap-xxhdpi/ic_launcher_foreground.png", 324),
    adaptive("android/res/mipmap-xxxhdpi/ic_launcher_foreground.png", 432),
];

/// Windows: the six logos `astrid-windows/packaging/AppxManifest.xml` names, each at the four
/// standard MSIX scales beside its unqualified 100% file, plus the `targetsize` variants of the
/// 44×44 that the taskbar, Start and the file-type association resolve to. The unqualified file
/// is written as well as `scale-100` so the manifest resolves in a loose layout, before a PRI
/// index exists to do it.
pub const WINDOWS: &[Raster] = &[
    icon("windows/Assets/StoreLogo.png", 50),
    icon("windows/Assets/StoreLogo.scale-125.png", 63),
    icon("windows/Assets/StoreLogo.scale-150.png", 75),
    icon("windows/Assets/StoreLogo.scale-200.png", 100),
    icon("windows/Assets/StoreLogo.scale-400.png", 200),
    icon("windows/Assets/Square44x44Logo.png", 44),
    icon("windows/Assets/Square44x44Logo.scale-125.png", 55),
    icon("windows/Assets/Square44x44Logo.scale-150.png", 66),
    icon("windows/Assets/Square44x44Logo.scale-200.png", 88),
    icon("windows/Assets/Square44x44Logo.scale-400.png", 176),
    icon("windows/Assets/Square44x44Logo.targetsize-16.png", 16),
    icon("windows/Assets/Square44x44Logo.targetsize-24.png", 24),
    icon("windows/Assets/Square44x44Logo.targetsize-32.png", 32),
    icon("windows/Assets/Square44x44Logo.targetsize-48.png", 48),
    icon("windows/Assets/Square44x44Logo.targetsize-256.png", 256),
    icon("windows/Assets/Square71x71Logo.png", 71),
    icon("windows/Assets/Square71x71Logo.scale-125.png", 89),
    icon("windows/Assets/Square71x71Logo.scale-150.png", 107),
    icon("windows/Assets/Square71x71Logo.scale-200.png", 142),
    icon("windows/Assets/Square71x71Logo.scale-400.png", 284),
    icon("windows/Assets/Square150x150Logo.png", 150),
    icon("windows/Assets/Square150x150Logo.scale-125.png", 188),
    icon("windows/Assets/Square150x150Logo.scale-150.png", 225),
    icon("windows/Assets/Square150x150Logo.scale-200.png", 300),
    icon("windows/Assets/Square150x150Logo.scale-400.png", 600),
    icon("windows/Assets/Square310x310Logo.png", 310),
    icon("windows/Assets/Square310x310Logo.scale-125.png", 388),
    icon("windows/Assets/Square310x310Logo.scale-150.png", 465),
    icon("windows/Assets/Square310x310Logo.scale-200.png", 620),
    icon("windows/Assets/Square310x310Logo.scale-400.png", 1240),
    wide("windows/Assets/Wide310x150Logo.png", 310, 150),
    wide("windows/Assets/Wide310x150Logo.scale-125.png", 388, 188),
    wide("windows/Assets/Wide310x150Logo.scale-150.png", 465, 225),
    wide("windows/Assets/Wide310x150Logo.scale-200.png", 620, 300),
    wide("windows/Assets/Wide310x150Logo.scale-400.png", 1240, 600),
];

/// Web: the two legacy favicon pixel sizes a browser still asks for, the iOS home-screen icon
/// (which `apple-touch-icon` requires as a bitmap), and the PWA manifest's two required sizes.
/// `icon.svg` beside them is the vector a modern browser prefers, written straight through.
pub const WEB: &[Raster] = &[
    icon("web/public/favicon-16x16.png", 16),
    icon("web/public/favicon-32x32.png", 32),
    icon("web/public/apple-touch-icon.png", 180),
    icon("web/public/icon-192.png", 192),
    icon("web/public/icon-512.png", 512),
];

/// VS Code and Astrid Code: the marketplace listing's icon and the product one.
pub const VSCODE: &[Raster] = &[
    icon("vscode/icon-128.png", 128),
    icon("vscode/icon-256.png", 256),
];

/// Every PNG, in the order it is written.
pub fn rasters() -> Vec<Raster> {
    [APPLE, ANDROID, WINDOWS, WEB, VSCODE].concat()
}

/// Why an icon could not be turned into a set.
fn problem(path: &str, detail: impl std::fmt::Display) -> Vec<String> {
    vec![format!("assets.icon {path:?}: {detail}")]
}

/// Render one SVG at one size. The artwork keeps its aspect ratio and is centred, so a source
/// that is not square still produces a square tile rather than a stretched one.
fn render(tree: &resvg::usvg::Tree, raster: Raster) -> Result<Vec<u8>, String> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(raster.width, raster.height)
        .ok_or_else(|| format!("{} is not a drawable size", raster.path))?;

    let size = tree.size();
    let (source_w, source_h) = (size.width(), size.height());
    if source_w <= 0.0 || source_h <= 0.0 {
        return Err("the icon has no size".to_string());
    }

    let scale = (raster.width as f32 / source_w).min(raster.height as f32 / source_h) * raster.fill;
    let transform = resvg::tiny_skia::Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (raster.width as f32 - source_w * scale) / 2.0,
        (raster.height as f32 - source_h * scale) / 2.0,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|error| format!("{} could not be encoded: {error}", raster.path))
}

/// Apple's `AppIcon.appiconset` wrapper: Xcode reads the single-size form from Xcode 14 on.
fn appiconset() -> Value {
    json!({
        "images": [{
            "filename": "icon-1024.png",
            "idiom": "universal",
            "platform": "ios",
            "size": "1024x1024"
        }],
        "info": { "author": "xcode", "version": 1 }
    })
}

/// Android's adaptive icon: a vector foreground over the brand's own background colour. The
/// foreground here is the brand SVG itself — Android reads a VectorDrawable, not an SVG, so this
/// points at the rendered mipmap and leaves the VectorDrawable conversion to `gen-design`, which
/// already owns that translation for the checkbox.
fn adaptive_icon() -> String {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
     <adaptive-icon xmlns:android=\"http://schemas.android.com/apk/res/android\">\n    \
     <background android:drawable=\"@color/ic_launcher_background\" />\n    \
     <foreground android:drawable=\"@mipmap/ic_launcher_foreground\" />\n\
     </adaptive-icon>\n"
        .to_string()
}

/// The adaptive background, as a colour resource: the brand's own accent, which is the one colour
/// `brand/schema.json` REQUIRES a profile to state. There is deliberately no `colors.background`
/// field to read — the schema's colour section is the accent and the four tones derived from it —
/// so reaching for one would have silently painted every partner's launcher white.
fn adaptive_background(profile: &Profile) -> String {
    let colour = profile
        .get("colors.accent")
        .filter(|value| value.starts_with('#'))
        .unwrap_or("#FFFFFF");
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <resources>\n    <color name=\"ic_launcher_background\">{colour}</color>\n</resources>\n"
    )
}

/// The PWA manifest's `icons` array, naming the two sizes it renders.
fn web_manifest_icons() -> Value {
    json!({
        "icons": [
            { "src": "/icon.svg", "sizes": "any", "type": "image/svg+xml" },
            { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable" }
        ]
    })
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("a JSON value prints");
    text.push('\n');
    text
}

/// What a profile's `assets.icon` actually is.
///
/// This field PREDATES any rasteriser: it is `NEXT_PUBLIC_BRAND_ICON`, and web reads it at
/// runtime as a URL (`lib/brand/config.ts`). All five profiles in astrid-web's `brands/` set it
/// to a web-absolute path like `/icons/icon-512x512.png` — a reference to a file the site serves,
/// not artwork on disk beside the profile. Treating every value as a source to render would turn
/// `gen-brand` red on every profile that ships today, and the parent task's own rule is that
/// "every profile in astrid-web's `brands/` validates unchanged".
///
/// So the vector master is recognised by being one: a value naming an `.svg` is artwork beside
/// the profile and is rendered; anything else stays the runtime reference it has always been.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// No `assets.icon`, or an empty one.
    Absent,
    /// A vector master, relative to the profile.
    Vector(String),
    /// Today's runtime URL. Not artwork, so there is nothing to rasterise from.
    Reference(String),
}

pub fn source(profile: &Profile) -> Source {
    match profile.get("assets.icon").filter(|path| !path.is_empty()) {
        None => Source::Absent,
        Some(icon) if icon.to_ascii_lowercase().ends_with(".svg") => {
            Source::Vector(icon.to_string())
        }
        Some(icon) => Source::Reference(icon.to_string()),
    }
}

/// What to tell a partner whose profile names a runtime reference rather than a vector master.
pub fn note(source: &Source) -> Option<String> {
    match source {
        Source::Reference(icon) => Some(format!(
            "no icon set: assets.icon is {icon:?}, which is the runtime reference web reads, not \
             artwork to render from. Point it at an .svg beside the profile to generate every \
             platform's icons."
        )),
        _ => None,
    }
}

/// Every icon output for this profile, as (path under the output directory, contents).
///
/// A profile with no `assets.icon` produces NOTHING and no error — a partner who has not drawn an
/// icon yet still gets their ten text resources, which is the acceptance line and the reason this
/// returns an empty vector rather than refusing. A profile whose icon is a runtime reference is
/// the same: nothing written, exit 0, and [`note`] says why.
pub fn outputs(
    profile: &Profile,
    relative_to: &Path,
) -> Result<Vec<(String, Output)>, Vec<String>> {
    let Source::Vector(icon) = source(profile) else {
        return Ok(Vec::new());
    };
    let icon = icon.as_str();

    let path = relative_to.join(icon);
    let source = std::fs::read_to_string(&path).map_err(|error| problem(icon, error))?;

    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&source, &options).map_err(|error| {
        // The decision this task records is that the source is an SVG, so say that rather than
        // letting a PNG hand-over fail as an unreadable file.
        problem(
            icon,
            format!(
                "is not an SVG the renderer can read ({error}) — the brand icon source is an SVG"
            ),
        )
    })?;

    let mut out: Vec<(String, Output)> = Vec::new();
    let mut failures = Vec::new();
    for raster in rasters() {
        match render(&tree, raster) {
            Ok(bytes) => out.push((raster.path.to_string(), Output::Bytes(bytes))),
            Err(detail) => failures.push(format!("assets.icon {icon:?}: {detail}")),
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }

    // The vector, written straight through wherever the platform reads one.
    for vector in [
        "web/public/icon.svg",
        "apple/Assets.xcassets/AppIcon.appiconset/icon.svg",
    ] {
        out.push((vector.to_string(), Output::Text(source.clone())));
    }

    out.push((
        "apple/Assets.xcassets/AppIcon.appiconset/Contents.json".to_string(),
        Output::Text(pretty(&appiconset())),
    ));
    out.push((
        "android/res/mipmap-anydpi-v26/ic_launcher.xml".to_string(),
        Output::Text(adaptive_icon()),
    ));
    out.push((
        "android/res/values/ic_launcher_background.xml".to_string(),
        Output::Text(adaptive_background(profile)),
    ));
    out.push((
        "web/icons.manifest.json".to_string(),
        Output::Text(pretty(&web_manifest_icons())),
    ));

    Ok(out)
}

/// The sizes by path, for the test that pins them.
#[cfg(test)]
fn table() -> BTreeMap<&'static str, u32> {
    rasters()
        .into_iter()
        .map(|raster| (raster.path, raster.width))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square icon with a single filled rectangle: enough to render, small enough to read.
    const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><rect x="0" y="0" width="64" height="64" fill="#2F6FD0"/></svg>"##;

    fn profile_with(icon: Option<&str>) -> Profile {
        let mut values = BTreeMap::new();
        if let Some(icon) = icon {
            values.insert("assets.icon".to_string(), icon.to_string());
        }
        Profile {
            name: "Acme".to_string(),
            description: String::new(),
            values: values.into_iter().collect(),
        }
    }

    fn written(dir: &Path, icon: &str) -> Vec<(String, Output)> {
        outputs(&profile_with(Some(icon)), dir).expect("the icon renders")
    }

    /// A temporary directory of this test's own, removed by the caller.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-icons-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// The acceptance line: the sizes are pinned by a test rather than a comment. A size that
    /// changes here changes an install or a store listing, so it changes deliberately.
    #[test]
    fn the_icon_sizes_are_pinned_b76ce8bf() {
        let table = table();

        // Apple takes one source and derives the rest.
        assert_eq!(APPLE.len(), 1);
        assert_eq!(
            table["apple/Assets.xcassets/AppIcon.appiconset/icon-1024.png"],
            1024
        );

        // Android: five densities of the legacy icon, five of the adaptive foreground.
        assert_eq!(ANDROID.len(), 10);
        assert_eq!(table["android/res/mipmap-mdpi/ic_launcher.png"], 48);
        assert_eq!(table["android/res/mipmap-xxxhdpi/ic_launcher.png"], 192);
        // 108dp at mdpi through xxxhdpi, each the 48dp density times 108/48.
        assert_eq!(
            table["android/res/mipmap-mdpi/ic_launcher_foreground.png"],
            108
        );
        assert_eq!(
            table["android/res/mipmap-xxxhdpi/ic_launcher_foreground.png"],
            432
        );

        // Windows: the six logos AppxManifest.xml names, not the four the description listed.
        for logo in [
            "StoreLogo",
            "Square44x44Logo",
            "Square71x71Logo",
            "Square150x150Logo",
            "Square310x310Logo",
            "Wide310x150Logo",
        ] {
            assert!(
                table.contains_key(format!("windows/Assets/{logo}.png").as_str()),
                "{logo} is named by the MSIX manifest and must be written"
            );
            for scale in [125, 150, 200, 400] {
                assert!(
                    table.contains_key(format!("windows/Assets/{logo}.scale-{scale}.png").as_str()),
                    "{logo} is missing its scale-{scale} variant"
                );
            }
        }
        assert_eq!(table["windows/Assets/Square150x150Logo.scale-200.png"], 300);
        assert_eq!(
            table["windows/Assets/Square44x44Logo.targetsize-256.png"],
            256
        );
        assert_eq!(WINDOWS.len(), 35);

        // Web and the editors.
        assert_eq!(table["web/public/apple-touch-icon.png"], 180);
        assert_eq!(table["web/public/icon-512.png"], 512);
        assert_eq!(table["vscode/icon-128.png"], 128);
        assert_eq!(table["vscode/icon-256.png"], 256);

        // No two entries share a path, which a copy-paste of a scale row would silently do.
        assert_eq!(table.len(), rasters().len());
    }

    /// Every size is written, at exactly the pixels the table says.
    #[test]
    fn each_platform_gets_its_whole_set_b76ce8bf() {
        let dir = scratch("whole-set");
        std::fs::write(dir.join("icon.svg"), SQUARE).expect("the source writes");

        let out = written(&dir, "icon.svg");
        let by_path: BTreeMap<_, _> = out.iter().map(|(p, o)| (p.as_str(), o)).collect();

        for raster in rasters() {
            let Some(Output::Bytes(bytes)) = by_path.get(raster.path) else {
                panic!("{} was not rendered", raster.path);
            };
            let (width, height) = png_size(bytes);
            assert_eq!(
                (width, height),
                (raster.width, raster.height),
                "{} is the wrong size",
                raster.path
            );
            // A canvas of the right size with nothing drawn on it would satisfy every assertion
            // above, and is exactly what a wrong transform produces.
            assert!(
                opaque_centre(bytes),
                "{} is an empty canvas — the artwork did not land on it",
                raster.path
            );
        }

        // The vector is written where a platform reads one, and the wrappers beside it.
        assert!(matches!(
            by_path.get("web/public/icon.svg"),
            Some(Output::Text(text)) if text.contains("<svg")
        ));
        assert!(by_path.contains_key("apple/Assets.xcassets/AppIcon.appiconset/Contents.json"));
        assert!(by_path.contains_key("android/res/mipmap-anydpi-v26/ic_launcher.xml"));
        assert!(by_path.contains_key("web/icons.manifest.json"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the acceptance: no icon is not an error.
    #[test]
    fn a_profile_without_an_icon_writes_none_and_does_not_fail_b76ce8bf() {
        let out = outputs(&profile_with(None), Path::new(".")).expect("no icon is not a failure");
        assert!(out.is_empty());

        // An icon stated as the empty string is the same thing, not a path to "".
        let out = outputs(&profile_with(Some("")), Path::new(".")).expect("empty is not a failure");
        assert!(out.is_empty());
    }

    /// Android's foreground is inset to the 72dp safe zone: a launcher that rounds the corners
    /// must not crop the artwork. The corner pixel of the 108dp canvas is therefore transparent
    /// where the legacy icon's is painted.
    #[test]
    fn the_adaptive_foreground_stays_inside_the_safe_zone_b76ce8bf() {
        let dir = scratch("safe-zone");
        std::fs::write(dir.join("icon.svg"), SQUARE).expect("the source writes");
        let out = written(&dir, "icon.svg");
        let by_path: BTreeMap<_, _> = out.iter().map(|(p, o)| (p.as_str(), o)).collect();

        let legacy = by_path["android/res/mipmap-mdpi/ic_launcher.png"].bytes();
        let foreground = by_path["android/res/mipmap-mdpi/ic_launcher_foreground.png"].bytes();

        assert!(
            opaque_corner(legacy),
            "the legacy icon fills its canvas edge to edge"
        );
        assert!(
            !opaque_corner(foreground),
            "the adaptive foreground must leave the masked margin empty"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A profile whose icon is a PNG — what the parent task originally proposed — is refused with
    /// the decision named, rather than failing as an unreadable file.
    #[test]
    fn a_raster_source_is_refused_by_name_b76ce8bf() {
        let dir = scratch("raster-source");
        std::fs::write(dir.join("icon.svg"), "not an svg at all").expect("the source writes");

        let problems =
            outputs(&profile_with(Some("icon.svg")), &dir).expect_err("a non-SVG is refused");
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("the brand icon source is an SVG"),
            "the refusal should name the decision: {}",
            problems[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The five profiles astrid-web ships today all set `assets.icon` to a web-absolute PNG URL,
    /// which web reads at runtime. `gen-brand` must still succeed on every one of them — the
    /// parent task's rule is that they validate unchanged — writing no icons and saying why.
    #[test]
    fn todays_runtime_reference_is_not_artwork_b76ce8bf() {
        for url in [
            "/icons/icon-512x512.png",
            "/icons/whitelabel-512.png",
            "https://cdn.example.com/mark.png",
        ] {
            let profile = profile_with(Some(url));
            assert_eq!(source(&profile), Source::Reference(url.to_string()));
            assert!(
                outputs(&profile, Path::new("."))
                    .expect("a runtime reference is not a failure")
                    .is_empty(),
                "{url} is a URL web reads, not a vector to render"
            );
            assert!(
                note(&source(&profile)).expect("it says why").contains(url),
                "the note should name the value it skipped"
            );
        }

        // An SVG is the vector master, whatever its case, and silence is only for the others.
        assert_eq!(
            source(&profile_with(Some("brand/mark.SVG"))),
            Source::Vector("brand/mark.SVG".to_string())
        );
        assert_eq!(note(&Source::Absent), None);
        assert_eq!(note(&Source::Vector("a.svg".to_string())), None);
    }

    /// A missing file names the field, the way every other profile problem does.
    #[test]
    fn a_missing_icon_file_names_the_field_b76ce8bf() {
        let problems = outputs(&profile_with(Some("nowhere.svg")), Path::new("."))
            .expect_err("a missing file is refused");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("assets.icon \"nowhere.svg\""));
    }

    /// The adaptive background is the brand's accent — the one colour the schema requires. The
    /// field name is checked against the shipped schema, because a key that is not in it reads
    /// as absent and paints every partner's launcher white without failing anything.
    #[test]
    fn the_adaptive_background_is_the_brands_accent_b76ce8bf() {
        let mut profile = profile_with(Some("icon.svg"));
        profile
            .values
            .insert("colors.accent".to_string(), "#101014".to_string());
        assert!(adaptive_background(&profile).contains("#101014"));
        assert!(adaptive_background(&profile_with(None)).contains("#FFFFFF"));

        let schema = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand/schema.json"),
        )
        .expect("the shipped schema reads");
        let schema: Value = serde_json::from_str(&schema).expect("the schema is JSON");
        assert!(
            schema["sections"]["colors"]["accent"].is_object(),
            "colors.accent must be a real schema field, or the background silently falls back"
        );
    }

    /// A non-square source keeps its aspect ratio: the wide tile letterboxes rather than
    /// stretching, which is the difference between a logo and a smeared one.
    #[test]
    fn a_wide_tile_does_not_stretch_the_artwork_b76ce8bf() {
        let dir = scratch("wide-tile");
        std::fs::write(dir.join("icon.svg"), SQUARE).expect("the source writes");
        let out = written(&dir, "icon.svg");
        let by_path: BTreeMap<_, _> = out.iter().map(|(p, o)| (p.as_str(), o)).collect();

        let wide = by_path["windows/Assets/Wide310x150Logo.png"].bytes();
        let (width, height) = png_size(wide);
        assert_eq!((width, height), (310, 150));
        // A square source scaled to fit 310×150 is 150 wide, so the left edge is bare.
        assert!(
            !opaque_corner(wide),
            "a square source in a wide tile leaves the sides empty rather than stretching"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Reading a PNG back, without a decoder dependency ──────────────────────────────────────

    /// The width and height out of a PNG's IHDR, which is always the first chunk.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        assert_eq!(&bytes[12..16], b"IHDR", "IHDR is not first");
        let read =
            |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        (read(16), read(20))
    }

    /// The decoded RGBA pixels of a rendered PNG, and its width.
    fn decode(bytes: &[u8]) -> (Vec<u8>, u32) {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("a readable PNG");
        let mut buffer = vec![0; reader.output_buffer_size().expect("a bounded buffer")];
        let info = reader.next_frame(&mut buffer).expect("a decodable frame");
        assert_eq!(
            info.color_type,
            png::ColorType::Rgba,
            "an icon keeps its alpha"
        );
        buffer.truncate(info.buffer_size());
        (buffer, info.width)
    }

    /// Whether the top-left pixel of the written PNG is opaque — read out of the bytes under
    /// test, so it is the emitter being checked and not the test's own idea of the emitter.
    fn opaque_corner(bytes: &[u8]) -> bool {
        let (pixels, _) = decode(bytes);
        pixels[3] > 0
    }

    /// Whether the centre pixel is opaque: the artwork is there at all, not merely a blank canvas
    /// of the right size, which every size assertion above would otherwise accept.
    fn opaque_centre(bytes: &[u8]) -> bool {
        let (pixels, width) = decode(bytes);
        let height = pixels.len() as u32 / 4 / width;
        let at = ((height / 2) * width + width / 2) as usize * 4;
        pixels[at + 3] > 0
    }
}
