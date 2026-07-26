//! Demo mode: a scripted fleet of fake missions so grok-orbit is fun to poke at
//! (and record GIFs of) without grok-build installed. Deterministic by default â€”
//! same seed, same show.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::model::{Activity, Mission, MissionStatus};

/// Small xorshift64* PRNG â€” deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo)
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.range(0, 100) < percent
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

const TITLES: &[&str] = &[
    "fix the flaky login test",
    "refactor the auth middleware",
    "why is prod on fire",
    "add dark mode to settings",
    "migrate the users table",
    "make CI 40% faster",
    "hunt the memory leak",
    "write docs nobody will read",
    "rename everything again",
    "upgrade to the new SDK",
    "delete 4,000 lines of dead code",
    "add rate limiting to the API",
    "unbreak the webhook retries",
    "port the parser to Rust",
];

const CWDS: &[&str] = &[
    "~/code/starship",
    "~/code/side-project-7",
    "~/work/monolith",
    "~/work/api-gateway",
    "~/code/dotfiles",
];

const MODELS: &[&str] = &["grok-4-code", "grok-4-heavy", "grok-4-mini"];

/// One scripted mission: a lifecycle of phases it walks through in real time.
struct DemoMission {
    mission: Mission,
    /// Seconds until the next phase change.
    phase_left: f64,
    /// Scripted final fate.
    will_crash: bool,
    /// Tokens burned per second while working.
    burn_rate: f64,
    done: bool,
}

pub struct DemoWorld {
    rng: Rng,
    missions: Vec<DemoMission>,
    spawn_cooldown: f64,
    serial: u64,
    /// Assumed context window for the fuel math (honors --context-window).
    ctx: f64,
}

impl DemoWorld {
    pub fn new(seed: u64, context_window: u64) -> Self {
        let mut world = DemoWorld {
            rng: Rng::new(seed),
            missions: Vec::new(),
            spawn_cooldown: 0.0,
            serial: 0,
            ctx: context_window.max(1) as f64,
        };
        // Start with a lively pad: a few missions mid-flight, one already landed.
        for _ in 0..4 {
            world.spawn(true);
        }
        world.spawn_landed();
        world
    }

    fn next_id(&mut self) -> String {
        self.serial += 1;
        format!(
            "demo-{:04x}-{:08x}",
            self.serial,
            self.rng.next_u64() as u32
        )
    }

    fn spawn(&mut self, midflight: bool) {
        let id = self.next_id();
        let title = (*self.rng.pick(TITLES)).to_string();
        let cwd = (*self.rng.pick(CWDS)).to_string();
        let model = (*self.rng.pick(MODELS)).to_string();
        let is_probe = self.rng.chance(20) && !self.missions.is_empty();
        let parent_id = if is_probe {
            self.missions
                .iter()
                .find(|m| m.mission.status == MissionStatus::Burning && !m.mission.is_probe)
                .map(|m| m.mission.id.clone())
        } else {
            None
        };
        let fuel = if midflight {
            self.rng.range(5, 60) as f64 / 100.0
        } else {
            0.02
        };
        let mission = Mission {
            id,
            title,
            cwd,
            model: Some(model),
            is_probe: parent_id.is_some(),
            parent_id,
            status: MissionStatus::Burning,
            tokens: (fuel * self.ctx) as u64,
            turns: Some(self.rng.range(1, 30)),
            fuel_used: fuel,
            last_activity: Some(SystemTime::now()),
            created_at: Some(SystemTime::now()),
            activity: Activity::ToolCall,
            path: PathBuf::from("demo"),
        };
        self.missions.push(DemoMission {
            mission,
            phase_left: self.rng.range(4, 15) as f64,
            will_crash: self.rng.chance(18),
            burn_rate: self.rng.range(800, 4200) as f64,
            done: false,
        });
    }

    fn spawn_landed(&mut self) {
        self.spawn(true);
        if let Some(dm) = self.missions.last_mut() {
            dm.mission.status = MissionStatus::Landed;
            dm.mission.activity = Activity::Unknown;
            dm.done = true;
        }
    }

