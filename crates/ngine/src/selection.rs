pub const SMALLEST_SIDE: u32 = 32;

/// A rectangle of a screen in its pixels, from its top-left. Every edge sits
/// on an even pixel, as video encoders need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The pixels a screen has, rounded down to even numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub width: u32,
    pub height: u32,
}

/// A point on a screen, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// What a press on the selection takes hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Inside,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Grip {
    fn moves_left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    fn moves_right(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }

    fn moves_top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    fn moves_bottom(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }
}

impl Bounds {
    pub fn of(width: u32, height: u32) -> Self {
        Self {
            width: width & !1,
            height: height & !1,
        }
    }
}

impl Point {
    /// The point `x`, `y` in points, on a screen whose points each cover
    /// `pixels_per_point` pixels across.
    pub fn from_points(x: f32, y: f32, pixels_per_point: f32) -> Self {
        Self {
            x: f64::from(x) * f64::from(pixels_per_point),
            y: f64::from(y) * f64::from(pixels_per_point),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=most, which a u32 holds"
)]
fn even_pixel(value: f64, most: u32) -> u32 {
    (value.round().clamp(0.0, f64::from(most)) as u32) & !1
}

fn span(one: f64, other: f64, most: u32) -> (u32, u32) {
    let start = even_pixel(one.min(other), most);
    let end = even_pixel(one.max(other), most).max(start.saturating_add(2).min(most));
    (end.saturating_sub(2).min(start), end)
}

impl Area {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(self) -> u32 {
        self.x + self.width
    }

    fn bottom(self) -> u32 {
        self.y + self.height
    }

    /// The area a drag from `from` to `to` draws, kept on the screen.
    pub fn drawn(from: Point, to: Point, bounds: Bounds) -> Self {
        let (x, right) = span(from.x, to.x, bounds.width);
        let (y, bottom) = span(from.y, to.y, bounds.height);
        Self::new(x, y, right - x, bottom - y)
    }

    /// Whether the area is large enough to record.
    pub fn is_usable(self) -> bool {
        self.width >= SMALLEST_SIDE && self.height >= SMALLEST_SIDE
    }

    /// The area moved by `dx`, `dy` pixels, stopping at the screen's edges.
    #[must_use]
    pub fn moved(self, dx: f64, dy: f64, bounds: Bounds) -> Self {
        let room_x = bounds.width.saturating_sub(self.width);
        let room_y = bounds.height.saturating_sub(self.height);
        Self {
            x: even_pixel(f64::from(self.x) + dx, room_x),
            y: even_pixel(f64::from(self.y) + dy, room_y),
            ..self
        }
    }

    /// The area with the edges `grip` holds moved by `dx`, `dy` pixels. The
    /// edges stop at the screen's edges and never come closer than
    /// [`SMALLEST_SIDE`] to the edge across from them.
    #[must_use]
    pub fn resized(self, grip: Grip, dx: f64, dy: f64, bounds: Bounds) -> Self {
        if grip == Grip::Inside {
            return self.moved(dx, dy, bounds);
        }
        let (mut left, mut right) = (self.x, self.right());
        let (mut top, mut bottom) = (self.y, self.bottom());
        if grip.moves_left() {
            left = even_pixel(f64::from(left) + dx, right.saturating_sub(SMALLEST_SIDE));
        }
        if grip.moves_right() {
            right = even_pixel(f64::from(right) + dx, bounds.width).max(left + SMALLEST_SIDE);
        }
        if grip.moves_top() {
            top = even_pixel(f64::from(top) + dy, bottom.saturating_sub(SMALLEST_SIDE));
        }
        if grip.moves_bottom() {
            bottom = even_pixel(f64::from(bottom) + dy, bounds.height).max(top + SMALLEST_SIDE);
        }
        Self::new(left, top, right - left, bottom - top)
    }

