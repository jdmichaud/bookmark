#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use clap::{Parser, Subcommand};
use reqwest;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::io::prelude::*;
use std::path::PathBuf;
use users::{get_current_uid, get_user_by_uid};

pub const DEFAULT_CONFIG: &str = include_str!("../config.yaml");
pub const USER_AGENT_STRING: &str =
  "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// A bookmark manager
#[derive(Debug, Parser)]
#[command(
  name = format!("{} ({} {})", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), env!("GIT_HASH")),
  version = "",
  max_term_width = 100,
)]
struct Opt {
  /// YAML configuration file to use.
  /// If not provided, use the file ~/.config/bookmark/config.yaml.
  /// If the file does not exists, use default embedded config (see --print-config)
  #[arg(short, long, value_name = "FILE", verbatim_doc_comment)]
  config: Option<PathBuf>,
  /// Print the used configuration and exit.
  /// You can use this option to initialize the default config file with:
  ///   mkdir -p ~/.config/bookmark/
  ///   bookmark --print-config ~/.config/bookmark/config.yaml
  #[arg(long, verbatim_doc_comment)]
  print_config: bool,
  /// Override the configured bookmark file
  #[arg(short, long, value_name = "FILE")]
  bookmarks: Option<String>,

  #[command(subcommand)]
  command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
  /// Adds a bookmark
  Add { url: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Metadata {
  #[serde(skip_serializing_if = "Option::is_none")]
  posted: Option<NaiveDateTime>,
  #[serde(skip_serializing_if = "Option::is_none")]
  user: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  referer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Bookmark {
  href: String,
  meta: Metadata,
  title: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChromiumConfig {
  enabled: bool,
  path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
  bookmarks: PathBuf,
  chromium: Option<ChromiumConfig>,
}

// Writes bookmarks to a file.
fn write_bookmarks(bookmarks: &Vec<Bookmark>, output_file: &PathBuf) -> Result<()> {
  let file = std::fs::File::create(output_file)?;
  let mut writer = std::io::BufWriter::new(file);
  serde_json::to_writer(&mut writer, &bookmarks)?;
  Ok(())
}

// Takes a bookmarks list and returns the list without duplicate href.
fn dedup(bookmarks: &[Bookmark], output_file: &PathBuf) -> Result<Vec<Bookmark>> {
  let mut new_bookmarks: Vec<Bookmark> = Vec::with_capacity(bookmarks.len());
  for bookmark in bookmarks {
    if let None = new_bookmarks.iter().find(|b| b.href == bookmark.href) {
      new_bookmarks.push(bookmark.clone());
    }
  }
  // Let sort the bookmarks by date while we're at it
  new_bookmarks.sort_by(|a, b| if a.meta.posted.is_none() || b.meta.posted.is_none() {
    a.href.cmp(&b.href)
  } else {
    a.meta.posted.unwrap().cmp(&b.meta.posted.unwrap())
  });
  write_bookmarks(&new_bookmarks, output_file)?;
  Ok(new_bookmarks)
}

// Fetches a URL with a fake user agent.
// Returns the page content.
fn fetch_http(config: &Config, url: &str) -> Result<String> {
  if chromium_available(config) {
    fetch_by_chromium(url)
  } else {
    let client = reqwest::blocking::Client::new();
    // Creating a client with a standard USER_AGENT because some site do not accetp "reqwests".
    let response = client
      .get(url)
      .header(reqwest::header::USER_AGENT, USER_AGENT_STRING)
      .send()?;
    // Check for status
    let response = match response.error_for_status() {
      Ok(response) => response,
      Err(e) => Err(e)?,
    };
    Ok(response.text()?)
  }
}

// Adds a bookmark based on a URL
// The function will treat hacker news stories differently as it will consider
// them as referer and the article pointer to as the original submission.
fn add(config: &Config, bookmarks: &mut Vec<Bookmark>, url: &str, output_file: &PathBuf) -> Result<()> {
  fn get_hn_article(config: &Config, url: &str) -> Result<(String, scraper::Html)> {
    let body = fetch_http(config, url)?;
    let hn_document = Html::parse_document(&body);
    let selector = Selector::parse(r#".titleline > a"#).unwrap();
    if let Some(title_line_element) = hn_document.select(&selector).next() {
      if let Some(article_url) = title_line_element.value().attr("href") {
        let article_body = fetch_http(config, article_url)?;
        Ok((article_url.to_string(), Html::parse_document(&article_body)))
      } else {
        Err(anyhow::anyhow!(
          "could not retrieve the article link from the hacker news post"
        ))
      }
    } else {
      Err(anyhow::anyhow!(
        "could not get the article title from the hacker news post"
      ))
    }
  }

  // Check the url is not already present
  if let Some(result) = bookmarks.iter().find(|b| b.href == *url) {
    eprint!(
      "warning: this url is already present in bookmarks: {}",
      result.title
    );
    if let Some(date) = result.meta.posted {
      eprint!(" added the {}", date);
    }
    eprintln!("");
    Ok(())
  } else {
    print!("fetching {}... ", url);
    let _ = std::io::stdout().flush();
    // If the url if from an hacker new post, fetch the original article
    let is_hacker_news = url.contains("news.ycombinator.com/item?id=");
    let (article_url, document) = if is_hacker_news {
      get_hn_article(config, url)?
    } else {
      let body = fetch_http(config, url)?;
      (url.to_string(), Html::parse_document(&body))
    };
    let selector = Selector::parse(r#"title"#).unwrap();
    // Get the title
    if let Some(title_element) = document.select(&selector).next() {
      if let Some(title) = title_element.text().next() {
        let user = get_user_by_uid(get_current_uid()).unwrap();
        // Create the new bookmark and add it to the list
        bookmarks.push(Bookmark {
          href: article_url,
          title: title.to_string(),
          meta: Metadata {
            posted: Some(chrono::offset::Utc::now().naive_utc()),
            user: Some(user.name().to_string_lossy().to_string()),
            referer: if is_hacker_news {
              Some(url.to_string())
            } else {
              None
            },
          },
        });
        // Write the bookmark file
        write_bookmarks(bookmarks, output_file)?;
        println!("\radded {}", title);
        Ok(())
      } else {
        Err(anyhow::anyhow!(
          "could not retrieve text from title in the html page"
        ))
      }
    } else {
      Err(anyhow::anyhow!(
        "could not retrieve title from the html page"
      ))
    }
  }
}

fn get_data_folder() -> Result<PathBuf> {
  let default_config_data_path: String =
    env::var("XDG_DATA_HOME").unwrap_or(env::var("HOME")? + "/.local/share") + "/bookmark/";
  std::fs::create_dir_all(&default_config_data_path)?;
  let path = std::path::PathBuf::from(&default_config_data_path);
  return Ok(path);
}

fn get_state_folder() -> Result<PathBuf> {
  let default_config_state_path: String =
    env::var("XDG_STATE_HOME").unwrap_or(env::var("HOME")? + "/.local/state") + "/bookmark/";
  std::fs::create_dir_all(&default_config_state_path)?;
  let path = std::path::PathBuf::from(&default_config_state_path);
  return Ok(path);
}

struct UrlStore {
  data_folder: PathBuf,
}

impl UrlStore {
  fn new() -> Result<Self> {
    Ok(UrlStore {
      data_folder: get_data_folder()?,
    })
  }

  fn get_hash(key: &str) -> String {
    use sha1::{Sha1, Digest};
    use base64ct::{Base64, Encoding};

    let mut hasher = Sha1::new();
    hasher.update(key);
    let hash = hasher.finalize();
    Base64::encode_string(&hash)
  }

  // pub fn get_url(url: &str) -> Result<String> {
  //   let content = fetch_http(url)?;
  //   let hash = get_hash(url);
  // }
}

// Checks if chromium is available in headless mode with the dump-dom option.
// It takes a while to launch chrome so the info is then recorded is a state file.
// TODO: This whole state machine in buggy and does not take into changes in
// configuration or in the chromium installation. Need to rewrite it at some point.
fn chromium_available(config: &Config) -> bool {
  // First check if chromium is enabled in the config and if yes, is a path is
  // provided
  let chromium_path = if let Some(chromium_config) = &config.chromium {
    if !chromium_config.enabled { return false; }
    if let Some(chromium_path) = &chromium_config.path {
      &chromium_path
    } else {
      "chromium"
    }
  } else {
    // Chromium is not enabled
    return false;
  };
  // If chromium is enabled, check if we haven't already recorded its state
  if let Ok(mut state_folder) = get_state_folder() {
    state_folder.push(std::path::PathBuf::from("chromium_available"));
    if let Ok(state_file) = std::fs::File::open(state_folder) {
      let mut reader = std::io::BufReader::new(state_file);
      let mut line = String::new();
      let _ = reader.read_line(&mut line);
      if line.len() > 0 {
        return line.chars().nth(0).unwrap() == '1';
      }
    }
  }

  let available = match std::process::Command::new(chromium_path)
    .args(["--headless", "--dump-dom", "www.google.com"])
    .output() {
    Ok(output) => if output.status.success() { true } else { false },
    Err(_) => false,
  };
  // Recorde the chromium state in a file
  if let Ok(mut state_folder) = get_state_folder() {
    state_folder.push("chromium_available");
    if let Ok(mut state_file) = std::fs::File::create(state_folder) {
      let _ = state_file.write_all(if available { b"1\n" } else { b"0\n" });
    }
  }
  return available;

}

// Fetches the page through chromium so that javascript can be interpreted if needed.
// Returns the resulting HTML content.
// ⚠️ This relies on undocumented chromium features
fn fetch_by_chromium(url: &str) -> Result<String> {
  let output = std::process::Command::new("chromium")
    .args(["--headless", "--dump-dom", url])
    .output()
    .expect("could not spawn chromium");
  Ok(String::from_utf8(output.stdout)?)
}

fn main() -> Result<()> {
  let default_config_file_path: String =
    env::var("XDG_CONFIG_HOME").unwrap_or(env::var("HOME")? + "/.config/") + "/bookmark/config.yaml";

  let opt = Opt::parse();
  // Load the config. We first check if a config file was provided as an option
  let config_file = if let Some(config_file) = opt.config {
    // Try to load it
    match std::fs::read_to_string(&config_file) {
      Err(e) => {
        eprintln!("error: {e}: {}", config_file.display());
        std::process::exit(1);
      }
      Ok(config) => config,
    }
  } else {
    // Otherwise, try the standard path
    if std::fs::metadata(std::path::Path::new(&default_config_file_path)).is_ok() {
      // Try to load it
      match std::fs::read_to_string(&default_config_file_path) {
        Err(e) => {
          eprintln!("error: {e}: {}", default_config_file_path);
          std::process::exit(1);
        }
        Ok(config) => config,
      }
    } else {
      // Otherwise, just use the embedded config file
      DEFAULT_CONFIG.to_string()
    }
  };

  if opt.print_config {
    // Print the content of the configuration file and exit
    println!("{}", config_file);
    return Ok(());
  }

  let mut config: Config = serde_yaml::from_str(&config_file)?;
  if let Some(bookmarks) = opt.bookmarks {
    config.bookmarks = std::path::PathBuf::from(&bookmarks);
  }

  // Load the bookmark files
  let mut bookmarks: Vec<Bookmark> = {
    let inputfile = match std::fs::File::open(&config.bookmarks) {
      Ok(inputfile) => inputfile,
      Err(e) => {
        eprintln!("{}: {}", config.bookmarks.display(), e);
        std::process::exit(1);
      }
    };
    match serde_json::from_reader(std::io::BufReader::new(inputfile)) {
      Ok(json) => json,
      Err(e) => {
        eprintln!(
          "{}: could not parse json file: {}",
          config.bookmarks.display(),
          e
        );
        std::process::exit(1);
      }
    }
  };
  // Everytime bookmark runs, it remove duplicates
  let new_bookmarks = dedup(&bookmarks[..], &config.bookmarks)?;
  if new_bookmarks.len() < bookmarks.len() {
    println!("deduped {} entries", bookmarks.len() - new_bookmarks.len());
    bookmarks = new_bookmarks;
  }

  match &opt.command {
    Some(Commands::Add { url }) => add(&config, &mut bookmarks, &url, &config.bookmarks)?,
    None => {
      // By default, just lists the bookmarks
      for i in 0..bookmarks.len() {
        println!("{} {} ({})", i + 1, bookmarks[i].title, bookmarks[i].href);
      }
    }
  }

  Ok(())
}
