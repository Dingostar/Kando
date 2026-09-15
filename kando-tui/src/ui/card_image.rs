//! Inline image previews for the card detail pane.
//!
//! Cards are Markdown, so they can carry `![alt](path)` references. This module
//! finds those references, resolves them against the board, and holds the
//! decoded image so the detail pane can render it with whatever graphics
//! protocol the terminal supports (Kitty, iTerm2, Sixel, or halfblocks).

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// Images larger than this are downscaled once, at load time. The protocol
/// resizes to the render area anyway; this only keeps an 8K wallpaper from
/// costing a visible pause every time the pane is resized.
const MAX_SOURCE_PIXELS: u32 = 1920;

/// Terminal graphics support plus the currently decoded image.
///
/// `None` in place of this struct means the terminal was never asked (tests) or
/// answered that it cannot draw images.
pub struct CardImages {
    picker: Picker,
    loaded: Option<Loaded>,
}

struct Loaded {
    path: PathBuf,
    /// Mtime at load time, so an image edited outside kando is picked up.
    mtime: Option<std::time::SystemTime>,
    protocol: StatefulProtocol,
}

impl CardImages {
    /// Query the terminal for graphics capabilities and font size.
    ///
    /// Must be called after entering the alternate screen but before reading
    /// any terminal events, per `ratatui-image`'s contract.
    pub fn query() -> Option<Self> {
        Picker::from_query_stdio()
            .ok()
            .map(|picker| Self { picker, loaded: None })
    }

    /// The decoded image for `path`, loading it if it isn't already current.
    ///
    /// Returns `Err` with a short, displayable reason when the file can't be
    /// read or decoded.
    pub fn protocol_for(&mut self, path: &Path) -> Result<&mut StatefulProtocol, String> {
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let current = self
            .loaded
            .as_ref()
            .is_some_and(|l| l.path == path && l.mtime == mtime);

        if !current {
            let img = load_image(path)?;
            self.loaded = Some(Loaded {
                path: path.to_path_buf(),
                mtime,
                protocol: self.picker.new_resize_protocol(img),
            });
        }

        Ok(&mut self.loaded.as_mut().expect("just loaded").protocol)
    }
}

fn load_image(path: &Path) -> Result<DynamicImage, String> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", short_name(path)))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", short_name(path)))?;
    let img = reader
        .decode()
        .map_err(|e| format!("{}: {e}", short_name(path)))?;

    if img.width() > MAX_SOURCE_PIXELS || img.height() > MAX_SOURCE_PIXELS {
        return Ok(img.resize(
            MAX_SOURCE_PIXELS,
            MAX_SOURCE_PIXELS,
            image::imageops::FilterType::Triangle,
        ));
    }
    Ok(img)
}

pub fn short_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Every `![alt](path)` target in a Markdown body, in document order.
///
/// Titles (`![a](p "t")`) and pointy brackets (`![a](<p>)`) are stripped, the
/// way any Markdown renderer would.
pub fn image_refs(body: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;

    while let Some(start) = body[i..].find("![") {
        let open = i + start;
        // Alt text runs to the first `]`, which must be followed by `(`.
        let Some(alt_end) = body[open..].find(']').map(|p| open + p) else {
            break;
        };
        if bytes.get(alt_end + 1) != Some(&b'(') {
            i = alt_end + 1;
            continue;
        }
        let Some(close) = body[alt_end..].find(')').map(|p| alt_end + p) else {
            break;
        };

        let target = clean_target(&body[alt_end + 2..close]);
        if !target.is_empty() {
            refs.push(target);
        }
        i = close + 1;
    }

    refs
}

fn clean_target(raw: &str) -> String {
    let mut t = raw.trim();

    // Drop a "title" or 'title' suffix.
    for quote in ['"', '\''] {
        if t.ends_with(quote) {
            if let Some(open) = t[..t.len() - 1].rfind(quote) {
                t = t[..open].trim_end();
            }
        }
    }

    t = t.trim();
    t = t.strip_prefix('<').unwrap_or(t);
    t = t.strip_suffix('>').unwrap_or(t);
    t.trim().to_string()
}

