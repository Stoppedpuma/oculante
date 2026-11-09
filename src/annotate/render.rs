//! Drawing annotations onto the edited image and keeping them drawn there as they change.

use image::{DynamicImage, GenericImageView};
use rayon::prelude::*;

use super::*;

/// Length along the arrow and half the width of its head: two lines back from the tip at
/// right angles to each other
pub(super) fn arrow_head(width: f32, length: f32) -> (f32, f32) {
    let head = (width * 2. + 6.).min(length * 0.45);
    (head, head)
}

/// The three points of an arrowhead at `to`: the tip and the ends of its two lines. The tip
/// sits half a line back from `to`, so the round line around it ends at `to`.
pub(super) fn arrow_head_corners(to: [i32; 2], width: f32) -> Option<[Point; 3]> {
    let (x, y) = (to[0] as f32, to[1] as f32);
    let length = x.hypot(y);
    if length < 1. {
        return None;
    }
    let (dx, dy) = (x / length, y / length);
    let back = (width / 2.).min(length * 0.3);
    let (tx, ty) = (x - dx * back, y - dy * back);
    let (head, half) = arrow_head(width, length);
    let (bx, by) = (tx - dx * head, ty - dy * head);
    Some([
        Point::from_xy(tx, ty),
        Point::from_xy(bx - dy * half, by + dx * half),
        Point::from_xy(bx + dy * half, by - dx * half),
    ])
}

/// An arrow from where it starts to `to` as one path: the line to the tip and the two lines of
/// the head. Stroked at once, where they meet is drawn once, so it is as see-through there as
/// anywhere.
fn arrow(to: [i32; 2], width: f32) -> Option<Path> {
    let [tip, left, right] = arrow_head_corners(to, width)?;
    let mut path = PathBuilder::new();
    path.move_to(0., 0.);
    path.line_to(tip.x, tip.y);
    path.move_to(left.x, left.y);
    path.line_to(tip.x, tip.y);
    path.line_to(right.x, right.y);
    path.finish()
}

/// The part of the edited image an annotation draws on
pub(super) fn area(a: &Annotation, t: Transform, size: (u32, u32)) -> Option<Rect> {
    // a spotlight dims all of the image
    if a.is_spotlight() {
        return (size.0 > 0 && size.1 > 0 && !a.is_empty()).then_some(Rect {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        });
    }
    // smoothed edges reach a little further
    let smooth = if matches!(a.shape, Shape::Redact { .. }) {
        0.
    } else {
        2.
    };
    in_pixels(extent(a, t, a.reach() + a.shadow_reach())?, smooth, size)
}

/// Of a spotlight, its spot and its smooth edge: when only the spot moves, turns or changes size,
/// that is what changes of the image, where it was and where it is
fn spot_area(a: &Annotation, t: Transform, size: (u32, u32)) -> Option<Rect> {
    if !a.is_spotlight() || a.is_empty() {
        return None;
    }
    in_pixels(extent(a, t, 0.)?, 2., size)
}

