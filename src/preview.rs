use anyhow::{Context, Result, bail};
use hayro::hayro_interpret::{self, Device, InterpreterSettings, TransformExt};
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::kurbo::{Affine, BezPath, Point, Rect, Shape};
use hayro::{RenderCache, RenderSettings, render};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PAGE_PIXELS: f32 = 4096.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Image,
    Pdf,
}

pub fn kind_for(path: &Path) -> Option<PreviewKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" => Some(PreviewKind::Image),
        "pdf" => Some(PreviewKind::Pdf),
        _ => None,
    }
}

pub struct LoadedPreview {
    pub linux_path: PathBuf,
    pub host_path: PathBuf,
    pub kind: PreviewKind,
    pub pdf_bytes: Option<Vec<u8>>,
    pub page_count: usize,
    pub first_page: Option<RenderedPage>,
}

pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub page_width: f32,
    pub page_height: f32,
    pub rgba: Vec<u8>,
    pub text: PdfTextPage,
}

#[derive(Clone, Debug, Default)]
pub struct PdfTextPage {
    pub glyphs: Vec<PdfGlyph>,
}

#[derive(Clone, Debug)]
pub struct PdfGlyph {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub baseline_y: f32,
}

impl PdfTextPage {
    pub fn nearest_glyph(&self, x: f32, y: f32) -> Option<usize> {
        self.glyphs.iter().enumerate().min_by(|(_, a), (_, b)| {
            let distance = |g: &PdfGlyph| {
                let dx = (g.x - x).max(0.0).max(x - g.x - g.width);
                let dy = (g.y - y).max(0.0).max(y - g.y - g.height);
                dx * dx + dy * dy
            };
            distance(a).total_cmp(&distance(b))
        }).map(|(index, _)| index)
    }

    pub fn selected_text(&self, anchor: usize, focus: usize) -> String {
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let mut result = String::new();
        let mut previous: Option<&PdfGlyph> = None;
        let mut line_height = 0.0_f32;
        for glyph in self.glyphs.get(start..=end).unwrap_or_default() {
            if glyph.text.chars().all(char::is_whitespace) {
                if previous.is_some() && !result.ends_with(' ') && !result.ends_with('\n') {
                    result.push(' ');
                }
                continue;
            }
            if let Some(last) = previous {
                let text_height = line_height.max(glyph.height).max(2.0);
                if (glyph.baseline_y - last.baseline_y).abs() > text_height * 0.6 {
                    while result.ends_with(' ') {
                        result.pop();
                    }
                    result.push('\n');
                    line_height = 0.0;
                } else if glyph.x > last.x + last.width + text_height * 0.5
                    && !result.ends_with(' ') && !result.ends_with('\n')
                    && !matches!(last.text.chars().last(), Some('-' | '\u{2010}' | '\u{2011}'))
                    && !matches!(glyph.text.chars().next(), Some('.' | ',' | ';' | ':' | '!' | '?' | '%' | ')' | ']' | '}'))
                {
                    result.push(' ');
                }
            }
            result.push_str(&glyph.text);
            line_height = line_height.max(glyph.height);
            previous = Some(glyph);
        }
        result.trim_end().to_string()
    }

    pub fn selection_rects(&self, anchor: usize, focus: usize) -> Vec<(f32, f32, f32, f32)> {
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let mut lines: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        for glyph in self.glyphs.get(start..=end).unwrap_or_default() {
            if glyph.text.trim().is_empty() {
                continue;
            }
            let right = glyph.x + glyph.width;
            let bottom = glyph.y + glyph.height;
            if let Some(line) = lines.last_mut() {
                let line_height = line.3 - line.1;
                let gap = if glyph.x > line.2 {
                    glyph.x - line.2
                } else if right < line.0 {
                    line.0 - right
                } else {
                    0.0
                };
                if (glyph.baseline_y - line.4).abs() <= line_height.max(glyph.height) * 0.4
                    && gap <= line_height.max(glyph.height) * 4.0
                {
                    line.0 = line.0.min(glyph.x);
                    line.1 = line.1.min(glyph.y);
                    line.2 = line.2.max(right);
                    line.3 = line.3.max(bottom);
                    continue;
                }
            }
            lines.push((glyph.x, glyph.y, right, bottom, glyph.baseline_y));
        }
        lines.into_iter().map(|(left, top, right, bottom, _)| {
            let padding = ((bottom - top) * 0.12).clamp(1.0, 3.0);
            (left, top - padding, right - left, bottom - top + padding * 2.0)
        }).collect()
    }
}

