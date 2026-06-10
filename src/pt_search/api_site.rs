use serde_json::Value;
use tracing::{debug, warn};

use crate::pt_search::config::{ApiSearchConfig, SiteConfig, format_size};
use crate::pt_search::{PtSearchResult, SiteAuth};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

/// Search an API-based PT site (e.g. M-Team)
pub async fn search(
    client: &reqwest::Client,
    keyword: &str,
    domain: &str,
    auth: &SiteAuth,
    config: &SiteConfig,
) -> Vec<PtSearchResult> {
    let crate::pt_search::config::SearchConfig::Api(api_config) = &config.search else {
        return Vec::new();
    };

    match search_api(client, keyword, domain, auth, config, api_config).await {
        Ok(results) => results,
        Err(e) => {
            warn!(site_id = config.site_id, "API search error: {}", e);
            Vec::new()
        }
    }
}

async fn search_api(
    client: &reqwest::Client,
    keyword: &str,
    domain: &str,
    auth: &SiteAuth,
    config: &SiteConfig,
    api_config: &ApiSearchConfig,
) -> Result<Vec<PtSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
    let url = build_url(domain, api_config.path);
    let mut req_builder = match api_config.method {
        "GET" => client.get(&url),
        _ => client.post(&url),
    };

    req_builder = req_builder
        .header("User-Agent", USER_AGENT)
        .header("Content-Type", "application/json");

    if let Some(ref cookies) = auth.cookies {
        req_builder = req_builder.header("Cookie", cookies);
    }

    // Apply headers with template resolution
    for &(key, value_tpl) in &api_config.headers {
        let resolved = resolve_auth_template(value_tpl, auth);
        req_builder = req_builder.header(key, &resolved);
    }

    // Build request body with keyword substitution
    let body_str = api_config.body_template.replace("{{keyword}}", keyword);
    let body: Value = serde_json::from_str(&body_str).unwrap_or(Value::Object(serde_json::Map::default()));

    if api_config.method != "GET" {
        req_builder = req_builder.json(&body);
    }

    debug!(site_id = config.site_id, url = %url, "API search request");

    let resp = req_builder.send().await?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()).into());
    }

    let json: Value = resp.json().await?;

    // Check for API error codes
    if let Some(code) = json.get("code") {
        let code_str = match code {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        if !code_str.is_empty() && code_str != "0" && code_str != "SUCCESS" {
            let msg = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            warn!(site_id = config.site_id, "API error: {}", msg);
            return Ok(Vec::new());
        }
    }

    // Navigate to data
    let mut data = &json;
    if let Some(data_path) = api_config.data_path {
        data = get_nested_value(data, data_path);
    }

    // Navigate to list
    let mut list = data;
    if let Some(list_path) = api_config.list_path {
        list = get_nested_value(list, list_path);
    }

    let Some(items) = list.as_array() else {
        warn!(site_id = config.site_id, "API response is not an array");
        return Ok(Vec::new());
    };

    let results: Vec<PtSearchResult> = items
        .iter()
        .filter_map(|item| map_api_result(item, &api_config.fields, domain))
        .collect();

    debug!(site_id = config.site_id, count = results.len(), "API search completed");
    Ok(results)
}

fn map_api_result(
    item: &Value,
    fields: &crate::pt_search::config::ApiFieldMap,
    _domain: &str,
) -> Option<PtSearchResult> {
    let get_str = |path: &str| -> String {
        let val = get_nested_value(item, path);
        match val {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            _ => val.to_string(),
        }
    };

    let get_opt = |path: Option<&str>| -> Option<String> { path.map(&get_str).filter(|s| !s.is_empty()) };

    let id = get_str(fields.id);
    let title = get_str(fields.title);

    if id.is_empty() && title.is_empty() {
        return None;
    }

    let size_raw = get_str(fields.size);
    let size_bytes: Option<u64> = size_raw.parse().ok();
    let size = format_size(&size_raw);

    let seeders = get_str(fields.seeders).parse::<i32>().unwrap_or(0);
    let leechers = get_str(fields.leechers).parse::<i32>().unwrap_or(0);
    let grabs = fields.grabs.map(|p| get_str(p).parse::<i32>().unwrap_or(0));

    Some(PtSearchResult {
        id,
        title,
        subtitle: get_opt(fields.subtitle),
        size,
        size_bytes,
        seeders,
        leechers,
        grabs,
        category: get_opt(fields.category).unwrap_or_default(),
        upload_time: get_opt(fields.upload_time).unwrap_or_default(),
        download_url: get_opt(fields.detail_url).unwrap_or_default(),
        detail_url: get_opt(fields.detail_url).unwrap_or_default(),
        poster_url: get_opt(fields.poster_url),
        imdb_url: get_opt(fields.imdb_url),
        imdb_rating: get_opt(fields.imdb_rating),
        douban_url: get_opt(fields.douban_url),
        douban_rating: get_opt(fields.douban_rating),
        discount: get_opt(fields.discount),
        discount_end_time: get_opt(fields.discount_end_time),
        video_codec: get_opt(fields.video_codec),
        audio_codec: get_opt(fields.audio_codec),
        resolution: get_opt(fields.resolution),
        source: get_opt(fields.source),
    })
}

fn get_nested_value<'a>(obj: &'a Value, path: &str) -> &'a Value {
    let mut current = obj;
    for part in path.split('.') {
        match current {
            Value::Object(map) => {
                current = map.get(part).unwrap_or(&Value::Null);
            }
            Value::Array(arr) => {
                if let Ok(idx) = part.parse::<usize>() {
                    current = arr.get(idx).unwrap_or(&Value::Null);
                } else {
                    return &Value::Null;
                }
            }
            _ => return &Value::Null,
        }
    }
    current
}

fn resolve_auth_template(template: &str, auth: &SiteAuth) -> String {
    let mut result = template.to_string();
    if let Some(ref key) = auth.api_key {
        result = result.replace("{{api_key}}", key);
    }
    if let Some(ref cookies) = auth.cookies {
        result = result.replace("{{cookies}}", cookies);
    }
    result
}

fn build_url(domain: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let base = domain.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}
