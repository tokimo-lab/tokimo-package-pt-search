use regex::Regex;
use scraper::{ElementRef, Selector};

use crate::pt_search::config::{FieldConfig, FieldMap, Filter};

/// Extract a single field value from an HTML element using a `FieldConfig`.
pub fn extract_field(row: &ElementRef, config: &FieldConfig) -> Option<String> {
    // Case matching: check CSS selectors, return mapped value
    if !config.case.is_empty() {
        for &(sel_str, val) in &config.case {
            if sel_str == "*" {
                return Some(val.to_string());
            }
            if let Ok(sel) = Selector::parse(sel_str)
                && row.select(&sel).next().is_some()
            {
                return Some(val.to_string());
            }
        }
        return None;
    }

    // Text template (no selector needed)
    if let Some(tpl) = config.text {
        return Some(tpl.to_string());
    }

    // Selector-based extraction
    let Some(sel_str) = config.selector else {
        return config.default_value.map(std::string::ToString::to_string);
    };

    let Ok(sel) = Selector::parse(sel_str) else {
        return config.default_value.map(std::string::ToString::to_string);
    };

    let Some(el) = row.select(&sel).next() else {
        return config.default_value.map(std::string::ToString::to_string);
    };

    let value = if config.method == Some("next_sibling") {
        // Get text of next sibling node
        get_next_sibling_text(&el)
    } else if let Some(attr) = config.attribute {
        el.value().attr(attr).unwrap_or("").to_string()
    } else {
        element_text(&el)
    };

    let value = if value.is_empty() {
        match config.default_value {
            Some(d) => d.to_string(),
            None => return None,
        }
    } else {
        value
    };

    let value = apply_filters(&value, &config.filters);

    if value.is_empty() {
        config.default_value.map(std::string::ToString::to_string)
    } else {
        Some(value)
    }
}

/// Extract all raw fields from a torrent row, then resolve templates
pub fn extract_all_fields(row: &ElementRef, field_map: &FieldMap) -> std::collections::HashMap<String, String> {
    let mut raw: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    // Phase 1: extract all non-template fields
    let configs = build_field_list(field_map);
    for (name, config) in &configs {
        if is_template_referencing_fields(config) {
            continue;
        }
        if let Some(val) = extract_field(row, config) {
            raw.insert(name.to_string(), val);
        }
    }

    // Phase 2: resolve template fields that reference other fields
    for (name, config) in &configs {
        if !is_template_referencing_fields(config) {
            continue;
        }
        if let Some(tpl) = config.text {
            let resolved = resolve_field_template(tpl, &raw);
            let resolved = apply_filters(&resolved, &config.filters);
            raw.insert(name.to_string(), resolved);
        }
    }

    raw
}

fn is_template_referencing_fields(config: &FieldConfig) -> bool {
    match config.text {
        Some(t) => t.contains("fields["),
        None => false,
    }
}

fn build_field_list(fm: &FieldMap) -> Vec<(&'static str, &FieldConfig)> {
    let mut list: Vec<(&str, &FieldConfig)> = vec![
        ("id", &fm.id),
        ("title_default", &fm.title_default),
        ("title_optional", &fm.title_optional),
        ("title", &fm.title),
        ("download", &fm.download),
        ("size", &fm.size),
        ("seeders", &fm.seeders),
        ("leechers", &fm.leechers),
        ("downloadvolumefactor", &fm.downloadvolumefactor),
        ("uploadvolumefactor", &fm.uploadvolumefactor),
    ];

    if let Some(ref c) = fm.description {
        list.push(("description", c));
    }
    if let Some(ref c) = fm.category {
        list.push(("category", c));
    }
    if let Some(ref c) = fm.details {
        list.push(("details", c));
    }
    if let Some(ref c) = fm.date_elapsed {
        list.push(("date_elapsed", c));
    }
    if let Some(ref c) = fm.date_added {
        list.push(("date_added", c));
    }
    if let Some(ref c) = fm.date {
        list.push(("date", c));
    }
    if let Some(ref c) = fm.grabs {
        list.push(("grabs", c));
    }
    if let Some(ref c) = fm.free_deadline {
        list.push(("free_deadline", c));
    }
    if let Some(ref c) = fm.imdbid {
        list.push(("imdbid", c));
    }

    for &(name, ref config) in &fm.extra {
        list.push((name, config));
    }

    list
}

