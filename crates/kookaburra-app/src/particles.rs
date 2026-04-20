//! Subtle click-particle puffs. When the user clicks a tile, a small
//! accent-colored burst is spawned at the click location and fades out
//! over `LIFETIME_MS`. Driven by the same `request_redraw` cycle as the
//! bell flash in `main.rs` — the app ages bursts each frame and culls
//! expired ones; the renderer just draws whatever it's handed.

use std::time::{Duration, Instant};

use kookaburra_core::config::Rgba;
use kookaburra_core::ids::TileId;
use kookaburra_render::RenderParticle;

/// Particles per click. Six feels like a "puff of ember" — fewer reads as
/// stingy, more reads as flashy.
pub const PARTICLE_COUNT: usize = 6;
/// Total lifetime of a burst. About a third of a second — long enough to
/// register, short enough to be gone before the user starts typing.
pub const LIFETIME: Duration = Duration::from_millis(320);
/// Maximum radial travel of a particle from its spawn point, in logical px.
pub const MAX_RADIUS_PX: f32 = 10.0;
/// Starting alpha (0..1) of the accent color. 0.7 × the already-muted
/// accent reads as present-but-quiet over the terminal bg.
pub const START_ALPHA: f32 = 0.7;
/// Logical-pixel size of a particle at spawn (it's a small filled square).
pub const START_SIZE_PX: f32 = 2.0;
/// Logical-pixel size at end-of-life.
pub const END_SIZE_PX: f32 = 1.0;

/// One click → one burst. All particles share a spawn time and origin;
/// their per-particle variation is derived deterministically from `seed`.
#[derive(Copy, Clone, Debug)]
pub struct ParticleBurst {
    pub tile_id: TileId,
    pub spawn_time: Instant,
    /// Origin in the tile's local logical-pixel space (top-left = 0,0).
    pub origin: (f32, f32),
    /// Per-burst jitter seed. Derived from the spawn instant so each click
    /// gets a unique angle distribution without threading an RNG.
    pub seed: u32,
}

impl ParticleBurst {
    #[must_use]
    pub fn new(tile_id: TileId, origin: (f32, f32), now: Instant) -> Self {
        let seed = spawn_seed(now);
        Self {
            tile_id,
            spawn_time: now,
            origin,
            seed,
        }
    }

    /// Is this burst still within its lifetime at `now`?
    #[must_use]
    pub fn is_live(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.spawn_time) < LIFETIME
    }
}

/// Evaluate every particle in `burst` at `now` and return the live ones.
/// Returns an empty vec when the burst is past its lifetime. Coordinates
/// are tile-local logical pixels — the renderer offsets by `rect.x/y`.
#[must_use]
pub fn evaluate(burst: &ParticleBurst, now: Instant) -> Vec<RenderParticle> {
    let age = now.saturating_duration_since(burst.spawn_time);
    if age >= LIFETIME {
        return Vec::new();
    }
    let t = age.as_secs_f32() / LIFETIME.as_secs_f32();
    // Ease-out radial travel — fast out of the gate, slows near the edge.
    let travel = 1.0 - (1.0 - t) * (1.0 - t);
    // Ease-out alpha fade: (1 - t)^2. Starts at 1, ends at 0.
    let alpha = ((1.0 - t) * (1.0 - t) * START_ALPHA * 255.0).clamp(0.0, 255.0) as u8;
    let size = START_SIZE_PX + (END_SIZE_PX - START_SIZE_PX) * t;

    let mut out = Vec::with_capacity(PARTICLE_COUNT);
    for i in 0..PARTICLE_COUNT {
        let angle = particle_angle(i, burst.seed);
        let r = MAX_RADIUS_PX * travel;
        let cx = burst.origin.0 + angle.cos() * r;
        let cy = burst.origin.1 + angle.sin() * r;
        out.push(RenderParticle {
            x: cx - size / 2.0,
            y: cy - size / 2.0,
            size,
            color: accent_with_alpha(alpha),
        });
    }
    out
}

/// Matches the hardcoded focused-tile border accent in
/// `kookaburra-render::emit_tile`. Keeping it here (vs. reading from the
/// theme) keeps particles on-brand across themes without plumbing a new
/// field; if we later add `Theme::accent`, swap this for that.
#[inline]
fn accent_with_alpha(a: u8) -> Rgba {
    Rgba {
        r: 0xff,
        g: 0xa5,
        b: 0x1c,
        a,
    }
}