/// The pixels of the image a box touches, `smooth` further on all sides, none outside it
fn in_pixels([left, top, right, bottom]: [f32; 4], smooth: f32, size: (u32, u32)) -> Option<Rect> {
    let (left, top) = (
        (left - smooth).floor().max(0.),
        (top - smooth).floor().max(0.),
    );
    let (right, bottom) = (
        (right + smooth).ceil().min(size.0 as f32),
        (bottom + smooth).ceil().min(size.1 as f32),
    );
    (right > left && bottom > top).then_some(Rect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

/// Draws an annotation, which reaches no further than `area`
pub(super) fn draw<P: Pixel<Subpixel = u8>>(
    img: &mut Pixels<P>,
    a: &Annotation,
    t: Transform,
    area: Rect,
) {
    if let Shape::Redact { style, .. } = &a.shape {
        match style {
            Redaction::Pixelate => {
                pixelate(img, area, (a.width as f32 * scale_of(t)).round() as u32)
            }
            Redaction::Fill => {
                let [r, g, b, _] = a.color;
                let color = *P::from_slice(&[r, g, b, 255][..channels::<P>()]);
                for y in area.y..area.y + area.height {
                    for x in area.x..area.x + area.width {
                        img.put_pixel(x, y, color);
                    }
                }
            }
        }
        return;
    }
    // a shadow is blurred from all that is drawn, also beyond the edge of the image: the
    // layer reaches as far past `area` as the shadow does
    let spread = a.shadow_size as f32 * scale_of(t);
    let margin = if a.casts_shadow() {
        spread.ceil() as u32 + 1
    } else {
        0
    };
    let (width, height) = (area.width + 2 * margin, area.height + 2 * margin);
    let Some(mut layer) = Pixmap::new(width, height) else {
        return;
    };
    let t = t.post_translate(margin as f32 - area.x as f32, margin as f32 - area.y as f32);
    // What it draws and its outline are each drawn opaque first, and made see-through as they
    // are put together, the outline only shows around it, not through it, as does its shadow.
    trace(&mut layer, a, t, false);
    let outline = if a.outline_width() > 0. {
        let Some(mut outline) = Pixmap::new(width, height) else {
            return;
        };
        trace(&mut outline, a, t, true);
        Some(outline)
    } else {
        None
    };
    let covered = knock_out(&mut layer, outline.as_ref(), a.color[3], a.outline_color[3]);
    if margin > 0 {
        add_shadow(&mut layer, &covered, a.shadow, spread);
    }
    // the number of a counter goes over it in its own color
    if let Shape::Counter(number) = &a.shape
        && let Some(digits) = counter_digits(*number, a.width as f32)
    {
        layer.fill_path(&digits, &paint(a.number_color), FillRule::Winding, t, None);
    }
    if margin > 0 {
        let inside =
            tiny_skia::IntRect::from_xywh(margin as i32, margin as i32, area.width, area.height);
        match inside.and_then(|inside| layer.clone_rect(inside)) {
            Some(inside) => layer = inside,
            None => return,
        }
    }
    blend(img, &layer, area);
}

fn paint([r, g, b, alpha]: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, alpha);
    paint.anti_alias = true;
    paint
}

/// Draws what an annotation covers, or with `outline` what its outline covers, in its color or
/// the color of its outline but opaque: as far as it covers a pixel, that is its alpha. The
/// outline covers what it is around as well. The number of a counter is not drawn.
fn trace(layer: &mut Pixmap, a: &Annotation, t: Transform, outline: bool) {
    let opaque = |[r, g, b, _]: [u8; 4]| paint([r, g, b, 255]);
    let (paint, outline) = if outline {
        (opaque(a.outline_color), a.outline_width())
    } else {
        (opaque(a.color), 0.)
    };
    let stroke = tiny_skia::Stroke {
        width: a.line_width() + outline * 2.,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    // a stamp and a counter are filled, their outline goes around them
    let fill_or_outline = |layer: &mut Pixmap, path: &Path| {
        if outline > 0. {
            let around = tiny_skia::Stroke {
                width: outline * 2.,
                ..stroke.clone()
            };
            layer.stroke_path(path, &paint, &around, t, None);
        }
        layer.fill_path(path, &paint, FillRule::Winding, t, None);
    };
    match &a.shape {
        Shape::Arrow(to) => {
            if let Some(path) = arrow(*to, a.line_width()) {
                layer.stroke_path(&path, &paint, &stroke, t, None);
            }
        }
        Shape::Line(to) => {
            if let Some(line) = polyline(&[[0, 0], *to]) {
                layer.stroke_path(&line, &paint, &stroke, t, None);
            }
        }
        Shape::Rect(size) => {
            if let Some(frame) = frame_path(*size, a.corner_radius()) {
                let stroke = tiny_skia::Stroke {
                    line_join: LineJoin::Miter,
                    ..stroke
                };
                layer.stroke_path(&frame, &paint, &stroke, t, None);
            }
        }
        Shape::Ellipse(size) => {
            let oval = tiny_skia::Rect::from_xywh(0., 0., size[0] as f32, size[1] as f32)
                .and_then(PathBuilder::from_oval);
            if let Some(oval) = oval {
                layer.stroke_path(&oval, &paint, &stroke, t, None);
            }
        }
        Shape::Pen(points) => {
            if let [only] = points.as_slice() {
                let dot =
                    PathBuilder::from_circle(only[0] as f32, only[1] as f32, stroke.width / 2.);
                if let Some(dot) = dot {
                    layer.fill_path(&dot, &paint, FillRule::Winding, t, None);
                }
            } else if let Some(line) = polyline(points) {
                layer.stroke_path(&line, &paint, &stroke, t, None);
            }
        }
        // the letters, or the letters with their outline in the color of the outline
        Shape::Text(text) => {
            let opaque = |[r, g, b, _]: [u8; 4]| [r, g, b, 255];
            let mut letters = a.clone();
            if outline > 0. {
                letters.color = opaque(a.outline_color);
                letters.outline_color = opaque(a.outline_color);
            } else {
                letters.color = opaque(a.color);
                letters.outline = 0;
            }
            if let Some(layout) = text_layout(text, &letters) {
                resvg::render(&layout.tree, t, &mut layer.as_mut());
            }
        }
        Shape::Stamp(stamp) => {
            if let Some(glyph) = stamp_path(*stamp, a.width as f32) {
                fill_or_outline(layer, &glyph);
            }
        }
        Shape::Counter(_) => {
            let r = a.width as f32 / 2.;
            if let Some(round) = PathBuilder::from_circle(r, r, r) {
                fill_or_outline(layer, &round);
            }
        }
        // spotlights dim all together, see `dim`
        Shape::Redact { .. } | Shape::Spotlight { .. } => {}
    }
}

/// Puts an annotation drawn opaque in `layer` together with its outline, also drawn opaque, and
/// makes them as see-through as their colors say. The outline shows only where the annotation
/// does not cover, so it does not show through it. Returns how much of each pixel the two
/// cover, from 0 to 1.
fn knock_out(
    layer: &mut Pixmap,
    outline: Option<&Pixmap>,
    alpha: u8,
    outline_alpha: u8,
) -> Vec<f32> {
    let (alpha, outline_alpha) = (alpha as f32 / 255., outline_alpha as f32 / 255.);
    let empty = [0u8; 4];
    let around = outline.map(|o| o.data().as_chunks::<4>().0);
    let mut covered = Vec::with_capacity((layer.width() * layer.height()) as usize);
    for (i, p) in layer
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let o = around.map_or(&empty, |around| &around[i]);
        if p[3] == 0 && o[3] == 0 {
            covered.push(0.);
            continue;
        }
        // as much as it covers of the pixel, the outline covers of the rest
        let inside = p[3] as f32 / 255.;
        let rest = (1. - inside) * outline_alpha;
        let mut out = [0u8; 4];
        for c in 0..4 {
            out[c] = (p[c] as f32 * alpha + o[c] as f32 * rest).round().min(255.) as u8;
        }
        // its colors are multiplied by alpha, none is more than alpha
        for c in 0..3 {
            out[c] = out[c].min(out[3]);
        }
        *p = out;
        covered.push(inside + (1. - inside) * o[3] as f32 / 255.);
    }
    covered
}

/// A frame this large, its corners rounded by `radius`
pub(super) fn frame_path(size: [i32; 2], radius: f32) -> Option<Path> {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let mut frame = PathBuilder::new();
    if radius <= 0. {
        frame.move_to(0., 0.);
        frame.line_to(w, 0.);
        frame.line_to(w, h);
        frame.line_to(0., h);
    } else {
        // a quarter of a circle, as a curve
        let r = radius;
        let k = r * (1. - 0.552_284_8);
        frame.move_to(r, 0.);
        frame.line_to(w - r, 0.);
        frame.cubic_to(w - k, 0., w, k, w, r);
        frame.line_to(w, h - r);
        frame.cubic_to(w, h - k, w - k, h, w - r, h);
        frame.line_to(r, h);
        frame.cubic_to(k, h, 0., h - k, 0., h - r);
        frame.line_to(0., r);
        frame.cubic_to(0., k, k, 0., r, 0.);
    }
    frame.close();
    frame.finish()
}

/// A line through the points of a pen
pub(super) fn polyline(points: &[[i32; 2]]) -> Option<Path> {
    let mut line = PathBuilder::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            line.move_to(p[0] as f32, p[1] as f32);
        } else {
            line.line_to(p[0] as f32, p[1] as f32);
        }
    }
    line.finish()
}

