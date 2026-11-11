//! Client for the public EVE-Scout API (https://api.eve-scout.com/v2/public/signatures).
//! A single request returns every known Thera and Turnur connection.

use chrono::{DateTime, Utc};
use serde::Deserialize;

pub const SIGNATURES_URL: &str = "https://api.eve-scout.com/v2/public/signatures";

/// A connection (wormhole signature) between a hub (Thera / Turnur) and another system.
#[derive(Clone, Debug, Deserialize)]
pub struct Signature {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub wh_type: String,
    /// small, medium, large, xlarge, capital
    #[serde(default)]
    pub max_ship_size: String,
    /// Le hub : Thera ou Turnur.
    pub out_system_name: String,
    #[serde(default)]
    pub out_signature: String,
    pub in_system_id: u64,
    pub in_system_name: String,
    /// hs, ls, ns, c1..c6, c13, …
    #[serde(default)]
    pub in_system_class: String,
    #[serde(default)]
    pub in_region_name: String,
    #[serde(default)]
    pub in_signature: String,
}

impl Signature {
    pub fn hub(&self) -> Hub {
        if self.out_system_name.eq_ignore_ascii_case("turnur") { Hub::Turnur } else { Hub::Thera }
    }

    pub fn space(&self) -> Space {
        match self.in_system_class.as_str() {
            "hs" => Space::High,
            "ls" => Space::Low,
            "ns" => Space::Null,
            _ => Space::Wormhole,
        }
    }

    /// Classe lisible : HS, LS, NS, C3, …
    pub fn class_label(&self) -> String {
        self.in_system_class.to_uppercase()
    }

    pub fn size_rank(&self) -> u8 {
        size_rank(&self.max_ship_size)
    }

    pub fn size_label(&self) -> &'static str {
        match self.max_ship_size.as_str() {
            "small" => "Small",
            "medium" => "Medium",
            "large" => "Large",
            "xlarge" => "XL",
            "capital" => "Capital",
            _ => "?",
        }
    }
}

pub fn size_rank(size: &str) -> u8 {
    match size {
        "small" => 0,
        "medium" => 1,
        "large" => 2,
        "xlarge" => 3,
        "capital" => 4,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hub {
    Thera,
    Turnur,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    High,
    Low,
    Null,
    Wormhole,
}

pub fn parse(json: &str) -> Result<Vec<Signature>, String> {
    serde_json::from_str(json).map_err(|e| format!("Unreadable response: {e}"))
}

/// Blocking request, called from the refresh thread.
pub fn fetch(client: &reqwest::blocking::Client) -> Result<Vec<Signature>, String> {
    let resp = client.get(SIGNATURES_URL).send().map_err(|e| format!("Network: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("EVE-Scout answered {status}"));
    }
    let body = resp.text().map_err(|e| format!("Read: {e}"))?;
    parse(&body)
}

pub fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("eve-scouting/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("client HTTP")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_fixture() {
        let sigs = parse(include_str!("../fixtures/signatures.json")).unwrap();
        assert!(!sigs.is_empty());
        assert!(sigs.iter().any(|s| s.hub() == Hub::Thera));
    }
}
