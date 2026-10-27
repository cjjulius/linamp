//! Parser for a classic skin's `REGION.TXT`, the window-shape masks. Each
//! section defines one or more polygons (outside-edge pixel coordinates, listed
//! clockwise); the window's visible/clickable area is the union of its polygons.
//! Anything outside every polygon is cut away. Coordinates are in native skin
//! pixels. Sections we care about: the main window (normal + window-shade), the
//! equalizer (normal + window-shade) and the playlist. The video and
//! mini-browser sections are ignored (LinAmp has neither).

/// A polygon as a flat list of (x, y) vertices in native skin pixels.
pub type Polygon = Vec<(i32, i32)>;

/// Window-shape masks parsed from `REGION.TXT`. An empty vector means the skin
/// defines no mask for that state, so the window stays rectangular.
#[derive(Default, Clone, Debug)]
pub struct SkinRegions {
    pub main: Vec<Polygon>,
    pub main_shade: Vec<Polygon>,
    pub eq: Vec<Polygon>,
    pub eq_shade: Vec<Polygon>,
    pub playlist: Vec<Polygon>,
}

impl SkinRegions {
    /// True when the skin defines no masks at all (every window is rectangular).
    pub fn is_empty(&self) -> bool {
        self.main.is_empty()
            && self.main_shade.is_empty()
            && self.eq.is_empty()
            && self.eq_shade.is_empty()
            && self.playlist.is_empty()
    }
}

/// Parse the text of a `REGION.TXT` file. Unknown sections and comment lines
/// (`;` … or blank) are ignored. Tolerant of the real-world variations: section
/// names are matched case-insensitively, and `NumPoints` may list several
/// polygon sizes (`NumPoints=8,12`) that partition the flat `PointList`.
pub fn parse(text: &str) -> SkinRegions {
    let mut regions = SkinRegions::default();
    let mut section: Option<String> = None;
    let mut num_points: Vec<usize> = Vec::new();
    let mut point_list: Vec<i32> = Vec::new();

    // Commit the accumulated NumPoints/PointList into the current section.
    let commit = |regions: &mut SkinRegions,
                  section: &Option<String>,
                  num_points: &[usize],
                  point_list: &[i32]| {
        let Some(sec) = section else { return };
        let polys = split_polygons(num_points, point_list);
        if polys.is_empty() {
            return;
        }
        match sec.as_str() {
            "normal" => regions.main = polys,
            "windowshade" => regions.main_shade = polys,
            "equalizer" => regions.eq = polys,
            "equalizerws" => regions.eq_shade = polys,
            "playlist" => regions.playlist = polys,
            _ => {}
        }
    };

    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // New section: commit the previous one first.
            commit(&mut regions, &section, &num_points, &point_list);
            section = Some(name.trim().to_ascii_lowercase());
            num_points.clear();
            point_list.clear();
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "numpoints" => {
                num_points = val
                    .split(',')
                    .filter_map(|n| n.trim().parse::<usize>().ok())
                    .collect();
            }
            "pointlist" => {
                point_list = val
                    .split(',')
                    .filter_map(|n| n.trim().parse::<i32>().ok())
                    .collect();
            }
            _ => {}
        }
    }
    commit(&mut regions, &section, &num_points, &point_list);
    regions
}

/// Drop a trailing `;` comment (the format uses `;` for comments, including
/// inline ones after a value).
fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Split the flat `PointList` (x,y,x,y,…) into polygons. When `num_points` lists
/// several sizes, each size is a polygon's vertex count; otherwise the whole
/// list is one polygon. Sizes that would overrun the available points are
/// clamped so malformed files can't panic.
fn split_polygons(num_points: &[usize], point_list: &[i32]) -> Vec<Polygon> {
    let all: Vec<(i32, i32)> = point_list.chunks_exact(2).map(|c| (c[0], c[1])).collect();
    if all.len() < 3 {
        return Vec::new();
    }
    // One polygon when NumPoints is absent or a single count.
    let sizes: Vec<usize> = if num_points.len() <= 1 {
        vec![all.len()]
    } else {
        num_points.to_vec()
    };
    let mut polys = Vec::new();
    let mut i = 0;
    for &n in &sizes {
        if n < 3 || i >= all.len() {
            i += n;
            continue;
        }
        let end = (i + n).min(all.len());
        polys.push(all[i..end].to_vec());
        i = end;
    }
    // If the declared sizes didn't consume everything (or were bogus), fall back
    // to treating the whole list as a single polygon.
    if polys.is_empty() {
        polys.push(all);
    }
    polys
}

