//! Open-Meteo weather client
//!
//! Free weather API with no API key required.
//! Used as fallback when Apple WeatherKit is not configured.
//! API docs: https://open-meteo.com/en/docs

use anyhow::{anyhow, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::apple_weather::{
    assess_day_weather, convert_temp, filter_days_by_range, geocode_city, DayWeather,
    QueryType, TemperatureUnit,
};

// ============================================================================
// Open-Meteo API Response Structures
// ============================================================================

#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    current: Option<OpenMeteoCurrent>,
    daily: Option<OpenMeteoDaily>,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoCurrent {
    temperature_2m: f64,
    apparent_temperature: f64,
    relative_humidity_2m: f64,
    wind_speed_10m: f64,
    weather_code: i32,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoDaily {
    time: Vec<String>,
    weather_code: Vec<i32>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_probability_max: Vec<f64>,
}

// ============================================================================
// WMO Weather Code Mapping
// ============================================================================

/// Map WMO weather interpretation codes to human-readable descriptions
fn format_wmo_code(code: i32) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "foggy",
        51 => "light drizzle",
        53 => "moderate drizzle",
        55 => "dense drizzle",
        56 | 57 => "freezing drizzle",
        61 => "slight rain",
        63 => "moderate rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 => "slight snow",
        73 => "moderate snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 => "slight rain showers",
        81 => "moderate rain showers",
        82 => "violent rain showers",
        85 => "slight snow showers",
        86 => "heavy snow showers",
        95 => "thunderstorms",
        96 | 99 => "thunderstorms with hail",
        _ => "unknown",
    }
}

/// Map WMO code to an Apple-style condition code for assess_day_weather compatibility
fn wmo_to_apple_condition(code: i32) -> &'static str {
    match code {
        0 => "Clear",
        1 => "MostlyClear",
        2 => "PartlyCloudy",
        3 => "Cloudy",
        45 | 48 => "Foggy",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "FreezingRain",
        61 | 80 => "Rain",
        63 | 81 => "Rain",
        65 | 82 => "HeavyRain",
        66 | 67 => "FreezingRain",
        71 | 85 => "Snow",
        73 => "Snow",
        75 | 86 => "HeavySnow",
        77 => "Flurries",
        95 => "Thunderstorms",
        96 | 99 => "Thunderstorms",
        _ => "Clear",
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Execute weather query using Open-Meteo API.
/// Same signature and output format as `execute_apple_weather`.
pub fn execute_open_meteo_weather(
    location: &str,
    date: Option<&str>,
    date_end: Option<&str>,
    query_type: QueryType,
) -> Result<String> {
    let (lat, lon) = geocode_city(location)?;

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?\
         latitude={}&longitude={}&\
         current=temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code&\
         daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&\
         timezone=auto",
        lat, lon
    );

    let response = client.get(&url).send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(anyhow!("Open-Meteo API error {}: {}", status, body));
    }

    let data: OpenMeteoResponse = serde_json::from_str(&body)?;
    let unit = TemperatureUnit::from_env();
    let suffix = unit.suffix();

    let result = match query_type {
        QueryType::Current => {
            let current = data
                .current
                .ok_or_else(|| anyhow!("No current weather data from Open-Meteo"))?;

            let temp = convert_temp(current.temperature_2m, unit);
            let feels_like = convert_temp(current.apparent_temperature, unit);

            serde_json::json!({
                "location": location,
                "query_type": "current",
                "source": "open-meteo",
                "current": {
                    "temperature": format!("{:.1}{}", temp, suffix),
                    "feels_like": format!("{:.1}{}", feels_like, suffix),
                    "condition": format_wmo_code(current.weather_code),
                    "humidity": format!("{:.0}%", current.relative_humidity_2m),
                    "wind_speed": format!("{:.1} km/h", current.wind_speed_10m),
                }
            })
        }

        QueryType::Forecast => {
            let daily = data
                .daily
                .ok_or_else(|| anyhow!("No forecast data from Open-Meteo"))?;

            // Convert Open-Meteo daily arrays to DayWeather for filter_days_by_range
            let days = to_day_weather_vec(&daily);
            let filtered = filter_days_by_range(&days, date, date_end);

            if filtered.is_empty() {
                return Err(anyhow!("No forecast data for specified date range"));
            }

            let forecast_days: Vec<serde_json::Value> = filtered
                .iter()
                .map(|day| {
                    let date_str = day.forecast_start.split('T').next().unwrap_or(&day.forecast_start);
                    let high = convert_temp(day.temperature_max, unit);
                    let low = convert_temp(day.temperature_min, unit);
                    serde_json::json!({
                        "date": date_str,
                        "condition": format_wmo_code(wmo_code_from_condition(&day.condition_code)),
                        "high": format!("{:.0}{}", high, suffix),
                        "low": format!("{:.0}{}", low, suffix),
                        "precipitation_chance": format!("{:.0}%", day.precipitation_chance * 100.0)
                    })
                })
                .collect();

            serde_json::json!({
                "location": location,
                "query_type": "forecast",
                "source": "open-meteo",
                "forecast": forecast_days
            })
        }

        QueryType::Assessment => {
            let daily = data
                .daily
                .ok_or_else(|| anyhow!("No forecast data from Open-Meteo"))?;

            let days = to_day_weather_vec(&daily);
            let filtered = filter_days_by_range(&days, date, date_end);

            if filtered.is_empty() {
                return Err(anyhow!("No forecast data for specified date range"));
            }

            let assessments: Vec<_> = filtered
                .iter()
                .map(|d| assess_day_weather(d, unit))
                .collect();

            let bad_days: Vec<serde_json::Value> = assessments
                .iter()
                .filter(|a| a.is_bad)
                .map(|a| {
                    serde_json::json!({
                        "date": a.date,
                        "reasons": a.reasons
                    })
                })
                .collect();

            let has_bad_weather = !bad_days.is_empty();

            serde_json::json!({
                "location": location,
                "query_type": "assessment",
                "source": "open-meteo",
                "has_bad_weather": has_bad_weather,
                "bad_days": bad_days,
                "days_checked": assessments.len()
            })
        }
    };

    serde_json::to_string_pretty(&result).map_err(|e| anyhow!("Failed to serialize result: {}", e))
}