/// Apply a chain of filters to a string value
pub fn apply_filters(value: &str, filters: &[Filter]) -> String {
    let mut result = value.to_string();
    for filter in filters {
        match filter {
            Filter::ReSearch(pattern, group) => {
                if let Ok(re) = Regex::new(pattern) {
                    if let Some(caps) = re.captures(&result) {
                        result = caps.get(*group).map(|m| m.as_str().to_string()).unwrap_or_default();
                    } else {
                        result.clear();
                    }
                }
            }
            Filter::Replace(search, replacement) => {
                result = result.replace(search, replacement);
            }
            Filter::QueryString(param) => {
                // Parse "?cat=123" or "cat=123" style strings
                let qs = if result.starts_with('?') || result.starts_with("http") {
                    result.clone()
                } else {
                    format!("?{result}")
                };
                if let Ok(url) = url::Url::parse(&format!("http://dummy{qs}")) {
                    result = url
                        .query_pairs()
                        .find(|(k, _)| k == param)
                        .map(|(_, v)| v.to_string())
                        .unwrap_or_default();
                } else {
                    result.clear();
                }
            }
            Filter::Split(sep, idx) => {
                let parts: Vec<&str> = result.split(sep).collect();
                result = parts.get(*idx).unwrap_or(&"").trim().to_string();
            }
            Filter::DateParse(_) => {
                // Pass through: we keep the string as-is
            }
        }
    }
    result
}

/// Resolve Jinja2-like field templates: `{{ fields['title_optional'] }}`,
/// `{% if fields['x'] %}...{% else %}...{% endif %}`
pub fn resolve_field_template<S: ::std::hash::BuildHasher>(
    template: &str,
    fields: &std::collections::HashMap<String, String, S>,
) -> String {
    let mut result = template.to_string();

    // Handle {% if fields['a'] or fields['b'] %}...{% else %}...{% endif %}
    let re_if_or = Regex::new(
        r"\{%\s*if\s+fields\['(\w+)'\]\s+or\s+fields\['(\w+)'\]\s*%\}(.*?)(?:\{%\s*else\s*%\}(.*?))?\{%\s*endif\s*%\}",
    )
    .unwrap();
    result = re_if_or
        .replace_all(&result, |caps: &regex::Captures| {
            let k1 = &caps[1];
            let k2 = &caps[2];
            let if_body = &caps[3];
            let else_body = caps.get(4).map_or("", |m| m.as_str());
            let has_k1 = fields.get(k1).is_some_and(|v| !v.is_empty());
            let has_k2 = fields.get(k2).is_some_and(|v| !v.is_empty());
            if has_k1 || has_k2 {
                resolve_field_template(if_body, fields)
            } else {
                resolve_field_template(else_body, fields)
            }
        })
        .to_string();

    // Handle {% if fields['x'] %}...{% else %}...{% endif %}
    let re_if =
        Regex::new(r"\{%\s*if\s+fields\['(\w+)'\]\s*%\}(.*?)(?:\{%\s*else\s*%\}(.*?))?\{%\s*endif\s*%\}").unwrap();
    result = re_if
        .replace_all(&result, |caps: &regex::Captures| {
            let key = &caps[1];
            let if_body = &caps[2];
            let else_body = caps.get(3).map_or("", |m| m.as_str());
            let has_val = fields.get(key).is_some_and(|v| !v.is_empty());
            if has_val {
                resolve_field_template(if_body, fields)
            } else {
                resolve_field_template(else_body, fields)
            }
        })
        .to_string();

    // Handle {% if fields['downloadvolumefactor']==0 %}...{% endif %}
    let re_if_eq = Regex::new(r"\{%\s*if\s+fields\['(\w+)'\]==(\d+)\s*%\}(.*?)\{%\s*endif\s*%\}").unwrap();
    result = re_if_eq
        .replace_all(&result, |caps: &regex::Captures| {
            let key = &caps[1];
            let expected = &caps[2];
            let body = &caps[3];
            let actual = fields.get(key).map_or("", std::string::String::as_str);
            if actual == expected {
                resolve_field_template(body, fields)
            } else {
                String::new()
            }
        })
        .to_string();

    // Handle {{ fields['x'] if fields['y'] else fields['z'] }}
    let re_ternary =
        Regex::new(r"\{\{\s*fields\['(\w+)'\]\s+if\s+fields\['(\w+)'\]\s+else\s+fields\['(\w+)'\]\s*\}\}").unwrap();
    result = re_ternary
        .replace_all(&result, |caps: &regex::Captures| {
            let val_key = &caps[1];
            let cond_key = &caps[2];
            let else_key = &caps[3];
            if fields.get(cond_key).is_some_and(|v| !v.is_empty()) {
                fields.get(val_key).cloned().unwrap_or_default()
            } else {
                fields.get(else_key).cloned().unwrap_or_default()
            }
        })
        .to_string();

    // Handle {{ fields['x'] }} simple substitution
    let re_simple = Regex::new(r"\{\{\s*fields\['(\w+)'\]\s*\}\}").unwrap();
    result = re_simple
        .replace_all(&result, |caps: &regex::Captures| {
            let key = &caps[1];
            fields.get(key).cloned().unwrap_or_default()
        })
        .to_string();

    // Handle {{ fields['x']+' '+fields['y']|join(' ') }} – simplified concat
    let re_concat =
        Regex::new(r"\{\{\s*fields\['(\w+)'\]\s*\+\s*'([^']*)'\s*\+\s*fields\['(\w+)'\](?:\|join\('[^']*'\))?\s*\}\}")
            .unwrap();
    result = re_concat
        .replace_all(&result, |caps: &regex::Captures| {
            let k1 = &caps[1];
            let sep = &caps[2];
            let k2 = &caps[3];
            format!(
                "{}{}{}",
                fields.get(k1).cloned().unwrap_or_default(),
                sep,
                fields.get(k2).cloned().unwrap_or_default()
            )
        })
        .to_string();

    // Handle {{max_time}} placeholder
    result = result.replace("{{max_time}}", "9999-12-31 23:59:59");

    // Clean up remaining Jinja2 artifacts
    let re_jinja_block = Regex::new(r"\{%.*?%\}").unwrap();
    result = re_jinja_block.replace_all(&result, "").to_string();
    let re_jinja_var = Regex::new(r"\{\{.*?\}\}").unwrap();
    result = re_jinja_var.replace_all(&result, "").to_string();

    result.trim().to_string()
}