    /// What a press at `point` takes hold of: an edge or a corner within
    /// `reach` pixels of it, or the inside. `None` outside the area.
    pub fn grip_at(self, point: Point, reach: f64) -> Option<Grip> {
        let near = |edge: u32, at: f64| (f64::from(edge) - at).abs() <= reach;
        let along = |start: u32, end: u32, at: f64| {
            at >= f64::from(start) - reach && at <= f64::from(end) + reach
        };
        let across = along(self.x, self.right(), point.x);
        let down = along(self.y, self.bottom(), point.y);
        let (left, right) = (near(self.x, point.x), near(self.right(), point.x));
        let (top, bottom) = (near(self.y, point.y), near(self.bottom(), point.y));
        let grip = match (left, right, top, bottom) {
            (true, _, true, _) => Grip::TopLeft,
            (_, true, true, _) => Grip::TopRight,
            (true, _, _, true) => Grip::BottomLeft,
            (_, true, _, true) => Grip::BottomRight,
            (true, ..) if down => Grip::Left,
            (_, true, ..) if down => Grip::Right,
            (_, _, true, _) if across => Grip::Top,
            (_, _, _, true) if across => Grip::Bottom,
            _ if self.contains(point) => Grip::Inside,
            _ => return None,
        };
        Some(grip)
    }

    fn contains(self, point: Point) -> bool {
        point.x >= f64::from(self.x)
            && point.x <= f64::from(self.right())
            && point.y >= f64::from(self.y)
            && point.y <= f64::from(self.bottom())
    }

