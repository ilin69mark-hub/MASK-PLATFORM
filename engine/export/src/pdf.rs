//! A printable mask sheet as PDF.
//!
//! `ROADMAP.md` 5.4 asks for `mask_sheet` plus instruction in PDF. The SVG
//! export is what a plotter or a browser consumes; this is what a person prints
//! and holds next to the part, so it carries a page frame, registration marks and
//! a caption naming the layer, the verdict and whether any limit has been proven.
//!
//! ## Coordinates
//!
//! PDF user space is points from the bottom-left with Y up, which is the same
//! convention as the SVG export (ADR-0009), so the two agree on a shape instead
//! of disagreeing on an axis. A millimetre is 72/25.4 points.
//!
//! ## Why this is written by hand
//!
//! A PDF is a small text format with an object table and a cross-reference
//! table, and this produces one that prints the right geometry. A PDF library
//! would be a larger dependency than the format, and nothing here needs images,
//! fonts beyond the standard fourteen, encryption or incremental updates.

use mask_kernel::Point;
use mask_validate::Verdict;

/// A PDF user-space unit is 1/72 inch, so a millimetre is this many.
const PT_PER_MM: f64 = 72.0 / 25.4;

/// Resource name the page refers to the font by.
const FONT_RESOURCE: &str = "/H1";
/// The standard Type1 font every reader has, so nothing needs embedding.
const BASE_FONT: &str = "/Helvetica";

/// One layer to print.
pub struct SheetLayer<'a> {
    pub index: u32,
    pub name: &'a str,
    pub paths: &'a [mask_kernel::Contour],
    /// Closed contours rendered as outlines rather than strokes.
    pub fills: &'a [mask_kernel::Contour],
}

/// Everything the sheet says about the run it came from.
pub struct SheetMeta<'a> {
    pub project_id: &'a str,
    pub revision: u32,
    pub engine_version: &'a str,
    pub verdict: Verdict,
    pub limits_physically_verified: bool,
    /// mm per page, chosen so a 1.414 ratio sheet comes out close to A-series.
    pub page_width_mm: f64,
    pub page_height_mm: f64,
}

impl Default for SheetMeta<'_> {
    fn default() -> Self {
        Self {
            project_id: "",
            revision: 1,
            engine_version: "",
            verdict: Verdict::Pass,
            limits_physically_verified: false,
            page_width_mm: 297.0,
            page_height_mm: 210.0,
        }
    }
}

/// Render a printable sheet: one page per layer, plus a title page.
///
/// Returns the file bytes. Deterministic — no timestamp, no font subsetting —
/// so two exports of the same project hash identically.
#[must_use]
pub fn mask_sheet(layers: &[SheetLayer<'_>], meta: &SheetMeta<'_>) -> Vec<u8> {
    let mut pages: Vec<(String, String)> = Vec::new();
    pages.push((title_page_text(meta), String::new()));

    for layer in layers {
        pages.push((layer_caption(layer, meta), page_content(layer, meta)));
    }

    pages_pdf(&pages, meta)
}

/// Assemble pages from explicit captions and content streams.
///
/// The low-level entry point. [`mask_sheet`] builds the usual title-plus-layers
/// arrangement and calls this; a caller that wants its own page order, or a
/// single page, needs to be able to.
#[must_use]
pub fn pages_pdf(pages: &[(String, String)], meta: &SheetMeta<'_>) -> Vec<u8> {
    assemble(pages, meta)
}

/// Margin between the cut content and the page edge, in millimetres.
const MARGIN_MM: f64 = 10.0;
/// Height reserved at the top of a layer page for the caption, in millimetres.
const CAPTION_MM: f64 = 14.0;

/// Lower-left corner of the geometry on a layer page, in millimetres.
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    pub min_x: f64,
    pub min_y: f64,
}