/// Resolve search query template variables
pub fn resolve_search_template(template: &str, keyword: &str) -> String {
    let mut result = template.to_string();

    result = result.replace("{{query.keyword}}", keyword);
    result = result.replace("{{ query.keyword }}", keyword);

    // {% if query.imdb_id %}...{% else %}...{% endif %} → take else branch
    let re_imdb = Regex::new(r"\{%\s*if\s+query\.imdb_id\s*%\}.*?\{%\s*else\s*%\}(.*?)\{%\s*endif\s*%\}").unwrap();
    result = re_imdb.replace_all(&result, "$1").to_string();

    // {% if query.free %}...{% else %}...{% endif %} → take else branch
    let re_free = Regex::new(r"\{%\s*if\s+query\.free\s*%\}.*?\{%\s*else\s*%\}(.*?)\{%\s*endif\s*%\}").unwrap();
    result = re_free.replace_all(&result, "$1").to_string();

    // Remove remaining template variables
    let re_query_var = Regex::new(r"\{\{\s*query\.\w+\s*\}\}").unwrap();
    result = re_query_var.replace_all(&result, "").to_string();

    let re_jinja = Regex::new(r"\{%.*?%\}").unwrap();
    result = re_jinja.replace_all(&result, "").to_string();

    result.trim().to_string()
}

/// Get text content of all descendants of an element
fn element_text(el: &ElementRef) -> String {
    el.text().collect::<Vec<_>>().join("").trim().to_string()
}

/// Get the text of the next sibling text node
fn get_next_sibling_text(el: &ElementRef) -> String {
    use scraper::node::Node;
    if let Some(sibling) = el.next_sibling()
        && let Node::Text(text) = sibling.value()
    {
        return text.trim().to_string();
    }
    String::new()
}