    /// The area as it fits on a screen of `bounds`, when enough of it does
    /// to record.
    pub fn on(self, bounds: Bounds) -> Option<Self> {
        let x = self.x & !1;
        let y = self.y & !1;
        let right = self.right().min(bounds.width) & !1;
        let bottom = self.bottom().min(bounds.height) & !1;
        let fitted = Self::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y));
        fitted.is_usable().then_some(fitted)
    }

    /// `[x, y, width, height]` in points, on a screen whose points each
    /// cover `pixels_per_point` pixels across.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a screen's pixels are far below the 2^24 an f32 holds exactly"
    )]
    pub fn in_points(self, pixels_per_point: f32) -> [f32; 4] {
        let scale = pixels_per_point.max(f32::EPSILON);
        [
            self.x as f32 / scale,
            self.y as f32 / scale,
            self.width as f32 / scale,
            self.height as f32 / scale,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Bounds = Bounds {
        width: 2880,
        height: 1800,
    };

    fn at(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn is_even(area: Area) -> bool {
        [area.x, area.y, area.width, area.height]
            .iter()
            .all(|value| value % 2 == 0)
    }

    #[test]
    fn a_drag_draws_an_area_on_even_pixels_whichever_way_it_goes() {
        let forward = Area::drawn(at(101.0, 51.0), at(1381.0, 771.0), SCREEN);
        let backward = Area::drawn(at(1381.0, 771.0), at(101.0, 51.0), SCREEN);

        assert_eq!(forward, backward);
        assert!(is_even(forward));
        assert_eq!(forward, Area::new(100, 50, 1280, 720));
    }

    #[test]
    fn a_drag_past_the_screen_stops_at_its_edges() {
        let area = Area::drawn(at(-300.0, -20.0), at(4000.0, 900.0), SCREEN);

        assert_eq!(area, Area::new(0, 0, 2880, 900));
    }

    #[test]
    fn a_click_without_a_drag_draws_nothing_usable() {
        let area = Area::drawn(at(500.0, 500.0), at(500.0, 500.0), SCREEN);

        assert!(!area.is_usable());
        assert_eq!(area.width, 2);
    }

    #[test]
    fn a_drag_at_the_far_edge_keeps_its_area_on_the_screen() {
        let area = Area::drawn(at(2880.0, 1800.0), at(2880.0, 1800.0), SCREEN);

        assert_eq!(area, Area::new(2878, 1798, 2, 2));
    }

    #[test]
    fn moving_keeps_the_size_and_stops_at_the_edges() {
        let area = Area::new(100, 100, 1280, 720);

        assert_eq!(
            area.moved(51.0, -21.0, SCREEN),
            Area::new(150, 78, 1280, 720)
        );
        assert_eq!(
            area.moved(5000.0, 5000.0, SCREEN),
            Area::new(1600, 1080, 1280, 720)
        );
        assert_eq!(
            area.moved(-5000.0, -5000.0, SCREEN),
            Area::new(0, 0, 1280, 720)
        );
    }

    #[test]
    fn an_edge_moves_alone_and_a_corner_moves_two_edges() {
        let area = Area::new(100, 100, 1280, 720);

        assert_eq!(
            area.resized(Grip::Right, 21.0, 300.0, SCREEN),
            Area::new(100, 100, 1300, 720)
        );
        assert_eq!(
            area.resized(Grip::Top, 300.0, -40.0, SCREEN),
            Area::new(100, 60, 1280, 760)
        );
        assert_eq!(
            area.resized(Grip::BottomLeft, -100.0, 100.0, SCREEN),
            Area::new(0, 100, 1380, 820)
        );
    }

    #[test]
    fn resizing_stops_at_the_screen_and_at_the_smallest_side() {
        let area = Area::new(100, 100, 1280, 720);

        assert_eq!(
            area.resized(Grip::BottomRight, 9000.0, 9000.0, SCREEN),
            Area::new(100, 100, 2780, 1700)
        );
        let squeezed = area.resized(Grip::Left, 5000.0, 0.0, SCREEN);
        assert_eq!(squeezed.width, SMALLEST_SIDE);
        assert_eq!(squeezed.right(), area.right());
        let flattened = area.resized(Grip::Bottom, 0.0, -5000.0, SCREEN);
        assert_eq!(flattened.height, SMALLEST_SIDE);
        assert_eq!(flattened.y, area.y);
    }

    #[test]
    fn a_press_finds_the_corner_the_edge_or_the_inside() {
        let area = Area::new(100, 100, 400, 200);

        assert_eq!(area.grip_at(at(98.0, 103.0), 8.0), Some(Grip::TopLeft));
        assert_eq!(area.grip_at(at(505.0, 300.0), 8.0), Some(Grip::BottomRight));
        assert_eq!(area.grip_at(at(300.0, 104.0), 8.0), Some(Grip::Top));
        assert_eq!(area.grip_at(at(500.0, 200.0), 8.0), Some(Grip::Right));
        assert_eq!(area.grip_at(at(300.0, 200.0), 8.0), Some(Grip::Inside));
        assert_eq!(area.grip_at(at(50.0, 50.0), 8.0), None);
        assert_eq!(area.grip_at(at(300.0, 320.0), 8.0), None);
    }

    #[test]
    fn a_retina_screen_has_two_pixels_for_every_point() {
        let point = Point::from_points(640.5, 360.0, 2.0);
        let area = Area::drawn(Point::from_points(10.0, 20.0, 2.0), point, SCREEN);

        assert_eq!(point, at(1281.0, 720.0));
        assert_eq!(area, Area::new(20, 40, 1260, 680));
        assert_eq!(area.in_points(2.0), [10.0, 20.0, 630.0, 340.0]);
    }

    #[test]
    fn a_remembered_area_is_fitted_to_a_smaller_screen_or_dropped() {
        let smaller = Bounds::of(1921, 1081);

        assert_eq!(
            smaller,
            Bounds {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            Area::new(1600, 900, 1000, 600).on(smaller),
            Some(Area::new(1600, 900, 320, 180))
        );
        assert_eq!(Area::new(1900, 1060, 400, 400).on(smaller), None);
        assert_eq!(
            Area::new(100, 100, 640, 360).on(smaller),
            Some(Area::new(100, 100, 640, 360))
        );
    }
}