/// Turn one reference into a file on disk.
///
/// Cards live at `<root>/.kando/columns/<column>/<id>.md`, so a path can
/// sensibly be written relative to the card, the project root, or the board's
/// own `assets/` directory. Remote URLs are not fetched.
pub fn resolve_image(kando_dir: &Path, column_slug: &str, reference: &str) -> Option<PathBuf> {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return None;
    }

    if let Some(rest) = reference.strip_prefix("~/") {
        let home = std::env::var_os("HOME")?;
        return first_existing([PathBuf::from(home).join(rest)]);
    }

    let path = Path::new(reference);
    if path.is_absolute() {
        return first_existing([path.to_path_buf()]);
    }

    let project_root = kando_dir.parent().unwrap_or(kando_dir);
    let card_dir = kando_dir.join("columns").join(column_slug);

    first_existing([
        card_dir.join(reference),
        project_root.join(reference),
        kando_dir.join(reference),
        kando_dir.join("assets").join(reference),
    ])
}

fn first_existing<const N: usize>(candidates: [PathBuf; N]) -> Option<PathBuf> {
    candidates.into_iter().find(|p| p.is_file())
}

/// Everything the detail pane needs to draw the image half.
///
/// Bundled into one struct so the already long `render_card_detail` signature
/// doesn't grow five more parameters.
pub struct ImagePane<'a> {
    /// `None` when the terminal can't draw images.
    pub images: Option<&'a mut CardImages>,
    pub kando_dir: &'a Path,
    pub column_slug: &'a str,
    /// Which of the card's images to show, wrapped into range by the caller.
    pub index: usize,
    /// User pressed `i` to hide the image half and read the text full height.
    pub collapsed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_image_refs_in_order() {
        let body = "text ![one](a.png) more\n\n![two](dir/b.jpg)\n";
        assert_eq!(image_refs(body), vec!["a.png", "dir/b.jpg"]);
    }

    #[test]
    fn strips_titles_and_brackets() {
        assert_eq!(image_refs(r#"![a](p.png "A title")"#), vec!["p.png"]);
        assert_eq!(image_refs("![a](p.png 'A title')"), vec!["p.png"]);
        assert_eq!(image_refs("![a](<p.png>)"), vec!["p.png"]);
        assert_eq!(image_refs("![a](  p.png  )"), vec!["p.png"]);
    }

    #[test]
    fn ignores_plain_links_and_empty_targets() {
        assert_eq!(image_refs("[not an image](a.png)"), Vec::<String>::new());
        assert_eq!(image_refs("![alt] (a.png)"), Vec::<String>::new());
        assert_eq!(image_refs("![alt]()"), Vec::<String>::new());
    }

    #[test]
    fn keeps_scanning_after_a_malformed_ref() {
        assert_eq!(image_refs("![a] text ![b](ok.png)"), vec!["ok.png"]);
    }

    #[test]
    fn empty_body_has_no_refs() {
        assert_eq!(image_refs(""), Vec::<String>::new());
    }

    fn board(root: &Path) -> PathBuf {
        let kando_dir = root.join(".kando");
        std::fs::create_dir_all(kando_dir.join("columns/in-progress")).unwrap();
        std::fs::create_dir_all(kando_dir.join("assets")).unwrap();
        kando_dir
    }

    #[test]
    fn resolves_against_assets_project_root_and_card_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = board(tmp.path());

        std::fs::write(kando_dir.join("assets/shot.png"), b"x").unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/diagram.png"), b"x").unwrap();
        std::fs::write(kando_dir.join("columns/in-progress/local.png"), b"x").unwrap();

        let r = |reference| resolve_image(&kando_dir, "in-progress", reference);

        assert_eq!(r("shot.png"), Some(kando_dir.join("assets/shot.png")));
        assert_eq!(
            r("docs/diagram.png"),
            Some(tmp.path().join("docs/diagram.png"))
        );
        assert_eq!(
            r("local.png"),
            Some(kando_dir.join("columns/in-progress/local.png"))
        );
        assert_eq!(
            r("../../assets/shot.png"),
            Some(kando_dir.join("columns/in-progress/../../assets/shot.png"))
        );
    }

    #[test]
    fn unresolvable_references_are_none() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = board(tmp.path());
        let r = |reference| resolve_image(&kando_dir, "in-progress", reference);

        assert_eq!(r("https://example.com/a.png"), None);
        assert_eq!(r("nope.png"), None);
        assert_eq!(r("/definitely/not/here.png"), None);
    }

    #[test]
    fn a_directory_is_not_an_image() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = board(tmp.path());
        assert_eq!(resolve_image(&kando_dir, "in-progress", "assets"), None);
    }
}