/// Hides an area in blocks, each the average color of its pixels
fn pixelate<P: Pixel<Subpixel = u8>>(img: &mut Pixels<P>, area: Rect, block: u32) {
    let n = channels::<P>();
    let block = block.max(2);
    let (right, bottom) = (area.x + area.width, area.y + area.height);
    for top in (area.y..bottom).step_by(block as usize) {
        for left in (area.x..right).step_by(block as usize) {
            let (r, b) = ((left + block).min(right), (top + block).min(bottom));
            // weighted by alpha, transparent pixels have no color to give
            let mut sum = [0u64; 4];
            for y in top..b {
                for x in left..r {
                    let p = img.get_pixel(x, y).channels();
                    // without alpha the image is opaque
                    let alpha = p.get(3).map_or(255, |a| *a as u64);
                    for c in 0..3 {
                        sum[c] += p[c] as u64 * alpha;
                    }
                    sum[3] += alpha;
                }
            }
            let count = ((r - left) * (b - top)) as u64;
            let color = match sum[3] {
                0 => [0; 4],
                alpha => [
                    (sum[0] / alpha) as u8,
                    (sum[1] / alpha) as u8,
                    (sum[2] / alpha) as u8,
                    (alpha / count) as u8,
                ],
            };
            for y in top..b {
                for x in left..r {
                    img.put_pixel(x, y, *P::from_slice(&color[..n]));
                }
            }
        }
    }
}

