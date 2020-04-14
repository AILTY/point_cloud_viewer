//! Calculations with Web Mercator coordinates.

use alga::general::SupersetOf;
use nalgebra::{RealField, Vector2};
use nav_types::WGS84;
use serde::{Deserialize, Serialize};

const TILE_SIZE: u32 = 256;

/// The max zoom level is currently 23 because of an implementation choice,
/// namely fitting `TILE_SIZE << MAX_ZOOM` in an `u32`, but theoretically nothing
/// stops us from going deeper.
pub const MAX_ZOOM: u8 = 23;

/// A Web Mercator coordinate. Essentially a position in a 2D map of the world.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WebMercatorCoord<S: RealField> {
    /// Implementation detail: This is normalized to [0, 1), so not zoom level 0.
    /// This makes calculations a bit simpler.
    normalized: Vector2<S>,
}

impl<S: RealField> WebMercatorCoord<S> {
    /// The constant for the latitude at which the map is cut off.
    /// Equal to `2.0 * E.powf(PI).arctan() - FRAC_PI_2`.
    /// In degrees, it's 85.051129 (cf. [Wikipedia](https://en.wikipedia.org/wiki/Web_Mercator_projection#Formulas))
    fn lat_bound_rad() -> S {
        nalgebra::convert(1.484_422_229_745_332_4f64)
    }

    /// lat_bound_sin = sin(lat_bound_rad)
    fn lat_bound_sin() -> S {
        nalgebra::convert(0.996_272_076_220_75f64)
    }

    /// Projects a lat/lng coordinate to Web Mercator.
    ///
    /// Equivalent to the formula on [Wikipedia](https://en.wikipedia.org/wiki/Web_Mercator_projection#Formulas).
    /// If the latitude is outside `[-85.051129, 85.051129]`, it is clamped to that interval first.
    pub fn from_lat_lng(lat_lng: &WGS84<S>) -> Self {
        // Implemented according to
        // https://developers.google.com/maps/documentation/javascript/examples/map-coordinates?csw=1
        // but clamping is done before the sin() operation.
        let lat = nalgebra::clamp(
            lat_lng.latitude(),
            -Self::lat_bound_rad(),
            Self::lat_bound_rad(),
        );
        let sin_y = lat.sin();

        let normalized = Vector2::new(
            nalgebra::convert::<_, S>(0.5) + lat_lng.longitude() / S::two_pi(),
            nalgebra::convert::<_, S>(0.5)
                - ((S::one() + sin_y) / (S::one() - sin_y)).ln()
                    * nalgebra::convert(0.25)
                    * S::frac_1_pi(),
        );
        Self { normalized }
    }
}

impl<S: RealField> WebMercatorCoord<S>
where
    f64: From<S>,
{
    /// Convert the Web Mercator coordinate back to lat/lng.
    ///
    /// The altitude returned is always 0.
    pub fn to_lat_lng(&self) -> WGS84<S> {
        let centered: Vector2<S> =
            self.normalized - Vector2::new(nalgebra::convert(0.5), nalgebra::convert(0.5));
        // Note that sin_term = -(2/(sin(y)-1)) - 1
        let sin_term = (-self.normalized.y * nalgebra::convert(4.0) * S::pi()).exp();
        let one_over_sin_y = (sin_term + S::one()) * nalgebra::convert(-0.5);
        let mut sin_y = (S::one() / one_over_sin_y) + nalgebra::convert(1.0);
        sin_y = nalgebra::clamp(sin_y, -Self::lat_bound_sin(), Self::lat_bound_sin());
        let longitude = nalgebra::clamp(S::two_pi() * centered.x, -S::pi(), S::pi());
        let deg_per_rad = nalgebra::convert::<_, S>(180.0) / S::pi();
        WGS84::new(
            sin_y.asin() * deg_per_rad,
            longitude * deg_per_rad,
            S::zero(),
        )
    }
}