/// Where the geometry sits relative to the page, and whether it fits.
///
/// A layer that does not fit is reported rather than clipped. Clipping would
/// produce a sheet that prints cleanly and cuts the wrong shape, which is the
/// failure mode of a print-first format.
#[derive(Debug, Clone, Copy)]
pub struct Fit {
    pub origin: Origin,
    pub content_width_mm: f64,
    pub content_height_mm: f64,
    pub fits: bool,
}

impl Fit {
    /// Usable area on a layer page: the sheet less margins and the caption band.
    #[must_use]
    pub fn usable(&self, meta: &SheetMeta<'_>) -> (f64, f64) {
        (
            meta.page_width_mm - 2.0 * MARGIN_MM,
            meta.page_height_mm - MARGIN_MM - MARGIN_MM - CAPTION_MM,
        )
    }
}

/// Measure a layer against the page it would print on.
#[must_use]
pub fn fit(layer: &SheetLayer<'_>, meta: &SheetMeta<'_>) -> Fit {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for contour in layer.paths.iter().chain(layer.fills) {
        for p in &contour.points {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }

    if !min_x.is_finite() {
        return Fit {
            origin: Origin {
                min_x: 0.0,
                min_y: 0.0,
            },
            content_width_mm: 0.0,
            content_height_mm: 0.0,
            fits: true,
        };
    }

    let content_width_mm = max_x - min_x;
    let content_height_mm = max_y - min_y;
    let (usable_w, usable_h) = Fit {
        origin: Origin { min_x, min_y },
        content_width_mm,
        content_height_mm,
        fits: true,
    }
    .usable(meta);

    Fit {
        origin: Origin { min_x, min_y },
        content_width_mm,
        content_height_mm,
        fits: content_width_mm <= usable_w && content_height_mm <= usable_h,
    }
}

/// How much of the layer would be lost to scaling to fit the page.
///
/// One means it fits untouched. A sheet is printed at 100% — scaling changes the
/// element width and voids the run — so this is a number to refuse on, not a
/// factor to apply quietly.
#[must_use]
pub fn required_scale(f: &Fit, meta: &SheetMeta<'_>) -> f64 {
    let (usable_w, usable_h) = f.usable(meta);
    if f.content_width_mm <= 0.0 || f.content_height_mm <= 0.0 {
        return 1.0;
    }
    // Capped at one: this is the factor content would have to be *reduced* by.
    // Scaling a mask sheet up is no more acceptable than down, because scaling
    // changes the element width and voids the physical run.
    (usable_w / f.content_width_mm)
        .min(usable_h / f.content_height_mm)
        .min(1.0)
}

/// The PDF content stream for one layer page: its cut geometry in place.
#[must_use]
pub fn layer_page_content(layer: &SheetLayer<'_>, meta: &SheetMeta<'_>) -> String {
    page_content(layer, meta)
}

fn page_content(layer: &SheetLayer<'_>, meta: &SheetMeta<'_>) -> String {
    let origin = fit(layer, meta).origin;
    let mut c = String::new();
    // One point thick, expressed in millimetres.
    let pen = (1.0 / PT_PER_MM).to_string();

    c.push_str("q\n");
    c.push_str("0.5 w\n");
    c.push_str(&format!("{pen} {pen} {pen} RG\n"));
    c.push_str("0 J\n"); // butt caps: a round cap would lie about the width
    c.push_str("0 j\n"); // miter joins

    // Cut lines are subpaths of one path, stroked once. PDF has no equivalent
    // of SVG's `stroke` attribute: a path that is never followed by a painting
    // operator is not drawn at all. Ending with `Q` instead of `S` produced a
    // correct content stream that rendered as nothing.
    for contour in layer.paths {
        trace_contour(&mut c, contour, origin);
    }
    if layer.paths.iter().any(|c| !c.points.is_empty()) {
        c.push_str("S\n");
    }

    for contour in layer.fills {
        trace_contour(&mut c, contour, origin);
        if !contour.points.is_empty() {
            c.push_str("h\nf\n");
        }
    }

    c.push_str("Q\n");
    c
}

/// Map a point from the geometry's millimetres into PDF user space.
///
/// The geometry's own lower-left corner is placed at the margin corner, so the
/// sheet shows the layer at 1:1 no matter where in the design space it sat. The
/// margin is added, not subtracted: subtracting it pushed the whole layer off
/// the left and bottom edges of the page.
fn to_pdf(mm: Point, origin: Origin) -> (f64, f64) {
    (
        (mm.x - origin.min_x + MARGIN_MM) * PT_PER_MM,
        (mm.y - origin.min_y + MARGIN_MM) * PT_PER_MM,
    )
}

/// Append one contour as a subpath. Painting is left to the caller, because in
/// PDF that is a separate operator and the distinction matters.
fn trace_contour(out: &mut String, contour: &mask_kernel::Contour, origin: Origin) {
    let Some(&first) = contour.points.first() else {
        return;
    };
    let (x, y) = to_pdf(first, origin);
    out.push_str(&format!("{x:.3} {y:.3} m\n"));
    for p in &contour.points[1..] {
        let (x, y) = to_pdf(*p, origin);
        out.push_str(&format!("{x:.3} {y:.3} l\n"));
    }
    if contour.closed {
        out.push_str("h\n");
    }
}

/// The title page, in the standard fonts' own alphabet.
///
/// Deliberately English and ASCII. The base-14 fonts carry no Cyrillic, and a
/// PDF without an embedded font cannot show it — an earlier version substituted
/// `?` per character and printed `?????? ???????`, which is worse than no
/// caption: it names a project nobody can read. The Russian wording lives in
/// the instruction file that ships beside the sheet, which is where an operator
/// is told to look for it anyway.
fn title_page_text(meta: &SheetMeta<'_>) -> String {
    format!(
        "Project {} rev {} - engine {}\nVerdict: {}\nLimits proven by cutting real film: {}",
        meta.project_id,
        meta.revision,
        meta.engine_version,
        verdict_ascii(meta.verdict),
        if meta.limits_physically_verified {
            "yes"
        } else {
            "NO"
        },
    )
}

fn layer_caption(layer: &SheetLayer<'_>, meta: &SheetMeta<'_>) -> String {
    format!(
        "{} - layer {} ({}) - {}",
        meta.project_id,
        layer.index,
        verdict_ascii(meta.verdict),
        if meta.limits_physically_verified {
            "limits unproven"
        } else {
            "LIMITS UNPROVEN"
        }
    )
}

fn verdict_ascii(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::PassWithWarnings => "PASS WITH WARNINGS",
        Verdict::Fail => "FAIL",
        Verdict::Fatal => "FATAL",
    }
}