/// Convert Open-Meteo daily arrays into Vec<DayWeather> for reuse with filter/assess functions
fn to_day_weather_vec(daily: &OpenMeteoDaily) -> Vec<DayWeather> {
    daily
        .time
        .iter()
        .enumerate()
        .map(|(i, date)| DayWeather {
            forecast_start: format!("{}T00:00:00Z", date),
            condition_code: wmo_to_apple_condition(daily.weather_code[i]).to_string(),
            temperature_max: daily.temperature_2m_max[i],
            temperature_min: daily.temperature_2m_min[i],
            precipitation_chance: daily.precipitation_probability_max[i] / 100.0,
        })
        .collect()
}

/// Reverse-map Apple condition code string back to a representative WMO code for display.
/// Only used within this module for formatting — not a general-purpose mapping.
fn wmo_code_from_condition(condition: &str) -> i32 {
    match condition {
        "Clear" => 0,
        "MostlyClear" => 1,
        "PartlyCloudy" => 2,
        "Cloudy" => 3,
        "Foggy" => 45,
        "Drizzle" => 51,
        "Rain" => 61,
        "HeavyRain" => 65,
        "Snow" => 71,
        "HeavySnow" => 75,
        "Flurries" => 77,
        "FreezingRain" => 66,
        "Thunderstorms" => 95,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_wmo_code() {
        assert_eq!(format_wmo_code(0), "clear");
        assert_eq!(format_wmo_code(2), "partly cloudy");
        assert_eq!(format_wmo_code(61), "slight rain");
        assert_eq!(format_wmo_code(95), "thunderstorms");
        assert_eq!(format_wmo_code(999), "unknown");
    }

    #[test]
    fn test_wmo_to_apple_condition() {
        assert_eq!(wmo_to_apple_condition(0), "Clear");
        assert_eq!(wmo_to_apple_condition(65), "HeavyRain");
        assert_eq!(wmo_to_apple_condition(95), "Thunderstorms");
    }

    #[test]
    fn test_to_day_weather_vec() {
        let daily = OpenMeteoDaily {
            time: vec!["2026-02-27".to_string(), "2026-02-28".to_string()],
            weather_code: vec![0, 61],
            temperature_2m_max: vec![15.0, 10.0],
            temperature_2m_min: vec![5.0, 2.0],
            precipitation_probability_max: vec![10.0, 80.0],
        };

        let days = to_day_weather_vec(&daily);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].condition_code, "Clear");
        assert_eq!(days[1].condition_code, "Rain");
        assert!((days[1].precipitation_chance - 0.80).abs() < 0.01);
    }

    #[test]
    fn test_open_meteo_response_deserialize() {
        let json = r#"{
            "current": {
                "temperature_2m": 12.5,
                "apparent_temperature": 10.2,
                "relative_humidity_2m": 65.0,
                "wind_speed_10m": 15.3,
                "weather_code": 2
            }
        }"#;
        let resp: OpenMeteoResponse = serde_json::from_str(json).unwrap();
        let current = resp.current.unwrap();
        assert!((current.temperature_2m - 12.5).abs() < 0.01);
        assert_eq!(current.weather_code, 2);
    }
}