#[derive(Default)]
struct TextDevice(PdfTextPage);

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<hayro_interpret::SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: hayro_interpret::BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &hayro_interpret::Paint<'a>, _: &hayro_interpret::PathDrawMode) {}
    fn push_clip_path(&mut self, _: &hayro_interpret::ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<hayro_interpret::SoftMask<'a>>, _: hayro_interpret::BlendMode) {}
    fn draw_glyph(&mut self, glyph: &hayro_interpret::font::Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &hayro_interpret::Paint<'a>, _: &hayro_interpret::GlyphDrawMode) {
        let Some(unicode) = glyph.as_unicode() else { return; };
        let text = match unicode {
            hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro_interpret::hayro_cmap::BfString::String(s) => s,
        };
        let rect = match glyph {
            hayro_interpret::font::Glyph::Outline(outline) => ((transform * glyph_transform) * outline.outline()).bounding_box(),
            hayro_interpret::font::Glyph::Type3(_) => return,
        };
        if text.is_empty() || !rect.x0.is_finite() || !rect.y0.is_finite() { return; }
        let baseline = (transform * glyph_transform) * Point::ZERO;
        self.0.glyphs.push(PdfGlyph { text, x: rect.x0 as f32, y: rect.y0 as f32,
            width: rect.width().max(2.0) as f32, height: rect.height().max(2.0) as f32,
            baseline_y: baseline.y as f32 });
    }
    fn draw_image(&mut self, _: hayro_interpret::Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

pub fn open(linux_path: PathBuf, host_path: PathBuf, kind: PreviewKind) -> Result<LoadedPreview> {
    if kind == PreviewKind::Image {
        if !host_path.is_file() {
            bail!("cannot read {}", linux_path.display());
        }
        return Ok(LoadedPreview {
            linux_path,
            host_path,
            kind,
            pdf_bytes: None,
            page_count: 0,
            first_page: None,
        });
    }

    let metadata = fs::metadata(&host_path)
        .with_context(|| format!("cannot read {}", linux_path.display()))?;
    if metadata.len() > MAX_PDF_BYTES {
        bail!("PDF is larger than 64 MiB");
    }
    let bytes = fs::read(&host_path)?;
    let pdf = Pdf::new(bytes.clone()).map_err(|_| anyhow::anyhow!("cannot read PDF"))?;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        bail!("PDF has no pages");
    }
    let first_page = Some(render_pdf_page(&pdf, 0, 1.5)?);
    Ok(LoadedPreview {
        linux_path,
        host_path,
        kind,
        pdf_bytes: Some(bytes),
        page_count,
        first_page,
    })
}

pub fn render_page(bytes: &[u8], index: usize, zoom: f32) -> Result<RenderedPage> {
    let pdf = Pdf::new(bytes.to_vec()).map_err(|_| anyhow::anyhow!("cannot read PDF"))?;
    render_pdf_page(&pdf, index, zoom)
}

fn render_pdf_page(pdf: &Pdf, index: usize, zoom: f32) -> Result<RenderedPage> {
    let page = pdf.pages().get(index).context("PDF page is out of range")?;
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        bail!("PDF page has invalid dimensions");
    }
    let scale = raster_scale(zoom, width as f32, height as f32);
    let pixmap = render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..Default::default()
        },
    );
    let cache = hayro_interpret::InterpreterCache::new();
    let mut context = hayro_interpret::Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, width as f64, height as f64),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut device = TextDevice::default();
    hayro_interpret::interpret_page(page, &mut context, &mut device);
    Ok(RenderedPage {
        width: pixmap.width() as u32,
        height: pixmap.height() as u32,
        page_width: width as f32,
        page_height: height as f32,
        rgba: pixmap.data_as_u8_slice().to_vec(),
        text: device.0,
    })
}

