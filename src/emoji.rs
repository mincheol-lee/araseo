use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::path::PathBuf;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::FontRef;

const ICON_SIZE: u32 = 24;
const EMOJIS: &[&str] = &[
    "🌿", "🧩", "🧪", "📚", "🎨", "🛠️", "📦", "⚙️", "💻", "🧱", "💡", "🗄️",
    "🌐", "📂", "📁", "🐳", "🙈", "🔒", "🔐", "📖", "⚖️", "📋", "🤖", "🦀",
    "🐍", "⚡", "🔷", "🐹", "☕", "💎", "🐦", "🎯", "🐘", "📝", "🐚", "📊",
    "🖼️", "🎵", "🎬", "📕", "📜", "🔤", "📄",
];

#[derive(Default)]
pub struct EmojiIcons {
    images: HashMap<&'static str, Image>,
}

impl EmojiIcons {
    pub fn load_system() -> Self {
        emoji_font_candidates()
            .into_iter()
            .find_map(|path| std::fs::read(path).ok())
            .and_then(|data| Self::from_font_data(&data))
            .unwrap_or_default()
    }

    pub fn from_font_data(data: &[u8]) -> Option<Self> {
        let font = FontRef::from_index(data, 0)?;
        let mut scale_context = ScaleContext::new();
        let images = EMOJIS
            .iter()
            .filter_map(|emoji| {
                render_emoji(&font, &mut scale_context, emoji).map(|image| (*emoji, image))
            })
            .collect();
        Some(Self { images })
    }

    pub fn get(&self, emoji: &str) -> Image {
        self.images.get(emoji).cloned().unwrap_or_default()
    }
}

fn emoji_font_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        paths.push(PathBuf::from(windows).join("Fonts").join("seguiemj.ttf"));
    }
    paths.push(PathBuf::from(r"C:\Windows\Fonts\seguiemj.ttf"));
    paths.push(PathBuf::from("/mnt/c/Windows/Fonts/seguiemj.ttf"));
    paths.push(PathBuf::from(
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    ));
    paths
}

fn render_emoji(
    font: &FontRef<'_>,
    scale_context: &mut ScaleContext,
    emoji: &str,
) -> Option<Image> {
    let character = emoji.chars().find(|character| *character != '\u{fe0f}')?;
    let glyph_id = font.charmap().map(character);
    let mut scaler = scale_context
        .builder(*font)
        .size(20.0)
        .hint(true)
        .build();
    let rendered = Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .render(&mut scaler, glyph_id)?;

    let source_width = rendered.placement.width as usize;
    let source_height = rendered.placement.height as usize;
    if source_width == 0 || source_height == 0 {
        return None;
    }

    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(ICON_SIZE, ICON_SIZE);
    let pixels = buffer.make_mut_slice();
    pixels.fill(Rgba8Pixel::new(0, 0, 0, 0));
    let copy_width = source_width.min(ICON_SIZE as usize);
    let copy_height = source_height.min(ICON_SIZE as usize);
    let source_x = source_width.saturating_sub(copy_width) / 2;
    let source_y = source_height.saturating_sub(copy_height) / 2;
    let target_x = (ICON_SIZE as usize - copy_width) / 2;
    let target_y = (ICON_SIZE as usize - copy_height) / 2;
    let fallback = fallback_color(emoji);

    for y in 0..copy_height {
        for x in 0..copy_width {
            let source_index = (source_y + y) * source_width + source_x + x;
            let target_index = (target_y + y) * ICON_SIZE as usize + target_x + x;
            pixels[target_index] = match rendered.content {
                Content::Color | Content::SubpixelMask => {
                    let index = source_index * 4;
                    Rgba8Pixel::new(
                        rendered.data[index],
                        rendered.data[index + 1],
                        rendered.data[index + 2],
                        rendered.data[index + 3],
                    )
                }
                Content::Mask => {
                    let alpha = rendered.data[source_index];
                    Rgba8Pixel::new(fallback[0], fallback[1], fallback[2], alpha)
                }
            };
        }
    }

    Some(Image::from_rgba8(buffer))
}

fn fallback_color(emoji: &str) -> [u8; 3] {
    match emoji {
        "🌿" | "🧩" | "🐍" => [101, 196, 102],
        "📁" | "📂" | "📦" | "⚡" | "💡" => [226, 184, 107],
        "🔷" | "🌐" | "💻" | "📚" | "🗄️" => [97, 175, 239],
        "🦀" | "🧱" | "🐘" => [224, 108, 117],
        "🧪" | "🎨" | "🤖" | "💎" => [198, 120, 221],
        _ => [201, 206, 215],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_segoe_emoji_with_non_black_color_pixels() {
        let Some(data) = emoji_font_candidates()
            .into_iter()
            .find_map(|path| std::fs::read(path).ok())
        else {
            return;
        };
        let icons = EmojiIcons::from_font_data(&data).expect("emoji font should parse");
        assert_eq!(
            icons.images.len(),
            EMOJIS.len(),
            "every tree emoji should have a rendered image"
        );
        for emoji in EMOJIS {
            let pixels = icons.get(emoji).to_rgba8().expect("embedded RGBA icon");
            assert!(
                pixels.as_slice().iter().any(|pixel| pixel.a > 0),
                "{emoji} rendered as an empty image"
            );
        }
        let pixels = icons.get("🧩").to_rgba8().expect("embedded RGBA icon");
        assert!(pixels.as_slice().iter().any(|pixel| {
            pixel.a > 0 && (pixel.r > 0 || pixel.g > 0 || pixel.b > 0)
        }));
        assert!(pixels.as_slice().iter().any(|pixel| {
            pixel.a > 0 && !(pixel.r == pixel.g && pixel.g == pixel.b)
        }));
    }
}
