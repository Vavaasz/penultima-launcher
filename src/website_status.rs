use anyhow::{Context, Result};
use chrono::{Local, NaiveDateTime};
use futures_util::future::join_all;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::constants::{
    BATTLE_PASS_URL, CHANGELOG_URL, EVENT_CALENDAR_URL, HTTP_REQUEST_TIMEOUT, INVESTMENT_URL,
    PACK_WEEK_URL, WEBSITE_BASE_URL,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebsiteStatus {
    pub online_players: Option<u32>,
    pub boosted_creature: Option<String>,
    pub boosted_creature_image_url: Option<String>,
    pub boosted_boss: Option<String>,
    pub boosted_boss_image_url: Option<String>,
    pub active_events: Vec<EventSummary>,
    pub upcoming_events: Vec<EventSummary>,
    pub battle_pass: Option<OfferSummary>,
    pub pack_week: Option<OfferSummary>,
    pub investor: Option<InvestorSummary>,
    pub changelogs: Vec<ChangelogEntry>,
    pub fetched_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub name: String,
    pub window: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferSummary {
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Vec<(String, String)>,
    pub previews: Vec<OfferPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferPreview {
    pub title: String,
    pub url: String,
    pub tile_size: f32,
    pub display_size: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvestorSummary {
    pub name: String,
    pub invested: String,
    pub daily_return: String,
    pub remaining: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub kind: String,
    pub area: String,
    pub date: String,
    pub body: String,
}

const WEBSITE_STATUS_CACHE_FILE: &str = "website-status-cache.json";

pub fn load_cached_status(state_path: &Path) -> Option<WebsiteStatus> {
    let cache_path = state_path.join(WEBSITE_STATUS_CACHE_FILE);
    let raw = fs::read_to_string(cache_path).ok()?;
    let mut status = serde_json::from_str::<WebsiteStatus>(&raw).ok()?;
    status.error = None;
    Some(status)
}

pub fn save_cached_status(state_path: &Path, status: &WebsiteStatus) -> Result<()> {
    fs::create_dir_all(state_path)
        .with_context(|| format!("failed to create {}", state_path.display()))?;
    let mut cached = status.clone();
    cached.error = None;
    let raw = serde_json::to_string(&cached).context("failed to serialize website status cache")?;
    fs::write(state_path.join(WEBSITE_STATUS_CACHE_FILE), raw)
        .context("failed to write website status cache")?;
    Ok(())
}

pub async fn fetch_website_status() -> Result<WebsiteStatus> {
    let client = Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .user_agent("PenultimaLauncher/website-status")
        .build()
        .context("failed to build website client")?;

    let urls = [
        WEBSITE_BASE_URL,
        EVENT_CALENDAR_URL,
        BATTLE_PASS_URL,
        PACK_WEEK_URL,
        INVESTMENT_URL,
        CHANGELOG_URL,
    ];
    let results = join_all(urls.iter().map(|url| fetch_text(&client, url))).await;
    let mut results = results.into_iter();

    let home = results
        .next()
        .expect("home result exists")
        .context("failed to fetch website home")?;
    let event_html = optional_text_or_retry(&client, EVENT_CALENDAR_URL, results.next()).await;
    let battle_pass_html = optional_text_or_retry(&client, BATTLE_PASS_URL, results.next()).await;
    let pack_week_html = optional_text_or_retry(&client, PACK_WEEK_URL, results.next()).await;
    let investment_html = optional_text_or_retry(&client, INVESTMENT_URL, results.next()).await;
    let changelog_html = optional_text_or_retry(&client, CHANGELOG_URL, results.next()).await;

    let mut status = WebsiteStatus {
        online_players: parse_online_players(&home),
        boosted_creature: parse_boosted_name(&home, "creature"),
        boosted_creature_image_url: parse_boosted_image_url(&home, "Creature"),
        boosted_boss: parse_boosted_name(&home, "boss"),
        boosted_boss_image_url: parse_boosted_image_url(&home, "Boss"),
        fetched_at: Some(Local::now().format("%Y-%m-%d %H:%M").to_string()),
        ..WebsiteStatus::default()
    };

    if let Some(html) = event_html.as_deref() {
        status.active_events = parse_event_section(html, "Active Now");
        status.upcoming_events = parse_event_section(html, "Upcoming Windows");
    }

    if let Some(html) = battle_pass_html.as_deref() {
        status.battle_pass = Some(parse_battle_pass(html));
    }

    if let Some(html) = pack_week_html.as_deref() {
        status.pack_week = Some(parse_pack_week(html));
    }

    if let Some(html) = investment_html.as_deref() {
        status.investor = parse_investor(html);
    }

    if let Some(html) = changelog_html.as_deref() {
        status.changelogs = parse_changelogs(html);
    }

    Ok(status)
}

async fn fetch_text(client: &Client, url: &str) -> Result<String> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

async fn optional_text_or_retry(
    client: &Client,
    url: &str,
    result: Option<Result<String>>,
) -> Option<String> {
    match result {
        Some(Ok(html)) => Some(html),
        Some(Err(_)) | None => fetch_text(client, url).await.ok(),
    }
}

fn parse_online_players(html: &str) -> Option<u32> {
    capture_first(html, r"(?i)([0-9][0-9.,]*)\s+Players Online").and_then(|value| {
        value
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .ok()
    })
}

fn parse_boosted_name(html: &str, kind: &str) -> Option<String> {
    let pattern = format!(
        r#"(?i)title="Today(?:'|&#039;|&apos;)s boosted {}:\s*([^"]+)""#,
        regex::escape(kind)
    );

    capture_first(html, &pattern).map(|value| title_case(&clean_text(&value)))
}

fn parse_boosted_image_url(html: &str, element_id: &str) -> Option<String> {
    let pattern = format!(
        r#"(?is)<img[^>]+id=["']{}["'][^>]+src=["']([^"']+)["']"#,
        regex::escape(element_id)
    );

    capture_first(html, &pattern).map(|url| normalize_url(&decode_html_entities(&url)))
}

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();

    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        trimmed.to_string()
    } else if trimmed.starts_with("//") {
        format!("https:{}", trimmed)
    } else if trimmed.starts_with('/') {
        format!("{}{}", WEBSITE_BASE_URL.trim_end_matches('/'), trimmed)
    } else {
        format!("{}/{}", WEBSITE_BASE_URL.trim_end_matches('/'), trimmed)
    }
}

fn parse_event_section(html: &str, heading: &str) -> Vec<EventSummary> {
    let section_pattern = format!(
        r#"(?is)<section[^>]*>\s*<h3>\s*{}\s*</h3>(.*?)</section>"#,
        regex::escape(heading)
    );

    let section = match capture_first(html, &section_pattern) {
        Some(value) => value,
        None => return Vec::new(),
    };

    let item_re = Regex::new(r#"(?is)<li>\s*<strong>(.*?)</strong>\s*<span>(.*?)</span>\s*</li>"#)
        .expect("valid event item regex");

    let events: Vec<EventSummary> = item_re
        .captures_iter(&section)
        .map(|captures| EventSummary {
            name: clean_text(&captures[1]),
            window: clean_text(&captures[2]),
        })
        .filter(|event| !event.name.is_empty())
        .take(6)
        .collect();

    if !events.is_empty() {
        return events;
    }

    let lines = visible_text_lines(&section);
    lines
        .windows(2)
        .filter_map(|pair| {
            let name = pair.first()?.trim();
            let window = pair.get(1)?.trim();
            if name.is_empty() || !window.contains('-') {
                return None;
            }

            Some(EventSummary {
                name: name.to_string(),
                window: window.to_string(),
            })
        })
        .take(3)
        .collect()
}

fn parse_battle_pass(html: &str) -> OfferSummary {
    let lines = visible_text_lines(html);
    let offer_scope = offer_scope(html, "bpass-hero", "Boosted Creature");
    let facts = wanted_facts(
        &lines,
        &[
            "Starts",
            "Launch price",
            "Estimated value",
            "Rewards",
            "Claim window",
            "Claim command",
        ],
    );

    OfferSummary {
        title: parse_page_title(html).unwrap_or_else(|| "Battle Pass".to_string()),
        subtitle: first_line_containing(&lines, "24-day reward track"),
        facts,
        previews: parse_offer_previews(&offer_scope, 8),
    }
}

fn parse_pack_week(html: &str) -> OfferSummary {
    let lines = visible_text_lines(html);
    let offer_scope = offer_scope(html, "packweek-hero", "Vanquisher Package");
    let facts = wanted_facts(
        &lines,
        &[
            "Launch price",
            "Launch window",
            "Regular price",
            "Bundle value",
        ],
    );

    OfferSummary {
        title: parse_page_title(html).unwrap_or_else(|| "Package Week".to_string()),
        subtitle: first_line_containing(&lines, "Frozen Session"),
        facts,
        previews: parse_offer_previews(&offer_scope, 6),
    }
}

fn offer_scope(html: &str, start_needle: &str, end_heading: &str) -> String {
    let start = html.find(start_needle).unwrap_or(0);
    let scoped = &html[start..];
    let end_pattern = format!(r#"(?is)<h3>\s*{}\s*</h3>"#, regex::escape(end_heading));

    if let Some(end) = Regex::new(&end_pattern)
        .ok()
        .and_then(|regex| regex.find(scoped))
    {
        scoped[..end.start()].to_string()
    } else {
        scoped.to_string()
    }
}

fn parse_offer_previews(html: &str, limit: usize) -> Vec<OfferPreview> {
    let sprite_re = Regex::new(
        r#"(?is)<span[^>]*class=["'][^"']*penultima-sprite[^"']*["'][^>]*>\s*<img\s+([^>]+)>"#,
    )
    .expect("valid offer sprite regex");

    let mut previews = Vec::new();

    for captures in sprite_re.captures_iter(html) {
        let attrs = &captures[1];
        let Some(src) = capture_attr(attrs, "src") else {
            continue;
        };

        if !src.contains("tools/sprite.php") {
            continue;
        }

        let url =
            bounded_animated_sprite_preview_url(&normalize_url(&decode_html_entities(&src)), 8);
        if previews
            .iter()
            .any(|preview: &OfferPreview| preview.url == url)
        {
            continue;
        }

        let title = capture_attr(attrs, "title")
            .or_else(|| capture_attr(attrs, "alt"))
            .map(|value| clean_text(&decode_html_entities(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Reward".to_string());

        let is_large_preview = url.contains("type=outfit") || url.contains("type=mount");
        let (tile_size, display_size) = if is_large_preview {
            (128.0, 128.0)
        } else {
            (64.0, 64.0)
        };

        previews.push(OfferPreview {
            title,
            url,
            tile_size,
            display_size,
        });

        if previews.len() >= limit {
            break;
        }
    }

    previews
}

pub(crate) fn static_sprite_preview_url(url: &str) -> String {
    sprite_preview_url_with_params(url, &[("animate", "0".to_string())])
}

pub(crate) fn bounded_animated_sprite_preview_url(url: &str, max_frames: usize) -> String {
    sprite_preview_url_with_params(
        url,
        &[
            ("animate", "1".to_string()),
            ("max_frames", max_frames.max(1).to_string()),
        ],
    )
}

fn sprite_preview_url_with_params(url: &str, replacements: &[(&str, String)]) -> String {
    if !url.contains("tools/sprite.php") {
        return url.to_string();
    }

    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut seen = Vec::new();
    let mut params = Vec::new();
    for param in query.split('&').filter(|param| !param.is_empty()) {
        let key = param.split_once('=').map(|(key, _)| key).unwrap_or(param);
        if let Some((replacement_key, replacement_value)) = replacements
            .iter()
            .find(|(replacement_key, _)| key.eq_ignore_ascii_case(replacement_key))
        {
            seen.push(*replacement_key);
            params.push(format!("{}={}", replacement_key, replacement_value));
        } else {
            params.push(param.to_string());
        }
    }

    for (replacement_key, replacement_value) in replacements {
        if !seen
            .iter()
            .any(|seen_key| seen_key.eq_ignore_ascii_case(replacement_key))
        {
            params.push(format!("{}={}", replacement_key, replacement_value));
        }
    }

    format!("{}?{}", base, params.join("&"))
}

fn capture_attr(attrs: &str, attr_name: &str) -> Option<String> {
    let pattern = format!(
        r#"(?is)\b{}\s*=\s*["']([^"']+)["']"#,
        regex::escape(attr_name)
    );

    capture_first(attrs, &pattern)
}

#[cfg(test)]
mod tests {
    use super::{bounded_animated_sprite_preview_url, static_sprite_preview_url};

    #[test]
    fn offer_sprite_previews_can_request_static_images() {
        let url = "https://ultimaotserv.online/tools/sprite.php?type=outfit&id=132&animate=1";
        assert_eq!(
            static_sprite_preview_url(url),
            "https://ultimaotserv.online/tools/sprite.php?type=outfit&id=132&animate=0"
        );
    }

    #[test]
    fn offer_sprite_previews_add_static_flag_when_missing() {
        let url = "https://ultimaotserv.online/tools/sprite.php?type=item&id=60525";
        assert_eq!(
            static_sprite_preview_url(url),
            "https://ultimaotserv.online/tools/sprite.php?type=item&id=60525&animate=0"
        );
    }

    #[test]
    fn boosted_sprite_previews_request_bounded_animations() {
        let url = "https://ultimaotserv.online/tools/sprite.php?type=item&id=60525&animate=0";
        assert_eq!(
            bounded_animated_sprite_preview_url(url, 16),
            "https://ultimaotserv.online/tools/sprite.php?type=item&id=60525&animate=1&max_frames=16"
        );
    }
}

fn parse_investor(html: &str) -> Option<InvestorSummary> {
    let top_re = Regex::new(
        r#"(?is)<td>\s*#1\s*</td>\s*<td>(.*?)</td>\s*<td>(.*?)</td>\s*<td>\s*<span[^>]*>(.*?)</span>\s*</td>\s*<td>(.*?)</td>"#,
    )
    .expect("valid investor regex");

    let captures = top_re.captures(html)?;
    let remaining = parse_cycle_remaining(html);

    Some(InvestorSummary {
        name: clean_text(&captures[1]),
        invested: clean_text(&captures[2]),
        daily_return: clean_text(&captures[3]),
        remaining,
    })
}

fn parse_cycle_remaining(html: &str) -> Option<String> {
    let cycle_re = Regex::new(
        r#"(?is)<td[^>]*>\s*<b>\s*Current cycle\s*</b>\s*</td>\s*<td>(.*?)\s+to\s+(.*?)</td>"#,
    )
    .expect("valid cycle regex");

    let captures = cycle_re.captures(html)?;
    let end_raw = clean_text(&captures[2]);
    let end = NaiveDateTime::parse_from_str(&end_raw, "%Y-%m-%d %H:%M:%S").ok()?;
    let now = Local::now().naive_local();

    if end <= now {
        return Some("next cycle pending".to_string());
    }

    let diff = end - now;
    let days = diff.num_days();
    let hours = diff.num_hours() % 24;

    Some(if days > 0 {
        format!("{}d {}h", days, hours)
    } else {
        format!("{}h", diff.num_hours().max(1))
    })
}

fn parse_changelogs(html: &str) -> Vec<ChangelogEntry> {
    let row_re = Regex::new(
        r#"(?is)<tr bgcolor="[^"]*">\s*<td>\s*<span[^>]*>\s*(?:<i[^>]*></i>\s*)?([^<]+)</span>\s*</td>\s*<td>\s*<span[^>]*>\s*(?:<i[^>]*></i>\s*)?([^<]+)</span>\s*</td>\s*<td>([^<]+)</td>\s*<td>(.*?)</td>\s*</tr>"#,
    )
    .expect("valid changelog row regex");

    row_re
        .captures_iter(html)
        .map(|captures| ChangelogEntry {
            kind: clean_text(&captures[1]),
            area: clean_text(&captures[2]),
            date: clean_text(&captures[3]),
            body: clean_text(&captures[4]),
        })
        .filter(|entry| !entry.body.is_empty())
        .take(12)
        .collect()
}

fn parse_page_title(html: &str) -> Option<String> {
    capture_first(html, r"(?is)<title>\s*(.*?)\s+-\s+Penultima\s*</title>")
        .map(|title| clean_text(&title))
}

fn wanted_facts(lines: &[String], labels: &[&str]) -> Vec<(String, String)> {
    labels
        .iter()
        .filter_map(|label| {
            value_after_label(lines, label).map(|value| ((*label).to_string(), value))
        })
        .collect()
}

fn value_after_label(lines: &[String], label: &str) -> Option<String> {
    let prefix = format!("{}:", label);

    for (index, line) in lines.iter().enumerate() {
        if line.eq_ignore_ascii_case(label) {
            return lines.get(index + 1).cloned();
        }

        if line
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            return Some(line[prefix.len()..].trim().to_string());
        }
    }

    None
}

fn first_line_containing(lines: &[String], needle: &str) -> Option<String> {
    lines
        .iter()
        .find(|line| line.contains(needle))
        .map(|line| line.to_string())
}

fn capture_first(haystack: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(haystack)
        .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
}

fn visible_text_lines(html: &str) -> Vec<String> {
    let script_re = Regex::new(r"(?is)<script[^>]*>.*?</script>").expect("valid script regex");
    let style_re = Regex::new(r"(?is)<style[^>]*>.*?</style>").expect("valid style regex");
    let break_re =
        Regex::new(r"(?i)</?(br|p|div|li|tr|td|th|h[1-6]|section|article|strong|span)[^>]*>")
            .expect("valid break regex");
    let tag_re = Regex::new(r"(?is)<[^>]+>").expect("valid tag regex");

    let mut text = script_re.replace_all(html, "\n").into_owned();
    text = style_re.replace_all(&text, "\n").into_owned();
    text = break_re.replace_all(&text, "\n").into_owned();
    text = tag_re.replace_all(&text, " ").into_owned();

    text.lines()
        .map(clean_text)
        .filter(|line| !line.is_empty())
        .collect()
}

fn clean_text(input: &str) -> String {
    let without_tags = Regex::new(r"(?is)<[^>]+>")
        .expect("valid tag cleanup regex")
        .replace_all(input, " ")
        .into_owned();

    decode_html_entities(&without_tags)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&ecirc;", "e")
}

fn title_case(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
