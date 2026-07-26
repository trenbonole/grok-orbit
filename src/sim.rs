//! The little universe: maps missions onto rockets with smooth, stateful
//! animation (launches, orbits, landings, crashes), plus ambient scenery
//! (starfield, comets). All positions live in normalized world space
//! (x: 0..1 left-to-right, y: 0..1 top-to-bottom); the renderer rasterizes.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::SystemTime;

use crate::demo::Rng;
use crate::model::{Mission, MissionStatus};

/// How long ago a finished mission may have ended and still get a sprite
/// (older ones live only in the Mission Log).
const RECENT_ARRIVAL_SECS: u64 = 30 * 60;
/// Hard cap on sprites so a busy pad never turns into soup.
const MAX_SPRITES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlightPhase {
    /// Climbing from the surface to orbit.
    Launching { progress: f64 },
    /// Circling the planet.
    Orbiting,
    /// Descending for a soft landing.
    Landing { progress: f64 },
    /// Parked on the surface after touchdown.
    Parked,
    /// Spinning off into the debris belt.
    Crashing { progress: f64 },
    /// Inert wreckage in the debris belt.
    Debris,
}

#[derive(Debug, Clone)]
pub struct Rocket {
    pub mission: Mission,
    pub phase: FlightPhase,
    /// Orbital angle in radians.
    pub angle: f64,
    /// Current distance from planet center, as a fraction of world height.
    pub radius: f64,
    /// Assigned cruising orbit radius.
    pub orbit_radius: f64,
    /// Per-rocket speed jitter so the fleet doesn't march in lockstep.
    pub speed_jitter: f64,
    /// Recent positions for the engine trail.
    pub trail: VecDeque<(f64, f64)>,
    /// Animation clock for blinking/flame frames.
    pub anim: f64,
    /// Fixed angle on the surface once parked / in the belt once debris.
    pub rest_angle: f64,
}

