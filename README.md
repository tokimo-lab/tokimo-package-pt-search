# tokimo-pt-search

PT site torrent search and user info scraping library for the Tokimo ecosystem.

## Features

- **Torrent Search**: Search 28+ Private Tracker sites by keyword
  - NexusPhp-based sites (22): hdfans, hdsky, audiences, azusa, btschool, chdbits, hddolby, HDHome, hhan, keepfrds, ourbits, pterclub, ptsbao, putao, ssd, ttg, ultrahd, exoticaz, filelist, iptorrents, hares, hdatmos, agsv, tjupt
  - API-based sites (1): M-Team
  - Public sites (3): acgrip, mikanani, sukebei
- **User Info**: Fetch account metadata (uploaded/downloaded, ratio, seeding/leeching) from PT sites
- **Multiple backends**: HTML scraping (NexusPhp), JSON API, and public site support
- **Adult content**: Optional adult content search toggle

## Usage

```rust
use tokimo_pt_search::{search_site, fetch_user_info, PtSiteInput, SiteAuth};

// Search a PT site
let results = search_site(&client, "hdsky", "keyword", "https://hdsky.me", auth, false).await;

// Fetch user info (M-Team only)
let info = fetch_user_info(&PtSiteInput { ... }).await;
```

## Dependencies

- `reqwest` — HTTP client
- `scraper` — HTML parsing
- `serde` / `serde_json` — serialization
- `regex` — pattern matching
- `url` — URL parsing
- `percent-encoding` — URL encoding