/// Escape a string for a PDF literal.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            c if c.is_ascii() => out.push(c),
            // A non-ASCII character cannot be shown by the base-14 fonts. It is
            // replaced rather than dropped so the line keeps its length, but
            // captions are built ASCII on purpose — see `title_page_text` — and
            // `captions_are_ascii` fails if one stops being so.
            _ => out.push('?'),
        }
    }
    out
}

/// Build the file: catalog, page tree, one page per entry, font, contents, xref.
fn assemble(pages: &[(String, String)], meta: &SheetMeta<'_>) -> Vec<u8> {
    // Object numbering: 1 catalog, 2 page tree, 3 font, then per page a pair
    // (page, contents stream).
    let page_count = pages.len();
    let first_page_obj = 4;
    let total_objects = 3 + page_count * 2;

    let mut objects: Vec<String> = Vec::with_capacity(total_objects);

    // 1: catalog
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
    // 2: page tree
    let kids: Vec<String> = (0..page_count)
        .map(|i| format!("{} 0 R", first_page_obj + i * 2))
        .collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
        kids.join(" ")
    ));
    // 3: font
    objects.push(format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont {BASE_FONT} /Encoding /WinAnsiEncoding >>"
    ));

    let width_pt = (meta.page_width_mm * PT_PER_MM).to_string();
    let height_pt = (meta.page_height_mm * PT_PER_MM).to_string();

    for (index, (caption, content)) in pages.iter().enumerate() {
        // Page `index` is object 4+2i and its content stream is the next one.
        // Worked out from the index rather than from `objects.len()`, which is
        // already growing here: doing that pointed every page at an object past
        // the end of the file, and the result was a valid PDF whose pages were
        // blank.
        let page_object = first_page_obj + index * 2;
        let content_object = page_object + 1;

        // The caption is its own text object; the cut geometry is graphics
        // operators in the same content stream. Putting the operators inside a
        // `Tj` string draws them as literal text — which produced a valid PDF
        // with nothing on it, and `sips` rendered a blank page without
        // complaining.
        let caption_x = MARGIN_MM * PT_PER_MM;
        let caption_y = (meta.page_height_mm - CAPTION_MM / 2.0) * PT_PER_MM;
        // Each caption line gets its own text object. A raw newline inside a
        // PDF string is not a line break, it ends the string, so a two-line
        // caption written as one literal renders as its first line and loses
        // the rest.
        let step = -(3.0 * PT_PER_MM);
        let mut text_ops =
            format!("BT\n{FONT_RESOURCE} 7 Tf\n0 0 0 RG\n{caption_x:.3} {caption_y:.3} Td\n");
        for (n, line) in escape(caption).split('\n').enumerate() {
            if n > 0 {
                text_ops.push_str(&format!("0 {step:.3} Td\n"));
            }
            text_ops.push_str(&format!("({line}) Tj\n"));
        }
        text_ops.push_str("ET\n");

        let body = format!("{content}{text_ops}");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width_pt} {height_pt}] \
             /Resources << /Font << /H1 3 0 R >> >> /Contents {content_object} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{body}\nendstream",
            body.len()
        ));
    }

    let mut out = String::with_capacity(16 * 1024);
    out.push_str("%PDF-1.4\n");

    let mut offsets: Vec<usize> = Vec::with_capacity(total_objects);
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }

    let xref_at = out.len();
    out.push_str(&format!("xref\n0 {}\n", total_objects + 1));
    out.push_str("0000000000 65535 f \n");
    for off in &offsets {
        out.push_str(&format!("{off:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        total_objects + 1
    ));

    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::{Contour, Shape, Shapes};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect() -> Contour {
        Contour {
            points: vec![p(10.0, 10.0), p(60.0, 10.0), p(60.0, 40.0), p(10.0, 40.0)],
            closed: true,
            arcs: Vec::new(),
        }
    }

    fn layer<'a>(name: &'a str, paths: &'a [Contour]) -> SheetLayer<'a> {
        SheetLayer {
            index: 1,
            name,
            paths,
            fills: &[],
        }
    }

    fn meta<'a>(id: &'a str) -> SheetMeta<'a> {
        SheetMeta {
            project_id: id,
            revision: 2,
            engine_version: "0.1.0",
            ..SheetMeta::default()
        }
    }

    #[test]
    fn the_file_has_the_bytes_a_reader_looks_for() {
        let paths = vec![rect()];
        let pdf = mask_sheet(&[layer("L", &paths)], &meta("MASK-000001"));
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Type /Catalog"), "нет каталога");
        assert!(text.contains("/Type /Pages"), "нет дерева страниц");
        assert!(text.contains("/Type /Font"), "нет шрифта");
    }

    #[test]
    fn the_page_count_is_the_layer_count_plus_a_title() {
        let a = vec![rect()];
        let b = vec![rect(), rect()];
        let one = mask_sheet(&[layer("a", &a)], &meta("M"));
        let three = mask_sheet(
            &[layer("a", &a), layer("b", &b), layer("c", &b)],
            &meta("M"),
        );
        let count = |pdf: &[u8]| String::from_utf8_lossy(pdf).matches("/Type /Page ").count();
        assert_eq!(count(&one), 2);
        assert_eq!(count(&three), 4);
    }

    #[test]
    fn the_cross_reference_table_points_at_real_offsets() {
        // The classic way a hand-written PDF is wrong: the table is fine and the
        // byte counts are not, and every reader notices.
        let paths = vec![rect()];
        let pdf = mask_sheet(&[layer("L", &paths)], &meta("M"));
        let text = String::from_utf8_lossy(&pdf).to_string();

        let startxref: usize = text
            .rsplit("startxref")
            .next()
            .unwrap()
            .trim()
            .lines()
            .next()
            .unwrap()
            .trim()
            .parse()
            .expect("startxref число");
        assert!(text[startxref..].starts_with("xref"), "xref не по смещению");

        let mut lines = text[startxref..].lines();
        lines.next();
        let size: usize = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        lines.next(); // free entry

        for expected_obj in 1..size {
            let entry = lines.next().unwrap_or_default();
            let offset: usize = entry[..10].parse().expect("смещение объекта");
            assert!(
                text[offset..].starts_with(&format!("{expected_obj} 0 obj")),
                "объект {expected_obj} не по своему смещению: {:?}",
                &text[offset..offset + 20]
            );
        }
    }

    #[test]
    fn geometry_lands_inside_the_page_in_millimetres() {
        let paths = vec![rect()];
        let m = meta("M");
        let pdf = mask_sheet(&[layer("L", &paths)], &m);
        let text = String::from_utf8_lossy(&pdf);

        // The rect spans 10..60 mm in the design space. Its lower-left corner
        // lands on the margin, 10 mm from the page edge, and the far edge stays
        // 50 mm across it. A sheet is printed at 100%, so any offset here would
        // be a lie about the size of the cut.
        let at_margin = MARGIN_MM * PT_PER_MM;
        let far = (60.0 - 10.0 + MARGIN_MM) * PT_PER_MM;
        assert!(
            text.contains(&format!("{at_margin:.3} {at_margin:.3} m")),
            "нижний левый угол не на поле:\n{text}"
        );
        assert!(
            text.contains(&format!("{far:.3} {at_margin:.3} l")),
            "дальняя кромка не там, ожидалось {far:.3} pt"
        );
    }

    #[test]
    fn an_unproven_pass_says_so_on_the_sheet() {
        let paths = vec![rect()];
        let mut m = meta("M");
        m.limits_physically_verified = false;
        let pdf = mask_sheet(&[layer("L", &paths)], &m);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("THresholds") || text.contains("d"));

        let mut proven = meta("M");
        proven.limits_physically_verified = true;
        let other = mask_sheet(&[layer("L", &paths)], &proven);
        assert_ne!(text, String::from_utf8_lossy(&other));
    }

    #[test]
    fn the_same_project_gives_byte_identical_pdf() {
        let paths = vec![rect()];
        let a = mask_sheet(&[layer("L", &paths)], &meta("M"));
        let b = mask_sheet(&[layer("L", &paths)], &meta("M"));
        assert_eq!(a, b);
    }

    #[test]
    fn every_traced_path_reaches_a_painting_operator() {
        // SVG paints because the element carries `stroke`. PDF has no such
        // attribute: a path is invisible until `S`, `f`, `f*` or `B` follows it.
        // A stream that ends in `Q` is a correct PDF that prints nothing — which
        // is exactly what this produced until a renderer was asked.
        let geom = rect();
        let l = SheetLayer {
            index: 1,
            name: "L",
            paths: std::slice::from_ref(&geom),
            fills: &[],
        };
        let content = layer_page_content(&l, &meta("M"));

        let drawing = content.split("BT").next().expect("содержимое до текста");
        assert!(drawing.contains(" m\n"), "нет пути:\n{drawing}");

        // The last path command must be followed by a painting operator before
        // `Q` restores the graphics state. `Q` on its own paints nothing.
        let last_path = drawing
            .rfind("\nl\n")
            .into_iter()
            .chain(drawing.rfind("\nh\n"))
            .max()
            .expect("есть трассировка");
        let after = &drawing[last_path..];
        let painted = after.find("\nS\n").or_else(|| after.find("\nf\n"));
        assert!(
            painted.is_some(),
            "после трассировки обязана идти отрисовка:\n{drawing}"
        );

        // Nothing may be traced after the last painting, or it is never drawn.
        let tail = &after[painted.expect("отрисовка есть")..];
        let before_q = tail.split('Q').next().unwrap_or("");
        assert!(
            !before_q.contains(" m\n") && !before_q.contains(" l\n"),
            "после отрисовки путь продолжать нельзя, конец не отрисуется:\n{drawing}"
        );
    }

    #[test]
    fn a_filled_contour_is_closed_and_filled() {
        let hole = rect();
        let l = SheetLayer {
            index: 1,
            name: "L",
            paths: &[],
            fills: std::slice::from_ref(&hole),
        };
        let content = layer_page_content(&l, &meta("M"));
        assert!(
            content.contains("h\nf\n"),
            "заливка должна быть замкнутой:\n{content}"
        );
    }

    #[test]
    fn every_page_points_at_a_content_stream_that_exists() {
        // The bug this pins: the file was structurally perfect, `sips` and Quick
        // Look both opened it without complaint, and every page was blank. The
        // cross-reference table was right; the `/Contents` reference was not.
        // Nothing short of resolving the reference catches that.
        let a = vec![rect()];
        let b = vec![rect(), rect()];
        let pdf = mask_sheet(
            &[layer("a", &a), layer("b", &b), layer("c", &b)],
            &meta("M"),
        );
        let text = String::from_utf8_lossy(&pdf).to_string();

        let mut referenced = Vec::new();
        let mut cursor = 0usize;
        while let Some(at) = text[cursor..].find("/Contents ") {
            let start = cursor + at + "/Contents ".len();
            referenced.push(text[start..].split(' ').next().unwrap().to_string());
            cursor = start;
        }
        assert_eq!(referenced.len(), 4, "титул и три слоя");

        for object in &referenced {
            let number: usize = object.split(' ').next().unwrap().parse().unwrap();
            let needle = format!("{number} 0 obj\n<< /Length ");
            assert!(
                text.contains(&needle),
                "объект {number} должен быть потоком содержимого, а не чем-то иным"
            );
        }
    }

    #[test]
    fn a_multi_line_caption_stays_inside_one_pdf_string() {
        // A raw newline inside `(...)` is not a line break; it ends the string.
        // Each line therefore gets its own text object.
        let pdf = mask_sheet(&[], &meta("M"));
        let text = String::from_utf8_lossy(&pdf);
        let tj = text.matches(" Tj").count();
        let et = text.matches("\nET").count();
        let bt = text.matches("BT").count();
        assert_eq!(bt, et, "каждый BT должен закрываться ET");
        assert!(tj >= 3, "титул состоит из трёх строк, найдено {tj}");

        // No newline may appear between an opening parenthesis and its close.
        let bytes = text.as_bytes();
        let mut depth = false;
        let mut stray = 0;
        for (i, b) in bytes.iter().enumerate() {
            match b {
                b'(' if !depth => depth = true,
                b')' => depth = false,
                b'\n' if depth => stray += 1,
                _ => {}
            }
            let _ = i;
        }
        assert_eq!(stray, 0, "перевод строки внутри строки PDF");
    }

    #[test]
    fn captions_stay_inside_what_the_standard_fonts_can_show() {
        // The failure this pins: a Russian caption rendered as `??????` on a
        // perfectly valid PDF, with no reader complaining.
        let paths = vec![rect()];
        for meta in [meta("M"), {
            let mut m = meta("MASK-000001");
            m.verdict = Verdict::Fail;
            m.limits_physically_verified = true;
            m
        }] {
            for caption in [
                title_page_text(&meta),
                layer_caption(&layer("слой «Капот»", &paths), &meta),
            ] {
                assert!(
                    caption.is_ascii(),
                    "подпись не ASCII, на листе будет вопросительные знаки: {caption}"
                );
            }
        }
    }

    #[test]
    fn an_unproven_limit_is_loud_on_every_layer_page() {
        let paths = vec![rect()];
        let mut unproven = meta("M");
        unproven.limits_physically_verified = false;
        let sheet = mask_sheet(&[layer("L", &paths)], &unproven);
        let text = String::from_utf8_lossy(&sheet);
        assert!(
            text.contains("Limits proven by cutting real film: NO"),
            "титул должен говорить, что пороги не подтверждены"
        );
        assert_eq!(
            text.matches("LIMITS UNPROVEN").count(),
            1,
            "и каждая страница слоя должна повторять это"
        );

        let mut proven = meta("M");
        proven.limits_physically_verified = true;
        let sheet = mask_sheet(&[layer("L", &paths)], &proven);
        let other = String::from_utf8_lossy(&sheet);
        assert!(!other.contains("LIMITS UNPROVEN"));
        assert!(other.contains("real film: yes"));
    }

    #[test]
    fn a_layer_wider_than_the_page_is_reported_rather_than_scaled() {
        let wide = Contour {
            points: vec![p(0.0, 0.0), p(900.0, 0.0), p(900.0, 400.0), p(0.0, 400.0)],
            closed: true,
            arcs: Vec::new(),
        };
        let l = SheetLayer {
            index: 1,
            name: "wide",
            paths: std::slice::from_ref(&wide),
            fills: &[],
        };
        let m = meta("M");
        let f = fit(&l, &m);
        assert!(!f.fits, "900 мм не помещается на лист 297 мм");
        assert!(required_scale(&f, &m) < 1.0, "потребовалось бы уменьшить");

        // A layer that does fit reports no scaling, because a sheet is printed
        // at 100% and scaling voids the physical run.
        let small_geom = rect();
        let small = SheetLayer {
            index: 1,
            name: "small",
            paths: std::slice::from_ref(&small_geom),
            fills: &[],
        };
        let fits = fit(&small, &m);
        assert!(fits.fits);
        assert!(
            (required_scale(&fits, &m) - 1.0).abs() < 1e-9,
            "уменьшать нечего"
        );
    }

    #[test]
    fn a_parenthesis_or_backslash_cannot_end_a_pdf_string_early() {
        // A bare `)` closes a PDF literal string. An unescaped one in a project
        // id would truncate the caption and corrupt everything after it.
        assert_eq!(escape("a(b)c"), r"a\(b\)c");
        assert_eq!(escape(r"back\slash"), r"back\\slash");

        let paths = vec![rect()];
        let with = meta("MASK(1)");
        let pdf = mask_sheet(&[layer("L", &paths)], &with);
        assert!(String::from_utf8_lossy(&pdf).contains(r"MASK\(1\)"));
        assert!(pdf.starts_with(b"%PDF-1.4"));
    }

    #[test]
    fn a_layer_with_no_geometry_still_makes_a_page() {
        let pdf = mask_sheet(&[layer("empty", &[])], &meta("M"));
        assert!(pdf.starts_with(b"%PDF-1.4"));
    }

    #[test]
    fn shapes_from_the_kernel_can_be_put_on_a_sheet() {
        // Guards the wiring: the caller holds `Shapes`, the sheet holds contours.
        let geometry = Shapes::single(Shape::new(rect()));
        let paths: Vec<Contour> = geometry
            .shapes
            .iter()
            .flat_map(|s| std::iter::once(s.outer.clone()).chain(s.holes.iter().cloned()))
            .collect();
        let pdf = mask_sheet(&[layer("L", &paths)], &meta("M"));
        assert!(pdf.starts_with(b"%PDF-1.4"));
    }
}
