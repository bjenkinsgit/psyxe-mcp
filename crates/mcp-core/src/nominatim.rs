//! Nominatim geocoding client (OpenStreetMap)
//!
//! Free geocoding API with no API key required.
//! Usage policy: max 1 request/sec, requires User-Agent header.
//! Used as fallback when Apple Maps is not configured.

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct NominatimResult {
    lat: String,
    lon: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct NominatimSearchResult {
    lat: String,
    lon: String,
    display_name: String,
    #[serde(rename = "type")]
    result_type: String,
    #[serde(default)]
    address: Option<serde_json::Value>,
}

/// Geocode a location string to (latitude, longitude) using Nominatim.
pub fn geocode(query: &str) -> Result<(f64, f64)> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&limit=1",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .header("User-Agent", "prolog-router/0.1")
        .send()
        .context("Nominatim geocode request failed")?;

    let status = response.status();
    let body = response.text().context("Failed to read Nominatim response")?;

    if !status.is_success() {
        return Err(anyhow!("Nominatim API error {}: {}", status, body));
    }

    let results: Vec<NominatimResult> =
        serde_json::from_str(&body).context("Failed to parse Nominatim response")?;

    let first = results
        .first()
        .ok_or_else(|| anyhow!("No geocoding results for '{}'", query))?;

    let lat: f64 = first.lat.parse().context("Invalid latitude from Nominatim")?;
    let lon: f64 = first.lon.parse().context("Invalid longitude from Nominatim")?;

    Ok((lat, lon))
}

/// Search for POIs near coordinates using a viewbox.
fn search_nearby_coords(query: &str, lat: f64, lon: f64, limit: u32) -> Result<Vec<NominatimSearchResult>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    // ±0.1° ≈ ~11km bounding box
    let viewbox = format!("{},{},{},{}", lon - 0.1, lat + 0.1, lon + 0.1, lat - 0.1);

    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&limit={}&viewbox={}&bounded=1&addressdetails=1",
        urlencoding::encode(query),
        limit,
        urlencoding::encode(&viewbox),
    );

    let response = client
        .get(&url)
        .header("User-Agent", "prolog-router/0.1")
        .send()
        .context("Nominatim search request failed")?;

    let status = response.status();
    let body = response.text().context("Failed to read Nominatim search response")?;

    if !status.is_success() {
        return Err(anyhow!("Nominatim API error {}: {}", status, body));
    }

    let results: Vec<NominatimSearchResult> =
        serde_json::from_str(&body).context("Failed to parse Nominatim search response")?;

    Ok(results)
}

/// Search for POIs near a location string. Geocodes first, then searches.
/// Returns JSON with the shared search_nearby output format.
pub fn search_pois(query: &str, near: &str, limit: u32) -> Result<String> {
    // Geocode the location using the existing fallback chain
    let (lat, lon) = crate::apple_weather::geocode_city(near)?;

    let results = search_nearby_coords(query, lat, lon, limit)?;

    let poi_results: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            // Build a short address from address components if available
            let address = if let Some(addr) = &r.address {
                let parts: Vec<&str> = [
                    addr.get("road").and_then(|v| v.as_str()),
                    addr.get("house_number").and_then(|v| v.as_str()),
                    addr.get("city").or_else(|| addr.get("town")).or_else(|| addr.get("village")).and_then(|v| v.as_str()),
                    addr.get("state").and_then(|v| v.as_str()),
                ]
                .into_iter()
                .flatten()
                .collect();
                if parts.is_empty() {
                    r.display_name.clone()
                } else {
                    parts.join(", ")
                }
            } else {
                r.display_name.clone()
            };

            // Extract name: use the part before the first comma in display_name
            let name = r.display_name.split(',').next().unwrap_or(&r.display_name).trim();

            serde_json::json!({
                "name": name,
                "address": address,
                "latitude": r.lat.parse::<f64>().unwrap_or(0.0),
                "longitude": r.lon.parse::<f64>().unwrap_or(0.0),
            })
        })
        .collect();

    let output = serde_json::json!({
        "location": near,
        "query": query,
        "source": "nominatim",
        "results": poi_results,
        "count": poi_results.len(),
    });

    Ok(serde_json::to_string_pretty(&output)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nominatim_result_deserialize() {
        let json = r#"[{"lat":"40.7127281","lon":"-74.0060152"}]"#;
        let results: Vec<NominatimResult> = serde_json::from_str(json).unwrap();
        assert_eq!(results.len(), 1);
        let lat: f64 = results[0].lat.parse().unwrap();
        assert!((lat - 40.71).abs() < 0.01);
    }

    #[test]
    fn test_empty_results() {
        let json = "[]";
        let results: Vec<NominatimResult> = serde_json::from_str(json).unwrap();
        assert!(results.is_empty());
    }
}