    /// Advance the script by `dt` seconds and return the current fleet.
    pub fn tick(&mut self, dt: f64) -> Vec<Mission> {
        self.spawn_cooldown -= dt;
        let live = self.missions.iter().filter(|m| !m.done).count();
        if self.spawn_cooldown <= 0.0 && live < 6 {
            self.spawn(false);
            self.spawn_cooldown = self.rng.range(6, 18) as f64;
        }

        let mut rng_rolls: Vec<(usize, u64)> = Vec::new();
        for (i, _) in self.missions.iter().enumerate() {
            rng_rolls.push((i, self.rng.next_u64()));
        }

        let ctx = self.ctx;
        for (i, roll) in rng_rolls {
            let dm = &mut self.missions[i];
            if dm.done {
                continue;
            }
            dm.phase_left -= dt;
            match dm.mission.status {
                MissionStatus::Burning => {
                    dm.mission.tokens += (dm.burn_rate * dt) as u64;
                    dm.mission.fuel_used = (dm.mission.tokens as f64 / ctx).min(1.0);
                    dm.mission.last_activity = Some(SystemTime::now());
                    if dm.phase_left <= 0.0 {
                        // Fuel exhausted => auto-compact ("mid-flight refuel").
                        if dm.mission.fuel_used >= 0.98 {
                            dm.mission.tokens = (ctx * 0.15) as u64;
                            dm.mission.fuel_used = 0.15;
                        }
                        let r = roll % 100;
                        if dm.will_crash && r < 30 {
                            dm.mission.status = MissionStatus::Lost;
                            dm.mission.activity = Activity::Error;
                            dm.done = true;
                        } else if r < 55 {
                            dm.mission.status = MissionStatus::Holding;
                            dm.mission.activity = if r < 20 {
                                Activity::Permission
                            } else {
                                Activity::Unknown
                            };
                            dm.phase_left = 3.0 + (roll % 9) as f64;
                        } else if r < 75 {
                            dm.mission.status = MissionStatus::Landed;
                            dm.mission.activity = Activity::Unknown;
                            dm.done = true;
                        } else {
                            dm.mission.activity = if r % 2 == 0 {
                                Activity::Talking
                            } else {
                                Activity::ToolCall
                            };
                            dm.phase_left = 4.0 + (roll % 12) as f64;
                        }
                    }
                }
                MissionStatus::Holding => {
                    if dm.phase_left <= 0.0 {
                        dm.mission.status = MissionStatus::Burning;
                        dm.mission.activity = Activity::ToolCall;
                        dm.mission.last_activity = Some(SystemTime::now());
                        dm.phase_left = 4.0 + (roll % 12) as f64;
                    }
                }
                MissionStatus::Landed | MissionStatus::Lost => {
                    dm.done = true;
                }
            }
        }

        // Keep the mission log from growing unbounded during long demos.
        if self.missions.len() > 40 {
            let cut = self.missions.len() - 40;
            self.missions.drain(0..cut);
        }

        self.missions.iter().map(|m| m.mission.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_deterministic_for_same_seed() {
        let run = |seed| {
            let mut w = DemoWorld::new(seed, 256_000);
            let mut ids = Vec::new();
            for _ in 0..200 {
                let fleet = w.tick(0.5);
                ids.push(
                    fleet
                        .iter()
                        .map(|m| format!("{}:{}", m.id, m.status.label()))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            ids
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43));
    }

    #[test]
    fn demo_missions_eventually_land_or_crash() {
        let mut w = DemoWorld::new(7, 256_000);
        let mut saw_landed = false;
        let mut saw_lost = false;
        for _ in 0..2000 {
            for m in w.tick(0.5) {
                match m.status {
                    MissionStatus::Landed => saw_landed = true,
                    MissionStatus::Lost => saw_lost = true,
                    _ => {}
                }
            }
        }
        assert!(saw_landed, "no mission ever landed");
        assert!(saw_lost, "no mission ever crashed");
    }

    #[test]
    fn probes_reference_existing_parents() {
        let mut w = DemoWorld::new(99, 256_000);
        for _ in 0..500 {
            let fleet = w.tick(0.5);
            for m in fleet.iter().filter(|m| m.is_probe) {
                let parent = m.parent_id.as_ref().unwrap();
                // Parent may have been drained from the log, but the id must be a demo id.
                assert!(parent.starts_with("demo-"));
            }
        }
    }
}
