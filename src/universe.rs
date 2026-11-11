//! The k-space stargate map (built into the binary from `data/universe.tsv`) and the
//! hourly activity stats published by ESI (the same ones Dotlan shows).

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::api::Space;

pub struct System {
    pub name: String,
    pub region: String,
    /// Security as stored in the SDE (true sec).
    pub security: f32,
}

impl System {
    /// Security as the game displays it (rounded to one decimal, 0.0–0.05 shown as 0.1).
    pub fn display_sec(&self) -> f32 {
        display_sec(self.security)
    }

    pub fn space(&self) -> Space {
        let s = self.display_sec();
        if s >= 0.5 {
            Space::High
        } else if s > 0.0 {
            Space::Low
        } else {
            Space::Null
        }
    }
}

pub fn display_sec(sec: f32) -> f32 {
    if sec > 0.0 && sec < 0.05 { 0.1 } else { (sec * 10.0).round() / 10.0 }
}

pub struct Universe {
    systems: HashMap<u64, System>,
    gates: HashMap<u64, Vec<u64>>,
}

impl Universe {
    pub fn parse(src: &str) -> Universe {
        let mut systems = HashMap::new();
        let mut gates: HashMap<u64, Vec<u64>> = HashMap::new();
        for line in src.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["S", id, name, region, sec] => {
                    if let (Ok(id), Ok(security)) = (id.parse(), sec.parse()) {
                        systems.insert(id, System { name: name.to_string(), region: region.to_string(), security });
                    }
                }
                ["J", a, b] => {
                    if let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>()) {
                        gates.entry(a).or_default().push(b);
                        gates.entry(b).or_default().push(a);
                    }
                }
                _ => {}
            }
        }
        Universe { systems, gates }
    }

    pub fn system(&self, id: u64) -> Option<&System> {
        self.systems.get(&id)
    }

    /// Every system within `max` gate jumps of `from` (itself included at distance 0).
    pub fn within(&self, from: u64, max: u32) -> Vec<(u64, u32)> {
        if !self.systems.contains_key(&from) {
            return Vec::new();
        }
        let mut dist: HashMap<u64, u32> = HashMap::from([(from, 0)]);
        let mut queue = VecDeque::from([from]);
        let mut out = vec![(from, 0)];
        while let Some(id) = queue.pop_front() {
            let d = dist[&id];
            if d == max {
                continue;
            }
            for &next in self.gates.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
                if !dist.contains_key(&next) {
                    dist.insert(next, d + 1);
                    out.push((next, d + 1));
                    queue.push_back(next);
                }
            }
        }
        out
    }
}

/// The map built into the binary, parsed on first use.
pub fn universe() -> &'static Universe {
    static U: OnceLock<Universe> = OnceLock::new();
    U.get_or_init(|| Universe::parse(include_str!("../data/universe.tsv")))
}

// ---------------------------------------------------------------------------------------------
// Hourly stats (ESI)

const KILLS_URL: &str = "https://esi.evetech.net/latest/universe/system_kills/";
const JUMPS_URL: &str = "https://esi.evetech.net/latest/universe/system_jumps/";

#[derive(Clone, Copy, Debug, Default)]
pub struct Activity {
    pub npc_kills: u32,
    pub ship_kills: u32,
    pub pod_kills: u32,
    pub jumps: u32,
}

pub type Stats = HashMap<u64, Activity>;

#[derive(Deserialize)]
struct KillRow {
    system_id: u64,
    #[serde(default)]
    npc_kills: u32,
    #[serde(default)]
    ship_kills: u32,
    #[serde(default)]
    pod_kills: u32,
}

#[derive(Deserialize)]
struct JumpRow {
    system_id: u64,
    #[serde(default)]
    ship_jumps: u32,
}

pub fn parse_stats(kills: &str, jumps: &str) -> Result<Stats, String> {
    let kills: Vec<KillRow> = serde_json::from_str(kills).map_err(|e| format!("unreadable system_kills: {e}"))?;
    let jumps: Vec<JumpRow> = serde_json::from_str(jumps).map_err(|e| format!("unreadable system_jumps: {e}"))?;
    let mut stats = Stats::new();
    for k in kills {
        let a = stats.entry(k.system_id).or_default();
        a.npc_kills = k.npc_kills;
        a.ship_kills = k.ship_kills;
        a.pod_kills = k.pod_kills;
    }
    for j in jumps {
        stats.entry(j.system_id).or_default().jumps = j.ship_jumps;
    }
    Ok(stats)
}

