//! Window placement policy.
//!
//! Iteration 1: every window is placed in the middle of the only output with a
//! small cascade offset, i.e. classic stacking behaviour. Tiling, snapping and
//! per-app rules will live here later.

use smithay::{
    desktop::{Space, Window},
    utils::{Logical, Point},
};

/// Offset of each subsequent window in the cascade, in logical pixels.
const CASCADE_STEP: i32 = 32;

/// How many steps the cascade wraps around before starting over.
const CASCADE_WRAP: i32 = 8;

/// Location for a newly mapped window.
pub fn location_for_new_window(space: &Space<Window>) -> Point<i32, Logical> {
    let Some(output) = space.outputs().next() else {
        return (0, 0).into();
    };
    let Some(output_geo) = space.output_geometry(output) else {
        return (0, 0).into();
    };

    let step = (space.elements().count() as i32 % CASCADE_WRAP) * CASCADE_STEP;
    // A third of the way in looks better than dead centre for the first window.
    let base = Point::<i32, Logical>::from((
        output_geo.loc.x + output_geo.size.w / 6,
        output_geo.loc.y + output_geo.size.h / 6,
    ));

    base + Point::from((step, step))
}
