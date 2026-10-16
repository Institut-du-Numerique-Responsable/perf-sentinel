//! The Perf Sentinel logo of the HTML dashboard, read from its SVG. The
//! file only uses absolute `M`, `L`, `H`, `V`, `C` and `Z` commands, solid
//! fills and one two-stop linear gradient, which is all this reads.

use super::pages::Rgb;

const LOGO_SVG: &str = include_str!("../../html/logo-horiz-light.svg");

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Paint {
    Solid(Rgb),
    Gradient {
        from: (f32, f32, Rgb),
        to: (f32, f32, Rgb),
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogoPath {
    pub(crate) segments: Vec<Segment>,
    pub(crate) paint: Paint,
}

pub(crate) struct Logo {
    /// `min_x, min_y, width, height` of the SVG `viewBox`.
    pub(crate) view_box: [f32; 4],
    pub(crate) paths: Vec<LogoPath>,
}

/// The parsed logo. The SVG is bundled, so a parse failure is a bug the
/// tests catch, not an input error.
pub(crate) fn logo() -> Logo {
    let view_box = attribute(LOGO_SVG, "viewBox")
        .map(numbers)
        .and_then(|values| <[f32; 4]>::try_from(values).ok())
        .expect("the logo has a four-number viewBox");
    let gradient = gradient();
    let paths = LOGO_SVG
        .split("<path")
        .skip(1)
        .filter_map(|element| {
            let d = attribute(element, "d")?;
            let fill = attribute(element, "fill")?;
            let paint = if fill.starts_with("url(") {
                gradient?
            } else {
                Paint::Solid(hex(fill)?)
            };
            Some(LogoPath {
                segments: segments(d),
                paint,
            })
        })
        .collect();
    Logo { view_box, paths }
}

/// The value of `name="..."` in `element`, the first one found.
fn attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = element.find(&needle)? + needle.len();
    let end = element[start..].find('"')? + start;
    Some(&element[start..end])
}

fn hex(color: &str) -> Option<Rgb> {
    let digits = color.strip_prefix('#')?;
    let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

fn gradient() -> Option<Paint> {
    let start = LOGO_SVG.find("<linearGradient")?;
    let element = &LOGO_SVG[start..];
    let number = |name| attribute(element, name)?.parse::<f32>().ok();
    let mut stops = element
        .split("<stop")
        .skip(1)
        .filter_map(|stop| hex(attribute(stop, "stop-color")?));
    let (from, to) = (stops.next()?, stops.next()?);
    Some(Paint::Gradient {
        from: (number("x1")?, number("y1")?, from),
        to: (number("x2")?, number("y2")?, to),
    })
}

fn numbers(text: &str) -> Vec<f32> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|token| token.parse().ok())
        .collect()
}

/// Path data into segments, tracking the pen for `H` and `V`.
fn segments(d: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut pen = (0.0_f32, 0.0_f32);
    for (command, args) in commands(d) {
        let chunk = |size: usize| {
            args.chunks_exact(size)
                .map(<[f32]>::to_vec)
                .collect::<Vec<_>>()
        };
        match command {
            'M' | 'L' => {
                for (index, point) in chunk(2).into_iter().enumerate() {
                    pen = (point[0], point[1]);
                    out.push(if command == 'M' && index == 0 {
                        Segment::Move(pen.0, pen.1)
                    } else {
                        Segment::Line(pen.0, pen.1)
                    });
                }
            }
            'H' => args.iter().for_each(|x| {
                pen.0 = *x;
                out.push(Segment::Line(pen.0, pen.1));
            }),
            'V' => args.iter().for_each(|y| {
                pen.1 = *y;
                out.push(Segment::Line(pen.0, pen.1));
            }),
            'C' => {
                for c in chunk(6) {
                    pen = (c[4], c[5]);
                    out.push(Segment::Cubic(c[0], c[1], c[2], c[3], c[4], c[5]));
                }
            }
            'Z' => out.push(Segment::Close),
            _ => {}
        }
    }
    out
}

/// Each command letter with the numbers that follow it.
fn commands(d: &str) -> Vec<(char, Vec<f32>)> {
    let mut out: Vec<(char, Vec<f32>)> = Vec::new();
    let mut number = String::new();
    let flush = |number: &mut String, out: &mut Vec<(char, Vec<f32>)>| {
        if let (Ok(value), Some(last)) = (number.parse::<f32>(), out.last_mut()) {
            last.1.push(value);
        }
        number.clear();
    };
    for c in d.chars() {
        match c {
            'A'..='Z' | 'a'..='z' => {
                flush(&mut number, &mut out);
                out.push((c, Vec::new()));
            }
            '-' if !number.is_empty() => {
                flush(&mut number, &mut out);
                number.push(c);
            }
            '.' if number.contains('.') => {
                flush(&mut number, &mut out);
                number.push(c);
            }
            '0'..='9' | '.' | '-' => number.push(c),
            _ => flush(&mut number, &mut out),
        }
    }
    flush(&mut number, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_parses_into_its_sixteen_paths() {
        let logo = logo();
        assert_eq!([30.0, 26.0, 296.0, 88.0], logo.view_box);
        assert_eq!(16, logo.paths.len());
        assert!(
            logo.paths
                .iter()
                .all(|path| matches!(path.segments.first(), Some(Segment::Move(..))))
        );
        assert!(matches!(logo.paths[0].paint, Paint::Gradient { .. }));
    }

    #[test]
    fn path_data_reads_implicit_separators() {
        assert_eq!(
            vec![
                Segment::Move(1.5, -2.0),
                Segment::Line(3.0, 0.5),
                Segment::Line(3.0, 4.0),
                Segment::Close
            ],
            segments("M1.5-2L3 .5V4Z")
        );
    }
}