/// Rasterize the union of `polys` into horizontal rectangles (native pixels),
/// one run per scanline. Used to drive the X11 Shape extension, which takes a
/// rectangle list. Even-odd fill; points outside `w`×`h` are clamped.
pub fn polygons_to_rects(polys: &[Polygon], w: u32, h: u32) -> Vec<(i32, i32, i32, i32)> {
    if polys.is_empty() || w == 0 || h == 0 {
        return Vec::new();
    }
    let (w, h) = (w as i32, h as i32);
    let mut rects = Vec::new();
    for y in 0..h {
        let cy = y as f32 + 0.5;
        let mut inside = vec![false; w as usize];
        for poly in polys {
            let n = poly.len();
            if n < 3 {
                continue;
            }
            // X-coordinates where this row crosses the polygon's edges.
            let mut xs: Vec<f32> = Vec::new();
            for i in 0..n {
                let (x1, y1) = poly[i];
                let (x2, y2) = poly[(i + 1) % n];
                let (y1f, y2f) = (y1 as f32, y2 as f32);
                if (y1f <= cy && y2f > cy) || (y2f <= cy && y1f > cy) {
                    let t = (cy - y1f) / (y2f - y1f);
                    xs.push(x1 as f32 + t * (x2 - x1) as f32);
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut k = 0;
            while k + 1 < xs.len() {
                let xa = xs[k].round() as i32;
                let xb = xs[k + 1].round() as i32;
                for x in xa.max(0)..xb.min(w) {
                    inside[x as usize] = true;
                }
                k += 2;
            }
        }
        let mut x = 0;
        while x < w {
            if inside[x as usize] {
                let s = x;
                while x < w && inside[x as usize] {
                    x += 1;
                }
                rects.push((s, y, x - s, 1));
            } else {
                x += 1;
            }
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterize_full_rectangle() {
        let sq = vec![vec![(0, 0), (4, 0), (4, 4), (0, 4)]];
        let rects = polygons_to_rects(&sq, 4, 4);
        assert_eq!(rects, vec![(0, 0, 4, 1), (0, 1, 4, 1), (0, 2, 4, 1), (0, 3, 4, 1)]);
    }

    #[test]
    fn rasterize_empty_when_no_polys() {
        assert!(polygons_to_rects(&[], 10, 10).is_empty());
    }

    #[test]
    fn rasterize_notched_shape_leaves_a_gap() {
        // A 10-wide row with a square hole isn't expressible as one polygon here,
        // but two side-by-side squares produce two runs on each covered row.
        let left = vec![(0, 0), (3, 0), (3, 2), (0, 2)];
        let right = vec![(6, 0), (9, 0), (9, 2), (6, 2)];
        let rects = polygons_to_rects(&[left, right], 10, 2);
        // Row 0: [0,3) and [6,9)
        assert!(rects.contains(&(0, 0, 3, 1)));
        assert!(rects.contains(&(6, 0, 3, 1)));
    }

    #[test]
    fn empty_or_comment_only_yields_no_masks() {
        let txt = "; just a comment\n\n;[Normal]\n;NumPoints=4\n;PointList=0,0, 1,0, 1,1, 0,1\n";
        assert!(parse(txt).is_empty());
    }

    #[test]
    fn single_polygon_main_section() {
        let txt = "[Normal]\nNumPoints=4\nPointList=0,0, 275,0, 275,116, 0,116\n";
        let r = parse(txt);
        assert_eq!(r.main.len(), 1);
        assert_eq!(r.main[0], vec![(0, 0), (275, 0), (275, 116), (0, 116)]);
        assert!(r.eq.is_empty() && r.playlist.is_empty());
    }

    #[test]
    fn multiple_polygons_split_by_numpoints() {
        // Two triangles: NumPoints=3,3 partitions the 6-point list.
        let txt = "[Equalizer]\nNumPoints=3,3\nPointList=0,0, 10,0, 0,10,  100,0, 110,0, 100,10\n";
        let r = parse(txt);
        assert_eq!(r.eq.len(), 2);
        assert_eq!(r.eq[0], vec![(0, 0), (10, 0), (0, 10)]);
        assert_eq!(r.eq[1], vec![(100, 0), (110, 0), (100, 10)]);
    }

    #[test]
    fn inline_comments_and_sections() {
        let txt = "[WindowShade]  ; main shade\nNumPoints=4  ; do last\nPointList=0,0, 275,0, 275,14, 0,14\n";
        let r = parse(txt);
        assert_eq!(r.main_shade.len(), 1);
        assert_eq!(r.main_shade[0].len(), 4);
    }

    #[test]
    fn section_names_are_case_insensitive() {
        let txt = "[playlist]\nNumPoints=3\nPointList=0,0, 5,0, 0,5\n";
        assert_eq!(parse(txt).playlist.len(), 1);
    }

    #[test]
    fn rasterize_concave_polygon_splits_a_row() {
        // A U-shape: a 10x10 square with a notch cut up from the bottom at x in [3,7).
        let u = vec![
            (0, 0),
            (10, 0),
            (10, 10),
            (7, 10),
            (7, 4),
            (3, 4),
            (3, 10),
            (0, 10),
        ];
        let rects = polygons_to_rects(&[u], 10, 10);
        // Above the notch the row is solid.
        assert!(rects.contains(&(0, 2, 10, 1)));
        // Inside the notch the even-odd fill splits the row into two runs.
        assert!(rects.contains(&(0, 7, 3, 1)));
        assert!(rects.contains(&(7, 7, 3, 1)));
        // Nothing fills the notch gap on that row.
        assert!(!rects.iter().any(|&(x, y, wd, _)| y == 7 && x <= 4 && x + wd > 5));
    }

    #[test]
    fn rasterize_clamps_vertices_outside_the_window() {
        // A polygon larger than the window is clamped to w x h.
        let big = vec![(-5, -5), (20, -5), (20, 20), (-5, 20)];
        let rects = polygons_to_rects(&[big], 4, 3);
        assert_eq!(rects, vec![(0, 0, 4, 1), (0, 1, 4, 1), (0, 2, 4, 1)]);
    }
}
