use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use scraper::{Html, Selector};
use tracing::{debug, warn};

fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

use crate::pt_search::config::{HtmlSearchConfig, SiteConfig, factors_to_discount, format_size};
use crate::pt_search::parser::{extract_all_fields, resolve_search_template};
use crate::pt_search::{PtSearchResult, SiteAuth};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

/// Search a NexusPhp/Public site by HTML scraping
pub async fn search(
    client: &reqwest::Client,
    keyword: &str,
    domain: &str,
    auth: &SiteAuth,
    config: &SiteConfig,
) -> Vec<PtSearchResult> {
    let crate::pt_search::config::SearchConfig::Html(search_config) = &config.search else {
        return Vec::new();
    };

    match search_html(client, keyword, domain, auth, config, search_config).await {
        Ok(results) => results,
        Err(e) => {
            warn!(site_id = config.site_id, "HTML search error: {}", e);
            Vec::new()
        }
    }
}

async fn search_html(
    client: &reqwest::Client,
    keyword: &str,
    domain: &str,
    auth: &SiteAuth,
    config: &SiteConfig,
    search_config: &HtmlSearchConfig,
) -> Result<Vec<PtSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
    let base = build_url(domain, search_config.path);

    // Build query parameters
    let mut query_parts: Vec<(String, String)> = Vec::new();
    for &(key, value_tpl) in &search_config.query {
        if key == "$raw" {
            continue;
        }
        let resolved = resolve_search_template(value_tpl, keyword);
        if !resolved.is_empty() {
            query_parts.push((key.to_string(), resolved));
        }
    }

    let url = if query_parts.is_empty() {
        base
    } else {
        let qs: String = query_parts
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{base}?{qs}")
    };

    debug!(site_id = config.site_id, url = %url, "Searching PT site");

    let mut req = client.get(&url).header("User-Agent", USER_AGENT);
    if let Some(ref cookies) = auth.cookies {
        req = req.header("Cookie", cookies);
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()).into());
    }

    let html_text = resp.text().await?;
    let document = Html::parse_document(&html_text);

    let row_sel =
        Selector::parse(config.row_selector).map_err(|_| format!("Invalid row selector: {}", config.row_selector))?;

    let mut results = Vec::new();
    for row_ref in document.select(&row_sel) {
        if let Some(result) = parse_row(&row_ref, domain, config)
            && (!result.id.is_empty() || !result.title.is_empty())
        {
            results.push(result);
        }
    }

    debug!(site_id = config.site_id, count = results.len(), "Search completed");
    Ok(results)
}

fn parse_row(row: &scraper::ElementRef, domain: &str, config: &SiteConfig) -> Option<PtSearchResult> {
    let raw = extract_all_fields(row, &config.fields);

    let id = raw.get("id").cloned().unwrap_or_default();
    let title = raw.get("title").cloned().unwrap_or_default();

    if id.is_empty() && title.is_empty() {
        return None;
    }

    let subtitle = raw
        .get("description")
        .or_else(|| raw.get("subject"))
        .filter(|s| !s.is_empty())
        .cloned();

    let size_raw = raw.get("size").cloned().unwrap_or_else(|| "0".into());
    let size = format_size(&size_raw);

    let seeders = raw
        .get("seeders")
        .and_then(|s| s.trim().replace(',', "").parse::<i32>().ok())
        .unwrap_or(0);
    let leechers = raw
        .get("leechers")
        .and_then(|s| s.trim().replace(',', "").parse::<i32>().ok())
        .unwrap_or(0);
    let grabs = raw
        .get("grabs")
        .and_then(|s| s.trim().replace(',', "").parse::<i32>().ok());

    let category = raw.get("category").cloned().unwrap_or_default();
    let upload_time = raw
        .get("date")
        .or_else(|| raw.get("date_elapsed"))
        .or_else(|| raw.get("date_added"))
        .cloned()
        .unwrap_or_default();

    let download_url = make_absolute(domain, raw.get("download").map_or("", std::string::String::as_str));
    let detail_url = make_absolute(
        domain,
        raw.get("details")
            .or_else(|| raw.get("id"))
            .map_or("", std::string::String::as_str),
    );

    let dl_factor: f64 = raw
        .get("downloadvolumefactor")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let ul_factor: f64 = raw
        .get("uploadvolumefactor")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let discount = factors_to_discount(dl_factor, ul_factor);

    let imdb_id = raw.get("imdbid").filter(|s| !s.is_empty());
    let imdb_url = imdb_id.map(|id| {
        if id.starts_with("http") {
            id.clone()
        } else {
            format!("https://www.imdb.com/title/{id}/")
        }
    });

    let free_deadline = raw.get("free_deadline").filter(|s| !s.is_empty()).cloned();

    Some(PtSearchResult {
        id,
        title,
        subtitle,
        size,
        size_bytes: None,
        seeders,
        leechers,
        grabs,
        category,
        upload_time,
        download_url,
        detail_url,
        poster_url: None,
        imdb_url,
        imdb_rating: None,
        douban_url: None,
        douban_rating: None,
        discount,
        discount_end_time: free_deadline,
        video_codec: None,
        audio_codec: None,
        resolution: None,
        source: None,
    })
}

fn build_url(domain: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let base = domain.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn make_absolute(domain: &str, path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    build_url(domain, path)
}