/// Evenly-distributed angle per particle (`i * 60°`) plus a small
/// seed-driven rotation so two back-to-back clicks don't spawn identical
/// patterns.
fn particle_angle(i: usize, seed: u32) -> f32 {
    let base = (i as f32) * std::f32::consts::TAU / (PARTICLE_COUNT as f32);
    // ±0.2 rad jitter derived from seed, stable across a single burst.
    let jitter = ((seed.wrapping_mul(2654435761).wrapping_add(i as u32 * 97)) & 0xffff) as f32;
    let rot = (jitter / 65535.0 - 0.5) * 0.4;
    base + rot
}

fn spawn_seed(t: Instant) -> u32 {
    // `Instant`'s internal representation isn't directly addressable, so
    // hash via its Duration-since-a-fixed-point. `elapsed()` works because
    // we only need *some* varying u32; we don't need monotonic semantics.
    let d = t.elapsed();
    (d.as_nanos() as u32) ^ (d.as_secs() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst_at(now: Instant) -> ParticleBurst {
        ParticleBurst::new(TileId::new(), (50.0, 40.0), now)
    }

    #[test]
    fn spawns_six_particles() {
        let now = Instant::now();
        let b = burst_at(now);
        let parts = evaluate(&b, now);
        assert_eq!(parts.len(), PARTICLE_COUNT);
    }

    #[test]
    fn alpha_starts_high_and_ends_zero() {
        let now = Instant::now();
        let b = burst_at(now);
        let at_start = evaluate(&b, now);
        let at_end = evaluate(&b, now + LIFETIME);
        // At t=0: alpha ≈ START_ALPHA * 255 = 178 ± rounding.
        assert!(at_start[0].color.a >= 170);
        // At t=lifetime: expired → empty vec (no particles to draw).
        assert!(at_end.is_empty());
    }

    #[test]
    fn alpha_monotonically_non_increasing() {
        let now = Instant::now();
        let b = burst_at(now);
        let mut last = 255u8;
        // Sample 10 points across the lifetime.
        for i in 0..10 {
            let t = now + LIFETIME.mul_f32(i as f32 / 10.0);
            let parts = evaluate(&b, t);
            if let Some(p) = parts.first() {
                assert!(p.color.a <= last, "alpha went up at step {i}");
                last = p.color.a;
            }
        }
    }

    #[test]
    fn midway_travels_substantially() {
        let now = Instant::now();
        let b = burst_at(now);
        let parts = evaluate(&b, now + LIFETIME / 2);
        // Ease-out: at t=0.5, travel = 1 - 0.25 = 0.75 → radius = 7.5 px.
        // The center of the quad is `x + size/2`, so recover the center
        // and measure its distance from the spawn origin.
        for p in &parts {
            let cx = p.x + p.size / 2.0;
            let cy = p.y + p.size / 2.0;
            let dx = cx - b.origin.0;
            let dy = cy - b.origin.1;
            let d = (dx * dx + dy * dy).sqrt();
            assert!(
                d > MAX_RADIUS_PX * 0.25 && d < MAX_RADIUS_PX * 0.95,
                "distance {d} out of expected mid-life band"
            );
        }
    }

    #[test]
    fn particles_have_distinct_angles() {
        let now = Instant::now();
        let b = burst_at(now);
        // Sample partway through the lifetime — at t=0 travel is 0 so
        // every particle sits on the origin and angles are undefined.
        let parts = evaluate(&b, now + LIFETIME / 4);
        // Compute each particle's angle from the origin and check they're
        // pairwise different (within a small epsilon — six particles on a
        // circle are 60° apart even before jitter).
        let angles: Vec<f32> = parts
            .iter()
            .map(|p| {
                let cx = p.x + p.size / 2.0;
                let cy = p.y + p.size / 2.0;
                (cy - b.origin.1).atan2(cx - b.origin.0)
            })
            .collect();
        for i in 0..angles.len() {
            for j in (i + 1)..angles.len() {
                let d = (angles[i] - angles[j]).abs();
                assert!(d > 0.1, "particles {i} and {j} share an angle");
            }
        }
    }

    #[test]
    fn burst_lifecycle_flag() {
        let now = Instant::now();
        let b = burst_at(now);
        assert!(b.is_live(now));
        assert!(b.is_live(now + LIFETIME / 2));
        assert!(!b.is_live(now + LIFETIME));
        assert!(!b.is_live(now + LIFETIME + Duration::from_millis(1)));
    }
}