impl<S: RealField + SupersetOf<u32>> WebMercatorCoord<S> {
    /// To use a Web Mercator coordinate, specify a zoom level in which it
    /// should be represented.
    /// Zoom level Z means the map coordinates are in the interval `[0, 256*2^Z)`
    /// in both dimensions, i.e. map resolution doubles at each zoom level.
    pub fn to_zoomed_coordinate(&self, z: u8) -> Option<Vector2<S>> {
        if z <= MAX_ZOOM {
            // 256 * 2^z
            let zoom: S = nalgebra::convert(TILE_SIZE << z);
            Some(self.normalized * zoom)
        } else {
            None
        }
    }

    /// The inverse of [`to_zoomed_coordinate`](#method.to_zoomed_coordinate).
    ///
    /// Returns `None` when `z` is greater than [`MAX_ZOOM`](index.html#constant.max_zoom)
    /// or when the coordinates are out of bounds for the zoom level `z`.
    pub fn from_zoomed_coordinate(coord: Vector2<S>, z: u8) -> Option<Self> {
        if z > MAX_ZOOM || coord.min() < S::zero() {
            return None;
        }
        // 256 * 2^z
        let zoom: S = nalgebra::convert(TILE_SIZE << z);
        if coord.max() < zoom {
            Some(Self {
                normalized: coord / zoom,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::{assert_abs_diff_eq, assert_relative_eq};
    use nalgebra::Vector2;
    use nav_types::WGS84;
    use std::f64::consts::PI;

    #[test]
    fn projection_corners() {
        // Checks that the corners of the map are at the expected coordinates
        let lat_bound_deg = WebMercatorCoord::<f64>::lat_bound_rad() * 180.0 / PI;
        let lat_lng_lower = WGS84::new(lat_bound_deg, -180.0, 0.0);
        let lat_lng_upper = WGS84::new(-lat_bound_deg, 180.0, 0.0);
        let lower_corner = WebMercatorCoord::from_lat_lng(&lat_lng_lower);
        let upper_corner = WebMercatorCoord::from_lat_lng(&lat_lng_upper);
        // The upper left corner of a world map
        let lower_corner_truth = Vector2::new(0.0, 0.0);
        // The lower right corner of a world map
        let upper_corner_truth = Vector2::new(256.0, 256.0);
        assert_abs_diff_eq!(
            upper_corner.to_zoomed_coordinate(0).unwrap(),
            upper_corner_truth,
            epsilon = 10e-10
        );
        assert_abs_diff_eq!(
            lower_corner.to_zoomed_coordinate(0).unwrap(),
            lower_corner_truth,
            epsilon = 10e-10
        );
    }

    #[test]
    fn projection_roundtrip() {
        // Checks that unprojection of a projection returns the original coordinate,
        // except for altitude, which is 0
        let test_coordinate = WGS84::new(37.407204, -122.147604, 1300.0);
        let projected = WebMercatorCoord::from_lat_lng(&test_coordinate);
        let unprojected = projected.to_lat_lng();
        assert_relative_eq!(test_coordinate.longitude(), unprojected.longitude());
        assert_relative_eq!(test_coordinate.latitude(), unprojected.latitude());
        assert_eq!(unprojected.altitude(), 0.0);
    }

    #[test]
    fn projection_ground_truth() {
        let test_coordinate = WGS84::new(37.407204, -122.147604, 0.0);
        // This test coordinate is at approx. pixel (165, 18) on this OSM tile at level 19:
        // https://a.tile.openstreetmap.org/19/84253/203324.png
        // So the pixel coordinate in a map at zoom level 19 (resolution 256*2^19) is this:
        let test_coordinate_web_mercator_zoomed_truth =
            Vector2::new(84253.0 * 256.0 + 165.0, 203324.0 * 256.0 + 18.0);
        let test_coordinate_web_mercator_zoomed = WebMercatorCoord::from_lat_lng(&test_coordinate)
            .to_zoomed_coordinate(19)
            .unwrap();
        // This was from eyeballing, so we shouldn't expect more than 20px accuracy at zoom 19.
        assert_abs_diff_eq!(
            test_coordinate_web_mercator_zoomed,
            test_coordinate_web_mercator_zoomed_truth,
            epsilon = 20.0
        );
    }
}