fn raster_scale(zoom: f32, width: f32, height: f32) -> f32 {
    let zoom = if zoom.is_finite() {
        zoom.clamp(0.25, 4.0)
    } else {
        1.0
    };
    (2.0 * zoom).min(MAX_PAGE_PIXELS / width.max(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_supported_formats_without_misclassifying_text() {
        assert_eq!(kind_for(Path::new("picture.PNG")), Some(PreviewKind::Image));
        assert_eq!(kind_for(Path::new("notes.pdf")), Some(PreviewKind::Pdf));
        assert_eq!(kind_for(Path::new("README.md")), None);
        assert_eq!(kind_for(Path::new("photo.png.txt")), None);
    }

    #[test]
    fn rejects_invalid_pdf_instead_of_opening_it_as_text() {
        let path = std::env::temp_dir().join(format!("araseo-invalid-pdf-{}", std::process::id()));
        fs::write(&path, b"not a pdf").unwrap();
        assert!(open(path.clone(), path.clone(), PreviewKind::Pdf).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn opens_and_renders_a_pdf_page() {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let objects: [&[u8]; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>",
            b"<< /Length 31 >>\nstream\nq 1 0 0 rg 0 0 20 20 re f Q\nendstream",
        ];
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(object);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Root 1 0 R /Size 5 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );

        let path = std::env::temp_dir().join(format!("araseo-render-pdf-{}", std::process::id()));
        fs::write(&path, pdf).unwrap();
        let opened = open(path.clone(), path.clone(), PreviewKind::Pdf).unwrap();
        assert_eq!(opened.page_count, 1);
        let page = opened.first_page.unwrap();
        assert!(page.width >= 100 && page.height >= 100);
        assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
        assert!(page.text.glyphs.is_empty());
        assert!(
            page.rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 150 && pixel[1] < 50 && pixel[2] < 50)
        );
        let bytes = opened.pdf_bytes.as_ref().unwrap();
        let larger = render_page(bytes, 0, 2.0).unwrap();
        assert!(larger.width > page.width && larger.height > page.height);
        assert_eq!((larger.page_width, larger.page_height), (100.0, 100.0));
        assert!(render_page(bytes, 1, 1.0).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn raster_scale_is_bounded_for_large_pages_and_invalid_zoom() {
        assert_eq!(raster_scale(1.0, 100.0, 100.0), 2.0);
        assert_eq!(raster_scale(2.0, 100.0, 100.0), 4.0);
        assert_eq!(raster_scale(f32::NAN, 100.0, 100.0), 2.0);
        assert_eq!(raster_scale(4.0, 5000.0, 2000.0), 4096.0 / 5000.0);
    }

    #[test]
    fn selects_and_copies_text_from_rendered_pdf() {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let content = b"BT /F1 18 Tf 20 70 Td (Hello PDF) Tj 0 -25 Td (Next) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), String::from_utf8_lossy(content)),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Root 1 0 R /Size 6 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());

        let page = render_page(&pdf, 0, 1.0).unwrap();
        assert_eq!(page.text.selected_text(0, page.text.glyphs.len() - 1), "Hello PDF\nNext");
        assert_eq!(page.text.selected_text(6, 8), "PDF");
        let first = &page.text.glyphs[0];
        assert_eq!(page.text.nearest_glyph(first.x + 1.0, first.y + 1.0), Some(0));
        let first_line = page.text.selection_rects(0, 8);
        assert_eq!(first_line.len(), 1, "one highlighted band should cover the first line");
        assert!(first_line[0].2 > page.text.glyphs[8].x - first_line[0].0);
        assert_eq!(page.text.selection_rects(0, page.text.glyphs.len() - 1).len(), 2);
        let zoomed = render_page(&pdf, 0, 2.0).unwrap();
        assert_eq!(page.text.glyphs.len(), zoomed.text.glyphs.len());
        assert!((page.text.glyphs[0].x - zoomed.text.glyphs[0].x).abs() < 0.01);
    }

    #[test]
    fn selection_bands_cover_word_gaps_without_joining_separate_columns() {
        let glyph = |text: &str, x, y, baseline_y| PdfGlyph {
            text: text.into(), x, y, width: 8.0, height: 10.0, baseline_y,
        };
        let page = PdfTextPage { glyphs: vec![
            glyph("A", 10.0, 10.0, 20.0),
            glyph("b", 18.0, 12.0, 20.0),
            glyph("C", 38.0, 10.0, 20.0),
            glyph("D", 180.0, 10.0, 20.0),
            glyph("E", 10.0, 35.0, 45.0),
        ] };
        let bands = page.selection_rects(0, 4);
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].0, 10.0);
        assert_eq!(bands[0].2, 36.0);
        assert!(bands[0].1 <= 10.0 && bands[0].1 + bands[0].3 >= 22.0);
        assert_eq!(bands[1].0, 180.0);
        assert_eq!(bands[2].0, 10.0);
    }

    #[test]
    fn copied_pdf_text_keeps_low_punctuation_and_hyphens_on_their_lines() {
        let glyph = |text: &str, x, y, width, height, baseline_y| PdfGlyph {
            text: text.into(), x, y, width, height, baseline_y,
        };
        let page = PdfTextPage { glyphs: vec![
            glyph("role", 450.0, 189.3, 26.0, 6.0, 195.0),
            glyph(".", 477.1, 193.7, 2.0, 2.0, 195.0),
            glyph(" ", 0.0, 0.0, 2.0, 2.0, 195.0),
            glyph("FDE", 482.5, 187.2, 22.0, 7.9, 195.0),
            glyph(" ", 0.0, 0.0, 2.0, 2.0, 195.0),
            glyph("sites", 100.0, 205.1, 25.0, 6.0, 210.9),
            glyph(",", 125.3, 209.6, 2.0, 2.5, 210.9),
            glyph(" ", 0.0, 0.0, 2.0, 2.0, 210.9),
            glyph("people", 100.0, 220.9, 30.0, 6.0, 226.7),
            glyph("-", 130.5, 222.9, 2.0, 2.0, 226.7),
            glyph("management", 134.0, 220.9, 60.0, 6.0, 226.7),
            glyph("-", 194.5, 222.9, 2.0, 2.0, 226.7),
            glyph("only", 198.0, 220.9, 22.0, 6.0, 226.7),
        ] };
        let expected = "role. FDE\nsites,\npeople-management-only";
        assert_eq!(page.selected_text(0, page.glyphs.len() - 1), expected);
        assert_eq!(page.selected_text(page.glyphs.len() - 1, 0), expected);
        assert_eq!(page.selected_text(0, 3), "role. FDE");
    }
}