/// Puts a layer, its colors multiplied by alpha, over a part of the image
fn blend<P: Pixel<Subpixel = u8>>(img: &mut Pixels<P>, layer: &Pixmap, area: Rect) {
    let n = channels::<P>();
    let stride = img.width() as usize * n;
    let (left, right) = (area.x as usize * n, (area.x + area.width) as usize * n);
    let rows = area.y as usize * stride..(area.y + area.height) as usize * stride;
    img.as_mut()[rows]
        .par_chunks_mut(stride)
        .zip(layer.data().par_chunks(area.width as usize * 4))
        .with_min_len(8)
        .for_each(|(row, src)| blend_row(&mut row[left..right], src, n));
}

/// One row of a layer over one row of the image with `n` channels
fn blend_row(dst: &mut [u8], src: &[u8], n: usize) {
    let (src, _) = src.as_chunks::<4>();
    for (d, s) in dst.chunks_exact_mut(n).zip(src) {
        match s[3] {
            0 => {}
            255 => d.copy_from_slice(&s[..n]),
            alpha => {
                let sa = alpha as f32 / 255.;
                // what shows of the pixel below, all of it without alpha
                let below = d.get(3).map_or(1., |a| *a as f32 / 255.) * (1. - sa);
                let out = sa + below;
                for c in 0..3 {
                    d[c] = ((s[c] as f32 + d[c] as f32 * below) / out).round() as u8;
                }
                if let Some(a) = d.get_mut(3) {
                    *a = (out * 255.).round() as u8;
                }
            }
        }
    }
}

/// Adds a shadow around the layer's content, which covers as much of each pixel as
/// `covered` says. The shadow is `dark` percent opaque at the edges and fades to nothing over
/// `spread` pixels. It is below the content, so it does not show through where the content is
/// see-through.
fn add_shadow(layer: &mut Pixmap, covered: &[f32], dark: u32, spread: f32) {
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    let mut alpha = covered.to_vec();
    // three box blurs are about a bell curve, together they reach `spread`
    let radius = ((spread / 3.).round() as usize).max(1);
    for _ in 0..3 {
        box_blur(&mut alpha, w, h, radius);
    }
    let dark = dark.min(100) as f32 / 100.;
    for ((p, blurred), covered) in layer
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(alpha)
        .zip(covered)
    {
        // half covered at an edge: as dark as it is set there
        let shade = dark * (blurred * 2.).clamp(0., 1.);
        let a = p[3] as f32 / 255.;
        // black where the content does not cover, its colors stay as they are multiplied by alpha
        p[3] = ((a + shade * (1. - covered)) * 255.).round().min(255.) as u8;
    }
}

