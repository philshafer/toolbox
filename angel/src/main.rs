//! angel: a household readability web proxy.
//!
//! `/read/<url>` fetches a page, pre-cleans it, runs readability (or
//! builds a headlines list for index pages), sanitizes the result, and
//! serves it with every link rewritten back through `/read/` and every
//! image through `/img/`.  `/article/<url>` and `/headlines/<url>`
//! force one rendering.

mod cache;
mod charset;
mod clean;
mod embedded;
mod extract;
mod fetch;
mod image;
mod index;
mod pages;
mod render;
mod rewrite;
mod sanitize;
mod server;
mod sitelist;
mod sites;
mod stats;
mod target;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

pub const DEFAULT_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:144.0) Gecko/20100101 Firefox/144.0";

#[derive(Debug, Clone)]
pub struct Opts {
    pub listen: SocketAddr,
    pub local_only: bool,
    pub threads: usize,
    pub user_agent: String,
    pub allow_private: bool,
    /// --render FILE: render a saved page to stdout and exit.
    pub render: Option<String>,
    /// --url URL: the page's original URL, for --render.
    pub url: Option<String>,
    /// --mode: rendering for --render.
    pub mode: render::Mode,
    /// --explain: print the rendering decision for --render.
    pub explain: bool,
    /// --bench N: time N renders of the --render file.
    pub bench: usize,
    /// --animated: what /img/ does with animated images.
    pub animated: image::Animated,
    /// --cache-mb, --image-cache-mb: cache sizes.
    pub cache_mb: usize,
    pub image_cache_mb: usize,
    /// --cache-min: shortest time a page or image is kept, in minutes.
    pub cache_min: u64,
    /// --list: the site list file.
    pub list: PathBuf,
    /// --sites: the per-site settings file.
    pub sites: PathBuf,
    /// --cookie-jar: where to keep the shared cookie jar.
    pub cookie_jar: Option<PathBuf>,
    /// --stats-size: requests kept for /stats.
    pub stats_size: usize,
    /// --log: JSON-lines request log.
    pub log: Option<PathBuf>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            listen: "127.0.0.1:8080".parse().unwrap(),
            local_only: false,
            threads: 8,
            user_agent: DEFAULT_UA.to_string(),
            allow_private: false,
            render: None,
            url: None,
            mode: render::Mode::Auto,
            explain: false,
            bench: 0,
            animated: image::Animated::Drop,
            cache_mb: 64,
            image_cache_mb: 128,
            cache_min: 60,
            list: PathBuf::from("data/angel.list"),
            sites: PathBuf::from("data/angel-sites.toml"),
            cookie_jar: None,
            stats_size: 1000,
            log: None,
        }
    }
}

const USAGE: &str = "\
usage: angel [options]
  --listen ADDR:PORT    address to listen on (default 127.0.0.1:8080)
  --local-only          refuse clients that are not on loopback
  --threads N           worker threads (default 8)
  --list PATH           site list file (default data/angel.list)
  --sites PATH          per-site settings (default data/angel-sites.toml)
  --cookie-jar PATH     keep the shared cookie jar in PATH (default: memory)
  --stats-size N        requests kept for /stats (default 1000)
  --log PATH            append a JSON line per request to PATH
  --user-agent STR      User-Agent sent upstream
  --allow-private       allow fetching private/loopback addresses
  --animated MODE       animated images: drop (default) or first-frame
  --cache-mb N          rendered-page cache size (default 64, 0 = off)
  --image-cache-mb N    image cache size (default 128, 0 = off)
  --cache-min N         keep pages and images at least N minutes
                        (default 60); the bar's reload link refetches
  --render FILE         render a saved HTML file to stdout and exit
  --url URL             original URL of the --render file
  --mode MODE           --render as auto (default), article or headlines
  --explain             --render prints its decision to stderr
  --bench N             --render N times and print timings instead
  --help                show this message
";

fn parse_opts() -> Result<Opts, lexopt::Error> {
    use lexopt::prelude::*;

    let mut o = Opts::default();
    let mut p = lexopt::Parser::from_env();
    while let Some(arg) = p.next()? {
        match arg {
            Long("listen") => o.listen = p.value()?.parse()?,
            Long("local-only") => o.local_only = true,
            Long("threads") => o.threads = p.value()?.parse::<usize>()?.max(1),
            Long("user-agent") => o.user_agent = p.value()?.string()?,
            Long("allow-private") => o.allow_private = true,
            Long("render") => o.render = Some(p.value()?.string()?),
            Long("url") => o.url = Some(p.value()?.string()?),
            Long("explain") => o.explain = true,
            Long("bench") => o.bench = p.value()?.parse()?,
            Long("cache-mb") => o.cache_mb = p.value()?.parse()?,
            Long("image-cache-mb") => o.image_cache_mb = p.value()?.parse()?,
            Long("cache-min") => o.cache_min = p.value()?.parse()?,
            Long("list") => o.list = PathBuf::from(p.value()?),
            Long("sites") => o.sites = PathBuf::from(p.value()?),
            Long("cookie-jar") => o.cookie_jar = Some(PathBuf::from(p.value()?)),
            Long("stats-size") => o.stats_size = p.value()?.parse()?,
            Long("log") => o.log = Some(PathBuf::from(p.value()?)),
            Long("mode") => {
                o.mode = match p.value()?.string()?.as_str() {
                    "auto" => render::Mode::Auto,
                    "article" => render::Mode::Article,
                    "headlines" => render::Mode::Headlines,
                    m => return Err(format!("--mode: unknown mode '{m}'").into()),
                }
            }
            Long("animated") => {
                o.animated = match p.value()?.string()?.as_str() {
                    "drop" => image::Animated::Drop,
                    "first-frame" => image::Animated::FirstFrame,
                    m => return Err(format!("--animated: unknown mode '{m}'").into()),
                }
            }
            Long("help") | Short('h') => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    let opts = match parse_opts() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("angel: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    if let Some(file) = &opts.render {
        return render::render_file(file, opts.url.as_deref(), opts.mode, opts.explain, opts.bench);
    }

    match server::run(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("angel: {e}");
            ExitCode::FAILURE
        }
    }
}