impl Rocket {
    /// Current world position.
    pub fn pos(&self, aspect: f64) -> (f64, f64) {
        let (cx, cy) = (0.5, 0.5);
        // Terminal cells are ~2x taller than wide; squash y so orbits look round.
        (
            cx + self.radius * self.angle.cos() * aspect,
            cy + self.radius * self.angle.sin(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Star {
    pub x: f64,
    pub y: f64,
    pub phase: f64,
    pub bright: bool,
}

#[derive(Debug, Clone)]
pub struct Comet {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
    pub trail: VecDeque<(f64, f64)>,
}

pub struct World {
    pub rockets: Vec<Rocket>,
    pub stars: Vec<Star>,
    pub comet: Option<Comet>,
    pub planet_radius: f64,
    pub time: f64,
    comet_cooldown: f64,
    rng: Rng,
    /// Sticky per-mission animation state across syncs.
    known: HashMap<String, ()>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let mut stars = Vec::new();
        for _ in 0..90 {
            stars.push(Star {
                x: rng.range(0, 1000) as f64 / 1000.0,
                y: rng.range(0, 1000) as f64 / 1000.0,
                phase: rng.range(0, 628) as f64 / 100.0,
                bright: rng.chance(20),
            });
        }
        World {
            rockets: Vec::new(),
            stars,
            comet: None,
            planet_radius: 0.13,
            time: 0.0,
            comet_cooldown: 12.0,
            rng,
            known: HashMap::new(),
        }
    }

    /// Reconcile the sprite fleet with the latest scan.
    pub fn sync(&mut self, missions: &[Mission], now: SystemTime) {
        let visible: Vec<&Mission> = missions
            .iter()
            .filter(|m| is_visible(m, now))
            .take(MAX_SPRITES)
            .collect();

        // Update or spawn.
        for m in &visible {
            if let Some(r) = self.rockets.iter_mut().find(|r| r.mission.id == m.id) {
                let old_status = r.mission.status;
                r.mission = (*m).clone();
                if old_status != m.status {
                    r.begin_transition(m.status);
                }
            } else {
                let orbit_radius = self.planet_radius
                    + 0.08
                    + (self.rng.range(0, 5) as f64) * 0.045
                    + if m.is_probe { -0.06 } else { 0.0 };
                let starts_in_orbit =
                    self.known.contains_key(&m.id) || m.status != MissionStatus::Burning;
                let mut rocket = Rocket {
                    mission: (*m).clone(),
                    phase: FlightPhase::Launching { progress: 0.0 },
                    angle: self.rng.range(0, 628) as f64 / 100.0,
                    radius: self.planet_radius,
                    orbit_radius: orbit_radius.max(self.planet_radius + 0.05),
                    speed_jitter: 0.7 + self.rng.range(0, 60) as f64 / 100.0,
                    trail: VecDeque::new(),
                    anim: self.rng.range(0, 100) as f64 / 25.0,
                    rest_angle: self.rng.range(0, 628) as f64 / 100.0,
                };
                // Sessions discovered mid-flight (or already finished) skip the
                // launch animation; only genuinely new sessions lift off.
                if starts_in_orbit {
                    rocket.phase = FlightPhase::Orbiting;
                    rocket.radius = rocket.orbit_radius;
                    rocket.begin_transition(m.status);
                } else {
                    self.known.insert(m.id.clone(), ());
                }
                self.rockets.push(rocket);
            }
            self.known.insert(m.id.clone(), ());
        }

        // Drop sprites whose mission vanished from the visible set.
        let keep: Vec<String> = visible.iter().map(|m| m.id.clone()).collect();
        self.rockets.retain(|r| keep.contains(&r.mission.id));

        // `known` only needs to cover sessions that still exist on disk;
        // prune it so a long-running watcher doesn't accumulate dead ids.
        self.known
            .retain(|id, _| missions.iter().any(|m| &m.id == id));
    }

    /// Advance animations by `dt` seconds.
    pub fn tick(&mut self, dt: f64) {
        self.time += dt;

        for r in &mut self.rockets {
            r.anim += dt;
            match r.phase {
                FlightPhase::Launching { progress } => {
                    let p = (progress + dt / 1.6).min(1.0);
                    r.radius =
                        self.planet_radius + (r.orbit_radius - self.planet_radius) * ease_out(p);
                    r.phase = if p >= 1.0 {
                        FlightPhase::Orbiting
                    } else {
                        FlightPhase::Launching { progress: p }
                    };
                    r.angle += dt * 0.5 * r.speed_jitter;
                }
                FlightPhase::Orbiting => {
                    let speed = match r.mission.status {
                        MissionStatus::Burning => 0.45,
                        MissionStatus::Holding => 0.07,
                        _ => 0.2,
                    };
                    r.angle += dt * speed * r.speed_jitter;
                }
                FlightPhase::Landing { progress } => {
                    let p = (progress + dt / 2.2).min(1.0);
                    r.radius =
                        r.orbit_radius - (r.orbit_radius - self.planet_radius) * ease_in_out(p);
                    r.angle += dt * 0.15 * r.speed_jitter;
                    if p >= 1.0 {
                        r.rest_angle = r.angle;
                        r.phase = FlightPhase::Parked;
                        r.trail.clear();
                    } else {
                        r.phase = FlightPhase::Landing { progress: p };
                    }
                }
                FlightPhase::Parked => {
                    r.angle = r.rest_angle;
                    r.radius = self.planet_radius;
                }
                FlightPhase::Crashing { progress } => {
                    let p = (progress + dt / 1.8).min(1.0);
                    let debris_radius = 0.42;
                    r.radius = r.radius + (debris_radius - r.radius) * (dt * 2.0).min(1.0);
                    r.angle += dt * 2.5; // tumbling
                    if p >= 1.0 {
                        r.rest_angle = r.angle;
                        r.phase = FlightPhase::Debris;
                        r.trail.clear();
                    } else {
                        r.phase = FlightPhase::Crashing { progress: p };
                    }
                }
                FlightPhase::Debris => {
                    // Wreckage drifts, slowly.
                    r.angle = r.rest_angle + (self.time * 0.02).sin() * 0.03;
                    r.radius = 0.42;
                }
            }

            // Engine trail only while actually moving under power.
            let powered = matches!(
                r.phase,
                FlightPhase::Launching { .. } | FlightPhase::Orbiting
            ) && r.mission.status == MissionStatus::Burning;
            if powered {
                let pos = r.pos(0.55);
                if r.trail
                    .back()
                    .map(|last| dist(*last, pos) > 0.012)
                    .unwrap_or(true)
                {
                    r.trail.push_back(pos);
                    if r.trail.len() > 7 {
                        r.trail.pop_front();
                    }
                }
            } else if !r.trail.is_empty() && self.time.fract() < dt {
                r.trail.pop_front();
            }
        }

        // Probes fly formation with their parent rocket when both are aloft.
        let parents: HashMap<String, (f64, f64)> = self
            .rockets
            .iter()
            .filter(|r| {
                !r.mission.is_probe
                    && matches!(
                        r.phase,
                        FlightPhase::Orbiting | FlightPhase::Launching { .. }
                    )
            })
            .map(|r| (r.mission.id.clone(), (r.angle, r.radius)))
            .collect();
        for r in &mut self.rockets {
            if !r.mission.is_probe
                || !matches!(
                    r.phase,
                    FlightPhase::Orbiting | FlightPhase::Launching { .. }
                )
            {
                continue;
            }
            if let Some((pa, pr)) = r
                .mission
                .parent_id
                .as_ref()
                .and_then(|pid| parents.get(pid))
            {
                let bob = (self.time * 1.8 + r.speed_jitter * 10.0).sin() * 0.06;
                r.angle = pa + 0.35 + bob;
                r.radius = (pr - 0.05).max(self.planet_radius + 0.03);
            }
        }

        // Comets: occasional, decorative.
        self.comet_cooldown -= dt;
        if self.comet.is_none() && self.comet_cooldown <= 0.0 {
            let from_left = self.rng.chance(50);
            self.comet = Some(Comet {
                x: if from_left { -0.05 } else { 1.05 },
                y: self.rng.range(5, 40) as f64 / 100.0,
                dx: if from_left { 0.12 } else { -0.12 },
                dy: 0.03,
                trail: VecDeque::new(),
            });
            self.comet_cooldown = 18.0 + self.rng.range(0, 20) as f64;
        }
        if let Some(c) = &mut self.comet {
            c.trail.push_back((c.x, c.y));
            if c.trail.len() > 10 {
                c.trail.pop_front();
            }
            c.x += c.dx * dt;
            c.y += c.dy * dt;
            if !(-0.1..=1.1).contains(&c.x) || c.y > 1.1 {
                self.comet = None;
            }
        }
    }
}

impl Rocket {
    fn begin_transition(&mut self, new_status: MissionStatus) {
        match new_status {
            MissionStatus::Landed => {
                if !matches!(
                    self.phase,
                    FlightPhase::Parked | FlightPhase::Landing { .. }
                ) {
                    self.phase = FlightPhase::Landing { progress: 0.0 };
                }
            }
            MissionStatus::Lost => {
                if !matches!(
                    self.phase,
                    FlightPhase::Debris | FlightPhase::Crashing { .. }
                ) {
                    self.phase = FlightPhase::Crashing { progress: 0.0 };
                }
            }
            MissionStatus::Burning | MissionStatus::Holding => {
                if matches!(
                    self.phase,
                    FlightPhase::Parked
                        | FlightPhase::Landing { .. }
                        | FlightPhase::Debris
                        | FlightPhase::Crashing { .. }
                ) {
                    // Back from the dead (resumed session): relaunch.
                    self.phase = FlightPhase::Launching { progress: 0.0 };
                    self.radius = self.orbit_radius.min(self.radius.max(0.0));
                }
            }
        }
    }
}

fn is_visible(m: &Mission, now: SystemTime) -> bool {
    match m.status {
        MissionStatus::Burning | MissionStatus::Holding => true,
        MissionStatus::Landed | MissionStatus::Lost => m
            .last_activity
            .map(|t| {
                // duration_since errs when t is in the future (clock skew /
                // sub-tick races) — that means "just now", i.e. visible.
                now.duration_since(t)
                    .map(|d| d.as_secs() <= RECENT_ARRIVAL_SECS)
                    .unwrap_or(true)
            })
            .unwrap_or(false),
    }
}

fn ease_out(p: f64) -> f64 {
    1.0 - (1.0 - p) * (1.0 - p)
}

fn ease_in_out(p: f64) -> f64 {
    if p < 0.5 {
        2.0 * p * p
    } else {
        1.0 - (-2.0 * p + 2.0).powi(2) / 2.0
    }
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Test fixture shared by unit tests in other modules.
#[cfg(test)]
pub mod tests_support {
    use super::*;
    use crate::model::Activity;
    use std::path::PathBuf;

    pub fn mission(id: &str) -> Mission {
        Mission {
            id: id.to_string(),
            title: format!("mission {id}"),
            cwd: "~/code".into(),
            model: None,
            parent_id: None,
            is_probe: false,
            status: MissionStatus::Burning,
            tokens: 1000,
            turns: Some(1),
            fuel_used: 0.1,
            last_activity: Some(SystemTime::now()),
            created_at: Some(SystemTime::now()),
            activity: Activity::Unknown,
            path: PathBuf::from("x"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Activity;
    use std::path::PathBuf;

    fn mission(id: &str, status: MissionStatus) -> Mission {
        Mission {
            id: id.to_string(),
            title: format!("mission {id}"),
            cwd: "~/code".into(),
            model: None,
            parent_id: None,
            is_probe: false,
            status,
            tokens: 1000,
            turns: Some(1),
            fuel_used: 0.1,
            last_activity: Some(SystemTime::now()),
            created_at: Some(SystemTime::now()),
            activity: Activity::Unknown,
            path: PathBuf::from("x"),
        }
    }

    #[test]
    fn burning_mission_gets_a_rocket_and_orbits() {
        let mut w = World::new(1);
        let now = SystemTime::now();
        w.sync(&[mission("a", MissionStatus::Burning)], now);
        assert_eq!(w.rockets.len(), 1);
        for _ in 0..200 {
            w.tick(0.1);
        }
        assert_eq!(w.rockets[0].phase, FlightPhase::Orbiting);
        let (x, y) = w.rockets[0].pos(0.55);
        assert!((0.0..=1.0).contains(&x), "x out of world: {x}");
        assert!((0.0..=1.0).contains(&y), "y out of world: {y}");
    }

    #[test]
    fn landing_and_crash_transitions_settle() {
        let mut w = World::new(2);
        let now = SystemTime::now();
        w.sync(
            &[
                mission("a", MissionStatus::Burning),
                mission("b", MissionStatus::Burning),
            ],
            now,
        );
        for _ in 0..100 {
            w.tick(0.1);
        }
        w.sync(
            &[
                mission("a", MissionStatus::Landed),
                mission("b", MissionStatus::Lost),
            ],
            now,
        );
        for _ in 0..200 {
            w.tick(0.1);
        }
        let a = w.rockets.iter().find(|r| r.mission.id == "a").unwrap();
        let b = w.rockets.iter().find(|r| r.mission.id == "b").unwrap();
        assert_eq!(a.phase, FlightPhase::Parked);
        assert_eq!(b.phase, FlightPhase::Debris);
    }

    #[test]
    fn vanished_missions_are_removed() {
        let mut w = World::new(3);
        let now = SystemTime::now();
        w.sync(&[mission("a", MissionStatus::Burning)], now);
        assert_eq!(w.rockets.len(), 1);
        w.sync(&[], now);
        assert_eq!(w.rockets.len(), 0);
    }

    #[test]
    fn old_finished_missions_are_not_visible() {
        let now = SystemTime::now();
        let mut m = mission("a", MissionStatus::Landed);
        m.last_activity = Some(now - std::time::Duration::from_secs(3600 * 24));
        assert!(!is_visible(&m, now));
        let live = mission("b", MissionStatus::Holding);
        assert!(is_visible(&live, now));
    }
}