/// Blurs by averaging each value with its neighbours within `radius`, first along
/// rows, then columns. Outside the image counts as zero, so edges fade.
fn box_blur(values: &mut [f32], w: usize, h: usize, radius: usize) {
    let size = (2 * radius + 1) as f32;
    let blur = |line: &[f32], out: &mut [f32]| {
        let n = line.len();
        let mut sum: f32 = line[..radius.min(n)].iter().sum();
        for i in 0..n {
            if i + radius < n {
                sum += line[i + radius];
            }
            out[i] = sum / size;
            if i >= radius {
                sum -= line[i - radius];
            }
        }
    };
    let mut line = vec![0f32; w.max(h)];
    let mut out = vec![0f32; w.max(h)];
    for y in 0..h {
        let row = &mut values[y * w..(y + 1) * w];
        line[..w].copy_from_slice(row);
        blur(&line[..w], &mut out[..w]);
        row.copy_from_slice(&out[..w]);
    }
    for x in 0..w {
        for y in 0..h {
            line[y] = values[y * w + x];
        }
        blur(&line[..h], &mut out[..h]);
        for y in 0..h {
            values[y * w + x] = out[y];
        }
    }
}

/// The area a spotlight leaves as it is, uses its own coordinates
fn spot_path(a: &Annotation) -> Option<Path> {
    match a.shape {
        Shape::Spotlight { size, oval: true } => {
            tiny_skia::Rect::from_xywh(0., 0., size[0] as f32, size[1] as f32)
                .and_then(PathBuilder::from_oval)
        }
        Shape::Spotlight { size, oval: false } => frame_path(size, a.corner_radius()),
        _ => None,
    }
}

/// Dims the image except for the spotlights, using the shade of the front most one.
/// `img` is the part of the edited image from `origin` on, `area` is in it.
fn dim<P: Pixel<Subpixel = u8>>(
    img: &mut Pixels<P>,
    spotlights: &[&Annotation],
    mapping: &Mapping,
    area: Rect,
    origin: (u32, u32),
) {
    let Some(front) = spotlights.last() else {
        return;
    };
    let Some(mut layer) = Pixmap::new(area.width, area.height) else {
        return;
    };
    let [r, g, b, alpha] = front.color;
    layer.fill(tiny_skia::Color::from_rgba8(r, g, b, alpha));
    // the spots are cut out of the shade, with smooth edges
    let cut = Paint {
        blend_mode: tiny_skia::BlendMode::Clear,
        anti_alias: true,
        ..Paint::default()
    };
    for a in spotlights {
        if let Some(spot) = spot_path(a) {
            let t = a
                .transform(mapping)
                .post_translate(-((origin.0 + area.x) as f32), -((origin.1 + area.y) as f32));
            layer.fill_path(&spot, &cut, FillRule::Winding, t, None);
        }
    }
    blend(img, &layer, area);
}

/// An annotation as the layer drew it
#[derive(Debug, Clone)]
struct Drawn {
    a: Annotation,
    /// What it covers of the image, none when it is not in it
    area: Option<Rect>,
    /// Of a spotlight, its spot, see `spot_area`
    spot: Option<Rect>,
}

impl Drawn {
    fn new(a: &Annotation, mapping: &Mapping, size: (u32, u32)) -> Self {
        let t = a.transform(mapping);
        Self {
            a: a.clone(),
            area: area(a, t, size),
            spot: spot_area(a, t, size),
        }
    }
}

/// The same spotlight and shade on an image of the same size, with its spot moved, turned or
/// resized.
fn only_the_spot_changed(was: &Drawn, is: &Drawn) -> bool {
    was.a.is_spotlight()
        && is.a.is_spotlight()
        && was.a.color == is.a.color
        && was.a.hidden == is.a.hidden
        && was.area == is.area
        && was.spot.is_some()
        && is.spot.is_some()
}