pub fn fetch_stats(client: &reqwest::blocking::Client) -> Result<Stats, String> {
    let get = |url: &str| -> Result<String, String> {
        let r = client.get(url).send().map_err(|e| format!("ESI: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("ESI answered {}", r.status()));
        }
        r.text().map_err(|e| format!("ESI: {e}"))
    };
    parse_stats(&get(KILLS_URL)?, &get(JUMPS_URL)?)
}

// ---------------------------------------------------------------------------------------------
// Hunting spots

/// A system reachable from a connection, with its activity over the last hour.
#[derive(Clone, Debug)]
pub struct Spot {
    pub id: u64,
    pub name: String,
    pub region: String,
    pub sec: f32,
    pub space: Space,
    pub distance: u32,
    pub act: Activity,
}

#[derive(Clone, Debug, Default)]
pub struct Hunt {
    /// Systems with NPC kills, by NPC kills (descending).
    pub spots: Vec<Spot>,
    /// Systems reachable within range (including those without activity).
    pub reachable: usize,
    pub total_npc: u32,
}

impl Hunt {
    pub fn best(&self) -> Option<&Spot> {
        self.spots.first()
    }
}

/// Which security the hunting spots are taken from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpotSecurity {
    Null,
    #[default]
    NullLow,
    All,
}

impl SpotSecurity {
    /// In the order the UI lists them.
    pub const ALL: [SpotSecurity; 3] = [SpotSecurity::Null, SpotSecurity::NullLow, SpotSecurity::All];

    pub fn label(self) -> &'static str {
        match self {
            SpotSecurity::Null => "Null",
            SpotSecurity::NullLow => "Null + Low",
            SpotSecurity::All => "All",
        }
    }

    pub fn allows(self, space: Space) -> bool {
        match self {
            SpotSecurity::Null => space == Space::Null,
            SpotSecurity::NullLow => matches!(space, Space::Null | Space::Low),
            SpotSecurity::All => true,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&x| x == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> SpotSecurity {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// Systems within `range` jumps of `from`, of the security asked, with NPC kills in the last hour.
pub fn hunt(u: &Universe, stats: &Stats, from: u64, range: u32, security: SpotSecurity) -> Hunt {
    let around = u.within(from, range);
    let mut spots: Vec<Spot> = around
        .iter()
        .filter_map(|&(id, distance)| {
            let sys = u.system(id)?;
            let act = stats.get(&id).copied().unwrap_or_default();
            let space = sys.space();
            if act.npc_kills == 0 || !security.allows(space) {
                return None;
            }
            Some(Spot { id, name: sys.name.clone(), region: sys.region.clone(), sec: sys.display_sec(), space, distance, act })
        })
        .collect();
    spots.sort_by(|a, b| b.act.npc_kills.cmp(&a.act.npc_kills).then(a.distance.cmp(&b.distance)));
    Hunt { total_npc: spots.iter().map(|s| s.act.npc_kills).sum(), reachable: around.len(), spots }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jita_has_its_neighbours() {
        let u = universe();
        let jita = 30000142;
        assert_eq!(u.system(jita).unwrap().name, "Jita");
        let near = u.within(jita, 1);
        assert_eq!(near.len(), 1 + 7, "Jita and its seven gates");
        assert!(near.iter().any(|&(id, d)| u.system(id).unwrap().name == "Perimeter" && d == 1));
    }

    #[test]
    fn range_is_respected() {
        let u = universe();
        let far = u.within(30000142, 10);
        assert!(far.iter().all(|&(_, d)| d <= 10));
        assert!(far.len() > u.within(30000142, 5).len());
    }

    #[test]
    fn display_security_rounds_like_the_game() {
        assert_eq!(display_sec(0.9459), 0.9);
        assert_eq!(display_sec(0.02), 0.1);
        assert_eq!(display_sec(-0.0823), -0.1);
    }

    #[test]
    fn spots_follow_the_security_asked() {
        let u = universe();
        let stats = parse_stats(include_str!("../fixtures/system_kills.json"), include_str!("../fixtures/system_jumps.json")).unwrap();
        // Jita: high-sec around, low-sec and null-sec within ten jumps.
        let spots = |sec| hunt(u, &stats, 30000142, 10, sec).spots;
        let (null, null_low, all) = (spots(SpotSecurity::Null), spots(SpotSecurity::NullLow), spots(SpotSecurity::All));
        assert!(null.iter().all(|s| s.space == Space::Null && s.act.npc_kills > 0));
        assert!(null_low.iter().all(|s| matches!(s.space, Space::Null | Space::Low)));
        assert!(null_low.iter().any(|s| s.space == Space::Low), "low-sec around Jita");
        assert!(all.iter().any(|s| s.space == Space::High), "high-sec around Jita");
        assert!(null.len() <= null_low.len() && null_low.len() <= all.len());
        assert!(all.windows(2).all(|w| w[0].act.npc_kills >= w[1].act.npc_kills));
    }

    #[test]
    fn spot_security_indexes_round_trip() {
        for sec in SpotSecurity::ALL {
            assert_eq!(SpotSecurity::from_index(sec.index()), sec);
        }
        assert_eq!(SpotSecurity::from_index(99), SpotSecurity::NullLow);
    }
}