/// The annotations in the edited image, kept up to date by drawing only what changed
#[derive(Debug, Clone, Default)]
pub struct AnnotationLayer {
    /// What was under the annotations, where `base_area` says
    base: Base,
    base_area: Option<Rect>,
    /// The annotations as they are in the image, with the part each one drew on
    drawn: Vec<Drawn>,
    fonts: u32,
    /// Of the image they were drawn on
    size: (u32, u32),
}

impl AnnotationLayer {
    /// The image was made anew, without annotations
    pub fn reset(&mut self) {
        self.base_area = None;
        self.drawn.clear();
    }

    /// Whether any annotation is in the image
    pub fn is_empty(&self) -> bool {
        self.drawn.iter().all(|d| d.area.is_none())
    }

    /// Brings the annotations in the image up to date. Returns the part of the image
    /// that changed.
    pub fn update<P: Pixel<Subpixel = u8>>(
        &mut self,
        img: &mut Pixels<P>,
        annotations: &[Annotation],
        mapping: &Mapping,
    ) -> Option<Rect> {
        let dirty = self.changes(annotations, mapping, img.dimensions())?;
        let kept = self.base_area.map_or(dirty, |b| rect_union(b, dirty));
        keep_base(&mut self.base, &mut self.base_area, img, kept);
        restore(img, &self.base, dirty);
        self.paint(img, mapping, dirty, (0, 0));
        Some(dirty)
    }

    /// As `update`, on an image in any layout. One that is not 8 bit RGB or RGBA, made so by a
    /// color type operation, is drawn on in 8 bit RGBA where it changed, and only the pixels
    /// the annotations change are put back into it, in its own layout.
    pub fn update_image(
        &mut self,
        img: &mut DynamicImage,
        annotations: &[Annotation],
        mapping: &Mapping,
    ) -> Option<Rect> {
        match img {
            DynamicImage::ImageRgb8(pixels) => return self.update(pixels, annotations, mapping),
            DynamicImage::ImageRgba8(pixels) => return self.update(pixels, annotations, mapping),
            _ => {}
        }
        let size = img.dimensions();
        let dirty = self.changes(annotations, mapping, size)?;
        let n = img.color().bytes_per_pixel() as usize;
        let kept = self.base_area.map_or(dirty, |b| rect_union(b, dirty));
        keep_base_bytes(
            &mut self.base,
            &mut self.base_area,
            img.as_bytes(),
            size,
            n,
            kept,
        );
        restore_bytes(bytes_mut(img)?, size.0, n, &self.base, dirty);
        let before = img
            .crop_imm(dirty.x, dirty.y, dirty.width, dirty.height)
            .to_rgba8();
        let mut work = before.clone();
        self.paint(&mut work, mapping, dirty, (dirty.x, dirty.y));
        // what is not drawn on keeps all of its depth
        let changed: Vec<bool> = before
            .pixels()
            .zip(work.pixels())
            .map(|(was, is)| was != is)
            .collect();
        let mut drawn = DynamicImage::ImageRgba8(work);
        let color = img.color();
        convert_color_type(&mut drawn, &ColorTypeExt::from_image(color));
        if drawn.color() != color {
            return Some(dirty);
        }
        let drawn = drawn.as_bytes();
        let (width, stride) = (dirty.width as usize, size.0 as usize * n);
        let out = bytes_mut(img)?;
        for (i, _) in changed.iter().enumerate().filter(|(_, changed)| **changed) {
            let (x, y) = (dirty.x as usize + i % width, dirty.y as usize + i / width);
            let at = y * stride + x * n;
            out[at..at + n].copy_from_slice(&drawn[i * n..(i + 1) * n]);
        }
        Some(dirty)
    }

    /// Which annotations changed since they were last drawn, and the part of the image to draw
    /// again for them, none if nothing changed. Remembers them as they are now.
    fn changes(
        &mut self,
        annotations: &[Annotation],
        mapping: &Mapping,
        size: (u32, u32),
    ) -> Option<Rect> {
        let fonts = FONTS_VERSION.load(Ordering::Relaxed);
        // with other fonts, or on an image of another size, all is drawn again. Edits that change
        // the image under the annotations reset the layer.
        let everything = fonts != self.fonts || size != self.size;
        let mut dirty: Option<Rect> = None;
        let mut add = |area: Option<Rect>| {
            if let Some(area) = area {
                dirty = Some(dirty.map_or(area, |d| rect_union(d, area)));
            }
        };
        // what is as it was drawn keeps what the layer knows of it, without a copy; only what
        // changed is copied and measured again
        let mut before = std::mem::take(&mut self.drawn).into_iter();
        let mut now = Vec::with_capacity(annotations.len());
        for a in annotations {
            match before.next() {
                Some(was) if !everything && was.a == *a => now.push(was),
                was => {
                    let is = Drawn::new(a, mapping, size);
                    match &was {
                        // A spotlight whose spot moved, turned or resized keeps its shade.
                        // Only the spot's old and new positions differ.
                        Some(was) if !everything && only_the_spot_changed(was, &is) => {
                            add(was.spot);
                            add(is.spot);
                        }
                        _ => {
                            add(was.as_ref().and_then(|d| d.area));
                            add(is.area);
                        }
                    }
                    now.push(is);
                }
            }
        }
        // no longer in the list
        for was in before {
            add(was.area);
        }
        self.fonts = fonts;
        self.size = size;
        self.drawn = now;
        let mut dirty = dirty?;
        // Anything drawn again is drawn in full, since a redaction needs all of its blocks.
        // The spotlight shade covers the whole image, so it is drawn only where needed.
        loop {
            let mut grown = false;
            for area in self
                .drawn
                .iter()
                .filter(|d| !d.a.is_spotlight())
                .filter_map(|d| d.area)
            {
                if overlap(area, dirty) && rect_union(area, dirty) != dirty {
                    dirty = rect_union(area, dirty);
                    grown = true;
                }
            }
            if !grown {
                break;
            }
        }
        Some(dirty)
    }

    /// Draws the annotations where `dirty` is in the edited image, onto `img`: the part of it
    /// from `origin` on, with what is under the annotations there
    fn paint<P: Pixel<Subpixel = u8>>(
        &self,
        img: &mut Pixels<P>,
        mapping: &Mapping,
        dirty: Rect,
        origin: (u32, u32),
    ) {
        let (x, y) = (origin.0 as f32, origin.1 as f32);
        let shift = |area: Rect| Rect {
            x: area.x - origin.0,
            y: area.y - origin.1,
            ..area
        };
        let spotlights: Vec<&Annotation> = self
            .drawn
            .iter()
            .filter(|d| d.a.is_spotlight() && d.area.is_some())
            .map(|d| &d.a)
            .collect();
        let mut dimmed = false;
        for Drawn { a, area, .. } in &self.drawn {
            if a.is_spotlight() {
                // where the one furthest back is
                if area.is_some() && !std::mem::replace(&mut dimmed, true) {
                    dim(img, &spotlights, mapping, shift(dirty), origin);
                }
            } else if let Some(area) = area.filter(|area| overlap(*area, dirty)) {
                let t = a.transform(mapping).post_translate(-x, -y);
                draw(img, a, t, shift(area));
            }
        }
    }
}

/// The bytes of an image in any layout
fn bytes_mut(img: &mut DynamicImage) -> Option<&mut [u8]> {
    use DynamicImage::*;
    Some(match img {
        ImageLuma8(p) => p.as_mut(),
        ImageLumaA8(p) => p.as_mut(),
        ImageRgb8(p) => p.as_mut(),
        ImageRgba8(p) => p.as_mut(),
        ImageLuma16(p) => bytemuck::cast_slice_mut(p.as_mut()),
        ImageLumaA16(p) => bytemuck::cast_slice_mut(p.as_mut()),
        ImageRgb16(p) => bytemuck::cast_slice_mut(p.as_mut()),
        ImageRgba16(p) => bytemuck::cast_slice_mut(p.as_mut()),
        ImageRgb32F(p) => bytemuck::cast_slice_mut(p.as_mut()),
        ImageRgba32F(p) => bytemuck::cast_slice_mut(p.as_mut()),
        _ => return None,
    })
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}
